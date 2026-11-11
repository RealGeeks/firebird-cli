//! `firebird-cli pages …` — the site's pages and the blocks on one.

use clap::{Args, Subcommand};

use super::{items, segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{cell, col, Column, Format};

pub const PATH: &str = "pages";

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

#[derive(Debug, Args)]
pub struct PagesCommand {
    #[command(subcommand)]
    pub command: PagesSub,
}

#[derive(Debug, Subcommand)]
pub enum PagesSub {
    /// List the site's pages (`GET /pages`)
    List,
    /// Show a page's blocks (`GET /pages/{pageUuid}/content`)
    #[command(after_help = "Notes:
  - The result is {initialEditorMode, blocks}. The blocks are the page as the key's user would open it in the editor.
  - A table shortens each block's data; use `-o json` for all of it, and `-q` for just the block instance uuids.
  - Draft blocks have different uuids from live ones; see `firebird-cli guide drafts`.")]
    Content {
        /// The page's uuid, from `pages list`
        #[arg(value_name = "PAGE_UUID")]
        page_uuid: String,
    },
}

pub fn run(ctx: &Context, cmd: &PagesCommand) -> Result<()> {
    match &cmd.command {
        PagesSub::List => {
            let response = ctx.client.get(PATH, &Query::new())?;
            ctx.printer.list(&items(&response.body), COLUMNS)
        }
        PagesSub::Content { page_uuid } => {
            let page = segment("the page uuid", page_uuid)?;
            let body = ctx
                .client
                .get(&format!("{PATH}/{page}/content"), &Query::new())?
                .body;
            if !ctx.printer.quiet && ctx.printer.format != Format::Table {
                return ctx.printer.raw(&body);
            }
            if let Some(mode) = body.pointer("/initialEditorMode/mode") {
                ctx.printer.note(&format!("editor mode: {}", cell(mode)));
            }
            let blocks = body.get("blocks").map(items).unwrap_or_default();
            ctx.printer.list(&blocks, BLOCK_COLUMNS)
        }
    }
}
