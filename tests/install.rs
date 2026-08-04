//! install / uninstall / doctor: managed-block idempotence, user-content
//! preservation, settings.json wiring, and doctor health detection.

mod common;

use common::IsolatedEnv;
use papercut::app::RunResult;
use papercut::cli::{InstallArgs, UninstallArgs};
use serde_json::Value;

fn install(yes: bool, harness: Option<&str>) {
    let r = papercut::commands::install::run(InstallArgs {
        yes,
        harness: harness.map(str::to_string),
    });
    assert!(matches!(r, RunResult::Ok { .. }), "install failed: {r:?}");
}

fn uninstall(harness: Option<&str>) {
    let r = papercut::commands::uninstall::run(UninstallArgs {
        harness: harness.map(str::to_string),
    });
    assert!(matches!(r, RunResult::Ok { .. }), "uninstall failed: {r:?}");
}

fn doctor_data() -> Value {
    match papercut::commands::doctor::run() {
        RunResult::Ok { data, .. } => data,
        RunResult::Err(e) => panic!("doctor returned Err: {e:?}"),
    }
}

fn check_ok(data: &Value, name: &str) -> bool {
    data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["name"] == name && c["ok"].as_bool() == Some(true))
}

#[test]
fn block_upsert_is_idempotent_and_preserves_user_content() {
    let env = IsolatedEnv::new().with_claude();
    std::fs::write(
        env.home.join(".claude/CLAUDE.md"),
        "# My rules\nDo good work.\n",
    )
    .unwrap();

    install(true, None);
    let after = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(after.contains("# My rules"), "user heading preserved");
    assert!(after.contains("Do good work."), "user body preserved");
    assert!(after.contains("papercut:begin v1"));
    assert_eq!(after.matches("papercut:begin").count(), 1);
    let len1 = after.len();

    // Reinstall must not duplicate or grow the block.
    install(true, None);
    let after2 = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(len1, after2.len(), "second install is a no-op on size");
    assert_eq!(after2.matches("papercut:begin").count(), 1);

    // Uninstall removes only our block; user content survives untouched.
    uninstall(None);
    let after3 = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(!after3.contains("papercut"), "block fully removed");
    assert!(after3.contains("# My rules"));
    assert!(after3.contains("Do good work."));
}

#[test]
fn settings_wiring_preserves_user_keys_and_is_idempotent() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    std::fs::write(
        &settings,
        r#"{"model":"opus","hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user-hook"}]}]}}"#,
    )
    .unwrap();

    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus", "unrelated top-level key preserved");

    let cmds: Vec<&str> = v["hooks"]["PostToolUse"][0]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["command"].as_str().unwrap())
        .collect();
    assert!(
        cmds.iter().any(|c| c.contains("user-hook")),
        "user hook kept"
    );
    assert!(
        cmds.iter()
            .any(|c| c.contains("papercut") && c.contains("_hook")),
        "our hook added"
    );
    assert!(papercut::adapters::claude_code::hook_wired());

    // Idempotent: exactly one papercut entry across all groups.
    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    let v2: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let ours = v2["hooks"]["PostToolUse"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array())
        .flatten()
        .filter(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.contains("papercut") && c.contains("_hook"))
        })
        .count();
    assert_eq!(ours, 1, "no duplicate papercut entries");

    // Uninstall removes only ours; model + user hook remain.
    let removed = papercut::adapters::claude_code::uninstall_hook().unwrap();
    assert_eq!(removed, 1);
    let v3: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v3["model"], "opus");
    let remaining: Vec<&str> = v3["hooks"]["PostToolUse"][0]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["command"].as_str().unwrap())
        .collect();
    assert!(remaining.iter().any(|c| c.contains("user-hook")));
    assert!(remaining
        .iter()
        .all(|c| !(c.contains("papercut") && c.contains("_hook"))));
    assert!(!papercut::adapters::claude_code::hook_wired());
}

#[test]
fn doctor_unhealthy_before_install_healthy_after() {
    let _env = IsolatedEnv::new().with_claude();
    let before = doctor_data();
    assert_eq!(before["healthy"], false);
    assert!(!check_ok(&before, "block:claude-code"));
    assert!(!check_ok(&before, "adapter:claude-code"));

    install(true, None);
    let after = doctor_data();
    assert_eq!(after["healthy"], true, "should be healthy after install");
    assert!(check_ok(&after, "block:claude-code"));
    assert!(check_ok(&after, "adapter:claude-code"));
}

#[test]
fn doctor_flags_stale_block_version() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);

    // Tamper: downgrade the managed-block version marker.
    let p = env.home.join(".claude/CLAUDE.md");
    let c = std::fs::read_to_string(&p)
        .unwrap()
        .replace("papercut:begin v1", "papercut:begin v0");
    std::fs::write(&p, c).unwrap();

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    assert!(!check_ok(&data, "block:claude-code"));
    assert!(
        check_ok(&data, "adapter:claude-code"),
        "hook wiring is independent"
    );

    // Remediation hints are deterministic for a given failing check.
    let block_check = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "block:claude-code")
        .unwrap();
    assert_eq!(block_check["hint"], "run: papercut install");
}

#[test]
fn doctor_flags_missing_hook() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);
    let _ = std::fs::remove_file(env.home.join(".claude/settings.json"));

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    assert!(!check_ok(&data, "adapter:claude-code"));
    assert!(check_ok(&data, "block:claude-code"), "block is independent");
}

/// `doctor` must never crash on an unwritable store — it reports the failure.
#[test]
fn doctor_flags_unwritable_store() {
    let env = IsolatedEnv::new().with_claude();
    // Point XDG at a regular file so create_dir_all(events) cannot succeed.
    let file = env.data.join("iamfile");
    std::fs::write(&file, "x").unwrap();
    std::env::set_var("XDG_DATA_HOME", &file);

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    assert!(!check_ok(&data, "store"));
}
