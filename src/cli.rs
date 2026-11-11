//! The command tree and top-level dispatch.

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::Shell;

use crate::client::Client;
use crate::commands::{
    api, auth, blocks, draft, guide, media, nav, pages, revisions, settings, versions, Context,
};
use crate::config::{self, Config, Overrides};
use crate::error::Result;
use crate::output::{Format, Printer};

const ABOUT: &str = "The Firebird command line: read and edit a Firebird website.";
const AFTER_HELP: &str = "\
Start with `firebird-cli auth login --site www.example.com --api-key-stdin`
(a key from the site's Integrations page), then `firebird-cli me`.

A <page> is its uuid or its slug (about, /about/; / or home for the home page).
Block edits go into the page's draft: `blocks patch about <block> --set title=Hi`,
then `draft validate about` and `draft publish about`. Nothing is live before that.

`firebird-cli guide` is the walkthrough for scripts and AI agents; `guide --list`
shows its topics and `guide <topic>` prints one, e.g. `guide drafts` (what is
live and what waits in a draft), `guide blocks` (how block data is written),
`guide undo`, `guide exit`.";

#[derive(Debug, Parser)]
#[command(name = "firebird-cli", version, about = ABOUT, after_help = AFTER_HELP, propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, clap::Args)]
#[command(next_help_heading = "Global options")]
pub struct Global {
    /// Site domain a key is stored under, e.g. www.example.com
    #[arg(long, global = true, env = "FIREBIRD_SITE", value_name = "DOMAIN")]
    pub site: Option<String>,
    /// API key; overrides the stored key for the site
    #[arg(
        long,
        global = true,
        env = "FIREBIRD_API_KEY",
        hide_env_values = true,
        value_name = "KEY"
    )]
    pub api_key: Option<String>,
    /// Reach the API at this origin instead of the site's stored one or
    /// https://firebird-api.witco.net
    #[arg(long, global = true, env = "FIREBIRD_BASE_URL", value_name = "URL")]
    pub base_url: Option<String>,
    /// Output format (default: table on a terminal, json when piped)
    #[arg(short = 'o', long, global = true, value_enum, value_name = "FORMAT")]
    pub output: Option<Format>,
    /// Shorthand for --output json
    #[arg(long, global = true, conflicts_with = "output")]
    pub json: bool,
    /// Print only identifiers, one per line
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
    /// Skip the confirmation before a publish, discard, remove or restore
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    /// Log requests to stderr (method, URL and status; never a key or token)
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,
    /// Retries after a 429 rate limit. Each wait prints
    /// `rate limited; retrying in Ns (attempt i of n)` on stderr
    #[arg(long, global = true, default_value_t = 3, value_name = "N")]
    pub max_retries: u32,
    /// Read body files even from hidden paths and credential folders
    /// (~/.ssh, ~/.aws, …), which are refused by default
    #[arg(
        long,
        global = true,
        env = crate::sources::ENV,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub allow_any_source: bool,
    /// Set by [`Cli::parse_args`]: `--api-key` was typed, not taken from the environment.
    #[arg(skip)]
    pub api_key_on_command_line: bool,
}

impl Global {
    pub fn format(&self) -> Option<Format> {
        if self.json {
            Some(Format::Json)
        } else {
            self.output
        }
    }

    pub fn overrides(&self) -> Overrides {
        Overrides {
            site: self.site.clone(),
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Store and manage site API keys
    Auth(auth::AuthCommand),
    /// Show the site, key and token in use (same as `auth status`)
    Me,
    /// Pages and the blocks on them
    Pages(pages::PagesCommand),
    /// Block types, and adding, changing, moving and removing a page's blocks
    Blocks(blocks::BlocksCommand),
    /// A page's unpublished changes: status, validate, publish, discard
    Draft(draft::DraftCommand),
    /// A page's published versions
    Versions(versions::VersionsCommand),
    /// Navigation bars and the sections a link can point at (read only)
    Nav(nav::NavCommand),
    /// The theme: colours, fonts, radii, spacing (read only)
    Styling(settings::StylingCommand),
    /// The site footer (read only)
    Footer(settings::FooterCommand),
    /// Page headers (read only)
    Headers(settings::HeadersCommand),
    /// Social media links (read only)
    Social(settings::SocialCommand),
    /// Featured areas (read only)
    Areas(settings::AreasCommand),
    /// The media library (read only)
    Media(media::MediaCommand),
    /// The history of the theme, navigation, footer, headers and social links (read only)
    Revisions(revisions::RevisionsCommand),
    /// Call any API endpoint directly
    Api(api::ApiArgs),
    /// The usage guide for scripts and AI agents, whole or one topic (`guide drafts`)
    Guide(guide::GuideArgs),
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

impl Cli {
    /// Parse the process's arguments, exiting with clap's message on a usage
    /// error, and note whether the API key was typed on the command line.
    pub fn parse_args() -> Self {
        let matches = Self::command().get_matches();
        let mut cli = Self::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
        cli.global.api_key_on_command_line = on_command_line(&matches, "api_key");
        cli
    }
}

/// Whether a global argument was given as a flag at any level of the command.
fn on_command_line(matches: &ArgMatches, id: &str) -> bool {
    let mut level = Some(matches);
    while let Some(current) = level {
        if current.value_source(id) == Some(ValueSource::CommandLine) {
            return true;
        }
        level = current.subcommand().map(|(_, sub)| sub);
    }
    false
}

pub fn run(cli: Cli) -> Result<()> {
    let printer = Printer::new(cli.global.format(), cli.global.quiet);
    crate::sources::set_allow_any(cli.global.allow_any_source);
    let overrides = cli.global.overrides();

    match cli.command {
        Command::Guide(args) => guide::run(&args),
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "firebird-cli", &mut std::io::stdout());
            Ok(())
        }
        Command::Auth(cmd) => {
            let mut config = Config::load()?;
            let mut env = auth::AuthEnv {
                config: &mut config,
                overrides: &overrides,
                printer,
                max_retries: cli.global.max_retries,
                verbose: cli.global.verbose,
                key_on_command_line: cli.global.api_key_on_command_line,
            };
            auth::run(&mut env, cmd)
        }
        command => {
            let config = Config::load()?;
            let target = config::resolve_target(&config, &overrides)?;
            let client = Client::new(&target, cli.global.max_retries, cli.global.verbose)?;
            let ctx = Context {
                client,
                printer,
                site: target.site,
                yes: cli.global.yes,
            };
            match command {
                Command::Me => auth::status(&ctx),
                Command::Pages(cmd) => pages::run(&ctx, &cmd),
                Command::Blocks(cmd) => blocks::run(&ctx, &cmd),
                Command::Draft(cmd) => draft::run(&ctx, &cmd),
                Command::Versions(cmd) => versions::run(&ctx, &cmd),
                Command::Nav(cmd) => nav::run(&ctx, &cmd),
                Command::Styling(cmd) => settings::styling(&ctx, &cmd),
                Command::Footer(cmd) => settings::footer(&ctx, &cmd),
                Command::Headers(cmd) => settings::headers(&ctx, &cmd),
                Command::Social(cmd) => settings::social(&ctx, &cmd),
                Command::Areas(cmd) => settings::areas(&ctx, &cmd),
                Command::Media(cmd) => media::run(&ctx, &cmd),
                Command::Revisions(cmd) => revisions::run(&ctx, &cmd),
                Command::Api(args) => api::run(&ctx, &args),
                Command::Guide(_) | Command::Completions { .. } | Command::Auth(_) => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_key_flag_is_told_apart_from_no_key() {
        let typed = |args: &[&str]| {
            Cli::command()
                .try_get_matches_from(args)
                .is_ok_and(|matches| on_command_line(&matches, "api_key"))
        };
        assert!(typed(&["firebird-cli", "--api-key", "k", "me"]));
        assert!(typed(&["firebird-cli", "auth", "login", "--api-key", "k"]));
        assert!(!typed(&[
            "firebird-cli",
            "auth",
            "login",
            "--api-key-stdin"
        ]));
    }
}
