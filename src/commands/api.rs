//! `firebird-cli api METHOD PATH` — call any API endpoint directly. The
//! escape hatch for anything the typed commands do not cover yet.

use clap::Args;
use reqwest::Method;
use serde_json::Value;

use super::Context;
use crate::client::Query;
use crate::error::{Error, Result};
use crate::sources;

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// HTTP method: GET, POST, PUT, PATCH or DELETE
    pub method: String,
    /// Path on the API, e.g. /pages or /blocks/hero
    pub path: String,
    /// Query parameter, key=value; repeatable
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
    /// JSON body: inline, `@file`, or `-` for stdin
    #[arg(short = 'd', long, value_name = "JSON")]
    pub data: Option<String>,
    /// Send no credentials, for the public endpoints (/healthz/ready, /docs)
    #[arg(long)]
    pub no_auth: bool,
}

pub fn run(ctx: &Context, args: &ApiArgs) -> Result<()> {
    let method = Method::from_bytes(args.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| Error::Usage(format!("unknown HTTP method '{}'", args.method)))?;
    let mut query = Query::new();
    for param in &args.params {
        let (k, v) = param
            .split_once('=')
            .ok_or_else(|| Error::Usage(format!("--param expects key=value, got '{param}'")))?;
        query.push((k.to_string(), v.to_string()));
    }
    let body: Option<Value> = match args.data.as_deref() {
        None => None,
        Some(source) => {
            let text = if let Some(path) = source.strip_prefix('@') {
                sources::read(path)?
            } else if source == "-" {
                sources::read("-")?
            } else {
                source.to_string()
            };
            Some(serde_json::from_str(&text)?)
        }
    };
    let response = ctx
        .client
        .request(&method, &args.path, &query, body.as_ref(), !args.no_auth)
        .map_err(|err| match err {
            Error::NotLoggedIn(message) => Error::NotLoggedIn(format!(
                "{message}. A public endpoint such as /healthz/ready needs none: pass --no-auth"
            )),
            other => other,
        })?;
    if let Some(location) = &response.location {
        ctx.printer
            .note(&format!("{} Location: {location}", response.status));
    }
    ctx.printer.raw(&response.body)
}
