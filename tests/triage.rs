//! triage-pack: structure, default filtering, determinism, and token-budget
//! truncation of the signal-clusters section.

mod common;

use common::IsolatedEnv;
use papercut::app::RunResult;
use papercut::cli::TriagePackArgs;
use papercut::commands::triage_pack;
use papercut::model::Status;

fn pack(repo: &str, status: Option<Status>, max_tokens: u32) -> String {
    match triage_pack::run(TriagePackArgs {
        repo: repo.into(),
        status,
        max_tokens,
    }) {
        RunResult::Ok { text, .. } => text,
        RunResult::Err(e) => panic!("triage-pack failed: {e:?}"),
    }
}

/// Seed: one open event, one fixed event (filtered out by default), one codex
/// signal that becomes a cluster.
fn seed() {
    let mut e1 = common::test_event("pc_01K0000000000000000000001", "build is flaky on mac");
    e1.status = Status::Open;
    let mut e2 = common::test_event("pc_01K0000000000000000000002", "already done");
    e2.status = Status::Fixed;
    papercut::store::write_event(&e1).unwrap();
    papercut::store::write_event(&e2).unwrap();

    let sig = papercut::signal::Signal::new(
        "2026-08-04T20:42:00Z",
        None,
        None,
        Some("codex".into()),
        "make build",
        2,
        Some("Error: no rule"),
        Some("sess-1".into()),
    );
    papercut::signal::append_signal("codex", "sess-1", &sig).unwrap();
}

#[test]
fn pack_structure_and_default_filter() {
    let _env = IsolatedEnv::new();
    seed();
    let md = pack("all", None, 12_000);

    assert!(md.contains("# Papercut triage pack"));
    assert!(md.contains("## Events"));
    assert!(
        md.contains("pc_01K0000000000000000000001"),
        "open event included"
    );
    assert!(
        !md.contains("already done"),
        "fixed event excluded by default"
    );
    assert!(md.contains("## Signal clusters"));
    assert!(md.contains("make build"), "cluster sample present");
    assert!(
        !md.contains("omitted"),
        "nothing dropped under a large budget"
    );
}

#[test]
fn pack_is_deterministic() {
    let _env = IsolatedEnv::new();
    seed();
    let a = pack("all", None, 12_000);
    let b = pack("all", None, 12_000);
    assert_eq!(a, b);
}

/// A tiny token budget must drop clusters but keep events — never silently
/// truncate the bundle mid-line.
#[test]
fn budget_truncates_clusters() {
    let _env = IsolatedEnv::new();
    seed();
    let md = pack("all", None, 1);
    assert!(md.contains("## Events"), "events are never budget-gated");
    assert!(
        md.contains("omitted"),
        "tiny budget should omit clusters: {md}"
    );
    assert!(
        !md.contains("make build"),
        "cluster sample dropped under budget"
    );
}

#[test]
fn explicit_status_overrides_default_filter() {
    let _env = IsolatedEnv::new();
    seed();
    let md = pack("all", Some(Status::Fixed), 12_000);
    assert!(
        md.contains("already done"),
        "fixed event included under --status fixed"
    );
    assert!(
        !md.contains("build is flaky"),
        "non-matching status excluded"
    );
}
