//! Where the CLI keeps site credentials and how a site is chosen for a run.
//!
//! Resolution order for the site and key, highest priority first:
//!
//! 1. `--site` / `--api-key` flags
//! 2. `FIREBIRD_SITE` / `FIREBIRD_API_KEY` environment variables
//! 3. the config file (`~/.config/firebird-cli/config.toml`), using
//!    `default_site` when only one site is stored or a default has been set
//!
//! The site domain is a label for a stored key: the API works out the site
//! from the key itself. The API is reached at [`DEFAULT_BASE_URL`] unless the
//! stored site, `FIREBIRD_BASE_URL` or `--base-url` says otherwise.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const CONFIG_DIR_ENV: &str = "FIREBIRD_CONFIG_DIR";
pub const DEFAULT_BASE_URL: &str = "https://firebird-api.witco.net";

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_site: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sites: BTreeMap<String, SiteConfig>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteConfig {
    pub api_key: String,
    /// The API origin this key was made for, when it is not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl SiteConfig {
    pub fn api_base(&self) -> &str {
        self.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL)
    }
}

pub fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    // ~/.config on every platform (honouring XDG_CONFIG_HOME), like most
    // developer CLIs, rather than macOS's Library/Application Support.
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => dirs::home_dir()
            .ok_or_else(|| Error::Config("cannot determine the home directory".into()))?
            .join(".config"),
    };
    Ok(base.join("firebird-cli"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Short-lived API tokens, kept apart from the keys; see [`crate::token`].
pub fn token_cache_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("tokens.json"))
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        tighten(path);
        let raw = fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        toml::from_str(&raw)
            .map_err(|e| Error::Config(format!("cannot parse {}: {e}", path.display())))
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = config_path()?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let raw = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("cannot serialize config: {e}")))?;
        write_private_file(path, raw.as_bytes())
            .map_err(|e| Error::Config(format!("cannot write {}: {e}", path.display())))
    }

    /// Store a key for a site, making it the default when it is the first.
    pub fn put_site(&mut self, domain: &str, site: SiteConfig) {
        let domain = normalize_domain(domain);
        if self.sites.is_empty() || self.default_site.is_none() {
            self.default_site = Some(domain.clone());
        }
        self.sites.insert(domain, site);
    }

    pub fn remove_site(&mut self, domain: &str) -> Option<SiteConfig> {
        let domain = normalize_domain(domain);
        let removed = self.sites.remove(&domain);
        if self.default_site.as_deref() == Some(domain.as_str()) {
            self.default_site = self.sites.keys().next().cloned();
        }
        removed
    }

    pub fn site(&self, domain: &str) -> Option<&SiteConfig> {
        self.sites.get(&normalize_domain(domain))
    }
}

/// Write a file atomically and privately: a new 0600 file next to it, renamed
/// over the old one, in a directory made 0700. A secret is never readable by
/// other users, a symlink planted at the path is replaced rather than
/// followed, and a crash mid-write leaves the previous file intact.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_private_dir(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = parent.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let written = write_private(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    tighten(path);
    Ok(())
}

/// Make a private file 0600 and its directory 0700 when other users can get
/// at them (a directory made by hand, a copied file), and say so on stderr.
/// Best effort: a chmod that fails is reported, not fatal. The current
/// directory is left alone when the file has no parent.
pub fn tighten(path: &Path) {
    #[cfg(unix)]
    {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        for (target, mode) in [(dir, 0o700), (Some(path), 0o600)] {
            let Some(target) = target else { continue };
            if let Some(message) = tighten_one(target, mode) {
                eprintln!("warning: {message}");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// chmod `target` to `mode` if group or others have any access to it; the
/// warning to print when something was (or could not be) changed.
#[cfg(unix)]
fn tighten_one(target: &Path, mode: u32) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    // not through a symlink: a planted link must not redirect the chmod
    let meta = fs::symlink_metadata(target).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    let current = meta.permissions().mode() & 0o777;
    // no bits for group or others
    if current.trailing_zeros() >= 6 {
        return None;
    }
    Some(
        match fs::set_permissions(target, fs::Permissions::from_mode(mode)) {
            Ok(()) => format!(
                "{} was accessible to other users (mode {current:o}); changed it to {mode:o}",
                target.display()
            ),
            Err(e) => format!(
                "{} is accessible to other users (mode {current:o}) and could not be changed to {mode:o}: {e}",
                target.display()
            ),
        },
    )
}

/// Create a new file readable only by its owner and write `bytes` to it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// `create_dir_all`, with new directories private to the owner on Unix.
/// (On Windows the user profile's ACL already keeps others out.)
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Accept `www.example.com`, `https://www.example.com/`, `example.com/blog`
/// and always keep just the host.
pub fn normalize_domain(input: &str) -> String {
    let trimmed = input.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    without_scheme
        .split('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Everything a command needs to talk to the API for one site.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    /// The stored site's label; `None` when a key was given without a site.
    pub site: Option<String>,
    /// The API origin, without a trailing slash.
    pub base_url: String,
    pub api_key: Option<String>,
}

/// Inputs to site resolution; flags and env are read by the caller so this
/// stays testable.
#[derive(Default, Clone)]
pub struct Overrides {
    pub site: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

fn given(value: Option<&String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty()).cloned()
}

/// The site a run is about: the one named, else the default, else the only
/// one stored.
pub fn selected_site(config: &Config, overrides: &Overrides) -> Option<String> {
    given(overrides.site.as_ref())
        .map(|s| normalize_domain(&s))
        .or_else(|| config.default_site.clone())
        .or_else(|| {
            if config.sites.len() == 1 {
                config.sites.keys().next().cloned()
            } else {
                None
            }
        })
}

pub fn resolve_target(config: &Config, overrides: &Overrides) -> Result<Target> {
    let override_key = given(overrides.api_key.as_ref()).map(|k| k.trim().to_string());
    let override_base = given(overrides.base_url.as_ref());

    let Some(site) = selected_site(config, overrides) else {
        // The API works out the site from the key, so a key (or a base URL,
        // for a call that needs no key) is enough without a stored site.
        if override_key.is_none() && override_base.is_none() {
            return Err(Error::Config(
                "no site selected. Pass --site <domain>, set FIREBIRD_SITE, or run `firebird-cli auth login --site <domain> --api-key-stdin`"
                    .into(),
            ));
        }
        let base_url = override_base.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        check_transport(&base_url)?;
        return Ok(Target {
            site: None,
            base_url: trim_base(&base_url),
            api_key: override_key,
        });
    };

    let stored = config.sites.get(&site);
    let base_url = override_base
        .clone()
        .or_else(|| stored.and_then(|s| s.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    check_transport(&base_url)?;
    // A stored key only goes to the API it was stored for. A --base-url or a
    // FIREBIRD_BASE_URL left over in the shell must bring its own key.
    if let (None, Some(stored), Some(base)) = (&override_key, stored, &override_base) {
        if !same_origin(base, stored.api_base()) {
            return Err(Error::Config(format!(
                "not sending the stored key for {site} to {base}; pass --api-key (or FIREBIRD_API_KEY) for that API, or unset FIREBIRD_BASE_URL"
            )));
        }
    }
    let api_key = override_key.or_else(|| stored.map(|s| s.api_key.clone()));

    Ok(Target {
        site: Some(site),
        base_url: trim_base(&base_url),
        api_key,
    })
}

fn trim_base(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

/// Whether two URLs share scheme, host and port.
pub fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a.trim()), url::Url::parse(b.trim())) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

/// Keys and tokens only travel over https, except to a local dev host
/// (localhost, a loopback address, `*.localhost`, `*.local` or `*.test`).
pub fn check_transport(base_url: &str) -> Result<()> {
    let url = url::Url::parse(base_url.trim())
        .map_err(|e| Error::Config(format!("invalid base URL {base_url}: {e}")))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if url.host().is_some_and(|h| is_local_host(&h)) => Ok(()),
        "http" => Err(Error::Config(format!(
            "refusing to send an API key over plain http to {base_url}; use https (plain http is only allowed for localhost, *.local and *.test)"
        ))),
        other => Err(Error::Config(format!(
            "unsupported scheme {other}: in {base_url}; use https://"
        ))),
    }
}

fn is_local_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
        url::Host::Domain(name) => {
            let name = name.to_ascii_lowercase();
            name == "localhost"
                || [".localhost", ".local", ".test"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(key: &str, base_url: Option<&str>) -> SiteConfig {
        SiteConfig {
            api_key: key.into(),
            base_url: base_url.map(str::to_string),
        }
    }

    fn named(site: &str) -> Overrides {
        Overrides {
            site: Some(site.into()),
            ..Default::default()
        }
    }

    #[test]
    fn plain_http_only_for_local_hosts() {
        for ok in [
            "https://firebird-api.witco.net",
            "http://localhost:9006",
            "http://127.0.0.1:9000",
            "http://[::1]:8000",
            "http://api.firebird.local",
            "http://api.test",
            "http://api.localhost",
        ] {
            assert!(check_transport(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://firebird-api.witco.net",
            "http://10.0.0.5",
            "ftp://firebird-api.witco.net",
            "http://local.example.com",
            "firebird-api.witco.net",
        ] {
            assert!(check_transport(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_api_base_defaults_and_can_be_stored_or_overridden() {
        let mut config = Config::default();
        config.put_site("www.a.com", stored("fb_live_a", None));
        config.put_site(
            "www.b.com",
            stored("fb_test_b", Some("https://api.staging.example.com/")),
        );
        let base = |overrides: &Overrides| resolve_target(&config, overrides).map(|t| t.base_url);

        assert_eq!(
            base(&named("www.a.com")).ok().as_deref(),
            Some(DEFAULT_BASE_URL)
        );
        assert_eq!(
            base(&named("www.b.com")).ok().as_deref(),
            Some("https://api.staging.example.com")
        );
        let local = Overrides {
            site: Some("www.a.com".into()),
            api_key: Some("fb_test_local".into()),
            base_url: Some("http://localhost:9006".into()),
        };
        assert_eq!(base(&local).ok().as_deref(), Some("http://localhost:9006"));
    }

    #[test]
    fn a_stored_key_is_not_sent_to_another_base_url() {
        let mut config = Config::default();
        config.put_site("www.a.com", stored("fb_live_a", None));
        let elsewhere = Overrides {
            site: Some("www.a.com".into()),
            base_url: Some("https://api.staging.example.com".into()),
            ..Default::default()
        };
        assert!(matches!(
            resolve_target(&config, &elsewhere),
            Err(Error::Config(m)) if m.contains("not sending the stored key")
        ));

        let with_key = Overrides {
            api_key: Some("fb_test_dev".into()),
            ..elsewhere
        };
        assert_eq!(
            resolve_target(&config, &with_key)
                .ok()
                .and_then(|t| t.api_key),
            Some("fb_test_dev".into())
        );

        let same = Overrides {
            site: Some("www.a.com".into()),
            base_url: Some(format!("{DEFAULT_BASE_URL}/")),
            ..Default::default()
        };
        assert_eq!(
            resolve_target(&config, &same).ok().and_then(|t| t.api_key),
            Some("fb_live_a".into())
        );
        // same host, another scheme or port, is another origin
        assert!(!same_origin(
            DEFAULT_BASE_URL,
            "http://firebird-api.witco.net"
        ));
        assert!(!same_origin(
            DEFAULT_BASE_URL,
            "https://firebird-api.witco.net:8443"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn saves_privately_and_replaces_a_planted_symlink() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir()?;
        let dir = base.path().join("firebird-cli");
        let path = dir.join("config.toml");
        let decoy = base.path().join("decoy");
        fs::write(&decoy, "untouched")?;
        fs::create_dir(&dir)?;
        std::os::unix::fs::symlink(&decoy, &path)?;

        let mut config = Config::default();
        config.put_site("www.a.com", stored("fb_live_a", None));
        assert!(config.save_to(&path).is_ok());

        assert_eq!(fs::read_to_string(&decoy)?, "untouched");
        assert!(!fs::symlink_metadata(&path)?.file_type().is_symlink());
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(Config::load_from(&path).ok(), Some(config));

        let fresh = base.path().join("new/firebird-cli/config.toml");
        assert!(Config::default().save_to(&fresh).is_ok());
        let dir_mode = fs::metadata(base.path().join("new/firebird-cli"))?
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn loose_permissions_are_tightened_on_load_and_save() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
        let base = tempfile::tempdir()?;
        let dir = base.path().join("firebird-cli");
        let path = dir.join("config.toml");
        fs::create_dir(&dir)?;
        fs::write(&path, "default_site = \"a.com\"\n")?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;

        let loaded = Config::load_from(&path).ok();
        assert_eq!(
            loaded.and_then(|c| c.default_site).as_deref(),
            Some("a.com")
        );
        assert_eq!(mode(&dir)?, 0o700);
        assert_eq!(mode(&path)?, 0o600);

        // an existing loose directory is tightened by a save too
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o775))?;
        assert!(Config::default().save_to(&path).is_ok());
        assert_eq!(mode(&dir)?, 0o700);
        assert_eq!(mode(&path)?, 0o600);

        // private already: nothing to say
        assert_eq!(tighten_one(&path, 0o600), None);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640))?;
        assert!(tighten_one(&path, 0o600).is_some_and(|m| m.contains("mode 640")));
        Ok(())
    }

    #[test]
    fn normalizes_domains() {
        assert_eq!(
            normalize_domain("https://WWW.Example.com/about/"),
            "www.example.com"
        );
        assert_eq!(normalize_domain(" example.com "), "example.com");
        assert_eq!(normalize_domain("http://localhost:3000"), "localhost:3000");
    }

    #[test]
    fn resolves_single_stored_site_by_default() {
        let mut config = Config::default();
        config
            .sites
            .insert("www.a.com".into(), stored("fb_live_a", None));
        let target = resolve_target(&config, &Overrides::default()).ok();
        assert_eq!(
            target.as_ref().and_then(|t| t.site.as_deref()),
            Some("www.a.com")
        );
        assert_eq!(target.and_then(|t| t.api_key), Some("fb_live_a".into()));
    }

    #[test]
    fn flag_key_beats_stored_key() {
        let mut config = Config::default();
        config.put_site("www.a.com", stored("stored", None));
        let overrides = Overrides {
            api_key: Some(" flag\n".into()),
            ..Default::default()
        };
        let target = resolve_target(&config, &overrides).ok();
        assert_eq!(target.and_then(|t| t.api_key), Some("flag".into()));
    }

    #[test]
    fn a_key_alone_is_enough_and_nothing_at_all_is_a_config_error() {
        let none = resolve_target(&Config::default(), &Overrides::default());
        assert!(matches!(none, Err(Error::Config(_))));

        let key_only = Overrides {
            api_key: Some("fb_live_env".into()),
            ..Default::default()
        };
        let target = resolve_target(&Config::default(), &key_only).ok();
        assert_eq!(
            target.as_ref().map(|t| t.base_url.as_str()),
            Some(DEFAULT_BASE_URL)
        );
        assert_eq!(target.and_then(|t| t.site), None);
    }

    #[test]
    fn an_unknown_site_has_no_key() {
        let mut config = Config::default();
        config.put_site("www.a.com", stored("fb_live_a", None));
        let target = resolve_target(&config, &named("www.b.com")).ok();
        assert_eq!(
            target.as_ref().and_then(|t| t.site.as_deref()),
            Some("www.b.com")
        );
        assert_eq!(target.and_then(|t| t.api_key), None);
    }

    #[test]
    fn removing_default_site_picks_another() {
        let mut config = Config::default();
        config.put_site("a.com", SiteConfig::default());
        config.put_site("b.com", SiteConfig::default());
        assert_eq!(config.default_site.as_deref(), Some("a.com"));
        assert!(config.remove_site("a.com").is_some());
        assert_eq!(config.default_site.as_deref(), Some("b.com"));
        assert!(config.remove_site("a.com").is_none());
    }

    #[test]
    fn round_trips_through_toml() -> std::io::Result<()> {
        let mut config = Config::default();
        config.put_site(
            "a.com",
            stored("fb_test_x", Some("https://api.staging.example.com")),
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        assert!(config.save_to(&path).is_ok());
        assert_eq!(Config::load_from(&path).ok(), Some(config));
        Ok(())
    }
}
