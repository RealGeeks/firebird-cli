//! Where the key and token may go, what firebird-cli refuses to read and
//! send, and what reaches the terminal.

use std::error::Error;
use std::path::PathBuf;

use assert_cmd::Command;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use firebird_cli::token::{self, Cache, Token};
use mockito::{Matcher, Mock, Server, ServerGuard};
use predicates::prelude::*;
use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;

const KEY: &str = "fb_test_abcdEFGHijklMNOPqrstUVWXyz0123456789";
const SITE: &str = "www.test.local";
const EXCHANGE: &str = "/auth/client-integration";

struct Env {
    server: ServerGuard,
    config_dir: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Env {
    fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            server: Server::new(),
            config_dir: tempfile::tempdir()?,
            work: tempfile::tempdir()?,
        })
    }

    fn config(&self) -> PathBuf {
        self.config_dir.path().join("firebird-cli")
    }

    /// A command with the site, key and API given by environment variables.
    fn cmd(&self) -> Result<Command, Box<dyn Error>> {
        let mut cmd = self.bare()?;
        cmd.env("FIREBIRD_SITE", SITE)
            .env("FIREBIRD_API_KEY", KEY)
            .env("FIREBIRD_BASE_URL", self.server.url());
        Ok(cmd)
    }

    /// A command that knows only the config directory.
    fn bare(&self) -> Result<Command, Box<dyn Error>> {
        let mut cmd = Command::cargo_bin("firebird-cli")?;
        cmd.env_clear()
            .env(
                "SYSTEMROOT",
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            )
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("FIREBIRD_CONFIG_DIR", self.config());
        Ok(cmd)
    }

    fn exchange(&mut self, jwt: &str, times: usize) -> Mock {
        self.server
            .mock("POST", EXCHANGE)
            .match_header("x-api-key", KEY)
            .with_status(200)
            .with_body(json!({ "token": jwt }).to_string())
            .expect(times)
            .create()
    }

    fn seed_token(&self, jwt: &str) {
        let id = token::fingerprint(&self.server.url(), KEY);
        Cache::at(self.config().join("tokens.json")).put(
            &id,
            &Token::new(jwt.to_string()),
            token::now(),
        );
    }

    fn file(&self, name: &str, body: &str) -> Result<PathBuf, Box<dyn Error>> {
        let path = self.work.path().join(name);
        std::fs::write(&path, body)?;
        Ok(path)
    }
}

fn jwt(label: &str, ttl: i64) -> String {
    let payload = json!({
        "siteUuid": "11111111-1111-4111-8111-111111111111",
        "clientRole": "integration",
        "exp": token::now() + ttl,
    });
    format!(
        "{}.{}.sig-{label}",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

/// Neither secret, nor a recognisable part of one, is in what was printed.
fn assert_no_secrets(output: &std::process::Output, jwts: &[&str]) {
    for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(KEY), "the key is on {stream}: {text}");
        assert!(
            !text.contains(&KEY[12..]),
            "part of the key is on {stream}: {text}"
        );
        for jwt in jwts {
            assert!(!text.contains(jwt), "the token is on {stream}: {text}");
            for part in jwt.split('.') {
                assert!(
                    !text.contains(part),
                    "part of the token is on {stream}: {text}"
                );
            }
        }
        assert!(!text.contains("Bearer"), "a header is on {stream}: {text}");
    }
}

// --- the key and token stay out of the output -------------------------------------

#[test]
fn verbose_output_never_shows_the_key_or_token() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("verbose", 900);
    env.exchange(&token, 3);
    env.server.mock("GET", "/pages").with_body("[]").create();

    let listed = env.cmd()?.args(["-v", "pages", "list"]).assert().success();
    let output = listed.get_output();
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(log.contains(&format!("> POST {}{EXCHANGE}", env.server.url())));
    assert!(log.contains("< 200 OK"));
    assert!(log.contains(&format!("> GET {}/pages", env.server.url())));
    assert_no_secrets(output, &[&token]);

    let me = env
        .cmd()?
        .args(["-v", "me", "-o", "table"])
        .assert()
        .success();
    assert_no_secrets(me.get_output(), &[&token]);

    let login = env
        .bare()?
        .args(["-v", "auth", "login", "--site", SITE, "--api-key-stdin"])
        .args(["--base-url", &env.server.url()])
        .write_stdin(KEY)
        .assert()
        .success();
    assert_no_secrets(login.get_output(), &[&token]);
    Ok(())
}

#[test]
fn errors_never_show_the_key_or_token() -> TestResult {
    let mut env = Env::new()?;
    let stale = jwt("stale", 900);
    let fresh = jwt("fresh", 900);
    env.seed_token(&stale);
    env.exchange(&fresh, 4);
    env.server
        .mock("GET", "/pages")
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .create();
    env.server
        .mock("GET", "/blocks/hero")
        .with_status(500)
        .with_body(r#"{"error":"An unexpected error occurred"}"#)
        .create();

    for format in ["json", "table"] {
        let refused = env
            .cmd()?
            .args(["-v", "pages", "list", "-o", format])
            .assert()
            .code(3);
        assert_no_secrets(refused.get_output(), &[&stale, &fresh]);
        let failed = env
            .cmd()?
            .args(["-v", "blocks", "schema", "hero", "-o", format])
            .assert()
            .code(1);
        assert_no_secrets(failed.get_output(), &[&stale, &fresh]);
    }

    // a rejected key, and an API that cannot be reached at all
    let mut wrong = Server::new();
    wrong
        .mock("POST", EXCHANGE)
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .create();
    let rejected = env
        .cmd()?
        .env("FIREBIRD_BASE_URL", wrong.url())
        .args(["-v", "me"])
        .assert()
        .code(3);
    assert_no_secrets(rejected.get_output(), &[]);
    let unreachable = env
        .cmd()?
        .env("FIREBIRD_BASE_URL", "http://127.0.0.1:1")
        .args(["-v", "me"])
        .assert()
        .code(8)
        .stderr(predicate::str::contains(r#""code":"network""#));
    assert_no_secrets(unreachable.get_output(), &[]);
    Ok(())
}

// --- where the key and token may be sent -------------------------------------------

#[test]
fn a_plain_http_api_is_refused_unless_it_is_local() -> TestResult {
    let env = Env::new()?;
    for base in [
        "http://firebird-api.witco.net",
        "http://10.0.0.5:9006",
        "ftp://api.test",
    ] {
        env.cmd()?
            .env("FIREBIRD_BASE_URL", base)
            .args(["pages", "list"])
            .assert()
            .code(3)
            .stderr(predicate::str::contains(r#""code":"config""#))
            .stderr(predicate::str::contains(KEY).not());
        env.bare()?
            .args(["auth", "login", "--site", SITE, "--api-key-stdin"])
            .args(["--base-url", base])
            .write_stdin(KEY)
            .assert()
            .code(3);
        env.bare()?
            .args([
                "api",
                "GET",
                "/healthz/ready",
                "--no-auth",
                "--base-url",
                base,
            ])
            .assert()
            .code(3);
    }
    env.cmd()?
        .env("FIREBIRD_BASE_URL", "http://firebird-api.witco.net")
        .args(["pages", "list"])
        .assert()
        .stderr(predicate::str::contains("plain http"));
    assert!(!env.config().join("config.toml").exists());
    Ok(())
}

#[test]
fn a_redirect_to_another_origin_is_not_followed() -> TestResult {
    let mut env = Env::new()?;
    let mut elsewhere = Server::new();
    let leaked_get = elsewhere
        .mock("GET", Matcher::Any)
        .with_body("[]")
        .expect(0)
        .create();
    let leaked_post = elsewhere
        .mock("POST", Matcher::Any)
        .with_body(r#"{"token":"x.y.z"}"#)
        .expect(0)
        .create();

    // a request carrying the token
    env.seed_token(&jwt("redirect", 900));
    env.server
        .mock("GET", "/pages")
        .with_status(302)
        .with_header("location", &format!("{}/pages", elsewhere.url()))
        .create();
    env.cmd()?
        .args(["pages", "list"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("another origin"));

    // the exchange carrying the key
    env.server
        .mock("POST", EXCHANGE)
        .with_status(307)
        .with_header("location", &format!("{}{EXCHANGE}", elsewhere.url()))
        .create();
    env.cmd()?
        .arg("me")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("another origin"));

    leaked_get.assert();
    leaked_post.assert();
    Ok(())
}

#[test]
fn a_redirect_on_the_same_origin_is_followed() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("same", 900);
    env.seed_token(&token);
    env.server
        .mock("GET", "/pages")
        .with_status(308)
        .with_header("location", "/v2/pages")
        .create();
    let moved = env
        .server
        .mock("GET", "/v2/pages")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_body(r#"[{"pageUuid":"p1"}]"#)
        .create();
    env.cmd()?
        .args(["pages", "list", "-q"])
        .assert()
        .success()
        .stdout("p1\n");
    moved.assert();
    Ok(())
}

#[test]
fn a_stored_key_is_not_sent_to_another_api() -> TestResult {
    let mut env = Env::new()?;
    env.exchange(&jwt("stored", 900), 1);
    env.bare()?
        .args(["auth", "login", "--site", SITE, "--api-key-stdin"])
        .args(["--base-url", &env.server.url()])
        .write_stdin(KEY)
        .assert()
        .success();

    let mut elsewhere = Server::new();
    let leaked = elsewhere
        .mock("POST", Matcher::Any)
        .with_body(r#"{"token":"x.y.z"}"#)
        .expect(0)
        .create();
    env.bare()?
        .env("FIREBIRD_BASE_URL", elsewhere.url())
        .args(["pages", "list"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("not sending the stored key"));
    // a full URL on another origin is refused too, token and all
    env.bare()?
        .args(["api", "GET", &format!("{}/pages", elsewhere.url())])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("only URLs on"));
    leaked.assert();
    Ok(())
}

// --- local files ----------------------------------------------------------------

#[test]
fn a_hidden_body_file_is_refused_unless_allowed() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("files", 900));
    let secret = env.file(".env", r#"{"data":{"title":"AWS_SECRET=abc"}}"#)?;
    let sent = env
        .server
        .mock("PATCH", "/pages/p1/blocks/b1/data")
        .with_body("{}")
        .expect(2)
        .create();
    let patch = ["api", "PATCH", "/pages/p1/blocks/b1/data", "-d"];

    env.cmd()?
        .args(patch)
        .arg(format!("@{}", secret.display()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("it is a hidden file"))
        .stderr(predicate::str::contains("--allow-any-source"))
        .stderr(predicate::str::contains("FIREBIRD_ALLOW_ANY_SOURCE=1"))
        .stderr(predicate::str::contains("AWS_SECRET").not());

    // the override, by flag and by environment variable
    env.cmd()?
        .args(patch)
        .arg(format!("@{}", secret.display()))
        .arg("--allow-any-source")
        .assert()
        .success();
    env.cmd()?
        .env("FIREBIRD_ALLOW_ANY_SOURCE", "1")
        .args(patch)
        .arg(format!("@{}", secret.display()))
        .assert()
        .success();
    sent.assert();
    Ok(())
}

#[test]
fn its_own_config_and_token_files_are_never_a_body() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("own", 900);
    env.exchange(&token, 1);
    env.bare()?
        .args(["auth", "login", "--site", SITE, "--api-key-stdin"])
        .args(["--base-url", &env.server.url()])
        .write_stdin(KEY)
        .assert()
        .success();
    let nothing = env.server.mock("POST", "/pages").expect(0).create();

    for name in ["config.toml", "tokens.json"] {
        let own = env.config().join(name);
        assert!(own.exists(), "{name}");
        let refused = env
            .bare()?
            .args(["api", "POST", "/pages", "-d"])
            .arg(format!("@{}", own.display()))
            .assert()
            .code(2)
            .stderr(predicate::str::contains("firebird-cli's own files"));
        assert_no_secrets(refused.get_output(), &[&token]);
    }
    nothing.assert();
    Ok(())
}

// --- arguments and the terminal ---------------------------------------------------

#[test]
fn an_identifier_cannot_change_the_path() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("path", 900));
    let nothing = env.server.mock("GET", Matcher::Any).expect(0).create();
    for bad in ["../drafts/summaries", "p1/draft", "p1?x=1", "%2e%2e"] {
        env.cmd()?
            .args(["pages", "content", bad])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("the page uuid must be"));
    }
    env.cmd()?
        .args(["blocks", "schema", "hero/../../pages"])
        .assert()
        .code(2);
    nothing.assert();
    Ok(())
}

#[test]
fn terminal_escapes_from_the_api_are_stripped_from_tables_and_errors() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("escapes", 900));
    env.server
        .mock("GET", "/pages")
        .with_body(
            json!([{"pageUuid": "p1", "slug": "home\u{1b}]0;PWNED\u{7}", "status": "active"}])
                .to_string(),
        )
        .create();
    env.server
        .mock("GET", "/blocks/hero")
        .with_status(404)
        .with_body(
            json!({"error": "Block\u{1b}[2J not found", "extra\u{1b}[1m": "x\u{7}"}).to_string(),
        )
        .create();

    let escape = || predicate::str::contains("\u{1b}").or(predicate::str::contains("\u{7}"));
    env.cmd()?
        .args(["pages", "list", "-o", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("home]0;PWNED"))
        .stdout(escape().not());
    env.cmd()?
        .args(["pages", "list", "-q"])
        .assert()
        .success()
        .stdout(escape().not());
    env.cmd()?
        .args(["blocks", "schema", "hero", "-o", "table"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("not_found: Block[2J not found"))
        .stderr(escape().not());
    Ok(())
}
