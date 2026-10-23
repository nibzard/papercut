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
use std::io::{Read, Seek, SeekFrom};
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

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CoverageProbe {
    pub supported_exec_records: usize,
    pub unsupported_exec_records: usize,
    pub observable_custom_results: usize,
    pub opaque_custom_results: usize,
}

/// Inspect a bounded tail of recent logs for execution record shapes. This is
/// a diagnostic only. It never changes sweep state.
pub fn coverage_probe() -> CoverageProbe {
    let Some(root) = sessions_root() else {
        return CoverageProbe::default();
    };
    let mut files = collect_jsonl(&root);
    files.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    let mut coverage = CoverageProbe::default();
    for path in files.into_iter().rev().take(16) {
        let Ok(mut file) = std::fs::File::open(path) else {
            continue;
        };
        let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let start = len.saturating_sub(256 * 1024);
        if file.seek(SeekFrom::Start(start)).is_err() {
            continue;
        }
        let mut text = String::new();
        if file.read_to_string(&mut text).is_err() {
            continue;
        }
        let mut custom_calls = std::collections::BTreeSet::new();
        for line in text.lines().skip(usize::from(start > 0)) {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(payload) = record.get("payload") else {
                continue;
            };
            let record_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
            match (record_type, name) {
                ("function_call", "exec_command" | "write_stdin")
                | ("custom_tool_call", "exec") => coverage.supported_exec_records += 1,
                ("function_call", "exec") | ("custom_tool_call", "exec_command") => {
                    coverage.unsupported_exec_records += 1;
                }
                _ => {}
            }
            if record_type == "custom_tool_call" && name == "exec" {
                if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                    custom_calls.insert(call_id.to_string());
                }
            } else if record_type == "custom_tool_call_output" {
                let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                if !custom_calls.remove(call_id) {
                    continue;
                }
                let output = output_text(payload.get("output"));
                if parse_json_exit_results(&output).is_empty() {
                    coverage.opaque_custom_results += 1;
                } else {
                    coverage.observable_custom_results += 1;
                }
            }
        }
    }
    coverage
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
    sweep_mode(ParseMode::All)
}

/// Re-read only record shapes that older adapter versions could not capture.
/// The separate watermark makes this explicit recovery idempotent without
/// re-emitting legacy direct-command failures.
pub fn backfill_current() -> SweepOutcome {
    sweep_mode(ParseMode::PreviouslyMissed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseMode {
    All,
    PreviouslyMissed,
}

fn sweep_mode(mode: ParseMode) -> SweepOutcome {
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
        if mode == ParseMode::PreviouslyMissed && mark.current_backfill_complete {
            out.warning = Some("current-format backfill already completed".into());
            return out;
        }
        if mode == ParseMode::All {
            // Adopt root-relative watermark keys (see `rel_key`), migrating any
            // legacy absolute-path keys written by older papercut versions first.
            relativize_legacy_keys(mark, &root);
            migrate_legacy(mark, &files, &root);
        }

        let normal_offsets: BTreeMap<String, u64> = mark
            .files
            .iter()
            .map(|(path, file_mark)| (path.clone(), file_mark.offset))
            .collect();
        let backfill_offsets: BTreeMap<String, u64> = mark
            .current_backfill_files
            .iter()
            .map(|(path, file_mark)| (path.clone(), file_mark.offset))
            .collect();

        let file_marks = if mode == ParseMode::PreviouslyMissed {
            &mut mark.current_backfill_files
        } else {
            &mut mark.files
        };

        for file in &files {
            let key = rel_key(file, &root);
            let scan_limit = if mode == ParseMode::PreviouslyMissed {
                match normal_offsets.get(&key).copied() {
                    Some(limit) if limit > 0 => Some(limit),
                    _ => continue,
                }
            } else {
                None
            };
            let suppress_current_before = if mode == ParseMode::All {
                backfill_offsets.get(&key).copied().unwrap_or(0)
            } else {
                0
            };
            let fm = file_marks.entry(key).or_default();
            let real_len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            let len = scan_limit.map_or(real_len, |limit| real_len.min(limit));
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

            let r = sweep_file(
                file,
                fm.offset,
                &mut fm.pending_calls,
                &mut fm.running_processes,
                &mut fm.session,
                mode,
                SweepWindow {
                    scan_limit,
                    suppress_current_before,
                },
            );
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

        // Deleted session files leave no zombie marks behind. Keys are
        // root-relative, so reattach the root before the existence check.
        file_marks.retain(|p, _| root.join(p).exists());
        if mode == ParseMode::PreviouslyMissed && !emit_failed {
            mark.current_backfill_complete = true;
        }
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
/// again. Inserted keys are root-relative (see `rel_key`).
fn migrate_legacy(mark: &mut SweepMark, files: &[PathBuf], root: &Path) {
    let Some(lp) = mark.last_path.take() else {
        return;
    };
    if mark.files.is_empty() {
        for f in files {
            let key = rel_key(f, root);
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
                        ..Default::default()
                    },
                );
            }
        }
    }
    mark.last_offset = 0;
    mark.pending_calls.clear();
    mark.last_session = None;
}

/// Watermark key for `file`, relative to the sessions `root` so the marks
/// survive a `$HOME` relocation: the absolute path changes, but the tree under
/// `sessions/` does not, and a synced `sweeps.json` keeps matching its files.
/// Falls back to the file's string form if `file` is not under `root`.
fn rel_key(file: &Path, root: &Path) -> String {
    file.strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file.to_string_lossy().into_owned())
}

/// Convert legacy absolute-path watermark keys to root-relative keys, once. A
/// key that is absolute but not under the current `root` — its `$HOME` is gone
/// after a relocation — cannot be relocated, so it is dropped: those files
/// re-sweep cleanly from the start rather than carrying a stale, never-matching
/// offset. Keys already root-relative (current format) pass through untouched.
fn relativize_legacy_keys(mark: &mut SweepMark, root: &Path) {
    if mark.files.is_empty() {
        return;
    }
    let mut next: BTreeMap<String, FileMark> = BTreeMap::new();
    for (k, v) in std::mem::take(&mut mark.files) {
        let p = Path::new(&k);
        let rel = if p.is_absolute() {
            match p.strip_prefix(root) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => continue, // absolute under a gone home; drop it
            }
        } else {
            k // already root-relative
        };
        next.entry(rel).or_insert(v);
    }
    mark.files = next;
}

struct FileResult {
    emitted: usize,
    had_command_event: bool,
    unrecognized: bool,
    new_offset: u64,
    /// A signal failed to persist; `new_offset` is capped at the failing line.
    emit_failed: bool,
}

#[derive(Clone, Copy)]
struct SweepWindow {
    scan_limit: Option<u64>,
    suppress_current_before: u64,
}

fn sweep_file(
    path: &Path,
    offset: u64,
    pending: &mut BTreeMap<String, PendingCall>,
    running: &mut BTreeMap<String, PendingCall>,
    session: &mut Option<SessionCtx>,
    mode: ParseMode,
    window: SweepWindow,
) -> FileResult {
    let empty = FileResult {
        emitted: 0,
        had_command_event: false,
        unrecognized: false,
        new_offset: offset,
        emit_failed: false,
    };
    // Read only the tail since the last sweep's offset, not the whole file: a
    // long-lived session log keeps growing, and re-reading the already-processed
    // prefix each run would scale memory and I/O with cumulative file size rather
    // than with the new content. The watermark guarantees `offset` sits on a line
    // boundary, so seeking here lands at the start of the next record.
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return empty,
    };
    let real_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => return empty,
    };
    let scan_len = window
        .scan_limit
        .map_or(real_len, |limit| real_len.min(limit));
    let seek_to = offset.min(scan_len);
    if file.seek(SeekFrom::Start(seek_to)).is_err() {
        return empty;
    }
    let mut bytes = Vec::new();
    if file
        .take(scan_len.saturating_sub(seek_to))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return empty;
    }

    // The rollout format is newline-delimited JSONL. A file that does not end in
    // a newline has a trailing fragment that is EITHER (a) a record the writer is
    // still appending — a partial line that will not parse — OR (b) a complete
    // record whose terminating newline was never flushed (e.g. a session ended
    // mid-line, or a writer that omits the final terminator). Case (a) MUST defer
    // to the next sweep so we never race the writer; case (b) would otherwise be
    // lost forever once the session goes quiet, because the file never grows past
    // it. We tell them apart by parsing the trailing line: a complete record is
    // given a synthetic terminator and swept now, while a partial line is left
    // untouched for the next run. The tail read above ends at EOF, so the file's
    // trailing fragment is the tail's trailing fragment.
    if !bytes.ends_with(b"\n") {
        let tail_start = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        if let Ok(tail) = std::str::from_utf8(&bytes[tail_start..]) {
            let tail = tail.trim_end_matches(['\r', ' ', '\t']);
            if !tail.is_empty() && serde_json::from_str::<Value>(tail).is_ok() {
                bytes.push(b'\n');
            }
        }
    }

    // `bytes` is already the tail from `offset`; no processed prefix to skip.
    let buf = &bytes[..];

    // Only parse up to the last complete line; a trailing partial line (a record
    // the writer is mid-append) is left for the next run.
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return empty; // no complete line yet
    };
    // Cap at the real file length: a synthetic trailing newline (a complete
    // record that lacked its own terminator) must not advance the watermark past
    // the true end, or the next sweep would see `len < offset` and false-shrink.
    let default_offset = (offset + last_nl as u64 + 1).min(scan_len);

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
                        if let Some(repository_url) = p
                            .get("git")
                            .and_then(|git| git.get("repository_url"))
                            .and_then(|value| value.as_str())
                        {
                            let normalized = crate::git_meta::normalize_remote(repository_url);
                            if !normalized.is_empty() {
                                sc.repo = Some(normalized);
                            }
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
                            let name = p.get("name").and_then(|value| value.as_str()).unwrap_or("");
                            if name == "exec_command" {
                                if let Some(cmd) = extract_cmd(p) {
                                    pending.insert(
                                        call_id,
                                        PendingCall {
                                            cmd,
                                            ts,
                                            commands: Vec::new(),
                                            process_id: None,
                                        },
                                    );
                                }
                            } else if name == "write_stdin" {
                                if let Some(process_id) = extract_process_id(p) {
                                    if let Some(mut call) = running.get(&process_id).cloned() {
                                        call.process_id = Some(process_id);
                                        pending.insert(call_id, call);
                                    }
                                }
                            }
                        } else if pt == "function_call_output" {
                            had_command_event = true;
                            let call_id = p
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output = output_text(p.get("output"));
                            if let Some(pc) = pending.remove(&call_id) {
                                if let Some(process_id) = parse_running_session(&output) {
                                    running.insert(process_id, pc);
                                } else if let Some(exit) = parse_exit(&output) {
                                    if let Some(process_id) = &pc.process_id {
                                        running.remove(process_id);
                                    }
                                    let current_record = pc.process_id.is_some();
                                    let should_emit = exit != 0
                                        && match mode {
                                            ParseMode::All => {
                                                !current_record
                                                    || line_start_abs
                                                        >= window.suppress_current_before
                                            }
                                            ParseMode::PreviouslyMissed => current_record,
                                        };
                                    if should_emit {
                                        let (session_id, cwd, repo) = session_tuple(session);
                                        let head = extract_output_head(&output);
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
                                        // The file name is sanitized so a
                                        // hostile session id in a rollout
                                        // cannot wedge the sweep in a
                                        // permanent retry, nor escape the
                                        // signals dir.
                                        let file_id = crate::signal::filename_session(&session_id);
                                        if append_signal(HARNESS, &file_id, &sig).is_ok() {
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
                        } else if pt == "custom_tool_call" {
                            recognized = true;
                            had_command_event = true;
                            if p.get("name").and_then(|value| value.as_str()) == Some("exec") {
                                let call_id = p
                                    .get("call_id")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let commands = extract_custom_exec_commands(p);
                                let cmd = commands
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| "functions.exec".into());
                                pending.insert(
                                    call_id,
                                    PendingCall {
                                        cmd,
                                        ts,
                                        commands,
                                        process_id: None,
                                    },
                                );
                            }
                        } else if pt == "custom_tool_call_output" {
                            recognized = true;
                            had_command_event = true;
                            let call_id = p
                                .get("call_id")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output = output_text(p.get("output"));
                            if let Some(pc) = pending.remove(&call_id) {
                                if mode == ParseMode::All
                                    && line_start_abs < window.suppress_current_before
                                {
                                    cursor += line_len;
                                    line_start_abs += line_len as u64;
                                    continue;
                                }
                                let results = parse_json_exit_results(&output);
                                for (index, (exit, head)) in results.into_iter().enumerate() {
                                    if exit == 0 {
                                        continue;
                                    }
                                    let cmd = pc
                                        .commands
                                        .get(index)
                                        .map(String::as_str)
                                        .unwrap_or(&pc.cmd);
                                    let (session_id, cwd, repo) = session_tuple(session);
                                    let sig = Signal::new(
                                        pc.ts.clone(),
                                        repo,
                                        cwd,
                                        Some(HARNESS.into()),
                                        cmd,
                                        exit,
                                        Some(&head),
                                        Some(session_id.clone()),
                                    );
                                    let file_id = crate::signal::filename_session(&session_id);
                                    if append_signal(HARNESS, &file_id, &sig).is_ok() {
                                        emitted += 1;
                                    } else {
                                        pending.insert(call_id.clone(), pc.clone());
                                        emit_failed = true;
                                        fail_offset = Some(line_start_abs);
                                        break;
                                    }
                                }
                                if emit_failed {
                                    break;
                                }
                            }
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

fn output_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn extract_process_id(payload: &Value) -> Option<String> {
    let arguments = payload.get("arguments")?.as_str()?;
    let parsed: Value = serde_json::from_str(arguments).ok()?;
    match parsed.get("session_id")? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_running_session(output: &str) -> Option<String> {
    const NEEDLE: &str = "Process running with session ID ";
    let rest = output.split_once(NEEDLE)?.1;
    let id: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect();
    (!id.is_empty()).then_some(id)
}

fn extract_custom_exec_commands(payload: &Value) -> Vec<String> {
    let Some(input) = payload.get("input").and_then(Value::as_str) else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    let mut remaining = input;
    const NEEDLE: &str = "tools.exec_command";
    while let Some(index) = remaining.find(NEEDLE) {
        let after = &remaining[index + NEEDLE.len()..];
        let boundary = after.find("tools.").unwrap_or(after.len());
        let call = &after[..boundary];
        if let Some(command) = extract_js_string_property(call, "cmd") {
            commands.push(truncate(&command, CMD_MAX));
        }
        remaining = &after[boundary..];
        if boundary == after.len() {
            break;
        }
    }
    commands
}

fn extract_js_string_property(source: &str, key: &str) -> Option<String> {
    let unquoted = format!("{key}:");
    let quoted = format!("\"{key}\":");
    let start = source
        .find(&unquoted)
        .map(|index| index + unquoted.len())
        .or_else(|| source.find(&quoted).map(|index| index + quoted.len()))?;
    let text = source[start..].trim_start();
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut escaped = false;
    let mut end = None;
    for (index, character) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            end = Some(index + character.len_utf8());
            break;
        }
    }
    let literal = &text[..end?];
    if quote == '"' {
        serde_json::from_str(literal).ok()
    } else {
        let inner = &literal[1..literal.len() - 1];
        Some(
            inner
                .replace("\\'", "'")
                .replace("\\\\", "\\")
                .replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\t", "\t"),
        )
    }
}

fn parse_json_exit_results(output: &str) -> Vec<(i32, String)> {
    // Current tool output can contain several text blocks. Status text can
    // precede the JSON result, including another empty `Output:` section.
    // Try each object or array boundary and accept the first JSON value that
    // contains command results.
    for (index, character) in output.char_indices() {
        if !matches!(character, '{' | '[') {
            continue;
        }
        let mut stream = serde_json::Deserializer::from_str(&output[index..]).into_iter::<Value>();
        let Some(Ok(value)) = stream.next() else {
            continue;
        };
        let mut results = Vec::new();
        collect_exit_results(&value, &mut results);
        if !results.is_empty() {
            return results;
        }
    }
    Vec::new()
}

fn collect_exit_results(value: &Value, results: &mut Vec<(i32, String)>) {
    match value {
        Value::Object(object) => {
            if let Some(exit) = object
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
            {
                let head = object
                    .get("output")
                    .and_then(Value::as_str)
                    .map(extract_output_head)
                    .unwrap_or_default();
                results.push((exit, head));
                return;
            }
            for child in object.values() {
                collect_exit_results(child, results);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_exit_results(item, results);
            }
        }
        _ => {}
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
    crate::util::parse_leading_i32(&output[idx + NEEDLE.len()..])
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
