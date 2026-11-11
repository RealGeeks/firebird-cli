//! One error type for the whole CLI, mapped onto stable exit codes so an
//! agent can branch on the outcome without parsing text.

use serde_json::Value;

/// Exit codes. Documented in `firebird-cli guide` and the README; keep them stable.
pub mod exit {
    pub const OK: i32 = 0;
    pub const GENERAL: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const AUTH: i32 = 3;
    pub const NOT_FOUND: i32 = 4;
    pub const VALIDATION: i32 = 5;
    pub const CONFLICT: i32 = 6;
    pub const RATE_LIMITED: i32 = 7;
    pub const NETWORK: i32 = 8;
    pub const UPGRADE_REQUIRED: i32 = 9;
}

/// An error response from the API. Firebird answers `{"error": "<message>"}`,
/// sometimes with a `code` or other fields next to it; `body` keeps the whole
/// document so nothing the API said is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub body: Option<Value>,
    pub retry_after: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Usage(String),

    #[error("{0}")]
    Config(String),

    #[error("not logged in: {0}")]
    NotLoggedIn(String),

    #[error("{code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
        body: Option<Value>,
        retry_after: Option<u64>,
    },

    #[error("network error: {0}")]
    Network(String),

    #[error("{0}")]
    Io(String),

    /// Whatever reads our output (`| head`, a closed pager) went away. Not a
    /// failure: `main` exits 0 without printing anything.
    #[error("output closed")]
    BrokenPipe,

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// What to do about it, for the errors where the message alone is not enough.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Error::NotLoggedIn(_) => {
                Some("run `firebird-cli auth login --site <domain> --api-key-stdin`")
            }
            Error::Api { status, .. } => match *status {
                401 => Some(
                    "the API key was not accepted: it may be mistyped, revoked, or made for another environment. Create a key on the site's Integrations page and run `firebird-cli auth login --site <domain> --api-key-stdin`",
                ),
                413 => Some("the request body is too large; keep JSON bodies small"),
                426 => Some(
                    "this firebird-cli is older than the API accepts; build a newer one, then run the command again",
                ),
                _ => None,
            },
            Error::Network(message) if message.contains("certificate") => Some(
                "the API's certificate was not trusted. Behind a proxy that inspects HTTPS (Zscaler and the like), install its root certificate in the system store, or point SSL_CERT_FILE at a PEM bundle that includes it; see `firebird-cli guide setup`",
            ),
            _ => None,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => exit::USAGE,
            Error::Config(_) | Error::NotLoggedIn(_) => exit::AUTH,
            Error::Api { status, .. } => api_exit_code(*status),
            Error::Network(_) => exit::NETWORK,
            Error::Io(_) | Error::Other(_) => exit::GENERAL,
            Error::BrokenPipe => exit::OK,
        }
    }

    /// A short machine-readable code for the JSON error output.
    pub fn code(&self) -> &str {
        match self {
            Error::Usage(_) => "usage",
            Error::Config(_) => "config",
            Error::NotLoggedIn(_) => "not_logged_in",
            Error::Api { code, .. } => code,
            Error::Network(_) => "network",
            Error::Io(_) => "io",
            Error::BrokenPipe => "broken_pipe",
            Error::Other(_) => "error",
        }
    }

    pub fn api(err: ApiError) -> Self {
        Error::Api {
            status: err.status,
            code: err.code,
            message: err.message,
            body: err.body,
            retry_after: err.retry_after,
        }
    }

    /// The message without the code prefix that `Display` adds.
    pub fn message(&self) -> String {
        match self {
            Error::Api { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }

    /// The API's error response, when there was one and it was JSON.
    pub fn body(&self) -> Option<&Value> {
        match self {
            Error::Api { body, .. } => body.as_ref(),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Error::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

pub fn api_exit_code(status: u16) -> i32 {
    match status {
        401 | 403 => exit::AUTH,
        404 => exit::NOT_FOUND,
        400 | 413 | 415 | 422 => exit::VALIDATION,
        409 => exit::CONFLICT,
        429 => exit::RATE_LIMITED,
        426 => exit::UPGRADE_REQUIRED,
        _ => exit::GENERAL,
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            return Error::BrokenPipe;
        }
        Error::Io(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        // writing JSON to stdout fails as a serde_json error too
        if err.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe) {
            return Error::BrokenPipe;
        }
        if err.is_io() {
            return Error::Io(err.to_string());
        }
        Error::Usage(format!("invalid JSON: {err}"))
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        // Strip the URL so nothing in a query string leaks into logs.
        let err = err.without_url();
        // reqwest's own text is just "error sending request"; the reason
        // (a refused certificate, a DNS failure) is further down the chain.
        let mut message = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            let text = cause.to_string();
            if !message.contains(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }
            source = cause.source();
        }
        Error::Network(message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, code: &str) -> Error {
        Error::Api {
            status,
            code: code.into(),
            message: String::new(),
            body: None,
            retry_after: None,
        }
    }

    #[test]
    fn statuses_map_onto_the_documented_exit_codes() {
        for (status, code) in [
            (400, 5),
            (401, 3),
            (403, 3),
            (404, 4),
            (409, 6),
            (413, 5),
            (422, 5),
            (426, 9),
            (429, 7),
            (500, 1),
            (503, 1),
        ] {
            assert_eq!(api(status, "x").exit_code(), code, "{status}");
        }
        assert_eq!(Error::Usage(String::new()).exit_code(), 2);
        assert_eq!(Error::NotLoggedIn(String::new()).exit_code(), 3);
        assert_eq!(Error::Network(String::new()).exit_code(), 8);
    }

    #[test]
    fn a_refused_certificate_points_at_the_proxy_setup() {
        let refused = Error::Network(
            "error sending request: client error (Connect): invalid peer certificate: UnknownIssuer".into(),
        );
        assert!(refused
            .hint()
            .is_some_and(|hint| hint.contains("SSL_CERT_FILE")));
        assert!(Error::Network("error sending request: dns error".into())
            .hint()
            .is_none());
    }

    #[test]
    fn hints_name_the_next_step() {
        assert!(api(401, "unauthorized")
            .hint()
            .is_some_and(|h| h.contains("auth login")));
        assert!(Error::NotLoggedIn(String::new())
            .hint()
            .is_some_and(|h| h.contains("--api-key-stdin")));
        assert!(api(404, "not_found").hint().is_none());
        assert!(api(422, "validation_error").hint().is_none());
    }
}
