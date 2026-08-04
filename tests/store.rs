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
