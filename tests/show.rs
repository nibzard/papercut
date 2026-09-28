//! One-record inspection through the public CLI, with an isolated store.

mod common;

use common::IsolatedEnv;
use papercut::model::{Resolution, Status};
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(common::bin())
        .args(args)
        .output()
        .expect("run papercut");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn show_displays_every_stored_field_without_changing_event() {
    let _env = IsolatedEnv::new();
    let id = "pc_01K000000000000000000000S";
    let mut event = common::test_event(id, &"An unusually long observation ".repeat(7));
    event.status = Status::Fixed;
    event.hypothesis = Some("The cache may be stale".into());
    event.suggested_fix = Some("Rebuild the cache".into());
    event.category = Some("tooling".into());
    event.context.repo = Some("github.com/example/repo".into());
    event.context.cwd = Some("packages/cli".into());
    event.context.git_sha = Some("abc1234".into());
    event.context.agent = Some("codex".into());
    event.context.session = Some("session-1".into());
    event.context.task = Some("PC-42".into());
    event.resolution = Some(Resolution {
        reason: "Pinned the tool".into(),
        ref_: Some("def5678".into()),
    });
    let path = papercut::store::write_event(&event).unwrap();
    let before = std::fs::read(&path).unwrap();

    let (code, text, stderr) = run(&["show", id]);
    assert_eq!(code, 0, "{stderr}");
    for expected in [
        id,
        "fixed",
        "2026-08-04T20:42:00Z",
        "in_moment",
        "An unusually long observation",
        "The cache may be stale",
        "Rebuild the cache",
        "tooling",
        "github.com/example/repo",
        "packages/cli",
        "abc1234",
        "codex",
        "session-1",
        "PC-42",
        "Pinned the tool",
        "def5678",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text}");
    }
    assert_eq!(std::fs::read(path).unwrap(), before);

    let (code, json, stderr) = run(&["show", id, "--output", "json"]);
    assert_eq!(code, 0, "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(envelope["status"], "ok");
    assert_eq!(
        envelope["data"]["event"],
        serde_json::to_value(&event).unwrap()
    );
    assert_eq!(envelope["errors"], serde_json::json!([]));
}

#[test]
fn unknown_id_is_an_honest_read_failure() {
    let _env = IsolatedEnv::new();
    let id = "pc_01K000000000000000000000Z";
    let (code, out, stderr) = run(&["show", id, "--output", "json"]);
    assert_eq!(code, 1, "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["errors"][0]["code"], "event_not_found");
}

#[test]
fn unreadable_event_is_reported_separately_from_missing_event() {
    let env = IsolatedEnv::new();
    let id = "pc_01K000000000000000000000R";
    let dir = env.data.join("papercuts/events");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.json")), "{ broken").unwrap();

    let (code, out, stderr) = run(&["show", id, "--output", "json"]);
    assert_eq!(code, 1, "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["errors"][0]["code"], "event_unreadable");
}

#[test]
fn short_reference_is_case_insensitive_and_ambiguity_is_reported() {
    let _env = IsolatedEnv::new();
    let first = format!("pc_{}0ABCDEFGH", "0".repeat(17));
    let second = format!("pc_{}1ABCDEFGH", "0".repeat(17));
    papercut::store::write_event(&common::test_event(&first, "first")).unwrap();
    papercut::store::write_event(&common::test_event(&second, "second")).unwrap();

    let (code, out, stderr) = run(&["show", "0abcdefgh"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("first"));

    let (code, out, stderr) = run(&["show", "abcdefgh", "--output", "json"]);
    assert_eq!(code, 2, "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(envelope["errors"][0]["code"], "ambiguous_ref");

    let (code, out, _) = run(&["list", "--repo", "all"]);
    assert_eq!(code, 0);
    assert!(out.contains("0ABCDEFGH"));
    assert!(out.contains("1ABCDEFGH"));
}
