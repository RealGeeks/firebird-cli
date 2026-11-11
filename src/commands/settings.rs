//! Reading the site's live settings: `styling`, `footer`, `headers`,
//! `social` and `areas`. Writing them is not built yet; they change the live
//! site at once, so until then that goes through `firebird-cli api`.

use clap::{Args, Subcommand};

use super::{items, segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{col, Column};

pub const FONT_COLUMNS: &[Column] = &[col("name", "/name"), col("url", "/url")];

pub const HEADER_COLUMNS: &[Column] = &[
    col("headerUuid", "/headerUuid"),
    col("name", "/name"),
    col("isSiteDefault", "/isSiteDefault"),
    col("isHidden", "/isHidden"),
    col("pageUuids", "/pageUuids"),
];

pub const AREA_COLUMNS: &[Column] = &[
    col("areaUuid", "/areaUuid"),
    col("name", "/name"),
    col("routeLocation", "/routeLocation"),
    col("customUrl", "/customUrl"),
    col("mediaUuid", "/mediaUuid"),
];

#[derive(Debug, Args)]
pub struct StylingCommand {
    #[command(subcommand)]
    pub command: StylingSub,
}

#[derive(Debug, Subcommand)]
pub enum StylingSub {
    /// Show the theme: colours, fonts, radii, spacing, colour mode (`GET /styling`)
    Get,
    /// List the fonts a theme can use (`GET /styling/fonts`)
    Fonts,
}

#[derive(Debug, Args)]
pub struct FooterCommand {
    #[command(subcommand)]
    pub command: FooterSub,
}

#[derive(Debug, Subcommand)]
pub enum FooterSub {
    /// Show the site footer (`GET /footer`)
    Get,
}

#[derive(Debug, Args)]
pub struct HeadersCommand {
    #[command(subcommand)]
    pub command: HeadersSub,
}

#[derive(Debug, Subcommand)]
pub enum HeadersSub {
    /// List the site's headers and the pages each applies to (`GET /headers`)
    List,
    /// Show one header (`GET /headers/{headerUuid}`)
    Get {
        /// The header's uuid, from `headers list`
        #[arg(value_name = "HEADER_UUID")]
        header_uuid: String,
    },
}

#[derive(Debug, Args)]
pub struct SocialCommand {
    #[command(subcommand)]
    pub command: SocialSub,
}

#[derive(Debug, Subcommand)]
pub enum SocialSub {
    /// Show the site's social media links (`GET /social-links`)
    Get,
}

#[derive(Debug, Args)]
pub struct AreasCommand {
    #[command(subcommand)]
    pub command: AreasSub,
}

#[derive(Debug, Subcommand)]
pub enum AreasSub {
    /// List the site's featured areas (`GET /areas`)
    List,
}

fn show(ctx: &Context, path: &str) -> Result<()> {
    ctx.printer.raw(&ctx.client.get(path, &Query::new())?.body)
}

fn list(ctx: &Context, path: &str, columns: &[Column]) -> Result<()> {
    let response = ctx.client.get(path, &Query::new())?;
    ctx.printer.list(&items(&response.body), columns)
}

pub fn styling(ctx: &Context, cmd: &StylingCommand) -> Result<()> {
    match cmd.command {
        StylingSub::Get => show(ctx, "styling"),
        StylingSub::Fonts => list(ctx, "styling/fonts", FONT_COLUMNS),
    }
}

pub fn footer(ctx: &Context, cmd: &FooterCommand) -> Result<()> {
    match cmd.command {
        FooterSub::Get => show(ctx, "footer"),
    }
}

pub fn headers(ctx: &Context, cmd: &HeadersCommand) -> Result<()> {
    match &cmd.command {
        HeadersSub::List => list(ctx, "headers", HEADER_COLUMNS),
        HeadersSub::Get { header_uuid } => {
            let header = segment("the headerUuid", header_uuid)?;
            show(ctx, &format!("headers/{header}"))
        }
    }
}

pub fn social(ctx: &Context, cmd: &SocialCommand) -> Result<()> {
    match cmd.command {
        SocialSub::Get => show(ctx, "social-links"),
    }
}

pub fn areas(ctx: &Context, cmd: &AreasCommand) -> Result<()> {
    match cmd.command {
        AreasSub::List => list(ctx, "areas", AREA_COLUMNS),
    }
}
