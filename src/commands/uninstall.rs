//! `papercut uninstall` — remove all managed blocks and adapters cleanly.

use crate::app::RunResult;
use crate::cli::UninstallArgs;
use crate::harness::{detect, DetectedHarness};
use crate::managed_block;
use crate::store;
use serde_json::{json, Value};

pub fn run(args: UninstallArgs) -> RunResult {
    let detected = detect();
    let selected = filter_harnesses(detected, args.harness.as_deref());

    let mut results: Vec<Value> = Vec::new();
    let mut cfg = store::read_config();

    for h in &selected {
        let mut removed_adapter = 0usize;
        if h.id == "claude-code" {
            if let Ok(n) = crate::adapters::claude_code::uninstall_hook() {
                removed_adapter = n;
            }
        }

        let content = std::fs::read_to_string(&h.instructions_file).unwrap_or_default();
        let (new_content, removed_block) = managed_block::remove(&content);
        if removed_block {
            let _ = std::fs::write(&h.instructions_file, new_content);
        }

        results.push(json!({
            "harness": h.id,
            "block_removed": removed_block,
            "adapter_removed": removed_adapter,
        }));

        cfg.installed.retain(|i| i.harness != h.id);
    }

    let _ = store::write_config(&cfg);

    let text = format_summary(&results);
    RunResult::Ok {
        data: json!({ "uninstalled": results }),
        text,
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
