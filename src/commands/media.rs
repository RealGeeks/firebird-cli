//! `firebird-cli media …` — the site's media library. Read only for now.

use clap::{Args, Subcommand};

use super::{segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{cell, col, Column, Format};

pub const PATH: &str = "media";

pub const COLUMNS: &[Column] = &[
    col("mediaUuid", "/mediaUuid"),
    col("filename", "/filename"),
    col("mimeType", "/mimeType"),
    col("width", "/width"),
    col("height", "/height"),
    col("assetRole", "/assetRole"),
    col("description", "/description"),
];

#[derive(Debug, Args)]
pub struct MediaCommand {
    #[command(subcommand)]
    pub command: MediaSub,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Match description and filename, ignoring case
    #[arg(long, value_name = "TEXT")]
    pub query: Option<String>,
    /// Comma-separated tags; matches media carrying any of them
    #[arg(long, value_name = "TAGS")]
    pub tags: Option<String>,
    /// all, image or video (the API's default is all)
    #[arg(long, value_name = "TYPE")]
    pub media_type: Option<String>,
    /// photo, illustration, logo, icon, headshot, property, background or banner
    #[arg(long, value_name = "ROLE")]
    pub asset_role: Option<String>,
    /// landscape, portrait or square
    #[arg(long, value_name = "ORIENTATION")]
    pub orientation: Option<String>,
    /// How many to return, 1 to 100 (the API's default is 20)
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// How many to skip
    #[arg(long, value_name = "N")]
    pub offset: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum MediaSub {
    /// List and search the media library (`GET /media`)
    #[command(after_help = "Notes:
  - The result is {items, total}; `total` counts every match, `items` is one page of them. Page with --limit and --offset.
  - Filter values are passed to the API as given, so a value it does not know is a 400 (exit 5).
  - A table and `-q` show the items; `-o json` has both.")]
    List(ListArgs),
    /// Show one media item (`GET /media/{mediaUuid}`)
    Get {
        /// The media's uuid, from `media list`
        #[arg(value_name = "MEDIA_UUID")]
        media_uuid: String,
    },
}

pub fn run(ctx: &Context, cmd: &MediaCommand) -> Result<()> {
    match &cmd.command {
        MediaSub::List(args) => {
            let number = |n: Option<u32>| n.map(|n| n.to_string());
            let query: Query = [
                ("query", args.query.clone()),
                ("tags", args.tags.clone()),
                ("mediaType", args.media_type.clone()),
                ("assetRole", args.asset_role.clone()),
                ("orientation", args.orientation.clone()),
                ("limit", number(args.limit)),
                ("offset", number(args.offset)),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.map(|v| (name.to_string(), v)))
            .collect();
            let body = ctx.client.get(PATH, &query)?.body;
            if ctx.printer.format == Format::Table {
                if let Some(total) = body.get("total") {
                    ctx.printer.note(&format!("{} in total", cell(total)));
                }
            }
            ctx.printer.wrapped(&body, "/items", COLUMNS)
        }
        MediaSub::Get { media_uuid } => {
            let media = segment("the mediaUuid", media_uuid)?;
            let body = ctx
                .client
                .get(&format!("{PATH}/{media}"), &Query::new())?
                .body;
            ctx.printer.raw(&body)
        }
    }
}
