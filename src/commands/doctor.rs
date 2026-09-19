//! `papercut doctor` — verify store, managed blocks, and adapter wiring.
//!
//! Doctor is a diagnostic that reports health in `data.healthy` and each
//! finding with a deterministic remediation hint. Its exit code is honest about
//! the result: 0 when every check passes, 1 when something is wrong — so a
//! script can gate on it. The checks payload always rides in the JSON envelope
//! (success-shaped), so an agent never loses the detail behind a non-zero exit.

use crate::app::RunResult;
use crate::harness::detect;
use crate::managed_block;
use crate::store;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Check {
    name: String,
    ok: bool,
    detail: String,
    hint: String,
}

pub fn run() -> RunResult {
    let mut checks: Vec<Check> = Vec::new();

    // 1. Store exists and is writable.
    checks.push(check_store());
    checks.push(check_store_permissions());

    // 1b. Orphaned .tmp files in events/ — leftovers from an interrupted atomic
    // write. They never corrupt reads (is_event_file excludes them) but they
    // accumulate; surface them so a human can clean up. Reported, never deleted
    // by doctor.
    checks.push(check_orphaned_tmp());

    // 2. Agent detection — informational only: `unknown` is an acceptable
    // outcome by design, so there is no failing state to gate on.
    let info = crate::detect::detect();
    checks.push(Check {
        name: "agent_detection".into(),
        ok: true,
        detail: format!(
            "informational: agent={} session={}",
            info.agent,
            info.session.as_deref().unwrap_or("-")
        ),
        hint: String::new(),
    });

    // 3. Per-harness managed block + adapter wiring.
    let detected = detect();
    for h in &detected {
        checks.push(check_block(format!("block:{}", h.id), &h.instructions_file));

        if h.id == "claude-code" {
            checks.push(check_claude_adapter());
        }
        if h.id == "codex" {
            // Informational: an empty sessions dir is normal on a fresh
            // Codex install, so absence is reported honestly, not failed.
            let detail = match crate::adapters::codex::sessions_root() {
                Some(root) if root.exists() => {
                    format!("informational: sessions dir present ({})", root.display())
                }
                Some(root) => format!(
                    "informational: sessions dir not found yet ({}); sweep warns until Codex records a session",
                    root.display()
                ),
                None => "informational: no home dir".into(),
            };
            checks.push(Check {
                name: "adapter:codex".into(),
                ok: true,
                detail,
                hint: String::new(),
            });
            checks.push(check_codex_freshness());
            checks.push(check_codex_format());
        }
    }

    // 4. Recorded install paths that detection no longer covers. A managed
    // block orphaned at a previously recorded path (config-dir drift) is
    // invisible to the per-harness checks above and would linger forever.
    let cfg = store::read_config();
    let detected_files: Vec<&PathBuf> = detected.iter().map(|h| &h.instructions_file).collect();
    for entry in &cfg.installed {
        let rec = PathBuf::from(&entry.instructions_file);
        if detected_files.iter().any(|f| **f == rec) {
            continue;
        }
        // An unreadable recorded file (permission denied, invalid UTF-8) gets
        // an honest diagnosis — consistent with check_block — instead of being
        // swallowed as "no block". A missing file is simply nothing to flag.
        match std::fs::read_to_string(&rec) {
            Ok(content) => {
                if managed_block::detect_version(&content).is_some() {
                    checks.push(Check {
                        name: format!("block:{}:recorded", entry.harness),
                        ok: false,
                        detail: format!(
                            "orphaned managed block at recorded path {}",
                            rec.display()
                        ),
                        hint: format!(
                            "run: papercut uninstall --harness {}, then papercut install --yes",
                            entry.harness
                        ),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                checks.push(Check {
                    name: format!("block:{}:recorded", entry.harness),
                    ok: false,
                    detail: format!("cannot read recorded path {}: {e}", rec.display()),
                    hint: "fix the file's permissions or encoding by hand".into(),
                });
            }
        }
    }

    let healthy = checks.iter().all(|c| c.ok);
    let data = json!({
        "healthy": healthy,
        "store": crate::paths::data_root()
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<no home data dir>".to_string()),
        "checks": checks.iter().map(|c| json!({
            "name": c.name,
            "ok": c.ok,
            "detail": c.detail,
            "hint": c.hint,
        })).collect::<Vec<Value>>(),
    });
    let text = format_doctor(&checks, healthy);
    // Health routes the full checks payload through the success envelope while
    // still exiting 1 when anything failed.
    RunResult::Health {
        data,
        text,
        healthy,
    }
}

/// Check one instructions file for the current managed block. An unreadable
/// file gets its own diagnosis: hinting "run install" there would loop,
/// because install refuses to modify a file it cannot read.
fn check_block(name: String, path: &Path) -> Check {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let v = managed_block::detect_version(&content);
            let ok = v == Some(managed_block::BLOCK_VERSION);
            Check {
                name,
                ok,
                detail: format!("version {v:?}"),
                hint: if ok {
                    String::new()
                } else {
                    "run: papercut install --yes".into()
                },
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Check {
            name,
            ok: false,
            detail: "instructions file missing".into(),
            hint: "run: papercut install --yes".into(),
        },
        Err(e) => Check {
            name,
            ok: false,
            detail: format!("cannot read {}: {e}", path.display()),
            hint: "fix the file's permissions or encoding by hand; install refuses to modify a file it cannot read".into(),
        },
    }
}

/// Check the Claude Code hook wiring. A corrupt settings.json masks as "hook
/// missing" through the lenient read path; diagnose the corruption instead
/// of hinting at install, which refuses to touch an unparseable file.
fn check_claude_adapter() -> Check {
    if let Some(reason) = crate::adapters::claude_code::settings_diagnosis() {
        return Check {
            name: "adapter:claude-code".into(),
            ok: false,
            detail: reason,
            hint: "back up and repair ~/.claude/settings.json by hand; install refuses to overwrite it".into(),
        };
    }
    let s = crate::adapters::claude_code::hook_status();
    let ok = s.present && s.exe_ok;
    let detail = match (s.present, s.exe_ok) {
        (true, true) => format!(
            "PostToolUseFailure Bash hook present (exe: {})",
            s.exe.as_deref().unwrap_or("?")
        ),
        (true, false) => format!(
            "hook points at missing or non-executable: {}",
            s.exe.as_deref().unwrap_or("?")
        ),
        (false, _) => "hook missing".into(),
    };
    Check {
        name: "adapter:claude-code".into(),
        ok,
        detail,
        hint: if ok {
            String::new()
        } else if !s.present {
            "run: papercut install --yes".into()
        } else {
            "reinstall papercut, or fix the hook command path".into()
        },
    }
}

/// Count leftover `.tmp` files in events/ (interrupted atomic writes). They are
/// invisible to reads but accumulate; this is informational, reported as a hint
/// — doctor never deletes them.
fn check_orphaned_tmp() -> Check {
    let Some(dir) = crate::store::events_dir() else {
        return Check {
            name: "orphaned_tmp".into(),
            ok: true,
            detail: "no home data dir".into(),
            hint: String::new(),
        };
    };
    let count = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        Err(_) => 0,
    };
    Check {
        name: "orphaned_tmp".into(),
        // Orphaned tmp files are never fatal (reads skip them), so this check
        // is informational: ok regardless of count, with a cleanup hint when any
        // are present.
        ok: true,
        detail: if count == 0 {
            "none".into()
        } else {
            format!("{count} orphaned .tmp file(s) in events/")
        },
        hint: if count == 0 {
            String::new()
        } else {
            "safe to delete: rm ~/.local/share/papercuts/events/*.tmp".into()
        },
    }
}

fn check_store() -> Check {
    let Some(root) = crate::paths::data_root() else {
        return Check {
            name: "store".into(),
            ok: false,
            detail: "no home data dir ($HOME/$XDG_DATA_HOME unset)".into(),
            hint: "set $HOME or $XDG_DATA_HOME".into(),
        };
    };
    if let Err(e) = store::ensure_store() {
        return Check {
            name: "store".into(),
            ok: false,
            detail: format!("cannot create {}: {e}", root.display()),
            hint: "check $XDG_DATA_HOME / $HOME / disk space".into(),
        };
    }
    // Probe writability with a throwaway file.
    let probe = root.join(".doctor_probe");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Check {
                name: "store".into(),
                ok: true,
                detail: format!("writable: {}", root.display()),
                hint: String::new(),
            }
        }
        Err(e) => Check {
            name: "store".into(),
            ok: false,
            detail: format!("unwritable: {e}"),
            hint: "fix permissions on the store dir".into(),
        },
    }
}

#[cfg(unix)]
fn check_store_permissions() -> Check {
    use std::os::unix::fs::PermissionsExt;

    let Some(root) = crate::paths::data_root() else {
        return Check {
            name: "store_permissions".into(),
            ok: false,
            detail: "no home data dir".into(),
            hint: "set $HOME or $XDG_DATA_HOME".into(),
        };
    };
    let mut wide = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(path) = pending.pop() {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.permissions().mode() & 0o077 != 0 {
            wide.push(path.clone());
        }
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
    }
    let ok = wide.is_empty();
    Check {
        name: "store_permissions".into(),
        ok,
        detail: if ok {
            "owner-only".into()
        } else {
            format!("{} path(s) allow group or other access", wide.len())
        },
        hint: if ok {
            String::new()
        } else {
            format!("run: chmod -R go-rwx {}", root.display())
        },
    }
}

#[cfg(not(unix))]
fn check_store_permissions() -> Check {
    Check {
        name: "store_permissions".into(),
        ok: true,
        detail: "not checked on this platform".into(),
        hint: String::new(),
    }
}

fn check_codex_freshness() -> Check {
    let age_days = crate::store::sweeps_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
        .map(|age| age.as_secs() / 86_400);
    let ok = age_days.is_some_and(|days| days <= 7);
    Check {
        name: "adapter:codex:freshness".into(),
        ok,
        detail: age_days
            .map(|days| format!("last sweep state update {days} day(s) ago"))
            .unwrap_or_else(|| "no sweep state found".into()),
        hint: if ok {
            String::new()
        } else {
            "run: papercut sweep; schedule it periodically with cron or a user timer".into()
        },
    }
}

fn check_codex_format() -> Check {
    let coverage = crate::adapters::codex::coverage_probe();
    let ok = coverage.unsupported_exec_records == 0;
    Check {
        name: "adapter:codex:format".into(),
        ok,
        detail: format!(
            "{} supported exec record(s), {} unsupported exec record(s); {} observable and {} opaque custom result(s) in recent logs",
            coverage.supported_exec_records,
            coverage.unsupported_exec_records,
            coverage.observable_custom_results,
            coverage.opaque_custom_results,
        ),
        hint: if !ok {
            "update papercut's Codex adapter against the installed log format".into()
        } else if coverage.opaque_custom_results > 0 {
            "Codex omitted child exit codes from some custom results; run papercut install --yes to refresh the result-preservation instruction".into()
        } else {
            String::new()
        },
    }
}

fn format_doctor(checks: &[Check], healthy: bool) -> String {
    let mut out = String::new();
    out.push_str(if healthy {
        "healthy\n"
    } else {
        "issues found\n"
    });
    for c in checks {
        let mark = if c.ok { "ok  " } else { "FAIL" };
        out.push_str(&format!("[{mark}] {:<22} {}\n", c.name, c.detail));
        if !c.hint.is_empty() {
            let label = if c.ok { "note" } else { "hint" };
            out.push_str(&format!("        {label}: {}\n", c.hint));
        }
    }
    out.trim_end().to_string()
}
