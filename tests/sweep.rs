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

// A second failing pair with its own call id, for append scenarios.
const CALL2: &str = r#"{"timestamp":"2026-08-04T20:00:04.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo build\"}","call_id":"c2"}}"#;
const OUT2_FAIL: &str = r#"{"timestamp":"2026-08-04T20:00:05.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":"Process exited with code 1\nOutput:\nerror[E0308]\n"}}"#;

// A parallel session that sorts after the first one, with its own ids.
const META_B: &str = r#"{"timestamp":"2026-08-04T21:00:00.000Z","type":"session_meta","payload":{"id":"sess-2","cwd":"/tmp/other","git":{"commit_hash":"def5678"}}}"#;
const CALL_B: &str = r#"{"timestamp":"2026-08-04T21:00:01.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"npm test\"}","call_id":"d1"}}"#;
const OUT_B_FAIL: &str = r#"{"timestamp":"2026-08-04T21:00:02.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"d1","output":"Process exited with code 1\nOutput:\n1 failing\n"}}"#;

fn append_rollout(path: &std::path::Path, body: &str) {
    let mut fh = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    fh.write_all(body.as_bytes()).unwrap();
}

/// The root-relative watermark key `sweep` stores for a session file, mirroring
/// `codex::rel_key`: stable across a `$HOME` relocation because only the tree
/// under `sessions/` is recorded.
fn rel_key(home: &std::path::Path, p: &std::path::Path) -> String {
    p.strip_prefix(home.join(".codex/sessions"))
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

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
fn current_custom_exec_failure_produces_signal() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-current.jsonl");
    let call = r#"{"timestamp":"2026-09-19T10:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"cx1","input":"text(await tools.exec_command({cmd:\"cargo test\"}));"}}"#;
    let output = r#"{"timestamp":"2026-09-19T10:00:01Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx1","output":[{"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n{\"exit_code\":7,\"output\":\"synthetic failure\"}"}]}}"#;
    write_rollout(&f, &format!("{META}\n{call}\n{output}\n"));

    let outcome = codex::sweep();
    assert_eq!(outcome.signals_emitted, 1);
    let (signals, skipped) = papercut::signal::read_signals("codex");
    assert_eq!(skipped, 0);
    assert_eq!(signals[0].cmd, "cargo test");
    assert_eq!(signals[0].exit, 7);
}

#[test]
fn current_custom_exec_maps_each_result_to_its_command() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-current-many.jsonl");
    let call = r#"{"timestamp":"2026-09-19T10:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"cx1","input":"const a = await tools.exec_command({cmd:\"true\"}); const b = await tools.exec_command({cmd:\"false\"});"}}"#;
    let output = r#"{"timestamp":"2026-09-19T10:00:01Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx1","output":[{"type":"input_text","text":"Output:\n[{\"exit_code\":0,\"output\":\"\"},{\"exit_code\":3,\"output\":\"failed second\"}]"}]}}"#;
    write_rollout(&f, &format!("{META}\n{call}\n{output}\n"));

    let outcome = codex::sweep();
    assert_eq!(outcome.signals_emitted, 1);
    let (signals, _) = papercut::signal::read_signals("codex");
    assert_eq!(signals[0].cmd, "false");
    assert_eq!(signals[0].exit, 3);
}

#[test]
fn current_custom_exec_reads_json_after_status_text_blocks() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-current-blocks.jsonl");
    let call = r#"{"timestamp":"2026-09-19T10:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"cx1","input":"text(await tools.exec_command({cmd:\"cargo clippy\"}));"}}"#;
    let output = r#"{"timestamp":"2026-09-19T10:00:01Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx1","output":[{"type":"input_text","text":"Script completed\nOutput:\n"},{"type":"input_text","text":"Warning: truncated output\n\n{\"exit_code\":4,\"output\":\"lint failed\"}"}]}}"#;
    write_rollout(&f, &format!("{META}\n{call}\n{output}\n"));

    let outcome = codex::sweep();
    assert_eq!(outcome.signals_emitted, 1);
    let (signals, _) = papercut::signal::read_signals("codex");
    assert_eq!(signals[0].cmd, "cargo clippy");
    assert_eq!(signals[0].exit, 4);
}

#[test]
fn session_repository_url_takes_precedence_over_cwd_lookup() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-repo.jsonl");
    let meta = r#"{"timestamp":"2026-09-19T10:00:00Z","type":"session_meta","payload":{"id":"sess-repo","cwd":"/deleted/repo","git":{"repository_url":"https://github.com/acme/widget.git"}}}"#;
    write_rollout(&f, &format!("{meta}\n{CALL}\n{OUT_FAIL}\n"));

    let outcome = codex::sweep();
    assert_eq!(outcome.signals_emitted, 1);
    let (signals, _) = papercut::signal::read_signals("codex");
    assert_eq!(signals[0].repo.as_deref(), Some("github.com/acme/widget"));
}

#[test]
fn polled_process_failure_keeps_original_command() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-polled.jsonl");
    let running = r#"{"timestamp":"2026-09-19T10:00:02Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Process running with session ID 431\nOutput:\n"}}"#;
    let poll = r#"{"timestamp":"2026-09-19T10:00:03Z","type":"response_item","payload":{"type":"function_call","name":"write_stdin","arguments":"{\"session_id\":431,\"chars\":\"\"}","call_id":"c2"}}"#;
    let failed = r#"{"timestamp":"2026-09-19T10:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":"Process exited with code 9\nOutput:\nlate failure"}}"#;
    write_rollout(
        &f,
        &format!("{META}\n{CALL}\n{running}\n{poll}\n{failed}\n"),
    );

    let outcome = codex::sweep();
    assert_eq!(outcome.signals_emitted, 1);
    let (signals, _) = papercut::signal::read_signals("codex");
    assert_eq!(signals[0].cmd, "make build");
    assert_eq!(signals[0].exit, 9);
}

#[test]
fn current_backfill_is_idempotent_and_skips_legacy_failures() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/09/19/rollout-backfill.jsonl");
    let call = r#"{"timestamp":"2026-09-19T10:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"cx1","input":"text(await tools.exec_command({cmd:\"cargo test\"}));"}}"#;
    let output = r#"{"timestamp":"2026-09-19T10:00:01Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx1","output":[{"type":"input_text","text":"Script completed\nOutput:\n{\"exit_code\":7,\"output\":\"synthetic failure\"}"}]}}"#;
    write_rollout(
        &f,
        &format!("{META}\n{CALL}\n{OUT_FAIL}\n{call}\n{output}\n"),
    );

    // Model an older adapter. It advanced the normal watermark and recorded
    // the legacy failure, but it did not understand the current record shape.
    let legacy = papercut::signal::Signal::new(
        "2026-09-19T09:00:00Z",
        None,
        None,
        Some("codex".into()),
        "make build",
        2,
        Some("old failure"),
        Some("sess-1".into()),
    );
    papercut::signal::append_signal("codex", "sess-1", &legacy).unwrap();
    let mut sweeps = papercut::store::Sweeps::default();
    sweeps.marks.insert(
        "codex".into(),
        papercut::store::SweepMark {
            files: [(
                rel_key(&env.home, &f),
                papercut::store::FileMark {
                    offset: f.metadata().unwrap().len(),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        },
    );
    papercut::store::write_sweeps(&sweeps).unwrap();

    let first = codex::backfill_current();
    assert_eq!(first.signals_emitted, 1);
    let second = codex::backfill_current();
    assert_eq!(second.signals_emitted, 0);
    assert_eq!(
        papercut::signal::read_signals("codex").0.len(),
        2,
        "the legacy failure is not duplicated and the backfill is one-time"
    );

    let next_call = r#"{"timestamp":"2026-09-19T10:01:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"cx2","input":"text(await tools.exec_command({cmd:\"cargo check\"}));"}}"#;
    let next_output = r#"{"timestamp":"2026-09-19T10:01:01Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx2","output":[{"type":"input_text","text":"Output:\n{\"exit_code\":8,\"output\":\"new failure\"}"}]}}"#;
    append_rollout(&f, &format!("{next_call}\n{next_output}\n"));
    let normal = codex::sweep();
    assert_eq!(normal.signals_emitted, 1);
    let signals = papercut::signal::read_signals("codex").0;
    assert_eq!(
        signals.len(),
        3,
        "normal sweep does not repeat the backfill"
    );
    assert_eq!(
        signals
            .iter()
            .filter(|signal| signal.cmd == "cargo test")
            .count(),
        1
    );
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
    append_rollout(&f, &format!("{CALL2}\n{OUT2_FAIL}\n"));

    let o3 = codex::sweep();
    assert_eq!(o3.signals_emitted, 1, "appended content is picked up");
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 2);
}

/// A `function_call` and its `function_call_output` that arrive in SEPARATE
/// sweeps — the call seen first, the output appended later — must still be
/// matched and emit a signal. The call is persisted in `sweeps.json`'s
/// `pending_calls` as long as `last_path` has not advanced to a newer swept
/// file, so an output appended on the next run still finds its call. This
/// guards the F1 cross-run fix; without it a split pair is silently lost.
#[test]
fn call_output_split_across_runs_is_not_lost() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-split.jsonl");

    // First sweep: only META + CALL are present; the output has not arrived yet.
    write_rollout(&f, &format!("{META}\n{CALL}\n"));
    let o1 = codex::sweep();
    assert_eq!(o1.signals_emitted, 0, "no output yet → no signal");
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert!(sigs.is_empty());

    // The output is appended later; last_path still names this file, so the
    // pending call c1 survives in the watermark and is matched on resume.
    let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
    fh.write_all(format!("{OUT_FAIL}\n").as_bytes()).unwrap();
    drop(fh);

    let o2 = codex::sweep();
    assert_eq!(
        o2.signals_emitted, 1,
        "split pair matched across runs, not lost"
    );
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].cmd, "make build");
    assert_eq!(sigs[0].exit, 2);
}

/// After a split pair is matched, a further sweep with no new content must not
/// re-emit the signal: the watermark advanced past the output line, so it is
/// never re-read. Guards the F3 idempotence property (no duplication).
#[test]
fn resweep_after_split_does_not_duplicate() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-split2.jsonl");

    // Sweep 1: call only (output not yet present).
    write_rollout(&f, &format!("{META}\n{CALL}\n"));
    codex::sweep();

    // Append the output; sweep 2 matches the split pair and emits once.
    let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
    fh.write_all(format!("{OUT_FAIL}\n").as_bytes()).unwrap();
    drop(fh);
    let o2 = codex::sweep();
    assert_eq!(o2.signals_emitted, 1);

    // Sweep 3: nothing new — must not duplicate.
    let o3 = codex::sweep();
    assert_eq!(o3.signals_emitted, 0, "no duplicate on re-sweep");
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 1, "still exactly one signal");
}

/// Two sessions run in parallel: the older-sorting file gets more content
/// AFTER a sweep has already processed a newer-sorting file. The appended
/// failures must still be swept — per-file watermarks, not a single
/// newest-file mark.
#[test]
fn parallel_sessions_interleaved_appends_are_both_swept() {
    let env = IsolatedEnv::new().with_codex();
    let a = env.home.join(".codex/sessions/2026/08/04/rollout-a.jsonl");
    let b = env.home.join(".codex/sessions/2026/08/04/rollout-b.jsonl");
    write_rollout(&a, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));
    write_rollout(&b, &format!("{META_B}\n{CALL_B}\n{OUT_B_FAIL}\n"));

    let o1 = codex::sweep();
    assert_eq!(o1.signals_emitted, 2, "both sessions swept");

    // The older-sorting session is still live and fails again.
    append_rollout(&a, &format!("{CALL2}\n{OUT2_FAIL}\n"));
    let o2 = codex::sweep();
    assert_eq!(
        o2.signals_emitted, 1,
        "appends to an older-sorting live session must be swept"
    );

    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 3);
    // The appended failure keeps its own file's session context.
    let late = sigs.iter().find(|s| s.cmd == "cargo build").unwrap();
    assert_eq!(late.session.as_deref(), Some("sess-1"));
    assert_eq!(late.cwd.as_deref(), Some("/tmp/proj"));

    let o3 = codex::sweep();
    assert_eq!(o3.signals_emitted, 0, "no duplicates on re-sweep");
}

/// A pending `function_call` in an older file must survive sweeps that touch
/// newer files, and match its output when that output finally arrives.
#[test]
fn pending_call_in_older_file_survives_newer_file_sweep() {
    let env = IsolatedEnv::new().with_codex();
    let a = env.home.join(".codex/sessions/2026/08/04/rollout-a.jsonl");
    let b = env.home.join(".codex/sessions/2026/08/04/rollout-b.jsonl");
    // File a: call without output yet. File b: a complete failing pair.
    write_rollout(&a, &format!("{META}\n{CALL}\n"));
    write_rollout(&b, &format!("{META_B}\n{CALL_B}\n{OUT_B_FAIL}\n"));

    let o1 = codex::sweep();
    assert_eq!(o1.signals_emitted, 1, "only file b has a complete pair");

    // The output for file a's call arrives after file b was swept.
    append_rollout(&a, &format!("{OUT_FAIL}\n"));
    let o2 = codex::sweep();
    assert_eq!(
        o2.signals_emitted, 1,
        "pending call in an older file matches its late output"
    );
    let (sigs, _) = papercut::signal::read_signals("codex");
    let late = sigs.iter().find(|s| s.cmd == "make build").unwrap();
    assert_eq!(late.exit, 2);
    assert_eq!(late.session.as_deref(), Some("sess-1"));
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

/// A pre-per-file `sweeps.json` (single last_path/last_offset watermark) must
/// migrate without re-emitting already-swept content, and appends to files the
/// old watermark had passed must be picked up afterwards.
#[test]
fn legacy_watermark_migrates_without_resweep_or_loss() {
    let env = IsolatedEnv::new().with_codex();
    let a = env.home.join(".codex/sessions/2026/08/04/rollout-a.jsonl");
    let b = env.home.join(".codex/sessions/2026/08/04/rollout-b.jsonl");
    write_rollout(&a, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));
    write_rollout(&b, &format!("{META_B}\n{CALL_B}\n{OUT_B_FAIL}\n"));

    // Hand-write the legacy watermark shape: both files fully processed, the
    // newer file named as last_path. Includes the dead last_ts field.
    let b_len = std::fs::metadata(&b).unwrap().len();
    let store_root = env.data.join("papercuts");
    std::fs::create_dir_all(&store_root).unwrap();
    std::fs::write(
        store_root.join("sweeps.json"),
        format!(
            r#"{{"schema_version":1,"marks":{{"codex":{{"last_ts":"","last_path":{},"last_offset":{}}}}}}}"#,
            serde_json::to_string(&b.to_string_lossy()).unwrap(),
            b_len
        ),
    )
    .unwrap();

    let o1 = codex::sweep();
    assert_eq!(o1.signals_emitted, 0, "migration must not re-sweep");

    // The old design lost appends to already-passed files; the migrated
    // per-file map must pick them up.
    append_rollout(&a, &format!("{CALL2}\n{OUT2_FAIL}\n"));
    let o2 = codex::sweep();
    assert_eq!(o2.signals_emitted, 1, "append to a passed file is swept");

    // The legacy fields are gone from the rewritten sweeps.json.
    let sweeps = papercut::store::read_sweeps();
    let mark = &sweeps.marks["codex"];
    assert!(mark.last_path.is_none(), "legacy fields cleared");
    assert!(!mark.files.is_empty(), "per-file marks written");
}

/// A session file that shrank (truncated/replaced) is re-read from the start
/// with a warning, instead of silently never being read again.
#[test]
fn shrunk_file_warns_and_resweeps_from_zero() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-shrink.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));
    let old_len = std::fs::metadata(&f).unwrap().len();
    assert_eq!(codex::sweep().signals_emitted, 1);

    // Replace the file with shorter content holding a fresh failing pair.
    let replacement = format!("{META}\n{CALL2}\n{OUT2_FAIL}\n");
    assert!(
        (replacement.len() as u64) < old_len,
        "fixture must actually shrink the file"
    );
    write_rollout(&f, &replacement);

    let o = codex::sweep();
    assert_eq!(o.signals_emitted, 1, "replaced content is swept");
    assert!(
        o.warning.as_deref().is_some_and(|w| w.contains("shrank")),
        "shrink must surface a warning, got {:?}",
        o.warning
    );

    let o2 = codex::sweep();
    assert_eq!(o2.signals_emitted, 0, "no duplicates after the re-read");
}

/// Marks for deleted session files are pruned from sweeps.json.
#[test]
fn deleted_file_mark_is_pruned() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-gone.jsonl");
    let keep = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-keep.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));
    write_rollout(&keep, &format!("{META_B}\n{CALL_B}\n{OUT_B_FAIL}\n"));
    assert_eq!(codex::sweep().signals_emitted, 2);

    std::fs::remove_file(&f).unwrap();
    codex::sweep();

    let sweeps = papercut::store::read_sweeps();
    let files = &sweeps.marks["codex"].files;
    assert!(
        !files.contains_key(&rel_key(&env.home, &f)),
        "deleted file's mark is pruned"
    );
    assert!(
        files.contains_key(&rel_key(&env.home, &keep)),
        "surviving file's mark is kept"
    );
}

/// `papercut sweep` exits 1 with a structured error when the watermark cannot
/// be persisted — signals recorded, honesty preserved.
#[test]
fn sweep_exits_1_when_watermark_unpersistable() {
    let env = IsolatedEnv::new().with_codex();
    let f = env.home.join(".codex/sessions/2026/08/04/rollout-wm.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));

    // sweeps.json as a DIRECTORY: signal appends succeed, the atomic rename
    // of the watermark fails.
    let store_root = env.data.join("papercuts");
    std::fs::create_dir_all(store_root.join("sweeps.json")).unwrap();

    let out = std::process::Command::new(common::bin())
        .args(["--output", "json", "sweep"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "watermark failure is exit 1");
    let env_json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env_json["status"], "error");
    assert_eq!(env_json["errors"][0]["code"], "sweep_state_write_failed");
    assert_eq!(env_json["errors"][0]["retryable"], true);
    assert_eq!(
        env_json["data"]["signals_total"], 1,
        "the signal itself was recorded and reported"
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

/// A rollout whose final record is COMPLETE but has no terminating newline is
/// still captured — the writer finished the line without flushing `\n`. Without
/// the synthetic-terminator handling this record would be deferred forever once
/// the session goes quiet, because the file never grows past it.
#[test]
fn complete_record_without_trailing_newline_is_captured() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-nonl.jsonl");
    // META + CALL are newline-terminated; OUT_FAIL has NO trailing newline.
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}"));

    let o = codex::sweep();
    assert_eq!(
        o.signals_emitted, 1,
        "the complete-but-unterminated final record is captured"
    );
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].cmd, "make build");

    // The watermark must not overshoot the real end: a re-sweep with no new
    // content is a no-op, not a false shrink that would duplicate the signal.
    let o2 = codex::sweep();
    assert_eq!(o2.signals_emitted, 0, "no duplicate, no false shrink");
    let (sigs2, _) = papercut::signal::read_signals("codex");
    assert_eq!(sigs2.len(), 1);
}

/// A rollout whose final line is INCOMPLETE (the writer is mid-append) is left
/// for the next sweep — never parsed mid-write. The synthetic-terminator logic
/// must capture only a *complete* unterminated record; a partial line defers.
/// The pending `function_call` survives in the watermark across the defer.
#[test]
fn partial_trailing_line_is_deferred_and_keeps_pending_call() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-torn.jsonl");
    // Newline-terminated META + CALL, then a TORN final line (incomplete JSON,
    // no terminator): the shape of a writer mid-append.
    write_rollout(&f, &format!("{META}\n{CALL}\n{{\"partial"));

    let o = codex::sweep();
    assert_eq!(
        o.signals_emitted, 0,
        "a partial trailing line is deferred, never parsed"
    );
    let (sigs, _) = papercut::signal::read_signals("codex");
    assert!(sigs.is_empty());

    // The watermark stopped at the CALL line, so its pending call survives for
    // the next sweep (it is not swallowed by advancing past the torn bytes).
    let sweeps = papercut::store::read_sweeps();
    let mark = &sweeps.marks["codex"];
    let fm = &mark.files[&rel_key(&env.home, &f)];
    assert!(
        fm.pending_calls.contains_key("c1"),
        "pending call c1 survives the deferred torn line"
    );
}

/// #11: watermark keys are root-relative, and a legacy absolute-path key (the
/// old format, or one written under a since-relocated `$HOME`) is migrated on
/// load so resuming does not re-emit signals already recorded. With absolute
/// keys, a copied/synced `sweeps.json` would miss the new path and cold-sweep,
/// duplicating every prior signal.
#[test]
fn legacy_absolute_key_is_relativized_without_duplicate() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-relocate.jsonl");
    let body = format!("{META}\n{CALL}\n{OUT_FAIL}\n");
    write_rollout(&f, &body);

    // Pre-seed sweeps.json as an OLD version would: an ABSOLUTE-path key for
    // this file, marked fully swept (offset == file length).
    let len = std::fs::metadata(&f).unwrap().len();
    let mut sweeps = papercut::store::Sweeps::default();
    let mut mark = papercut::store::SweepMark::default();
    mark.files.insert(
        f.to_string_lossy().into_owned(),
        papercut::store::FileMark {
            offset: len,
            ..Default::default()
        },
    );
    sweeps.marks.insert("codex".into(), mark);
    papercut::store::write_sweeps(&sweeps).unwrap();

    // Sweep: the absolute key migrates to relative, matches the file, and its
    // offset already covers the whole file — so nothing is re-emitted.
    let out = codex::sweep();
    assert_eq!(
        out.signals_emitted, 0,
        "migrated watermark skips a redundant re-sweep: {out:?}"
    );

    // The persisted key is now root-relative, and the absolute key is gone.
    let after = papercut::store::read_sweeps();
    let files = &after.marks["codex"].files;
    assert!(
        files.contains_key(&rel_key(&env.home, &f)),
        "key relativized on load: {files:?}"
    );
    assert!(
        !files.contains_key(&*f.to_string_lossy()),
        "legacy absolute key dropped: {files:?}"
    );

    // And a subsequent sweep with no new content is still a no-op.
    let again = codex::sweep();
    assert_eq!(again.signals_emitted, 0, "idempotent after migration");
}

/// #11: an absolute-path key whose `$HOME` is gone (a real relocation to a new
/// machine) cannot be relativized, so it is dropped — those files re-sweep from
/// the start. The sweep must still emit the signal for the now-unmarked file.
#[test]
fn orphaned_absolute_key_under_gone_home_is_dropped_and_reswept() {
    let env = IsolatedEnv::new().with_codex();
    let f = env
        .home
        .join(".codex/sessions/2026/08/04/rollout-orphan.jsonl");
    write_rollout(&f, &format!("{META}\n{CALL}\n{OUT_FAIL}\n"));

    // An absolute key under a DIFFERENT root that no longer exists: the file at
    // `f` has no mark, and the stale mark points at a path nothing matches.
    let mut sweeps = papercut::store::Sweeps::default();
    let mut mark = papercut::store::SweepMark::default();
    mark.files.insert(
        "/gone/home/.codex/sessions/2026/08/04/rollout-orphan.jsonl".into(),
        papercut::store::FileMark {
            offset: 9999,
            ..Default::default()
        },
    );
    sweeps.marks.insert("codex".into(), mark);
    papercut::store::write_sweeps(&sweeps).unwrap();

    let out = codex::sweep();
    assert_eq!(
        out.signals_emitted, 1,
        "unmarked real file is swept: {out:?}"
    );
    let after = papercut::store::read_sweeps();
    let files = &after.marks["codex"].files;
    assert!(
        !files.contains_key("/gone/home/.codex/sessions/2026/08/04/rollout-orphan.jsonl"),
        "orphan absolute key dropped: {files:?}"
    );
    assert!(
        files.contains_key(&rel_key(&env.home, &f)),
        "real file recorded under a relative key: {files:?}"
    );
}
