//! `papercut list` — filtered listing.

use crate::app::RunResult;
use crate::cli::ListArgs;
use crate::model::Status;
use crate::output::ErrorItem;
use crate::paths::RepoScope;
use crate::query::{current_repo, repo_scope, Filters, ProductScope};
use crate::util::{append_wrapped, md_single_line, truncate, SKIPPED_LIST_MAX, SKIPPED_REASON_MAX};
use serde_json::json;
use std::io::IsTerminal;

pub fn run(args: ListArgs) -> RunResult {
    let product = match ProductScope::from_args(args.product.as_deref(), args.unattributed) {
        Ok(scope) => scope,
        Err(message) => {
            return RunResult::usage(ErrorItem::new(
                "invalid_product",
                message,
                false,
                "provide a nonblank --product ID",
            ))
        }
    };
    let cur = current_repo();
    let scope = repo_scope(
        args.repo.as_deref(),
        cur.as_deref(),
        args.product.is_some() || args.unattributed,
    );
    let filters = Filters {
        scope,
        product,
        status: args.status,
        agent: args.agent,
        since_days: args.since,
    };
    let (mut events, skipped) = crate::query::load(&filters);
    events.sort_by(|left, right| {
        status_order(left.status)
            .cmp(&status_order(right.status))
            .then_with(|| left.id.cmp(&right.id))
    });

    let data = json!({
        "events": events,
        "count": events.len(),
        "skipped": skipped,
    });
    let color = should_color(
        std::io::stdout().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()),
    );
    let text = render_text(&events, &skipped, &filters.scope, color);
    let text = match &filters.product {
        ProductScope::One(id) => format!("Product: {}\n{text}", md_single_line(id)),
        ProductScope::Unattributed => format!("Unattributed legacy evidence\n{text}"),
        _ => text,
    };
    RunResult::Ok { data, text }
}

fn render_text(
    events: &[crate::model::Event],
    skipped: &[crate::store::SkippedFile],
    scope: &RepoScope,
    color: bool,
) -> String {
    if events.is_empty() {
        let mut s = match scope {
            RepoScope::One(repo) => format!(
                "No papercuts for {}.\nRun `papercut list --repo all` to see all repos.",
                md_single_line(repo)
            ),
            RepoScope::All => "No papercuts found.".to_string(),
        };
        append_skipped(&mut s, skipped);
        return s;
    }
    let all_ids: Vec<String> = crate::store::read_all_events()
        .0
        .into_iter()
        .map(|event| event.id)
        .collect();
    let name = match scope {
        RepoScope::One(repo) => repo.rsplit('/').next().unwrap_or(repo),
        RepoScope::All => "all repos",
    };
    let mut out = format!(
        "{}\n",
        styled(&format!("Papercuts · {}", md_single_line(name)), "1", color)
    );
    let breakdown = [
        Status::Open,
        Status::Candidate,
        Status::Fixed,
        Status::Promoted,
        Status::Duplicate,
        Status::Dismissed,
    ]
    .into_iter()
    .filter_map(|status| {
        let count = events.iter().filter(|event| event.status == status).count();
        (count > 0).then(|| format!("{count} {}", status.label()))
    })
    .collect::<Vec<_>>()
    .join(" · ");
    out.push_str(&breakdown);
    out.push('\n');
    let common_agent = events
        .first()
        .and_then(|first| first.context.agent.as_deref())
        .filter(|agent| {
            events
                .iter()
                .all(|event| event.context.agent.as_deref() == Some(*agent))
        });
    if let Some(agent) = common_agent {
        out.push_str(&styled(
            &format!("by {}\n", md_single_line(agent)),
            "2",
            color,
        ));
    }
    let active_count = events
        .iter()
        .take_while(|event| !event.status.is_terminal())
        .count();
    let (active, reviewed) = events.split_at(active_count);
    for (section, entries) in [("Needs attention", active), ("Reviewed", reviewed)] {
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n{}\n",
            styled(&format!("{section} ({})", entries.len()), "1", color)
        ));
        for event in entries {
            append_event(
                &mut out,
                event,
                scope,
                &all_ids,
                common_agent.is_some(),
                color,
            );
        }
    }
    append_skipped(&mut out, skipped);
    out.trim_end().to_string()
}

fn append_event(
    out: &mut String,
    event: &crate::model::Event,
    scope: &RepoScope,
    all_ids: &[String],
    common_agent: bool,
    color: bool,
) {
    let (mark, color_code) = match event.status {
        Status::Open => ("●", "33"),
        Status::Candidate => ("◇", "36"),
        Status::Fixed => ("✓", "32"),
        Status::Promoted => ("↗", "34"),
        Status::Duplicate => ("≈", "2"),
        Status::Dismissed => ("×", "2"),
    };
    let status = format!("{mark} {}", event.status.label().to_ascii_uppercase());
    let reference = crate::id::short_ref(&event.id, all_ids);
    let mut metadata = format!(
        "{}  {}",
        md_single_line(&reference),
        display_date(&event.created_at)
    );
    if !common_agent {
        if let Some(agent) = &event.context.agent {
            metadata.push_str(&format!("  ·  {}", md_single_line(agent)));
        }
    }
    out.push_str(&format!(
        "\n{}  {}\n",
        styled(&status, color_code, color),
        styled(&metadata, "2", color)
    ));
    append_wrapped(out, "  ", &event.summary);
    if let Some(product) = event.attributed_product() {
        out.push_str(&format!("  Product: {}\n", md_single_line(&product.id)));
        if let Some(version) = &product.version {
            out.push_str(&format!("  Version: {}\n", md_single_line(version)));
        }
        if let Some(surface) = &product.surface {
            out.push_str(&format!("  Surface: {}\n", md_single_line(surface)));
        }
    } else {
        out.push_str("  Product: (unattributed legacy)\n");
    }
    if let Some(hypothesis) = &event.hypothesis {
        append_wrapped(out, "  ? Hypothesis: ", hypothesis);
    }
    if let Some(fix) = &event.suggested_fix {
        append_wrapped(out, "  → Suggested fix: ", fix);
    }
    if event.status.is_terminal() {
        if let Some(resolution) = &event.resolution {
            append_wrapped(out, "  ↳ Resolution: ", &resolution.reason);
        }
    }
    if let RepoScope::All = scope {
        out.push_str(&format!(
            "  Repo: {}\n",
            md_single_line(event.context.repo.as_deref().unwrap_or("(unknown)"))
        ));
    }
    if let Some(category) = &event.category {
        out.push_str(&format!("  Category: {}\n", md_single_line(category)));
    }
    if let Some(task) = &event.context.task {
        out.push_str(&format!("  Task: {}\n", md_single_line(task)));
    }
}

fn display_date(created_at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(date) = created_at.get(..10) else {
        return md_single_line(created_at);
    };
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return md_single_line(created_at);
    };
    let (Ok(month), Ok(day)) = (month.parse::<usize>(), day.parse::<u8>()) else {
        return md_single_line(created_at);
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return md_single_line(created_at);
    }
    format!("{day} {} {year}", MONTHS[month - 1])
}

fn should_color(terminal: bool, term: Option<&str>, no_color: bool) -> bool {
    terminal && !term.is_some_and(|value| value.eq_ignore_ascii_case("dumb")) && !no_color
}

fn styled(value: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value.to_string()
    }
}

fn status_order(status: Status) -> u8 {
    match status {
        Status::Open => 0,
        Status::Candidate => 1,
        Status::Fixed => 2,
        Status::Promoted => 3,
        Status::Duplicate => 4,
        Status::Dismissed => 5,
    }
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
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "({} unreadable event file(s) skipped)\n",
        skipped.len()
    ));
    for s in skipped.iter().take(SKIPPED_LIST_MAX) {
        out.push_str(&format!(
            "  - {}: {}\n",
            md_single_line(&s.file_label()),
            md_single_line(&truncate(&s.reason, SKIPPED_REASON_MAX))
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
        assert_eq!(
            render_text(&[], &[], &RepoScope::All, false),
            "No papercuts found."
        );
        let scoped = render_text(
            &[],
            &[],
            &RepoScope::One("github.com/foo/bar".into()),
            false,
        );
        assert!(scoped.contains("No papercuts for github.com/foo/bar"));
        assert!(scoped.contains("papercut list --repo all"));
        let skipped = vec![SkippedFile {
            file: "pc_01KGARBAGE0000000000000Z.json".into(),
            reason: "parse error:EOF".into(),
            repo: None,
            product: None,
        }];
        let txt = render_text(&[], &skipped, &RepoScope::All, false);
        assert!(txt.contains("1 unreadable event file(s) skipped"));
        assert!(txt.contains("pc_01KGARBAGE0000000000000Z.json: parse error:EOF"));
    }

    #[test]
    fn scoped_report_prioritizes_full_observation() {
        let e = crate::model::Event {
            schema_version: 1,
            id: "pc_01K000000000000000000000A".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            product: None,
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
        let txt = render_text(
            std::slice::from_ref(&e),
            &[],
            &RepoScope::One("github.com/foo/bar".into()),
            false,
        );
        assert!(txt.contains("● OPEN  0000000A  4 Aug 2026"));
        assert!(txt.contains("Papercuts · bar"));
        assert!(txt.find("● OPEN") < txt.find("glob ate my args"));
    }

    /// A multiline summary must remain prose inside its record.
    #[test]
    fn multiline_summary_stays_in_one_record() {
        let e = crate::model::Event {
            schema_version: 1,
            id: "pc_01K000000000000000000000B".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            product: None,
            summary: "first line\nsecond line".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext::default(),
            resolution: None,
        };
        let txt = render_text(std::slice::from_ref(&e), &[], &RepoScope::All, false);
        assert!(txt.contains("first line second line"));
        assert_eq!(txt.matches("● OPEN").count(), 1);
    }

    /// A skipped-file reason containing embedded markdown (a newline + heading,
    /// a forged bullet) must collapse onto a single bullet line instead of
    /// breaking out of the skipped listing into forged structure. Guards the
    /// md_single_line neutralization shared by `list`, `render`, and `triage-pack`.
    #[test]
    fn skipped_reason_with_markdown_collapses_to_one_line() {
        let skipped = vec![SkippedFile {
            file: "pc_01KGARBAGE0000000000000Z.json".into(),
            reason: "parse error\n## forged heading\n- forged bullet".into(),
            repo: None,
            product: None,
        }];
        let txt = render_text(&[], &skipped, &RepoScope::All, false);
        let bullets: Vec<&str> = txt.lines().filter(|l| l.starts_with("  - pc_")).collect();
        assert_eq!(bullets.len(), 1, "one collapsed skipped bullet: {txt}");
        assert!(
            !txt.lines().any(|l| l.trim_start().starts_with("## ")),
            "no forged heading leaked onto its own line: {txt}"
        );
        assert!(
            txt.contains("parse error") && txt.contains("forged"),
            "reason text still present, just collapsed: {txt}"
        );
    }

    /// An event whose `id` carries an embedded newline must not forge a heading.
    #[test]
    fn id_with_embedded_newline_cannot_forge_heading() {
        let e = crate::model::Event {
            schema_version: 1,
            id: "pc_evil\n## forged\n- bullet".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            product: None,
            summary: "real".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext::default(),
            resolution: None,
        };
        let txt = render_text(std::slice::from_ref(&e), &[], &RepoScope::All, false);
        assert!(
            !txt.lines().any(|l| l.trim_start().starts_with("## ")),
            "no forged heading from the id: {txt}"
        );
    }

    #[test]
    fn full_summary_and_separate_optional_fields() {
        let mut e = crate::model::Event {
            schema_version: 1,
            id: "pc_01K000000000000000000000C".into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            product: None,
            summary: "word ".repeat(50),
            hypothesis: Some("It might be a stale cache".into()),
            suggested_fix: Some("Clear the cache".into()),
            category: None,
            context: EventContext::default(),
            resolution: None,
        };
        let txt = render_text(std::slice::from_ref(&e), &[], &RepoScope::All, false);
        assert_eq!(txt.matches("word").count(), 50);
        assert!(txt.contains("Hypothesis: It might be a stale cache"));
        assert!(txt.contains("Suggested fix: Clear the cache"));
        e.context.repo = Some("github.com/foo/bar".into());
        let global = render_text(&[e], &[], &RepoScope::All, false);
        assert!(global.contains("Repo: github.com/foo/bar"));
    }

    #[test]
    fn color_is_tty_only_and_respects_no_color() {
        assert!(should_color(true, Some("xterm-256color"), false));
        assert!(!should_color(false, Some("xterm-256color"), false));
        assert!(!should_color(true, Some("dumb"), false));
        assert!(!should_color(true, Some("xterm"), true));
        assert_eq!(styled("● OPEN", "33", true), "\x1b[33m● OPEN\x1b[0m");
    }
}
