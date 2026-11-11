//! `firebird-cli auth …` — store, inspect and remove site credentials.

use std::io::{self, IsTerminal, Read};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use super::Context;
use crate::client::Client;
use crate::config::{self, Config, Overrides, SiteConfig, Target};
use crate::error::{Error, Result};
use crate::output::{col, Column, Format, Printer};
use crate::token::{self, Cache, Token};

pub const SITE_COLUMNS: &[Column] = &[
    col("site", "/site"),
    col("default", "/default"),
    col("prefix", "/prefix"),
    col("base_url", "/base_url"),
];

/// The token claims `me` shows, as the API names them.
const CLAIMS: &[&str] = &[
    "siteUuid",
    "userUuid",
    "userType",
    "clientUuid",
    "clientRole",
    "tokenKind",
    "integrationUuid",
];

#[derive(Debug, Args)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSub,
}

#[derive(Debug, Subcommand)]
pub enum AuthSub {
    /// Check an API key with the API and save it for a site
    #[command(after_help = "Notes:
  - Create the key on the site's Integrations page in the admin. Production keys start `fb_live_`, others `fb_test_`.
  - Prefer `--api-key-stdin` (e.g. `pbpaste | firebird-cli auth login --site www.example.com --api-key-stdin`): `--api-key` leaves the key in shell history and the process list.
  - `--site` is a label for the stored key; the API works out the site from the key itself.
  - The key is exchanged before it is saved, so a mistyped or revoked key is exit 3 and nothing is stored.
  - A key for another environment needs that API: `--base-url https://api.staging.example.com` is stored with the key.")]
    Login(LoginArgs),
    /// Forget the stored key for a site
    Logout,
    /// Show the site, key and token in use (same as `me`)
    Status,
    /// List the sites with stored keys
    Sites,
    /// Make a stored site the default
    Use {
        /// Site domain, as given to `auth login --site`
        site: String,
    },
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Read the API key from stdin instead of `--api-key`
    #[arg(long)]
    pub api_key_stdin: bool,
}

pub struct AuthEnv<'a> {
    pub config: &'a mut Config,
    pub overrides: &'a Overrides,
    pub printer: Printer,
    pub max_retries: u32,
    pub verbose: bool,
    /// `--api-key` was typed on the command line rather than set in the environment.
    pub key_on_command_line: bool,
}

pub fn run(env: &mut AuthEnv<'_>, cmd: AuthCommand) -> Result<()> {
    match cmd.command {
        AuthSub::Login(args) => login(env, &args),
        AuthSub::Logout => logout(env),
        AuthSub::Status => {
            let target = config::resolve_target(env.config, env.overrides)?;
            let client = Client::new(&target, env.max_retries, env.verbose)?;
            status(&Context {
                client,
                printer: env.printer,
                site: target.site,
            })
        }
        AuthSub::Sites => sites(env),
        AuthSub::Use { site } => {
            let domain = config::normalize_domain(&site);
            if env.config.site(&domain).is_none() {
                return Err(Error::Config(format!(
                    "no stored key for {domain}; run `firebird-cli auth login --site {domain} --api-key-stdin`"
                )));
            }
            env.config.default_site = Some(domain.clone());
            env.config.save()?;
            env.printer.note(&format!("Default site is now {domain}"));
            if env.printer.format != Format::Table {
                env.printer.raw(&json!({ "default_site": domain }))?;
            }
            Ok(())
        }
    }
}

/// `me` / `auth status`. The API has no "who am I" endpoint, so this
/// exchanges the key for a new token and reports what the token says.
pub fn status(ctx: &Context) -> Result<()> {
    let token = ctx.client.exchange()?;
    ctx.printer
        .raw(&identity(ctx.site.as_deref(), &ctx.client, &token))
}

/// The site label, API, key prefix and the token's claims and expiry. The
/// claims are read from the token without verifying it; the API does that.
fn identity(site: Option<&str>, client: &Client, token: &Token) -> Value {
    let mut doc = serde_json::Map::new();
    doc.insert("site".into(), json!(site));
    doc.insert("api_base".into(), json!(client.api_base()));
    doc.insert("key_prefix".into(), json!(client.api_key().map(key_prefix)));
    let claims = token::claims(&token.jwt).unwrap_or(Value::Null);
    for name in CLAIMS {
        doc.insert(
            (*name).into(),
            claims.get(*name).cloned().unwrap_or(Value::Null),
        );
    }
    let expires_at = token
        .exp
        .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0))
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    doc.insert("token_expires_at".into(), json!(expires_at));
    Value::Object(doc)
}

fn login(env: &mut AuthEnv<'_>, args: &LoginArgs) -> Result<()> {
    let Some(site) = env
        .overrides
        .site
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return Err(Error::Usage(
            "--site <domain> is required to log in (or set FIREBIRD_SITE)".into(),
        ));
    };
    let domain = config::normalize_domain(site);
    if domain.is_empty() {
        return Err(Error::Usage(format!("'{site}' is not a site domain")));
    }

    let api_key = if args.api_key_stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf.trim().to_string()
    } else if let Some(key) = env.overrides.api_key.as_deref() {
        if env.key_on_command_line {
            eprintln!(
                "warning: --api-key leaves the key in shell history and the process list; prefer --api-key-stdin"
            );
        }
        key.trim().to_string()
    } else {
        return Err(Error::Usage(
            "no API key given. Create one on the site's Integrations page, then pipe it in with --api-key-stdin (or pass --api-key, or set FIREBIRD_API_KEY)".into(),
        ));
    };
    if api_key.is_empty() {
        return Err(Error::Usage("empty API key".into()));
    }

    let stored_base = env
        .overrides
        .base_url
        .as_deref()
        .map(|base| base.trim().trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty());
    let base_url = stored_base
        .clone()
        .unwrap_or_else(|| config::DEFAULT_BASE_URL.to_string());
    config::check_transport(&base_url)?;

    // Verify before storing so a typo never ends up in the config file.
    let client = Client::new(
        &Target {
            site: Some(domain.clone()),
            base_url,
            api_key: Some(api_key.clone()),
        },
        env.max_retries,
        env.verbose,
    )?;
    let token = client.exchange()?;

    env.config.put_site(
        &domain,
        SiteConfig {
            api_key,
            base_url: stored_base,
        },
    );
    let path = env.config.save()?;

    env.printer.note(&format!(
        "Logged in to {domain}; key stored in {}",
        path.display()
    ));
    let mut summary = identity(Some(&domain), &client, &token);
    if let Some(doc) = summary.as_object_mut() {
        doc.insert("config_path".into(), json!(path.display().to_string()));
        doc.insert(
            "default".into(),
            json!(env.config.default_site.as_deref() == Some(domain.as_str())),
        );
    }
    env.printer.raw(&summary)
}

fn logout(env: &mut AuthEnv<'_>) -> Result<()> {
    let Some(domain) = config::selected_site(env.config, env.overrides) else {
        return Err(Error::Config(
            "no site selected. Pass --site <domain> to say which stored key to forget".into(),
        ));
    };
    let Some(removed) = env.config.remove_site(&domain) else {
        return Err(Error::Config(format!("no stored key for {domain}")));
    };
    env.config.save()?;
    // its token is no use to anyone once the key is gone from here
    let api_base = removed.api_base().trim().trim_end_matches('/');
    Cache::open().forget(&token::fingerprint(api_base, &removed.api_key));
    env.printer
        .note(&format!("Removed the stored key for {domain}"));
    if env.printer.format != Format::Table {
        env.printer
            .raw(&json!({ "logged_out": true, "site": domain }))?;
    }
    Ok(())
}

fn sites(env: &AuthEnv<'_>) -> Result<()> {
    let rows: Vec<Value> = env
        .config
        .sites
        .iter()
        .map(|(domain, site)| {
            json!({
                "site": domain,
                "default": env.config.default_site.as_deref() == Some(domain.as_str()),
                "prefix": key_prefix(&site.api_key),
                "base_url": site.api_base(),
            })
        })
        .collect();
    if rows.is_empty() && io::stdout().is_terminal() {
        env.printer.note(
            "No sites stored. Run `firebird-cli auth login --site <domain> --api-key-stdin`.",
        );
        return Ok(());
    }
    env.printer.list(&rows, SITE_COLUMNS)
}

/// `fb_live_abcd…` — enough to match the key in the admin, never the secret:
/// at most 12 characters, and never more than half of a short key.
pub fn key_prefix(key: &str) -> String {
    let shown = 12.min(key.chars().count() / 2);
    let prefix: String = key.chars().take(shown).collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_prefix_is_twelve_characters_at_most() {
        assert_eq!(
            key_prefix("fb_live_abcdEFGHijklMNOPqrstUVWX"),
            "fb_live_abcd…"
        );
        assert_eq!(key_prefix("fb_test_"), "fb_t…");
        assert_eq!(key_prefix(""), "…");
    }
}
