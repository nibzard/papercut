//! `papercut install` — detect harnesses, install managed blocks, wire adapters.

use crate::app::RunResult;
use crate::cli::InstallArgs;
use crate::harness::{detect, DetectedHarness, HarnessTier};
use crate::managed_block::{self, Action};
use crate::output::ErrorItem;
use crate::store::{self, AdapterState, InstalledHarness};
use serde_json::{json, Value};
use std::path::Path;

pub fn run(args: InstallArgs) -> RunResult {
    let _ = args.yes; // install never prompts; --yes is the explicit script flag.
    let detected = detect();
    let selected = filter_harnesses(detected, args.harness.as_deref());

    if selected.is_empty() {
        return RunResult::err(ErrorItem::new(
            "no_harnesses",
            "no harnesses detected to install into",
            false,
            "create the harness config dir (e.g. ~/.claude), or pass --harness <id>",
        ));
    }

    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "papercut".to_string());

    let mut results: Vec<Value> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    let mut cfg = store::read_config();

    for h in &selected {
        let mut step = json!({
            "harness": h.id,
            "instructions_file": h.instructions_file.to_string_lossy(),
            "tier": h.tier.label(),
        });

        // 1. Managed block upsert.
        // Only a missing file is treated as empty (create fresh). Any other read
        // failure (invalid UTF-8, permission denied, transient I/O) must surface
        // as an error: feeding "" to upsert and overwriting would destroy the
        // user's real content outside our managed markers.
        let content = match std::fs::read_to_string(&h.instructions_file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                errs.push(format!(
                    "{}: read {}: {e}",
                    h.id,
                    h.instructions_file.display()
                ));
                continue;
            }
        };
        let (new_content, action) = managed_block::upsert(&content);
        let parent = h
            .instructions_file
            .parent()
            .unwrap_or_else(|| Path::new("."));
        if let Err(e) = std::fs::create_dir_all(parent) {
            errs.push(format!("{}: mkdir {}: {e}", h.id, parent.display()));
            continue;
        }
        if let Err(e) = std::fs::write(&h.instructions_file, new_content) {
            errs.push(format!(
                "{}: write {}: {e}",
                h.id,
                h.instructions_file.display()
            ));
            continue;
        }
        step["block"] = json!(match action {
            Action::Inserted => "inserted",
            Action::Updated => "updated",
            Action::Unchanged => "unchanged",
        });

        // 2. Adapter wiring.
        let (adapter, adapter_err) = wire(h, &exe);
        if let Some(e) = adapter_err {
            errs.push(format!("{}: adapter: {e}", h.id));
        }
        step["adapter"] = json!(adapter);

        results.push(step);

        // 3. Record in config.json (for doctor / uninstall).
        cfg.installed.retain(|i| i.harness != h.id);
        cfg.installed.push(InstalledHarness {
            harness: h.id.clone(),
            instructions_file: h.instructions_file.to_string_lossy().into_owned(),
            block_version: managed_block::BLOCK_VERSION,
            adapter,
        });
    }

    cfg.schema_version = crate::model::SCHEMA_VERSION;
    if let Err(e) = store::write_config(&cfg) {
        errs.push(format!("config: {e}"));
    }

    let text = format_summary(&results);
    if errs.is_empty() {
        RunResult::Ok {
            data: json!({ "installed": results }),
            text,
        }
    } else {
        // One structured error per failure; the partial result (what was
        // configured) rides in `data` so an agent does not lose successful work.
        let errors = errs
            .iter()
            .map(|m| {
                ErrorItem::new(
                    "install_partial",
                    m.clone(),
                    true,
                    "re-run papercut install; check permissions on the listed paths",
                )
            })
            .collect::<Vec<_>>();
        RunResult::Err {
            data: json!({ "installed": results }),
            errors,
        }
    }
}

/// Wire the harness's signal adapter, returning (state, optional error).
fn wire(h: &DetectedHarness, exe: &str) -> (Option<AdapterState>, Option<String>) {
    match h.tier {
        HarnessTier::Live => {
            // Currently only Claude Code is a live-tier harness.
            match crate::adapters::claude_code::install_hook(exe) {
                Ok(_) => (
                    Some(AdapterState {
                        kind: "claude-code-hook".into(),
                        detail: "PostToolUseFailure Bash".into(),
                    }),
                    None,
                ),
                Err(e) => (None, Some(format!("{e}"))),
            }
        }
        HarnessTier::Sweep => (
            Some(AdapterState {
                kind: "codex-sweep".into(),
                detail: "rollout jsonl".into(),
            }),
            None,
        ),
        HarnessTier::None => (None, None),
    }
}

fn filter_harnesses(detected: Vec<DetectedHarness>, filter: Option<&str>) -> Vec<DetectedHarness> {
    match filter {
        None => detected,
        Some(list) => {
            let want: Vec<&str> = list
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            detected
                .into_iter()
                .filter(|d| want.iter().any(|w| *w == d.id))
                .collect()
        }
    }
}

fn format_summary(results: &[Value]) -> String {
    if results.is_empty() {
        return "nothing installed".into();
    }
    let mut out = String::new();
    for r in results {
        let id = r.get("harness").and_then(|v| v.as_str()).unwrap_or("?");
        let block = r.get("block").and_then(|v| v.as_str()).unwrap_or("?");
        let adapter = r
            .get("adapter")
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        out.push_str(&format!("{id}: block {block}, adapter {adapter}\n"));
    }
    out.trim_end().to_string()
}
