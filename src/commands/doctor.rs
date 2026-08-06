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
        let has_block = std::fs::read_to_string(&rec)
            .ok()
            .and_then(|c| managed_block::detect_version(&c))
            .is_some();
        if has_block {
            checks.push(Check {
                name: format!("block:{}:recorded", entry.harness),
                ok: false,
                detail: format!("orphaned managed block at recorded path {}", rec.display()),
                hint: format!(
                    "run: papercut uninstall --harness {}, then papercut install --yes",
                    entry.harness
                ),
            });
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
        if !c.ok && !c.hint.is_empty() {
            out.push_str(&format!("        hint: {}\n", c.hint));
        }
    }
    out.trim_end().to_string()
}
