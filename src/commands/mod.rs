//! Shared plumbing for the commands: the run context and path building.

pub mod api;
pub mod auth;
pub mod blocks;
pub mod guide;
pub mod pages;

use serde_json::Value;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::output::Printer;

pub struct Context {
    pub client: Client,
    pub printer: Printer,
    /// The stored site's label, when the run selected one.
    pub site: Option<String>,
}

/// The list in a response: the API returns a bare JSON array.
pub fn items(body: &Value) -> Vec<Value> {
    body.as_array().cloned().unwrap_or_default()
}

/// An identifier from the command line as one URL path segment. Firebird's
/// identifiers are UUIDs and code names, so anything that could change the
/// path (`/`, `..`, `?`, `#`, `%`) is a usage error rather than a request.
pub fn segment(label: &str, value: &str) -> Result<String> {
    let value = value.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if value.is_empty() || !value.chars().all(allowed) {
        return Err(Error::Usage(format!(
            "{label} must be letters, digits, '-' or '_', got '{value}'"
        )));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_cannot_change_the_path() {
        assert_eq!(
            segment("pageUuid", " 0b0e7c1e-6f0a-4b7e-9d2e-3f1a2b3c4d5e ").ok(),
            Some("0b0e7c1e-6f0a-4b7e-9d2e-3f1a2b3c4d5e".into())
        );
        assert_eq!(
            segment("codeName", "hero_banner").ok(),
            Some("hero_banner".into())
        );
        for bad in ["", "a/b", "../drafts", "p1?x=1", "p1#x", "%2e%2e", "a b"] {
            assert!(
                matches!(segment("pageUuid", bad), Err(Error::Usage(_))),
                "{bad}"
            );
        }
    }
}
