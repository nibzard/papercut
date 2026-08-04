//! Git context inference for events.
//!
//! Every git operation degrades gracefully: a non-repo cwd, a missing remote,
//! or an unborn HEAD never fails `add`. We capture only pointers — normalized
//! remote (or repo root), repo-relative cwd, short sha — never file contents,
//! env values, or transcripts.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct GitMeta {
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub git_sha: Option<String>,
}

/// Infer repo / cwd / sha for the directory `start`.
pub fn gather(start: &Path) -> GitMeta {
    let toplevel = git(start, &["rev-parse", "--show-toplevel"]);

    let toplevel = match toplevel {
        Some(t) => PathBuf::from(t),
        None => return GitMeta::default(), // not a git repo
    };

    let remote = git(start, &["remote", "get-url", "origin"]);
    let repo = remote
        .as_deref()
        .map(normalize_remote)
        .filter(|r| !r.is_empty())
        .or_else(|| toplevel.to_str().map(|s| s.to_string()));

    let cwd = std::env::current_dir().ok().and_then(|c| {
        c.strip_prefix(&toplevel).ok().map(|rel| {
            let s = rel.to_string_lossy().to_string();
            if s.is_empty() {
                ".".to_string()
            } else {
                s
            }
        })
    });

    let git_sha = git(start, &["rev-parse", "--short", "HEAD"]);

    GitMeta { repo, cwd, git_sha }
}

/// Run `git` in `start`, returning trimmed stdout on success.
fn git(start: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(start)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Normalize a git remote URL to a stable `host/path` form.
///
/// Handles scp-like (`git@host:path`), `ssh://`, `https://`, `http://`,
/// `git://`, userinfo, ports, and trailing `.git`.
pub fn normalize_remote(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    while s.ends_with(".git") {
        s.truncate(s.len() - 4);
    }
    while s.ends_with('/') {
        s.pop();
    }

    let mut had_scheme = false;
    for scheme in ["ssh://", "https://", "http://", "git://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            had_scheme = true;
            break;
        }
    }

    // Strip leading userinfo (e.g. "git@").
    if let Some(at) = s.find('@') {
        let first_slash = s.find('/').unwrap_or(s.len());
        if at < first_slash {
            s = s[at + 1..].to_string();
        }
    }

    let first_slash = s.find('/').unwrap_or(s.len());
    let (host_port, path) = s.split_at(first_slash);

    // For scp-like syntax (no scheme) the colon in host_port separates host
    // from the path; fold it into the path.
    let mut path = path.to_string();
    if !had_scheme && host_port.contains(':') {
        let colon = host_port.find(':').unwrap();
        let path_start = &host_port[colon + 1..];
        path = format!("/{path_start}{path}");
    }

    let host = host_port.split(':').next().unwrap_or(host_port);
    let host = host.to_lowercase();
    let path = path.trim_matches('/').to_string();
    if path.is_empty() {
        host
    } else {
        format!("{host}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scp_like() {
        assert_eq!(
            normalize_remote("git@github.com:foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn https() {
        assert_eq!(
            normalize_remote("https://github.com/foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn ssh_with_port_and_userinfo() {
        assert_eq!(
            normalize_remote("ssh://git@github.com:22/foo/bar.git"),
            "github.com/foo/bar"
        );
    }

    #[test]
    fn nested_groups() {
        assert_eq!(
            normalize_remote("git@gitlab.com:group/sub/repo.git"),
            "gitlab.com/group/sub/repo"
        );
    }

    #[test]
    fn host_lowercased_path_preserved() {
        assert_eq!(
            normalize_remote("https://GitHub.com/Foo/Bar.git"),
            "github.com/Foo/Bar"
        );
    }

    #[test]
    fn no_dot_suffix() {
        assert_eq!(
            normalize_remote("https://github.com/foo/bar"),
            "github.com/foo/bar"
        );
    }
}
