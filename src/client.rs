//! A thin blocking HTTP client for the Firebird API. It knows how an API key
//! becomes a bearer token, the JSON conventions, the error body and 429
//! retries, and nothing about individual resources.
//!
//! The key and the token are only ever sent to the configured API origin:
//! no cross-origin redirects, no full URLs elsewhere, https only (plain http
//! just for local dev hosts). Neither is ever printed, `-v` included.

use std::cell::RefCell;
use std::thread;
use std::time::Duration;

use reqwest::blocking::{Client as HttpClient, RequestBuilder, Response};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT,
};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use crate::config::{self, Target};
use crate::error::{ApiError, Error, Result};
use crate::token::{self, Cache, Token};

pub const USER_AGENT_VALUE: &str = concat!("firebird-cli/", env!("CARGO_PKG_VERSION"));
pub const EXCHANGE_PATH: &str = "auth/client-integration";
const API_KEY_HEADER: &str = "X-Api-Key";
const MAX_RETRY_WAIT: u64 = 30;
const MAX_REDIRECTS: usize = 10;
/// Larger responses are refused rather than buffered: no API response is
/// anywhere near this, so hitting it means something is wrong.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

pub type Query = Vec<(String, String)>;

pub struct Client {
    http: HttpClient,
    /// The API origin with a trailing slash; paths are appended to it.
    root: String,
    api_key: Option<String>,
    /// The bearer token in use by this process.
    token: RefCell<Option<Token>>,
    cache: Cache,
    max_retries: u32,
    verbose: bool,
}

/// A successful response, decoded.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
    pub location: Option<String>,
}

/// What a request authenticates with.
enum Credential<'a> {
    None,
    ApiKey(&'a str),
    Bearer(&'a str),
}

impl Client {
    pub fn new(target: &Target, max_retries: u32, verbose: bool) -> Result<Self> {
        config::check_transport(&target.base_url)?;
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        let http = HttpClient::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(60))
            .redirect(same_origin_redirects())
            .build()?;
        Ok(Self {
            http,
            root: format!("{}/", target.base_url.trim_end_matches('/')),
            api_key: target.api_key.clone().filter(|k| !k.trim().is_empty()),
            token: RefCell::new(None),
            cache: Cache::open(),
            max_retries,
            verbose,
        })
    }

    /// The API origin, without a trailing slash.
    pub fn api_base(&self) -> &str {
        self.root.trim_end_matches('/')
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Resolve a path against the API origin. A full URL is only accepted on
    /// that origin: requests carry the token, so a URL from anywhere else (a
    /// pasted link, a crafted response) is refused.
    pub fn url(&self, path: &str) -> Result<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            if config::same_origin(&self.root, path) {
                return Ok(path.to_string());
            }
            return Err(Error::Usage(format!(
                "refusing to send this site's credentials to {path}: only URLs on {} are allowed",
                self.api_base()
            )));
        }
        Ok(format!("{}{}", self.root, path.trim_start_matches('/')))
    }

    pub fn get(&self, path: &str, query: &Query) -> Result<ApiResponse> {
        self.request(&Method::GET, path, query, None, true)
    }

    /// POST, with a JSON body when the route takes one.
    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<ApiResponse> {
        self.request(&Method::POST, path, &Query::new(), body, true)
    }

    pub fn patch(&self, path: &str, body: &Value) -> Result<ApiResponse> {
        self.request(&Method::PATCH, path, &Query::new(), Some(body), true)
    }

    pub fn delete(&self, path: &str) -> Result<ApiResponse> {
        self.request(&Method::DELETE, path, &Query::new(), None, true)
    }

    /// Perform a request. With `auth`, it carries the bearer token; a 401
    /// means the token was not accepted, so it is thrown away and the request
    /// retried once with a newly exchanged one.
    pub fn request(
        &self,
        method: &Method,
        path: &str,
        query: &Query,
        body: Option<&Value>,
        auth: bool,
    ) -> Result<ApiResponse> {
        let url = self.url(path)?;
        if !auth {
            return self.send(method, &url, query, body, &Credential::None);
        }
        let token = self.token()?;
        match self.send(method, &url, query, body, &Credential::Bearer(&token.jwt)) {
            Err(Error::Api { status: 401, .. }) => {
                if self.verbose {
                    eprintln!("* the API refused the token; exchanging the key again");
                }
                self.forget_token();
                let token = self.exchange()?;
                let retried = self.send(method, &url, query, body, &Credential::Bearer(&token.jwt));
                if matches!(retried, Err(Error::Api { status: 401, .. })) {
                    self.forget_token();
                }
                retried
            }
            other => other,
        }
    }

    /// The token to send: this process's, else the cached one, else a new one.
    fn token(&self) -> Result<Token> {
        let now = token::now();
        if let Some(token) = self.token.borrow().as_ref() {
            // one that could not be read for its expiry lasts the process
            if token.exp.is_none() || token.is_fresh(now) {
                return Ok(token.clone());
            }
        }
        if let Some(token) = self.cache_id().and_then(|id| self.cache.get(&id, now)) {
            self.token.replace(Some(token.clone()));
            return Ok(token);
        }
        self.exchange()
    }

    /// Exchange the API key for a new token and remember it.
    pub fn exchange(&self) -> Result<Token> {
        let Some(key) = &self.api_key else {
            return Err(Error::NotLoggedIn(
                "no API key for this site. Run `firebird-cli auth login --site <domain> --api-key-stdin` or set FIREBIRD_API_KEY".into(),
            ));
        };
        let url = self.url(EXCHANGE_PATH)?;
        let response = self.send(
            &Method::POST,
            &url,
            &Query::new(),
            None,
            &Credential::ApiKey(key),
        )?;
        let jwt = response
            .body
            .get("token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::Other("the API's key exchange returned no token".into()))?;
        let token = Token::new(jwt.to_string());
        if let Some(id) = self.cache_id() {
            self.cache.put(&id, &token, token::now());
        }
        self.token.replace(Some(token.clone()));
        Ok(token)
    }

    fn forget_token(&self) {
        self.token.replace(None);
        if let Some(id) = self.cache_id() {
            self.cache.forget(&id);
        }
    }

    fn cache_id(&self) -> Option<String> {
        self.api_key
            .as_deref()
            .map(|key| token::fingerprint(self.api_base(), key))
    }

    /// Send a request, retrying 429s up to `max_retries` times. Every
    /// back-off prints a one-line notice on stderr, verbose or not, so a long
    /// wait never looks like a hang; stdout is untouched. Verbose lines show
    /// the method, URL and status only: never a header or a body.
    fn send(
        &self,
        method: &Method,
        url: &str,
        query: &Query,
        body: Option<&Value>,
        credential: &Credential<'_>,
    ) -> Result<ApiResponse> {
        let mut attempt = 0;
        loop {
            let request = self.build(method, url, query, body, credential);
            if self.verbose {
                eprintln!("> {method} {url}");
            }
            let response = request.send()?;
            let status = response.status();
            if self.verbose {
                eprintln!("< {status}");
            }
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < self.max_retries {
                let wait = retry_after(&response).unwrap_or(5).min(MAX_RETRY_WAIT);
                attempt += 1;
                eprintln!(
                    "rate limited; retrying in {wait}s (attempt {attempt} of {})",
                    self.max_retries
                );
                thread::sleep(Duration::from_secs(wait));
                continue;
            }
            return decode(response);
        }
    }

    fn build(
        &self,
        method: &Method,
        url: &str,
        query: &Query,
        body: Option<&Value>,
        credential: &Credential<'_>,
    ) -> RequestBuilder {
        let mut request = self.http.request(method.clone(), url);
        if !query.is_empty() {
            request = request.query(query);
        }
        request = match credential {
            Credential::None => request,
            Credential::ApiKey(key) => request.header(API_KEY_HEADER, *key),
            Credential::Bearer(jwt) => request.header(AUTHORIZATION, format!("Bearer {jwt}")),
        };
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").json(body);
        }
        request
    }
}

/// Follow redirects only while they stay on the origin of the original
/// request. reqwest drops `Authorization` when a single hop changes host, but
/// a later same-host hop on the new origin gets it back, and it knows nothing
/// of `X-Api-Key`, so a redirect to another origin is not followed at all.
fn same_origin_redirects() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let same_origin = attempt
            .previous()
            .first()
            .is_none_or(|first| first.origin() == attempt.url().origin());
        if same_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn retry_after(response: &Response) -> Option<u64> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_retry_after(v, chrono::Utc::now()))
}

/// `Retry-After` is either delay-seconds or an HTTP-date. A date in the past
/// means "now" (0).
fn parse_retry_after(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<u64> {
    let value = value.trim();
    if let Ok(secs) = value.parse() {
        return Some(secs);
    }
    let when = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let secs = (when.with_timezone(&chrono::Utc) - now).num_seconds();
    Some(u64::try_from(secs).unwrap_or(0))
}

fn decode(response: Response) -> Result<ApiResponse> {
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let retry = retry_after(&response);
    if status.is_redirection() {
        return Err(Error::Other(format!(
            "the API redirected to {}, another origin; firebird-cli does not follow it with your credentials. Check the API base URL (--base-url)",
            location.as_deref().unwrap_or("an unknown location")
        )));
    }
    let text = read_limited(response)?;

    if status.is_success() {
        let body = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };
        return Ok(ApiResponse {
            status: status.as_u16(),
            body,
            location,
        });
    }

    Err(Error::api(parse_error(status.as_u16(), &text, retry)))
}

/// The body as text, refusing anything over `MAX_RESPONSE_BYTES`.
fn read_limited(response: Response) -> Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Io(format!("reading the response: {e}")))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(Error::Other(format!(
            "the response is larger than {MAX_RESPONSE_BYTES} bytes; refusing to read it"
        )));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Turn an error response into `ApiError`. Firebird's shape is
/// `{"error": "<message>"}`, sometimes with a `code` and other fields; a
/// non-JSON body (a proxy 502, an HTML 404) is tolerated.
pub fn parse_error(status: u16, text: &str, retry_after: Option<u64>) -> ApiError {
    let body: Option<Value> = serde_json::from_str(text).ok();
    let field = |name: &str| body.as_ref().and_then(|b| b.get(name));
    let code = field("code")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map_or_else(|| default_code(status), str::to_string);
    let message = match field("error") {
        Some(Value::String(message)) => message.clone(),
        Some(Value::Object(inner)) => inner
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| default_message(status, text), str::to_string),
        _ => field("message")
            .and_then(Value::as_str)
            .map_or_else(|| default_message(status, text), str::to_string),
    };
    ApiError {
        status,
        code,
        message,
        body,
        retry_after,
    }
}

fn default_code(status: u16) -> String {
    match status {
        400 | 422 => "validation_error",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        405 => "method_not_allowed",
        409 => "conflict",
        413 => "payload_too_large",
        426 => "upgrade_required",
        429 => "rate_limited",
        500..=599 => "server_error",
        _ => "http_error",
    }
    .to_string()
}

fn default_message(status: u16, text: &str) -> String {
    let snippet: String = text.trim().chars().take(200).collect();
    if snippet.is_empty() || snippet.starts_with('<') {
        format!("HTTP {status} (no JSON error body; is this the Firebird API?)")
    } else {
        format!("HTTP {status}: {snippet}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_plain_error_body() {
        let err = parse_error(400, r#"{"error":"pageUuid must be a valid UUID"}"#, None);
        assert_eq!(err.code, "validation_error");
        assert_eq!(err.message, "pageUuid must be a valid UUID");
        assert_eq!(
            err.body,
            Some(json!({"error": "pageUuid must be a valid UUID"}))
        );
    }

    #[test]
    fn the_bodys_own_code_wins_and_extra_fields_are_kept() {
        let err = parse_error(
            422,
            r#"{"error":"Already current","code":"REVISION_ALREADY_CURRENT"}"#,
            None,
        );
        assert_eq!(err.code, "REVISION_ALREADY_CURRENT");

        let locked = parse_error(
            409,
            r#"{"error":"Locked","lockedPages":[{"pageUuid":"p1"}]}"#,
            None,
        );
        assert_eq!(locked.code, "conflict");
        assert_eq!(
            locked
                .body
                .as_ref()
                .and_then(|b| b.pointer("/lockedPages/0/pageUuid")),
            Some(&json!("p1"))
        );

        let no_message = parse_error(422, r#"{"errors":[{"path":"title"}]}"#, None);
        assert_eq!(no_message.code, "validation_error");
        assert!(no_message.message.starts_with("HTTP 422"));
        assert!(no_message.body.is_some());
    }

    #[test]
    fn statuses_have_default_codes() {
        for (status, code) in [
            (401, "unauthorized"),
            (403, "forbidden"),
            (404, "not_found"),
            (409, "conflict"),
            (429, "rate_limited"),
            (500, "server_error"),
            (418, "http_error"),
        ] {
            assert_eq!(parse_error(status, "{}", None).code, code);
        }
    }

    #[test]
    fn tolerates_html_bodies() {
        let err = parse_error(502, "<html>bad gateway</html>", None);
        assert_eq!(err.code, "server_error");
        assert!(err.message.contains("HTTP 502"));
        assert_eq!(err.body, None);
    }

    #[test]
    fn retry_after_takes_seconds_or_an_http_date() {
        let now = chrono::DateTime::parse_from_rfc3339("2015-10-21T07:27:30Z")
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_default();
        assert_eq!(parse_retry_after(" 12 ", now), Some(12));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(30)
        );
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:00:00 GMT", now),
            Some(0)
        );
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[test]
    fn resolves_paths_on_the_api_origin_only() {
        let target = Target {
            site: Some("www.x.com".into()),
            base_url: "https://api.x.com".into(),
            api_key: None,
        };
        let Ok(client) = Client::new(&target, 0, false) else {
            return;
        };
        let url = |p: &str| client.url(p).ok();
        assert_eq!(url("pages").as_deref(), Some("https://api.x.com/pages"));
        assert_eq!(
            url("/pages/p1/content").as_deref(),
            Some("https://api.x.com/pages/p1/content")
        );
        assert_eq!(
            url("https://api.x.com/blocks?includeSchema=false").as_deref(),
            Some("https://api.x.com/blocks?includeSchema=false")
        );
        // credentials never go to another origin, even a sibling or downgrade
        assert_eq!(url("https://www.x.com/pages"), None);
        assert_eq!(url("https://api.x.com.evil.net/pages"), None);
        assert_eq!(url("http://api.x.com/pages"), None);
        assert_eq!(url("https://api.x.com:8443/pages"), None);
    }

    #[test]
    fn refuses_a_plain_http_api() {
        let target = Target {
            site: None,
            base_url: "http://api.x.com".into(),
            api_key: Some("fb_test_k".into()),
        };
        assert!(matches!(
            Client::new(&target, 0, false),
            Err(Error::Config(_))
        ));
    }
}
