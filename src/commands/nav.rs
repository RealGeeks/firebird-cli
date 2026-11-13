//! `firebird-cli nav …` — navigation bars, their items, and the page
//! sections a link can point at.
//!
//! A bar is read whole and saved whole: `GET /navigation/{navBarUuid}`
//! returns nested `items`, and `PUT` on the same path takes that list back
//! and replaces every item on the bar.

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::pages::PAGE_HELP;
use super::settings::undo_note;
use super::{confirm, items, read_json, resolve_page, segment, Context};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{cell, col, Column, Format};

pub const PATH: &str = "navigation";

/// The site's default bar, in place of a navBarUuid in the path.
pub const DEFAULT_BAR: &str = "default";

/// The `resourceType` of a bar in the revision history.
const HISTORY: &str = "navigation";

pub const BAR_COLUMNS: &[Column] = &[
    col("navBarUuid", "/navBarUuid"),
    col("name", "/name"),
    col("isSiteDefault", "/isSiteDefault"),
    col("pageUuids", "/pageUuids"),
];

/// The columns of [`rows`]: a bar's items flattened for a table.
pub const ITEM_COLUMNS: &[Column] = &[
    col("navItemUuid", "/navItemUuid"),
    col("label", "/label"),
    col("parent", "/parent"),
    col("linkType", "/linkType"),
    col("target", "/target"),
];

pub const ANCHOR_COLUMNS: &[Column] = &[
    col("blockInstanceUuid", "/blockInstanceUuid"),
    col("label", "/label"),
];

#[derive(Debug, Args)]
pub struct NavCommand {
    #[command(subcommand)]
    pub command: NavSub,
}

#[derive(Debug, Args)]
pub struct SetArgs {
    /// The bar to replace: its navBarUuid from `nav list`, or `default` for the site's default bar
    #[arg(value_name = "NAV_BAR")]
    pub nav_bar: String,
    /// The bar's items as a JSON object: inline, `@file`, or `-` for stdin.
    /// Either what `nav get -o json` prints, or just {"items": [...]}
    #[arg(short = 'd', long, value_name = "JSON")]
    pub data: String,
    /// Move the bar to this page (sends its `pageUuid`)
    #[arg(long, value_name = "PAGE", conflicts_with = "detach")]
    pub page: Option<String>,
    /// Take the bar off the page it is on (sends `pageUuid: null`)
    #[arg(long)]
    pub detach: bool,
}

#[derive(Debug, Subcommand)]
pub enum NavSub {
    /// List the site's navigation bars and the pages each applies to (`GET /navigation`)
    #[command(after_help = "Notes:
  - The result is {defaultNavBarUuid, navBars}. Each bar has `navBarUuid`, `name`, `isSiteDefault` and the `pageUuids` it applies to; a page on no bar uses the site default.
  - This lists bars only. `nav get <NAV_BAR>` reads one bar's items.
  - A table and `-q` show the bars; `-o json` has everything.")]
    List,
    /// Show one navigation bar and its items (`GET /navigation/{navBarUuid}`)
    #[command(after_help = "Notes:
  - NAV_BAR is a navBarUuid from `nav list`, or `default` for the site's default bar. With no argument it is `default`.
  - The result is {navBarUuid, name, isSiteDefault, pageUuids, items}. `items` is nested and in display order: a link, or a dropdown (`linkType` \"folder\") with its links under `children`, one level deep.
  - A link's `linkType` is customPage, standardPage, externalUrl or anchor, with its target in `customPageUuid`, `standardPageUuid`, `externalUrl` or `pageSection`. `pageSlug`, `pageStatus` and `standardPagePath` are read-only.
  - --page shows the bar that page uses: its own, else the site default. That is two calls: `GET /navigation` to find the bar listing the page, then the read.
  - An unknown navBarUuid is 404 (exit 4), and so is `default` on a site that has no bar yet.
  - `-o json` is the API's response, and what `nav set` takes back. A table shows the items flattened, a dropdown's links with its label under `parent`; `-q` prints every navItemUuid.")]
    Get {
        /// The bar to show: its navBarUuid, or `default` for the site's default bar
        #[arg(value_name = "NAV_BAR", conflicts_with = "page")]
        nav_bar: Option<String>,
        /// Show the bar this page uses: its own, else the site default
        #[arg(long, value_name = "PAGE")]
        page: Option<String>,
    },
    /// Replace every item on a navigation bar (`PUT /navigation/{navBarUuid}`)
    #[command(after_help = "Notes:
  - This changes the live site at once; it is not part of a page's draft. Asks first on a terminal; -y skips the question.
  - It REPLACES the bar: an item left out of `items` is deleted. Read the bar with `nav get <NAV_BAR> -o json`, edit `items`, and pass the result back with --data. The read-only fields in it are ignored.
  - Only `items` is sent, and `pageUuid` when you gave one. The `navBarUuid`, `name`, `isSiteDefault` and `pageUuids` of a read are not sent, so the read can be passed back whole.
  - A new item has `navItemUuid: null` and any `key` not used by another item in the same request. An existing item keeps its `navItemUuid`.
  - `pageUuid` only matters for moving a bar. Without it the bar stays on the page it is on. --page <PAGE> moves the bar to that page, --detach takes it off its page; a `pageUuid` key in --data does the same (a uuid or null), and the flags win over it. The site default cannot be moved to a page (400, exit 5).
  - `default` saves the site's default bar and creates it when the site has none. An unknown navBarUuid is 404 (exit 4).
  - 409 (exit 6) means the navigation changed elsewhere since you read it: run `nav get` again, reapply the change and save.
  - The result is {navBarUuid, revisionUuid}: the revision this save recorded (`-q` prints it). Undo with `revisions restore` of the revision before it; on a terminal the note after the save names it.")]
    Set(SetArgs),
    /// List the sections of a page a link can scroll to (`GET /navigation/anchor-targets`)
    Anchors {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
}

pub fn run(ctx: &Context, cmd: &NavCommand) -> Result<()> {
    match &cmd.command {
        NavSub::List => {
            let body = ctx.client.get(PATH, &Query::new())?.body;
            ctx.printer.wrapped(&body, "/navBars", BAR_COLUMNS)
        }
        NavSub::Get { nav_bar, page } => {
            let bar = if let Some(page) = page {
                let page = resolve_page(ctx, page)?;
                let navigation = ctx.client.get(PATH, &Query::new())?.body;
                segment("the navBarUuid", &bar_for_page(&navigation, &page))?
            } else {
                bar_segment(nav_bar.as_deref().unwrap_or(DEFAULT_BAR))?
            };
            let body = ctx
                .client
                .get(&format!("{PATH}/{bar}"), &Query::new())?
                .body;
            if ctx.printer.quiet || ctx.printer.format == Format::Table {
                ctx.printer.note(&describe(&body));
                ctx.printer.list(&rows(&body), ITEM_COLUMNS)
            } else {
                ctx.printer.wrapped(&body, "/items", ITEM_COLUMNS)
            }
        }
        NavSub::Set(args) => set(ctx, args),
        NavSub::Anchors { page } => {
            let query: Query = vec![("pageUuid".into(), resolve_page(ctx, page)?)];
            let response = ctx.client.get(&format!("{PATH}/anchor-targets"), &query)?;
            ctx.printer.list(&items(&response.body), ANCHOR_COLUMNS)
        }
    }
}

fn set(ctx: &Context, args: &SetArgs) -> Result<()> {
    let bar = bar_segment(&args.nav_bar)?;
    let page = if args.detach {
        Some(Value::Null)
    } else {
        args.page
            .as_deref()
            .map(|page| resolve_page(ctx, page).map(Value::String))
            .transpose()?
    };
    let input = read_json(&args.data)?;
    refuse_another_bars_items(&input, &bar)?;
    let body = save_body(&input, page)?;
    confirm(
        ctx,
        &format!("replace every item on navigation bar {bar} on the live site"),
    )?;
    let saved = ctx
        .client
        .put(&format!("{PATH}/{bar}"), &Value::Object(body))?
        .body;

    if ctx.printer.format == Format::Table && !ctx.printer.quiet {
        let recorded = saved.get("revisionUuid").is_some_and(|r| !r.is_null());
        let key = saved.get("navBarUuid").map(cell).filter(|k| !k.is_empty());
        ctx.printer.note(&if recorded {
            undo_note(ctx, HISTORY, key.as_deref())
        } else {
            "Live now. This save recorded no revision.".to_string()
        });
    }
    ctx.printer.one(&saved, "/revisionUuid")
}

/// A save replaces every item, so the output of `nav get` for one bar must
/// not land on another. Data with no `navBarUuid` is not a read and passes.
fn refuse_another_bars_items(input: &Value, bar: &str) -> Result<()> {
    let Some(read_from) = input.get("navBarUuid").and_then(Value::as_str) else {
        return Ok(());
    };
    let same = if bar == DEFAULT_BAR {
        input.get("isSiteDefault").and_then(Value::as_bool) != Some(false)
    } else {
        read_from.eq_ignore_ascii_case(bar)
    };
    if same {
        return Ok(());
    }
    Err(Error::Usage(format!(
        "--data is navigation bar {read_from}, not {bar}, and saving replaces every item on {bar}. To copy its items on purpose, remove `navBarUuid` from the data"
    )))
}

/// A `NAV_BAR` argument as a path segment: a navBarUuid or `default`.
fn bar_segment(nav_bar: &str) -> Result<String> {
    let bar = segment("the navBarUuid", nav_bar)?;
    Ok(if bar.eq_ignore_ascii_case(DEFAULT_BAR) {
        DEFAULT_BAR.to_string()
    } else {
        bar
    })
}

/// The bar `page` uses, from `GET /navigation`: the one listing it among its
/// `pageUuids`, else the site default.
fn bar_for_page(navigation: &Value, page: &str) -> String {
    items(navigation.get("navBars").unwrap_or(&Value::Null))
        .iter()
        .find(|bar| {
            items(bar.get("pageUuids").unwrap_or(&Value::Null))
                .iter()
                .any(|uuid| uuid.as_str().is_some_and(|u| u.eq_ignore_ascii_case(page)))
        })
        .and_then(|bar| bar.get("navBarUuid").and_then(Value::as_str))
        .unwrap_or(DEFAULT_BAR)
        .to_string()
}

/// The body of a save from what the caller gave: its `items` untouched, and
/// `pageUuid` only when asked for, by `page` (the flags) or else by a
/// `pageUuid` key in the input. A read's `pageUuids` is never a `pageUuid`.
fn save_body(input: &Value, page: Option<Value>) -> Result<Map<String, Value>> {
    let items = input
        .get("items")
        .filter(|items| items.is_array())
        .ok_or_else(|| {
            Error::Usage(
                "--data must be a JSON object with an `items` array: the output of `nav get -o json`, or {\"items\": [...]}"
                    .into(),
            )
        })?;
    let mut body = Map::new();
    body.insert("items".into(), items.clone());
    if let Some(page) = page.or_else(|| input.get("pageUuid").cloned()) {
        if !(page.is_null() || page.is_string()) {
            return Err(Error::Usage(
                "`pageUuid` in --data must be a page's uuid or null".into(),
            ));
        }
        body.insert("pageUuid".into(), page);
    }
    Ok(body)
}

/// A bar's items as table rows, in display order: each top-level item, then
/// a dropdown's links with the dropdown's label as their `parent`.
fn rows(bar: &Value) -> Vec<Value> {
    let mut rows = Vec::new();
    for item in items(bar.get("items").unwrap_or(&Value::Null)) {
        rows.push(row(&item, None));
        for child in items(item.get("children").unwrap_or(&Value::Null)) {
            rows.push(row(&child, item.get("label")));
        }
    }
    rows
}

fn row(item: &Value, parent: Option<&Value>) -> Value {
    json!({
        "navItemUuid": item.get("navItemUuid"),
        "label": item.get("label"),
        "parent": parent,
        "linkType": item.get("linkType"),
        "target": target(item),
    })
}

/// Where a link goes, as a person reads it: the page's slug or path, the
/// URL, or the page section.
fn target(item: &Value) -> Value {
    let field = |name: &str| item.get(name).filter(|value| !value.is_null()).cloned();
    match item.get("linkType").and_then(Value::as_str) {
        Some("customPage") => field("pageSlug")
            .map(|slug| Value::String(format!("/{}", cell(&slug).trim_matches('/'))))
            .or_else(|| field("customPageUuid")),
        Some("standardPage") => field("standardPagePath").or_else(|| field("standardPageUuid")),
        Some("externalUrl") => field("externalUrl"),
        Some("anchor") => field("pageSection"),
        _ => None,
    }
    .unwrap_or(Value::Null)
}

/// One line saying which bar a table shows.
fn describe(bar: &Value) -> String {
    let text = |name: &str| bar.get(name).map(cell).unwrap_or_default();
    let place = if bar.get("isSiteDefault") == Some(&Value::Bool(true)) {
        "the site default".to_string()
    } else {
        match text("pageUuids") {
            pages if pages.is_empty() => "on no page".to_string(),
            pages => format!("on page {pages}"),
        }
    };
    format!(
        "navigation bar {} ({}), {place}",
        text("navBarUuid"),
        text("name")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_uses_its_own_bar_or_the_default() {
        let navigation = json!({
            "defaultNavBarUuid": "site-bar",
            "navBars": [
                {"navBarUuid": "home-bar", "pageUuids": ["p-home"]},
                {"navBarUuid": "site-bar", "isSiteDefault": true, "pageUuids": []},
            ],
        });
        assert_eq!(bar_for_page(&navigation, "p-home"), "home-bar");
        assert_eq!(bar_for_page(&navigation, "P-HOME"), "home-bar");
        assert_eq!(bar_for_page(&navigation, "p-about"), DEFAULT_BAR);
        assert_eq!(bar_for_page(&json!({}), "p-home"), DEFAULT_BAR);
    }

    #[test]
    fn default_is_the_default_bar_whatever_its_case() {
        assert_eq!(bar_segment(" Default ").ok().as_deref(), Some(DEFAULT_BAR));
        assert_eq!(bar_segment("home-bar").ok().as_deref(), Some("home-bar"));
        assert!(matches!(bar_segment("a/b"), Err(Error::Usage(_))));
    }

    #[test]
    fn a_read_of_one_bar_is_not_saved_onto_another() {
        assert!(refuse_another_bars_items(&read(), "home-bar").is_ok());
        assert!(refuse_another_bars_items(&read(), "HOME-BAR").is_ok());
        assert!(matches!(
            refuse_another_bars_items(&read(), "other-bar"),
            Err(Error::Usage(m)) if m.contains("home-bar") && m.contains("other-bar")
        ));
        // a page's bar is not the site default; the default's own read is
        assert!(refuse_another_bars_items(&read(), DEFAULT_BAR).is_err());
        let site_default = json!({"navBarUuid": "d", "isSiteDefault": true, "items": []});
        assert!(refuse_another_bars_items(&site_default, DEFAULT_BAR).is_ok());
        // hand-written data names no bar
        assert!(refuse_another_bars_items(&json!({"items": []}), "other-bar").is_ok());
    }

    fn read() -> Value {
        json!({
            "navBarUuid": "home-bar",
            "name": "Home bar",
            "isSiteDefault": false,
            "pageUuids": ["p-home"],
            "items": [
                {"key": "i1", "navItemUuid": "i1", "label": "About", "linkType": "customPage",
                 "customPageUuid": "p-about", "pageSlug": "about", "pageStatus": "active"},
                {"key": "i2", "navItemUuid": "i2", "label": "More", "linkType": "folder", "children": [
                    {"key": "i3", "navItemUuid": "i3", "label": "Search", "linkType": "standardPage",
                     "standardPageUuid": "sp-1", "standardPagePath": "/search"},
                    {"key": "i4", "navItemUuid": "i4", "label": "Blog", "linkType": "externalUrl",
                     "externalUrl": "https://blog.example.com"},
                ]},
                {"key": "i5", "navItemUuid": "i5", "label": "Contact", "linkType": "anchor",
                 "pageSection": "contact"},
            ],
        })
    }

    #[test]
    fn a_read_is_saved_as_its_items_and_nothing_else() {
        let body = save_body(&read(), None).ok().map(Value::Object);
        assert_eq!(body, Some(json!({"items": read()["items"]})));
    }

    #[test]
    fn page_uuid_is_sent_only_when_asked_for() {
        let with_key = |page: Value| {
            let mut input = read();
            input["pageUuid"] = page;
            input
        };
        let sent = |input: &Value, page: Option<Value>| {
            save_body(input, page)
                .ok()
                .and_then(|body| body.get("pageUuid").cloned())
        };
        assert_eq!(sent(&read(), None), None);
        assert_eq!(sent(&with_key(Value::Null), None), Some(Value::Null));
        assert_eq!(sent(&with_key(json!("p-1")), None), Some(json!("p-1")));
        // the flags win over the key
        assert_eq!(
            sent(&with_key(json!("p-1")), Some(Value::Null)),
            Some(Value::Null)
        );
        assert_eq!(sent(&read(), Some(json!("p-2"))), Some(json!("p-2")));
        assert!(matches!(
            save_body(&with_key(json!(["p-1"])), None),
            Err(Error::Usage(m)) if m.contains("pageUuid")
        ));
    }

    #[test]
    fn the_input_must_hold_an_items_array() {
        for bad in [json!([]), json!({}), json!({"items": {}}), json!("items")] {
            assert!(
                matches!(save_body(&bad, None), Err(Error::Usage(m)) if m.contains("items")),
                "{bad}"
            );
        }
        let empty = save_body(&json!({"items": []}), None)
            .ok()
            .map(Value::Object);
        assert_eq!(empty, Some(json!({"items": []})));
    }

    #[test]
    fn rows_flatten_a_dropdown_and_name_each_target() {
        let cells: Vec<Vec<String>> = rows(&read())
            .iter()
            .map(|row| {
                ITEM_COLUMNS
                    .iter()
                    .map(|c| row.pointer(c.pointer).map(cell).unwrap_or_default())
                    .collect()
            })
            .collect();
        let expected = [
            ["i1", "About", "", "customPage", "/about"],
            ["i2", "More", "", "folder", ""],
            ["i3", "Search", "More", "standardPage", "/search"],
            [
                "i4",
                "Blog",
                "More",
                "externalUrl",
                "https://blog.example.com",
            ],
            ["i5", "Contact", "", "anchor", "contact"],
        ];
        assert_eq!(cells, expected);
        assert!(rows(&json!({})).is_empty());
    }

    #[test]
    fn a_table_says_which_bar_it_shows() {
        assert_eq!(
            describe(&read()),
            "navigation bar home-bar (Home bar), on page p-home"
        );
        assert_eq!(
            describe(&json!({"navBarUuid": "b", "name": "Main", "isSiteDefault": true})),
            "navigation bar b (Main), the site default"
        );
    }
}
