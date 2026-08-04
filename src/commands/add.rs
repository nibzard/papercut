//! `papercut add` — record one event.

use crate::app::RunResult;
use crate::cli::AddArgs;
use crate::detect::detect;
use crate::git_meta;
use crate::id::new_id;
use crate::model::{Event, EventContext, Source, Status, SCHEMA_VERSION};
use crate::output::ErrorItem;
use crate::store::{ensure_store, write_event};
use crate::time::now_rfc3339;
use serde_json::json;

pub fn run(args: AddArgs) -> RunResult {
    let summary = args.message.trim();
    if summary.is_empty() {
        return RunResult::Err(ErrorItem::new(
            "empty_message",
            "message is empty",
            false,
            "papercut add \"<what you were doing, what got in the way>\"",
        ));
    }

    if let Err(e) = ensure_store() {
        return RunResult::Err(ErrorItem::new(
            "store_unwritable",
            format!("cannot create store: {e}"),
            true,
            "check $XDG_DATA_HOME / $HOME / disk space",
        ));
    }

    let info = detect();
    let agent = args.agent.clone().unwrap_or(info.agent);
    let gm = git_meta::gather(&std::env::current_dir().unwrap_or_default());

    let event = Event {
        schema_version: SCHEMA_VERSION,
        id: new_id(),
        created_at: now_rfc3339(),
        source: Source::InMoment,
        status: Status::Open,
        summary: summary.to_string(),
        hypothesis: args.hypothesis.clone().filter(|s| !s.is_empty()),
        suggested_fix: args.fix.clone().filter(|s| !s.is_empty()),
        category: args.category.clone().filter(|s| !s.is_empty()),
        context: EventContext {
            repo: gm.repo,
            cwd: gm.cwd,
            git_sha: gm.git_sha,
            agent: Some(agent),
            session: info.session,
            task: args.task.clone().filter(|s| !s.is_empty()),
        },
        resolution: None,
    };

    if let Err(e) = event.validate() {
        return RunResult::Err(ErrorItem::new(
            "invalid_event",
            e,
            false,
            "unexpected for status=open; this is a bug",
        ));
    }

    let id = event.id.clone();
    match write_event(&event) {
        Ok(_) => RunResult::Ok {
            data: json!({ "id": id, "recorded": true }),
            text: id,
        },
        Err(e) => RunResult::Err(ErrorItem::new(
            "write_failed",
            format!("report NOT recorded: {e}"),
            true,
            "re-run papercut add; the store may be unwritable",
        )),
    }
}
