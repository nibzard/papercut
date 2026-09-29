//! `papercut show` — inspect one complete report without mutating the store.

use crate::app::RunResult;
use crate::cli::ShowArgs;
use crate::model::Event;
use crate::output::ErrorItem;
use crate::store::read_events_by_ids;
use crate::util::{append_wrapped, md_single_line};
use serde_json::json;

pub fn run(args: ShowArgs) -> RunResult {
    let is_full = args.id.starts_with(crate::id::ID_PREFIX);
    let (events, skipped) = if is_full {
        read_events_by_ids(std::slice::from_ref(&args.id))
    } else {
        if args.id.len() < 6
            || args.id.len() > 26
            || !args.id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return RunResult::usage(ErrorItem::new(
                "invalid_ref",
                "event reference must be a full ID or an ID suffix of 6–26 letters/digits",
                false,
                "copy a Ref from `papercut list`, or use a full ID",
            ));
        }
        let (all, skipped) = crate::store::read_all_events();
        let upper = args.id.to_ascii_uppercase();
        let matches: Vec<Event> = all
            .into_iter()
            .filter(|event| event.id.ends_with(&upper))
            .collect();
        (matches, skipped)
    };
    if events.len() > 1 {
        return RunResult::usage(ErrorItem::new(
            "ambiguous_ref",
            format!("reference {} matches {} events", args.id, events.len()),
            false,
            "use a longer suffix or a full ID from `papercut list --output json`",
        ));
    }
    if let Some(event) = events.into_iter().next() {
        return RunResult::Ok {
            data: json!({ "event": event }),
            text: render_text(&event),
        };
    }

    if let Some(file) = skipped
        .iter()
        .find(|file| file.file == format!("{}.json", args.id))
    {
        return RunResult::err(ErrorItem::new(
            "event_unreadable",
            format!("event {} could not be read: {}", args.id, file.reason),
            false,
            "inspect the event file in the private store",
        ));
    }
    RunResult::err(ErrorItem::new(
        "event_not_found",
        format!("no event with ID {}", args.id),
        false,
        "run `papercut list --repo all` to find an event reference",
    ))
}

fn render_text(event: &Event) -> String {
    let mut out = format!(
        "Papercut {}\nStatus: {}\nCreated: {}\nSource: {}\nSchema: {}\n\nObservation:\n",
        md_single_line(&event.id),
        event.status.label(),
        md_single_line(&event.created_at),
        event.source.label(),
        event.schema_version,
    );
    append_wrapped(&mut out, "  ", &event.summary);
    if let Some(product) = event.attributed_product() {
        append_optional(&mut out, "Product", Some(&product.id));
        append_optional(&mut out, "Product version", product.version.as_deref());
        append_optional(&mut out, "Product surface", product.surface.as_deref());
    } else {
        out.push_str("Product: (unattributed legacy)\n");
    }
    if let Some(value) = &event.hypothesis {
        append_wrapped(&mut out, "Hypothesis: ", value);
    }
    if let Some(value) = &event.suggested_fix {
        append_wrapped(&mut out, "Suggested fix: ", value);
    }
    if let Some(value) = &event.category {
        out.push_str(&format!("Category: {}\n", md_single_line(value)));
    }

    out.push_str("\nContext:\n");
    append_optional(&mut out, "Repo", event.context.repo.as_deref());
    append_optional(&mut out, "Working directory", event.context.cwd.as_deref());
    append_optional(&mut out, "Git SHA", event.context.git_sha.as_deref());
    append_optional(&mut out, "Agent", event.context.agent.as_deref());
    append_optional(&mut out, "Session", event.context.session.as_deref());
    append_optional(&mut out, "Task", event.context.task.as_deref());

    if let Some(resolution) = &event.resolution {
        out.push_str("\nResolution:\n");
        append_wrapped(&mut out, "  Reason: ", &resolution.reason);
        append_optional(&mut out, "Ref", resolution.ref_.as_deref());
    }
    out.trim_end().to_string()
}

fn append_optional(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value {
        append_wrapped(out, &format!("  {label}: "), value);
    }
}
