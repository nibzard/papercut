//! `papercut add` — record one event.

use crate::app::RunResult;
use crate::cli::AddArgs;
use crate::detect::detect;
use crate::git_meta;
use crate::id::new_id;
use crate::model::{Event, EventContext, Product, Source, Status, SCHEMA_VERSION};
use crate::output::ErrorItem;
use crate::store::{ensure_store, write_event};
use crate::time::now_rfc3339;
use serde_json::json;

pub fn run(args: AddArgs) -> RunResult {
    let summary = args.message.trim();
    if summary.is_empty() {
        return RunResult::err(ErrorItem::new(
            "empty_message",
            "message is empty",
            false,
            "papercut add --product <id> -- \"<what you were doing, what got in the way>\"",
        ));
    }

    let product_id = args.product.trim();
    if product_id.is_empty() {
        return RunResult::usage(ErrorItem::new(
            "missing_product",
            "a designated product ID is required",
            false,
            "use --product <id> from the task's product designation",
        ));
    }
    for (name, value) in [
        ("product-version", &args.product_version),
        ("surface", &args.surface),
    ] {
        if value.as_ref().is_some_and(|v| v.trim().is_empty()) {
            return RunResult::usage(ErrorItem::new(
                "blank_product_field",
                format!("--{name} cannot be blank"),
                false,
                format!("provide a value for --{name} or omit it"),
            ));
        }
    }

    if let Err(e) = ensure_store() {
        return RunResult::err(ErrorItem::new(
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
        product: Some(Product {
            id: product_id.to_string(),
            version: args.product_version.as_ref().map(|v| v.trim().to_string()),
            surface: args.surface.as_ref().map(|v| v.trim().to_string()),
        }),
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
        return RunResult::err(ErrorItem::new(
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
        Err(e) => RunResult::err(ErrorItem::new(
            "write_failed",
            format!("report NOT recorded: {e}"),
            true,
            "re-run papercut add; the store may be unwritable",
        )),
    }
}
