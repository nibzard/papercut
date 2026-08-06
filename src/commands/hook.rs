//! `papercut _hook <harness>` — the live hook entry point.
//!
//! Invariant: this path is invisible and infallible to the caller. It reads the
//! harness payload on stdin, records a signal if a Bash command exited non-zero,
//! and swallows every error. The command always exits 0 and never prints.

use crate::cli::HookArgs;
use crate::signal::{append_signal, Signal};
use crate::time::now_rfc3339;
use crate::util::STDIN_MAX;
use serde_json::Value;
use std::io::Read;

pub fn run(args: HookArgs) {
    let _ = capture(&args.harness);
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

    let exit = payload
        .pointer("/tool_result/exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if exit == 0 {
        return Ok(());
    }

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
    let output = payload
        .pointer("/tool_result/output")
        .and_then(|v| v.as_str());

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
        exit as i32,
        output,
        session.clone(),
    );

    let sess = session.unwrap_or_else(|| "unknown".to_string());
    // Appending is best-effort; a broken sink (or an unsafe session id that the
    // store refuses to turn into a path) never surfaces in the task.
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
