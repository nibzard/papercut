//! `papercut render` — deterministic markdown projection.

use crate::app::RunResult;
use crate::cli::RenderArgs;
use crate::output::ErrorItem;
use crate::paths::{data_root, resolve_repo_filter, RepoScope};
use crate::projection::render_markdown;
use crate::query::{current_repo, Filters};
use serde_json::json;

pub fn run(args: RenderArgs) -> RunResult {
    let cur = current_repo();
    let scope = resolve_repo_filter(Some(args.repo.as_str()), cur.as_deref());
    let filters = Filters {
        scope: scope.clone(),
        ..Default::default()
    };
    let (events, _skipped) = crate::query::load(&filters);
    let md = render_markdown(&scope, &events);

    let mut wrote: Option<String> = None;
    if args.write {
        let path = match &scope {
            RepoScope::One(_) => std::env::current_dir()
                .unwrap_or_default()
                .join("PAPERCUTS.md"),
            RepoScope::All => match data_root() {
                Some(r) => r.join("PAPERCUTS.md"),
                None => {
                    return RunResult::Err(ErrorItem::new(
                        "write_failed",
                        "cannot write global projection: no home data dir",
                        true,
                        "run inside a repo (--repo .), or set $HOME / $XDG_DATA_HOME",
                    ))
                }
            },
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return RunResult::Err(ErrorItem::new(
                    "write_failed",
                    format!("cannot create {}: {e}", parent.display()),
                    true,
                    "check directory permissions",
                ));
            }
        }
        if let Err(e) = std::fs::write(&path, &md) {
            return RunResult::Err(ErrorItem::new(
                "write_failed",
                format!("cannot write {}: {e}", path.display()),
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
            "write": wrote,
        }),
        text: md,
    }
}
