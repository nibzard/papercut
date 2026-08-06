//! `papercut list` — filtered listing.

use crate::app::RunResult;
use crate::cli::ListArgs;
use crate::paths::resolve_repo_filter;
use crate::query::{current_repo, Filters};
use crate::util::{md_single_line, truncate, SKIPPED_LIST_MAX, SKIPPED_REASON_MAX};
use serde_json::json;

pub fn run(args: ListArgs) -> RunResult {
    let cur = current_repo();
    let scope = resolve_repo_filter(Some(args.repo.as_str()), cur.as_deref());
    let filters = Filters {
        scope,
        status: args.status,
        agent: args.agent,
        since_days: args.since,
    };
    let (events, skipped) = crate::query::load(&filters);

    let data = json!({
        "events": events,
        "count": events.len(),
        "skipped": skipped,
    });
    let text = render_text(&events, &skipped);
    RunResult::Ok { data, text }
}

fn render_text(events: &[crate::model::Event], skipped: &[crate::store::SkippedFile]) -> String {
    if events.is_empty() {
        let mut s = String::from("no events");
        append_skipped(&mut s, skipped);
        return s;
    }
    let mut out = String::new();
    for e in events {
        let repo = e.context.repo.as_deref().unwrap_or("(global)");
        out.push_str(&format!(
            "{}  {:9} {}  {}\n",
            e.id,
            e.status.label(),
            truncate(repo, 40),
            truncate(&md_single_line(&e.summary), 72),
        ));
    }
    append_skipped(&mut out, skipped);
    out.trim_end().to_string()
}

/// Append a human-readable list of skipped (quarantined) event files with the
/// reason each was dropped, so a human can see WHICH file vanished and WHY
/// (schema / invariant / corruption) rather than just an opaque count. The
/// reason is truncated and the listing is count-capped so a store with many
/// corrupt files cannot bloat the output.
fn append_skipped(out: &mut String, skipped: &[crate::store::SkippedFile]) {
    if skipped.is_empty() {
        return;
    }
    out.push_str(&format!(
        "({} unreadable event file(s) skipped)\n",
        skipped.len()
    ));
    for s in skipped.iter().take(SKIPPED_LIST_MAX) {
        out.push_str(&format!(
            "  - {}: {}\n",
            s.file_label(),
            truncate(&s.reason, SKIPPED_REASON_MAX)
        ));
    }
    if skipped.len() > SKIPPED_LIST_MAX {
        out.push_str(&format!(
            "  … {} more not shown\n",
            skipped.len() - SKIPPED_LIST_MAX
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventContext, Source, Status};
    use crate::store::SkippedFile;

    #[test]
    fn empty_listing() {
        assert_eq!(render_text(&[], &[]), "no events");
        let skipped = vec![SkippedFile {
            file: "pc_01KGARBAGE0000000000000Z.json".into(),
            reason: "parse error:EOF".into(),
            repo: None,
        }];
        let txt = render_text(&[], &skipped);
        assert!(txt.contains("1 unreadable event file(s) skipped"));
        assert!(txt.contains("pc_01KGARBAGE0000000000000Z.json: parse error:EOF"));
    }

    #[test]
    fn one_per_line() {
        let e = crate::model::Event {
            schema_version: 1,
            id: "pc_01K000000000000000000000A".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            summary: "glob ate my args".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext {
                repo: Some("github.com/foo/bar".into()),
                ..Default::default()
            },
            resolution: None,
        };
        let txt = render_text(std::slice::from_ref(&e), &[]);
        assert!(txt.contains("pc_01K000000000000000000000A"));
        assert!(txt.contains("open"));
        assert!(txt.contains("github.com/foo/bar"));
        assert_eq!(txt.lines().count(), 1);
    }

    /// A multiline summary must not forge extra rows — `list` is one event per
    /// line, so embedded newlines collapse onto the single row.
    #[test]
    fn multiline_summary_stays_one_line() {
        let e = crate::model::Event {
            schema_version: 1,
            id: "pc_01K000000000000000000000B".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            summary: "first line\nsecond line".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext::default(),
            resolution: None,
        };
        let txt = render_text(std::slice::from_ref(&e), &[]);
        assert_eq!(
            txt.lines().count(),
            1,
            "multiline summary collapses to one row: {txt}"
        );
        assert!(txt.contains("first line") && txt.contains("second line"));
    }
}
