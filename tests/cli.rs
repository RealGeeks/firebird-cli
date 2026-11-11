//! End-to-end tests: run the `firebird-cli` binary against a mock Firebird API.

use std::error::Error;
use std::path::PathBuf;

use assert_cmd::Command;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use firebird_cli::token::{self, Cache, Token};
use mockito::{Matcher, Mock, Server, ServerGuard};
use predicates::prelude::*;
use serde_json::{json, Value};

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

    /// The config directory, one level down so the CLI has to create it.
    fn config(&self) -> PathBuf {
        self.config_dir.path().join("firebird-cli")
    }

    /// A command with the site, key and API given by environment variables.
    fn cmd(&self) -> Result<Command, Box<dyn Error>> {
        let mut cmd = self.bare()?;
        cmd.env("FIREBIRD_SITE", SITE).env("FIREBIRD_API_KEY", KEY);
        Ok(cmd)
    }

    /// A command that knows the API and config directory but no site or key.
    fn bare(&self) -> Result<Command, Box<dyn Error>> {
        let mut cmd = Command::cargo_bin("firebird-cli")?;
        cmd.env_clear()
            .env(
                "SYSTEMROOT",
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            )
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("FIREBIRD_BASE_URL", self.server.url())
            .env("FIREBIRD_CONFIG_DIR", self.config());
        Ok(cmd)
    }

    /// The key exchange, answering with `jwt`.
    fn exchange(&mut self, jwt: &str, times: usize) -> Mock {
        self.server
            .mock("POST", EXCHANGE)
            .match_header("x-api-key", KEY)
            .match_header("authorization", Matcher::Missing)
            .with_status(200)
            .with_body(json!({ "token": jwt }).to_string())
            .expect(times)
            .create()
    }

    /// Put a token in the cache as an earlier run would have.
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

/// An unsigned JWT that expires `ttl` seconds from now. `label` makes two
/// tokens differ.
fn jwt(label: &str, ttl: i64) -> String {
    let payload = json!({
        "siteUuid": "11111111-1111-4111-8111-111111111111",
        "userUuid": "22222222-2222-4222-8222-222222222222",
        "userType": 2,
        "clientUuid": "33333333-3333-4333-8333-333333333333",
        "clientRole": "integration",
        "tokenKind": "integration",
        "integrationUuid": "44444444-4444-4444-8444-444444444444",
        "label": label,
        "iat": token::now(),
        "exp": token::now() + ttl,
    });
    format!(
        "{}.{}.sig-{label}",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

fn bearer(jwt: &str) -> String {
    format!("Bearer {jwt}")
}

fn parse(out: &[u8]) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(out)?)
}

/// The JSON error document: the last line on stderr.
fn error_of(stderr: &[u8]) -> Result<Value, Box<dyn Error>> {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().last().unwrap_or_default();
    Ok(serde_json::from_str(line)?)
}

#[cfg(unix)]
fn mode(path: &std::path::Path) -> Result<u32, Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

// --- auth ---------------------------------------------------------------------

#[test]
fn login_exchanges_the_key_and_stores_it_privately() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("login", 900);
    let exchange = env.exchange(&token, 1);

    let out = env
        .bare()?
        .args([
            "auth",
            "login",
            "--site",
            "https://WWW.Test.local/",
            "--api-key-stdin",
        ])
        .write_stdin(format!("{KEY}\n"))
        .assert()
        .success()
        .stderr(predicate::str::contains("warning").not())
        .get_output()
        .stdout
        .clone();
    exchange.assert();

    let summary = parse(&out)?;
    assert_eq!(summary["site"], SITE);
    assert_eq!(summary["api_base"], env.server.url());
    assert_eq!(summary["siteUuid"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(summary["clientRole"], "integration");
    assert_eq!(summary["key_prefix"], "fb_test_abcd…");
    assert_eq!(summary["default"], true);
    let printed = String::from_utf8_lossy(&out);
    assert!(!printed.contains(KEY) && !printed.contains(&token));

    let config_path = env.config().join("config.toml");
    let config = std::fs::read_to_string(&config_path)?;
    assert!(config.contains(&format!("default_site = \"{SITE}\"")));
    assert!(config.contains(&format!("api_key = \"{KEY}\"")));
    assert!(config.contains(&format!("base_url = \"{}\"", env.server.url())));
    assert!(!config.contains(&token));
    #[cfg(unix)]
    {
        assert_eq!(mode(&env.config())?, 0o700);
        assert_eq!(mode(&config_path)?, 0o600);
        assert_eq!(mode(&env.config().join("tokens.json"))?, 0o600);
    }

    // the stored key is used from here on, with the token login obtained
    let pages = env
        .server
        .mock("GET", "/pages")
        .match_header("authorization", bearer(&token).as_str())
        .with_body("[]")
        .create();
    env.bare()?.args(["pages", "list"]).assert().success();
    pages.assert();
    exchange.assert();
    Ok(())
}

#[test]
fn a_bad_key_fails_with_exit_3_and_stores_nothing() -> TestResult {
    let mut env = Env::new()?;
    let exchange = env
        .server
        .mock("POST", EXCHANGE)
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .expect(1)
        .create();

    let assert = env
        .bare()?
        .args(["auth", "login", "--site", SITE, "--api-key-stdin"])
        .write_stdin("fb_test_wrong")
        .assert()
        .code(3);
    exchange.assert();

    let error = error_of(&assert.get_output().stderr)?;
    assert_eq!(error["error"]["code"], "unauthorized");
    assert_eq!(error["error"]["message"], "Unauthorized");
    assert_eq!(error["error"]["status"], 401);
    assert!(error["error"]["hint"]
        .as_str()
        .is_some_and(|hint| hint.contains("Integrations page")));
    assert!(!env.config().join("config.toml").exists());
    assert!(!env.config().join("tokens.json").exists());
    Ok(())
}

#[test]
fn login_needs_a_site_and_a_key() -> TestResult {
    let env = Env::new()?;
    env.bare()?
        .args(["auth", "login", "--api-key-stdin"])
        .write_stdin(KEY)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--site <domain> is required"));
    env.bare()?
        .args(["auth", "login", "--site", SITE])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--api-key-stdin"));
    env.bare()?
        .args(["auth", "login", "--site", SITE, "--api-key-stdin"])
        .write_stdin("  \n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("empty API key"));
    assert!(!env.config().join("config.toml").exists());
    Ok(())
}

#[test]
fn a_key_typed_on_the_command_line_gets_a_warning() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("typed", 900);
    env.exchange(&token, 2);
    let warning = "--api-key leaves the key in shell history";

    env.bare()?
        .args(["auth", "login", "--site", SITE, "--api-key", KEY])
        .assert()
        .success()
        .stderr(predicate::str::contains(warning))
        .stderr(predicate::str::contains(KEY).not());
    // from the environment it is not on the command line
    env.bare()?
        .env("FIREBIRD_API_KEY", KEY)
        .args(["auth", "login", "--site", SITE])
        .assert()
        .success()
        .stderr(predicate::str::contains(warning).not());
    Ok(())
}

#[test]
fn sites_use_and_logout_manage_stored_keys() -> TestResult {
    let mut env = Env::new()?;
    env.exchange(&jwt("sites", 900), 2);
    for site in ["www.a.test", "www.b.test"] {
        env.bare()?
            .args(["auth", "login", "--site", site, "--api-key-stdin"])
            .write_stdin(KEY)
            .assert()
            .success();
    }

    let out = env.bare()?.args(["auth", "sites"]).assert().success();
    let sites = parse(&out.get_output().stdout)?;
    assert_eq!(sites[0]["site"], "www.a.test");
    assert_eq!(sites[0]["default"], true);
    assert_eq!(sites[0]["prefix"], "fb_test_abcd…");
    assert_eq!(sites[1]["default"], false);
    assert!(!String::from_utf8_lossy(&out.get_output().stdout).contains(KEY));

    env.bare()?
        .args(["auth", "use", "www.b.test"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""default_site": "www.b.test""#));
    env.bare()?
        .args(["auth", "use", "www.nope.test"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no stored key for www.nope.test"));

    // logout forgets the default site's key and the token cached for it
    assert!(std::fs::read_to_string(env.config().join("tokens.json"))?.contains("sig-sites"));
    env.bare()?.args(["auth", "logout"]).assert().success();
    env.bare()?
        .args(["auth", "logout", "--site", "www.a.test"])
        .assert()
        .success();
    let config = std::fs::read_to_string(env.config().join("config.toml"))?;
    assert!(!config.contains(KEY));
    assert!(!std::fs::read_to_string(env.config().join("tokens.json"))?.contains("sig-sites"));
    env.bare()?
        .args(["auth", "logout", "--site", "www.a.test"])
        .assert()
        .code(3);
    Ok(())
}

#[test]
fn a_missing_key_exits_3_with_a_json_error() -> TestResult {
    let env = Env::new()?;
    env.bare()?
        .env("FIREBIRD_SITE", SITE)
        .args(["pages", "list"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(r#""code":"not_logged_in""#))
        .stderr(predicate::str::contains("auth login"));
    env.bare()?
        .env_remove("FIREBIRD_BASE_URL")
        .args(["pages", "list"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no site selected"));
    Ok(())
}

#[test]
fn me_prints_the_claims_decoded_from_a_new_token() -> TestResult {
    let mut env = Env::new()?;
    // a cached token is not enough: `me` proves the key still works
    env.seed_token(&jwt("cached", 900));
    let token = jwt("me", 900);
    let exchange = env.exchange(&token, 2);

    let out = env.cmd()?.arg("me").assert().success();
    let me = parse(&out.get_output().stdout)?;
    assert_eq!(me["site"], SITE);
    assert_eq!(me["api_base"], env.server.url());
    assert_eq!(me["key_prefix"], "fb_test_abcd…");
    assert_eq!(me["siteUuid"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(me["userUuid"], "22222222-2222-4222-8222-222222222222");
    assert_eq!(me["userType"], 2);
    assert_eq!(me["clientRole"], "integration");
    assert_eq!(me["tokenKind"], "integration");
    let expires = me["token_expires_at"].as_str().unwrap_or_default();
    assert!(expires.ends_with('Z') && expires.len() == 20, "{expires}");
    let printed = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(!printed.contains(KEY) && !printed.contains(&token));

    env.cmd()?
        .args(["auth", "status", "-o", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("clientRole"))
        .stdout(predicate::str::contains("integration"));
    exchange.assert();
    Ok(())
}

// --- the token ----------------------------------------------------------------

#[test]
fn the_token_is_cached_and_reused_by_the_next_process() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("cached", 900);
    let exchange = env.exchange(&token, 1);
    let pages = env
        .server
        .mock("GET", "/pages")
        .match_header("authorization", bearer(&token).as_str())
        .with_body("[]")
        .expect(3)
        .create();

    for _ in 0..3 {
        env.cmd()?.args(["pages", "list"]).assert().success();
    }
    exchange.assert();
    pages.assert();
    #[cfg(unix)]
    assert_eq!(mode(&env.config().join("tokens.json"))?, 0o600);
    Ok(())
}

#[test]
fn a_token_about_to_expire_is_replaced() -> TestResult {
    let mut env = Env::new()?;
    let old = jwt("old", 30);
    let new = jwt("new", 900);
    env.seed_token(&old);
    let exchange = env.exchange(&new, 1);
    let pages = env
        .server
        .mock("GET", "/pages")
        .match_header("authorization", bearer(&new).as_str())
        .with_body("[]")
        .expect(2)
        .create();

    env.cmd()?.args(["pages", "list"]).assert().success();
    env.cmd()?.args(["pages", "list"]).assert().success();
    exchange.assert();
    pages.assert();
    Ok(())
}

#[test]
fn a_refused_token_is_exchanged_again_once() -> TestResult {
    let mut env = Env::new()?;
    let stale = jwt("stale", 900);
    let fresh = jwt("fresh", 900);
    env.seed_token(&stale);
    let refused = env
        .server
        .mock("GET", "/pages")
        .match_header("authorization", bearer(&stale).as_str())
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .expect(1)
        .create();
    let exchange = env.exchange(&fresh, 1);
    let accepted = env
        .server
        .mock("GET", "/pages")
        .match_header("authorization", bearer(&fresh).as_str())
        .with_body(r#"[{"pageUuid":"p1","slug":"home","status":"active"}]"#)
        .expect(2)
        .create();

    env.cmd()?
        .args(["pages", "list", "-q"])
        .assert()
        .success()
        .stdout("p1\n");
    // the replacement is what the cache holds now
    env.cmd()?.args(["pages", "list"]).assert().success();
    refused.assert();
    exchange.assert();
    accepted.assert();
    Ok(())
}

#[test]
fn a_request_refused_twice_is_exit_3_after_one_more_exchange() -> TestResult {
    let mut env = Env::new()?;
    let exchange = env.exchange(&jwt("any", 900), 2);
    let refused = env
        .server
        .mock("GET", "/pages")
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .expect(2)
        .create();

    let assert = env.cmd()?.args(["pages", "list"]).assert().code(3);
    assert_eq!(
        error_of(&assert.get_output().stderr)?["error"]["code"],
        "unauthorized"
    );
    exchange.assert();
    refused.assert();
    assert!(!std::fs::read_to_string(env.config().join("tokens.json"))?.contains("sig-any"));
    Ok(())
}

// --- commands -----------------------------------------------------------------

#[test]
fn read_commands_call_their_routes() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("read", 900));
    let pages = env
        .server
        .mock("GET", "/pages")
        .with_body(
            r#"[{"pageUuid":"p1","slug":"home","status":"active"},{"pageUuid":"p2","slug":"about","status":"unpublished"}]"#,
        )
        .expect(3)
        .create();
    let content = env
        .server
        .mock("GET", "/pages/p1/content")
        .with_body(
            r#"{"initialEditorMode":{"mode":"live","pageUuid":"p1"},"blocks":[{"blockInstanceUuid":"b1","codeName":"hero","sortOrder":0,"data":{"title":"Hi"}}]}"#,
        )
        .expect(3)
        .create();
    let types = env
        .server
        .mock("GET", "/blocks")
        .match_query(Matcher::UrlEncoded("includeSchema".into(), "false".into()))
        .with_body(r#"[{"codeName":"hero","label":"Hero","category":"layout"}]"#)
        .expect(2)
        .create();
    let schema = env
        .server
        .mock("GET", "/blocks/hero")
        .with_body(r#"{"codeName":"hero","jsonSchema":{"type":"object"},"defaults":{"title":""}}"#)
        .create();

    let out = env.cmd()?.args(["pages", "list"]).assert().success();
    let list = parse(&out.get_output().stdout)?;
    assert_eq!(list[1]["slug"], "about");
    env.cmd()?
        .args(["pages", "list", "-q"])
        .assert()
        .success()
        .stdout("p1\np2\n");
    env.cmd()?
        .args(["pages", "list", "-o", "jsonl"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(r#"{"pageUuid":"p1""#));

    let out = env
        .cmd()?
        .args(["pages", "content", "p1"])
        .assert()
        .success();
    let page = parse(&out.get_output().stdout)?;
    assert_eq!(page["initialEditorMode"]["mode"], "live");
    assert_eq!(page["blocks"][0]["data"]["title"], "Hi");
    env.cmd()?
        .args(["pages", "content", "p1", "-q"])
        .assert()
        .success()
        .stdout("b1\n");
    env.cmd()?
        .args(["pages", "content", "p1", "-o", "table"])
        .assert()
        .success()
        .stderr(predicate::str::contains("editor mode: live"))
        .stdout(predicate::str::contains("hero"));

    let out = env.cmd()?.args(["blocks", "types"]).assert().success();
    assert_eq!(parse(&out.get_output().stdout)?[0]["label"], "Hero");
    env.cmd()?
        .args(["blocks", "types", "-q"])
        .assert()
        .success()
        .stdout("hero\n");

    let out = env
        .cmd()?
        .args(["blocks", "schema", "hero"])
        .assert()
        .success();
    assert_eq!(
        parse(&out.get_output().stdout)?["jsonSchema"]["type"],
        "object"
    );

    for mock in [pages, content, types, schema] {
        mock.assert();
    }
    Ok(())
}

#[test]
fn api_passes_method_path_query_and_body() -> TestResult {
    let mut env = Env::new()?;
    let token = jwt("api", 900);
    env.seed_token(&token);
    let body = json!({"data": {"title": "Welcome home", "tags": ["a", "b"]}});
    let patched = env
        .server
        .mock("PATCH", "/pages/p1/blocks/b1/data")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("dryRun".into(), "true".into()),
            Matcher::UrlEncoded("note".into(), "a b&c".into()),
        ]))
        .match_header("authorization", bearer(&token).as_str())
        .match_header("content-type", "application/json")
        .match_header("accept", "application/json")
        .match_header(
            "user-agent",
            Matcher::Regex(r"^firebird-cli/\d+\.\d+\.\d+$".into()),
        )
        .match_body(Matcher::Json(body.clone()))
        .with_body(r#"{"draftPageUuid":"d1"}"#)
        .expect(3)
        .create();
    let route = "/pages/p1/blocks/b1/data";
    let params = ["-p", "dryRun=true", "--param", "note=a b&c"];

    let out = env
        .cmd()?
        .args(["api", "patch", route, "-d", &body.to_string()])
        .args(params)
        .assert()
        .success();
    assert_eq!(parse(&out.get_output().stdout)?["draftPageUuid"], "d1");

    let file = env.file("patch.json", &body.to_string())?;
    env.cmd()?
        .args(["api", "PATCH", route, "-d"])
        .arg(format!("@{}", file.display()))
        .args(params)
        .assert()
        .success();
    env.cmd()?
        .args(["api", "PATCH", route, "-d", "-"])
        .args(params)
        .write_stdin(body.to_string())
        .assert()
        .success();
    patched.assert();

    let delete = env
        .server
        .mock("DELETE", "/pages/p1/blocks/b1")
        .match_body(Matcher::Exact(String::new()))
        .with_status(204)
        .create();
    env.cmd()?
        .args(["api", "DELETE", "pages/p1/blocks/b1"])
        .assert()
        .success()
        .stdout("{}\n");
    delete.assert();

    env.cmd()?
        .args(["api", "GET", "/pages", "-p", "novalue"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("key=value"));
    env.cmd()?
        .args(["api", "POST", "/pages", "-d", "{not json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid JSON"));
    Ok(())
}

#[test]
fn api_without_auth_sends_no_credentials() -> TestResult {
    let mut env = Env::new()?;
    let exchange = env.exchange(&jwt("unused", 900), 0);
    let health = env
        .server
        .mock("GET", "/healthz/ready")
        .match_header("authorization", Matcher::Missing)
        .match_header("x-api-key", Matcher::Missing)
        .with_body("OK")
        .expect(2)
        .create();

    env.cmd()?
        .args(["api", "GET", "/healthz/ready", "--no-auth"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""raw": "OK""#));
    // no site and no key at all: a base URL is enough for a public endpoint
    env.bare()?
        .args(["api", "GET", "/healthz/ready", "--no-auth"])
        .assert()
        .success();
    health.assert();
    exchange.assert();
    Ok(())
}

#[test]
fn error_statuses_map_onto_exit_codes() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("errors", 900));
    let cases: [(usize, &str, i32, &str); 8] = [
        (
            400,
            r#"{"error":"pageUuid must be a valid UUID"}"#,
            5,
            "validation_error",
        ),
        (403, r#"{"error":"Forbidden"}"#, 3, "forbidden"),
        (404, r#"{"error":"Page not found"}"#, 4, "not_found"),
        (
            409,
            r#"{"error":"Locked","lockedPages":[{"pageUuid":"p1"}]}"#,
            6,
            "conflict",
        ),
        (
            422,
            r#"{"error":"Already current","code":"REVISION_ALREADY_CURRENT"}"#,
            5,
            "REVISION_ALREADY_CURRENT",
        ),
        (
            422,
            r#"{"errors":[{"blockInstanceUuid":"b1"}]}"#,
            5,
            "validation_error",
        ),
        (
            500,
            r#"{"error":"An unexpected error occurred"}"#,
            1,
            "server_error",
        ),
        (502, "<html>bad gateway</html>", 1, "server_error"),
    ];
    for (index, (status, body, exit, code)) in cases.into_iter().enumerate() {
        let path = format!("/case/{index}");
        env.server
            .mock("GET", path.as_str())
            .with_status(status)
            .with_body(body)
            .create();
        let assert = env.cmd()?.args(["api", "GET", &path]).assert().code(exit);
        let error = error_of(&assert.get_output().stderr)?;
        assert_eq!(error["error"]["code"], code, "{status} {body}");
        assert_eq!(error["error"]["status"], status);
        assert_eq!(error["error"]["exit_code"], exit);
        // the whole body survives, whatever shape it had
        match serde_json::from_str::<Value>(body) {
            Ok(sent) => assert_eq!(error["error"]["body"], sent),
            Err(_) => assert_eq!(error["error"]["body"], Value::Null),
        }
    }

    // in table mode the extra fields are still shown
    env.cmd()?
        .args(["api", "GET", "/case/3", "-o", "table"])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("error: conflict: Locked"))
        .stderr(predicate::str::contains(
            r#"lockedPages: [{"pageUuid":"p1"}]"#,
        ));
    Ok(())
}

#[test]
fn a_rate_limit_is_retried_then_exit_7() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("limited", 900));
    let limited = env
        .server
        .mock("GET", "/pages")
        .with_status(429)
        .with_header("retry-after", "0")
        .with_body(r#"{"error":"Too many requests"}"#)
        .expect(3)
        .create();

    let assert = env
        .cmd()?
        .args(["pages", "list", "--max-retries", "2"])
        .assert()
        .code(7)
        .stderr(predicate::str::contains(
            "rate limited; retrying in 0s (attempt 2 of 2)",
        ));
    limited.assert();
    let error = error_of(&assert.get_output().stderr)?;
    assert_eq!(error["error"]["code"], "rate_limited");
    assert_eq!(error["error"]["retry_after"], 0);
    Ok(())
}

#[test]
fn a_rate_limit_that_clears_succeeds_and_zero_retries_gives_up() -> TestResult {
    let mut env = Env::new()?;
    env.seed_token(&jwt("clears", 900));
    let limited = env
        .server
        .mock("GET", "/pages")
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(1)
        .create();
    let ok = env
        .server
        .mock("GET", "/pages")
        .with_body(r#"[{"pageUuid":"p1"}]"#)
        .expect(1)
        .create();
    env.cmd()?
        .args(["pages", "list", "-q"])
        .assert()
        .success()
        .stdout("p1\n")
        .stderr(predicate::str::contains("attempt 1 of 3"));
    limited.assert();
    ok.assert();

    let once = env
        .server
        .mock("GET", "/blocks")
        .match_query(Matcher::Any)
        .with_status(429)
        .expect(1)
        .create();
    env.cmd()?
        .args(["blocks", "types", "--max-retries", "0"])
        .assert()
        .code(7);
    once.assert();
    Ok(())
}

// --- guide and completions ------------------------------------------------------

#[test]
fn guide_lists_topics_and_prints_one() -> TestResult {
    let env = Env::new()?;
    env.bare()?
        .args(["guide", "--list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(" 1  Setup"))
        .stdout(predicate::str::contains(
            "Drafts, publishing and what is live",
        ))
        .stdout(predicate::str::contains("Exit codes"));
    env.bare()?
        .args(["guide", "drafts"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "## 4. Drafts, publishing and what is live",
        ))
        .stdout(predicate::str::contains("## 5.").not());
    env.bare()?
        .args(["guide", "exit"])
        .assert()
        .success()
        .stdout(predicate::str::contains("| 7    | Rate limited"));
    env.bare()?
        .args(["guide", "12"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("## 12. Exit codes"));
    env.bare()?
        .arg("guide")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("# firebird-cli"))
        .stdout(predicate::str::contains("## 12. Exit codes"));
    env.bare()?
        .args(["guide", "nonsense"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no guide section matches"));
    Ok(())
}

#[test]
fn completions_are_generated() -> TestResult {
    let env = Env::new()?;
    env.bare()?
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("firebird-cli"));
    Ok(())
}
