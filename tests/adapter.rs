//! Claude Code live adapter: the hook capture path and settings.json wiring.
//!
//! The hook is the most invariant-sensitive code in the project — it must be
//! silent and infallible from the parent task's point of view, recording a
//! signal only for failed Bash commands.
//!
//! Payload fixtures mirror payloads captured live from Claude Code v2.1.223
//! on 2026-08-06. A failed Bash command fires `PostToolUseFailure` with a
//! top-level `error` string ("Exit code N\n<output>") and an `is_interrupt`
//! bool; there is no `tool_response` on failures. A successful command fires
//! `PostToolUse` with a `tool_response` object and no exit information.
//! Re-verify against the installed harness on updates — do not edit by hand.

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
    assert!(out.stderr.is_empty(), "hook must never print to stderr");
}

/// The verified failure payload shape (PostToolUseFailure).
fn failure_payload(cmd: &str, error: &str, session: &str) -> Value {
    json!({
        "session_id": session,
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/proj",
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "tool_input": {"command": cmd, "description": "test"},
        "tool_use_id": "toolu_test",
        "error": error,
        "is_interrupt": false,
        "duration_ms": 250,
    })
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
fn failed_bash_records_one_signal() {
    let _env = IsolatedEnv::new();
    hook(&failure_payload("make test", "Exit code 2\nboom", "sess-a"));

    let sigs = session_signals("sess-a");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["cmd"], "make test");
    assert_eq!(sigs[0]["exit"], 2);
    assert_eq!(sigs[0]["agent"], "claude-code");
    assert_eq!(sigs[0]["session"], "sess-a");
    assert_eq!(sigs[0]["cwd"], "/proj");
    assert_eq!(
        sigs[0]["stderr_head"], "boom",
        "the redundant Exit code line is stripped from the head"
    );
}

/// A failure whose error string carries no parseable exit code (e.g. a
/// timeout) still records, with the -1 sentinel: failed, code unknown.
#[test]
fn failure_with_unparseable_exit_still_records() {
    let _env = IsolatedEnv::new();
    hook(&failure_payload(
        "sleep 30",
        "Command timed out after 5s",
        "sess-t",
    ));
    let sigs = session_signals("sess-t");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["exit"], -1);
    assert_eq!(sigs[0]["stderr_head"], "Command timed out after 5s");
}

/// A user interruption is not repo friction: is_interrupt=true records nothing.
#[test]
fn interrupted_command_records_nothing() {
    let _env = IsolatedEnv::new();
    let mut p = failure_payload("sleep 999", "Exit code 130", "sess-i");
    p["is_interrupt"] = json!(true);
    hook(&p);
    assert!(session_signals("sess-i").is_empty());
}

/// The verified success shape: PostToolUse with a tool_response object and no
/// exit information. Records nothing.
#[test]
fn successful_bash_records_nothing() {
    let _env = IsolatedEnv::new();
    hook(&json!({
        "session_id": "sess-b",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/proj",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "true", "description": "test"},
        "tool_response": {
            "stdout": "", "stderr": "", "interrupted": false,
            "isImage": false, "noOutputExpected": false
        },
        "tool_use_id": "toolu_test",
        "duration_ms": 379,
    }));
    assert!(session_signals("sess-b").is_empty());
}

#[test]
fn non_bash_tool_records_nothing() {
    let _env = IsolatedEnv::new();
    let mut p = failure_payload("n/a", "Exit code 1", "sess-c");
    p["tool_name"] = json!("Edit");
    p["tool_input"] = json!({"file_path": "/x"});
    hook(&p);
    assert!(session_signals("sess-c").is_empty());
}

/// A hostile/garbage session id must neither escape signals/<harness>/ nor
/// drop the signal: the filename is sanitized, the signal is kept.
#[test]
fn unsafe_session_id_still_records_safely() {
    let env = IsolatedEnv::new();
    hook(&failure_payload(
        "make test",
        "Exit code 2\nboom",
        "../../../evil",
    ));
    let (sigs, _) = papercut::signal::read_signals("claude-code");
    assert_eq!(sigs.len(), 1, "signal kept under a sanitized filename");
    // Nothing escaped the signals dir.
    assert!(!env.data.join("papercuts/evil.jsonl").exists());
    assert!(!env.data.join("evil.jsonl").exists());
}

/// An absurdly long session id is bounded before it becomes a filename.
#[test]
fn oversized_session_id_still_records() {
    let _env = IsolatedEnv::new();
    let long_id = "s".repeat(1024);
    hook(&failure_payload("make test", "Exit code 2\nboom", &long_id));
    let (sigs, _) = papercut::signal::read_signals("claude-code");
    assert_eq!(sigs.len(), 1, "signal kept under a bounded filename");
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
    let mut p = failure_payload("placeholder", "Exit code 1\ne", "sess-d");
    p["tool_input"] = json!({"command": big, "description": "test"});
    hook(&p);
    let sigs = session_signals("sess-d");
    assert_eq!(sigs.len(), 1);
    let cmd = sigs[0]["cmd"].as_str().unwrap();
    assert!(cmd.chars().count() <= papercut::util::CMD_MAX + 1);
    assert!(cmd.ends_with('…'), "truncated value ends with an ellipsis");
}

#[test]
fn large_stderr_head_is_bounded() {
    let _env = IsolatedEnv::new();
    let huge = format!("Exit code 3\n{}", "line of noise\n".repeat(200));
    hook(&failure_payload("flaky", &huge, "sess-e"));
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
    let big_error = format!("Exit code 1\n{}", "noise line\n".repeat(4000)); // ~45 KB, under the 1 MiB cap
    hook(&failure_payload("make build", &big_error, "sess-big"));
    let sigs = session_signals("sess-big");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["cmd"], "make build");
    assert_eq!(sigs[0]["exit"], 1);
}
