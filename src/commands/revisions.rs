//! `firebird-cli revisions …` — the history the theme, navigation, footer,
//! headers, social links and site settings keep, and restoring from it.

use clap::{Args, Subcommand};

use serde_json::{json, Map, Value};

use super::{confirm, segment, Context};
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
  - `header` and `navigation` need --key (the headerUuid from `headers list`, the navBarUuid from `nav list`); the others are kept per site and refuse one (400, exit 5).
  - A table and `-q` show the revisions; `-o json` has both.")]
    List(ListArgs),
    /// Show one revision and the setting as it was then (`GET /revisions/{revisionUuid}`)
    Get {
        /// The revision's uuid, from `revisions list`
        #[arg(value_name = "REVISION_UUID")]
        revision_uuid: String,
    },
    /// Put a setting back to what a revision recorded (`POST /revisions/{revisionUuid}/restore`)
    #[command(after_help = "Notes:
  - Live at once. Asks first on a terminal; -y skips the question.
  - The newest revision is the current state, so restoring it is refused (422, exit 5, code REVISION_ALREADY_CURRENT). To undo the latest change, restore the revision before it.
  - A restore is itself recorded as a new revision, so it can be undone the same way. The result is that new revision (`-q` prints its revisionUuid).
  - A revision that can no longer be applied (it links to a page since deleted, say) is a 422 whose `code` says why; 409 (exit 6) means the setting changed during the restore: run it again.
  - Restoring a navigation revision recreates its items under new uuids; run `nav get` again afterwards.")]
    Restore {
        /// The revision's uuid, from `revisions list`
        #[arg(value_name = "REVISION_UUID")]
        revision_uuid: String,
        /// A note recorded with the restore (up to 500 characters)
        #[arg(short = 'm', long, value_name = "TEXT")]
        message: Option<String>,
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
        RevisionsSub::Restore {
            revision_uuid,
            message,
        } => {
            let revision = segment("the revisionUuid", revision_uuid)?;
            let mut body = Map::new();
            if let Some(message) = message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                body.insert("message".into(), json!(message));
            }
            confirm(
                ctx,
                &format!("restore revision {revision} on the live site"),
            )?;
            let restored = ctx
                .client
                .post(
                    &format!("{PATH}/{revision}/restore"),
                    Some(&Value::Object(body)),
                )?
                .body;
            ctx.printer.note(
                "Live now. The restore is a new revision; restore the one before it to undo.",
            );
            ctx.printer.one(&restored, "/revision/revisionUuid")
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
