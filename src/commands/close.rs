//! papercut close — record a verified terminal disposition.

use crate::app::RunResult;
use crate::cli::CloseArgs;
use crate::model::{Resolution, Status};
use crate::output::ErrorItem;
use serde_json::json;

pub fn run(args: CloseArgs) -> RunResult {
    if !args.status.is_terminal() {
        return RunResult::usage(ErrorItem::new(
            "non_terminal_status",
            "close requires fixed, promoted, duplicate, or dismissed",
            false,
            "choose a terminal --status",
        ));
    }
    if args.reason.trim().is_empty() {
        return RunResult::usage(ErrorItem::new(
            "empty_reason",
            "resolution reason is empty",
            false,
            "pass --reason with the verified outcome",
        ));
    }
    if args.status == Status::Fixed
        && args
            .ref_
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return RunResult::usage(ErrorItem::new(
            "missing_ref",
            "fixed status requires a non-empty reference",
            false,
            "pass --ref with a commit, issue, documentation, or system reference",
        ));
    }
    if !args.id.starts_with("pc_")
        || !args
            .id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return RunResult::usage(ErrorItem::new(
            "invalid_event_id",
            "event id is not a safe papercut identifier",
            false,
            "copy the id from papercut list --output json",
        ));
    }

    let (mut events, skipped) = crate::store::read_events_by_ids(std::slice::from_ref(&args.id));
    if events.is_empty() {
        let detail = skipped
            .iter()
            .find(|item| item.file.starts_with(&args.id))
            .map(|item| format!(": {}", item.reason))
            .unwrap_or_default();
        return RunResult::err(ErrorItem::new(
            "event_not_found",
            format!("event {} was not found or readable{detail}", args.id),
            false,
            "copy the id from papercut list --output json",
        ));
    }

    let mut event = events.remove(0);
    event.status = args.status;
    event.resolution = Some(Resolution {
        reason: args.reason.trim().to_string(),
        ref_: args
            .ref_
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        resolved_at: Some(crate::time::now_rfc3339()),
        remedy: Some(args.remedy),
    });
    if let Err(reason) = event.validate() {
        return RunResult::err(ErrorItem::new(
            "invalid_resolution",
            reason,
            false,
            "check the terminal status, reason, and reference",
        ));
    }

    match crate::store::write_event(&event) {
        Ok(_) => RunResult::Ok {
            data: json!({
                "id": event.id,
                "status": event.status.label(),
                "resolution": event.resolution,
            }),
            text: format!("{}  {}", event.id, event.status.label()),
        },
        Err(error) => RunResult::err(ErrorItem::new(
            "write_failed",
            format!("event was not closed: {error:#}"),
            true,
            "check store permissions and retry",
        )),
    }
}
