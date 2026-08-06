//! `papercut _hook <harness>` — the live hook entry point.
//!
//! Invariant: this path is invisible and infallible to the caller. It reads the
//! harness payload on stdin, records a signal when a Bash command failed, and
//! swallows every error. The command always exits 0 and never prints.

use crate::cli::HookArgs;
use crate::signal::{append_signal, filename_session, Signal};
use crate::time::now_rfc3339;
use crate::util::{parse_leading_i32, STDIN_MAX};
use serde_json::Value;
use std::io::Read;

/// The prefix of the harness failure string, followed by the exit code.
const EXIT_NEEDLE: &str = "Exit code ";

pub fn run(args: HookArgs) {
    let _ = capture(&args.harness);
}

/// A failure derived from the payload: the exit code (`-1` = failed, code
/// unknown) and the output text kept as the stderr head.
struct Failure {
    exit: i32,
    output: Option<String>,
}

/// Derive a Bash failure from the payload, or `None` for success or a user
/// interruption.
///
/// Verified live against Claude Code v2.1.223 (2026-08-06): a failed Bash
/// command fires `PostToolUseFailure` with a top-level `error` STRING of the
/// form `"Exit code N\n<combined output>"` plus an `is_interrupt` bool; there
/// is no `tool_response` on failures. A successful command fires `PostToolUse`
/// with a `tool_response` object and no exit information. The match here is
/// content-based (the `error` field), so an event-name drift alone cannot
/// break it. Re-verify on harness updates — never trust memory or docs.
fn bash_failure(payload: &Value) -> Option<Failure> {
    if payload.get("is_interrupt").and_then(|v| v.as_bool()) == Some(true) {
        return None; // a user interruption is not repo friction
    }
    let error = payload.get("error").and_then(|v| v.as_str())?;
    match error.strip_prefix(EXIT_NEEDLE).and_then(parse_leading_i32) {
        Some(0) => None, // defensive: a zero exit is not a failure
        Some(exit) => {
            // Drop the "Exit code N" first line — it is redundant with the
            // exit field. The remainder is the combined output head.
            let rest = error.split_once('\n').map(|(_, r)| r.to_string());
            Some(Failure { exit, output: rest })
        }
        None => Some(Failure {
            // The harness reported a failure without a parseable code (for
            // example a timeout). -1 is the documented "code unknown" value.
            exit: -1,
            output: Some(error.to_string()),
        }),
    }
}

fn capture(harness: &str) -> anyhow::Result<()> {
    // Bound the read: a runaway stdin must never OOM the hook into a non-zero
    // exit. Anything beyond the cap is dropped, the truncated payload then fails
    // to parse below, and we record nothing — silent and infallible, as required.
    let mut buf = String::new();
    std::io::stdin()
        .take(STDIN_MAX as u64)
        .read_to_string(&mut buf)?;

    // Garbage on stdin must never crash the parent task.
    let payload: Value = serde_json::from_str(&buf).unwrap_or(Value::Null);

    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if tool != "Bash" {
        return Ok(());
    }

    let Some(failure) = bash_failure(&payload) else {
        return Ok(());
    };

    let cmd = payload
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cwd = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let session = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Resolve repo context (F6) so signals carry the repository they came from,
    // enabling cross-repo recurrence analysis at triage. Falls back to the
    // process cwd when the payload omits one; any git failure degrades to None.
    let repo = resolve_repo(cwd.as_deref());

    let sig = Signal::new(
        now_rfc3339(),
        repo,
        cwd,
        Some(harness.to_string()),
        cmd,
        failure.exit,
        failure.output.as_deref(),
        session.clone(),
    );

    // The file name is sanitized so a hostile session id keeps the signal
    // instead of dropping it; appending stays best-effort — a broken sink
    // never surfaces in the task.
    let sess = filename_session(session.as_deref().unwrap_or("unknown"));
    let _ = append_signal(harness, &sess, &sig);
    Ok(())
}

/// Resolve a normalized repo id for the signal, preferring the payload cwd and
/// falling back to the process cwd. Uses the lean two-call resolver (`repo_of`)
/// since the hook fires per failed Bash command and needs only `.repo`.
/// Captures only a pointer; never fails.
fn resolve_repo(payload_cwd: Option<&str>) -> Option<String> {
    let cur = std::env::current_dir().ok();
    let start: &std::path::Path = payload_cwd.map(std::path::Path::new).or(cur.as_deref())?;
    crate::git_meta::repo_of(start)
}
