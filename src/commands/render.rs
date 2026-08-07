//! `papercut render` — deterministic markdown projection.

use crate::app::RunResult;
use crate::cli::RenderArgs;
use crate::output::ErrorItem;
use crate::paths::{data_root, resolve_repo_filter, RepoScope};
use crate::projection::render_markdown;
use crate::query::{current_repo, Filters};
use crate::util::{md_code_span, md_single_line, truncate, SKIPPED_LIST_MAX, SKIPPED_REASON_MAX};
use serde_json::json;
use std::path::PathBuf;

pub fn run(args: RenderArgs) -> RunResult {
    let cur = current_repo();
    let scope = resolve_repo_filter(Some(args.repo.as_str()), cur.as_deref());
    let filters = Filters {
        scope: scope.clone(),
        ..Default::default()
    };
    let (events, skipped) = crate::query::load(&filters);
    let mut md = render_markdown(&scope, &events);
    // F8: surface quarantined event files inline with their reasons, so a human
    // reading the projection knows WHICH file vanished and WHY (schema /
    // invariant / corruption) rather than seeing only an absent section. The
    // skipped list is sorted by file name (store.rs), so this stays
    // byte-identical for a given store. We do NOT point at `doctor` here: doctor
    // checks store writability, managed blocks, and hook wiring — it never reads
    // events and cannot explain a quarantine.
    if !skipped.is_empty() {
        md.push_str(&format!(
            "\n> _{} event file(s) skipped (unreadable or invalid):_\n",
            skipped.len()
        ));
        for s in skipped.iter().take(SKIPPED_LIST_MAX) {
            md.push_str(&format!(
                "> - {}: {}\n",
                md_code_span(&md_single_line(&s.file_label())),
                md_single_line(&truncate(&s.reason, SKIPPED_REASON_MAX))
            ));
        }
        if skipped.len() > SKIPPED_LIST_MAX {
            md.push_str(&format!(
                "> - … {} more not shown\n",
                skipped.len() - SKIPPED_LIST_MAX
            ));
        }
    }

    let mut wrote: Option<String> = None;
    if args.write {
        let path = match &scope {
            // C3: write to the working-tree root, not whichever subdir we were
            // invoked from — but only when the current repo actually matches the
            // scope. A mismatch is refused (exit 1) rather than silently dropping
            // repo A's projection into repo B's tree or an arbitrary cwd.
            RepoScope::One(want) => match write_target_for_repo(want) {
                Ok(p) => p,
                Err(item) => return RunResult::err(item),
            },
            RepoScope::All => match data_root() {
                Some(r) => r.join("PAPERCUTS.md"),
                None => {
                    return RunResult::err(ErrorItem::new(
                        "write_failed",
                        "cannot write global projection: no home data dir",
                        true,
                        "run inside a repo (--repo .), or set $HOME / $XDG_DATA_HOME",
                    ))
                }
            },
        };
        if let Some(parent) = path.parent() {
            // 0700 like the rest of the private store: for the global scope the
            // parent IS data_root, so a first-ever `render --write` must create
            // it owner-only, not at the umask default. For a repo-scoped write
            // the parent is the existing repo root, which create_private_dir
            // leaves untouched (existing dirs are never re-chmodded).
            if let Err(e) = crate::store::create_private_dir(parent) {
                return RunResult::err(ErrorItem::new(
                    "write_failed",
                    format!("cannot create {}: {e}", parent.display()),
                    true,
                    "check directory permissions",
                ));
            }
        }
        if let Err(e) = crate::store::write_atomic(&path, md.as_bytes()) {
            return RunResult::err(ErrorItem::new(
                "write_failed",
                format!("cannot write {}: {e:#}", path.display()),
                true,
                "check file permissions",
            ));
        }
        wrote = Some(path.to_string_lossy().into_owned());
    }

    RunResult::Ok {
        data: json!({
            "bytes": md.len(),
            "events": events.len(),
            "skipped": skipped,
            "write": wrote,
        }),
        text: md,
    }
}

/// Where to write a repo-scoped projection: the working-tree root when the
/// current repo matches `want`. A mismatch returns `Err` so the caller exits 1
/// instead of writing repo `want`'s projection into an unrelated tree.
fn write_target_for_repo(want: &str) -> Result<PathBuf, ErrorItem> {
    let cur = std::env::current_dir().unwrap_or_default();
    let meta = crate::git_meta::gather(&cur);
    match (&meta.repo, &meta.toplevel) {
        (Some(r), Some(top)) if r == want => Ok(top.clone().join("PAPERCUTS.md")),
        _ => {
            // The hint must stay accurate whether or not there is a current
            // repo. "Drop --repo to scope to the current repo" only holds when
            // one exists — outside any repo, dropping --repo resolves to the
            // global projection (written to the private store), not a repo.
            let suffix = meta
                .repo
                .as_deref()
                .map(|r| format!(" (currently inside {r})"))
                .unwrap_or_else(|| " (currently outside any repo)".to_string());
            let hint = if meta.repo.is_some() {
                "run from inside the target repo, drop --repo to scope to the current repo, or omit --write"
            } else {
                "run from inside the target repo, or omit --write (dropping --repo here selects the global projection, not a repo)"
            };
            Err(ErrorItem::new(
                "write_refused",
                format!("render --write --repo {want} must run inside repo {want}{suffix}"),
                true,
                hint,
            ))
        }
    }
}
