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
        RunResult::Health { data, .. } => data,
        RunResult::Ok { data, .. } => data,
        RunResult::Err { errors, .. } => panic!("doctor returned Err: {errors:?}"),
        RunResult::Usage { errors } => panic!("doctor returned Usage: {errors:?}"),
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
    assert!(after.contains("papercut:begin v3"));
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

#[cfg(unix)]
#[test]
fn install_preserves_shared_instruction_symlink_and_mode() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let env = IsolatedEnv::new().with_codex();
    let shared = env.home.join("shared-instructions.md");
    std::fs::write(&shared, "# Shared rules\n").unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = env.home.join(".codex/AGENTS.md");
    symlink(&shared, &link).unwrap();

    install(true, Some("codex"));
    assert!(
        link.is_symlink(),
        "install must keep the harness path as a symlink"
    );
    assert!(std::fs::read_to_string(&shared)
        .unwrap()
        .contains("papercut:begin"));
    assert_eq!(
        shared.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );

    uninstall(Some("codex"));
    assert!(link.is_symlink(), "uninstall must keep the symlink too");
    assert!(!std::fs::read_to_string(&shared)
        .unwrap()
        .contains("papercut"));
    assert_eq!(
        shared.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
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

    // The user's PostToolUse hook is untouched; ours wires the failure event.
    assert_eq!(
        v.pointer("/hooks/PostToolUse/0/hooks/0/command")
            .and_then(|c| c.as_str()),
        Some("echo user-hook"),
        "user hook kept where it was"
    );
    let ours_cmd = v
        .pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
        .and_then(|c| c.as_str())
        .unwrap();
    assert!(
        ours_cmd.contains("papercut") && ours_cmd.contains("_hook"),
        "our hook added under PostToolUseFailure"
    );
    assert!(papercut::adapters::claude_code::hook_present());

    // Idempotent: exactly one papercut entry across all events and groups.
    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    let v2: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let ours = count_our_entries(&v2);
    assert_eq!(ours, 1, "no duplicate papercut entries");

    // Uninstall removes only ours; model + user hook remain.
    let removed = papercut::adapters::claude_code::uninstall_hook().unwrap();
    assert_eq!(removed, 1);
    let v3: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v3["model"], "opus");
    assert_eq!(
        v3.pointer("/hooks/PostToolUse/0/hooks/0/command")
            .and_then(|c| c.as_str()),
        Some("echo user-hook"),
        "user hook survives uninstall"
    );
    assert_eq!(count_our_entries(&v3), 0);
    assert!(!papercut::adapters::claude_code::hook_present());
}

/// Count papercut hook entries across every hook event and group.
fn count_our_entries(v: &Value) -> usize {
    v["hooks"]
        .as_object()
        .map(|events| {
            events
                .values()
                .filter_map(|a| a.as_array())
                .flatten()
                .flat_map(|g| g["hooks"].as_array())
                .flatten()
                .filter(|h| {
                    h["command"]
                        .as_str()
                        .is_some_and(|c| c.contains("papercut") && c.contains("_hook"))
                })
                .count()
        })
        .unwrap_or(0)
}

/// Concurrent installs race on read-modify-write and can double the block;
/// the block engine owns every complete region, so the next pass converges
/// to exactly one block with no leaked body text.
#[test]
fn concurrent_installs_converge_to_one_block() {
    let env = IsolatedEnv::new().with_claude();
    std::fs::write(env.home.join(".claude/CLAUDE.md"), "# user rules\n").unwrap();

    let children: Vec<_> = (0..4)
        .map(|_| {
            std::process::Command::new(common::bin())
                .args(["install", "--yes"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        c.wait().unwrap();
    }

    // Whatever interleaving happened, one more install converges the file.
    install(true, None);
    let after = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(after.matches("papercut:begin").count(), 1);
    assert_eq!(after.matches("### Log papercuts").count(), 1);
    assert!(after.contains("# user rules"));

    // And uninstall leaves no marker or body behind.
    uninstall(None);
    let clean = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(!clean.contains("papercut"), "no residue: {clean}");
    assert!(clean.contains("# user rules"));
}

/// A user hook whose command merely CONTAINS the substring `_hook
/// claude-code` (e.g. an unrelated tool's `_hook claude-code-audit`) is not
/// ours and must survive uninstall untouched.
#[test]
fn unrelated_hook_containing_substring_survives_uninstall() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks":{"PostToolUseFailure":[{"matcher":"Bash","hooks":[{"type":"command","command":"/usr/local/bin/other-tool _hook claude-code-audit"}]}]}}"#,
    )
    .unwrap();

    let removed = papercut::adapters::claude_code::uninstall_hook().unwrap();
    assert_eq!(removed, 0, "the user's hook is not ours");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        v.pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
            .and_then(|c| c.as_str()),
        Some("/usr/local/bin/other-tool _hook claude-code-audit"),
        "user hook untouched"
    );
}

/// An executable path containing spaces must produce a runnable (quoted)
/// hook command, and doctor's exe check must resolve it.
#[test]
#[cfg(unix)]
fn exe_path_with_spaces_is_quoted_and_resolvable() {
    use std::os::unix::fs::PermissionsExt;
    let env = IsolatedEnv::new().with_claude();
    let dir = env.home.join("bin with space");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("papercut");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

    papercut::adapters::claude_code::install_hook(&exe.to_string_lossy()).unwrap();

    let status = papercut::adapters::claude_code::hook_status();
    assert!(status.present);
    assert_eq!(
        status.exe.as_deref(),
        Some(&*exe.to_string_lossy()),
        "the exe token round-trips through quoting"
    );
    assert!(
        status.exe_ok,
        "a quoted exe path with spaces must resolve: {:?}",
        status.exe
    );

    // The command in settings.json is shell-safe: the exe is one token.
    let settings = env.home.join(".claude/settings.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let cmd = v
        .pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
        .and_then(|c| c.as_str())
        .unwrap();
    assert!(
        cmd.starts_with('\''),
        "a path with spaces must be quoted: {cmd}"
    );
}

/// A spaced exe path that ALSO contains a single quote round-trips through
/// hook_command's `'` → `'\''` encoding and first_token's decode, so doctor
/// resolves it. The runtime hook is valid shell; only doctor's read-back broke
/// before (it stopped at the first quote).
#[test]
#[cfg(unix)]
fn exe_path_with_space_and_apostrophe_round_trips() {
    use std::os::unix::fs::PermissionsExt;
    let env = IsolatedEnv::new().with_claude();
    let dir = env.home.join("Jane's bin");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("papercut");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

    papercut::adapters::claude_code::install_hook(&exe.to_string_lossy()).unwrap();
    let status = papercut::adapters::claude_code::hook_status();
    assert!(status.present, "entry present: {:?}", status);
    assert_eq!(
        status.exe.as_deref(),
        Some(&*exe.to_string_lossy()),
        "the full spaced+quoted exe is decoded as one token: {:?}",
        status.exe
    );
    assert!(
        status.exe_ok,
        "doctor resolves the wired hook: {:?}",
        status
    );
}

/// An apostrophe-bearing exe path with NO space (e.g.
/// `/home/O'Brien/.local/bin/papercut`) is the case quoting-on-whitespace
/// missed: the path was emitted unquoted, so the shell rejected the bare
/// apostrophe as an unterminated quote and the hook never fired — while
/// `doctor` decoded the path back and reported it healthy. Always quoting fixes
/// the runtime; this test pins it by EXECUTING the emitted command in a shell.
#[test]
#[cfg(unix)]
fn exe_path_with_apostrophe_no_space_runs_in_shell() {
    use std::os::unix::fs::PermissionsExt;
    let env = IsolatedEnv::new().with_claude();
    let dir = env.home.join("O'Brien");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("papercut");
    std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

    papercut::adapters::claude_code::install_hook(&exe.to_string_lossy()).unwrap();

    let settings = env.home.join(".claude/settings.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let cmd = v
        .pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
        .and_then(|c| c.as_str())
        .unwrap();
    assert!(
        cmd.starts_with('\''),
        "an apostrophe path must be quoted even without spaces: {cmd}"
    );

    // doctor's read-back still resolves (the regression was runtime-only).
    let status = papercut::adapters::claude_code::hook_status();
    assert!(status.present);
    assert_eq!(status.exe.as_deref(), Some(&*exe.to_string_lossy()));
    assert!(status.exe_ok, "doctor resolves the quoted apostrophe exe");

    // The real regression guard: the emitted command is VALID SHELL and runs.
    // Before the always-quote fix this exited non-zero (unterminated quote).
    let rc = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .unwrap()
        .code()
        .unwrap();
    assert_eq!(rc, 0, "the hook command must run in a shell: {cmd}");
}

/// A settings.json that is valid JSON but not an object (`[]`, `42`) makes
/// install refuse ("root is not an object"); doctor must diagnose it too
/// instead of hinting "run install" — which loops.
#[test]
fn doctor_diagnoses_non_object_settings() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);
    for bad in ["[]", "42", "\"oops\""] {
        std::fs::write(env.home.join(".claude/settings.json"), bad).unwrap();
        let data = doctor_data();
        let adapter = data["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "adapter:claude-code")
            .unwrap();
        assert_eq!(adapter["ok"], false, "{bad} must be unhealthy");
        assert!(
            adapter["detail"]
                .as_str()
                .unwrap()
                .contains("not an object"),
            "diagnosis names the wrong type for {bad}: {}",
            adapter["detail"]
        );
        assert!(
            !adapter["hint"]
                .as_str()
                .unwrap()
                .contains("papercut install"),
            "hint must not loop into install for {bad}: {}",
            adapter["hint"]
        );
    }
}

/// An UNREADABLE instructions file at a drifted/recorded path is flagged by
/// doctor — not silently swallowed as "no block" (consistent with the detected
/// path check).
#[test]
fn doctor_flags_unreadable_recorded_path() {
    let env = IsolatedEnv::new();
    std::fs::create_dir_all(env.home.join(".opencode")).unwrap();
    install(true, Some("opencode"));
    // Drift: detection moves to XDG; the recorded legacy path is now unreadable.
    std::fs::create_dir_all(env.home.join(".config/opencode")).unwrap();
    std::fs::write(env.home.join(".opencode/AGENTS.md"), b"\xff\xfe broken").unwrap();

    let data = doctor_data();
    let recorded = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"].as_str().unwrap_or("").contains("recorded"))
        .unwrap_or_else(|| panic!("a recorded-path check exists: {data}"));
    assert_eq!(recorded["ok"], false);
    assert!(
        recorded["detail"].as_str().unwrap().contains("cannot read"),
        "diagnosis names the read failure: {}",
        recorded["detail"]
    );
}

/// Our entry drifted under a non-Bash matcher never fires for Bash — doctor
/// must read it as missing, and a reinstall must move it back.
#[test]
fn matcher_drift_reads_as_missing_and_reinstall_heals() {
    let env = IsolatedEnv::new().with_claude();
    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    assert!(papercut::adapters::claude_code::hook_present());

    // Drift: another tool rewrites the group's matcher.
    let settings = env.home.join(".claude/settings.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    v["hooks"]["PostToolUseFailure"][0]["matcher"] = Value::String("Edit".into());
    std::fs::write(&settings, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    assert!(
        !papercut::adapters::claude_code::hook_present(),
        "an entry under a non-Bash matcher is a dead hook"
    );

    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    assert!(papercut::adapters::claude_code::hook_present());
    let v2: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(count_our_entries(&v2), 1, "moved back, not duplicated");
}

/// An install made by an older papercut wired PostToolUse, which never fires
/// on failures. Doctor must read it as missing, and a reinstall must move the
/// entry to PostToolUseFailure instead of leaving a dead duplicate behind.
#[test]
fn legacy_posttooluse_wiring_is_healed_on_reinstall() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x/papercut _hook claude-code"}]}]}}"#,
    )
    .unwrap();

    assert!(
        !papercut::adapters::claude_code::hook_present(),
        "a PostToolUse-only wiring is a dead hook, not present"
    );

    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(count_our_entries(&v), 1, "moved, not duplicated");
    assert!(
        v.pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
            .is_some(),
        "entry now lives under PostToolUseFailure"
    );
    assert!(
        v.pointer("/hooks/PostToolUse").is_none(),
        "the legacy event key we emptied is gone"
    );
    assert!(papercut::adapters::claude_code::hook_present());
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

#[cfg(unix)]
#[test]
fn doctor_flags_store_paths_with_broad_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let env = IsolatedEnv::new();
    papercut::store::ensure_store().unwrap();
    let root = env.data.join("papercuts");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();

    let data = doctor_data();
    let check = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "store_permissions")
        .unwrap();
    assert_eq!(check["ok"], false);
    assert!(check["hint"].as_str().unwrap().contains("chmod -R go-rwx"));
}

#[test]
fn doctor_flags_missing_codex_sweep_state() {
    let _env = IsolatedEnv::new().with_codex();
    let data = doctor_data();
    let check = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "adapter:codex:freshness")
        .unwrap();
    assert_eq!(check["ok"], false);
    assert!(check["hint"].as_str().unwrap().contains("papercut sweep"));
}

#[test]
fn doctor_flags_stale_block_version() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);

    // Tamper: downgrade the managed-block version marker.
    let p = env.home.join(".claude/CLAUDE.md");
    let c = std::fs::read_to_string(&p)
        .unwrap()
        .replace("papercut:begin v3", "papercut:begin v0");
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
    assert_eq!(block_check["hint"], "run: papercut install --yes");
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

/// F4: a hook entry that points at a missing executable is a dead hook — doctor
/// must not call it healthy just because the command substring is present.
#[test]
fn doctor_flags_dead_hook_pointing_at_missing_exe() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);
    // Tamper: keep our entry but retarget its exe token at a path that does not
    // exist. The command substring is still ours, so a substring-only check
    // would wrongly report healthy.
    let settings = env.home.join(".claude/settings.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    for g in v["hooks"]["PostToolUseFailure"].as_array_mut().unwrap() {
        for h in g["hooks"].as_array_mut().unwrap() {
            if h["command"]
                .as_str()
                .is_some_and(|c| c.contains("_hook claude-code"))
            {
                h["command"] = Value::String("/does/not/exist/papercut _hook claude-code".into());
            }
        }
    }
    std::fs::write(&settings, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let data = doctor_data();
    assert_eq!(data["healthy"], false, "dead-hook exe must be unhealthy");
    assert!(!check_ok(&data, "adapter:claude-code"));
    let adapter = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "adapter:claude-code")
        .unwrap();
    assert!(
        adapter["detail"]
            .as_str()
            .unwrap()
            .contains("missing or non-executable"),
        "detail must explain the dead exe: {}",
        adapter["detail"]
    );
}

/// Install recorded `instructions_file` in config.json; when the harness's
/// detected config path later drifts (OpenCode legacy → XDG), uninstall must
/// still clean the RECORDED file instead of reporting "absent" and orphaning
/// the block forever.
#[test]
fn uninstall_cleans_recorded_path_after_config_dir_drift() {
    let env = IsolatedEnv::new();
    std::fs::create_dir_all(env.home.join(".opencode")).unwrap();
    install(true, Some("opencode"));
    let legacy = env.home.join(".opencode/AGENTS.md");
    assert!(std::fs::read_to_string(&legacy)
        .unwrap()
        .contains("papercut:begin"));

    // The user later gains the XDG config dir; detection moves there.
    std::fs::create_dir_all(env.home.join(".config/opencode")).unwrap();

    uninstall(Some("opencode"));
    assert!(
        !std::fs::read_to_string(&legacy)
            .unwrap()
            .contains("papercut"),
        "block removed from the recorded legacy path"
    );
    let cfg = papercut::store::read_config();
    assert!(
        cfg.installed.iter().all(|i| i.harness != "opencode"),
        "config entry dropped once the recorded path is clean"
    );
}

/// Doctor must surface a managed block orphaned at a previously recorded
/// path the current detection no longer looks at.
#[test]
fn doctor_flags_orphaned_block_at_recorded_path() {
    let env = IsolatedEnv::new();
    std::fs::create_dir_all(env.home.join(".opencode")).unwrap();
    install(true, Some("opencode"));
    std::fs::create_dir_all(env.home.join(".config/opencode")).unwrap();

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    let orphan = data["checks"].as_array().unwrap().iter().find(|c| {
        c["name"]
            .as_str()
            .is_some_and(|n| n.contains("opencode") && n.contains("recorded"))
    });
    let orphan = orphan.expect("a recorded-path check exists");
    assert_eq!(orphan["ok"], false);
    assert!(
        orphan["hint"].as_str().unwrap().contains("uninstall"),
        "hint points at the command that can clean it: {}",
        orphan["hint"]
    );
}

/// An instructions file that EXISTS but cannot be read gets an honest
/// diagnosis — not "block missing, run install" (install refuses exactly
/// that file, so the old hint looped).
#[test]
fn doctor_diagnoses_unreadable_instructions_file() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);
    std::fs::write(env.home.join(".claude/CLAUDE.md"), b"\xff\xfe broken \xff").unwrap();

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    let block = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "block:claude-code")
        .unwrap();
    assert_eq!(block["ok"], false);
    assert!(
        block["detail"].as_str().unwrap().contains("cannot read"),
        "diagnosis names the read failure: {}",
        block["detail"]
    );
    assert!(
        !block["hint"].as_str().unwrap().contains("papercut install"),
        "hint must not loop into a command that refuses this file: {}",
        block["hint"]
    );
}

/// A corrupt settings.json reads as "hook missing" through the lenient path;
/// doctor must diagnose the corruption instead of hinting at install, which
/// refuses to touch an unparseable file.
#[test]
fn doctor_diagnoses_corrupt_settings_json() {
    let env = IsolatedEnv::new().with_claude();
    install(true, None);
    std::fs::write(env.home.join(".claude/settings.json"), "{ not json {{{").unwrap();

    let data = doctor_data();
    assert_eq!(data["healthy"], false);
    let adapter = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "adapter:claude-code")
        .unwrap();
    assert_eq!(adapter["ok"], false);
    assert!(
        adapter["detail"]
            .as_str()
            .unwrap()
            .contains("not valid JSON"),
        "diagnosis names the corruption: {}",
        adapter["detail"]
    );
    assert!(
        !adapter["hint"]
            .as_str()
            .unwrap()
            .contains("papercut install"),
        "hint must not loop into a command that refuses this file: {}",
        adapter["hint"]
    );
}

/// A user's pre-existing empty `"hooks": {}` key is the user's content, not
/// ours — uninstall must leave it in place.
#[test]
fn user_empty_hooks_key_survives_uninstall() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    std::fs::write(&settings, r#"{"hooks":{},"model":"opus"}"#).unwrap();

    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    papercut::adapters::claude_code::uninstall_hook().unwrap();

    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    assert!(
        v.get("hooks").is_some_and(|h| h.as_object().is_some()),
        "the user's hooks key survives uninstall: {v}"
    );
}

/// Orphaned `.tmp` files in events/ (interrupted atomic writes) are invisible
/// to reads but accumulate; doctor reports them with a cleanup hint. It never
/// deletes them.
#[test]
fn doctor_reports_orphaned_tmp_files() {
    let _env = IsolatedEnv::new().with_claude();
    papercut::store::ensure_store().unwrap();
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01KTMP0000000000000000A.json.tmp"),
        "leftover",
    )
    .unwrap();

    let data = doctor_data();
    let tmp_check = data["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "orphaned_tmp")
        .unwrap();
    // Informational: never fails the overall health on its own.
    assert_eq!(tmp_check["ok"], true);
    assert!(
        tmp_check["detail"]
            .as_str()
            .unwrap()
            .contains("1 orphaned .tmp"),
        "reports the leftover: {}",
        tmp_check["detail"]
    );
    assert!(
        tmp_check["hint"].as_str().unwrap().contains("rm "),
        "hint offers a cleanup command: {}",
        tmp_check["hint"]
    );
    // doctor did not delete it.
    assert!(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01KTMP0000000000000000A.json.tmp")
            .exists(),
        "doctor reports, never deletes"
    );
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

/// `--harness` restricts install (and uninstall) to the named harness; every
/// other detected harness is left untouched. (Documented flag, previously
/// untested — every other test passes `None`.)
#[test]
fn harness_filter_restricts_install_and_uninstall() {
    let env = IsolatedEnv::new().with_claude().with_codex();

    let r = papercut::commands::install::run(InstallArgs {
        yes: true,
        harness: Some("claude-code".into()),
    });
    let data = match r {
        RunResult::Ok { data, .. } => data,
        RunResult::Err { errors, .. } => panic!("install failed: {errors:?}"),
        RunResult::Health { .. } => panic!("install unexpectedly returned Health"),
        RunResult::Usage { errors } => panic!("install usage error: {errors:?}"),
    };
    let installed: Vec<&str> = data["installed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["harness"].as_str().unwrap())
        .collect();
    assert_eq!(
        installed,
        vec!["claude-code"],
        "only the filtered harness installs"
    );

    // claude-code got the block; codex's file was never created/touched.
    let claude_md = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude_md.contains("papercut:begin v3"));
    let codex_md = std::fs::read_to_string(env.home.join(".codex/AGENTS.md")).unwrap_or_default();
    assert!(
        !codex_md.contains("papercut"),
        "codex untouched by a claude-code-only install"
    );

    // The same filter scopes uninstall to claude-code only.
    uninstall(Some("claude-code"));
    let after = std::fs::read_to_string(env.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(
        !after.contains("papercut"),
        "filtered uninstall removed the block"
    );
}

/// An instructions file that EXISTS but is unreadable (invalid UTF-8) must not
/// be clobbered. Previously install read it as "" (unwrap_or_default), fed that
/// to upsert, and overwrote the real file with a bare managed block — destroying
/// the user's content and exiting 0.
#[test]
fn install_refuses_to_clobber_unreadable_instructions_file() {
    let env = IsolatedEnv::new().with_claude();
    let path = env.home.join(".claude/CLAUDE.md");
    // Invalid UTF-8 → read_to_string fails with InvalidData, not NotFound.
    std::fs::write(&path, b"\xff\xfe PRECIOUS NON-UTF8 \xff\xfe").unwrap();

    let r = papercut::commands::install::run(InstallArgs {
        yes: true,
        harness: None,
    });
    assert!(
        matches!(r, RunResult::Err { .. }),
        "unreadable file must surface an error, not exit 0"
    );

    // The original bytes survive untouched — install never overwrote it.
    let after = std::fs::read(&path).unwrap();
    assert!(
        after.contains(&0xffu8),
        "original invalid bytes preserved, not destroyed"
    );
    assert!(
        !String::from_utf8_lossy(&after).contains("papercut:begin"),
        "no managed block written into an unreadable file"
    );
}

/// `install_hook` must refuse to overwrite a settings.json it cannot parse,
/// rather than swallowing the parse error to `{}` and rewriting a hook-only
/// object over the user's file.
#[test]
fn install_hook_refuses_unparseable_settings() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    let original = "{ not valid json {{{";
    std::fs::write(&settings, original).unwrap();

    let r = papercut::adapters::claude_code::install_hook("/x/papercut");
    assert!(r.is_err(), "must refuse to overwrite unparseable settings");

    let after = std::fs::read_to_string(&settings).unwrap();
    assert_eq!(
        after, original,
        "unparseable settings left byte-identical, not overwritten"
    );
}

/// A 0-byte (or whitespace-only) settings.json is semantically "no settings",
/// not corruption — `touch ~/.claude/settings.json` must not block install.
/// Previously `from_slice(b"")` failed with "EOF while parsing a value", which
/// the strict write path treated as "refuse to overwrite", stalling the adapter
/// step. Blank ⇒ `{}` (same as absent).
#[test]
fn install_hook_accepts_blank_settings_file() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");

    for blank in ["", "  \n\t \n"] {
        std::fs::write(&settings, blank).unwrap();
        papercut::adapters::claude_code::install_hook("/x/papercut")
            .unwrap_or_else(|e| panic!("blank settings must not block install: {e}"));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(
            v["hooks"]["PostToolUseFailure"]
                .pointer("/0/hooks/0/command")
                .is_some(),
            "our hook wired over a blank settings file: {v}"
        );
        assert!(papercut::adapters::claude_code::hook_present());
    }
}

/// A UTF-8 BOM-prefixed settings.json (written by some Windows editors) must not
/// be misread as corrupt. Previously the BOM bytes (`EF BB BF`) failed the
/// ascii-whitespace blank check AND `serde_json::from_slice` rejected the BOM, so
/// a BOM-only file stalled install ("refuse to overwrite") and a BOM-prefixed
/// `{}` was treated as corrupt — diverging from the lenient `read_settings`
/// path, which silently read it as `{}`. Stripping the BOM first keeps the three
/// read paths in agreement.
#[test]
fn install_hook_accepts_bom_prefixed_settings() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    const BOM: &[u8] = b"\xef\xbb\xbf";

    // BOM-only ⇒ blank ⇒ installs the hook, not refused as corrupt.
    std::fs::write(&settings, BOM).unwrap();
    papercut::adapters::claude_code::install_hook("/x/papercut")
        .unwrap_or_else(|e| panic!("BOM-only settings must not block install: {e}"));
    let v: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert!(
        v.pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
            .is_some(),
        "our hook wired over a BOM-only file: {v}"
    );

    // BOM-prefixed valid JSON parses (BOM stripped); user keys survive.
    let mut with_bom = BOM.to_vec();
    with_bom.extend_from_slice(br#"{"model":"opus"}"#);
    std::fs::write(&settings, &with_bom).unwrap();
    papercut::adapters::claude_code::install_hook("/x/papercut")
        .unwrap_or_else(|e| panic!("BOM-prefixed JSON must parse, not be refused: {e}"));
    let v2: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(v2["model"], "opus", "user key preserved past BOM strip");
    assert!(v2
        .pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
        .is_some());
}

/// A user-owned empty PostToolUse group (a placeholder for another matcher) must
/// survive both install and uninstall — we touch only entries whose command is
/// ours. Previously the empty-group retain pruned it unconditionally.
#[test]
fn empty_user_posttooluse_group_is_preserved() {
    let env = IsolatedEnv::new().with_claude();
    let settings = env.home.join(".claude/settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[]}]}}"#,
    )
    .unwrap();

    papercut::adapters::claude_code::install_hook("/x/papercut").unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        v.pointer("/hooks/PostToolUse/0/matcher")
            .and_then(|m| m.as_str()),
        Some("Edit"),
        "user's empty Edit group preserved on install: {v}"
    );
    assert!(
        v.pointer("/hooks/PostToolUseFailure/0/hooks/0/command")
            .is_some(),
        "our hook wired under the failure event"
    );

    // Uninstall removes only our (Bash) group; the empty Edit group stays.
    papercut::adapters::claude_code::uninstall_hook().unwrap();
    let v2: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        v2.pointer("/hooks/PostToolUse/0/matcher")
            .and_then(|m| m.as_str()),
        Some("Edit"),
        "user's empty Edit group survives uninstall: {v2}"
    );
    assert!(
        v2.pointer("/hooks/PostToolUseFailure").is_none(),
        "our event key (emptied by removing our hook) is dropped"
    );
}
