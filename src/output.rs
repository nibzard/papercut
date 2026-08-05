//! The single output envelope used by every command in `--output json` mode,
//! and the terse human text used otherwise.
//!
//! Envelope shape (stable, minimal):
//! ```jsonc
//! { "schema_version": 1, "status": "ok"|"error", "data": <any>, "errors": [ ... ] }
//! ```

use serde::Serialize;
use serde_json::Value;

/// How a command prints its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OutputMode {
    #[default]
    #[value(name = "text")]
    Text,
    #[value(name = "json")]
    Json,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorItem {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub hint: String,
}

impl ErrorItem {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            hint: hint.into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Envelope {
    pub schema_version: u32,
    pub status: &'static str,
    pub data: Value,
    pub errors: Vec<ErrorItem>,
}

pub const ENVELOPE_SCHEMA_VERSION: u32 = 1;

pub fn envelope_ok(data: Value) -> Envelope {
    Envelope {
        schema_version: ENVELOPE_SCHEMA_VERSION,
        status: "ok",
        data,
        errors: vec![],
    }
}

pub fn envelope_err(data: Value, errors: Vec<ErrorItem>) -> Envelope {
    Envelope {
        schema_version: ENVELOPE_SCHEMA_VERSION,
        status: "error",
        data,
        errors,
    }
}

/// Print an envelope as pretty JSON to stdout.
pub fn print_json(env: &Envelope) {
    // to_string_pretty never fails for our value types.
    println!(
        "{}",
        serde_json::to_string_pretty(env).unwrap_or_else(|_| "{}".into())
    );
}

/// Print a human-facing message to stderr.
pub fn eprintln_text(msg: &str) {
    eprintln!("{msg}");
}
