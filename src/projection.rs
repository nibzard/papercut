//! Deterministic markdown projection of the store.
//!
//! `render` output is byte-identical for the same set of events: fixed status
//! ordering, stable field order, no timestamps, no platform-dependent separators.

use crate::model::{Event, Status};
use crate::paths::RepoScope;
use crate::util::{md_code_span, md_indent_continuation, md_single_line, truncate};

const SUMM_MAX: usize = 200;

pub fn render_markdown(scope: &RepoScope, events: &[Event]) -> String {
    // Sort by id (≈ chronological) so output is independent of input order.
    let mut events: Vec<&Event> = events.iter().collect();
    events.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::new();
    out.push_str("# Papercuts\n\n");
    match scope {
        RepoScope::One(r) => out.push_str(&format!("repo: {}\n\n", md_code_span(r))),
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
                md_single_line(&e.id),
                md_indent_continuation(&truncate(&e.summary, SUMM_MAX), "  ")
            ));
            let mut bits: Vec<String> = Vec::new();
            if let Some(product) = e.attributed_product() {
                bits.push(format!("product: {}", md_code_span(&product.id)));
                if let Some(version) = &product.version {
                    bits.push(format!("version: {}", md_code_span(version)));
                }
                if let Some(surface) = &product.surface {
                    bits.push(format!("surface: {}", md_code_span(surface)));
                }
            } else {
                bits.push("unattributed legacy".into());
            }
            if let Some(repo) = &e.context.repo {
                if matches!(scope, RepoScope::All) {
                    bits.push(format!("consumer repo: {}", md_code_span(repo)));
                }
            }
            if let Some(a) = &e.context.agent {
                bits.push(format!("_{}_", md_single_line(a)));
            }
            if let Some(c) = &e.context.cwd {
                bits.push(md_code_span(c));
            }
            if let Some(s) = &e.context.git_sha {
                bits.push(md_code_span(s));
            }
            if let Some(t) = &e.context.task {
                bits.push(format!("task: {}", md_single_line(t)));
            }
            if !bits.is_empty() {
                out.push_str(&format!("  - {}\n", bits.join(" · ")));
            }
            if let Some(h) = &e.hypothesis {
                out.push_str(&format!(
                    "  - hypothesis: {}\n",
                    md_indent_continuation(&truncate(h, SUMM_MAX), "    ")
                ));
            }
            if let Some(fx) = &e.suggested_fix {
                out.push_str(&format!(
                    "  - fix: {}\n",
                    md_indent_continuation(&truncate(fx, SUMM_MAX), "    ")
                ));
            }
            if let Some(cat) = &e.category {
                out.push_str(&format!("  - category: {}\n", md_single_line(cat)));
            }
            // Only terminal events project their resolution. A reopened papercut
            // (non-terminal) may keep a stale resolution in the model — that's
            // allowed and load does not quarantine it — but surfacing "resolved:"
            // under an `## open` section would contradict the status grouping.
            // Grouping already keys off `status`; this keeps the per-event line
            // consistent with it.
            if e.status.is_terminal() {
                if let Some(res) = &e.resolution {
                    let reason = md_indent_continuation(&truncate(&res.reason, SUMM_MAX), "    ");
                    let mut line = format!("  - resolved: {reason}");
                    if let Some(rf) = &res.ref_ {
                        line.push_str(&format!(" ({})", md_code_span(rf)));
                    }
                    out.push_str(&line);
                    out.push('\n');
                }
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
            product: None,
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

    /// A multiline summary cannot forge a heading or a peer list item: every
    /// continuation line is indented off column 0, so attacker-shaped text
    /// stays a continuation of its own bullet instead of structuring the doc.
    #[test]
    fn multiline_summary_cannot_forge_structure() {
        let mut e = ev("pc_01K000000000000000000000F", Status::Open, "real");
        e.summary = "real\n## open (99)\n- fake peer event".into();
        let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
        // No line begins at column 0 with the forged heading or item.
        assert!(
            !md.lines().any(|l| l.starts_with("## open (99)")),
            "forged heading must not structure the doc: {md}"
        );
        assert!(
            !md.lines().any(|l| l == "- fake peer event"),
            "forged peer item must not appear at column 0: {md}"
        );
        assert!(md.contains("real"), "real summary text present");
        assert!(
            md.contains("fake peer"),
            "forged text kept as data, indented"
        );
    }

    /// The `agent` field is attacker-controllable (`add --agent`); a multiline
    /// value must not forge a heading in the projection.
    #[test]
    fn multiline_agent_cannot_forge_structure() {
        let mut e = ev("pc_01K000000000000000000000H", Status::Open, "ok");
        e.context.agent = Some("codex\n## INJECTED HEADING".into());
        let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
        assert!(
            !md.lines().any(|l| l.starts_with("## INJECTED")),
            "forged heading from agent blocked: {md}"
        );
    }

    /// A multiline hypothesis/fix is indented under its sub-bullet, never at
    /// column 0.
    #[test]
    fn multiline_hypothesis_cannot_forge_structure() {
        let mut e = ev("pc_01K000000000000000000000G", Status::Open, "ok");
        e.hypothesis = Some("guess\n## evil".into());
        let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
        assert!(
            !md.lines().any(|l| l.starts_with("## evil")),
            "forged heading from hypothesis blocked: {md}"
        );
    }

    /// A reopened papercut (status flipped back to `open`) is allowed to keep its
    /// stale resolution in the model, but render must NOT show a `resolved:` line
    /// for it — that would contradict the `## open` grouping. Only terminal
    /// statuses project their resolution.
    #[test]
    fn reopened_event_suppresses_resolution_line() {
        let mut e = ev("pc_01K000000000000000000000E", Status::Open, "recurred");
        // A ref distinct from the helper's `git_sha` so we can tell a leaked ref
        // apart from the legitimately-rendered sha line.
        e.resolution = Some(Resolution {
            reason: "previously fixed".into(),
            ref_: Some("deadbeef".into()),
        });
        let md = render_markdown(&RepoScope::All, std::slice::from_ref(&e));
        assert!(md.contains("## open"), "still grouped under open: {md}");
        assert!(
            !md.contains("resolved:"),
            "stale resolution must not project for a non-terminal event: {md}"
        );
        assert!(
            !md.contains("deadbeef"),
            "stale ref must not leak into a non-terminal projection: {md}"
        );
    }
}
