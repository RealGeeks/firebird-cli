//! `firebird-cli blocks …` — the block types a site can use and their schemas.

use clap::{Args, Subcommand};

use super::{items, segment, Context};
use crate::client::Query;
use crate::error::Result;
use crate::output::{col, Column};

pub const PATH: &str = "blocks";

pub const COLUMNS: &[Column] = &[
    col("codeName", "/codeName"),
    col("label", "/label"),
    col("category", "/category"),
    col("description", "/description"),
];

#[derive(Debug, Args)]
pub struct BlocksCommand {
    #[command(subcommand)]
    pub command: BlocksSub,
}

#[derive(Debug, Subcommand)]
pub enum BlocksSub {
    /// List the block types this site can use (`GET /blocks?includeSchema=false`)
    Types,
    /// Show one block type's JSON Schema, defaults and editor config (`GET /blocks/{codeName}`)
    #[command(after_help = "Notes:
  - Read `jsonSchema` and `defaults` before writing a block's data; never guess its shape.
  - A block type this site is not entitled to is a 404 (exit 4), the same as one that does not exist.")]
    Schema {
        /// The block type's codeName, from `blocks types`
        #[arg(value_name = "CODE_NAME")]
        code_name: String,
    },
}

pub fn run(ctx: &Context, cmd: &BlocksCommand) -> Result<()> {
    match &cmd.command {
        BlocksSub::Types => {
            let query: Query = vec![("includeSchema".into(), "false".into())];
            let response = ctx.client.get(PATH, &query)?;
            ctx.printer.list(&items(&response.body), COLUMNS)
        }
        BlocksSub::Schema { code_name } => {
            let code_name = segment("the block codeName", code_name)?;
            let response = ctx
                .client
                .get(&format!("{PATH}/{code_name}"), &Query::new())?;
            ctx.printer.raw(&response.body)
        }
    }
}
