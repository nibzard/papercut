//! Command dispatch and the success/failure boundary.
//!
//! Exit codes are honest and deterministic: 0 = success; 1 = real failure (a
//! report NOT recorded, an unwritable store, …) or `doctor` finding problems
//! (`Health { healthy: false }`); 2 = usage error (handled by clap before
//! `run`, in `main`).
//!
//! Only the hook path (`_hook`) deviates: it is always silent and always 0,
//! so a signal capture can never surface in the parent task.

use crate::cli::{Cli, Command};
use crate::commands;
use crate::output::{envelope_err, envelope_ok, print_json, ErrorItem, OutputMode};
use serde_json::Value;

/// What a command produced.
#[derive(Debug)]
pub enum RunResult {
    /// Success: `data` for JSON mode, `text` for text mode. Exits 0.
    Ok { data: Value, text: String },
    /// Real failure → exit 1. `data` carries any partial results (e.g. the
    /// harnesses an install DID configure before one failed) so an agent does
    /// not lose the successful work; `errors` carries one or more structured
    /// errors. The hook path never produces this.
    Err { data: Value, errors: Vec<ErrorItem> },
    /// A successful diagnostic that may still report problems. `data`/`text`
    /// are emitted under the normal `ok` envelope; the process exits 0 when
    /// `healthy` and 1 otherwise. Used by `doctor`: it ran fine (so the full
    /// checks payload stays in `data`), but problems still surface as exit 1.
    Health { data: Value, text: String, healthy: bool },
}

impl RunResult {
    /// Single-error convenience for the common failure case (no partial data).
    pub fn err(item: ErrorItem) -> Self {
        Self::Err {
            data: Value::Null,
            errors: vec![item],
        }
    }
}

/// Run a parsed CLI to completion, returning the process exit code.
pub fn run(cli: Cli) -> i32 {
    let mode = cli.output;

    // The hook path is the exception: silent and infallible from the caller's
    // point of view, regardless of output mode.
    if let Command::Hook(args) = cli.command {
        commands::hook::run(args);
        return 0;
    }

    let res = match cli.command {
        Command::Add(a) => commands::add::run(a),
        Command::List(a) => commands::list::run(a),
        Command::Render(a) => commands::render::run(a),
        Command::Install(a) => commands::install::run(a),
        Command::Uninstall(a) => commands::uninstall::run(a),
        Command::Doctor => commands::doctor::run(),
        Command::Sweep(a) => commands::sweep::run(a),
        Command::TriagePack(a) => commands::triage_pack::run(a),
        Command::Hook(_) => unreachable!(),
    };

    match res {
        RunResult::Ok { data, text } => {
            match mode {
                OutputMode::Json => print_json(&envelope_ok(data)),
                OutputMode::Text => println!("{text}"),
            }
            0
        }
        RunResult::Health {
            data,
            text,
            healthy,
        } => {
            match mode {
                OutputMode::Json => print_json(&envelope_ok(data)),
                OutputMode::Text => println!("{text}"),
            }
            if healthy {
                0
            } else {
                1
            }
        }
        RunResult::Err { data, errors } => {
            match mode {
                OutputMode::Json => print_json(&envelope_err(data, errors.clone())),
                OutputMode::Text => {
                    for item in &errors {
                        eprintln!("error: {}", item.message);
                    }
                }
            }
            1
        }
    }
}
