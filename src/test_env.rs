//! One process-wide lock for unit tests that mutate environment variables.
//!
//! `std::env` is process-global and the lib test binary runs threaded. Every
//! module that touches `HOME` / `XDG_DATA_HOME` / agent-detection vars in a
//! test MUST serialize through this ONE lock — a per-module lock does not
//! serialize against other modules, and an interleaved `remove_var` can point
//! a concurrently running test at the developer's real store.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock};

static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Holds the process-wide env lock and restores the named variables to their
/// prior values on drop — including on panic, so a failing test cannot leak
/// its fake environment into the next one.
pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    /// Acquire the lock and snapshot `vars`. A poisoned lock is recovered:
    /// the panicking holder already restored its vars in its own drop, so the
    /// environment is consistent and the poison flag alone must not cascade
    /// unrelated test failures.
    pub(crate) fn acquire(vars: &[&'static str]) -> Self {
        let lock = LOCK
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let saved = vars.iter().map(|v| (*v, std::env::var_os(v))).collect();
        Self { _lock: lock, saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (var, val) in &self.saved {
            match val {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }
}
