//! `firebird-cli update` — replace this binary with a release from GitHub.
//!
//! It installs the same archives `install.sh` and `install.ps1` do, checked
//! against the same `.sha256` files and, before anything is extracted,
//! against the archive's build-provenance attestation (`provenance`). The
//! requests go to GitHub through their own clients, so the site's API key and
//! token are never part of them.

use std::io::IsTerminal;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Args;
use self_update::backends::github;
use self_update::errors::Error as UpdateError;
use self_update::Release;
use serde_json::json;

use super::provenance;
use crate::config;
use crate::error::{Error, Result};
use crate::output::{Format, Printer};

const REPO: &str = "RealGeeks/firebird-cli";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// How often `notify` looks for a newer release, and the file in the config
/// directory that holds the time of the last look.
const CHECK_INTERVAL: u64 = 24 * 60 * 60;
const CHECK_FILE: &str = "update-check";

#[derive(Debug, Args)]
#[command(after_help = "Notes:
  Nothing updates on its own: a script keeps the version it was written
  against until something runs `firebird-cli update`.
  On a terminal, any command says once a day when a newer release exists.
  Piped output and CI never get that notice or its lookup;
  FIREBIRD_CLI_NO_UPDATE_CHECK=1 turns it off everywhere.
  `--check` prints {\"current_version\", \"release_version\", \"update_available\"}
  and exits 0 either way; branch on `update_available`.
  The archive is verified against the sha256 published beside it and its
  build-provenance attestation must show it was built by RealGeeks/firebird-cli's
  release workflow (the check `gh attestation verify` makes); the running
  binary is only replaced after both pass. FIREBIRD_CLI_SKIP_PROVENANCE=1
  skips the attestation (for mirrors without GitHub access) and says so.
  A binary in a directory you cannot write (a root-owned /usr/local/bin)
  fails before downloading; re-run with sudo, or reinstall elsewhere with
  FIREBIRD_CLI_INSTALL_DIR.
  GitHub limits anonymous lookups per address; GH_TOKEN or GITHUB_TOKEN
  raises the limit. FIREBIRD_CLI_REPO names a fork, as it does for install.sh, with
  a warning; its builds still have to be attested by RealGeeks/firebird-cli.")]
pub struct UpdateArgs {
    /// Report whether a newer release exists without installing it
    #[arg(long)]
    pub check: bool,
    /// Install this release instead of the latest, e.g. v0.3.0; an older tag downgrades
    #[arg(long, value_name = "TAG", conflicts_with = "check")]
    pub tag: Option<String>,
}

/// Where releases come from: the repository, and the settings `install.sh`
/// reads from the same environment variables.
struct Source {
    owner: String,
    name: String,
    api_url: Option<String>,
    token: Option<String>,
}

const GITHUB_API: &str = "https://api.github.com";

impl Source {
    fn from_env() -> Result<Self> {
        let repo = env("FIREBIRD_CLI_REPO").unwrap_or_else(|| REPO.to_string());
        let Some((owner, name)) = repo.split_once('/') else {
            return Err(Error::Config(format!(
                "FIREBIRD_CLI_REPO must be owner/name, got '{repo}'"
            )));
        };
        // Tests point this at a mock server; the same https rule as the API applies.
        let api_url = env("FIREBIRD_CLI_UPDATE_API_URL");
        if let Some(url) = &api_url {
            config::check_transport(url).map_err(|_| {
                Error::Config(format!(
                    "FIREBIRD_CLI_UPDATE_API_URL must be https (plain http only for local hosts), got {url}"
                ))
            })?;
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
            api_url,
            token: env("GH_TOKEN").or_else(|| env("GITHUB_TOKEN")),
        })
    }

    /// Where releases come from when it is not the official repository on
    /// github.com, for the warning `run` prints.
    fn redirected(&self) -> Option<String> {
        let repo = format!("{}/{}", self.owner, self.name);
        let other_repo = !repo.eq_ignore_ascii_case(REPO);
        match (&self.api_url, other_repo) {
            (None, false) => None,
            (None, true) => Some(format!("FIREBIRD_CLI_REPO={repo}")),
            (Some(url), false) => Some(format!("FIREBIRD_CLI_UPDATE_API_URL={url}")),
            (Some(url), true) => Some(format!(
                "FIREBIRD_CLI_REPO={repo} and FIREBIRD_CLI_UPDATE_API_URL={url}"
            )),
        }
    }

    fn builder(&self, target: &str) -> github::UpdateBuilder {
        let mut builder = github::Update::configure();
        builder
            .repo_owner(&self.owner)
            .repo_name(&self.name)
            .bin_name("firebird-cli")
            .current_version(CURRENT)
            .target(target)
            // no prompt, and nothing printed on stdout but the result document
            .unattended();
        if let Some(url) = &self.api_url {
            builder.api_base_url(url);
        }
        if let Some(token) = &self.token {
            builder.auth_token(token);
        }
        builder
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The target triple of the release archive for this machine. Linux is
/// always the static musl build, whatever this binary was compiled against.
pub fn release_target() -> Result<String> {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        "windows" => "pc-windows-msvc",
        other => {
            return Err(Error::Other(format!(
                "firebird-cli publishes no release for {other}; build it from source"
            )))
        }
    };
    Ok(format!("{arch}-{os}"))
}

/// The archive `release.yml` publishes for a tag and target.
pub fn archive_name(tag: &str, target: &str) -> String {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("firebird-cli-{tag}-{target}.{ext}")
}

pub fn run(printer: Printer, args: &UpdateArgs) -> Result<()> {
    let target = release_target()?;
    let source = Source::from_env()?;
    // In every output mode, like API warnings: a redirected update source is
    // worth knowing about even when an agent is the one reading.
    if let Some(redirect) = source.redirected() {
        eprintln!(
            "warning: looking for releases somewhere other than {REPO} on github.com ({redirect}); only builds attested by {REPO}'s release workflow are installed"
        );
    }
    let updater = source.builder(&target).build().map_err(failure)?;

    let release = match &args.tag {
        Some(tag) => {
            let tag = normalize_tag(tag);
            updater.get_release_version(&tag).map_err(|err| match err {
                UpdateError::NotFound { .. } => not_found(format!("no release {tag}")),
                other => failure(other),
            })?
        }
        None => updater
            .get_latest_release()
            .map_err(|err| match err {
                UpdateError::NotFound { .. } => not_found(format!(
                    "{}/{} has no published release yet; build from source until it does",
                    source.owner, source.name
                )),
                other => failure(other),
            })?
            .into_vec()
            .into_iter()
            .next()
            .ok_or_else(|| {
                not_found(format!("{}/{} has no releases", source.owner, source.name))
            })?,
    };
    let version = release.version().to_string();
    let newer = self_update::version::bump_is_greater(CURRENT, &version).map_err(failure)?;

    let mut doc = json!({
        "current_version": CURRENT,
        "release_version": version,
        "update_available": newer,
        "updated": false,
        "target": target,
    });
    // An explicit tag installs whatever it names; the latest only when it is newer.
    let wanted = if args.tag.is_some() {
        version != CURRENT
    } else {
        newer
    };
    if args.check || !wanted {
        if newer {
            printer.note(&format!(
                "firebird-cli {version} is available (this is {CURRENT}); run `firebird-cli update`"
            ));
        } else {
            printer.note(&format!("firebird-cli {CURRENT} is up to date"));
        }
        return printer.raw(&doc);
    }

    install(printer, &source, &release, &target)?;
    doc["updated"] = json!(true);
    if let Ok(path) = std::env::current_exe() {
        doc["path"] = json!(path.display().to_string());
    }
    printer.note(&format!("Updated firebird-cli {CURRENT} to {version}"));
    printer.raw(&doc)
}

/// After a command, at most once a day, tell a person at a terminal that a
/// newer release exists. Scripts, agents and CI get nothing: their stdout is
/// not a terminal. Failures are silent; this must never break a
/// command.
pub fn notify(printer: Printer) {
    let wanted = printer.format == Format::Table
        && std::io::stderr().is_terminal()
        && env("CI").is_none()
        && env("FIREBIRD_CLI_NO_UPDATE_CHECK").is_none();
    if !wanted {
        return;
    }
    let Ok(dir) = config::config_dir() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Some(version) = newer_release(&dir.join(CHECK_FILE), now, latest_version) {
        printer.note(&format!(
            "firebird-cli {version} is available (this is {CURRENT}); run `firebird-cli update`"
        ));
    }
}

/// The newer release to announce, if a look is due. The time is recorded
/// before looking, so a failed or slow lookup is not repeated by every
/// command; when it cannot be recorded (no config directory yet) nothing
/// is looked up at all.
fn newer_release(
    stamp: &Path,
    now: u64,
    latest: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let last = std::fs::read_to_string(stamp)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok());
    if last.is_some_and(|then| now.saturating_sub(then) < CHECK_INTERVAL) {
        return None;
    }
    std::fs::write(stamp, now.to_string()).ok()?;
    let version = latest()?;
    self_update::version::bump_is_greater(CURRENT, &version)
        .ok()?
        .then_some(version)
}

fn latest_version() -> Option<String> {
    let source = Source::from_env().ok()?;
    let release = source
        .builder(&release_target().ok()?)
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?
        .get_latest_release()
        .ok()?
        .into_vec()
        .into_iter()
        .next()?;
    Some(release.version().to_string())
}

fn install(printer: Printer, source: &Source, release: &Release, target: &str) -> Result<()> {
    let tag = format!("v{}", release.version());
    let archive = archive_name(&tag, target);
    let folder = format!("firebird-cli-{tag}-{target}");
    if !release.assets().iter().any(|a| a.name() == archive) {
        return Err(Error::Other(format!(
            "release {tag} has no archive for {target} ({archive})"
        )));
    }

    check_writable()?;
    printer.note(&format!("Downloading firebird-cli {tag} for {target}..."));
    let wanted = archive.clone();
    let mut builder = source.builder(target);
    builder
        .release_tag(&tag)
        // by exact name: the `.sha256` beside each archive also contains the target
        .asset_matcher(move |assets| assets.iter().find(|a| a.name() == wanted).cloned())
        .checksum_from_asset(format!("{archive}.sha256"))
        .bin_path_in_archive(format!(
            "{folder}/firebird-cli{}",
            std::env::consts::EXE_SUFFIX
        ));
    if env(provenance::SKIP_ENV).is_some() {
        eprintln!(
            "warning: {} is set: not checking the archive's build provenance, only its sha256",
            provenance::SKIP_ENV
        );
    } else {
        // After the sha256 and GitHub's digest pass, before anything is extracted.
        let api = source
            .api_url
            .clone()
            .unwrap_or_else(|| GITHUB_API.to_string());
        let token = source.token.clone();
        builder.verify_archive(move |path| {
            let digest = provenance::sha256_file(path).map_err(UpdateError::from)?;
            provenance::check(&api, token.as_deref(), &digest)
                .map_err(UpdateError::archive_verification_rejected)
        });
    }
    builder
        .build()
        .map_err(failure)?
        .update_extended()
        .map_err(failure)?;
    Ok(())
}

const NOT_WRITABLE: &str =
    "re-run with sudo, or reinstall to a directory you own (FIREBIRD_CLI_INSTALL_DIR)";

/// Fail before downloading when the binary's directory cannot take the new
/// file. The probe creates a file beside the binary and never opens the
/// binary itself: on macOS, opening a signed executable for writing gets it
/// killed on its next run, so a refused update would break a working install.
fn check_writable() -> Result<()> {
    let exe = std::env::current_exe()?;
    let Some(dir) = exe.parent() else {
        return Ok(());
    };
    let probe = dir.join(format!(".firebird-cli-update-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            // best effort: a leftover empty file is harmless
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(Error::Io(format!(
            "cannot write to {}; {NOT_WRITABLE}",
            dir.display()
        ))),
        // anything else is inconclusive: let the install itself report it
        Err(_) => Ok(()),
    }
}

fn normalize_tag(tag: &str) -> String {
    let tag = tag.trim();
    if tag.starts_with('v') {
        tag.to_string()
    } else {
        format!("v{tag}")
    }
}

fn not_found(message: String) -> Error {
    Error::Api {
        status: 404,
        code: "not_found".into(),
        message,
        body: None,
        retry_after: None,
    }
}

/// Map the updater's errors onto the CLI's exit codes.
fn failure(err: UpdateError) -> Error {
    match err {
        UpdateError::Transport(e) => Error::Network(e.to_string()),
        UpdateError::RateLimited { retry_after, .. } => Error::Api {
            status: 429,
            code: "rate_limited".into(),
            message: "GitHub is rate limiting release lookups from this address; set GH_TOKEN or retry later".into(),
            body: None,
            retry_after: retry_after.map(|d| d.as_secs()),
        },
        UpdateError::NotFound { .. } => not_found("the release was not found on GitHub".into()),
        UpdateError::ArchiveVerificationRejected { reason, .. } => Error::Other(format!(
            "refusing to install: the archive's build provenance did not check out ({}). Nothing was changed. If you are installing from a mirror that cannot reach GitHub, set {}=1 to rely on the sha256 alone",
            reason.unwrap_or_else(|| "no reason given".into()),
            provenance::SKIP_ENV
        )),
        UpdateError::InstallPathNotWritable { path, .. } => Error::Io(format!(
            "cannot write {}; {NOT_WRITABLE}",
            path.display()
        )),
        other => Error::Other(format!("update failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_match_the_release_workflow() {
        assert_eq!(
            archive_name("v0.6.0", "aarch64-apple-darwin"),
            "firebird-cli-v0.6.0-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            archive_name("v0.6.0", "x86_64-pc-windows-msvc"),
            "firebird-cli-v0.6.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn linux_always_takes_the_static_build() -> Result<()> {
        let target = release_target()?;
        assert!(!target.contains("gnu"), "{target}");
        Ok(())
    }

    #[test]
    fn a_newer_release_is_announced_once_a_day() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let stamp = dir.path().join(CHECK_FILE);
        let newer = || Some("999.0.0".to_string());

        assert_eq!(
            newer_release(&stamp, 1_000_000, newer).as_deref(),
            Some("999.0.0")
        );
        // within the day: no lookup at all
        let looked = std::cell::Cell::new(false);
        let spy = || {
            looked.set(true);
            Some("999.0.0".to_string())
        };
        assert_eq!(
            newer_release(&stamp, 1_000_000 + CHECK_INTERVAL - 1, spy),
            None
        );
        assert!(!looked.get());
        // a day later it looks again
        assert!(newer_release(&stamp, 1_000_000 + CHECK_INTERVAL, newer).is_some());
        Ok(())
    }

    #[test]
    fn nothing_is_announced_when_current_or_offline() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let stamp = dir.path().join(CHECK_FILE);
        assert_eq!(
            newer_release(&stamp, 1_000_000, || Some(CURRENT.to_string())),
            None
        );
        // a failed lookup still counts as today's
        let offline = dir.path().join("offline");
        assert_eq!(newer_release(&offline, 1_000_000, || None), None);
        assert_eq!(
            newer_release(&offline, 1_000_001, || Some("999.0.0".into())),
            None
        );
        Ok(())
    }

    #[test]
    fn without_a_config_directory_nothing_is_looked_up() {
        let looked = std::cell::Cell::new(false);
        let stamp = Path::new("/nonexistent-firebird-cli-dir/update-check");
        let spy = || {
            looked.set(true);
            Some("999.0.0".to_string())
        };
        assert_eq!(newer_release(stamp, 1_000_000, spy), None);
        assert!(!looked.get());
    }

    #[test]
    fn tags_gain_their_v() {
        assert_eq!(normalize_tag("0.6.0"), "v0.6.0");
        assert_eq!(normalize_tag(" v0.6.0 "), "v0.6.0");
    }
}
