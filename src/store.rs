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
    create_private_dir(&root)?;
    create_private_dir(&root.join("events"))?;
    create_private_dir(&root.join("signals"))?;
    Ok(())
}

/// Create `path` (and parents) with mode 0700 on unix, so the private store is
/// owner-only by default, not just private-by-location. Existing dirs are left
/// as-is (a user may have intentionally widened them).
#[cfg(unix)]
pub(crate) fn create_private_dir(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("create {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))
}

/// Write `bytes` to `final_path` atomically: temp file in the same directory,
/// then rename. Rename is atomic on the same filesystem, so a process killed
/// mid-write leaves no partial file and a concurrent reader sees either the
/// old or the new content, never a torn one. The temp name carries the pid so
/// concurrent writers (e.g. racing installs) never clobber each other's temp
/// file. On failure the temp file is removed (best-effort) so it cannot
/// accumulate.
pub(crate) fn write_atomic(final_path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let name = final_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("atomic write target has no file name")?;
    let tmp_path = final_path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp_path, bytes) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e).with_context(|| format!("write temp file {}", tmp_path.display()));
    }
    if let Err(e) = std::fs::rename(&tmp_path, final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e).with_context(|| format!("commit {}", final_path.display()));
    }
    Ok(())
}

/// Atomically write an event to `events/<id>.json` (pretty JSON).
pub fn write_event(event: &Event) -> anyhow::Result<PathBuf> {
    ensure_store()?;
    let dir = events_dir().context("no home data dir: set $HOME or $XDG_DATA_HOME")?;
    let final_path = dir.join(format!("{}.json", event.id));
    let bytes = serde_json::to_vec_pretty(event).context("serialize event")?;
    write_atomic(&final_path, &bytes)?;
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
    // Atomic: a torn config.json would lose install bookkeeping and make
    // doctor/uninstall misreport state.
    write_atomic(&path, &bytes).context("write config.json")?;
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
    /// Per-file progress: session-file path relative to the harness sessions
    /// root → its mark. Every file keeps its own offset, pending calls, and
    /// session context, so parallel sessions never lose appended content and a
    /// call/output pair split across sweeps is matched whenever the output
    /// arrives. Relative keys survive a `$HOME` relocation; legacy absolute
    /// keys are folded to relative on load (see `codex::relativize_legacy_keys`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileMark>,

    // Legacy single-file watermark, kept for read compatibility only.
    // `codex::sweep` folds these fields into `files` once, then clears them;
    // they are never written again. (The old `last_ts` field was dead and is
    // dropped entirely — unknown fields are ignored on read.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_path: Option<String>,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub last_offset: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_calls: BTreeMap<String, PendingCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session: Option<SessionCtx>,
}

fn u64_is_zero(n: &u64) -> bool {
    *n == 0
}

/// Sweep progress within one session file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileMark {
    /// Byte offset where the next sweep resumes.
    #[serde(default)]
    pub offset: u64,
    /// `function_call`s in this file whose `function_call_output` has not
    /// arrived yet. Persisted so the pair is matched when the output lands,
    /// even after other files were swept in between.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_calls: BTreeMap<String, PendingCall>,
    /// The `session_meta` context seen in this file, so a resumed file still
    /// knows its session id / cwd / repo even though `session_meta` lives
    /// above the resume offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionCtx>,
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
    // Atomic so a torn write cannot corrupt the watermark file — a corrupt
    // sweeps.json falls back to defaults and re-sweeps all history.
    write_atomic(&path, &bytes).context("write sweeps.json")?;
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
    use crate::test_env::EnvGuard;

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

    /// Removes its temp tree on drop, including when the test body panics — so a
    /// failing `with_store` test cannot leak a `pc-test-*` dir under `$TMP`.
    /// (EnvGuard already restores the env vars on panic; this closes the fs side.)
    struct TmpGuard(std::path::PathBuf);
    impl Drop for TmpGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Run `f` against a fresh, isolated store under a temp XDG_DATA_HOME.
    /// The guards restore the prior HOME/XDG values AND remove the temp tree,
    /// even when `f` panics.
    fn with_store<F: FnOnce()>(f: F) {
        let _g = EnvGuard::acquire(&["HOME", "XDG_DATA_HOME"]);
        let tmp = std::env::temp_dir().join(format!("pc-test-{}", crate::id::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let _cleanup = TmpGuard(tmp);
        f();
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
    fn legacy_sweep_mark_shape_deserializes() {
        // The pre-per-file shape, including the removed last_ts field.
        let json = r#"{"schema_version":1,"marks":{"codex":{"last_ts":"2026-08-04T00:00:00Z","last_path":"/tmp/a.jsonl","last_offset":42,"pending_calls":{"c1":{"cmd":"x","ts":"t"}}}}}"#;
        let s: Sweeps = serde_json::from_str(json).unwrap();
        let m = &s.marks["codex"];
        assert_eq!(m.last_path.as_deref(), Some("/tmp/a.jsonl"));
        assert_eq!(m.last_offset, 42);
        assert_eq!(m.pending_calls.len(), 1);
        assert!(m.files.is_empty());
    }

    #[test]
    fn sweep_mark_roundtrips_per_file_map() {
        let mut m = SweepMark::default();
        m.files.insert(
            "/tmp/a.jsonl".into(),
            FileMark {
                offset: 7,
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("last_ts"), "dead field stays gone");
        assert!(!json.contains("last_path"), "cleared legacy fields skipped");
        let back: SweepMark = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files["/tmp/a.jsonl"].offset, 7);
    }

    #[test]
    fn write_sweeps_leaves_no_tmp_file() {
        with_store(|| {
            let mut s = Sweeps::default();
            s.marks.insert("codex".into(), SweepMark::default());
            write_sweeps(&s).unwrap();
            assert!(read_sweeps().marks.contains_key("codex"));
            let dir = sweeps_path().unwrap().parent().unwrap().to_path_buf();
            let tmp_left = std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
            assert!(!tmp_left, "atomic write cleans up its temp file");
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
                    detail: "PostToolUseFailure Bash".into(),
                }),
            });
            write_config(&cfg).unwrap();
            let back = read_config();
            assert_eq!(back.installed.len(), 1);
            assert_eq!(back.installed[0].harness, "claude-code");
        });
    }
}
