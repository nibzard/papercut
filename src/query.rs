//! Shared event loading + filtering for `list`, `render`, and `triage-pack`.

use crate::model::{Event, Status};
use crate::paths::RepoScope;
use crate::store::{read_all_events, SkippedFile};
use crate::time::{now_unix, parse_rfc3339_unix};

/// The repo the CLI is currently running inside, if any.
pub fn current_repo() -> Option<String> {
    let cur = std::env::current_dir().ok()?;
    crate::git_meta::gather(&cur).repo
}

pub struct Filters {
    pub scope: RepoScope,
    pub status: Option<Status>,
    pub agent: Option<String>,
    pub since_days: Option<u32>,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            scope: RepoScope::All,
            status: None,
            agent: None,
            since_days: None,
        }
    }
}

/// Load events matching `filters`, sorted by id (≈ chronological).
pub fn load(f: &Filters) -> (Vec<Event>, Vec<SkippedFile>) {
    let (mut events, skipped) = read_all_events();
    let cutoff = f
        .since_days
        .map(|d| now_unix().saturating_sub(d as u64 * 86_400));

    events.retain(|e| {
        if let Some(want) = f.status {
            if e.status != want {
                return false;
            }
        }
        if let Some(a) = &f.agent {
            if e.context.agent.as_deref() != Some(a.as_str()) {
                return false;
            }
        }
        if let Some(c) = cutoff {
            // Drop parseable timestamps older than the cutoff; keep unparseable
            // ones (never hide data we can't reason about).
            if let Some(t) = parse_rfc3339_unix(&e.created_at) {
                if t < c {
                    return false;
                }
            }
        }
        match &f.scope {
            RepoScope::All => true,
            RepoScope::One(r) => e.context.repo.as_deref() == Some(r.as_str()),
        }
    });

    events.sort_by(|a, b| a.id.cmp(&b.id));
    (events, skipped)
}
