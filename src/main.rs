use firebird_cli::cli::{run, Cli};
use firebird_cli::error::Error;
use firebird_cli::output::{print_error, Format, Printer};

const ISSUES: &str = "https://github.com/RealGeeks/firebird-cli/issues";

fn main() {
    let cli = Cli::parse_args();
    let format = Printer::new(cli.global.format(), false).format;
    install_panic_hook(format);
    if let Err(err) = run(cli) {
        if matches!(err, Error::BrokenPipe) {
            std::process::exit(0);
        }
        let format = if matches!(format, Format::Jsonl) {
            Format::Json
        } else {
            format
        };
        print_error(&err, format);
        std::process::exit(err.exit_code());
    }
}

/// A bug should still honour the error contract: a JSON error on stderr in
/// JSON mode and exit code 1, not Rust's panic text and 101.
fn install_panic_hook(format: Format) {
    std::panic::set_hook(Box::new(move |info| {
        let detail = info
            .payload()
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        // `print!`/`println!` panic when stdout's reader has gone (`| head`);
        // stop quietly like other command-line tools instead of reporting a bug.
        if detail.starts_with("failed printing to stdout") {
            std::process::exit(0);
        }
        let message = format!("firebird-cli hit an internal error{location}: {detail}");
        if format == Format::Table {
            eprintln!("error: {message}\n  please report it: {ISSUES}");
        } else {
            let envelope = serde_json::json!({ "error": {
                "code": "internal_error",
                "message": message,
                "hint": format!("please report it: {ISSUES}"),
                "exit_code": 1,
            }});
            eprintln!("{envelope}");
        }
        std::process::exit(1);
    }));
}
