//! `firebird-cli pages …` — the site's pages and the blocks on one.

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::{confirm, find_page, items, page_path, Context, PAGES};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{cell, col, Column, Format};

pub const COLUMNS: &[Column] = &[
    col("pageUuid", "/pageUuid"),
    col("slug", "/slug"),
    col("status", "/status"),
    col("metaTitle", "/metaTitle"),
    col("updatedDate", "/updatedDate"),
];

pub const BLOCK_COLUMNS: &[Column] = &[
    col("blockInstanceUuid", "/blockInstanceUuid"),
    col("codeName", "/codeName"),
    col("sortOrder", "/sortOrder"),
    col("parentBlockInstanceUuid", "/parentBlockInstanceUuid"),
    col("data", "/data"),
];

/// What `<page>` accepts, for `--help`.
pub const PAGE_HELP: &str =
    "The page: its uuid, or its slug (about, /about/; / or home for the home page)";

const NO_HISTORY: &str =
    "Page settings keep no history: there is no undo, so note the old values first (`pages get`).";

/// A page's settings, named after the API's fields.
#[derive(Debug, Args)]
pub struct PageFields {
    /// The page's path: lowercase letters, digits and hyphens, with `/`
    /// between parts (`about`, `areas/downtown`)
    #[arg(long, value_name = "SLUG")]
    pub slug: Option<String>,
    /// The title for search engines and the browser tab
    #[arg(long, value_name = "TEXT")]
    pub meta_title: Option<String>,
    /// The description for search engines
    #[arg(long, value_name = "TEXT")]
    pub meta_description: Option<String>,
    /// Keywords for search engines
    #[arg(long, value_name = "TEXT")]
    pub meta_keywords: Option<String>,
}

/// The flags and the API field each one sets.
const META: [&str; 3] = ["metaTitle", "metaDescription", "metaKeywords"];

impl PageFields {
    fn meta(&self) -> [&Option<String>; 3] {
        [
            &self.meta_title,
            &self.meta_description,
            &self.meta_keywords,
        ]
    }

    /// The slug as the API takes it: no slashes at either end.
    fn slug(&self) -> Option<String> {
        self.slug
            .as_deref()
            .map(|slug| slug.trim().trim_matches('/').to_string())
    }
}

/// The body for `PUT /pages/{pageUuid}`. The API clears any meta field the
/// body leaves out and requires the slug, so every setting the caller did
/// not give is carried over from `current`. A meta flag given as an empty
/// string is left out, which is how the API clears it.
pub fn update_body(
    current: &Value,
    fields: &PageFields,
    status: Option<&str>,
) -> Map<String, Value> {
    let mut body = Map::new();
    let slug = fields.slug().map_or_else(
        || current.get("slug").cloned().unwrap_or_else(|| json!("")),
        Value::String,
    );
    body.insert("slug".into(), slug);
    for (field, given) in META.iter().zip(fields.meta()) {
        let value = match given {
            Some(text) => Some(json!(text)),
            None => current.get(*field).cloned(),
        };
        if let Some(value) = value.filter(|v| v.as_str().is_some_and(|text| !text.is_empty())) {
            body.insert((*field).to_string(), value);
        }
    }
    if let Some(status) = status {
        body.insert("status".into(), json!(status));
    }
    body
}

#[derive(Debug, Args)]
pub struct PagesCommand {
    #[command(subcommand)]
    pub command: PagesSub,
}

#[derive(Debug, Subcommand)]
pub enum PagesSub {
    /// List the site's pages (`GET /pages`)
    List,
    /// Show one page's settings: slug, status, meta title and description
    #[command(after_help = "Notes:
  - The API has no route for one page's settings, so this reads `GET /pages` and picks the page out of it.
  - An unknown page is exit 4, with the site's slugs in the message.")]
    Get {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
    /// Show a page's blocks (`GET /pages/{pageUuid}/content`)
    #[command(after_help = "Notes:
  - The result is {initialEditorMode, blocks}: the page as the key's user would open it in the editor. While the page has a draft that this user may edit, these are the draft's blocks, under the draft's ids.
  - A table shortens each block's data; use `-o json` for all of it, and `-q` for just the block instance uuids.
  - See `firebird-cli guide drafts` for how live and draft block ids relate.")]
    Content {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
    /// Create a page (`POST /pages`)
    #[command(after_help = format!("Notes:
  - Needs --slug. The slug must not be in use (409, exit 6), and the site must be allowed more than one page (403, exit 3, if not).
  - The page exists on the site at once, with no blocks; add them with `blocks add`, which starts its draft. `-q` prints the new pageUuid.
  - {NO_HISTORY}"))]
    Create(PageFields),
    /// Change a page's settings: slug, meta fields, status (`PUT /pages/{pageUuid}`)
    #[command(after_help = format!("Notes:
  - Live at once. Asks first on a terminal; -y skips the question. This is not publishing: it does not make a draft's block changes live (`draft publish` does).
  - The API replaces the settings whole and clears any meta field left out, so this command reads the page first and sends back every setting you did not pass. Two writers doing that at once can undo each other.
  - Pass an empty value to clear a meta field: --meta-keywords \"\".
  - --status is active or unpublished. Setting a page active is refused (403, exit 3) when the site's owner handles publishing; the home page cannot be unpublished (400, exit 5). A slug in use is 409 (exit 6).
  - {NO_HISTORY}"))]
    Update {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        #[command(flatten)]
        fields: PageFields,
        /// active or unpublished
        #[arg(long, value_name = "STATUS")]
        status: Option<String>,
    },
}

pub fn run(ctx: &Context, cmd: &PagesCommand) -> Result<()> {
    match &cmd.command {
        PagesSub::List => {
            let response = ctx.client.get(PAGES, &Query::new())?;
            ctx.printer.list(&items(&response.body), COLUMNS)
        }
        PagesSub::Get { page } => {
            let pages = items(&ctx.client.get(PAGES, &Query::new())?.body);
            ctx.printer.one(&find_page(&pages, page)?, "/pageUuid")
        }
        PagesSub::Create(fields) => {
            let Some(slug) = fields.slug().filter(|slug| !slug.is_empty()) else {
                return Err(Error::Usage("a new page needs a --slug".into()));
            };
            let mut body = Map::new();
            body.insert("slug".into(), json!(slug));
            for (field, given) in META.iter().zip(fields.meta()) {
                if let Some(text) = given.as_deref().filter(|text| !text.is_empty()) {
                    body.insert((*field).to_string(), json!(text));
                }
            }
            let created = ctx.client.post(PAGES, Some(&Value::Object(body)))?.body;
            ctx.printer.note("Created. Page settings keep no history.");
            ctx.printer.one(&created, "/pageUuid")
        }
        PagesSub::Update {
            page,
            fields,
            status,
        } => {
            if fields.slug.is_none()
                && status.is_none()
                && fields.meta().iter().all(|m| m.is_none())
            {
                return Err(Error::Usage(
                    "nothing to change: give --slug, --meta-title, --meta-description, --meta-keywords or --status".into(),
                ));
            }
            let pages = items(&ctx.client.get(PAGES, &Query::new())?.body);
            let current = find_page(&pages, page)?;
            let uuid = current
                .get("pageUuid")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Other("the API listed a page without a pageUuid".into()))?;
            let body = update_body(&current, fields, status.as_deref());
            confirm(
                ctx,
                &format!("change the settings of page {page} on the live site"),
            )?;
            let updated = ctx
                .client
                .put(&format!("{PAGES}/{uuid}"), &Value::Object(body))?
                .body;
            ctx.printer
                .note("Live now. Page settings keep no history, so there is no undo.");
            ctx.printer.one(&updated, "/pageUuid")
        }
        PagesSub::Content { page } => {
            let path = format!("{}/content", page_path(ctx, page)?);
            let body = ctx.client.get(&path, &Query::new())?.body;
            if let Some(mode) = body.pointer("/initialEditorMode/mode") {
                if ctx.printer.format == Format::Table {
                    ctx.printer.note(&format!("editor mode: {}", cell(mode)));
                }
            }
            ctx.printer.wrapped(&body, "/blocks", BLOCK_COLUMNS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> PageFields {
        PageFields {
            slug: None,
            meta_title: None,
            meta_description: None,
            meta_keywords: None,
        }
    }

    fn about() -> Value {
        json!({
            "pageUuid": "p1",
            "siteUuid": "s1",
            "slug": "about",
            "metaTitle": "About Us",
            "metaDescription": "Who we are",
            "metaKeywords": null,
            "status": "active",
            "pageTypeId": null,
            "updatedDate": "2026-01-01T00:00:00.000Z",
        })
    }

    #[test]
    fn settings_not_given_are_carried_over() {
        let given = PageFields {
            meta_title: Some("About the team".into()),
            ..fields()
        };
        assert_eq!(
            Value::Object(update_body(&about(), &given, None)),
            json!({"slug": "about", "metaTitle": "About the team", "metaDescription": "Who we are"})
        );
    }

    #[test]
    fn an_empty_value_clears_and_a_slug_loses_its_slashes() {
        let given = PageFields {
            slug: Some("/about-us/".into()),
            meta_description: Some(String::new()),
            meta_keywords: Some("team, story".into()),
            ..fields()
        };
        assert_eq!(
            Value::Object(update_body(&about(), &given, Some("unpublished"))),
            json!({
                "slug": "about-us",
                "metaTitle": "About Us",
                "metaKeywords": "team, story",
                "status": "unpublished",
            })
        );
    }

    #[test]
    fn the_home_page_keeps_its_empty_slug() {
        let home = json!({"pageUuid": "p0", "slug": "", "metaTitle": null});
        let given = PageFields {
            meta_title: Some("Home".into()),
            ..fields()
        };
        assert_eq!(
            Value::Object(update_body(&home, &given, None)),
            json!({"slug": "", "metaTitle": "Home"})
        );
    }
}
