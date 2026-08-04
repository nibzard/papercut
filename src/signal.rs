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

/// `~/.local/share/papercuts/signals/<harness>/`, or `None` if no home data dir
/// is resolvable (the hook path then silently no-ops).
pub fn signals_dir(harness: &str) -> Option<PathBuf> {
    data_root().map(|r| r.join("signals").join(harness))
}

/// Path to a session's signal file.
pub fn session_file(harness: &str, session: &str) -> Option<PathBuf> {
    signals_dir(harness).map(|d| d.join(format!("{session}.jsonl")))
}

/// Append one signal line to `<harness>/<session>.jsonl`. Creates dirs.
pub fn append_signal(harness: &str, session: &str, signal: &Signal) -> anyhow::Result<()> {
    let dir = signals_dir(harness).context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    std::fs::create_dir_all(&dir).context("create signals dir")?;
    let path = session_file(harness, session).context("no home data dir")?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open signal file {}", path.display()))?;
    let mut line = serde_json::to_string(signal).context("serialize signal")?;
    line.push('\n');
    f.write_all(line.as_bytes())
        .with_context(|| format!("write signal file {}", path.display()))?;
    Ok(())
}

/// Read every signal line for a harness, skipping unparseable lines.
pub fn read_signals(harness: &str) -> (Vec<Signal>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let Some(dir) = signals_dir(harness) else {
        return (out, 0);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (out, 0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
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
}
