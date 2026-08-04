//! Codex sweep adapter: parsing session rollouts into signals, incremental
//! idempotence, and graceful degradation on unknown formats.

mod common;

use common::IsolatedEnv;
use papercut::adapters::codex;
use std::io::Write;

fn write_rollout(path: &std::path::Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

const META: &str = r#"{"timestamp":"2026-08-04T20:00:00.000Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/tmp/proj","git":{"commit_hash":"abc1234"}}}"#;
const CALL: &str = r#"{"timestamp":"2026-08-04T20:00:01.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"make build\"}","call_id":"c1"}}"#;
const OUT_FAIL: &str = r#"{"timestamp":"2026-08-04T20:00:02.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Wall time: 1.0 seconds\nProcess exited with code 2\nOutput:\nError: no rule to make target\nmore\nlines\n"}}"#;

#[test]
fn failing_command_produces_signal() {
    let env = IsolatedEnv::new().with_codex();
    let f = env.home.join(".codex/sessions/2026/08/04/rollout-a.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));

    let o = codex::sweep();
    assert_eq!(o.signals_emitted, 1);
    assert!(o.warning.is_none(), "recognized format must not warn");

    let (sigs, _skip) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].cmd, "make build");
    assert_eq!(sigs[0].exit, 2);
    assert_eq!(sigs[0].session.as_deref(), Some("sess-1"));
    assert_eq!(sigs[0].cwd.as_deref(), Some("/tmp/proj"));
    assert_eq!(sigs[0].agent.as_deref(), Some("codex"));
}

#[test]
fn zero_exit_produces_no_signal() {
    let env = IsolatedEnv::new().with_codex();
    let out_ok = r#"{"timestamp":"2026-08-04T20:00:03.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Process exited with code 0\n"}}"#;
    let f = env.home.join(".codex/sessions/2026/08/04/rollout-ok.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{out_ok}\n"));

    let o = codex::sweep();
    assert_eq!(o.signals_emitted, 0);
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert!(sigs.is_empty());
}

#[test]
fn incremental_sweep_does_not_duplicate_then_picks_up_new() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-inc.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));

    let o1 = codex::sweep();
    assert_eq!(o1.signals_emitted, 1);

    let o2 = codex::sweep();
    assert_eq!(
        o2.signals_emitted, 0,
        "re-sweeping unchanged logs is a no-op"
    );
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 1);

    // Append a second failing command with a fresh call_id to the same file.
    let call2 = r#"{"timestamp":"2026-08-04T20:00:04.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo build\"}","call_id":"c2"}}"#;
    let out2 = r#"{"timestamp":"2026-08-04T20:00:05.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":"Process exited with code 1\nOutput:\nerror[E0308]\n"}}"#;
    let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
    fh.write_all(format!("{call2}\n{out2}\n").as_bytes())
        .unwrap();
    drop(fh);

    let o3 = codex::sweep();
    assert_eq!(o3.signals_emitted, 1, "appended content is picked up");
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 2);
}

#[test]
fn unknown_format_is_noop_with_warning() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-weird.jsonl");
    write_rollout(&f, "{\"weird\":\"format\",\"foo\":1}\n{\"bar\":2}\n");

    let o = codex::sweep();
    assert_eq!(o.signals_emitted, 0);
    assert!(
        o.warning.is_some(),
        "unrecognized format must surface a warning"
    );
}

#[test]
fn missing_sessions_dir_warns_cleanly() {
    let _env = IsolatedEnv::new(); // no .codex/sessions
    let o = codex::sweep();
    assert_eq!(o.signals_emitted, 0);
    assert!(
        o.warning.is_some(),
        "absent sessions dir is a warning, not an error"
    );
}
