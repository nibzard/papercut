//! Command dispatch and the success/failure boundary.
//!
//! Exit codes are honest and deterministic:
//!   - 0 = success
//!   - 1 = real failure (report NOT recorded, unwritable store, …)
//!   - 2 = usage error (handled by clap before `run`)
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
    /// Success: `data` for JSON mode, `text` for text mode.
    Ok { data: Value, text: String },
    /// Real failure → exit 1.
    Err(ErrorItem),
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
        RunResult::Err(item) => {
            match mode {
                OutputMode::Json => print_json(&envelope_err(item.clone())),
                OutputMode::Text => eprintln!("error: {}", item.message),
            }
            1
        }
    }
}
