//! Reports-only compatibility behavior for `papercut sweep`.

use crate::app::RunResult;
use crate::cli::SweepArgs;
use crate::harness::unknown_harness_error;
use serde_json::json;

pub fn run(args: SweepArgs) -> RunResult {
    if let Some(item) = unknown_harness_error(&args.harness) {
        return RunResult::usage(item);
    }
    RunResult::Ok {
        data: json!({
            "results": [],
            "signals_total": 0,
            "sessions_scanned": 0,
            "capture_mode": "reports_only",
        }),
        text: "reports_only: sweep is suspended until failures can be attributed to a designated product; 0 sessions scanned, 0 signals recorded".into(),
    }
}
