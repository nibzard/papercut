//! Deterministic markdown projection of the store.
//!
//! `render` output is byte-identical for the same set of events: fixed status
//! ordering, stable field order, no timestamps, no platform-dependent separators.

use crate::model::{Event, Status};
use crate::paths::RepoScope;
use crate::util::truncate;

const SUMM_MAX: usize = 200;

pub fn render_markdown(scope: &RepoScope, events: &[Event]) -> String {
    // Sort by id (≈ chronological) so output is independent of input order.
    let mut events: Vec<&Event> = events.iter().collect();
    events.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::new();
    out.push_str("# Papercuts\n\n");
    match scope {
        RepoScope::One(r) => out.push_str(&format!("repo: `{r}`\n\n")),
        RepoScope::All => out.push_str("scope: global (all repos)\n\n"),
    }

    if events.is_empty() {
        out.push_str("_No events recorded._\n");
        return out;
    }

    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in &events {
        *counts.entry(e.status.label()).or_default() += 1;
    }
    let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    out.push_str(&format!(
        "{} event(s) ({})\n\n",
        events.len(),
        parts.join(", ")
    ));

    for st in [
        Status::Open,
        Status::Candidate,
        Status::Fixed,
        Status::Promoted,
        Status::Duplicate,
        Status::Dismissed,
    ] {
        let group: Vec<&&Event> = events.iter().filter(|e| e.status == st).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("## {} ({})\n\n", st.label(), group.len()));
        for e in group {
            out.push_str(&format!(
                "- **{}** {}\n",
                e.id,
                truncate(&e.summary, SUMM_MAX)
            ));
            let mut bits: Vec<String> = Vec::new();
            if let Some(a) = &e.context.agent {
                bits.push(format!("_{a}_"));
            }
            if let Some(c) = &e.context.cwd {
                bits.push(format!("`{c}`"));
            }
            if let Some(s) = &e.context.git_sha {
                bits.push(format!("`{s}`"));
            }
            if let Some(t) = &e.context.task {
                bits.push(format!("task: {t}"));
            }
            if !bits.is_empty() {
                out.push_str(&format!("  - {}\n", bits.join(" · ")));
            }
            if let Some(h) = &e.hypothesis {
                out.push_str(&format!("  - hypothesis: {}\n", truncate(h, SUMM_MAX)));
            }
            if let Some(fx) = &e.suggested_fix {
                out.push_str(&format!("  - fix: {}\n", truncate(fx, SUMM_MAX)));
            }
            if let Some(cat) = &e.category {
                out.push_str(&format!("  - category: {cat}\n"));
            }
            if let Some(res) = &e.resolution {
                let mut line = format!("  - resolved: {}", res.reason);
                if let Some(rf) = &res.ref_ {
                    line.push_str(&format!(" (`{rf}`)"));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventContext, Resolution, Source};

    fn ev(id: &str, status: Status, summary: &str) -> Event {
        Event {
            schema_version: 1,
            id: id.into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status,
            summary: summary.into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext {
                agent: Some("claude-code".into()),
                cwd: Some("src".into()),
                git_sha: Some("abc1234".into()),
                ..Default::default()
            },
            resolution: None,
        }
    }

    #[test]
    fn same_events_byte_identical() {
        let events = vec![
            ev("pc_01K000000000000000000000A", Status::Open, "a"),
            ev("pc_01K000000000000000000000B", Status::Fixed, "b"),
        ];
        let a = render_markdown(&RepoScope::All, &events);
        let b = render_markdown(&RepoScope::All, &events);
        assert_eq!(a, b);
    }

    #[test]
    fn open_before_terminal_section_order() {
        let events = vec![
            ev("pc_01K000000000000000000000A", Status::Fixed, "fixed"),
            ev("pc_01K000000000000000000000B", Status::Open, "open"),
        ];
        let md = render_markdown(&RepoScope::All, &events);
        let open_idx = md.find("## open").unwrap();
        let fixed_idx = md.find("## fixed").unwrap();
        assert!(open_idx < fixed_idx);
    }

    #[test]
    fn renders_resolution_ref() {
        let mut e = ev("pc_01K000000000000000000000C", Status::Fixed, "x");
        e.resolution = Some(Resolution {
            reason: "pinned dep".into(),
            ref_: Some("abc1234".into()),
        });
        let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
        assert!(md.contains("resolved: pinned dep (`abc1234`)"));
    }
}
