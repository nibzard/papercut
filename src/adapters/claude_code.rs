//! Claude Code signal adapter.
//!
//! Verified live (2026-08-06, Claude Code v2.1.223): a FAILED Bash command
//! fires the `PostToolUseFailure` hook with a JSON payload on stdin carrying
//! `tool_name`, `tool_input.command`, `session_id`, `cwd`, a top-level `error`
//! string (`"Exit code N\n<combined output>"`), and an `is_interrupt` bool.
//! `PostToolUse` fires only on SUCCESS, with a `tool_response` object
//! (`{stdout, stderr, interrupted, …}`) and no exit information — so only
//! `PostToolUseFailure` is wired; wiring `PostToolUse` would spawn the hook on
//! every successful command for zero signal. Hook failures are non-blocking by
//! design, which already serves the "never fail the parent task" invariant; we
//! additionally swallow every error inside the hook. Re-verify these shapes
//! against the installed harness on updates — never trust memory or docs.
//!
//! The settings.json wiring is the JSON analog of a managed block: we touch
//! only hook entries whose `command` is ours, preserving every other key the
//! user has.

use crate::paths::home_dir;
use anyhow::Context;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The hook event that carries Bash failures.
pub const HOOK_EVENT: &str = "PostToolUseFailure";
/// Event keys an older papercut install may hold our entry under. Removal
/// scans these too, so a rerun of `install` (or an `uninstall`) heals a
/// pre-PostToolUseFailure wiring instead of leaving a dead entry behind.
const LEGACY_HOOK_EVENTS: &[&str] = &["PostToolUse"];

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
    // Atomic (temp + rename): settings.json is the one file papercut does NOT
    // own — Claude Code reads it on every launch. A torn in-place write would
    // corrupt the user's entire harness config (every hook and permission), and
    // the strict load path above then refuses to overwrite the unparseable
    // result, so papercut could not self-heal its own torn write. Rename is
    // atomic on the same filesystem, matching every other persistence path
    // (events / config / sweeps / instructions / render --write).
    crate::store::write_atomic(&path, &bytes).context("write settings.json")?;
    Ok(())
}

/// Recognize the exact command shape we emit (`<exe> _hook claude-code`):
/// the command's trailing whitespace-split tokens are exactly `_hook` then
/// `claude-code`. Token-anchored so an unrelated user hook whose command
/// merely CONTAINS the substring (e.g. `other-tool _hook claude-code-audit`)
/// is never classified — and never deleted — as ours.
fn is_our_hook(cmd: &str) -> bool {
    let mut toks = cmd.split_whitespace().rev();
    toks.next() == Some("claude-code") && toks.next() == Some("_hook")
}

/// The hook command line for settings.json. The executable is ALWAYS
/// single-quoted, with any internal single quotes encoded via the shell `'\''`
/// idiom that [`first_token`] / [`decode_single_quoted`] reverse on read-back.
/// Quoting only on whitespace left an apostrophe-bearing path with NO space
/// (e.g. `/home/O'Brien/.local/bin/papercut`) unquoted — the shell then rejects
/// it as an unterminated quote, so the hook never fired while `doctor` parsed
/// the path back and reported it healthy. Always quoting closes that hole
/// regardless of which shell metacharacters the path contains; the trailing
/// `_hook claude-code` tokens still anchor [`is_our_hook`].
fn hook_command(exe: &str) -> String {
    format!("'{}' _hook claude-code", exe.replace('\'', r"'\''"))
}

/// Remove our hook entries under `event`. Drops only groups papercut emptied
/// by that removal; a group that was already empty (a user placeholder) or
/// that still holds other hooks is left untouched. Removes the `event` key
/// itself only when our removal emptied it. Returns the count removed.
fn remove_our_entries(hooks: &mut serde_json::Map<String, Value>, event: &str) -> usize {
    let mut removed = 0usize;
    if let Some(arr) = hooks.get_mut(event).and_then(|a| a.as_array_mut()) {
        let mut drop_idx: Vec<usize> = Vec::new();
        for (gi, group) in arr.iter_mut().enumerate() {
            if let Some(hs) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = hs.len();
                hs.retain(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_none_or(|c| !is_our_hook(c))
                });
                let removed_here = before - hs.len();
                removed += removed_here;
                if removed_here > 0 && hs.is_empty() {
                    drop_idx.push(gi);
                }
            }
        }
        for gi in drop_idx.into_iter().rev() {
            arr.remove(gi);
        }
        if arr.is_empty() && removed > 0 {
            hooks.remove(event);
        }
    }
    removed
}

/// Idempotently install the failure hook (see [`HOOK_EVENT`]). Returns true if
/// present. A rerun also strips our entry from legacy event keys, healing an
/// install made by an older papercut.
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

    // Remove our existing entries everywhere first (idempotent), including
    // legacy event keys from older installs.
    remove_our_entries(hooks, HOOK_EVENT);
    for ev in LEGACY_HOOK_EVENTS {
        remove_our_entries(hooks, ev);
    }

    if !hooks.contains_key(HOOK_EVENT) {
        hooks.insert(HOOK_EVENT.into(), json!([]));
    }
    let ptu = hooks
        .get_mut(HOOK_EVENT)
        .and_then(|a| a.as_array_mut())
        .with_context(|| format!("hooks.{HOOK_EVENT} is not an array"))?;

    let our_entry = json!({ "type": "command", "command": hook_command(exe) });
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

/// Remove our hook entries from the failure event and every legacy event key.
/// Returns the count removed. Idempotent.
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
    removed += remove_our_entries(hooks, HOOK_EVENT);
    for ev in LEGACY_HOOK_EVENTS {
        removed += remove_our_entries(hooks, ev);
    }
    // The `hooks` key itself is never deleted, even when empty: we cannot
    // know whether install created it or the user already had `"hooks": {}`,
    // and deleting a user's key would violate "touch only what is ours". An
    // empty leftover object is harmless.
    // Nothing was removed → the document is unchanged; don't rewrite it.
    if removed > 0 {
        write_settings(&settings)?;
    }
    Ok(removed)
}

/// Diagnose the settings.json file for `doctor`: `None` when the file is
/// absent, blank, or a valid JSON object; `Some(reason)` when it exists but
/// cannot be read, parsed, or is valid JSON of the wrong type (an array or
/// scalar). The lenient read path masks all of those as "no settings", which
/// would send doctor to a "run install" hint — but install refuses to overwrite
/// an unparseable or non-object file, so this gives doctor the honest diagnosis
/// instead and keeps the two commands from looping.
pub fn settings_diagnosis() -> Option<String> {
    match load_settings_for_write() {
        Err(e) => Some(format!("{e:#}")),
        Ok(v) if !v.is_object() => Some(format!(
            "~/.claude/settings.json is valid JSON but not an object ({}) — install refuses to overwrite it; back it up and repair it by hand",
            json_type_label(&v)
        )),
        Ok(_) => None,
    }
}

/// One-word JSON type label for the diagnosis message.
fn json_type_label(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Is our failure-hook entry present in settings.json (under [`HOOK_EVENT`])?
/// This checks only that the command string is wired in — not that the
/// executable it names actually exists. Use this to assert settings.json
/// manipulation (install / uninstall idempotence); use [`hook_wired`] for the
/// full health notion.
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
#[derive(Debug)]
pub struct HookStatus {
    pub present: bool,
    pub exe_ok: bool,
    pub exe: Option<String>,
}

pub fn hook_status() -> HookStatus {
    let settings = read_settings();
    // Only a group whose matcher is exactly "Bash" fires for Bash commands —
    // our entry drifted under any other matcher is a dead hook and must not
    // read as present.
    let cmd = settings
        .pointer(&format!("/hooks/{HOOK_EVENT}"))
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter(|g| g.get("matcher").and_then(|m| m.as_str()) == Some("Bash"))
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

/// The executable token from a command string `<exe> _hook claude-code`. This
/// is the inverse of [`hook_command`]: a leading single-quoted token is decoded
/// back through the `'\''` idiom (so a spaced path that also contains a single
/// quote round-trips as one token), a leading double-quoted token is read to its
/// closing quote, and an unquoted token ends at the first whitespace. `None` if
/// the command has no token.
fn first_token(cmd: &str) -> Option<String> {
    let cmd = cmd.trim_start();
    if let Some(rest) = cmd.strip_prefix('\'') {
        return Some(decode_single_quoted(rest));
    }
    if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"')?;
        let tok = &rest[..end];
        return if tok.is_empty() {
            None
        } else {
            Some(tok.to_string())
        };
    }
    let tok = cmd.split_whitespace().next()?;
    if tok.is_empty() {
        None
    } else {
        Some(tok.to_string())
    }
}

/// Decode the body of a single-quoted shell token (the text after the opening
/// `'`), reversing [`hook_command`]'s `'` → `'\''` encoding. The `'\''` sequence
/// (close, backslash-escaped quote, reopen) is one literal `'`; a lone `'` ends
/// the token. Returns the decoded value; an unterminated token yields what was
/// read so far (best effort — the command came from our own settings file).
fn decode_single_quoted(after_open: &str) -> String {
    let mut chars = after_open.chars().peekable();
    let mut out = String::new();
    loop {
        // Literal run up to the next quote.
        while let Some(&c) = chars.peek() {
            if c == '\'' {
                break;
            }
            out.push(c);
            chars.next();
        }
        // Consume the quote that ended the run (or end on EOF).
        if chars.next() != Some('\'') {
            return out;
        }
        // `'\''` idiom? After the close, the next two chars are `\'`.
        let mut look = chars.clone();
        if look.next() == Some('\\') && look.next() == Some('\'') {
            chars.next(); // '\'
            chars.next(); // '\''
                          // The following `'` reopens the quote; consume it as the start of
                          // the next literal run.
            if chars.peek() == Some(&'\'') {
                chars.next();
            }
            out.push('\'');
            continue;
        }
        return out; // lone close
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
