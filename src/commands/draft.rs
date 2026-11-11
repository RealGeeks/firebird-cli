//! `firebird-cli draft …` — a page's unpublished changes: what is waiting,
//! whether it can be published, publishing it and throwing it away.

use std::io::Write as _;

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::pages::PAGE_HELP;
use super::{confirm, items, page_path, Context, PAGES};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{col, publish_error_lines, Column, Format};

pub const SUMMARY_COLUMNS: &[Column] = &[
    col("pageUuid", "/pageUuid"),
    col("pageSlug", "/pageSlug"),
    col("pageTitle", "/pageTitle"),
    col("draftPageUuid", "/draftPageUuid"),
    col("isOwn", "/isOwn"),
    col("createdBy", "/createdBy"),
];

pub const PUBLISHED_COLUMNS: &[Column] = &[
    col("pageUuid", "/pageUuid"),
    col("slug", "/slug"),
    col("versionNumber", "/versionNumber"),
    col("liveUrl", "/liveUrl"),
    col("draftPageUuid", "/draftPageUuid"),
];

const BLOCK_ERROR_COLUMNS: &[Column] = &[
    col("blockInstanceUuid", "/blockInstanceUuid"),
    col("blockLabel", "/blockLabel"),
];

#[derive(Debug, Args)]
pub struct DraftCommand {
    #[command(subcommand)]
    pub command: DraftSub,
}

#[derive(Debug, Subcommand)]
pub enum DraftSub {
    /// List every page on the site with unpublished changes (`GET /pages/drafts/summaries`)
    #[command(after_help = "Notes:
  - Every active draft on the site is listed, not only this key's; `isOwn` is true for the ones its user started.")]
    List,
    /// Show a page's draft, or null when it has none (`GET /pages/{pageUuid}/draft`)
    #[command(after_help = "Notes:
  - The result is the draft's record (draftPageUuid, status, who created and last updated it), or JSON `null` when the page has no draft. Exit 0 either way.
  - For the draft's blocks use `pages content <page>`; for whether it can be published, `draft validate <page>`.")]
    Status {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
    /// Check whether a page's draft can be published (`POST /pages/{pageUuid}/draft/validate`)
    #[command(after_help = "Notes:
  - The result is {hasDraft, errors}, printed on stdout whatever it says.
  - Exit 5 when `errors` is not empty: the same errors `draft publish` would refuse with. Exit 0 when it is empty, including when the page has no draft (`hasDraft` false).
  - Each error names a block (`blockInstanceUuid`, a draft id) and its fields; fix them with `blocks patch`. `-q` prints just those block ids.")]
    Validate {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
    /// Make a page's draft live (`POST /pages/{pageUuid}/draft/publish`)
    #[command(after_help = "Notes:
  - Visitors see the change as soon as this returns. Publish only when you were asked to.
  - Asks first on a terminal; anywhere else it needs -y.
  - Run `draft validate <page>` first. A draft with errors is a 422 (exit 5) listing them; 403 (exit 3) means the site's owner handles publishing; 409 (exit 6) means someone else holds a page's edit lock, named under `lockedPages`.
  - The result lists what was published, with the `versionNumber` to give `versions restore` to go back.
  - Without --message the API writes a summary of the changes for the version history.")]
    Publish {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        /// Shown in the page's version history (1 to 500 characters)
        #[arg(short = 'm', long, value_name = "TEXT")]
        message: Option<String>,
    },
    /// Throw away a page's draft (`POST /pages/{pageUuid}/draft/discard`)
    #[command(after_help = "Notes:
  - Every unpublished change on the page is lost, other people's included. There is no undo.
  - Asks first on a terminal; anywhere else it needs -y.
  - A page with no draft is not an error: the result is {\"ok\": true} either way.
  - 409 (exit 6) means someone else holds the page's edit lock.")]
    Discard {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
}

pub fn run(ctx: &Context, cmd: &DraftCommand) -> Result<()> {
    match &cmd.command {
        DraftSub::List => {
            let path = format!("{PAGES}/drafts/summaries");
            let response = ctx.client.get(&path, &Query::new())?;
            ctx.printer.list(&items(&response.body), SUMMARY_COLUMNS)
        }
        DraftSub::Status { page } => {
            let path = format!("{}/draft", page_path(ctx, page)?);
            let body = ctx.client.get(&path, &Query::new())?.body;
            if body.is_null() {
                ctx.printer.note("This page has no draft.");
            }
            ctx.printer.one(&body, "/draftPageUuid")
        }
        DraftSub::Validate { page } => validate(ctx, page),
        DraftSub::Publish { page, message } => {
            let path = format!("{}/draft/publish", page_path(ctx, page)?);
            let mut body = Map::new();
            if let Some(message) = message.as_deref().map(str::trim) {
                if message.is_empty() {
                    return Err(Error::Usage("--message is empty".into()));
                }
                body.insert("publishMessage".into(), json!(message));
            }
            confirm(
                ctx,
                &format!("publish the draft of page {page} to the live site"),
            )?;
            let published = ctx.client.post(&path, Some(&Value::Object(body)))?.body;
            ctx.printer
                .wrapped(&published, "/published", PUBLISHED_COLUMNS)
        }
        DraftSub::Discard { page } => {
            let path = format!("{}/draft/discard", page_path(ctx, page)?);
            confirm(
                ctx,
                &format!("discard every unpublished change on page {page}"),
            )?;
            let body = ctx.client.post(&path, None)?.body;
            ctx.printer.raw(&body)
        }
    }
}

fn validate(ctx: &Context, page: &str) -> Result<()> {
    let path = format!("{}/draft/validate", page_path(ctx, page)?);
    let body = ctx.client.post(&path, None)?.body;
    let errors = body.get("errors").cloned().unwrap_or(Value::Null);
    let blocks: Vec<Value> = items(&errors)
        .iter()
        .flat_map(|page| items(page.get("blockErrors").unwrap_or(&Value::Null)))
        .collect();

    if ctx.printer.quiet {
        // the blocks that need attention, one draft block id per line
        ctx.printer.list(&blocks, BLOCK_ERROR_COLUMNS)?;
    } else if ctx.printer.format != Format::Table {
        ctx.printer.raw(&body)?;
    } else if blocks.is_empty() {
        let has_draft = body.get("hasDraft").and_then(Value::as_bool) != Some(false);
        ctx.printer.note(if has_draft {
            "The draft can be published."
        } else {
            "This page has no draft."
        });
        ctx.printer.raw(&body)?;
    } else {
        let mut out = std::io::stdout().lock();
        for line in publish_error_lines(&errors).unwrap_or_default() {
            writeln!(out, "{line}")?;
        }
    }

    if items(&errors).is_empty() {
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "the draft cannot be published yet: {} block(s) need attention, listed in `errors` on stdout",
        blocks.len()
    )))
}
