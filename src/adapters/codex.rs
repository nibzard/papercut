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
use crate::store::{read_sweeps, write_sweeps, FileMark, PendingCall, SessionCtx, SweepMark};
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
    /// A signal could not be appended; the failing file's offset was held so
    /// the next sweep retries it.
    #[serde(skip_serializing)]
    pub emit_failed: bool,
    /// sweeps.json could not be persisted; the next sweep re-reads the same
    /// content and can duplicate signals.
    #[serde(skip_serializing)]
    pub watermark_error: Option<String>,
}

/// Sweep new Codex session content since the last run, appending signals.
/// Idempotent across runs via per-file byte-offset high-water marks.
///
/// Correctness properties the watermark upholds:
/// - Every session file keeps its own mark, so parallel sessions are safe: an
///   append to an older-sorting file is swept even after newer files were
///   processed.
/// - A `function_call` / `function_call_output` pair split across sweeps is
///   matched whenever the output arrives — the file's pending calls persist in
///   its `FileMark` and the file is re-entered whenever it grows.
/// - A signal that fails to persist does not advance the watermark past it: the
///   offset is held at the failing line so the next sweep retries, rather than
///   committing the offset and silently dropping the signal.
/// - A file that shrank (truncated or replaced) is re-read from the start with
///   a warning: the duplication is bounded to one re-read per shrink event,
///   while skipping would lose the new content forever.
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
    let mut unrecognized_files = 0usize;
    let mut shrunk_files = 0usize;
    let mut emit_failed = false;
    {
        let mark = sweeps.marks.entry(HARNESS.into()).or_default();
        migrate_legacy(mark, &files);

        for file in &files {
            let key = file.to_string_lossy().into_owned();
            let fm = mark.files.entry(key).or_default();
            let len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            if len < fm.offset {
                // The file shrank: it was truncated or replaced, and the old
                // content is gone. Re-read from the start; stale pending calls
                // and session context would mis-pair against the new content.
                *fm = FileMark::default();
                shrunk_files += 1;
            }
            if len <= fm.offset {
                continue; // nothing new in this file
            }

            let r = sweep_file(file, fm.offset, &mut fm.pending_calls, &mut fm.session);
            out.sessions_scanned += 1;
            if r.had_command_event {
                out.sessions_with_commands += 1;
            }
            out.signals_emitted += r.emitted;
            if r.unrecognized {
                unrecognized_files += 1;
            }

            // Advance this file's offset to its stopping point. On a
            // persistence failure `new_offset` is already capped at the
            // failing line.
            fm.offset = r.new_offset;
            if r.emit_failed {
                emit_failed = true;
                // Stop at the first persistence failure so the offset stays
                // put and the next sweep retries the failing line. Files not
                // yet visited keep their own untouched marks.
                break;
            }
        }

        // Deleted session files leave no zombie marks behind.
        mark.files.retain(|p, _| Path::new(p).exists());
    }

    out.emit_failed = emit_failed;
    let mut warnings: Vec<String> = Vec::new();
    if emit_failed {
        warnings.push(
            "a signal could not be persisted; the watermark was held so the next sweep retries it"
                .into(),
        );
    }
    if shrunk_files > 0 {
        warnings.push(format!(
            "{shrunk_files} codex session file(s) shrank; re-read from the start (bounded duplicates possible)"
        ));
    }
    if out.signals_emitted == 0 && out.sessions_with_commands == 0 && unrecognized_files > 0 {
        warnings.push(format!(
            "codex log format unrecognized in {unrecognized_files} file(s); no signals extracted"
        ));
    }
    if !warnings.is_empty() {
        out.warning = Some(warnings.join("; "));
    }

    if let Err(e) = write_sweeps(&sweeps) {
        out.watermark_error = Some(format!("{e:#}"));
    }
    out
}

/// Fold the legacy single-file watermark into the per-file map, once.
///
/// Old semantics treated every file that sorts before `last_path` as fully
/// processed, so those files fast-forward to their current length: no
/// duplicate emission, and future appends ARE picked up (which the old design
/// lost). `last_path` itself carries its offset, pending calls, and session
/// context over verbatim. The legacy fields are cleared and never written
/// again.
fn migrate_legacy(mark: &mut SweepMark, files: &[PathBuf]) {
    let Some(lp) = mark.last_path.take() else {
        return;
    };
    if mark.files.is_empty() {
        for f in files {
            let key = f.to_string_lossy().into_owned();
            if f.as_path() < Path::new(&lp) {
                let len = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                mark.files.insert(
                    key,
                    FileMark {
                        offset: len,
                        ..Default::default()
                    },
                );
            } else if Path::new(&lp) == f.as_path() {
                mark.files.insert(
                    key,
                    FileMark {
                        offset: mark.last_offset,
                        pending_calls: std::mem::take(&mut mark.pending_calls),
                        session: mark.last_session.take(),
                    },
                );
            }
        }
    }
    mark.last_offset = 0;
    mark.pending_calls.clear();
    mark.last_session = None;
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
                            let output = p.get("output").and_then(|v| v.as_str()).unwrap_or("");
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
