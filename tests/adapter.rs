//! A stale hook must be silent and must not create unattributed signals.

mod common;

use common::IsolatedEnv;
use std::io::Write;
use std::process::{Command, Stdio};

fn hook(payload: &[u8]) {
    let mut child = Command::new(common::bin())
        .args(["_hook", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}

#[test]
fn failed_bash_payload_is_silently_ignored() {
    let _env = IsolatedEnv::new();
    hook(br#"{"tool_name":"Bash","tool_input":{"command":"make test"},"error":"Exit code 2\nboom","session_id":"old-session"}"#);
    let (signals, _) = papercut::signal::read_signals("claude-code");
    assert!(signals.is_empty());
    assert!(!papercut::signal::signals_dir("claude-code")
        .unwrap()
        .exists());
}

#[test]
fn malformed_payload_is_silently_ignored() {
    let _env = IsolatedEnv::new();
    hook(b"not JSON");
    let (signals, _) = papercut::signal::read_signals("claude-code");
    assert!(signals.is_empty());
}

#[test]
fn historical_partial_signal_is_still_readable() {
    let _env = IsolatedEnv::new();
    let sig = papercut::signal::Signal::new(
        "2026-08-04T20:00:00Z",
        None,
        None,
        Some("claude-code".into()),
        "old failed command",
        1,
        None,
        Some("sess-m".into()),
    );
    papercut::signal::append_signal("claude-code", "sess-m", &sig).unwrap();
    let path = papercut::signal::session_file("claude-code", "sess-m").unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"{\"partial")
        .unwrap();
    let (signals, skipped) = papercut::signal::read_signals("claude-code");
    assert_eq!(signals.len(), 1);
    assert_eq!(skipped, 1);
}
