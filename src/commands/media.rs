//! `firebird-cli media …` — the site's media library: search it, add to it
//! by importing from a URL or uploading a file, and describe what is in it.

use std::path::Path;

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::{confirm, segment, Context};
use crate::client::{self, Query};
use crate::error::{Error, Result};
use crate::output::{cell, col, Column, Format};
use crate::sources;

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

/// The types the API accepts, by file extension. Images may be up to 20 MB
/// and videos up to 75 MB; the API enforces both.
pub const MIME_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("avif", "image/avif"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
];

/// The media type for a file name, from its extension.
pub fn mime_type(filename: &str) -> Option<&'static str> {
    let extension = Path::new(filename)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    MIME_TYPES
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, mime)| *mime)
}

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
    /// Copy an image from a public https address into the library (`POST /media/import`)
    #[command(after_help = "Notes:
  - Only import an image the person has confirmed they have the rights to reuse.
  - The API fetches the address itself and checks the file's type from its bytes. The result is the new media record; `-q` prints its mediaUuid, ready to set on a block.
  - This only adds to the library. Media cannot be deleted through the API.")]
    Import {
        /// The image's public https address
        #[arg(value_name = "URL")]
        url: String,
        /// Alternative text for the image (up to 500 characters)
        #[arg(long, value_name = "TEXT")]
        alt: Option<String>,
    },
    /// Upload a local image or video to the library
    #[command(after_help = "Notes:
  - Three steps: the API is asked for an upload address (`POST /media/upload-url`), the file's bytes are PUT to that address, and the upload is confirmed (`POST /media/confirm`). The result is the new media record; `-q` prints its mediaUuid.
  - Accepted: JPEG, PNG, WebP, GIF and AVIF images up to 20 MB; MP4, WebM and QuickTime videos up to 75 MB. The type comes from the file's extension unless --mime-type says otherwise; anything else is refused by the API (400, exit 5).
  - The upload address is on a storage host, not the API. Your API key and token are not sent there; it must be https (or a local dev host), and redirects are not followed.
  - Hidden files and credential folders are refused like any other file the CLI sends (--allow-any-source overrides).
  - If the upload or the confirmation fails, the API is left with a pending record that never shows in the library; a `warning:` line on stderr gives its mediaUuid. Run the upload again.
  - This only adds to the library. Media cannot be deleted through the API.")]
    Upload {
        /// The file to upload
        #[arg(value_name = "FILE")]
        file: String,
        /// Alternative text for the image
        #[arg(long, value_name = "TEXT")]
        alt: Option<String>,
        /// The file's media type, when its extension does not say
        #[arg(long, value_name = "TYPE")]
        mime_type: Option<String>,
        /// The name to store it under (default: the file's own name)
        #[arg(long, value_name = "NAME")]
        filename: Option<String>,
    },
    /// Change a media item's alt text, description, tags or role (`PATCH /media/{mediaUuid}`)
    #[command(after_help = "Notes:
  - Live at once wherever the media is used, and media details keep no history: there is no undo, so note the old values first (`media get`). Asks first on a terminal; -y skips the question.
  - Only the flags given change. `null` clears --alt, --description or --asset-role; --tags replaces the whole list (an empty value clears it).")]
    Update {
        /// The media's uuid, from `media list`
        #[arg(value_name = "MEDIA_UUID")]
        media_uuid: String,
        /// Alternative text (up to 1000 characters); `null` clears it
        #[arg(long, value_name = "TEXT")]
        alt: Option<String>,
        /// What the image shows (up to 5000 characters); `null` clears it
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// Comma-separated tags, replacing the ones it has
        #[arg(long, value_name = "TAGS")]
        tags: Option<String>,
        /// photo, illustration, logo, icon, headshot, property, background or banner; `null` clears it
        #[arg(long, value_name = "ROLE")]
        asset_role: Option<String>,
    },
}

/// The body for `PATCH /media/{mediaUuid}`: the flags given, `null` clearing.
pub fn update_body(
    alt: Option<&str>,
    description: Option<&str>,
    tags: Option<&str>,
    asset_role: Option<&str>,
) -> Map<String, Value> {
    let text = |value: &str| {
        if value.trim() == "null" {
            Value::Null
        } else {
            json!(value)
        }
    };
    let mut body = Map::new();
    for (field, value) in [
        ("alt", alt),
        ("description", description),
        ("assetRole", asset_role),
    ] {
        if let Some(value) = value {
            body.insert(field.into(), text(value));
        }
    }
    if let Some(tags) = tags {
        let list: Vec<&str> = tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .collect();
        body.insert("tags".into(), json!(list));
    }
    body
}

fn upload(
    ctx: &Context,
    file: &str,
    alt: Option<&str>,
    mime: Option<&str>,
    filename: Option<&str>,
) -> Result<()> {
    let path = Path::new(file);
    sources::check(path)?;
    let filename = filename
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| {
            Error::Usage(format!(
                "cannot tell a file name from '{file}'; pass --filename"
            ))
        })?;
    let Some(mime) = mime.or_else(|| mime_type(&filename)) else {
        let known: Vec<&str> = MIME_TYPES.iter().map(|(extension, _)| *extension).collect();
        return Err(Error::Usage(format!(
            "cannot tell the media type of '{filename}' from its extension (known: {}); pass --mime-type",
            known.join(", ")
        )));
    };
    let bytes = std::fs::read(path).map_err(|e| Error::Io(format!("cannot read {file}: {e}")))?;
    if bytes.is_empty() {
        return Err(Error::Usage(format!("{file} is empty")));
    }

    let mut request = Map::new();
    request.insert("filename".into(), json!(filename));
    request.insert("mimeType".into(), json!(mime));
    request.insert("filesize".into(), json!(bytes.len()));
    if let Some(alt) = alt {
        request.insert("alt".into(), json!(alt));
    }
    let pending = ctx
        .client
        .post("media/upload-url", Some(&Value::Object(request)))?
        .body;
    let field = |name: &str| {
        pending
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::Other(format!("the API's upload response has no {name}")))
    };
    let media_uuid = field("mediaUuid")?;
    let presigned_url = field("presignedUrl")?;

    let warn = |step: &str| {
        eprintln!(
            "warning: {step} failed, so media {} is left pending and will not show in the library; run the upload again",
            crate::output::sanitize(&media_uuid)
        );
    };
    client::upload_to_storage(&presigned_url, mime, bytes, ctx.client.verbose())
        .inspect_err(|_| warn("the upload"))?;
    let confirmed = ctx
        .client
        .post("media/confirm", Some(&json!({ "mediaUuid": media_uuid })))
        .inspect_err(|_| warn("confirming the upload"))?;
    ctx.printer.one(&confirmed.body, "/mediaUuid")
}

pub fn run(ctx: &Context, cmd: &MediaCommand) -> Result<()> {
    match &cmd.command {
        MediaSub::Import { url, alt } => {
            let mut body = Map::new();
            body.insert("url".into(), json!(url.trim()));
            if let Some(alt) = alt {
                body.insert("alt".into(), json!(alt));
            }
            let imported = ctx
                .client
                .post(&format!("{PATH}/import"), Some(&Value::Object(body)))?
                .body;
            ctx.printer.one(&imported, "/mediaUuid")
        }
        MediaSub::Upload {
            file,
            alt,
            mime_type,
            filename,
        } => upload(
            ctx,
            file,
            alt.as_deref(),
            mime_type.as_deref(),
            filename.as_deref(),
        ),
        MediaSub::Update {
            media_uuid,
            alt,
            description,
            tags,
            asset_role,
        } => {
            let media = segment("the mediaUuid", media_uuid)?;
            let body = update_body(
                alt.as_deref(),
                description.as_deref(),
                tags.as_deref(),
                asset_role.as_deref(),
            );
            if body.is_empty() {
                return Err(Error::Usage(
                    "nothing to change: give --alt, --description, --tags or --asset-role".into(),
                ));
            }
            confirm(
                ctx,
                &format!("change the details of media {media} wherever it is used"),
            )?;
            let updated = ctx
                .client
                .patch(&format!("{PATH}/{media}"), &Value::Object(body))?
                .body;
            ctx.printer
                .note("Live now. Media details keep no history, so there is no undo.");
            ctx.printer.one(&updated, "/mediaUuid")
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_type_comes_from_the_extension() {
        assert_eq!(mime_type("hero.JPG"), Some("image/jpeg"));
        assert_eq!(mime_type("photos/front.porch.webp"), Some("image/webp"));
        assert_eq!(mime_type("tour.mov"), Some("video/quicktime"));
        assert_eq!(mime_type("notes.pdf"), None);
        assert_eq!(mime_type("noextension"), None);
    }

    #[test]
    fn update_sends_only_what_was_given() {
        assert_eq!(
            Value::Object(update_body(
                Some("A porch"),
                None,
                Some(" Exterior, porch ,,"),
                None
            )),
            json!({"alt": "A porch", "tags": ["Exterior", "porch"]})
        );
        assert_eq!(
            Value::Object(update_body(
                Some("null"),
                Some("null"),
                Some(""),
                Some("null")
            )),
            json!({"alt": null, "description": null, "tags": [], "assetRole": null})
        );
        assert_eq!(update_body(None, None, None, None).len(), 0);
    }
}
