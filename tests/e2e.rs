//! End-to-end CLI tests via the built binary, against an isolated fake HOME.

mod common;

use common::IsolatedEnv;
use std::process::{Command, Stdio};

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(common::bin())
        .args(args)
        .output()
        .expect("run papercut");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(common::bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run papercut");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn fresh_home_install_add_render() {
    let env = IsolatedEnv::new().with_claude();

    // install wires the managed block + hook; --yes is the required go-ahead.
    let (c, _, _) = run(&["install", "--yes"]);
    assert_eq!(c, 0);
    let claude_md = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude_md.contains("papercut:begin v1"));

    // add records an event and prints the id in text mode.
    let (c, out, err) = run(&["add", "shell ate my glob"]);
    assert_eq!(c, 0, "stderr: {err}");
    let id = out.trim();
    assert!(papercut::id::looks_like_id(id));

    // json mode carries the id in the envelope.
    let (c, out, _) = run(&["--output", "json", "add", "second report"]);
    assert_eq!(c, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "ok");
    assert!(v["data"]["id"].as_str().unwrap().starts_with("pc_"));

    // list shows both.
    let (c, out, _) = run(&["list"]);
    assert_eq!(c, 0);
    assert!(out.contains("2 open"));
    assert!(out.contains("shell ate my glob"));
    assert!(out.contains("second report"));

    // render is deterministic markdown.
    let (c, out, _) = run(&["render"]);
    assert_eq!(c, 0);
    assert!(out.contains("# Papercuts"));
    assert!(out.contains("## open (2)"));
}

#[test]
fn add_multiline_and_quoted_message() {
    let _env = IsolatedEnv::new();
    let msg = "line one\nline two with -d flag and 'quotes'";
    let (c, out, _) = run(&["add", "--", msg]);
    assert_eq!(c, 0);
    let id = out.trim();
    let (events, _skip) = papercut::store::read_all_events();
    let ev = events.iter().find(|e| e.id == id).unwrap();
    assert_eq!(ev.summary, msg);
}

#[test]
fn add_in_non_repo_dir_still_records() {
    let _env = IsolatedEnv::new();
    let tmp = std::env::temp_dir().join(format!("pc-nongit-{}", papercut::id::new_id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let (c, out, _) = run_in(&tmp, &["add", "no git here"]);
    assert_eq!(c, 0);
    let id = out.trim();
    let (events, _) = papercut::store::read_all_events();
    let ev = events.iter().find(|e| e.id == id).unwrap();
    assert!(
        ev.context.repo.is_none(),
        "no repo metadata outside a git repo"
    );
}

#[test]
fn empty_message_is_real_failure_exit_1() {
    let _env = IsolatedEnv::new();
    let (c, _out, _err) = run(&["add", "   "]);
    assert_eq!(c, 1, "empty message must be exit 1, not recorded");
}

#[test]
fn usage_error_is_exit_2() {
    let _env = IsolatedEnv::new();
    // No required message argument.
    let (c, _out, _err) = run(&["add"]);
    assert_eq!(c, 2, "usage error must be exit 2");
}

/// `--help` is part of the documented contract: it exits 0 (not clap's 2) and
/// carries the exit-code table and EXAMPLES, so an agent introspecting usage
/// learns the real command surface and the JSON envelope.
#[test]
fn help_exits_zero_with_contract_text() {
    let _env = IsolatedEnv::new();
    let (c, out, _err) = run(&["--help"]);
    assert_eq!(c, 0, "--help must exit 0");
    assert!(
        out.contains("Exit codes"),
        "help documents exit codes: {out}"
    );
    assert!(out.contains("EXAMPLES"), "help lists examples: {out}");
    assert!(
        out.contains("--output json"),
        "help documents the json envelope: {out}"
    );
}

/// `install` modifies N config files; the explicit go-ahead is required.
/// Bare `papercut install` is a usage error, not a silent proceed.
#[test]
fn install_without_yes_is_usage_error() {
    let _env = IsolatedEnv::new();
    let (c, _out, err) = run(&["install"]);
    assert_eq!(c, 2, "missing --yes must be a usage error");
    assert!(err.contains("--yes"), "the error names the flag: {err}");
}

/// An unknown `--harness` id is a usage error (exit 2), never a silent no-op —
/// across sweep, install, and uninstall.
#[test]
fn unknown_harness_is_usage_error_exit_2() {
    let _env = IsolatedEnv::new();
    // install needs --yes too (it is required), so the only usage fault here
    // is the unknown harness id.
    let cases: &[&[&str]] = &[
        &["--output", "json", "sweep", "--harness", "codx"],
        &["--output", "json", "install", "--yes", "--harness", "codx"],
        &["--output", "json", "uninstall", "--harness", "codx"],
    ];
    for args in cases {
        let (c, out, _err) = run(args);
        assert_eq!(c, 2, "unknown harness must exit 2: {args:?}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error", "envelope status: {args:?}");
        assert_eq!(
            v["errors"][0]["code"], "unknown_harness",
            "error code: {args:?}"
        );
        assert!(
            v["errors"][0]["hint"]
                .as_str()
                .unwrap()
                .contains("known harness ids"),
            "hint lists valid ids: {args:?}"
        );
    }
}

/// doctor unhealthy: the JSON envelope's status must agree with the exit code
/// (status `error`, an `unhealthy` error item, exit 1) — not `status: ok` with
/// a buried `data.healthy: false`.
#[test]
fn doctor_unhealthy_envelope_matches_exit_code() {
    let _env = IsolatedEnv::new().with_claude();
    // No install → doctor finds the block missing → unhealthy.
    let (c, out, _err) = run(&["--output", "json", "doctor"]);
    assert_eq!(c, 1, "unhealthy doctor exits 1");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "error", "envelope status agrees with exit 1");
    assert_eq!(v["errors"][0]["code"], "unhealthy");
    assert_eq!(
        v["data"]["healthy"], false,
        "full checks payload still present"
    );
}

/// The hook path must stay silent and exit 0 even on a malformed invocation
/// (corrupted wiring). `_hook` with NO harness arg, and with a bad extra flag,
/// must never reach clap's usage-error path (exit 2 + stderr).
#[test]
fn hook_malformed_invocation_is_silent_exit_0() {
    let _env = IsolatedEnv::new();

    // Missing harness arg: no-op, silent, exit 0.
    let (c, out, err) = run(&["_hook"]);
    assert_eq!(c, 0, "missing hook arg still exits 0");
    assert!(
        out.is_empty() && err.is_empty(),
        "silent on missing arg: {err}"
    );

    // An extra flag after the harness must not trip clap (exit 2). The hook
    // runs with the harness it got and ignores the rest, silent, exit 0.
    let (c, out, err) = run(&["_hook", "claude-code", "--bogus-flag"]);
    assert_eq!(c, 0, "extra hook arg still exits 0, not clap's 2");
    assert!(
        out.is_empty() && err.is_empty(),
        "silent on extra arg: {err}"
    );
}

#[test]
fn json_envelope_shape_on_error() {
    let _env = IsolatedEnv::new();
    let (c, out, _) = run(&["--output", "json", "add", ""]);
    assert_eq!(c, 1);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "error");
    assert!(v["errors"].is_array());
    let err = &v["errors"][0];
    assert!(err["code"].is_string());
    assert!(err["retryable"].is_boolean());
    assert!(err["hint"].is_string());
}

#[test]
fn hook_is_silent_and_exits_zero_regardless_of_input() {
    let _env = IsolatedEnv::new();
    // The verified PostToolUseFailure payload shape (Claude Code v2.1.223,
    // captured 2026-08-06).
    let payload = serde_json::json!({
        "session_id": "abc",
        "cwd": "/proj",
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "tool_input": {"command": "false", "description": "test"},
        "error": "Exit code 1\nnope",
        "is_interrupt": false,
    });
    let mut cmd = Command::new(common::bin());
    cmd.args(["_hook", "claude-code"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "hook must not print to stdout");
    assert!(out.stderr.is_empty(), "hook must not print to stderr");

    // garbage stdin → still silent and 0
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
