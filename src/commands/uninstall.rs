//! `papercut uninstall` — remove all managed blocks and adapters cleanly.
//!
//! Unlike the silent hook path, this CLI command is honest about failure. A
//! block write, adapter removal, or config write that fails is reported with a
//! non-zero exit rather than swallowed, and the partial result records what
//! actually came off disk — `block_removed` reflects the real write outcome,
//! not the in-memory intent.

use crate::app::RunResult;
use crate::cli::UninstallArgs;
use crate::harness::{detect, DetectedHarness};
use crate::managed_block;
use crate::output::ErrorItem;
use crate::store;
use serde_json::{json, Value};

pub fn run(args: UninstallArgs) -> RunResult {
    let detected = detect();
    let selected = filter_harnesses(detected, args.harness.as_deref());

    let mut results: Vec<Value> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    let mut cfg = store::read_config();

    for h in &selected {
        let mut removed_adapter = 0usize;
        if h.id == "claude-code" {
            match crate::adapters::claude_code::uninstall_hook() {
                Ok(n) => removed_adapter = n,
                Err(e) => errs.push(format!("{}: adapter: {e}", h.id)),
            }
        }

        // Read the instructions file. Missing = nothing to remove; any other
        // read error (invalid UTF-8, permission denied, transient I/O) means we
        // cannot safely inspect or rewrite it, so we refuse rather than feed ""
        // to remove and report a misleading "nothing to uninstall".
        let (new_content, removed_block) = match std::fs::read_to_string(&h.instructions_file) {
            Ok(t) => managed_block::remove(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => {
                errs.push(format!(
                    "{}: read {}: {e}",
                    h.id,
                    h.instructions_file.display()
                ));
                // Block state unknown; keep the config entry and record nothing
                // removed for this harness. Adapter removal above still stands.
                results.push(json!({
                    "harness": h.id,
                    "block_removed": false,
                    "adapter_removed": removed_adapter,
                }));
                continue;
            }
        };
        // Reflect the actual on-disk outcome: a failed write means the block is
        // still present, so report `false` and surface the error.
        let block_persisted = if removed_block {
            match std::fs::write(&h.instructions_file, &new_content) {
                Ok(()) => true,
                Err(e) => {
                    errs.push(format!(
                        "{}: write {}: {e}",
                        h.id,
                        h.instructions_file.display()
                    ));
                    false
                }
            }
        } else {
            false
        };

        results.push(json!({
            "harness": h.id,
            "block_removed": block_persisted,
            "adapter_removed": removed_adapter,
        }));

        // Keep config consistent with disk: drop the entry only when the block
        // is actually gone — written out, or absent to begin with. A failed
        // write leaves the block on disk, so the entry must stay (otherwise a
        // later `doctor`/`install` would misreport state).
        let block_gone = !removed_block || block_persisted;
        if block_gone {
            cfg.installed.retain(|i| i.harness != h.id);
        }
    }

    if let Err(e) = store::write_config(&cfg) {
        errs.push(format!("config: {e}"));
    }

    let text = format_summary(&results);
    if errs.is_empty() {
        RunResult::Ok {
            data: json!({ "uninstalled": results }),
            text,
        }
    } else {
        let errors = errs
            .iter()
            .map(|m| {
                ErrorItem::new(
                    "uninstall_partial",
                    m.clone(),
                    true,
                    "re-run papercut uninstall; check permissions on the listed paths",
                )
            })
            .collect::<Vec<_>>();
        RunResult::Err {
            data: json!({ "uninstalled": results }),
            errors,
        }
    }
}

fn filter_harnesses(detected: Vec<DetectedHarness>, filter: Option<&str>) -> Vec<DetectedHarness> {
    match filter {
        None => detected,
        Some(list) => {
            let want: Vec<&str> = list.split(',').map(|s| s.trim()).collect();
            detected
                .into_iter()
                .filter(|d| want.iter().any(|w| *w == d.id))
                .collect()
        }
    }
}

fn format_summary(results: &[Value]) -> String {
    if results.is_empty() {
        return "nothing to uninstall".into();
    }
    let mut out = String::new();
    for r in results {
        let id = r.get("harness").and_then(|v| v.as_str()).unwrap_or("?");
        let b = r
            .get("block_removed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let a = r
            .get("adapter_removed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        out.push_str(&format!(
            "{id}: block {}, adapter entries removed {}\n",
            yn(b),
            a
        ));
    }
    out.trim_end().to_string()
}

fn yn(b: bool) -> &'static str {
    if b {
        "removed"
    } else {
        "absent"
    }
}
