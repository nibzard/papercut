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
use std::path::{Path, PathBuf};

/// `~/.claude/settings.json`.
pub fn settings_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// Strip a single leading UTF-8 BOM (`EF BB BF`) if present. Some editors write
/// `settings.json` BOM-prefixed; `serde_json` does not skip it, so a BOM-only
/// file is "blank-but-not-ascii-whitespace" and a BOM-prefixed `{}` fails to
/// parse. Stripping the BOM first keeps the blank/absent/corrupt classification
/// — and the lenient read path — all in agreement.
fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(b)
}

fn read_settings() -> Value {
    let Some(path) = settings_path() else {
        return json!({});
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(strip_bom(&bytes)).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    }
}

/// Read & parse settings.json strictly for a *mutating* operation: `Ok({})`
/// when the file is absent OR blank (nothing to preserve), `Err` when it exists
/// but cannot be read or holds unparseable JSON. A mutation must never overwrite
/// a file it could not parse — that would silently destroy the user's (possibly
/// hand-edited) content, the JSON analog of the managed-block invariant. The
/// lenient [`read_settings`] stays for read-only [`hook_status`], which must
/// never fail.
///
/// A 0-byte or whitespace-only file is semantically "no settings" (same as
/// absent): it carries no content to destroy, so `from_slice(b"")` failing with
/// "EOF while parsing a value" must NOT be treated as corruption that blocks the
/// mutation. Treating blank as `Ok({})` keeps this consistent with the lenient
/// read path.
fn load_settings_for_write() -> anyhow::Result<Value> {
    let path = settings_path().context("no HOME; cannot locate claude settings")?;
    match std::fs::read(&path) {
        Ok(raw) => {
            // A leading BOM is an editor artifact, not content; strip it before
            // the blank check and the parse so a BOM-only file reads as blank
            // and a BOM-prefixed `{}` parses instead of being misread as corrupt.
            let bytes = strip_bom(&raw);
            if bytes.iter().all(|b| b.is_ascii_whitespace()) {
                Ok(json!({}))
            } else {
                serde_json::from_slice(bytes).with_context(|| {
                    format!(
                        "{} exists but is not valid JSON; refusing to overwrite — back it up or repair it first",
                        path.display()
                    )
                })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
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
    // Original bytes for the byte-identical skip (None when absent). Read errors
    // other than NotFound propagate: we never clobber a file we could not read.
    let existing: Option<Vec<u8>> = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    // A blank (0-byte / whitespace-only) file parses to EOF, but it carries no
    // content to preserve — treat it like an absent file (`{}`), not corruption.
    // Strip a leading BOM first so a BOM-only file is blank and a BOM-prefixed
    // `{}` parses, consistent with load_settings_for_write / read_settings.
    let blank = existing
        .as_deref()
        .map(strip_bom)
        .is_some_and(|b| b.iter().all(|x| x.is_ascii_whitespace()));
    let mut settings = match (&existing, blank) {
        (None, _) | (Some(_), true) => json!({}),
        (Some(b), false) => serde_json::from_slice(strip_bom(b)).with_context(|| {
            format!(
                "{} exists but is not valid JSON; refusing to overwrite — back it up or repair it first",
                path.display()
            )
        })?,
    };
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

    // Remove our existing hook entries from every group (idempotent). We do NOT
    // prune empty groups here: install only adds, and deleting a user-owned
    // empty-placeholder group would violate "we touch only entries whose command
    // is ours". A group emptied by this dedup either gets our hook re-added (the
    // Bash group) or is a harmless empty no-op left in place.
    for group in ptu.iter_mut() {
        if let Some(arr) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            arr.retain(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_none_or(|c| !is_our_hook(c))
            });
        }
    }

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
    let mut settings = load_settings_for_write()?;
    let mut removed = 0usize;
    let Some(hooks) = settings
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
    else {
        return Ok(0);
    };
    if let Some(ptu) = hooks.get_mut("PostToolUse").and_then(|a| a.as_array_mut()) {
        // Drop only groups papercut emptied by removing its own hook. A group
        // that was already empty (a user placeholder) or that still holds other
        // hooks is left untouched — we touch only entries whose command is ours.
        let mut drop_idx: Vec<usize> = Vec::new();
        for (gi, group) in ptu.iter_mut().enumerate() {
            if let Some(arr) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = arr.len();
                arr.retain(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_none_or(|c| !is_our_hook(c))
                });
                let removed_here = before - arr.len();
                removed += removed_here;
                if removed_here > 0 && arr.is_empty() {
                    drop_idx.push(gi);
                }
            }
        }
        for gi in drop_idx.into_iter().rev() {
            ptu.remove(gi);
        }
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

/// Is our PostToolUse Bash hook entry present in settings.json? This checks
/// only that the command string is wired in — not that the executable it names
/// actually exists. Use this to assert settings.json manipulation (install /
/// uninstall idempotence); use [`hook_wired`] for the full health notion.
pub fn hook_present() -> bool {
    hook_status().present
}

/// Is our hook both present AND pointing at a runnable executable? This is the
/// full health notion `doctor` gates on — a present entry whose exe is missing
/// or non-executable is a dead hook and must read as unhealthy.
pub fn hook_wired() -> bool {
    let s = hook_status();
    s.present && s.exe_ok
}

/// Status of our hook: whether the entry exists in settings.json AND whether the
/// executable it points at actually resolves to a runnable file. A present entry
/// whose `exe` is missing or non-executable is a dead hook — `doctor` must not
/// call it healthy.
pub struct HookStatus {
    pub present: bool,
    pub exe_ok: bool,
    pub exe: Option<String>,
}

pub fn hook_status() -> HookStatus {
    let settings = read_settings();
    let cmd = settings
        .pointer("/hooks/PostToolUse")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .flat_map(|g| {
                    g.get("hooks")
                        .and_then(|h| h.as_array())
                        .into_iter()
                        .flatten()
                })
                .find_map(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .filter(|c| is_our_hook(c))
                })
        });
    match cmd {
        None => HookStatus {
            present: false,
            exe_ok: false,
            exe: None,
        },
        Some(c) => {
            let exe = first_token(c);
            let exe_ok = exe.as_deref().is_some_and(exe_resolves);
            HookStatus {
                present: true,
                exe_ok,
                exe,
            }
        }
    }
}

/// The executable token from a command string `<exe> _hook claude-code`,
/// stripping surrounding quotes. `None` if the command has no token.
fn first_token(cmd: &str) -> Option<String> {
    let tok = cmd.split_whitespace().next()?;
    let trimmed = tok.trim_matches(|c| c == '"' || c == '\'');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Does `exe` resolve to an existing, executable file? A path (absolute or
/// containing a separator) is checked directly; a bare name is searched on
/// `$PATH`. No `which` crate — boring-deps only.
fn exe_resolves(exe: &str) -> bool {
    let p = Path::new(exe);
    if p.is_absolute() || exe.contains(std::path::MAIN_SEPARATOR) || exe.contains('/') {
        return p.is_file() && is_executable(p);
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| {
        let candidate = dir.join(exe);
        candidate.is_file() && is_executable(&candidate)
    })
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}
