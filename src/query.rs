//! Shared event loading + filtering for `list`, `render`, and `triage-pack`.

use crate::model::{Event, Status};
use crate::paths::RepoScope;
use crate::store::{read_all_events, SkippedFile};
use crate::time::{now_unix, parse_rfc3339_unix};

/// The repo the CLI is currently running inside, if any.
pub fn current_repo() -> Option<String> {
    let cur = std::env::current_dir().ok()?;
    crate::git_meta::repo_of(&cur)
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

    // Normalize the repo filter once: stored repos are already normalized, so a
    // user passing `--repo https://github.com/foo/bar.git` must match events
    // stored as `github.com/foo/bar`. Local-path repos (no remote) are stored
    // as the toplevel path and matched literally.
    let norm_filter = match &f.scope {
        RepoScope::One(r) => Some(crate::git_meta::normalize_remote(r)),
        RepoScope::All => None,
    };

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
            RepoScope::One(r) => e
                .context
                .repo
                .as_deref()
                .is_some_and(|repo| repo_matches(repo, r, norm_filter.as_deref())),
        }
    });

    events.sort_by(|a, b| a.id.cmp(&b.id));

    // Quarantined files are a store-global concern, but a repo-scoped view must
    // not surface them indiscriminately: `render --write --repo A` commits the
    // projection into A's PAPERCUTS.md, and dumping every repo's corrupt files
    // there misattributes store-internal data into a published tree. Attribute
    // where the event parsed (SkippedFile.repo); under `One(r)` keep only skips
    // attributable to `r` and drop the unknowable ones (parse/read errors),
    // which still appear under the global `All` view.
    let scoped = match &f.scope {
        RepoScope::All => skipped,
        RepoScope::One(r) => skipped
            .into_iter()
            .filter(|s| {
                s.repo
                    .as_deref()
                    .is_some_and(|repo| repo_matches(repo, r, norm_filter.as_deref()))
            })
            .collect(),
    };
    (events, scoped)
}

/// Does a stored `repo` id match a `--repo` filter? A literal match covers
/// local-path repos (stored as the toplevel path); a normalized match covers
/// remote URLs the user may have pasted verbatim (`https://…/foo/bar.git`).
fn repo_matches(repo: &str, filter: &str, norm_filter: Option<&str>) -> bool {
    repo == filter || norm_filter.is_some_and(|n| repo == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_filter_matches_normalized_remote_url() {
        // Stored normalized id matches a full URL the user pasted.
        let norm = crate::git_meta::normalize_remote("https://github.com/foo/bar.git");
        assert!(repo_matches(
            "github.com/foo/bar",
            "https://github.com/foo/bar.git",
            Some(&norm)
        ));
        // And the already-normalized form.
        assert!(repo_matches(
            "github.com/foo/bar",
            "github.com/foo/bar",
            Some(&norm)
        ));
    }

    #[test]
    fn repo_filter_matches_local_path_literal() {
        // A local repo (no remote) is stored as its path; the user passes the
        // same path, matched literally.
        let path = "/abs/path/repo";
        let norm = crate::git_meta::normalize_remote(path);
        assert!(repo_matches(path, path, Some(&norm)));
    }

    #[test]
    fn repo_filter_rejects_unrelated() {
        // norm is always normalize_remote(FILTER), as load() computes it.
        let norm = crate::git_meta::normalize_remote("github.com/other/repo");
        assert!(!repo_matches(
            "github.com/foo/bar",
            "github.com/other/repo",
            Some(&norm)
        ));
    }
}
