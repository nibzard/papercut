//! Product attribution across consumer repos and the reports-only upgrade.

mod common;

use common::IsolatedEnv;
use papercut::model::Product;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn cli(cwd: &Path, args: &[&str]) -> Output {
    Command::new(common::bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn json(cwd: &Path, args: &[&str]) -> Value {
    let out = cli(cwd, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn consumer(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(init.success());
    let remote = format!("https://github.com/example/{name}.git");
    let add = Command::new("git")
        .args(["remote", "add", "origin", &remote])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(add.success());
    dir
}

#[test]
fn sdk_reports_follow_product_across_consumer_repos() {
    let env = IsolatedEnv::new();
    let first = consumer(&env.home, "consumer-one");
    let second = consumer(&env.home, "consumer-two");

    json(
        &first,
        &[
            "--output",
            "json",
            "add",
            "--product",
            "example-sdk",
            "--product-version",
            "1.0",
            "--surface",
            "Client.list",
            "The docs omitted pagination",
        ],
    );
    json(
        &second,
        &[
            "--output",
            "json",
            "add",
            "--product",
            "example-sdk",
            "The iterator was hard to discover",
        ],
    );
    json(
        &first,
        &[
            "--output",
            "json",
            "add",
            "--product",
            "other-cli",
            "Help omitted an option",
        ],
    );

    let all_sdk = json(
        &first,
        &["--output", "json", "list", "--product", "example-sdk"],
    );
    assert_eq!(all_sdk["data"]["count"], 2);
    let events = all_sdk["data"]["events"].as_array().unwrap();
    let repos: Vec<&str> = events
        .iter()
        .map(|e| e["context"]["repo"].as_str().unwrap())
        .collect();
    assert!(repos.contains(&"github.com/example/consumer-one"));
    assert!(repos.contains(&"github.com/example/consumer-two"));
    assert!(events
        .iter()
        .any(|e| e["product"]["version"] == "1.0" && e["product"]["surface"] == "Client.list"));

    let scoped = json(
        &first,
        &[
            "--output",
            "json",
            "list",
            "--product",
            "example-sdk",
            "--repo",
            ".",
        ],
    );
    assert_eq!(scoped["data"]["count"], 1);
    let other = json(
        &first,
        &["--output", "json", "list", "--product", "other-cli"],
    );
    assert_eq!(other["data"]["count"], 1);

    let pack = json(
        &first,
        &[
            "--output",
            "json",
            "triage-pack",
            "--product",
            "example-sdk",
        ],
    );
    let markdown = pack["data"]["markdown"].as_str().unwrap();
    assert!(markdown.contains("The docs omitted pagination"));
    assert!(markdown.contains("The iterator was hard to discover"));
    assert!(!markdown.contains("Help omitted an option"));
    assert_eq!(pack["data"]["signal_clusters"], 0);
}

#[test]
fn legacy_reports_stay_visible_without_invented_product_identity() {
    let env = IsolatedEnv::new();
    let mut old = common::test_event("pc_01K000000000000000000000A", "old shell alias report");
    old.context.repo = Some("github.com/example/consumer-one".into());
    let path = papercut::store::write_event(&old).unwrap();
    let before = std::fs::read(&path).unwrap();

    let new = cli(
        &env.home,
        &[
            "add",
            "--product",
            "example-sdk",
            "--",
            "Public help gave a misleading result",
        ],
    );
    assert_eq!(new.status.code(), Some(0));
    let product = json(
        &env.home,
        &["--output", "json", "list", "--product", "example-sdk"],
    );
    assert_eq!(product["data"]["count"], 1);
    let legacy = json(&env.home, &["--output", "json", "list", "--unattributed"]);
    assert_eq!(legacy["data"]["count"], 1);
    assert_eq!(legacy["data"]["events"][0]["schema_version"], 1);
    assert!(legacy["data"]["events"][0].get("product").is_none());

    let id = old.id.as_str();
    let shown = json(&env.home, &["--output", "json", "show", id]);
    assert_eq!(shown["data"]["event"]["summary"], "old shell alias report");
    assert_eq!(std::fs::read(path).unwrap(), before);

    let malformed = serde_json::json!({
        "schema_version": 2, "id": "pc_01K000000000000000000000B",
        "created_at": "2026-09-29T09:00:00Z", "source": "in_moment",
        "status": "open", "summary": "missing product", "context": {}
    });
    std::fs::write(
        papercut::store::events_dir()
            .unwrap()
            .join("pc_01K000000000000000000000B.json"),
        serde_json::to_vec(&malformed).unwrap(),
    )
    .unwrap();
    let (events, skipped) = papercut::store::read_all_events();
    assert_eq!(events.len(), 2);
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].reason.contains("requires product"));
}

#[test]
fn product_is_required_and_rendering_keeps_global_view_private() {
    let env = IsolatedEnv::new();
    let repo = consumer(&env.home, "sample-app");
    let missing = cli(&repo, &["--output", "json", "add", "confusing output"]);
    assert_eq!(missing.status.code(), Some(2));
    let blank = cli(
        &repo,
        &[
            "--output",
            "json",
            "add",
            "--product",
            "   ",
            "confusing output",
        ],
    );
    assert_eq!(blank.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&blank.stdout).unwrap()["errors"][0]["code"],
        "missing_product"
    );
    assert!(papercut::store::read_all_events().0.is_empty());

    json(
        &repo,
        &[
            "--output",
            "json",
            "add",
            "--product",
            "target-cli",
            "Help hid the format option",
        ],
    );
    let global = json(
        &repo,
        &[
            "--output",
            "json",
            "render",
            "--product",
            "target-cli",
            "--write",
        ],
    );
    let private_path = env.data.join("papercuts/PAPERCUTS.md");
    assert_eq!(
        global["data"]["write"],
        private_path.to_string_lossy().as_ref()
    );
    assert!(private_path.exists());
    assert!(!repo.join("PAPERCUTS.md").exists());

    json(
        &repo,
        &[
            "--output",
            "json",
            "render",
            "--product",
            "target-cli",
            "--repo",
            ".",
            "--write",
        ],
    );
    assert!(repo.join("PAPERCUTS.md").exists());
}

#[cfg(unix)]
#[test]
fn upgrade_preserves_stowed_symlinks_and_unrelated_hook() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let env = IsolatedEnv::new().with_claude();
    let dotfiles = env.home.join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let instructions_target = dotfiles.join("CLAUDE.md");
    let settings_target = dotfiles.join("settings.json");
    std::fs::write(&instructions_target, "# Personal rules\n").unwrap();
    std::fs::write(&settings_target, r#"{"model":"opus","hooks":{"PostToolUseFailure":[{"matcher":"Bash","hooks":[{"type":"command","command":"/old/papercut _hook claude-code"},{"type":"command","command":"echo user-hook"}]}]}}"#).unwrap();
    std::fs::set_permissions(&settings_target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let instructions_link = env.home.join(".claude/CLAUDE.md");
    let settings_link = env.home.join(".claude/settings.json");
    symlink("../dotfiles/CLAUDE.md", &instructions_link).unwrap();
    symlink("../dotfiles/settings.json", &settings_link).unwrap();

    let installed = cli(&env.home, &["install", "--yes"]);
    assert_eq!(
        installed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(instructions_link
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(settings_link
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::metadata(&settings_target)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(std::fs::read_to_string(&instructions_target)
        .unwrap()
        .contains("papercut:begin v2"));
    let settings: Value =
        serde_json::from_slice(&std::fs::read(&settings_target).unwrap()).unwrap();
    assert_eq!(settings["model"], "opus");
    let hooks = settings["hooks"]["PostToolUseFailure"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0]["command"], "echo user-hook");

    let doctor = json(&env.home, &["--output", "json", "doctor"]);
    assert_eq!(doctor["data"]["healthy"], true);
    let uninstalled = cli(&env.home, &["uninstall"]);
    assert_eq!(uninstalled.status.code(), Some(0));
    assert!(instructions_link
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(settings_link
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::metadata(&settings_target)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(instructions_target).unwrap(),
        "# Personal rules\n"
    );
    let settings: Value = serde_json::from_slice(&std::fs::read(settings_target).unwrap()).unwrap();
    assert_eq!(
        settings["hooks"]["PostToolUseFailure"][0]["hooks"][0]["command"],
        "echo user-hook"
    );
}

#[test]
fn v2_model_rejects_empty_optional_metadata() {
    let mut event = common::test_event("pc_01K000000000000000000000A", "observed obstacle");
    event.schema_version = 2;
    event.product = Some(Product {
        id: "test-sdk".into(),
        version: Some("  ".into()),
        surface: None,
    });
    assert!(event.validate().unwrap_err().contains("product.version"));
}
