//! `firebird-cli nav …` — navigation bars and the page sections a link can
//! point at. Read only for now.

use clap::{Args, Subcommand};
use serde_json::Value;

use super::pages::PAGE_HELP;
use super::{items, resolve_page, segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{cell, col, Column, Format};

pub const PATH: &str = "navigation";

pub const ITEM_COLUMNS: &[Column] = &[
    col("navItemUuid", "/navItemUuid"),
    col("label", "/label"),
    col("isFolder", "/isFolder"),
    col("parentNavItemUuid", "/parentNavItemUuid"),
    col("followsNavItemUuid", "/followsNavItemUuid"),
    col("pageSlug", "/pageSlug"),
    col("externalUrl", "/externalUrl"),
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

#[derive(Debug, Subcommand)]
pub enum NavSub {
    /// Show a navigation bar's items and every bar on the site (`GET /navigation`)
    #[command(after_help = "Notes:
  - The result is {navBarUuid, items, navBars}. `items` is a flat list: `parentNavItemUuid` and `followsNavItemUuid` give each item's place. `navBars` lists every bar with the `pageUuids` it applies to.
  - With no flag this is the site's default bar. A page can have its own bar; --page shows the one that page uses.
  - The API's only filter is `navBarUuid`, so --page is two calls: one to find the page's bar in `navBars`, one to read it.
  - A table and `-q` show the items; `-o json` has everything.")]
    Get {
        /// The bar to show, by its navBarUuid
        #[arg(long, value_name = "UUID", conflicts_with = "page")]
        nav_bar: Option<String>,
        /// Show the bar this page uses: its own, else the site default
        #[arg(long, value_name = "PAGE")]
        page: Option<String>,
    },
    /// List the sections of a page a link can scroll to (`GET /navigation/anchor-targets`)
    Anchors {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
}

pub fn run(ctx: &Context, cmd: &NavCommand) -> Result<()> {
    match &cmd.command {
        NavSub::Get { nav_bar, page } => {
            let mut bar = nav_bar
                .as_deref()
                .map(|uuid| segment("the navBarUuid", uuid))
                .transpose()?;
            let mut body = None;
            if let Some(page) = page {
                let page = resolve_page(ctx, page)?;
                let default = ctx.client.get(PATH, &Query::new())?.body;
                bar = bar_for_page(&default, &page);
                if bar.is_none() {
                    body = Some(default);
                }
            }
            let body = if let Some(body) = body {
                body
            } else {
                let query: Query = bar
                    .into_iter()
                    .map(|uuid| ("navBarUuid".to_string(), uuid))
                    .collect();
                ctx.client.get(PATH, &query)?.body
            };
            if ctx.printer.format == Format::Table {
                if let Some(uuid) = body.get("navBarUuid") {
                    ctx.printer.note(&format!("navigation bar {}", cell(uuid)));
                }
            }
            ctx.printer.wrapped(&body, "/items", ITEM_COLUMNS)
        }
        NavSub::Anchors { page } => {
            let query: Query = vec![("pageUuid".into(), resolve_page(ctx, page)?)];
            let response = ctx.client.get(&format!("{PATH}/anchor-targets"), &query)?;
            ctx.printer.list(&items(&response.body), ANCHOR_COLUMNS)
        }
    }
}

/// The bar that lists `page` among its `pageUuids`, unless it is the one
/// `navigation` already holds.
fn bar_for_page(navigation: &Value, page: &str) -> Option<String> {
    let shown = navigation.get("navBarUuid").and_then(Value::as_str);
    items(navigation.get("navBars").unwrap_or(&Value::Null))
        .iter()
        .find(|bar| {
            items(bar.get("pageUuids").unwrap_or(&Value::Null))
                .iter()
                .any(|uuid| uuid.as_str().is_some_and(|u| u.eq_ignore_ascii_case(page)))
        })
        .and_then(|bar| bar.get("navBarUuid").and_then(Value::as_str))
        .filter(|uuid| Some(*uuid) != shown)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_page_uses_its_own_bar_or_the_default() {
        let navigation = json!({
            "navBarUuid": "default",
            "items": [],
            "navBars": [
                {"navBarUuid": "home-bar", "pageUuids": ["p-home"]},
                {"navBarUuid": "default", "pageUuids": ["p-listed"]},
            ],
        });
        assert_eq!(
            bar_for_page(&navigation, "p-home").as_deref(),
            Some("home-bar")
        );
        assert_eq!(
            bar_for_page(&navigation, "P-HOME").as_deref(),
            Some("home-bar")
        );
        // already the bar that was read, or on no bar at all
        assert_eq!(bar_for_page(&navigation, "p-listed"), None);
        assert_eq!(bar_for_page(&navigation, "p-about"), None);
        assert_eq!(bar_for_page(&json!({}), "p-home"), None);
    }
}
