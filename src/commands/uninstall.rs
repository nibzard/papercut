//! `papercut uninstall` — remove all managed blocks and adapters cleanly.
//!
//! Unlike the silent hook path, this CLI command is honest about failure. A
//! block write, adapter removal, or config write that fails is reported with a
//! non-zero exit rather than swallowed, and the partial result records what
//! actually came off disk — `block_removed` reflects the real write outcome,
//! not the in-memory intent.

use crate::app::RunResult;
use crate::cli::UninstallArgs;
use crate::harness::{detect, filter_detected, filter_selects, unknown_harness_error};
use crate::managed_block;
use crate::output::ErrorItem;
use crate::store;
use serde_json::{json, Value};
use std::path::PathBuf;

/// One harness to clean: every instructions file that may hold our block —
/// the currently detected path plus any different path recorded in
/// config.json (config-dir drift, or a harness no longer detected at all).
struct Target {
    id: String,
    files: Vec<PathBuf>,
}

pub fn run(args: UninstallArgs) -> RunResult {
    let filter = args.harness.as_deref();
    if let Some(item) = unknown_harness_error(&args.harness) {
        return RunResult::usage(item);
    }
    let selected = filter_detected(detect(), filter);

    let mut results: Vec<Value> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    let mut cfg = store::read_config();

    let mut targets: Vec<Target> = selected
        .iter()
        .map(|h| Target {
            id: h.id.clone(),
            files: vec![h.instructions_file.clone()],
        })
        .collect();
    for entry in &cfg.installed {
        if !filter_selects(filter, &entry.harness) {
            continue;
        }
        let rec = PathBuf::from(&entry.instructions_file);
        match targets.iter_mut().find(|t| t.id == entry.harness) {
            Some(t) => {
                if !t.files.contains(&rec) {
                    t.files.push(rec);
                }
            }
            None => targets.push(Target {
                id: entry.harness.clone(),
                files: vec![rec],
            }),
        }
    }

    for t in &targets {
        let mut removed_adapter = 0usize;
        if t.id == "claude-code" {
            match crate::adapters::claude_code::uninstall_hook() {
                Ok(n) => removed_adapter = n,
                Err(e) => errs.push(format!("{}: adapter: {e}", t.id)),
            }
        }

        // Clean every file this harness may hold a block in. Missing files
        // mean nothing to remove; any other read error (invalid UTF-8,
        // permission denied) means we cannot safely inspect or rewrite the
        // file, so we refuse rather than feed "" to remove and report a
        // misleading "nothing to uninstall".
        let mut any_removed = false;
        // Every file verified block-free (removed and written, or had no
        // block). A failed read or write leaves the block state dirty.
        let mut clean = true;
        for file in &t.files {
            match std::fs::read_to_string(file) {
                Ok(text) => {
                    let (new_content, removed_block) = managed_block::remove(&text);
                    if removed_block {
                        // Reflect the actual on-disk outcome: a failed write
                        // means the block is still present.
                        match std::fs::write(file, &new_content) {
                            Ok(()) => any_removed = true,
                            Err(e) => {
                                errs.push(format!("{}: write {}: {e}", t.id, file.display()));
                                clean = false;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    errs.push(format!("{}: read {}: {e}", t.id, file.display()));
                    clean = false;
                }
            }
        }

        results.push(json!({
            "harness": t.id,
            "block_removed": any_removed,
            "adapter_removed": removed_adapter,
        }));

        // Keep config consistent with disk: drop the entry only when every
        // file is verified block-free. A failed read or write leaves the
        // entry in place so a later `doctor`/`uninstall` still sees it.
        if clean {
            cfg.installed.retain(|i| i.harness != t.id);
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
