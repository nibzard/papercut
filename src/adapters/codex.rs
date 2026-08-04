//! Codex signal adapter (sweep tier).
//!
//! Codex keeps no live hook, so `papercut sweep` parses its on-disk session
//! rollout logs (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`) and extracts
//! failed `exec_command` calls as Layer 1 signals.
//!
//! Verified format (2026-08-04) from a local install. Each line is one JSON
//! object: `session_meta` carries payload.{id, cwd, git.commit_hash};
//! `response_item/function_call` carries payload.{name="exec_command",
//! arguments=`{"cmd":"..."}`, call_id}; `response_item/function_call_output`
//! carries payload.{call_id, output} where `output` contains the line
//! `Process exited with code <N>`.
//!
//! Unknown / changed formats degrade to a no-op with a warning — never a crash.

use crate::paths::home_dir;
use crate::signal::{append_signal, Signal};
use crate::store::{read_sweeps, write_sweeps};
use crate::util::{cap_chars, first_n_lines, truncate, CMD_MAX};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const HARNESS: &str = "codex";

pub fn sessions_root() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".codex").join("sessions"))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SweepOutcome {
    pub harness: String,
    pub signals_emitted: usize,
    pub sessions_scanned: usize,
    pub sessions_with_commands: usize,
    pub warning: Option<String>,
}

/// Sweep new Codex session content since the last run, appending signals.
/// Idempotent across runs via per-harness byte-offset high-water marks.
pub fn sweep() -> SweepOutcome {
    let mut out = SweepOutcome {
        harness: HARNESS.into(),
        ..Default::default()
    };
    let Some(root) = sessions_root() else {
        out.warning = Some("codex sessions dir not found ($HOME unset)".into());
        return out;
    };

    let mut files = collect_jsonl(&root);
    files.sort();
    if files.is_empty() {
        out.warning = Some(format!("no codex session logs under {}", root.display()));
        return out;
    }

    let mut sweeps = read_sweeps();
    let mark = sweeps.marks.entry(HARNESS.into()).or_default();
    let mut last_path = mark.last_path.clone();
    let mut last_offset = mark.last_offset;
    let mut unrecognized_files = 0usize;

    for file in &files {
        // Skip files entirely before the watermark (already processed).
        let already_past = match &last_path {
            Some(lp) => file.as_path() < Path::new(lp),
            None => false,
        };
        if already_past {
            continue;
        }
        let resume_offset = match &last_path {
            Some(lp) if Path::new(lp) == file.as_path() => last_offset,
            _ => 0,
        };

        let r = sweep_file(file, resume_offset);
        out.sessions_scanned += 1;
        if r.had_command_event {
            out.sessions_with_commands += 1;
        }
        out.signals_emitted += r.emitted;
        if r.unrecognized {
            unrecognized_files += 1;
        }

        // Advance the watermark to the end of this file.
        last_path = Some(file.to_string_lossy().into_owned());
        last_offset = r.new_offset;
    }

    if out.signals_emitted == 0 && out.sessions_with_commands == 0 && unrecognized_files > 0 {
        out.warning = Some(format!(
            "codex log format unrecognized in {unrecognized_files} file(s); no signals extracted"
        ));
    }

    mark.last_path = last_path;
    mark.last_offset = last_offset;
    let _ = write_sweeps(&sweeps);
    out
}

struct FileResult {
    emitted: usize,
    had_command_event: bool,
    unrecognized: bool,
    new_offset: u64,
}

fn sweep_file(path: &Path, offset: u64) -> FileResult {
    let empty = FileResult {
        emitted: 0,
        had_command_event: false,
        unrecognized: false,
        new_offset: offset,
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return empty,
    };
    let start = (offset as usize).min(bytes.len());
    let buf = &bytes[start..];

    // Only parse up to the last complete line; leave the trailing partial line
    // (if the writer is mid-append) for the next run.
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return empty; // no complete line yet
    };
    let new_offset = offset + last_nl as u64 + 1;
    let text = String::from_utf8_lossy(&buf[..=last_nl]);

    let mut pending: HashMap<String, (String, String)> = HashMap::new(); // call_id → (cmd, ts)
    let mut session_id = String::from("unknown");
    let mut cwd: Option<String> = None;
    let mut git_sha: Option<String> = None;
    let mut emitted = 0usize;
    let mut had_command_event = false;
    let mut recognized = false; // saw session_meta or a function_call
    let mut has_content = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        has_content = true;
        let Ok(o) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ttype = o.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = o
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if ttype == "session_meta" {
            recognized = true;
            if let Some(p) = o.get("payload") {
                if let Some(id) = p.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
                if let Some(c) = p.get("cwd").and_then(|v| v.as_str()) {
                    cwd = Some(c.to_string());
                }
                if let Some(g) = p
                    .get("git")
                    .and_then(|g| g.get("commit_hash"))
                    .and_then(|v| v.as_str())
                {
                    git_sha = Some(g.to_string());
                }
            }
            continue;
        }

        if ttype != "response_item" {
            continue;
        }
        let Some(p) = o.get("payload") else {
            continue;
        };
        let pt = p.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if pt == "function_call" {
            recognized = true;
            had_command_event = true;
            let call_id = p
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(cmd) = extract_cmd(p) {
                pending.insert(call_id, (cmd, ts));
            }
            continue;
        }

        if pt == "function_call_output" {
            had_command_event = true;
            let call_id = p
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = p.get("output").and_then(|v| v.as_str()).unwrap_or("");
            if let Some((cmd, cts)) = pending.remove(&call_id) {
                if let Some(exit) = parse_exit(output) {
                    if exit != 0 {
                        let head = extract_output_head(output);
                        let _ = git_sha; // context gathered; could be attached if schema grows
                        let sig = Signal::new(
                            cts,
                            None,
                            cwd.clone(),
                            Some(HARNESS.into()),
                            &cmd,
                            exit,
                            Some(&head),
                            Some(session_id.clone()),
                        );
                        if append_signal(HARNESS, &session_id, &sig).is_ok() {
                            emitted += 1;
                        }
                    }
                }
            }
            continue;
        }
    }

    FileResult {
        emitted,
        had_command_event,
        unrecognized: has_content && !recognized,
        new_offset,
    }
}

fn extract_cmd(p: &Value) -> Option<String> {
    let args = p.get("arguments").and_then(|v| v.as_str())?;
    let av: Value = serde_json::from_str(args).ok()?;
    for k in ["cmd", "command"] {
        if let Some(c) = av.get(k).and_then(|v| v.as_str()) {
            return Some(truncate(c, CMD_MAX));
        }
    }
    None
}

fn parse_exit(output: &str) -> Option<i32> {
    const NEEDLE: &str = "Process exited with code ";
    let idx = output.find(NEEDLE)?;
    let rest = &output[idx + NEEDLE.len()..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num.parse::<i32>().ok()
}

fn extract_output_head(output: &str) -> String {
    let after = match output.find("Output:\n") {
        Some(i) => &output[i + "Output:\n".len()..],
        None => output,
    };
    let lines = first_n_lines(after, 5);
    cap_chars(&lines, 800)
}

fn collect_jsonl(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exit_code() {
        assert_eq!(parse_exit("Process exited with code 0\n"), Some(0));
        assert_eq!(
            parse_exit("foo\nProcess exited with code 127\nbar"),
            Some(127)
        );
        assert_eq!(parse_exit("no code here"), None);
    }

    #[test]
    fn extracts_cmd_from_arguments() {
        let p = serde_json::json!({
            "type": "function_call",
            "name": "exec_command",
            "arguments": "{\"cmd\":\"ls -la\"}",
            "call_id": "c1",
        });
        assert_eq!(extract_cmd(&p).as_deref(), Some("ls -la"));
    }
}
