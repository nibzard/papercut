//! Claude Code signal adapter.
//!
//! Verified live (2026-08-04): Claude Code's `PostToolUse` hook receives a JSON
//! payload on stdin with `tool_name`, `tool_input.command`, `tool_result.exit_code`,
//! `tool_result.output`, `session_id`, and `cwd`. Hook failures are non-blocking
//! by design, which already serves the "never fail the parent task" invariant;
//! we additionally swallow every error inside the hook.
//!
//! The settings.json wiring is the JSON analog of a managed block: we touch
//! only `hooks.PostToolUse` entries whose `command` is ours, preserving every
//! other key the user has.

use crate::paths::home_dir;
use anyhow::Context;
use serde_json::{json, Value};
use std::path::PathBuf;

/// `~/.claude/settings.json`.
pub fn settings_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".claude").join("settings.json"))
}

fn read_settings() -> Value {
    let Some(path) = settings_path() else {
        return json!({});
    };
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    }
}

fn write_settings(v: &Value) -> anyhow::Result<()> {
    let path = settings_path().context("no HOME; cannot locate claude settings")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create ~/.claude")?;
    }
    let bytes = serde_json::to_vec_pretty(v).context("serialize settings")?;
    std::fs::write(&path, bytes).context("write settings.json")?;
    Ok(())
}

/// Recognize the exact command shape we emit (`<exe> _hook claude-code`).
/// Matching the agent suffix avoids classifying an unrelated user hook that
/// merely happens to contain the substrings "papercut" and "_hook".
fn is_our_hook(cmd: &str) -> bool {
    cmd.contains("_hook claude-code")
}

/// Idempotently install the PostToolUse Bash hook. Returns true if present.
pub fn install_hook(exe: &str) -> anyhow::Result<bool> {
    let path = settings_path().context("no HOME; cannot locate claude settings")?;
    let existing = std::fs::read(&path).ok();
    let mut settings = read_settings();
    let obj = settings
        .as_object_mut()
        .context("settings.json root is not an object")?;
    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .context("settings.hooks is not an object")?;
    if !hooks.contains_key("PostToolUse") {
        hooks.insert("PostToolUse".into(), json!([]));
    }
    let ptu = hooks
        .get_mut("PostToolUse")
        .and_then(|a| a.as_array_mut())
        .context("hooks.PostToolUse is not an array")?;

    // Remove our existing hook entries from every group (idempotent).
    for group in ptu.iter_mut() {
        if let Some(arr) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            arr.retain(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_none_or(|c| !is_our_hook(c))
            });
        }
    }
    // Drop groups left empty by that removal that contained only our hook.
    ptu.retain(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .is_none_or(|a| !a.is_empty())
    });

    let our_entry = json!({ "type": "command", "command": format!("{exe} _hook claude-code") });
    let bash_idx = ptu
        .iter()
        .position(|g| g.get("matcher").and_then(|m| m.as_str()) == Some("Bash"));
    match bash_idx {
        Some(i) => ptu[i]
            .get_mut("hooks")
            .and_then(|h| h.as_array_mut())
            .context("bash group hooks not an array")?
            .push(our_entry),
        None => ptu.push(json!({ "matcher": "Bash", "hooks": [our_entry] })),
    }

    // Skip the write when the document is already byte-identical, so repeated
    // installs are a true no-op. (serde_json sorts object keys; enabling its
    // `preserve_order` feature would retain the user's order but adds a
    // dependency, which needs sign-off — so the first install on a hand-edited
    // file reorders keys once, and never again.)
    let new_bytes = serde_json::to_vec_pretty(&settings).context("serialize settings")?;
    if existing.as_deref() != Some(new_bytes.as_slice()) {
        write_settings(&settings)?;
    }
    Ok(true)
}

/// Remove our hook entries. Returns the count removed. Idempotent.
pub fn uninstall_hook() -> anyhow::Result<usize> {
    let mut settings = read_settings();
    let mut removed = 0usize;
    let Some(hooks) = settings
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
    else {
        return Ok(0);
    };
    if let Some(ptu) = hooks.get_mut("PostToolUse").and_then(|a| a.as_array_mut()) {
        for group in ptu.iter_mut() {
            if let Some(arr) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = arr.len();
                arr.retain(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_none_or(|c| !is_our_hook(c))
                });
                removed += before - arr.len();
            }
        }
        ptu.retain(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|a| !a.is_empty())
        });
        if ptu.is_empty() {
            hooks.remove("PostToolUse");
        }
    }
    if hooks.is_empty() {
        settings
            .as_object_mut()
            .expect("settings object")
            .remove("hooks");
    }
    // Nothing was removed → the document is unchanged; don't rewrite it.
    if removed > 0 {
        write_settings(&settings)?;
    }
    Ok(removed)
}

/// Is our PostToolUse Bash hook present in settings.json?
pub fn hook_wired() -> bool {
    let settings = read_settings();
    settings
        .pointer("/hooks/PostToolUse")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|g| {
                g.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                    hs.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .is_some_and(is_our_hook)
                    })
                })
            })
        })
}
