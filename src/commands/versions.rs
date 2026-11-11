//! `firebird-cli versions …` — the versions of a page that have been
//! published, and bringing one back as the page's draft.

use clap::{Args, Subcommand};

use super::pages::{BLOCK_COLUMNS, PAGE_HELP};
use super::{confirm, items, page_path, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{col, Column};

pub const COLUMNS: &[Column] = &[
    col("versionNumber", "/versionNumber"),
    col("status", "/status"),
    col("publishMessage", "/publishMessage"),
    col("publishedAt", "/publishedAt"),
    col("publishedBy", "/publishedBy"),
    col("draftPageUuid", "/draftPageUuid"),
];

#[derive(Debug, Args)]
pub struct VersionsCommand {
    #[command(subcommand)]
    pub command: VersionsSub,
}

#[derive(Debug, Subcommand)]
pub enum VersionsSub {
    /// List a page's published versions (`GET /pages/{pageUuid}/versions`)
    List {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
    },
    /// Show one version and its blocks (`GET /pages/{pageUuid}/versions/{n}`)
    #[command(after_help = "Notes:
  - The result is {version, blocks}. A table and `-q` show the blocks; `-o json` has both.")]
    Get {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        /// The version number, from `versions list`
        #[arg(value_name = "N")]
        number: u32,
    },
    /// Replace the page's draft with a published version (`POST …/versions/{n}/restore`)
    #[command(after_help = "Notes:
  - The whole draft is replaced by that version, so any unpublished change on the page is lost. The live page does not change: the result is a draft, and `draft publish` makes it live.
  - Asks first on a terminal; anywhere else it needs -y.
  - 409 (exit 6) means someone else holds the page's edit lock.")]
    Restore {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        /// The version number, from `versions list`
        #[arg(value_name = "N")]
        number: u32,
    },
}

pub fn run(ctx: &Context, cmd: &VersionsCommand) -> Result<()> {
    match &cmd.command {
        VersionsSub::List { page } => {
            let path = format!("{}/versions", page_path(ctx, page)?);
            let response = ctx.client.get(&path, &Query::new())?;
            ctx.printer.list(&items(&response.body), COLUMNS)
        }
        VersionsSub::Get { page, number } => {
            let path = format!("{}/versions/{number}", page_path(ctx, page)?);
            let body = ctx.client.get(&path, &Query::new())?.body;
            ctx.printer.wrapped(&body, "/blocks", BLOCK_COLUMNS)
        }
        VersionsSub::Restore { page, number } => {
            let path = format!("{}/versions/{number}/restore", page_path(ctx, page)?);
            confirm(
                ctx,
                &format!("replace the draft of page {page} with version {number}"),
            )?;
            let body = ctx.client.post(&path, None)?.body;
            ctx.printer.one(&body, "/draftPageUuid")
        }
    }
}
