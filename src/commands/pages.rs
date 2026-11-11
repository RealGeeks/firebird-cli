//! `firebird-cli pages …` — the site's pages and the blocks on one.

use clap::{Args, Subcommand};

use super::{find_page, items, page_path, Context, PAGES};
use crate::client::Query;
use crate::error::Result;
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
