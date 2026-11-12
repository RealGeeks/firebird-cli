//! `firebird-cli areas …` — the site's featured areas. They are live as
//! soon as they are saved and keep no history.

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::{confirm, items, segment, Context};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{col, Column};

pub const PATH: &str = "areas";

pub const COLUMNS: &[Column] = &[
    col("areaUuid", "/areaUuid"),
    col("name", "/name"),
    col("routeLocation", "/routeLocation"),
    col("customUrl", "/customUrl"),
    col("mediaUuid", "/mediaUuid"),
];

const NO_HISTORY: &str = "Featured areas keep no history: there is no undo, so note the old values first (`areas list`).";

#[derive(Debug, Args)]
pub struct AreasCommand {
    #[command(subcommand)]
    pub command: AreasSub,
}

/// An area's fields, named after the API's. Only the ones given are sent.
#[derive(Debug, Args)]
pub struct AreaFields {
    /// The area's name
    #[arg(long, value_name = "TEXT")]
    pub name: Option<String>,
    /// A description; `null` clears it
    #[arg(long, value_name = "TEXT")]
    pub description: Option<String>,
    /// The uuid of the area's image in the media library; `null` clears it
    #[arg(long, value_name = "UUID")]
    pub media_uuid: Option<String>,
    /// The location the area's link searches; `null` clears it
    #[arg(long, value_name = "TEXT")]
    pub route_location: Option<String>,
    /// A link to use instead; `null` clears it
    #[arg(long, value_name = "URL")]
    pub custom_url: Option<String>,
    /// Latitude, -90 to 90; `null` clears it
    #[arg(long, value_name = "NUMBER", allow_hyphen_values = true)]
    pub lat: Option<String>,
    /// Longitude, -180 to 180; `null` clears it
    #[arg(long, value_name = "NUMBER", allow_hyphen_values = true)]
    pub lon: Option<String>,
}

impl AreaFields {
    /// The request body: the flags given, under the API's field names.
    pub fn body(&self) -> Result<Map<String, Value>> {
        let text = |value: &str| {
            if value.trim() == "null" {
                Value::Null
            } else {
                json!(value)
            }
        };
        let number = |name: &str, value: &str| -> Result<Value> {
            if value.trim() == "null" {
                return Ok(Value::Null);
            }
            value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(|n| json!(n))
                .ok_or_else(|| {
                    Error::Usage(format!("--{name} must be a number or null, got '{value}'"))
                })
        };
        let mut body = Map::new();
        if let Some(name) = &self.name {
            body.insert("name".into(), json!(name));
        }
        for (field, value) in [
            ("description", &self.description),
            ("mediaUuid", &self.media_uuid),
            ("routeLocation", &self.route_location),
            ("customUrl", &self.custom_url),
        ] {
            if let Some(value) = value {
                body.insert(field.into(), text(value));
            }
        }
        for (field, value) in [("lat", &self.lat), ("lon", &self.lon)] {
            if let Some(value) = value {
                body.insert(field.into(), number(field, value)?);
            }
        }
        Ok(body)
    }
}

#[derive(Debug, Subcommand)]
pub enum AreasSub {
    /// List the site's featured areas (`GET /areas`)
    List,
    /// Add a featured area (`POST /areas`)
    #[command(after_help = format!("Notes:
  - The area is on the live site at once. `-q` prints its areaUuid.
  - An area links to a location search (--route-location) or to --custom-url, not both (400, exit 5).
  - {NO_HISTORY}"))]
    Create(AreaFields),
    /// Change a featured area (`PATCH /areas/{areaUuid}`)
    #[command(after_help = format!("Notes:
  - Live at once. Only the flags given change; `null` clears a field that can be empty. Asks first on a terminal; -y skips the question.
  - An area links to a location search (--route-location) or to --custom-url, not both (400, exit 5): clear one when setting the other, e.g. --route-location null --custom-url https://…
  - {NO_HISTORY}"))]
    Update {
        /// The area's uuid, from `areas list`
        #[arg(value_name = "AREA_UUID")]
        area_uuid: String,
        #[command(flatten)]
        fields: AreaFields,
    },
    /// Delete a featured area for good (`DELETE /areas/{areaUuid}`)
    #[command(after_help = format!("Notes:
  - Permanent and live at once. Confirm the area's name with the person first. Asks first on a terminal; -y skips the question.
  - {NO_HISTORY}"))]
    Delete {
        /// The area's uuid, from `areas list`
        #[arg(value_name = "AREA_UUID")]
        area_uuid: String,
    },
}

pub fn run(ctx: &Context, cmd: &AreasCommand) -> Result<()> {
    match &cmd.command {
        AreasSub::List => {
            let response = ctx.client.get(PATH, &Query::new())?;
            ctx.printer.list(&items(&response.body), COLUMNS)
        }
        AreasSub::Create(fields) => {
            let body = fields.body()?;
            if !body.contains_key("name") {
                return Err(Error::Usage("an area needs a --name".into()));
            }
            let created = ctx.client.post(PATH, Some(&Value::Object(body)))?.body;
            ctx.printer
                .note("Live now. Featured areas keep no history.");
            ctx.printer.one(&created, "/areaUuid")
        }
        AreasSub::Update { area_uuid, fields } => {
            let area = segment("the areaUuid", area_uuid)?;
            let body = fields.body()?;
            if body.is_empty() {
                return Err(Error::Usage(
                    "nothing to change: give at least one field, e.g. --name".into(),
                ));
            }
            confirm(ctx, &format!("change area {area} on the live site"))?;
            let updated = ctx
                .client
                .patch(&format!("{PATH}/{area}"), &Value::Object(body))?
                .body;
            ctx.printer
                .note("Live now. Featured areas keep no history, so there is no undo.");
            ctx.printer.one(&updated, "/areaUuid")
        }
        AreasSub::Delete { area_uuid } => {
            let area = segment("the areaUuid", area_uuid)?;
            confirm(ctx, &format!("delete area {area} for good"))?;
            let deleted = ctx.client.delete(&format!("{PATH}/{area}"))?.body;
            ctx.printer.raw(&deleted)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> AreaFields {
        AreaFields {
            name: None,
            description: None,
            media_uuid: None,
            route_location: None,
            custom_url: None,
            lat: None,
            lon: None,
        }
    }

    #[test]
    fn only_the_flags_given_are_sent_and_null_clears() {
        let given = AreaFields {
            name: Some("Downtown".into()),
            description: Some("null".into()),
            route_location: Some("downtown".into()),
            lat: Some("33.75".into()),
            lon: Some("-84.39".into()),
            ..fields()
        };
        assert_eq!(
            given.body().ok().map(Value::Object),
            Some(json!({
                "name": "Downtown",
                "description": null,
                "routeLocation": "downtown",
                "lat": 33.75,
                "lon": -84.39,
            }))
        );
        assert_eq!(fields().body().ok().map(|b| b.len()), Some(0));
        let cleared = AreaFields {
            lat: Some("null".into()),
            ..fields()
        };
        assert_eq!(
            cleared.body().ok().map(Value::Object),
            Some(json!({"lat": null}))
        );
    }

    #[test]
    fn a_coordinate_must_be_a_number() {
        let bad = AreaFields {
            lon: Some("west".into()),
            ..fields()
        };
        assert!(matches!(bad.body(), Err(Error::Usage(m)) if m.contains("--lon")));
    }
}
