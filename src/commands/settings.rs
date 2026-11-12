//! The site's live settings: `styling`, `footer`, `headers` and `social`.
//!
//! The API saves each of them as a full replacement: a `PUT` of the whole
//! object, every field required. `set`/`update` here gives "only what you
//! pass changes" on top of that: read the current value, write the caller's
//! fields over it, and send the whole object back, holding exactly the
//! fields the save takes. Two writers doing that at once can undo each
//! other; the API has no version check to catch it.

use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use super::{confirm, items, merge, segment, Context, FieldArgs};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{cell, col, sanitize, Column, Format};

pub const FONT_COLUMNS: &[Column] = &[col("name", "/name"), col("url", "/url")];

pub const HEADER_COLUMNS: &[Column] = &[
    col("headerUuid", "/headerUuid"),
    col("name", "/name"),
    col("isSiteDefault", "/isSiteDefault"),
    col("isHidden", "/isHidden"),
    col("pageUuids", "/pageUuids"),
];

/// One of the settings the API saves whole.
pub struct Setting {
    /// What to call it in a prompt or a message.
    pub label: &'static str,
    /// The fields the save takes: the body sent holds these and nothing else.
    /// Keep in step with the API's save schema.
    pub fields: &'static [&'static str],
    /// The key the saved object comes back under, beside `revisionUuid`.
    pub saved_as: &'static str,
    /// Its `resourceType` in the revision history.
    pub history: &'static str,
}

pub const STYLING: Setting = Setting {
    label: "the site's theme",
    fields: &[
        "themeColor",
        "primaryColor",
        "headingFontName",
        "bodyFontName",
        "cardRadius",
        "buttonRadius",
        "spacing",
        "colorMode",
    ],
    saved_as: "styling",
    history: "styling",
};

pub const SOCIAL: Setting = Setting {
    label: "the site's social links",
    fields: &[
        "facebook",
        "twitter",
        "linkedin",
        "instagram",
        "youtube",
        "tiktok",
        "pinterest",
    ],
    saved_as: "socialLinks",
    history: "social_links",
};

pub const FOOTER: Setting = Setting {
    label: "the site's footer",
    fields: &[
        "logoMediaUuid",
        "secondaryLogoMediaUuid",
        "showLogo",
        "showSecondaryLogo",
        "fullName",
        "address",
        "phone",
        "licenseNumber",
        "isEqualHousingOpportunity",
        "isEqualHousingLender",
        "isRealtor",
        "overrideText",
        "backgroundColor",
    ],
    saved_as: "footer",
    history: "footer",
};

pub const HEADER: Setting = Setting {
    label: "this header",
    fields: &[
        "logoMediaUuid",
        "secondaryLogoMediaUuid",
        "showLogo",
        "showSecondaryLogo",
        "isSticky",
        "isTransparent",
        "isHidden",
        "backgroundColor",
        "logoPosition",
    ],
    saved_as: "header",
    history: "header",
};

const SET_NOTES: &str = "\
  - This changes the live site at once; it is not part of a page's draft. Asks first on a terminal; -y skips the question.
  - The API replaces the whole object on every save. This command reads the current value, writes your fields over it and sends everything back, so only what you pass changes. Two writers doing that at the same moment can undo each other's change.
  - Only the fields the save takes are sent; read-only fields in the read are left out. A field the setting does not have is exit 2.
  - The result is the object as stored plus `revisionUuid`, the revision this save recorded (`-q` prints it). If the API stored a field differently from what was sent, a `warning:` line on stderr says so.
  - Undo with `revisions restore <revisionUuid>` of the revision before this one; on a terminal the note after the save names it.";

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
    /// Change the theme: read it, apply your fields, save it whole (`PUT /styling`)
    #[command(after_help = format!("Fields: themeColor, primaryColor (#RRGGBB or null), headingFontName, bodyFontName (from `styling fonts`), cardRadius, buttonRadius, spacing (compact, default, spacious), colorMode (system, light, dark).

Notes:
{SET_NOTES}
  - A font the site has never chosen reads as the default font's name, and any save stores that name (even if null is sent). What visitors see does not change; restoring an older revision is the only way back to an unset font."))]
    Set(FieldArgs),
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
    /// Change the footer: read it, apply your fields, save it whole (`PUT /footer`)
    #[command(after_help = format!("Fields: fullName, address, phone, licenseNumber, overrideText (HTML, sanitized by the API), logoMediaUuid, secondaryLogoMediaUuid, showLogo, showSecondaryLogo, isEqualHousingOpportunity, isEqualHousingLender, isRealtor, backgroundColor.

Notes:
{SET_NOTES}
  - An empty or null text field hides that line: `footer set phone=null`."))]
    Set(FieldArgs),
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
    /// Change a header: read it, apply your fields, save it whole (`PUT /headers/{headerUuid}`)
    #[command(after_help = format!("Fields: logoMediaUuid, secondaryLogoMediaUuid, showLogo, showSecondaryLogo, isSticky, isTransparent, isHidden, backgroundColor, logoPosition (left, center).

Notes:
{SET_NOTES}
  - This does not create or delete a header, rename it, or change which pages use it.
  - `isTransparent` only applies to a sticky header; the API stores it as false otherwise."))]
    Update {
        /// The header's uuid, from `headers list`
        #[arg(value_name = "HEADER_UUID")]
        header_uuid: String,
        #[command(flatten)]
        fields: FieldArgs,
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
    /// Change social links: read them, apply your fields, save them all (`PUT /social-links`)
    #[command(after_help = format!("Fields: facebook, twitter, linkedin, instagram, youtube, tiktok, pinterest. Each is a handle; a profile URL is reduced to its handle by the API, and an empty value removes the link: `social set tiktok=`.

Notes:
{SET_NOTES}"))]
    Set(FieldArgs),
}

fn show(ctx: &Context, path: &str) -> Result<()> {
    ctx.printer.raw(&ctx.client.get(path, &Query::new())?.body)
}

fn list(ctx: &Context, path: &str, columns: &[Column]) -> Result<()> {
    let response = ctx.client.get(path, &Query::new())?;
    ctx.printer.list(&items(&response.body), columns)
}

pub fn styling(ctx: &Context, cmd: &StylingCommand) -> Result<()> {
    match &cmd.command {
        StylingSub::Get => show(ctx, "styling"),
        StylingSub::Fonts => list(ctx, "styling/fonts", FONT_COLUMNS),
        StylingSub::Set(fields) => save(ctx, &STYLING, "styling", None, fields),
    }
}

pub fn footer(ctx: &Context, cmd: &FooterCommand) -> Result<()> {
    match &cmd.command {
        FooterSub::Get => show(ctx, "footer"),
        FooterSub::Set(fields) => save(ctx, &FOOTER, "footer", None, fields),
    }
}

pub fn headers(ctx: &Context, cmd: &HeadersCommand) -> Result<()> {
    match &cmd.command {
        HeadersSub::List => list(ctx, "headers", HEADER_COLUMNS),
        HeadersSub::Get { header_uuid } => {
            let header = segment("the headerUuid", header_uuid)?;
            show(ctx, &format!("headers/{header}"))
        }
        HeadersSub::Update {
            header_uuid,
            fields,
        } => {
            let header = segment("the headerUuid", header_uuid)?;
            let path = format!("headers/{header}");
            save(ctx, &HEADER, &path, Some(&header), fields)
        }
    }
}

pub fn social(ctx: &Context, cmd: &SocialCommand) -> Result<()> {
    match &cmd.command {
        SocialSub::Get => show(ctx, "social-links"),
        SocialSub::Set(fields) => save(ctx, &SOCIAL, "social-links", None, fields),
    }
}

/// The body to save: the fields of `setting` as they are in `current`, with
/// `changes` written over them. A change to a field the read has but the
/// save does not take (a read-only one) is dropped; a field the setting does
/// not have at all is a usage error.
pub fn replacement(
    setting: &Setting,
    current: &Value,
    mut changes: Map<String, Value>,
) -> Result<Map<String, Value>> {
    let unknown: Vec<String> = changes
        .keys()
        .filter(|key| !setting.fields.contains(&key.as_str()) && current.get(key).is_none())
        .map(|key| format!("'{key}'"))
        .collect();
    if !unknown.is_empty() {
        return Err(Error::Usage(format!(
            "{} has no field {}. Its fields: {}",
            setting.label,
            unknown.join(", "),
            setting.fields.join(", ")
        )));
    }
    changes.retain(|key, _| setting.fields.contains(&key.as_str()));
    if changes.is_empty() {
        return Err(Error::Usage(format!(
            "nothing to change: only read-only fields were given. The fields that can be set: {}",
            setting.fields.join(", ")
        )));
    }
    let mut body: Map<String, Value> = setting
        .fields
        .iter()
        .filter_map(|field| {
            current
                .get(field)
                .map(|value| ((*field).to_string(), value.clone()))
        })
        .collect();
    merge(&mut body, changes);
    Ok(body)
}

/// The fields the API stored with another value than the one sent, one line
/// each. The API normalizes some values (a profile URL becomes its handle).
pub fn stored_differently(sent: &Map<String, Value>, stored: &Value) -> Vec<String> {
    sent.iter()
        .filter_map(|(key, value)| {
            let kept = stored.get(key)?;
            (kept != value).then(|| format!("{key} was stored as {kept}, not {value} as sent"))
        })
        .collect()
}

fn save(
    ctx: &Context,
    setting: &Setting,
    path: &str,
    history_key: Option<&str>,
    fields: &FieldArgs,
) -> Result<()> {
    let changes = fields.changes()?;
    let current = ctx.client.get(path, &Query::new())?.body;
    let body = replacement(setting, &current, changes)?;
    confirm(ctx, &format!("change {} on the live site", setting.label))?;
    let saved = ctx.client.put(path, &Value::Object(body.clone()))?.body;

    let stored = saved.get(setting.saved_as).unwrap_or(&Value::Null);
    for line in stored_differently(&body, stored) {
        eprintln!("warning: {}", sanitize(&line));
    }
    if ctx.printer.format == Format::Table && !ctx.printer.quiet {
        ctx.printer
            .note(&undo_note(ctx, setting.history, history_key));
    }
    ctx.printer.one(&saved, "/revisionUuid")
}

/// What to tell a person after a live save: that it is live, and the command
/// that takes it back. Looks up the revision before the newest one; if that
/// cannot be read the note says where the history is instead.
fn undo_note(ctx: &Context, history: &str, key: Option<&str>) -> String {
    let mut query: Query = vec![
        ("resourceType".into(), history.to_string()),
        ("limit".into(), "2".into()),
    ];
    let mut list = format!("firebird-cli revisions list --type {history}");
    if let Some(key) = key {
        query.push(("resourceKey".into(), key.to_string()));
        list = format!("{list} --key {key}");
    }
    let previous = ctx
        .client
        .get("revisions", &query)
        .ok()
        .and_then(|response| response.body.pointer("/revisions/1/revisionUuid").map(cell))
        .filter(|uuid| !uuid.is_empty());
    match previous {
        Some(uuid) => format!("Live now. To undo: firebird-cli revisions restore {uuid}"),
        None => format!("Live now. Its history: {list}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn changes(value: &Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn styling() -> Value {
        json!({
            "siteUuid": "s1",
            "themeColor": "blue",
            "primaryColor": null,
            "headingFontName": "Lora",
            "bodyFontName": null,
            "headingFontUrl": "https://fonts.example.com/lora",
            "bodyFontUrl": null,
            "cardRadius": "0.75rem",
            "buttonRadius": "0.75rem",
            "spacing": "default",
            "colorMode": "system",
        })
    }

    #[test]
    fn the_body_is_the_save_fields_with_the_changes_applied() {
        let body = replacement(
            &STYLING,
            &styling(),
            changes(&json!({"spacing": "compact", "primaryColor": "#112233"})),
        );
        assert_eq!(
            body.ok().map(Value::Object),
            Some(json!({
                "themeColor": "blue",
                "primaryColor": "#112233",
                "headingFontName": "Lora",
                "bodyFontName": null,
                "cardRadius": "0.75rem",
                "buttonRadius": "0.75rem",
                "spacing": "compact",
                "colorMode": "system",
            }))
        );
    }

    #[test]
    fn read_only_fields_are_dropped_and_unknown_ones_refused() {
        // the read piped back in, with one field edited
        let mut edited = changes(&styling());
        edited.insert("colorMode".into(), json!("dark"));
        let body = replacement(&STYLING, &styling(), edited).ok();
        assert_eq!(
            body.as_ref().and_then(|b| b.get("colorMode")),
            Some(&json!("dark"))
        );
        assert_eq!(
            body.as_ref().map(|b| b.contains_key("siteUuid")),
            Some(false)
        );
        assert_eq!(body.map(|b| b.len()), Some(STYLING.fields.len()));

        assert!(matches!(
            replacement(&STYLING, &styling(), changes(&json!({"spacng": "compact"}))),
            Err(Error::Usage(m)) if m.contains("'spacng'") && m.contains("spacing, colorMode")
        ));
        assert!(matches!(
            replacement(&STYLING, &styling(), changes(&json!({"siteUuid": "s2"}))),
            Err(Error::Usage(m)) if m.contains("only read-only fields")
        ));
    }

    #[test]
    fn a_field_the_read_lacks_is_sent_only_when_given() {
        let current = json!({"facebook": "example"});
        let body = replacement(&SOCIAL, &current, changes(&json!({"tiktok": ""}))).ok();
        assert_eq!(
            body.map(Value::Object),
            Some(json!({"facebook": "example", "tiktok": ""}))
        );
    }

    #[test]
    fn says_which_fields_were_stored_differently() {
        let sent = changes(&json!({
            "instagram": "https://instagram.com/example",
            "bodyFontName": null,
            "spacing": "compact",
        }));
        let stored = json!({
            "instagram": "example",
            "bodyFontName": "Geist",
            "spacing": "compact",
            "siteUuid": "s1",
        });
        assert_eq!(
            stored_differently(&sent, &stored),
            vec![
                r#"bodyFontName was stored as "Geist", not null as sent"#.to_string(),
                r#"instagram was stored as "example", not "https://instagram.com/example" as sent"#
                    .to_string(),
            ]
        );
        assert_eq!(
            stored_differently(&sent, &Value::Null),
            Vec::<String>::new()
        );
    }
}
