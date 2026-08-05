//! The central store: read/write events, config, and sweep high-water marks.
//!
//! Layout (under `data_root()`):
//! ```text
//! events/pc_<ulid>.json     one file per event — no merge conflicts
//! signals/<harness>/<session>.jsonl   (see signal.rs)
//! sweeps.json               per-harness high-water marks for `sweep`
//! config.json               what install wired, for doctor/uninstall
//! ```
//!
//! Events are written atomically (temp file + rename) so a process killed
//! mid-write leaves no partial event. Reading is lenient: malformed event
//! files are skipped and reported, never fatal.

use crate::model::{Event, SCHEMA_VERSION};
use crate::paths::data_root;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn events_dir() -> Option<PathBuf> {
    data_root().map(|r| r.join("events"))
}

/// Ensure the store's directory tree exists.
pub fn ensure_store() -> anyhow::Result<()> {
    let root = data_root().context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    std::fs::create_dir_all(root.join("events")).context("create events dir")?;
    std::fs::create_dir_all(root.join("signals")).context("create signals dir")?;
    Ok(())
}

/// Atomically write an event to `events/<id>.json` (pretty JSON).
pub fn write_event(event: &Event) -> anyhow::Result<PathBuf> {
    ensure_store()?;
    let dir = events_dir().context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    let final_path = dir.join(format!("{}.json", event.id));
    let tmp_path = dir.join(format!("{}.json.tmp", event.id));
    let bytes = serde_json::to_vec_pretty(event).context("serialize event")?;
    std::fs::write(&tmp_path, &bytes)
        .with_context(|| format!("write temp event {}", tmp_path.display()))?;
    // rename is atomic on the same filesystem (temp lives in the same dir).
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("commit event {}", final_path.display()))?;
    Ok(final_path)
}

/// One malformed/unreadable event file encountered while scanning.
///
/// `repo` is the file's repo attribution when it is knowable: a file that
/// parsed but was then quarantined (unknown schema version, or a violated
/// status/resolution invariant) still carries `context.repo`, so its quarantine
/// is attributable. A file that failed to parse or to read has no knowable
/// repo (`None`) — `query::load` therefore excludes unattributable skips from a
/// repo-scoped view rather than leaking every repo's corrupt files into it.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedFile {
    pub file: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl SkippedFile {
    /// The file name plus, when the repo is knowable, a ` [repo]` suffix — so
    /// the human-readable listings (`list`/`render`/`triage-pack`) can tell
    /// attributable quarantines apart under the global view, where several
    /// repos' corrupt files interleave. Unattributable skips (parse/read
    /// failures carry `repo: None`) get the bare name.
    pub fn file_label(&self) -> String {
        match &self.repo {
            Some(r) => format!("{} [{}]", self.file, r),
            None => self.file.clone(),
        }
    }
}

/// Read every event file, sorted by id (≈ chronological). Malformed files are
/// skipped and reported, never panic.
pub fn read_all_events() -> (Vec<Event>, Vec<SkippedFile>) {
    match events_dir() {
        Some(d) => read_events_in(d),
        None => (Vec::new(), Vec::new()),
    }
}

/// Read events whose id (filename) is in `ids`.
pub fn read_events_by_ids(ids: &[String]) -> (Vec<Event>, Vec<SkippedFile>) {
    let (mut all, skipped) = read_all_events();
    let want: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    all.retain(|e| want.contains(e.id.as_str()));
    (all, skipped)
}

fn read_events_in(dir: PathBuf) -> (Vec<Event>, Vec<SkippedFile>) {
    let mut events = Vec::new();
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (events, skipped);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_event_file(&path) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                skipped.push(SkippedFile {
                    file: name,
                    reason: format!("read error: {e}"),
                    repo: None,
                });
                continue;
            }
        };
        match serde_json::from_str::<Event>(&text) {
            Ok(ev) => {
                // A file may parse yet still be unsafe to act on: an unknown
                // schema version (forward-incompatible) or a violated status/
                // resolution invariant. Validate on load and quarantine such
                // events into `skipped` rather than feeding them to triage or
                // render as if they were sound.
                if ev.schema_version != SCHEMA_VERSION {
                    skipped.push(SkippedFile {
                        file: name,
                        reason: format!(
                            "schema version {} not supported (this build reads {})",
                            ev.schema_version, SCHEMA_VERSION
                        ),
                        repo: ev.context.repo.clone(),
                    });
                    continue;
                }
                if let Err(reason) = ev.validate() {
                    skipped.push(SkippedFile {
                        file: name,
                        reason,
                        repo: ev.context.repo.clone(),
                    });
                    continue;
                }
                events.push(ev);
            }
            Err(e) => skipped.push(SkippedFile {
                file: name,
                reason: format!("parse error: {e}"),
                repo: None,
            }),
        }
    }
    events.sort_by(|a, b| a.id.cmp(&b.id));
    // Deterministic skip order (by file name) so `list`/`render`/`triage-pack`
    // surface the same reasons in the same order for a given store — required
    // for render's byte-identical-output guarantee.
    skipped.sort_by(|a, b| a.file.cmp(&b.file));
    (events, skipped)
}

fn is_event_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    name.starts_with(crate::id::ID_PREFIX) && name.ends_with(".json") && !name.ends_with(".tmp")
}

// ── config.json: install bookkeeping ───────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub installed: Vec<InstalledHarness>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledHarness {
    pub harness: String,
    /// Absolute path to the global instructions file holding the managed block.
    pub instructions_file: String,
    pub block_version: u32,
    /// Adapter wiring state; `None` means reports-only (no live adapter).
    pub adapter: Option<AdapterState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterState {
    pub kind: String,
    pub detail: String,
}

pub fn config_path() -> Option<PathBuf> {
    data_root().map(|r| r.join("config.json"))
}

pub fn read_config() -> Config {
    let Some(path) = config_path() else {
        return Config {
            schema_version: SCHEMA_VERSION,
            installed: vec![],
        };
    };
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|_| Config {
            schema_version: SCHEMA_VERSION,
            installed: vec![],
        }),
        Err(_) => Config {
            schema_version: SCHEMA_VERSION,
            installed: vec![],
        },
    }
}

pub fn write_config(cfg: &Config) -> anyhow::Result<()> {
    ensure_store()?;
    let path = config_path().context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    let bytes = serde_json::to_vec_pretty(cfg).context("serialize config")?;
    std::fs::write(path, bytes).context("write config.json")?;
    Ok(())
}

// ── sweeps.json: incremental high-water marks ──────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sweeps {
    pub schema_version: u32,
    #[serde(default)]
    pub marks: BTreeMap<String, SweepMark>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SweepMark {
    /// RFC 3339 ts of the most recent signal extracted, "" if none yet.
    pub last_ts: String,
    /// Absolute path of the last file processed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_path: Option<String>,
    /// Byte offset within `last_path` where the next sweep should resume.
    #[serde(default)]
    pub last_offset: u64,
    /// `function_call`s seen in `last_path` whose `function_call_output` has not
    /// arrived yet, persisted so a call/output pair split across two sweeps is
    /// not silently lost — as long as `last_path` has not advanced to a newer
    /// swept file (see `codex::sweep`'s known limitation: once the watermark
    /// advances past this file the pending calls are dropped and the file is
    /// never revisited, so even a same-file pair can be lost once a newer file
    /// is swept).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_calls: BTreeMap<String, PendingCall>,
    /// The last `session_meta` context seen in `last_path`, so a resumed file
    /// still knows its session id / cwd / repo even though `session_meta` lives
    /// above the resume offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session: Option<SessionCtx>,
}

/// A `function_call` awaiting its `function_call_output`, persisted in the
/// sweep watermark so the pair survives a sweep-boundary split.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingCall {
    pub cmd: String,
    pub ts: String,
}

/// Session-level context extracted from `session_meta`, cached in the watermark
/// so a partially-read file can still emit well-formed signals on resume.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionCtx {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub git_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
}

pub fn sweeps_path() -> Option<PathBuf> {
    data_root().map(|r| r.join("sweeps.json"))
}

pub fn read_sweeps() -> Sweeps {
    let Some(path) = sweeps_path() else {
        return Sweeps::default();
    };
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_default(),
        Err(_) => Sweeps::default(),
    }
}

pub fn write_sweeps(s: &Sweeps) -> anyhow::Result<()> {
    ensure_store()?;
    let path = sweeps_path().context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    let bytes = serde_json::to_vec_pretty(s).context("serialize sweeps")?;
    std::fs::write(path, bytes).context("write sweeps.json")?;
    Ok(())
}

impl Default for Sweeps {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            marks: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventContext, Source, Status};
    use std::sync::{Mutex, OnceLock};

    /// Serialize tests that mutate process-global env vars (XDG_DATA_HOME).
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn mk(id: &str) -> Event {
        Event {
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            created_at: "2026-08-04T20:42:00Z".into(),
            source: Source::InMoment,
            status: Status::Open,
            summary: "s".into(),
            hypothesis: None,
            suggested_fix: None,
            category: None,
            context: EventContext::default(),
            resolution: None,
        }
    }

    /// Run `f` against a fresh, isolated store under a temp XDG_DATA_HOME.
    fn with_store<F: FnOnce()>(f: F) {
        let _g = lock().lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("pc-test-{}", crate::id::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        f();
        std::env::remove_var("XDG_DATA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_and_read_roundtrips() {
        with_store(|| {
            let e = mk("pc_01K0000000000000000000001");
            write_event(&e).unwrap();
            let (got, skipped) = read_all_events();
            assert_eq!(skipped.len(), 0);
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].id, e.id);
        });
    }

    #[test]
    fn malformed_files_are_skipped_not_fatal() {
        with_store(|| {
            write_event(&mk("pc_01K0000000000000000000002")).unwrap();
            // Drop a garbage event file directly.
            std::fs::write(
                events_dir()
                    .unwrap()
                    .join("pc_01KGARBAGE00000000000000.json"),
                "{ not valid json",
            )
            .unwrap();
            let (got, skipped) = read_all_events();
            assert_eq!(got.len(), 1, "valid event still reads");
            assert_eq!(skipped.len(), 1, "garbage skipped");
        });
    }

    #[test]
    fn invariant_violation_is_skipped_on_load() {
        with_store(|| {
            ensure_store().unwrap();
            // A terminal status with no resolution violates the model invariant.
            let mut bad = mk("pc_01K0000000000000000000003");
            bad.status = Status::Fixed;
            bad.resolution = None;
            std::fs::write(
                events_dir().unwrap().join(format!("{}.json", bad.id)),
                serde_json::to_string(&bad).unwrap(),
            )
            .unwrap();
            let (got, skipped) = read_all_events();
            assert!(got.is_empty(), "invalid event is quarantined");
            assert_eq!(skipped.len(), 1);
            assert!(skipped[0].reason.contains("fixed"));
        });
    }

    #[test]
    fn unknown_schema_version_is_skipped_on_load() {
        with_store(|| {
            ensure_store().unwrap();
            let mut fut = mk("pc_01K0000000000000000000004");
            fut.schema_version = SCHEMA_VERSION + 1;
            std::fs::write(
                events_dir().unwrap().join(format!("{}.json", fut.id)),
                serde_json::to_string(&fut).unwrap(),
            )
            .unwrap();
            let (got, skipped) = read_all_events();
            assert!(got.is_empty(), "forward-incompatible event quarantined");
            assert_eq!(skipped.len(), 1);
            assert!(skipped[0].reason.contains("schema version"));
        });
    }

    #[test]
    fn tmp_files_are_ignored() {
        with_store(|| {
            ensure_store().unwrap();
            std::fs::write(
                events_dir()
                    .unwrap()
                    .join("pc_01KTMP0000000000000000A.json.tmp"),
                "{}",
            )
            .unwrap();
            let (got, skipped) = read_all_events();
            assert!(got.is_empty());
            assert!(skipped.is_empty());
        });
    }

    #[test]
    fn config_roundtrips() {
        with_store(|| {
            let mut cfg = read_config();
            cfg.installed.push(InstalledHarness {
                harness: "claude-code".into(),
                instructions_file: "/tmp/x".into(),
                block_version: 1,
                adapter: Some(AdapterState {
                    kind: "claude-code-hook".into(),
                    detail: "PostToolUse Bash".into(),
                }),
            });
            write_config(&cfg).unwrap();
            let back = read_config();
            assert_eq!(back.installed.len(), 1);
            assert_eq!(back.installed[0].harness, "claude-code");
        });
    }
}
