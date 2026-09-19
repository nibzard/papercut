//! Layer 1 signals — deliberately dumb raw facts about failed commands.
//!
//! These are high-volume, zero-trust records. They are never deduped at
//! capture time (duplicates are evidence). The append path is used by live
//! hook adapters and by `sweep`; both must swallow every error so a broken
//! signal sink can never surface in the parent task.

use crate::paths::data_root;
use crate::util::{
    cap_chars, first_n_lines, truncate, CMD_MAX, FIELD_MAX, STDERR_MAX_CHARS, STDERR_MAX_LINES,
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub ts: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent: Option<String>,
    pub cmd: String,
    pub exit: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stderr_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session: Option<String>,
}

impl Signal {
    /// Construct a signal, truncating every field aggressively so no runaway
    /// command, path, or stderr burst can bloat the store.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ts: impl Into<String>,
        repo: Option<String>,
        cwd: Option<String>,
        agent: Option<String>,
        cmd: &str,
        exit: i32,
        stderr: Option<&str>,
        session: Option<String>,
    ) -> Self {
        let bound = |o: Option<String>| o.map(|s| cap_chars(&s, FIELD_MAX));
        let stderr_head = stderr.map(|s| {
            let lines = first_n_lines(s, STDERR_MAX_LINES);
            cap_chars(&lines, STDERR_MAX_CHARS)
        });
        Self {
            ts: ts.into(),
            repo: bound(repo),
            cwd: bound(cwd),
            agent: bound(agent),
            cmd: truncate(cmd, CMD_MAX),
            exit,
            stderr_head,
            session: bound(session),
        }
    }
}

/// A path segment is safe to join into the store tree only if it is non-empty
/// and contains no path separators or traversal components. This guards
/// `<harness>` and `<session>` so a hostile or malformed value can never escape
/// the `signals/` directory (`../../etc/...`) or scribble outside the store.
pub fn safe_segment(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && !s.contains('\\') && s != "." && s != ".."
}

/// Reduce a harness-supplied session id to a safe signal file name: keep only
/// ASCII alphanumerics plus `.`, `_`, `-`; cap the length; fall back to
/// "unknown" when nothing safe remains. Capture callers use this so a hostile
/// or malformed session id costs the signal a pretty file name, never the
/// signal itself. The signal's `session` FIELD keeps the original (bounded)
/// value — only the file name is sanitized.
pub fn filename_session(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    if safe_segment(&cleaned) {
        cleaned
    } else {
        "unknown".to_string()
    }
}

/// `~/.local/share/papercuts/signals/<harness>/`, or `None` if no home data dir
/// is resolvable **or** the harness segment is unsafe. Returning `None` makes
/// the hook path silently no-op rather than write outside the store.
pub fn signals_dir(harness: &str) -> Option<PathBuf> {
    if !safe_segment(harness) {
        return None;
    }
    data_root().map(|r| r.join("signals").join(harness))
}

/// Path to a session's signal file, or `None` if either segment is unsafe or no
/// home data dir is resolvable.
pub fn session_file(harness: &str, session: &str) -> Option<PathBuf> {
    if !safe_segment(session) {
        return None;
    }
    signals_dir(harness).map(|d| d.join(format!("{session}.jsonl")))
}

/// Append one signal line to `<harness>/<session>.jsonl`. Creates dirs.
///
/// The whole serialized line is written in a single `write_all` (which loops on
/// short writes) under `O_APPEND`, so concurrent appenders never overwrite each
/// other's line. `sync_all` is best-effort: a successful `write_all` means the
/// line is already in the kernel page cache and visible to the next reader, so
/// we treat it as recorded and ignore an `fsync` error. Returning `Ok` here (not
/// `Err`) on a sync failure is deliberate — the sweep pins its watermark to the
/// failing line on `Err` and would re-append the same output next run,
/// manufacturing an accidental duplicate. Only a `write_all` failure (the line
/// genuinely did not land) returns `Err` so the caller can retry.
///
/// Every caller (hook adapter, sweep) discards the error anyway — a broken
/// signal sink must never surface in the parent task.
pub fn append_signal(harness: &str, session: &str, signal: &Signal) -> anyhow::Result<()> {
    // Validate BOTH segments before touching the filesystem: an unsafe session
    // must degrade to an error WITHOUT a directory-creation side effect, which
    // would otherwise mkdir the signals/<harness> subtree (in the real store
    // when a test forgets to isolate) even though the signal is then rejected.
    let path = session_file(harness, session).context("unsafe or unresolvable signal path")?;
    let dir = signals_dir(harness).context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    // 0700, like the rest of the private store: the signal log carries failed
    // command lines and a bounded stderr head, which can include leaked env or
    // secret values, so the subtree must be owner-only — not merely
    // private-by-location. `create_private_dir` is recursive, so the first hook
    // to fire creates data_root/signals/<harness> (and any missing parents) all
    // at 0700, even when no `install`/`add`/`sweep` has run `ensure_store` first.
    crate::store::create_private_dir(&dir).context("create signals dir")?;
    let mut line = serde_json::to_string(signal).context("serialize signal")?;
    line.push('\n');
    // O_APPEND: the kernel advances the offset and writes atomically, so two
    // concurrent appenders never overwrite each other's line. write_all loops
    // on short writes so the whole line lands in one logical write.
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options
        .open(&path)
        .with_context(|| format!("open signal file {}", path.display()))?;
    f.write_all(line.as_bytes())
        .with_context(|| format!("write signal file {}", path.display()))?;
    // Best-effort durability: the bytes are already visible to readers, so a
    // sync failure is not a write failure (see the doc comment above).
    let _ = f.sync_all();
    Ok(())
}

/// Read every signal line for a harness, skipping unparseable lines.
pub fn read_signals(harness: &str) -> (Vec<Signal>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let Some(dir) = signals_dir(harness) else {
        return (out, 0);
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (out, 0),
        Err(_) => return (out, 1),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Signal>(line) {
                Ok(s) => out.push(s),
                Err(_) => skipped += 1,
            }
        }
    }
    out.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.cmd.cmp(&b.cmd)));
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_command_and_stderr() {
        let long_cmd = "x".repeat(1000);
        let long_err = "line\n".repeat(50);
        let s = Signal::new(
            "2026-08-04T20:42:00Z",
            None,
            None,
            None,
            &long_cmd,
            1,
            Some(&long_err),
            None,
        );
        assert!(s.cmd.chars().count() <= CMD_MAX + 1); // +1 for ellipsis
        let head_lines = s.stderr_head.unwrap();
        assert!(head_lines.lines().count() <= STDERR_MAX_LINES);
    }

    #[test]
    fn none_stderr_omits_field() {
        let s = Signal::new("t", None, None, None, "cmd", 2, None, None);
        let j = serde_json::to_string(&s).unwrap();
        assert!(!j.contains("stderr_head"));
    }

    #[test]
    fn long_string_fields_are_bounded() {
        let big = "p".repeat(5000);
        let s = Signal::new(
            "t",
            Some(big.clone()),
            Some(big.clone()),
            Some(big.clone()),
            "cmd",
            2,
            None,
            Some(big.clone()),
        );
        // +1 for the truncation ellipsis.
        assert!(s.cwd.as_ref().unwrap().chars().count() <= FIELD_MAX + 1);
        assert!(s.session.as_ref().unwrap().chars().count() <= FIELD_MAX + 1);
        assert!(s.agent.as_ref().unwrap().chars().count() <= FIELD_MAX + 1);
        assert!(s.repo.as_ref().unwrap().chars().count() <= FIELD_MAX + 1);
        assert!(s.cwd.unwrap().ends_with('…'));
    }

    #[test]
    fn safe_segment_rejects_traversal() {
        assert!(safe_segment("claude-code"));
        assert!(safe_segment("01HZXSESSION"));
        // Rejects empties, traversal, and anything with a separator.
        assert!(!safe_segment(""));
        assert!(!safe_segment("."));
        assert!(!safe_segment(".."));
        assert!(!safe_segment("../.."));
        assert!(!safe_segment("a/b"));
        assert!(!safe_segment("a\\b"));
        assert!(!safe_segment("/etc/passwd"));
    }

    #[test]
    fn unsafe_segments_yield_no_path() {
        // A traversal harness must not resolve to a directory at all.
        assert!(signals_dir("../../etc").is_none());
        assert!(session_file("../etc", "sess").is_none());
        assert!(session_file("ok", "../../etc").is_none());
        // And therefore append degrades to an error the caller discards,
        // never a write outside the store.
        let s = Signal::new("t", None, None, None, "cmd", 1, None, None);
        assert!(append_signal("../../etc", "sess", &s).is_err());
        assert!(append_signal("ok", "../pwn", &s).is_err());
    }

    /// Removes its temp tree on drop (even on panic), mirroring `store::with_store`.
    struct TmpGuard(std::path::PathBuf);
    impl Drop for TmpGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn isolated() -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!("pc-sig-{}", crate::id::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        tmp
    }

    /// `append_signal` must create the `signals/<harness>` subtree owner-only
    /// (0700): the signal log carries failed command lines and a bounded stderr
    /// head, which can include leaked env or secret values, so it is private by
    /// PERMISSION, not merely private by location.
    #[test]
    fn append_signal_creates_owner_only_signals_dir() {
        let _g = crate::test_env::EnvGuard::acquire(&["HOME", "XDG_DATA_HOME"]);
        let tmp = isolated();
        let _cleanup = TmpGuard(tmp);

        let s = Signal::new("t", None, None, None, "cmd", 1, None, None);
        append_signal("ok", "sess", &s).expect("a safe signal appends");

        let dir = signals_dir("ok").expect("safe harness resolves");
        assert!(dir.exists(), "signals/ok created");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o700,
                "signals dir must be owner-only (0700), got {:o}",
                mode
            );
        }
        let (sigs, _) = read_signals("ok");
        assert_eq!(sigs.len(), 1, "the signal landed");
    }

    /// An unsafe session segment must degrade to an error WITHOUT creating the
    /// `signals/<harness>` subtree as a side effect. Before the validate-before-
    /// mkdir ordering, a real-store test that forgot to isolate could mkdir the
    /// harness dir even though the signal was then rejected.
    #[test]
    fn unsafe_session_errors_without_creating_dirs() {
        let _g = crate::test_env::EnvGuard::acquire(&["HOME", "XDG_DATA_HOME"]);
        let tmp = isolated();
        let _cleanup = TmpGuard(tmp);

        let s = Signal::new("t", None, None, None, "cmd", 1, None, None);
        let res = append_signal("ok", "../pwn", &s);
        assert!(res.is_err(), "an unsafe session segment must error");
        assert!(
            signals_dir("ok").map(|d| !d.exists()).unwrap_or(true),
            "unsafe session must not mkdir signals/ok"
        );
        let (sigs, _) = read_signals("ok");
        assert!(sigs.is_empty(), "nothing was written");
    }
}
