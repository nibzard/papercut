//! Claude Code live adapter: the hook capture path and settings.json wiring.
//!
//! The hook is the most invariant-sensitive code in the project — it must be
//! silent and infallible from the parent task's point of view, recording a
//! signal only for non-zero Bash exits.

mod common;

use common::IsolatedEnv;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// Run the hook with a JSON payload; assert it exits 0 and prints nothing.
fn hook(payload: &Value) {
    let mut cmd = Command::new(common::bin());
    cmd.args(["_hook", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "hook must always exit 0");
    assert!(out.stdout.is_empty(), "hook must never print to stdout");
}

fn session_signals(session: &str) -> Vec<Value> {
    let path = papercut::signal::session_file("claude-code", session).unwrap();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[test]
fn non_zero_bash_records_one_signal() {
    let _env = IsolatedEnv::new();
    hook(&json!({
        "tool_name": "Bash",
        "tool_input": {"command": "make test"},
        "tool_result": {"exit_code": 2, "output": "boom"},
        "session_id": "sess-a",
        "cwd": "/proj",
    }));

    let sigs = session_signals("sess-a");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["cmd"], "make test");
    assert_eq!(sigs[0]["exit"], 2);
    assert_eq!(sigs[0]["agent"], "claude-code");
    assert_eq!(sigs[0]["session"], "sess-a");
    assert_eq!(sigs[0]["cwd"], "/proj");
    assert_eq!(sigs[0]["stderr_head"], "boom");
}

#[test]
fn zero_exit_records_nothing() {
    let _env = IsolatedEnv::new();
    hook(&json!({
        "tool_name": "Bash",
        "tool_input": {"command": "true"},
        "tool_result": {"exit_code": 0, "output": ""},
        "session_id": "sess-b",
    }));
    assert!(session_signals("sess-b").is_empty());
}

#[test]
fn non_bash_tool_records_nothing() {
    let _env = IsolatedEnv::new();
    hook(&json!({
        "tool_name": "Edit",
        "tool_input": {"file_path": "/x"},
        "tool_result": {"exit_code": 1},
        "session_id": "sess-c",
    }));
    assert!(session_signals("sess-c").is_empty());
}

#[test]
fn garbage_stdin_is_swallowed_silently() {
    let _env = IsolatedEnv::new();
    let mut cmd = Command::new(common::bin());
    cmd.args(["_hook", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"not json at all {{{")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn long_command_is_truncated() {
    let _env = IsolatedEnv::new();
    let big = "x".repeat(5000);
    hook(&json!({
        "tool_name": "Bash",
        "tool_input": {"command": big},
        "tool_result": {"exit_code": 1, "output": "e"},
        "session_id": "sess-d",
    }));
    let sigs = session_signals("sess-d");
    assert_eq!(sigs.len(), 1);
    let cmd = sigs[0]["cmd"].as_str().unwrap();
    assert!(cmd.chars().count() <= papercut::util::CMD_MAX + 1);
    assert!(cmd.ends_with('…'), "truncated value ends with an ellipsis");
}

#[test]
fn large_stderr_head_is_bounded() {
    let _env = IsolatedEnv::new();
    let huge = "line of noise\n".repeat(200);
    hook(&json!({
        "tool_name": "Bash",
        "tool_input": {"command": "flaky"},
        "tool_result": {"exit_code": 3, "output": huge},
        "session_id": "sess-e",
    }));
    let sigs = session_signals("sess-e");
    assert_eq!(sigs.len(), 1);
    let head = sigs[0]["stderr_head"].as_str().unwrap();
    assert!(head.lines().count() <= papercut::util::STDERR_MAX_LINES);
    assert!(head.chars().count() <= papercut::util::STDERR_MAX_CHARS);
}

/// A signal file interrupted mid-append (the hook killed between write_all calls)
/// leaves a partial line. Reads must skip it and never crash — the parent task
/// already moved on and must never notice.
#[test]
fn partial_signal_line_is_skipped_not_fatal() {
    let _env = IsolatedEnv::new();
    let sig = papercut::signal::Signal::new(
        "2026-08-04T20:00:00Z",
        None,
        None,
        Some("claude-code".into()),
        "ok cmd",
        1,
        None,
        Some("sess-m".into()),
    );
    papercut::signal::append_signal("claude-code", "sess-m", &sig).unwrap();

    // Simulate a torn write: append a half line.
    let path = papercut::signal::session_file("claude-code", "sess-m").unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"partial")
        .unwrap();

    let (sigs, skipped) = papercut::signal::read_signals("claude-code");
    assert_eq!(sigs.len(), 1, "valid signal still reads");
    assert_eq!(skipped, 1, "torn line is skipped, not fatal");
}

/// A large-but-valid payload (well under the stdin cap) still records exactly
/// one signal — guards the stdin cap against being too small for real payloads
/// with sizable command output.
#[test]
fn large_valid_payload_still_records() {
    let _env = IsolatedEnv::new();
    let big_output = "noise line\n".repeat(4000); // ~45 KB, under the 1 MiB cap
    hook(&json!({
        "tool_name": "Bash",
        "tool_input": {"command": "make build"},
        "tool_result": {"exit_code": 1, "output": big_output},
        "session_id": "sess-big",
    }));
    let sigs = session_signals("sess-big");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["cmd"], "make build");
    assert_eq!(sigs[0]["exit"], 1);
}
