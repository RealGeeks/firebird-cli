//! `firebird-cli blocks …` — the block types a site can use, and adding,
//! changing, moving and removing the blocks on a page. Every write goes into
//! the page's draft; nothing is live until `draft publish`.

use clap::{Args, Subcommand, ValueEnum};
use serde_json::{json, Map, Value};

use super::pages::PAGE_HELP;
use super::{block_data, block_path, confirm, items, page_path, segment, Context};
use crate::client::Query;
use crate::error::{Error, Result};
use crate::output::{cell, col, Column};

pub const PATH: &str = "blocks";

pub const COLUMNS: &[Column] = &[
    col("codeName", "/codeName"),
    col("label", "/label"),
    col("category", "/category"),
    col("description", "/description"),
];

/// Where `-q` finds the block's id in a write's response.
const DRAFT_BLOCK_ID: &str = "/data/draftBlockInstanceUuid";
const DRAFT_PAGE_ID: &str = "/draftPageUuid";

const BLOCK_HELP: &str =
    "The block's uuid: a blockInstanceUuid from `pages content`, or the draftBlockInstanceUuid a write returned";

const DRAFT_NOTES: &str = "\
  - This writes to the page's draft; visitors see nothing until `draft publish`. The first write to a page creates the draft and takes the page's edit lock for the key's user.
  - A 409 (exit 6) means someone else holds the page's edit lock.
  - A write the API refuses can still leave a new, empty draft and the edit lock behind. If you did not mean to start one, check `draft status <page>` and drop it with `draft discard <page>`.";

#[derive(Debug, Args)]
pub struct BlocksCommand {
    #[command(subcommand)]
    pub command: BlocksSub,
}

#[derive(Debug, Args)]
pub struct DataArgs {
    /// Block data as a JSON object: inline, `@file`, or `-` for stdin
    #[arg(short = 'd', long, value_name = "JSON")]
    pub data: Option<String>,
    /// One field: key=value, repeatable. The value is JSON when it parses
    /// as JSON (`3`, `true`, `null`, `["a"]`), else text. A dotted key nests:
    /// primaryButton.label=Go. Wins over the same field in --data
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Direction {
    Up,
    Down,
    Top,
    Bottom,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
            Direction::Top => "top",
            Direction::Bottom => "bottom",
        }
    }
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
    /// Add a block to a page's draft (`POST /pages/{pageUuid}/blocks`)
    #[command(after_help = format!("Notes:
  - The new block starts with its type's defaults. With --data or --set this is two calls: the block is added, then its data is patched. If the patch is refused the block is still there; a `warning:` line on stderr gives its id.
  - The result is {{draftPageUuid, data}}; `data.draftBlockInstanceUuid` is the new block's id, and what `-q` prints.
{DRAFT_NOTES}"))]
    Add {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        /// The block type's codeName, from `blocks types`
        #[arg(value_name = "CODE_NAME")]
        code_name: String,
        /// Where to insert it, counting from 0; the end of the page when omitted
        #[arg(long, value_name = "N")]
        position: Option<u32>,
        #[command(flatten)]
        data: DataArgs,
    },
    /// Change some fields of a block's data (`PATCH /pages/{pageUuid}/blocks/{uuid}/data`)
    #[command(after_help = format!("Notes:
  - A merge patch: only the fields you send change. Set a field to null to clear it (--set subtitle=null). Nested objects merge; arrays are replaced whole.
  - Read the block type's schema first: `blocks schema <codeName>`. A field the schema does not allow is a 400 (exit 5).
  - The result is {{draftPageUuid, data}} with the block's whole data after the change; `-q` prints `data.draftBlockInstanceUuid`.
{DRAFT_NOTES}"))]
    Patch {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        #[arg(value_name = "BLOCK", help = BLOCK_HELP)]
        block: String,
        #[command(flatten)]
        data: DataArgs,
    },
    /// Move a block among its siblings (`POST …/blocks/{uuid}/move`)
    #[command(after_help = format!("Notes:\n{DRAFT_NOTES}"))]
    Move {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        #[arg(value_name = "BLOCK", help = BLOCK_HELP)]
        block: String,
        #[arg(value_enum)]
        direction: Direction,
    },
    /// Copy a block, placing the copy after it (`POST …/blocks/{uuid}/duplicate`)
    #[command(after_help = format!("Notes:
  - The result is {{draftPageUuid, data}}; `data.draftBlockInstanceUuid` is the copy's id, and what `-q` prints.
{DRAFT_NOTES}"))]
    Duplicate {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        #[arg(value_name = "BLOCK", help = BLOCK_HELP)]
        block: String,
    },
    /// Remove a block from a page's draft (`DELETE …/blocks/{uuid}`)
    #[command(after_help = format!("Notes:
  - Asks first on a terminal; anywhere else it needs -y.
{DRAFT_NOTES}"))]
    Remove {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        #[arg(value_name = "BLOCK", help = BLOCK_HELP)]
        block: String,
    },
    /// Set the order of a page's blocks (`POST /pages/{pageUuid}/blocks/order`)
    #[command(after_help = format!("Notes:
  - Give every block on the page, in the order wanted. The API refuses a list that does not name every block in the draft (a 400, exit 5), so nothing is dropped by leaving one out.
{DRAFT_NOTES}"))]
    Order {
        #[arg(value_name = "PAGE", help = PAGE_HELP)]
        page: String,
        /// Every block's uuid, in the order wanted
        #[arg(value_name = "BLOCK", required = true, num_args = 1..)]
        blocks: Vec<String>,
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
        BlocksSub::Add {
            page,
            code_name,
            position,
            data,
        } => add(ctx, page, code_name, *position, data),
        BlocksSub::Patch { page, block, data } => {
            let Some(data) = block_data(data.data.as_deref(), &data.set)? else {
                return Err(Error::Usage(
                    "nothing to change: give the fields with --data or --set".into(),
                ));
            };
            let path = format!("{}/data", block_path(ctx, page, block)?);
            let body = ctx.client.patch(&path, &json!({ "data": data }))?.body;
            ctx.printer.one(&body, DRAFT_BLOCK_ID)
        }
        BlocksSub::Move {
            page,
            block,
            direction,
        } => {
            let path = format!("{}/move", block_path(ctx, page, block)?);
            let body = json!({ "direction": direction.as_str() });
            let moved = ctx.client.post(&path, Some(&body))?.body;
            ctx.printer.one(&moved, DRAFT_PAGE_ID)
        }
        BlocksSub::Duplicate { page, block } => {
            let path = format!("{}/duplicate", block_path(ctx, page, block)?);
            let body = ctx.client.post(&path, None)?.body;
            ctx.printer.one(&body, DRAFT_BLOCK_ID)
        }
        BlocksSub::Remove { page, block } => {
            let path = block_path(ctx, page, block)?;
            confirm(ctx, &format!("remove block {block} from page {page}"))?;
            let body = ctx.client.delete(&path)?.body;
            ctx.printer.one(&body, DRAFT_PAGE_ID)
        }
        BlocksSub::Order { page, blocks } => {
            let ordered = blocks
                .iter()
                .map(|block| segment("a block uuid", block))
                .collect::<Result<Vec<_>>>()?;
            let path = format!("{}/blocks/order", page_path(ctx, page)?);
            let body = json!({ "orderedBlockInstanceUuids": ordered });
            let reordered = ctx.client.post(&path, Some(&body))?.body;
            ctx.printer.one(&reordered, DRAFT_PAGE_ID)
        }
    }
}

fn add(
    ctx: &Context,
    page: &str,
    code_name: &str,
    position: Option<u32>,
    data: &DataArgs,
) -> Result<()> {
    // read the data before anything is written, so a bad flag adds nothing
    let data = block_data(data.data.as_deref(), &data.set)?;
    let page = page_path(ctx, page)?;
    let mut body = Map::new();
    body.insert("blockCodeName".into(), json!(code_name.trim()));
    if let Some(position) = position {
        body.insert("position".into(), json!(position));
    }
    let added = ctx
        .client
        .post(&format!("{page}/blocks"), Some(&Value::Object(body)))?
        .body;
    let Some(data) = data else {
        return ctx.printer.one(&added, DRAFT_BLOCK_ID);
    };
    let block = added
        .pointer(DRAFT_BLOCK_ID)
        .map(cell)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Other("the API added a block but returned no id for it".into()))?;
    let path = format!(
        "{page}/blocks/{}/data",
        segment("the new block's uuid", &block)?
    );
    let patched = ctx
        .client
        .patch(&path, &json!({ "data": data }))
        .inspect_err(|_| {
            eprintln!(
                "warning: block {block} was added to the draft with its default data, but setting its data failed. Fix the data and run `blocks patch`, or take it out with `blocks remove`"
            );
        })?;
    ctx.printer.one(&patched.body, DRAFT_BLOCK_ID)
}
