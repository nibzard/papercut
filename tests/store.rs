//! Store-layer integration: concurrency, atomicity, lenient reads.
//!
//! Each test points the store at an isolated fake HOME/XDG via `common`.

mod common;

use common::IsolatedEnv;
use std::process::Command;

/// Many processes writing at once must all land, with unique ids and no losses.
/// Atomic temp+rename is what makes this safe; a partial write would show as a
/// dropped or malformed event.
#[test]
fn parallel_adds_all_land_with_unique_ids() {
    let env = IsolatedEnv::new();
    let n = 12usize;
    let mut handles = Vec::new();
    for i in 0..n {
        let bin = common::bin();
        let home = env.home.clone();
        let data = env.data.clone();
        handles.push(std::thread::spawn(move || {
            let out = Command::new(&bin)
                .args(["add", &format!("concurrent report {i}")])
                .env("HOME", &home)
                .env("XDG_DATA_HOME", &data)
                .output()
                .expect("run papercut");
            assert!(
                out.status.success(),
                "add {i} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let (events, skipped) = papercut::store::read_all_events();
    assert_eq!(events.len(), n, "every concurrent add must be recorded");
    assert_eq!(
        skipped.len(),
        0,
        "no file should be malformed under concurrency"
    );

    let mut ids: Vec<_> = events.iter().map(|e| e.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "ids must be unique");
}

/// A corrupt event file is skipped and reported — never fatal to a read.
#[test]
fn malformed_file_is_skipped_not_fatal() {
    let _env = IsolatedEnv::new();
    let e = common::test_event("pc_01K00000000000000000000AA", "good event");
    papercut::store::write_event(&e).unwrap();

    // Drop a garbage file straight into the events dir.
    papercut::store::ensure_store().unwrap();
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01KBAD0000000000000000ZZ.json"),
        "{ not valid json",
    )
    .unwrap();

    let (events, skipped) = papercut::store::read_all_events();
    assert_eq!(events.len(), 1, "valid event still reads");
    assert_eq!(skipped.len(), 1, "garbage file is skipped, not fatal");
    assert!(skipped[0].file.starts_with("pc_01KBAD"));
}

/// A leftover `.json.tmp` (e.g. a process killed mid-rename) is invisible to reads.
#[test]
fn orphaned_tmp_files_are_ignored() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01KTMP0000000000000000AA.json.tmp"),
        "{}",
    )
    .unwrap();
    let (events, skipped) = papercut::store::read_all_events();
    assert!(events.is_empty());
    assert!(skipped.is_empty());
}

/// A `fixed` event whose `ref` is an empty string is quarantined on load. serde
/// deserializes `"ref":""` as `Some("")` (not `None`), so an `is_none()`-only
/// check would let it through. The reason must name the file and the rule.
#[test]
fn fixed_with_empty_ref_is_skipped_on_load() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let json = r#"{
        "schema_version": 1,
        "id": "pc_01K000000000000000000000B",
        "created_at": "2026-08-04T20:42:00Z",
        "source": "in_moment",
        "status": "fixed",
        "summary": "x",
        "context": {},
        "resolution": { "reason": "done", "ref": "" }
    }"#;
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01K000000000000000000000B.json"),
        json,
    )
    .unwrap();

    let (events, skipped) = papercut::store::read_all_events();
    assert!(events.is_empty(), "empty-ref fixed event is quarantined");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].reason.contains("fixed"));
    assert!(skipped[0].reason.contains("ref"));
}

/// Reopening a fixed papercut (editing `status` back to `open` but leaving the
/// resolution) is the documented one-field-edit workflow. It must NOT be
/// quarantined — otherwise a previously-valid event silently disappears from
/// `list` and the triage pack.
#[test]
fn reopened_event_keeping_resolution_loads() {
    let _env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let json = r#"{
        "schema_version": 1,
        "id": "pc_01K000000000000000000000C",
        "created_at": "2026-08-04T20:42:00Z",
        "source": "in_moment",
        "status": "open",
        "summary": "recurred",
        "context": {},
        "resolution": { "reason": "previously fixed", "ref": "abc1234" }
    }"#;
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01K000000000000000000000C.json"),
        json,
    )
    .unwrap();

    let (events, skipped) = papercut::store::read_all_events();
    assert!(skipped.is_empty(), "reopened event must not be quarantined");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, papercut::model::Status::Open);
}

#[test]
fn invalid_events_directory_is_reported() {
    let _env = IsolatedEnv::new();
    let root = papercut::paths::data_root().unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("events"), "not a directory").unwrap();

    let (events, skipped) = papercut::store::read_all_events();
    assert!(events.is_empty());
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].reason.contains("events directory"));
}

#[cfg(unix)]
#[test]
fn store_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let _env = IsolatedEnv::new();
    let event = common::test_event("pc_01K000000000000000000000D", "private");
    let path = papercut::store::write_event(&event).unwrap();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
}

#[cfg(unix)]
#[test]
fn signal_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let _env = IsolatedEnv::new();
    let signal = papercut::signal::Signal::new(
        "2026-09-19T00:00:00Z",
        None,
        None,
        Some("codex".into()),
        "false",
        1,
        Some("failed"),
        Some("s1".into()),
    );
    papercut::signal::append_signal("codex", "s1", &signal).unwrap();
    let path = papercut::signal::session_file("codex", "s1").unwrap();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
}
