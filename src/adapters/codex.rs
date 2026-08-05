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
use crate::store::{read_sweeps, write_sweeps, PendingCall, SessionCtx};
use crate::util::{cap_chars, first_n_lines, truncate, CMD_MAX};
use serde_json::Value;
use std::collections::BTreeMap;
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
///
/// Correctness properties the watermark upholds:
/// - A `function_call` / `function_call_output` pair split across two sweeps is
///   matched **while the file remains the watermark's `last_path`**: pending
///   calls are persisted in `sweeps.json` (`pending_calls`), so an output
///   appended on the next run still finds its call even though `session_meta`
///   lives above the resume offset. If a newer file is swept before the output
///   arrives the pending call is dropped (see the limitation below) and the pair
///   is not matched.
/// - A signal that fails to persist does not advance the watermark past it: the
///   offset is held at the failing line so the next sweep retries, rather than
///   committing the offset and silently dropping the signal.
///
/// Known limitation (deliberate, given Layer 1 is best-effort): the watermark
/// tracks a single newest file. When the sweep advances to a newer session file
/// it drops the previous file's still-unresolved pending calls, and a file once
/// passed is never revisited. So a `function_call` whose `function_call_output`
/// is appended only after the watermark has advanced to a newer swept file is
/// not matched — even when both endpoints live in the *same* file, because once
/// the watermark advances past it the file is never re-read and its pending call
/// is dropped. (A newer file merely *appearing* on disk is not enough — while
/// `last_path` still names this file, a resumed sweep keeps reading it and the
/// pending call survives.) This affects only interleaved/delayed-output
/// sessions; the common single-session-in-order case is fully covered.
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
    // `pending` and `session` belong to `last_path` and ride along the loop.
    let mut pending = mark.pending_calls.clone();
    let mut session = mark.last_session.clone();
    let mut unrecognized_files = 0usize;
    let mut emit_failed = false;

    for file in &files {
        // Skip files entirely before the watermark (already processed).
        let already_past = match &last_path {
            Some(lp) => file.as_path() < Path::new(lp),
            None => false,
        };
        if already_past {
            continue;
        }
        let is_resume = matches!(&last_path, Some(lp) if Path::new(lp) == file.as_path());
        let resume_offset = if is_resume { last_offset } else { 0 };
        if !is_resume {
            // Moving to a new file: the previous file's pending calls and
            // session context no longer apply.
            pending.clear();
            session = None;
        }

        let r = sweep_file(file, resume_offset, &mut pending, &mut session);
        out.sessions_scanned += 1;
        if r.had_command_event {
            out.sessions_with_commands += 1;
        }
        out.signals_emitted += r.emitted;
        if r.unrecognized {
            unrecognized_files += 1;
        }

        // Advance the watermark to this file's stopping point. On a persistence
        // failure `new_offset` is already capped at the failing line.
        last_path = Some(file.to_string_lossy().into_owned());
        last_offset = r.new_offset;
        if r.emit_failed {
            emit_failed = true;
            // Stop at the first persistence failure so the watermark stays put
            // and the next sweep retries the failing line.
            break;
        }
    }

    if emit_failed {
        out.warning = Some(
            "a signal could not be persisted; the watermark was held so the next sweep retries it"
                .into(),
        );
    } else if out.signals_emitted == 0 && out.sessions_with_commands == 0 && unrecognized_files > 0
    {
        out.warning = Some(format!(
            "codex log format unrecognized in {unrecognized_files} file(s); no signals extracted"
        ));
    }

    mark.last_path = last_path;
    mark.last_offset = last_offset;
    mark.pending_calls = pending;
    mark.last_session = session;
    let _ = write_sweeps(&sweeps);
    out
}

struct FileResult {
    emitted: usize,
    had_command_event: bool,
    unrecognized: bool,
    new_offset: u64,
    /// A signal failed to persist; `new_offset` is capped at the failing line.
    emit_failed: bool,
}

fn sweep_file(
    path: &Path,
    offset: u64,
    pending: &mut BTreeMap<String, PendingCall>,
    session: &mut Option<SessionCtx>,
) -> FileResult {
    let empty = FileResult {
        emitted: 0,
        had_command_event: false,
        unrecognized: false,
        new_offset: offset,
        emit_failed: false,
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
    let default_offset = offset + last_nl as u64 + 1;

    let mut emitted = 0usize;
    let mut had_command_event = false;
    let mut recognized = false; // saw session_meta or a function_call
    let mut has_content = false;
    let mut emit_failed = false;
    let mut fail_offset: Option<u64> = None;

    // Walk complete lines, tracking each line's absolute byte offset so a
    // persistence failure can pin the watermark to the failing line.
    let mut line_start_abs = offset;
    let mut cursor = 0usize;
    while cursor <= last_nl {
        let nl = match buf[cursor..=last_nl].iter().position(|&b| b == b'\n') {
            Some(i) => i,
            None => break,
        };
        let line_bytes = &buf[cursor..cursor + nl];
        let line_len = nl + 1;
        let line = match std::str::from_utf8(line_bytes) {
            Ok(s) => s,
            Err(_) => {
                // Unparseable bytes; skip the line without failing the sweep.
                cursor += line_len;
                line_start_abs += line_len as u64;
                continue;
            }
        };
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            has_content = true;
            if let Ok(o) = serde_json::from_str::<Value>(line) {
                let ttype = o.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let ts = o
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if ttype == "session_meta" {
                    recognized = true;
                    let sc = session.get_or_insert_with(SessionCtx::default);
                    if let Some(p) = o.get("payload") {
                        if let Some(id) = p.get("id").and_then(|v| v.as_str()) {
                            sc.session_id = id.to_string();
                        }
                        if let Some(c) = p.get("cwd").and_then(|v| v.as_str()) {
                            let cwd_str = c.to_string();
                            // Resolve the repo once per session (F6): a real
                            // remote normalizes to host/path; a bare local repo
                            // falls back to its path; a non-repo yields None.
                            if sc.repo.is_none() {
                                sc.repo = resolve_repo(Some(&cwd_str));
                            }
                            sc.cwd = Some(cwd_str);
                        }
                        if let Some(g) = p
                            .get("git")
                            .and_then(|g| g.get("commit_hash"))
                            .and_then(|v| v.as_str())
                        {
                            sc.git_sha = Some(g.to_string());
                        }
                    }
                } else if ttype == "response_item" {
                    if let Some(p) = o.get("payload") {
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
                                pending.insert(call_id, PendingCall { cmd, ts });
                            }
                        } else if pt == "function_call_output" {
                            had_command_event = true;
                            let call_id = p
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output =
                                p.get("output").and_then(|v| v.as_str()).unwrap_or("");
                            if let Some(pc) = pending.remove(&call_id) {
                                if let Some(exit) = parse_exit(output) {
                                    if exit != 0 {
                                        let (session_id, cwd, repo) = session_tuple(session);
                                        let head = extract_output_head(output);
                                        let sig = Signal::new(
                                            pc.ts.clone(),
                                            repo,
                                            cwd,
                                            Some(HARNESS.into()),
                                            &pc.cmd,
                                            exit,
                                            Some(&head),
                                            Some(session_id.clone()),
                                        );
                                        if append_signal(HARNESS, &session_id, &sig).is_ok() {
                                            emitted += 1;
                                        } else {
                                            // Persistence failed: restore the
                                            // pending call, pin the watermark to
                                            // this line, and stop so the next
                                            // sweep retries it (F3).
                                            pending.insert(call_id, pc);
                                            emit_failed = true;
                                            fail_offset = Some(line_start_abs);
                                            break;
                                        }
                                    }
                                }
                                // exit == 0 or unparseable: the call resolved
                                // without a failure signal; consume it.
                            }
                            // No pending call for this id: an orphan output
                            // (e.g. call seen before our first sweep) — ignore.
                        }
                    }
                }
            }
        }

        cursor += line_len;
        line_start_abs += line_len as u64;
    }

    let new_offset = fail_offset.unwrap_or(default_offset);
    FileResult {
        emitted,
        had_command_event,
        unrecognized: has_content && !recognized,
        new_offset,
        emit_failed,
    }
}

/// Borrow the session context into the (session_id, cwd, repo) tuple a signal
/// needs, defaulting the session id when none was seen.
fn session_tuple(session: &Option<SessionCtx>) -> (String, Option<String>, Option<String>) {
    match session {
        Some(sc) => (
            if sc.session_id.is_empty() {
                "unknown".to_string()
            } else {
                sc.session_id.clone()
            },
            sc.cwd.clone(),
            sc.repo.clone(),
        ),
        None => ("unknown".to_string(), None, None),
    }
}

/// Resolve a normalized repo identifier for `cwd` via git, degrading to `None`
/// outside a repo. Uses the lean two-call resolver (`repo_of`) — the sweep runs
/// per session and needs only `.repo`. Captures only a pointer (host/path or
/// repo root), never file contents or env values.
fn resolve_repo(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    crate::git_meta::repo_of(Path::new(cwd))
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
