//! The short-lived API token (a JWT) an API key is exchanged for, and the
//! file that lets the next process reuse it instead of exchanging again.
//!
//! The JWT is only decoded to read its expiry and, for `me`, its claims. The
//! signature is not verified: the API does that on every request.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config;

/// A token this close to expiry is replaced rather than sent.
pub const REFRESH_MARGIN_SECS: i64 = 60;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    #[serde(rename = "token")]
    pub jwt: String,
    /// Unix time the token expires; `None` when the JWT could not be read.
    pub exp: Option<i64>,
}

impl Token {
    pub fn new(jwt: String) -> Self {
        let exp = claims(&jwt)
            .as_ref()
            .and_then(|c| c.get("exp"))
            .and_then(Value::as_i64);
        Self { jwt, exp }
    }

    /// Usable for a request made at `now`, with the refresh margin to spare.
    pub fn is_fresh(&self, now: i64) -> bool {
        self.exp.is_some_and(|exp| exp - now > REFRESH_MARGIN_SECS)
    }
}

/// The JWT's payload as JSON, unverified.
pub fn claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Names a cache entry without storing the key: a token is only reused for
/// the same key at the same API.
pub fn fingerprint(base_url: &str, api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(base_url.as_bytes());
    hasher.update([0]);
    hasher.update(api_key.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    tokens: BTreeMap<String, Token>,
}

/// Tokens on disk, by [`fingerprint`]. Every operation is best effort: a
/// cache that cannot be read or written costs one extra exchange, never a
/// failed command.
pub struct Cache {
    path: Option<PathBuf>,
}

impl Cache {
    pub fn open() -> Self {
        Self {
            path: config::token_cache_path().ok(),
        }
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn load(&self) -> CacheFile {
        let Some(path) = self.path.as_ref().filter(|p| p.exists()) else {
            return CacheFile::default();
        };
        config::tighten(path);
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, file: &CacheFile) {
        let (Some(path), Ok(raw)) = (&self.path, serde_json::to_vec(file)) else {
            return;
        };
        let _ = config::write_private_file(path, &raw);
    }

    /// The stored token for `id`, if it is still fresh at `now`.
    pub fn get(&self, id: &str, now: i64) -> Option<Token> {
        self.load().tokens.remove(id).filter(|t| t.is_fresh(now))
    }

    /// Store `token` for `id`, dropping every entry that has expired.
    pub fn put(&self, id: &str, token: &Token, now: i64) {
        if token.exp.is_none() {
            return;
        }
        let mut file = self.load();
        file.tokens
            .retain(|_, t| t.exp.is_some_and(|exp| exp > now));
        file.tokens.insert(id.to_string(), token.clone());
        self.save(&file);
    }

    pub fn forget(&self, id: &str) {
        let mut file = self.load();
        if file.tokens.remove(id).is_some() {
            self.save(&file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt(payload: &Value) -> String {
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    #[test]
    fn reads_claims_without_verifying() {
        let token = jwt(&json!({"siteUuid": "s1", "clientRole": "integration", "exp": 2000}));
        assert_eq!(
            claims(&token).and_then(|c| c.get("siteUuid").cloned()),
            Some(json!("s1"))
        );
        assert_eq!(Token::new(token).exp, Some(2000));
        assert_eq!(claims("not-a-jwt"), None);
        assert_eq!(claims("a.!!!.c"), None);
        assert_eq!(Token::new("opaque".into()).exp, None);
    }

    #[test]
    fn a_token_is_replaced_a_minute_before_it_expires() {
        let token = Token::new(jwt(&json!({"exp": 1000})));
        assert!(token.is_fresh(900));
        assert!(!token.is_fresh(940));
        assert!(!token.is_fresh(1001));
        assert!(!Token::new("opaque".into()).is_fresh(0));
    }

    #[test]
    fn the_fingerprint_depends_on_both_key_and_api() {
        let a = fingerprint("https://api.test", "fb_test_a");
        assert_eq!(a.len(), 64);
        assert_eq!(a, fingerprint("https://api.test", "fb_test_a"));
        assert_ne!(a, fingerprint("https://api.test", "fb_test_b"));
        assert_ne!(a, fingerprint("https://other.test", "fb_test_a"));
        assert!(!a.contains("fb_test_a"));
    }

    #[test]
    fn caches_fresh_tokens_and_prunes_expired_ones() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("firebird-cli/tokens.json");
        let cache = Cache::at(path.clone());
        assert!(cache.get("a", 0).is_none());

        let old = Token::new(jwt(&json!({"exp": 1000})));
        let new = Token::new(jwt(&json!({"exp": 5000})));
        cache.put("old", &old, 0);
        cache.put("new", &new, 2000);
        cache.put("opaque", &Token::new("opaque".into()), 2000);

        assert!(cache.get("new", 2000) == Some(new));
        assert!(cache.get("new", 4990).is_none());
        assert!(cache.get("old", 0).is_none());
        assert!(cache.get("opaque", 0).is_none());
        let raw = fs::read_to_string(&path)?;
        assert!(!raw.contains(&old.jwt));

        cache.forget("new");
        assert!(cache.get("new", 2000).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        }

        fs::write(&path, "not json")?;
        assert!(cache.get("new", 2000).is_none());
        Ok(())
    }
}
