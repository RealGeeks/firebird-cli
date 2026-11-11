//! Which local files firebird-cli will read and send to the API.
//!
//! A body (`api -d @file`) reads whatever path it is given and sends it to
//! the API, where it may end up on a public page. An agent talked into
//! "sending" `~/.ssh/id_rsa` would publish a private key. So hidden files,
//! files inside hidden directories directly in the home directory (dotfiles,
//! `~/.ssh`, `~/.aws`, …), well-known credential directories and
//! firebird-cli's own config and token files are refused unless the run says
//! otherwise with `--allow-any-source` or `FIREBIRD_ALLOW_ANY_SOURCE=1`.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config;
use crate::error::{Error, Result};

pub const ENV: &str = "FIREBIRD_ALLOW_ANY_SOURCE";
pub const FLAG: &str = "--allow-any-source";

/// Paths under the home directory that hold credentials. Most are hidden
/// anyway; naming them gives a clearer error.
const CREDENTIAL_PATHS: &[&str] = &[
    ".ssh",
    ".aws",
    ".config/gcloud",
    ".gnupg",
    ".kube",
    ".docker",
    ".azure",
    ".netrc",
    ".git-credentials",
    ".config/gh",
];

static ALLOW_ANY: AtomicBool = AtomicBool::new(false);

/// Set once from the global flag (or its environment variable).
pub fn set_allow_any(allow: bool) {
    ALLOW_ANY.store(allow, Ordering::Relaxed);
}

pub fn allow_any() -> bool {
    ALLOW_ANY.load(Ordering::Relaxed)
}

/// Read a body file, or stdin for `-`. Files that look like secrets are
/// refused; see [`check`].
pub fn read(path: &str) -> Result<String> {
    if path == "-" {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    check(Path::new(path))?;
    fs::read_to_string(path).map_err(|e| Error::Io(format!("cannot read {path}: {e}")))
}

/// Refuse to read `path` when it looks like a secret, unless overridden.
/// A path that does not exist passes: reading it reports that.
pub fn check(path: &Path) -> Result<()> {
    match why_refused(path) {
        Some(reason) => Err(Error::Usage(format!(
            "refusing to read {}: {reason}, and what firebird-cli reads is sent to the API. If you really mean to send it, pass {FLAG} (or set {ENV}=1)",
            path.display(),
        ))),
        None => Ok(()),
    }
}

/// The reason `path` is refused this run, if it is.
pub fn why_refused(path: &Path) -> Option<String> {
    if allow_any() {
        return None;
    }
    let home = dirs::home_dir();
    let own: Vec<PathBuf> = [config::config_path(), config::token_cache_path()]
        .into_iter()
        .filter_map(std::result::Result::ok)
        .collect();
    refusal(path, home.as_deref(), &own)
}

/// Why `path` must not be read, if it must not. Both the path as given and
/// where it leads (symlinks resolved) are checked, so a link in an ordinary
/// folder does not reach into `~/.ssh`. `own` is firebird-cli's own files.
fn refusal(path: &Path, home: Option<&Path>, own: &[PathBuf]) -> Option<String> {
    let given = absolute(path);
    let home = home.map(absolute);
    let home = home.as_deref();
    let real = fs::canonicalize(path).ok();
    let home_real = home.and_then(|h| fs::canonicalize(h).ok());
    let own: Vec<PathBuf> = own
        .iter()
        .flat_map(|file| [Some(absolute(file)), fs::canonicalize(file).ok()])
        .flatten()
        .collect();
    for candidate in std::iter::once(given).chain(real) {
        if own.contains(&candidate) {
            return Some(
                "it is one of firebird-cli's own files, which hold your API keys and tokens".into(),
            );
        }
        if candidate
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            return Some("it is a hidden file".into());
        }
        for home in [home, home_real.as_deref()].into_iter().flatten() {
            let Ok(inside) = candidate.strip_prefix(home) else {
                continue;
            };
            if let Some(dir) = CREDENTIAL_PATHS
                .iter()
                .find(|dir| inside.starts_with(Path::new(dir)))
            {
                return Some(format!("~/{dir} holds credentials"));
            }
            // Only a hidden folder directly in home (~/.ssh, ~/.config, ~/.local):
            // deeper ones are project folders, or on Windows the temp directory
            // (AppData\Local\Temp\.tmpXXXX) that lives under the profile.
            if let Some(hidden) = inside.components().next().and_then(|c| match c {
                Component::Normal(name) => name.to_str().filter(|n| n.starts_with('.')),
                _ => None,
            }) {
                return Some(format!(
                    "it is inside the hidden directory ~/{hidden}, where credentials and settings live"
                ));
            }
        }
    }
    None
}

/// The path made absolute against the working directory, with `.` and `..`
/// folded away lexically (symlinks are left to `canonicalize`).
fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Unix-style test path made absolute on Windows too (`C:/home/...`).
    fn abs(path: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:{path}"))
        } else {
            PathBuf::from(path)
        }
    }

    fn refused(path: &Path, home: &Path) -> Option<String> {
        refusal(
            path,
            Some(home),
            &[
                home.join(".config/firebird-cli/config.toml"),
                home.join(".config/firebird-cli/tokens.json"),
            ],
        )
    }

    #[test]
    fn credentials_and_hidden_files_are_refused() {
        let home = &abs("/home/agent");
        for (path, reason) in [
            ("/home/agent/.ssh/id_rsa", "~/.ssh holds credentials"),
            ("/home/agent/.aws/credentials", "~/.aws"),
            ("/home/agent/.config/gcloud/creds.json", "~/.config/gcloud"),
            ("/home/agent/.kube/config", "~/.kube"),
            ("/home/agent/.docker/config.json", "~/.docker"),
            ("/home/agent/.gnupg/secring.gpg", "~/.gnupg"),
            ("/home/agent/.config/firebird-cli/config.toml", "own files"),
            ("/home/agent/.config/firebird-cli/tokens.json", "own files"),
            ("/home/agent/.zsh_history", "hidden file"),
            ("/srv/site/.env", "hidden file"),
            (
                "/home/agent/.local/share/notes.txt",
                "hidden directory ~/.local",
            ),
            ("/home/agent/work/../.ssh/id_ed25519", "~/.ssh"),
        ] {
            let why = refused(&abs(path), home);
            assert!(
                why.as_deref().is_some_and(|w| w.contains(reason)),
                "{path}: {why:?}"
            );
        }
    }

    #[test]
    fn ordinary_files_are_read() {
        let home = &abs("/home/agent");
        for path in [
            "/home/agent/work/block.json",
            "/home/agent/work/./data/hero.json",
            // a hidden folder deeper down (a project's, Windows' temp dir) is fine
            "/home/agent/AppData/Local/Temp/.tmpX1/body.json",
            // hidden directories outside home (a temp dir) are fine
            "/tmp/.tmpAbC123/body.json",
            "/var/folders/xy/T/.tmpQ/body.json",
        ] {
            assert_eq!(refused(&abs(path), home), None, "{path}");
        }
    }

    #[test]
    fn its_own_files_are_refused_wherever_they_live() {
        let config = abs("/srv/firebird-cli/config.toml");
        let tokens = abs("/srv/firebird-cli/tokens.json");
        let home = abs("/home/agent");
        let own = [config.clone(), tokens.clone()];
        for file in [&config, &tokens] {
            let why = refusal(file, Some(&home), &own);
            assert!(why.is_some_and(|w| w.contains("API keys")));
        }
        // their neighbours are not
        assert_eq!(
            refusal(&abs("/srv/firebird-cli/body.json"), Some(&home), &own),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_a_credential_directory_is_refused() -> std::io::Result<()> {
        let home = tempfile::tempdir()?;
        let ssh = home.path().join(".ssh");
        fs::create_dir(&ssh)?;
        fs::write(ssh.join("id_rsa"), "secret")?;
        let work = tempfile::tempdir()?;
        let link = work.path().join("body.json");
        std::os::unix::fs::symlink(ssh.join("id_rsa"), &link)?;
        let why = refusal(&link, Some(home.path()), &[]);
        assert!(why.is_some_and(|w| w.contains(".ssh")));
        Ok(())
    }
}
