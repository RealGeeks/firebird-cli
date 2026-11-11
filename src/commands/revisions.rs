//! `firebird-cli revisions …` — the history the theme, navigation, footer,
//! headers, social links and site settings keep. Read only for now.

use clap::{Args, Subcommand};

use super::{segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{cell, col, Column, Format};

pub const PATH: &str = "revisions";

pub const COLUMNS: &[Column] = &[
    col("revisionUuid", "/revisionUuid"),
    col("revisionNumber", "/revisionNumber"),
    col("source", "/source"),
    col("message", "/message"),
    col("createdAt", "/createdAt"),
    col("actorUuid", "/actorUuid"),
];

#[derive(Debug, Args)]
pub struct RevisionsCommand {
    #[command(subcommand)]
    pub command: RevisionsSub,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Which setting: styling, social_links, footer, header, navigation or site_settings
    #[arg(long = "type", value_name = "RESOURCE_TYPE")]
    pub resource_type: String,
    /// The header's or nav bar's uuid; needed for header and navigation only
    #[arg(long = "key", value_name = "RESOURCE_KEY")]
    pub resource_key: Option<String>,
    /// Only the revisions one change set recorded
    #[arg(long, value_name = "UUID")]
    pub change_set: Option<String>,
    /// The `nextCursor` of the page before
    #[arg(long, value_name = "N")]
    pub cursor: Option<u64>,
    /// How many to return, 1 to 50 (the API's default is 20)
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum RevisionsSub {
    /// List a setting's revisions, newest first (`GET /revisions`)
    #[command(after_help = "Notes:
  - The result is {revisions, nextCursor}. The newest revision is the current state. Pass `nextCursor` as --cursor for the next page; null means there is no more.
  - `header` and `navigation` need --key (from `headers list` or `nav get`); the others are kept per site and refuse one (400, exit 5).
  - A table and `-q` show the revisions; `-o json` has both.")]
    List(ListArgs),
    /// Show one revision and the setting as it was then (`GET /revisions/{revisionUuid}`)
    Get {
        /// The revision's uuid, from `revisions list`
        #[arg(value_name = "REVISION_UUID")]
        revision_uuid: String,
    },
}

pub fn run(ctx: &Context, cmd: &RevisionsCommand) -> Result<()> {
    match &cmd.command {
        RevisionsSub::List(args) => {
            let query: Query = [
                ("resourceType", Some(args.resource_type.clone())),
                ("resourceKey", args.resource_key.clone()),
                ("changeSetUuid", args.change_set.clone()),
                ("cursor", args.cursor.map(|n| n.to_string())),
                ("limit", args.limit.map(|n| n.to_string())),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.map(|v| (name.to_string(), v)))
            .collect();
            let body = ctx.client.get(PATH, &query)?.body;
            if ctx.printer.format == Format::Table {
                if let Some(cursor) = body.get("nextCursor").filter(|c| !c.is_null()) {
                    ctx.printer
                        .note(&format!("more: --cursor {}", cell(cursor)));
                }
            }
            ctx.printer.wrapped(&body, "/revisions", COLUMNS)
        }
        RevisionsSub::Get { revision_uuid } => {
            let revision = segment("the revisionUuid", revision_uuid)?;
            let body = ctx
                .client
                .get(&format!("{PATH}/{revision}"), &Query::new())?
                .body;
            ctx.printer.raw(&body)
        }
    }
}
