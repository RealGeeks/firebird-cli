//! `firebird-cli update` against a mock of GitHub's releases API. The
//! installing tests run a copy of the binary, since a successful update
//! replaces it.

use std::error::Error;

use assert_cmd::Command;
use mockito::{Matcher, Mock, Server, ServerGuard};
use serde_json::{json, Value};

type TestResult = Result<(), Box<dyn Error>>;

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "/repos/RealGeeks/firebird-cli/releases";
const KEY: &str = "fb_test_abcdEFGHijklMNOPqrstUVWXyz0123456789";

struct Env {
    server: ServerGuard,
    config_dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            server: Server::new(),
            config_dir: tempfile::tempdir()?,
        })
    }

    fn cmd(&self) -> Result<Command, Box<dyn Error>> {
        Ok(self.configure(Command::cargo_bin("firebird-cli")?))
    }

    fn configure(&self, mut cmd: Command) -> Command {
        cmd.env_clear()
            .env(
                "SYSTEMROOT",
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            )
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            // a site key in the environment must never reach the release host
            .env("FIREBIRD_SITE", "www.test.local")
            .env("FIREBIRD_API_KEY", KEY)
            .env("FIREBIRD_BASE_URL", self.server.url())
            .env("FIREBIRD_CONFIG_DIR", self.config_dir.path())
            .env("FIREBIRD_CLI_UPDATE_API_URL", self.server.url());
        cmd
    }

    fn release(&mut self, path: &str, tag: &str, assets: &Value) -> Mock {
        self.server
            .mock("GET", format!("{RELEASES}/{path}").as_str())
            .match_header("authorization", Matcher::Missing)
            .match_header("x-api-key", Matcher::Missing)
            .with_body(
                json!({
                    "tag_name": tag,
                    "name": tag,
                    "created_at": "2026-01-01T00:00:00Z",
                    "assets": assets,
                })
                .to_string(),
            )
            .create()
    }

    /// Fails the test if the updater ever exchanges the site key or calls the API.
    fn no_site_requests(&mut self) -> Mock {
        self.server
            .mock("POST", "/auth/client-integration")
            .expect(0)
            .create()
    }
}

fn parse(out: &[u8]) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(out)?)
}

/// The JSON error document: the last line of stderr, after any warnings
/// (every test here points FIREBIRD_CLI_UPDATE_API_URL at a mock, which warns).
fn parse_error(stderr: &[u8]) -> Result<Value, Box<dyn Error>> {
    let text = String::from_utf8_lossy(stderr);
    let last = text.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    parse(last.as_bytes())
}

fn message(stderr: &[u8]) -> Result<String, Box<dyn Error>> {
    Ok(parse_error(stderr)?["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

#[test]
fn check_reports_a_newer_release_and_installs_nothing() -> TestResult {
    let mut env = Env::new()?;
    let latest = env.release("latest", "v99.0.0", &json!([]));
    let site = env.no_site_requests();

    let out = env.cmd()?.args(["update", "--check"]).output()?;

    assert!(out.status.success());
    latest.assert();
    site.assert();
    let doc = parse(&out.stdout)?;
    assert_eq!(doc["current_version"], CURRENT);
    assert_eq!(doc["release_version"], "99.0.0");
    assert_eq!(doc["update_available"], true);
    assert_eq!(doc["updated"], false);
    // neither the key nor a token file is part of an update
    let printed = [out.stdout, out.stderr].concat();
    assert!(!String::from_utf8_lossy(&printed).contains(KEY));
    assert!(!env.config_dir.path().join("tokens.json").exists());
    Ok(())
}

#[test]
fn upgrade_is_an_alias() -> TestResult {
    let mut env = Env::new()?;
    env.release("latest", &format!("v{CURRENT}"), &json!([]));
    let out = env.cmd()?.args(["upgrade", "--check"]).output()?;
    assert!(out.status.success());
    assert_eq!(parse(&out.stdout)?["update_available"], false);
    Ok(())
}

#[test]
fn the_current_release_is_up_to_date_without_downloading() -> TestResult {
    let mut env = Env::new()?;
    // no assets and no download mocks: anything fetched beyond this would fail
    env.release("latest", &format!("v{CURRENT}"), &json!([]));

    let out = env.cmd()?.arg("update").output()?;

    assert!(out.status.success());
    let doc = parse(&out.stdout)?;
    assert_eq!(doc["update_available"], false);
    assert_eq!(doc["updated"], false);
    Ok(())
}

#[test]
fn a_repository_with_no_release_yet_is_not_found() -> TestResult {
    let mut env = Env::new()?;
    // what GitHub answers for releases/latest before the first release
    env.server
        .mock("GET", format!("{RELEASES}/latest").as_str())
        .with_status(404)
        .with_body(r#"{"message":"Not Found"}"#)
        .create();

    for args in [vec!["update"], vec!["update", "--check"]] {
        let out = env.cmd()?.args(&args).output()?;
        assert_eq!(out.status.code(), Some(4), "{args:?}");
        assert_eq!(parse_error(&out.stderr)?["error"]["code"], "not_found");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{args:?}");
    }
    Ok(())
}

#[test]
fn an_unknown_tag_is_not_found() -> TestResult {
    let mut env = Env::new()?;
    env.server
        .mock("GET", format!("{RELEASES}/tags/v0.0.1").as_str())
        .with_status(404)
        .with_body(r#"{"message":"Not Found"}"#)
        .create();

    let out = env.cmd()?.args(["update", "--tag", "0.0.1"]).output()?;

    assert_eq!(out.status.code(), Some(4));
    let err = parse_error(&out.stderr)?;
    assert_eq!(err["error"]["code"], "not_found");
    // a redirected release source is announced in every output mode
    assert!(String::from_utf8_lossy(&out.stderr)
        .starts_with("warning: looking for releases somewhere other than RealGeeks/firebird-cli"));
    assert!(message(&out.stderr)?.contains("v0.0.1"));
    Ok(())
}

#[test]
fn a_release_without_this_platform_fails_before_downloading() -> TestResult {
    let mut env = Env::new()?;
    env.release("latest", "v99.0.0", &json!([]));

    let out = env.cmd()?.arg("update").output()?;

    assert_eq!(out.status.code(), Some(1));
    assert!(message(&out.stderr)?.contains("has no archive for"));
    Ok(())
}

#[test]
fn the_release_host_must_be_https() -> TestResult {
    let out = Env::new()?
        .cmd()?
        .env("FIREBIRD_CLI_UPDATE_API_URL", "http://releases.example.com")
        .args(["update", "--check"])
        .output()?;

    assert_eq!(out.status.code(), Some(3));
    Ok(())
}

#[test]
fn a_malformed_repo_override_is_a_config_error() -> TestResult {
    let out = Env::new()?
        .cmd()?
        .env("FIREBIRD_CLI_REPO", "not-a-repo")
        .args(["update", "--check"])
        .output()?;

    assert_eq!(out.status.code(), Some(3));
    assert!(message(&out.stderr)?.contains("owner/name"));
    Ok(())
}

#[test]
fn piped_commands_never_look_for_a_newer_release() -> TestResult {
    let mut env = Env::new()?;
    let lookup = env
        .server
        .mock("GET", format!("{RELEASES}/latest").as_str())
        .expect(0)
        .create();
    env.cmd()?.args(["auth", "sites"]).assert().success();
    env.cmd()?.args(["guide", "--list"]).assert().success();
    lookup.assert();
    assert!(!env.config_dir.path().join("update-check").exists());
    Ok(())
}

/// Replacing the binary: unix only, where a shell script can stand in for
/// the new release.
#[cfg(unix)]
mod install {
    use std::fmt::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use firebird_cli::commands::update::{archive_name, release_target};
    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};

    use super::*;

    const TAG: &str = "v99.0.0";

    /// A copy of the built binary that the test is free to replace.
    fn copy_of_binary(dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
        let path = dir.join("firebird-cli");
        std::fs::copy(assert_cmd::cargo::cargo_bin("firebird-cli"), &path)?;
        Ok(path)
    }

    /// The archive `release.yml` would publish, holding a script that
    /// answers `--version` as the new release.
    fn archive(target: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let script = b"#!/bin/sh\necho 'firebird-cli 99.0.0'\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        tar.append_data(
            &mut header,
            format!("firebird-cli-{TAG}-{target}/firebird-cli"),
            &script[..],
        )?;
        Ok(tar.into_inner()?.finish()?)
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut hex, b| {
                let _ = write!(hex, "{b:02x}");
                hex
            })
    }

    /// Mock the release and its two assets; `sum` is what the `.sha256` says.
    fn publish(env: &mut Env, bytes: &[u8], sum: &str) -> Result<Mock, Box<dyn Error>> {
        let target = release_target()?;
        let name = archive_name(TAG, &target);
        let base = env.server.url();
        let assets = json!([
            // the checksum first, as GitHub may list it: selection is by name
            {"name": format!("{name}.sha256"), "url": format!("{base}/assets/2")},
            {"name": name, "url": format!("{base}/assets/1")},
        ]);
        env.release("latest", TAG, &assets);
        env.release(&format!("tags/{TAG}"), TAG, &assets);
        env.server
            .mock("GET", "/assets/2")
            .with_body(format!("{sum}  {name}\n"))
            .create();
        Ok(env
            .server
            .mock("GET", "/assets/1")
            .match_header("authorization", Matcher::Missing)
            .match_header("x-api-key", Matcher::Missing)
            .with_body(bytes)
            .create())
    }

    fn version_of(binary: &Path) -> Result<String, Box<dyn Error>> {
        let out = std::process::Command::new(binary)
            .arg("--version")
            .output()?;
        assert!(
            out.status.success(),
            "{} --version: {:?} {}",
            binary.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Mock GitHub's attestation lookup for `digest`.
    fn attestations(env: &mut Env, digest: &str) -> Mock {
        env.server.mock(
            "GET",
            format!("/repos/RealGeeks/firebird-cli/attestations/sha256:{digest}").as_str(),
        )
    }

    #[test]
    fn update_replaces_the_binary_after_the_checksum_passes() -> TestResult {
        let mut env = Env::new()?;
        let dir = tempfile::tempdir()?;
        let binary = copy_of_binary(dir.path())?;
        let bytes = archive(&release_target()?)?;
        let download = publish(&mut env, &bytes, &sha256(&bytes))?;
        // a test archive has no real attestation: this is the documented way past it
        let lookup = attestations(&mut env, &sha256(&bytes)).expect(0).create();
        let site = env.no_site_requests();

        let out = env
            .configure(Command::new(&binary))
            .env("FIREBIRD_CLI_SKIP_PROVENANCE", "1")
            .arg("update")
            .output()?;
        lookup.assert();
        site.assert();
        assert!(String::from_utf8_lossy(&out.stderr)
            .contains("not checking the archive's build provenance"));

        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        download.assert();
        let doc = parse(&out.stdout)?;
        assert_eq!(doc["release_version"], "99.0.0");
        assert_eq!(doc["updated"], true);
        assert_eq!(version_of(&binary)?, "firebird-cli 99.0.0");
        let mode = std::fs::metadata(&binary)?.permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the new binary is executable");
        Ok(())
    }

    #[test]
    fn an_archive_without_an_attestation_is_not_installed() -> TestResult {
        let mut env = Env::new()?;
        let dir = tempfile::tempdir()?;
        let binary = copy_of_binary(dir.path())?;
        let bytes = archive(&release_target()?)?;
        let download = publish(&mut env, &bytes, &sha256(&bytes))?;
        let lookup = attestations(&mut env, &sha256(&bytes))
            // the site key never goes to GitHub
            .match_header("authorization", Matcher::Missing)
            .match_header("x-api-key", Matcher::Missing)
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create();

        let out = env
            .configure(Command::new(&binary))
            .arg("update")
            .output()?;

        assert_eq!(out.status.code(), Some(1));
        download.assert();
        lookup.assert();
        assert!(out.stdout.is_empty(), "no result document on failure");
        let message = message(&out.stderr)?;
        assert!(
            message.contains("no build-provenance attestation"),
            "{message}"
        );
        assert!(
            message.contains("FIREBIRD_CLI_SKIP_PROVENANCE"),
            "{message}"
        );
        assert_eq!(version_of(&binary)?, format!("firebird-cli {CURRENT}"));
        Ok(())
    }

    #[test]
    fn an_attestation_for_another_archive_is_not_accepted() -> TestResult {
        let mut env = Env::new()?;
        let dir = tempfile::tempdir()?;
        let binary = copy_of_binary(dir.path())?;
        let bytes = archive(&release_target()?)?;
        publish(&mut env, &bytes, &sha256(&bytes))?;
        // a genuine attestation (of geekcli v0.12.1), served for an archive it
        // does not cover and signed by a workflow this CLI does not trust
        let lookup = attestations(&mut env, &sha256(&bytes))
            .with_body(include_str!(
                "../src/testdata/geekcli-attestation-v0.12.1.json"
            ))
            .create();

        let out = env
            .configure(Command::new(&binary))
            .arg("update")
            .output()?;

        assert_eq!(out.status.code(), Some(1));
        lookup.assert();
        let message = message(&out.stderr)?;
        assert!(message.contains("did not verify"), "{message}");
        assert_eq!(version_of(&binary)?, format!("firebird-cli {CURRENT}"));
        Ok(())
    }

    #[test]
    fn a_read_only_directory_fails_before_downloading() -> TestResult {
        let mut env = Env::new()?;
        let dir = tempfile::tempdir()?;
        let binary = copy_of_binary(dir.path())?;
        let bytes = archive(&release_target()?)?;
        let download = publish(&mut env, &bytes, &sha256(&bytes))?.expect(0);
        let read_only = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(dir.path(), read_only)?;

        let out = env.configure(Command::new(&binary)).arg("update").output();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))?;
        let out = out?;

        assert_eq!(out.status.code(), Some(1));
        download.assert();
        assert!(message(&out.stderr)?.contains("cannot write to"));
        assert_eq!(version_of(&binary)?, format!("firebird-cli {CURRENT}"));
        Ok(())
    }

    #[test]
    fn a_checksum_mismatch_leaves_the_binary_alone() -> TestResult {
        let mut env = Env::new()?;
        let dir = tempfile::tempdir()?;
        let binary = copy_of_binary(dir.path())?;
        let bytes = archive(&release_target()?)?;
        publish(&mut env, &bytes, &"0".repeat(64))?;

        let out = env
            .configure(Command::new(&binary))
            .arg("update")
            .output()?;

        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty(), "no result document on failure");
        assert!(message(&out.stderr)?.to_lowercase().contains("checksum"));
        assert_eq!(version_of(&binary)?, format!("firebird-cli {CURRENT}"));
        Ok(())
    }
}
