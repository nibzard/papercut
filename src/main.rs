use clap::error::ErrorKind;
use clap::Parser;
use papercut::cli::Cli;
use papercut::output::{envelope_err, print_json, ErrorItem, OutputMode};
use serde_json::Value;

fn main() {
    let mode = detect_output_mode();

    // The hook path is silent and infallible from the caller's point of view.
    // Intercept it BEFORE clap: corrupted settings.json wiring can invoke the
    // hook with a missing or malformed argument, and that must never reach
    // clap's usage-error path (exit 2 + stderr) — which Claude Code surfaces.
    // A missing harness arg no-ops; any error is swallowed; always exit 0.
    if std::env::args().nth(1).as_deref() == Some("_hook") {
        let harness = std::env::args().nth(2).unwrap_or_default();
        papercut::commands::hook::run(papercut::cli::HookArgs { harness });
        std::process::exit(0);
    }

    let code = match Cli::try_parse() {
        Ok(cli) => papercut::run(cli),
        Err(e) => match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                // Help/version are not errors: print and exit 0.
                let _ = e.print();
                0
            }
            _ => {
                if matches!(mode, OutputMode::Json) {
                    // Honor the JSON-everywhere contract even on the usage-error
                    // path, so an agent that always passes --output json gets a
                    // parseable envelope (exit 2) instead of clap's plain text.
                    let item = ErrorItem::new(
                        "usage",
                        e.to_string(),
                        false,
                        "run `papercut --help` or `papercut <command> --help`",
                    );
                    print_json(&envelope_err(Value::Null, vec![item]));
                    2
                } else {
                    let _ = e.print();
                    2
                }
            }
        },
    };
    std::process::exit(code);
}

/// Best-effort pre-scan for the global `--output` flag. clap fails before the
/// global flag is reliably captured on a usage error, so we look at argv first.
/// Recognizes `--output json` and `--output=json`; stops at a literal `--`.
fn detect_output_mode() -> OutputMode {
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--" {
            break;
        }
        if a == "--output" {
            if let Some(v) = args.next() {
                if v == "json" {
                    json = true;
                }
            }
        } else if let Some(v) = a.strip_prefix("--output=") {
            if v == "json" {
                json = true;
            }
        }
    }
    if json {
        OutputMode::Json
    } else {
        OutputMode::Text
    }
}
