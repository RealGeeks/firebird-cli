//! `firebird-cli` — a command-line client for the Firebird API.
//!
//! The binary lives in `main.rs`; everything else is here so integration
//! tests and other tools can reuse it.

pub mod cli;
pub mod client;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;
pub mod sources;
pub mod token;

pub const GUIDE: &str = include_str!("../docs/GUIDE.md");
