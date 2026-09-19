//! Resolution health metrics derived from event records.

use std::collections::BTreeMap;

use crate::app::RunResult;
use crate::model::Status;
use serde_json::json;

pub fn run() -> RunResult {
    let (events, skipped) = crate::store::read_all_events();
    let mut statuses: BTreeMap<&str, usize> = BTreeMap::new();
    let mut remedies: BTreeMap<&str, usize> = BTreeMap::new();
    let mut resolution_seconds = Vec::new();
    let mut resolved_without_metrics = 0usize;

    for event in &events {
        *statuses.entry(event.status.label()).or_default() += 1;
        if !event.status.is_terminal() {
            continue;
        }
        let Some(resolution) = &event.resolution else {
            continue;
        };
        if let Some(remedy) = resolution.remedy {
            *remedies.entry(remedy.label()).or_default() += 1;
        }
        match (
            crate::time::parse_rfc3339_unix(&event.created_at),
            resolution
                .resolved_at
                .as_deref()
                .and_then(crate::time::parse_rfc3339_unix),
        ) {
            (Some(created), Some(resolved)) if resolved >= created => {
                resolution_seconds.push(resolved - created);
            }
            _ => resolved_without_metrics += 1,
        }
    }
    resolution_seconds.sort_unstable();
    let median = match resolution_seconds.len() {
        0 => None,
        len if len % 2 == 1 => resolution_seconds.get(len / 2).copied(),
        len => {
            Some(resolution_seconds[len / 2 - 1].saturating_add(resolution_seconds[len / 2]) / 2)
        }
    };
    let terminal = events
        .iter()
        .filter(|event| event.status.is_terminal())
        .count();
    let open = events
        .iter()
        .filter(|event| matches!(event.status, Status::Open | Status::Candidate))
        .count();
    let data = json!({
        "events": events.len(),
        "open_or_candidate": open,
        "terminal": terminal,
        "statuses": statuses,
        "remedies": remedies,
        "median_resolution_seconds": median,
        "terminal_events_without_timing": resolved_without_metrics,
        "skipped": skipped,
        "recurrence_after_fix": "requires semantic linkage during triage",
    });
    let text = format!(
        "events: {}\nopen or candidate: {}\nterminal: {}\nmedian resolution: {}\nterminal records without timing: {}",
        events.len(),
        open,
        terminal,
        median
            .map(|seconds| format!("{seconds}s"))
            .unwrap_or_else(|| "unknown".into()),
        resolved_without_metrics,
    );
    RunResult::Ok { data, text }
}
