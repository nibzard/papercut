//! `papercut triage-pack` — a token-budget-aware markdown bundle for triage.

use std::collections::{BTreeMap, BTreeSet};

use crate::app::RunResult;
use crate::cli::TriagePackArgs;
use crate::model::{Event, Status};
use crate::paths::{resolve_repo_filter, RepoScope};
use crate::query::{current_repo, Filters};
use crate::signal::read_signals;
use crate::util::truncate;
use serde_json::json;

/// Harness signal dirs we know how to read.
const SIGNAL_HARNESSES: &[&str] = &["codex", "claude-code"];

pub fn run(args: TriagePackArgs) -> RunResult {
    let cur = current_repo();
    let scope = resolve_repo_filter(Some(args.repo.as_str()), cur.as_deref());

    let (mut events, _skipped) = crate::query::load(&Filters {
        scope: scope.clone(),
        ..Default::default()
    });
    // Default triage focus: open + candidate. An explicit --status overrides.
    match args.status {
        Some(s) => events.retain(|e| e.status == s),
        None => events.retain(|e| matches!(e.status, Status::Open | Status::Candidate)),
    }

    let clusters = cluster_signals();
    let md = build_pack(&scope, &events, &clusters, args.max_tokens);

    RunResult::Ok {
        data: json!({
            "bytes": md.len(),
            "events": events.len(),
            "signal_clusters": clusters.len(),
        }),
        text: md,
    }
}

struct Cluster {
    key: String,
    sample: String,
    count: usize,
    exits: Vec<i32>,
    repos: BTreeSet<String>,
    agents: BTreeSet<String>,
}

fn cluster_signals() -> Vec<Cluster> {
    let mut map: BTreeMap<String, Cluster> = BTreeMap::new();
    for h in SIGNAL_HARNESSES {
        let (sigs, _skip) = read_signals(h);
        for s in sigs {
            let key = normalize_key(&s.cmd);
            let c = map.entry(key.clone()).or_insert_with(|| Cluster {
                key: key.clone(),
                sample: s.cmd.clone(),
                count: 0,
                exits: Vec::new(),
                repos: BTreeSet::new(),
                agents: BTreeSet::new(),
            });
            c.count += 1;
            c.exits.push(s.exit);
            if let Some(r) = s.repo {
                c.repos.insert(r);
            }
            if let Some(a) = s.agent {
                c.agents.insert(a);
            }
        }
    }
    let mut v: Vec<Cluster> = map.into_values().collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
    v
}

/// Group failures by their program (first token, basename if path-like).
fn normalize_key(cmd: &str) -> String {
    let tok = cmd.split_whitespace().next().unwrap_or("");
    let base = tok.rsplit('/').next().unwrap_or(tok);
    base.to_lowercase()
}

fn build_pack(
    scope: &RepoScope,
    events: &[Event],
    clusters: &[Cluster],
    max_tokens: u32,
) -> String {
    let budget = (max_tokens as usize).saturating_mul(4);
    let mut out = String::new();
    out.push_str("# Papercut triage pack\n\n");
    match scope {
        RepoScope::One(r) => out.push_str(&format!("repo: `{r}`\n\n")),
        RepoScope::All => out.push_str("scope: global (all repos)\n\n"),
    }
    out.push_str(&format!(
        "source: {} event(s), {} signal cluster(s)\n\n",
        events.len(),
        clusters.len()
    ));

    // Events.
    out.push_str("## Events\n\n");
    if events.is_empty() {
        out.push_str("_none_\n\n");
    } else {
        for e in events {
            out.push_str(&format!(
                "- **{}** [{}] {}\n",
                e.id,
                e.status.label(),
                truncate(&e.summary, 160)
            ));
        }
        out.push('\n');
    }

    // Signal clusters, token-budget-aware.
    out.push_str("## Signal clusters (recurring failures)\n\n");
    if clusters.is_empty() {
        out.push_str("_none — no automatic signals recorded_\n");
        return out;
    }
    let mut omitted = 0usize;
    for (i, c) in clusters.iter().enumerate() {
        let line = format_cluster(c);
        if out.len() + line.len() > budget {
            omitted = clusters.len() - i;
            break;
        }
        out.push_str(&line);
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n... {omitted} more cluster(s) omitted (token budget; raise --max-tokens)\n"
        ));
    }

    out
}

fn format_cluster(c: &Cluster) -> String {
    let repos = if c.repos.is_empty() {
        "?".to_string()
    } else {
        c.repos.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    let agents = if c.agents.is_empty() {
        "?".to_string()
    } else {
        c.agents.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    format!(
        "- **{count}×** `{sample}` — agents: {agents}; repos: {repos}\n",
        count = c.count,
        sample = truncate(&c.sample, 80),
        agents = truncate(&agents, 60),
        repos = truncate(&repos, 60),
    )
}
