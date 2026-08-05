//! Filesystem path resolution.
//!
//! The store is private by default and lives under the user's data dir:
//! `$XDG_DATA_HOME/papercuts/` or `~/.local/share/papercuts/`. Harness
//! global-instructions files live under `$HOME` (e.g. `~/.claude/CLAUDE.md`).
//!
//! Tests override these by setting `XDG_DATA_HOME` / `HOME` to a fake dir —
//! never the real ones.

use std::env;
use std::path::PathBuf;

/// The user's home directory from `$HOME`.
///
/// A relative or empty `$HOME` is treated as unset: a relative home would
/// resolve the private store against the cwd and could land it inside a git
/// repo. Only an absolute home is honored.
pub fn home_dir() -> Option<PathBuf> {
    let h = env::var_os("HOME")?;
    if h.is_empty() {
        return None;
    }
    let p = PathBuf::from(h);
    if !p.is_absolute() {
        return None;
    }
    Some(p)
}

/// Root of the central papercut store, or `None` if no home data dir can be
/// resolved.
///
/// We deliberately do NOT fall back to a cwd-relative path when `$HOME` and
/// `$XDG_DATA_HOME` are both unset (or relative): that could land the private
/// store inside a git repo and get it committed. A relative `$XDG_DATA_HOME`
/// is likewise ignored for the same reason. Instead, callers handle `None`
/// explicitly — the CLI surfaces an honest error (exit 1); the live hook path
/// silently no-ops, since a missing store beats an insecure one.
pub fn data_root() -> Option<PathBuf> {
    if let Some(x) = env::var_os("XDG_DATA_HOME") {
        if !x.is_empty() {
            let p = PathBuf::from(&x);
            if p.is_absolute() {
                return Some(p.join("papercuts"));
            }
        }
    }
    if let Some(h) = home_dir() {
        return Some(h.join(".local").join("share").join("papercuts"));
    }
    None
}

/// Resolve a path that may be `.` (current repo) or `all` against a base.
pub fn resolve_repo_filter(spec: Option<&str>, current_repo: Option<&str>) -> RepoScope {
    match spec {
        None => match current_repo {
            // Default: scope to the current repo when inside one, else global.
            Some(r) => RepoScope::One(r.to_string()),
            None => RepoScope::All,
        },
        Some(".") => match current_repo {
            Some(r) => RepoScope::One(r.to_string()),
            None => RepoScope::All,
        },
        Some("all") | Some("*") => RepoScope::All,
        Some(other) => RepoScope::One(other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub enum RepoScope {
    All,
    One(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    // env vars are process-global; serialize tests that mutate them.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn data_root_none_when_home_and_xdg_unset() {
        let _g = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let old_home = env::var_os("HOME");
        let old_xdg = env::var_os("XDG_DATA_HOME");
        env::remove_var("XDG_DATA_HOME");
        env::remove_var("HOME");
        assert!(
            data_root().is_none(),
            "no cwd-relative fallback — None when home is unset"
        );
        // An empty value is treated as unset too.
        env::set_var("HOME", "");
        assert!(data_root().is_none());
        // Restore.
        match old_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }
        match old_xdg {
            Some(v) => env::set_var("XDG_DATA_HOME", v),
            None => env::remove_var("XDG_DATA_HOME"),
        }
    }

    /// A relative `XDG_DATA_HOME` must NOT be honored — the private store would
    /// resolve against the cwd and could land inside a git repo.
    #[test]
    fn relative_xdg_is_ignored() {
        let _g = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let old_home = env::var_os("HOME");
        let old_xdg = env::var_os("XDG_DATA_HOME");
        env::set_var("HOME", "/tmp/papercut-abs-home");
        env::set_var("XDG_DATA_HOME", "relative/data");
        let root = data_root().expect("falls back to absolute HOME");
        assert!(
            root.starts_with("/tmp/papercut-abs-home"),
            "relative XDG must be ignored, got {}",
            root.display()
        );
        // A relative HOME is treated as unset too.
        env::set_var("HOME", "relative/home");
        env::remove_var("XDG_DATA_HOME");
        assert!(data_root().is_none(), "relative HOME yields no store");
        match old_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }
        match old_xdg {
            Some(v) => env::set_var("XDG_DATA_HOME", v),
            None => env::remove_var("XDG_DATA_HOME"),
        }
    }
}
