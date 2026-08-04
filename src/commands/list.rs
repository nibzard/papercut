//! `papercut list` — filtered listing.

use crate::app::RunResult;
use crate::cli::ListArgs;
use crate::paths::resolve_repo_filter;
use crate::query::{current_repo, Filters};
use crate::util::truncate;
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
        "skipped_files": skipped.len(),
    });
    let text = render_text(&events, skipped.len());
    RunResult::Ok { data, text }
}

fn render_text(events: &[crate::model::Event], skipped: usize) -> String {
    if events.is_empty() {
        let mut s = String::from("no events");
        if skipped > 0 {
            s.push_str(&format!(" ({skipped} unreadable file(s) skipped)"));
        }
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
            truncate(&e.summary, 72),
        ));
    }
    if skipped > 0 {
        out.push_str(&format!("({skipped} unreadable event file(s) skipped)\n"));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventContext, Source, Status};

    #[test]
    fn empty_listing() {
        assert_eq!(render_text(&[], 0), "no events");
        assert_eq!(
            render_text(&[], 1),
            "no events (1 unreadable file(s) skipped)"
        );
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
        let txt = render_text(std::slice::from_ref(&e), 0);
        assert!(txt.contains("pc_01K000000000000000000000A"));
        assert!(txt.contains("open"));
        assert!(txt.contains("github.com/foo/bar"));
        assert_eq!(txt.lines().count(), 1);
    }
}
