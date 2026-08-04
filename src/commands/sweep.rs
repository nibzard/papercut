//! `papercut sweep` — parse harness session logs since the last sweep.

use crate::app::RunResult;
use crate::cli::SweepArgs;
use serde_json::{json, Value};

pub fn run(args: SweepArgs) -> RunResult {
    let want_codex = match args.harness.as_deref() {
        None => true,
        Some(s) => s.split(',').any(|x| x.trim() == "codex"),
    };

    let mut results: Vec<Value> = Vec::new();
    let mut total = 0usize;
    if want_codex {
        let o = crate::adapters::codex::sweep();
        total += o.signals_emitted;
        results.push(json!({
            "harness": o.harness,
            "signals": o.signals_emitted,
            "sessions_scanned": o.sessions_scanned,
            "sessions_with_commands": o.sessions_with_commands,
            "warning": o.warning,
        }));
    }

    let data = json!({ "results": results, "signals_total": total });
    let text = format_sweep(&results, total);
    RunResult::Ok { data, text }
}

fn format_sweep(results: &[Value], total: usize) -> String {
    if results.is_empty() {
        return "no sweep-tier harnesses selected".into();
    }
    let mut out = String::new();
    for r in results {
        let h = r.get("harness").and_then(|v| v.as_str()).unwrap_or("?");
        let n = r.get("signals").and_then(|v| v.as_u64()).unwrap_or(0);
        let scanned = r
            .get("sessions_scanned")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        out.push_str(&format!("{h}: {n} signal(s) from {scanned} session(s)\n"));
        if let Some(w) = r.get("warning").and_then(|v| v.as_str()) {
            out.push_str(&format!("  warning: {w}\n"));
        }
    }
    out.push_str(&format!("total: {total} signal(s)"));
    out
}
