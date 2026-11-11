//! Printing results. JSON is the default when stdout is not a terminal so
//! agents and scripts get something parseable; a table when it is.

use std::io::{self, IsTerminal, Write};

use comfy_table::{presets::UTF8_FULL_CONDENSED, ContentArrangement, Table};
use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// Pretty-printed JSON (default when piped)
    Json,
    /// One JSON document per line, for lists
    Jsonl,
    /// A human-readable table (default on a terminal)
    Table,
}

#[derive(Debug, Clone, Copy)]
pub struct Printer {
    pub format: Format,
    /// Print only identifiers (one per line) instead of the full resource.
    pub quiet: bool,
}

impl Printer {
    pub fn new(format: Option<Format>, quiet: bool) -> Self {
        let format = format.unwrap_or_else(|| {
            if io::stdout().is_terminal() {
                Format::Table
            } else {
                Format::Json
            }
        });
        Self { format, quiet }
    }

    /// Print a list of resources. JSON is the array as the API returned it.
    /// With `-q`, the first column is the identifier that gets printed.
    pub fn list(&self, items: &[Value], columns: &[Column]) -> Result<()> {
        if self.quiet {
            return print_ids(items, columns);
        }
        match self.format {
            Format::Json => print_json(&Value::Array(items.to_vec())),
            Format::Jsonl => print_jsonl(items),
            Format::Table => print_table(items, columns),
        }
    }

    /// Print a free-form JSON document (the `me` summary, a raw call).
    pub fn raw(&self, value: &Value) -> Result<()> {
        match self.format {
            Format::Table => print_flat(value),
            Format::Json | Format::Jsonl => print_json(value),
        }
    }

    /// A line of status for humans; suppressed in JSON mode so stdout stays
    /// parseable. Goes to stderr.
    pub fn note(&self, text: &str) {
        if self.format == Format::Table {
            eprintln!("{}", sanitize(text));
        }
    }
}

/// A column in a table: header and a JSON pointer (`/slug`, `/data/title`).
#[derive(Debug, Clone, Copy)]
pub struct Column {
    pub header: &'static str,
    pub pointer: &'static str,
}

pub const fn col(header: &'static str, pointer: &'static str) -> Column {
    Column { header, pointer }
}

pub fn print_json(value: &Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

fn print_jsonl(items: &[Value]) -> Result<()> {
    let mut out = io::stdout().lock();
    for item in items {
        serde_json::to_writer(&mut out, item)?;
        writeln!(out)?;
    }
    Ok(())
}

fn print_ids(items: &[Value], columns: &[Column]) -> Result<()> {
    let mut out = io::stdout().lock();
    for item in items {
        writeln!(out, "{}", id_of(item, columns))?;
    }
    Ok(())
}

fn id_of(item: &Value, columns: &[Column]) -> String {
    columns
        .first()
        .and_then(|c| item.pointer(c.pointer))
        .map_or_else(|| cell(item), cell)
}

fn print_table(items: &[Value], columns: &[Column]) -> Result<()> {
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(columns.iter().map(|c| c.header));
    for item in items {
        table.add_row(
            columns
                .iter()
                .map(|c| truncate(&cell(item.pointer(c.pointer).unwrap_or(&Value::Null)))),
        );
    }
    let mut out = io::stdout().lock();
    writeln!(out, "{table}")?;
    Ok(())
}

fn print_flat(value: &Value) -> Result<()> {
    let mut out = io::stdout().lock();
    let mut rows = Vec::new();
    flatten("", value, &mut rows);
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(["field", "value"]);
    for (k, v) in rows {
        // keys come from the server too
        table.add_row([sanitize(&k), truncate(&v)]);
    }
    writeln!(out, "{table}")?;
    Ok(())
}

fn flatten(prefix: &str, value: &Value, rows: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, rows);
            }
        }
        other => rows.push((prefix.to_string(), cell(other))),
    }
}

/// Drop control characters (keeping newlines and tabs) from text bound for a
/// terminal: site content must not be able to send escape sequences that
/// rewrite the screen, the window title or the clipboard.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

/// Render a JSON value as a table cell.
pub fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => sanitize(s),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => items.iter().map(cell).collect::<Vec<_>>().join(", "),
        Value::Object(_) => sanitize(&value.to_string()),
    }
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 120;
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > LIMIT {
        let cut: String = flat.chars().take(LIMIT).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

/// The fields of an API error response beyond the `error` and `code` that the
/// message and code already carry (publish's `errors`, a 409's `lockedPages`).
fn extra_fields(body: &Value) -> Vec<(&String, &Value)> {
    body.as_object()
        .map(|map| {
            map.iter()
                .filter(|(key, _)| !matches!(key.as_str(), "error" | "code"))
                .collect()
        })
        .unwrap_or_default()
}

/// The error as the JSON document printed on stderr in JSON mode.
pub fn error_document(err: &Error) -> Value {
    let mut doc = serde_json::Map::new();
    doc.insert("code".into(), Value::String(err.code().to_string()));
    doc.insert("message".into(), Value::String(err.message()));
    if let Some(status) = err.status() {
        doc.insert("status".into(), Value::from(status));
    }
    if let Some(body) = err.body() {
        doc.insert("body".into(), body.clone());
    }
    if let Some(retry) = err.retry_after() {
        doc.insert("retry_after".into(), Value::from(retry));
    }
    if let Some(hint) = err.hint() {
        doc.insert("hint".into(), Value::String(hint.to_string()));
    }
    doc.insert("exit_code".into(), Value::from(err.exit_code()));
    serde_json::json!({ "error": Value::Object(doc) })
}

/// Print an error to stderr in the selected format.
pub fn print_error(err: &Error, format: Format) {
    match format {
        Format::Json | Format::Jsonl => eprintln!("{}", error_document(err)),
        Format::Table => {
            eprintln!("error: {}", sanitize(&err.to_string()));
            for (name, value) in err.body().map(extra_fields).unwrap_or_default() {
                eprintln!("  {}: {}", sanitize(name), sanitize(&value.to_string()));
            }
            if let Some(retry) = err.retry_after() {
                eprintln!("  retry after {retry}s");
            }
            if let Some(hint) = err.hint() {
                eprintln!("  hint: {}", sanitize(hint));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_terminal_escapes_from_cells() {
        let title = Value::String("Hi\u{1b}]0;PWNED\u{7}\u{1b}[31m red\u{9b}2J\nnext\tcol".into());
        assert_eq!(cell(&title), "Hi]0;PWNED[31m red2J\nnext\tcol");
        let nested = json!({"k": "a\u{1b}[2Jb"});
        assert!(!cell(&nested).chars().any(char::is_control));
    }

    #[test]
    fn strips_terminal_escapes_from_field_names() {
        let doc = json!({"evil\u{1b}]52;c;cHduZWQ=\u{7}": {"\u{1b}[2Jkey": "v"}});
        let mut rows = Vec::new();
        flatten("", &doc, &mut rows);
        let shown: Vec<String> = rows.iter().map(|(k, _)| sanitize(k)).collect();
        assert_eq!(shown, vec!["evil]52;c;cHduZWQ=.[2Jkey".to_string()]);
    }

    #[test]
    fn quiet_prints_the_first_column() {
        let columns = [col("pageUuid", "/pageUuid"), col("slug", "/slug")];
        assert_eq!(
            id_of(&json!({"pageUuid": "p1", "slug": "s"}), &columns),
            "p1"
        );
        assert_eq!(id_of(&json!("bare"), &columns), "bare");
    }

    #[test]
    fn cells_summarise_nested_values() {
        assert_eq!(cell(&json!(["a", 2, true])), "a, 2, true");
        assert_eq!(cell(&json!(null)), "");
        assert_eq!(cell(&json!({"mode": "live"})), r#"{"mode":"live"}"#);
    }

    #[test]
    fn truncates_long_text() {
        let long = "word ".repeat(100);
        let out = truncate(&long);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 121);
    }

    #[test]
    fn the_error_document_keeps_the_whole_api_body() {
        let body = json!({"error": "Draft has errors", "errors": [{"blockInstanceUuid": "b1"}]});
        let err = Error::Api {
            status: 422,
            code: "validation_error".into(),
            message: "Draft has errors".into(),
            body: Some(body.clone()),
            retry_after: None,
        };
        let doc = error_document(&err);
        assert_eq!(doc["error"]["code"], "validation_error");
        assert_eq!(doc["error"]["status"], 422);
        assert_eq!(doc["error"]["exit_code"], 5);
        assert_eq!(doc["error"]["body"], body);
        let extras = extra_fields(&body);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, "errors");
    }
}
