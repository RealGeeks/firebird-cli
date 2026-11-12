//! Shared plumbing for the commands: the run context, page references,
//! block data from flags, path building and the confirmation prompt.

pub mod api;
pub mod areas;
pub mod auth;
pub mod blocks;
pub mod draft;
pub mod guide;
pub mod media;
pub mod nav;
pub mod pages;
pub mod provenance;
pub mod revisions;
pub mod settings;
pub mod update;
pub mod versions;

use std::io::{self, IsTerminal, Write};

use serde_json::{Map, Value};

use crate::client::{Client, Query};
use crate::error::{Error, Result};
use crate::output::Printer;
use crate::sources;

pub const PAGES: &str = "pages";

pub struct Context {
    pub client: Client,
    pub printer: Printer,
    /// The stored site's label, when the run selected one.
    pub site: Option<String>,
    /// `-y`: do what was asked without the confirmation prompt.
    pub yes: bool,
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

/// `8-4-4-4-12` hex digits.
pub fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The page a `<page>` argument names, as a page from `GET /pages`. A uuid is
/// matched by `pageUuid`; anything else is a slug, with or without slashes
/// (`about`, `/about/`), and `/` or `home` is the home page, whose slug is
/// the empty string.
pub fn find_page(pages: &[Value], reference: &str) -> Result<Value> {
    let reference = reference.trim();
    let field = |page: &Value, name: &str| {
        page.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let by_slug = |slug: &str| -> Vec<&Value> {
        pages
            .iter()
            .filter(|p| field(p, "slug").trim_matches('/') == slug)
            .collect()
    };
    let wanted = reference.trim_matches('/');
    let matches: Vec<&Value> = if is_uuid(reference) {
        pages
            .iter()
            .filter(|p| field(p, "pageUuid").eq_ignore_ascii_case(reference))
            .collect()
    } else {
        let exact = by_slug(wanted);
        if exact.is_empty() && wanted == "home" {
            by_slug("")
        } else {
            exact
        }
    };
    match matches.as_slice() {
        [page] => Ok((*page).clone()),
        [] => {
            let mut slugs: Vec<String> = pages
                .iter()
                .map(|p| format!("/{}", field(p, "slug").trim_matches('/')))
                .collect();
            slugs.sort();
            Err(Error::Api {
                status: 404,
                code: "not_found".into(),
                message: format!(
                    "no page '{reference}' on this site. Its pages: {}",
                    slugs.join(", ")
                ),
                body: None,
                retry_after: None,
            })
        }
        many => Err(Error::Usage(format!(
            "'{reference}' matches {} pages ({}); use the page's uuid from `pages list`",
            many.len(),
            many.iter()
                .map(|p| field(p, "pageUuid"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The `pageUuid` a `<page>` argument names. A uuid is used as given, with
/// no request; a slug is looked up in `GET /pages`.
pub fn resolve_page(ctx: &Context, reference: &str) -> Result<String> {
    let reference = reference.trim();
    if is_uuid(reference) {
        return Ok(reference.to_ascii_lowercase());
    }
    if reference.is_empty() {
        return Err(Error::Usage(
            "a page is its uuid or its slug; use / or home for the home page".into(),
        ));
    }
    let pages = items(&ctx.client.get(PAGES, &Query::new())?.body);
    let page = find_page(&pages, reference)?;
    page.get("pageUuid")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Other("the API listed a page without a pageUuid".into()))
}

/// `pages/{pageUuid}` for a `<page>` argument.
pub fn page_path(ctx: &Context, reference: &str) -> Result<String> {
    Ok(format!("{PAGES}/{}", resolve_page(ctx, reference)?))
}

/// `pages/{pageUuid}/blocks/{blockInstanceUuid}` for `<page> <block>`.
pub fn block_path(ctx: &Context, page: &str, block: &str) -> Result<String> {
    let block = segment("the block uuid", block)?;
    Ok(format!("{}/blocks/{block}", page_path(ctx, page)?))
}

/// JSON given inline, as `@file`, or `-` for stdin.
pub fn read_json(source: &str) -> Result<Value> {
    let text = if let Some(path) = source.strip_prefix('@') {
        sources::read(path)?
    } else if source == "-" {
        sources::read("-")?
    } else {
        source.to_string()
    };
    Ok(serde_json::from_str(&text)?)
}

/// What to change, for the commands that take `key=value` arguments.
#[derive(Debug, clap::Args)]
pub struct FieldArgs {
    /// A field to change: key=value, repeatable. The value is JSON when it
    /// parses as JSON (`true`, `null`, `3`, `["a"]`), else text. Wins over
    /// the same field in --data
    #[arg(value_name = "KEY=VALUE")]
    pub fields: Vec<String>,
    /// Fields to change as a JSON object: inline, `@file`, or `-` for stdin
    #[arg(short = 'd', long, value_name = "JSON")]
    pub data: Option<String>,
}

impl FieldArgs {
    /// The fields given, or a usage error when there are none.
    pub fn changes(&self) -> Result<Map<String, Value>> {
        block_data(self.data.as_deref(), &self.fields)?.ok_or_else(|| {
            Error::Usage("nothing to change: give key=value fields or --data".into())
        })
    }
}

/// Write `changes` over `target`: objects merge key by key, anything else
/// (arrays and null included) replaces what was there.
pub fn merge(target: &mut Map<String, Value>, changes: Map<String, Value>) {
    for (key, value) in changes {
        match (target.get_mut(&key), value) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => merge(existing, incoming),
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

/// Block data from `--data` and `--set`: the `--data` object with each
/// `--set key=value` written over it. `None` when neither flag was given.
pub fn block_data(data: Option<&str>, sets: &[String]) -> Result<Option<Map<String, Value>>> {
    if data.is_none() && sets.is_empty() {
        return Ok(None);
    }
    let mut object = match data.map(read_json).transpose()? {
        None => Map::new(),
        Some(Value::Object(map)) => map,
        Some(_) => return Err(Error::Usage("--data must be a JSON object".into())),
    };
    for set in sets {
        let (key, value) = set
            .split_once('=')
            .filter(|(key, _)| key.split('.').all(|part| !part.is_empty()))
            .ok_or_else(|| Error::Usage(format!("expected key=value, got '{set}'")))?;
        // JSON when it is JSON (3, true, null, [..], {..}, "quoted"); else the text itself
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()));
        set_path(&mut object, key, value);
    }
    Ok(Some(object))
}

/// Set `a.b.c` in `object`, making (or replacing a non-object with) the
/// objects on the way.
fn set_path(object: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            object.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            let child = object
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !child.is_object() {
                *child = Value::Object(Map::new());
            }
            if let Value::Object(map) = child {
                set_path(map, rest, value);
            }
        }
    }
}

/// Ask before something that cannot simply be taken back, when a human is
/// at the keyboard. `-y` skips the question, and non-interactive callers
/// (agents, scripts) proceed without one, as in geekcli.
pub fn confirm(ctx: &Context, action: &str) -> Result<()> {
    if ctx.yes || !io::stdin().is_terminal() {
        return Ok(());
    }
    eprint!("{}? [y/N] ", capitalize(action));
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(Error::Usage(format!("did not {action}: not confirmed")))
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ABOUT: &str = "0b0e7c1e-6f0a-4b7e-9d2e-3f1a2b3c4d5e";
    const HOME: &str = "11111111-1111-4111-8111-111111111111";

    fn pages() -> Vec<Value> {
        items(&json!([
            {"pageUuid": ABOUT, "slug": "about"},
            {"pageUuid": HOME, "slug": ""},
            {"pageUuid": "22222222-2222-4222-8222-222222222222", "slug": "areas/downtown"},
        ]))
    }

    fn uuid_of(reference: &str) -> Option<String> {
        find_page(&pages(), reference).ok().and_then(|p| {
            p.get("pageUuid")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    }

    #[test]
    fn identifiers_cannot_change_the_path() {
        assert_eq!(
            segment("pageUuid", &format!(" {ABOUT} ")).ok(),
            Some(ABOUT.into())
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

    #[test]
    fn tells_a_uuid_from_a_slug() {
        assert!(is_uuid(ABOUT));
        assert!(is_uuid(&ABOUT.to_ascii_uppercase()));
        for slug in [
            "about",
            "home",
            "",
            "0b0e7c1e-6f0a-4b7e-9d2e",
            "0b0e7c1e6f0a4b7e9d2e3f1a2b3c4d5e",
        ] {
            assert!(!is_uuid(slug), "{slug}");
        }
    }

    #[test]
    fn a_page_is_found_by_uuid_slug_or_home() {
        assert_eq!(uuid_of(ABOUT).as_deref(), Some(ABOUT));
        for reference in ["about", "/about", "/about/", " about "] {
            assert_eq!(uuid_of(reference).as_deref(), Some(ABOUT), "{reference}");
        }
        for home in ["/", "home", "/home/"] {
            assert_eq!(uuid_of(home).as_deref(), Some(HOME), "{home}");
        }
        assert_eq!(
            uuid_of("/areas/downtown/").as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
    }

    #[test]
    fn a_page_called_home_wins_over_the_home_page() {
        let mut pages = pages();
        pages.push(json!({"pageUuid": "33333333-3333-4333-8333-333333333333", "slug": "home"}));
        let found = find_page(&pages, "home").ok();
        assert_eq!(
            found.as_ref().and_then(|p| p.get("slug")),
            Some(&json!("home"))
        );
    }

    #[test]
    fn an_unknown_page_is_not_found_and_lists_the_slugs() {
        let err = find_page(&pages(), "contact").err();
        assert_eq!(err.as_ref().map(Error::exit_code), Some(4));
        assert!(err
            .map(|e| e.message())
            .is_some_and(|m| m.contains("/, /about, /areas/downtown")));
        let unknown_uuid = find_page(&pages(), "99999999-9999-4999-8999-999999999999").err();
        assert_eq!(unknown_uuid.map(|e| e.exit_code()), Some(4));
    }

    #[test]
    fn an_ambiguous_page_is_a_usage_error() {
        let mut pages = pages();
        pages.push(json!({"pageUuid": "44444444-4444-4444-8444-444444444444", "slug": "/about/"}));
        assert!(matches!(
            find_page(&pages, "about"),
            Err(Error::Usage(m)) if m.contains("matches 2 pages") && m.contains(ABOUT)
        ));
    }

    fn data(data: Option<&str>, sets: &[&str]) -> Result<Option<Value>> {
        let sets: Vec<String> = sets.iter().map(ToString::to_string).collect();
        Ok(block_data(data, &sets)?.map(Value::Object))
    }

    #[test]
    fn set_values_are_json_when_they_parse_and_text_otherwise() {
        let built = data(
            None,
            &[
                "title=Welcome home",
                "columns=3",
                "isCentered=true",
                "subtitle=null",
                "tags=[\"a\",\"b\"]",
                "zip=\"02134\"",
                "note=a=b",
                "empty=",
            ],
        );
        assert_eq!(
            built.ok().flatten(),
            Some(json!({
                "title": "Welcome home",
                "columns": 3,
                "isCentered": true,
                "subtitle": null,
                "tags": ["a", "b"],
                "zip": "02134",
                "note": "a=b",
                "empty": "",
            }))
        );
    }

    #[test]
    fn dotted_keys_nest_and_set_wins_over_data() {
        let built = data(
            Some(
                r#"{"title":"From data","primaryButton":{"label":"Old","url":"/x"},"layout":"split"}"#,
            ),
            &[
                "title=From set",
                "primaryButton.label=Go",
                "secondaryButton.style.color=#ff0000",
                "layout.kind=wide",
            ],
        );
        assert_eq!(
            built.ok().flatten(),
            Some(json!({
                "title": "From set",
                "primaryButton": {"label": "Go", "url": "/x"},
                "secondaryButton": {"style": {"color": "#ff0000"}},
                "layout": {"kind": "wide"},
            }))
        );
    }

    #[test]
    fn data_must_be_an_object_and_set_must_have_a_key() {
        assert!(matches!(data(None, &[]), Ok(None)));
        assert!(matches!(data(Some("{}"), &[]), Ok(Some(_))));
        assert!(
            matches!(data(Some("[1]"), &[]), Err(Error::Usage(m)) if m.contains("JSON object"))
        );
        assert!(matches!(data(Some("{nope"), &[]), Err(Error::Usage(_))));
        for bad in ["novalue", "=x", "a..b=1", ".a=1", "a.=1"] {
            assert!(
                matches!(data(None, &[bad]), Err(Error::Usage(m)) if m.contains("key=value")),
                "{bad}"
            );
        }
    }
}
