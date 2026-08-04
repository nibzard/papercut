//! Shared helpers for integration tests.
//!
//! `std::env` is process-global, so every test that points the store or harness
//! detection at a fake HOME/XDG must run serialized within a test binary. We
//! hold a global mutex for the test's lifetime and point both vars at a unique
//! temp tree, restoring (clearing) them on drop.
//!
//! This module is compiled into every integration test binary, only some of
//! which use every helper — so unused items here are expected, not dead code.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    LOCK.get_or_init(Mutex::default)
}

/// An isolated HOME + XDG_DATA_HOME pair. Hold it for the test body.
pub struct IsolatedEnv {
    pub home: PathBuf,
    pub data: PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl IsolatedEnv {
    pub fn new() -> Self {
        let guard = env_lock().lock().unwrap();
        let base = std::env::temp_dir().join(format!("pc-it-{}", papercut::id::new_id()));
        let home = base.join("home");
        let data = base.join("data");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        // Harness detection looks under $HOME; the store under $XDG_DATA_HOME.
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_DATA_HOME", &data);
        Self {
            home,
            data,
            _guard: guard,
        }
    }

    /// Create the fake harness config dir so `install` detects claude-code.
    pub fn with_claude(self) -> Self {
        std::fs::create_dir_all(self.home.join(".claude")).unwrap();
        self
    }

    /// Create the fake codex sessions root.
    pub fn with_codex(self) -> Self {
        std::fs::create_dir_all(self.home.join(".codex").join("sessions")).unwrap();
        self
    }
}

impl Default for IsolatedEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        std::env::remove_var("XDG_DATA_HOME");
        std::env::remove_var("HOME");
        if let Some(parent) = self.home.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

/// Path to the built papercut binary (set by cargo for integration tests).
pub fn bin() -> String {
    env!("CARGO_BIN_EXE_papercut").to_string()
}

/// A minimal valid event for seeding a store in tests.
pub fn test_event(id: &str, summary: &str) -> papercut::model::Event {
    use papercut::model::*;
    Event {
        schema_version: 1,
        id: id.into(),
        created_at: "2026-08-04T20:42:00Z".into(),
        source: Source::InMoment,
        status: Status::Open,
        summary: summary.into(),
        hypothesis: None,
        suggested_fix: None,
        category: None,
        context: EventContext::default(),
        resolution: None,
    }
}
