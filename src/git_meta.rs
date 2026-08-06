//! Git context inference for events.
//!
//! Every git operation degrades gracefully: a non-repo cwd, a missing remote,
//! or an unborn HEAD never fails `add`. We capture only pointers — normalized
//! remote (or repo root), repo-relative cwd, short sha — never file contents,
//! env values, or transcripts.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct GitMeta {
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub git_sha: Option<String>,
    /// Absolute path to the working-tree root (`git rev-parse --show-toplevel`),
    /// so callers can place files at the repo root regardless of the cwd they
    /// were invoked from. `None` outside a repo.
    pub toplevel: Option<PathBuf>,
}

/// Infer repo / cwd / sha for the directory `start`.
///
/// `cwd` is derived relative to `start` (not the papercut process's cwd), so
/// the result is correct for whatever directory the caller names. Today both
/// callers (`add`, `render`) pass the process cwd as `start`; deriving `cwd`
/// from `start` rather than re-reading the process cwd keeps that honest if a
/// future caller passes something else. Use [`repo_of`] when only the repo id
/// is needed: it skips the unused `git_sha` subprocess and is the right choice
/// for hot paths (the live hook, the codex sweep) — which is why those call
/// `repo_of`, not `gather`.
pub fn gather(start: &Path) -> GitMeta {
    let Some(toplevel) = toplevel_of(start) else {
        return GitMeta::default(); // not a git repo
    };
    let repo = repo_id_from(start, &toplevel);

    // Repo-relative cwd for `start`. `start` may be relative; if it can't be
    // expressed against the absolute `toplevel`, leave cwd unset rather than
    // silently substituting the process cwd.
    let cwd = start.strip_prefix(&toplevel).ok().map(|rel| {
        let s = rel.to_string_lossy().into_owned();
        if s.is_empty() {
            ".".to_string()
        } else {
            s
        }
    });

    let git_sha = git(start, &["rev-parse", "--short", "HEAD"]);

    GitMeta {
        repo,
        cwd,
        git_sha,
        toplevel: Some(toplevel),
    }
}

/// The normalized repo id for `start` only — `toplevel` + `remote get-url
/// origin`, two local git queries, no `git_sha`. This is the lean resolver for
/// hot paths: the live hook fires once per failed Bash command and needs only
/// `.repo`, so paying for an unused `rev-parse --short HEAD` on every failure
/// is waste. [`gather`] remains the full resolver for `add`/`render`.
pub fn repo_of(start: &Path) -> Option<String> {
    let toplevel = toplevel_of(start)?;
    repo_id_from(start, &toplevel)
}

fn toplevel_of(start: &Path) -> Option<PathBuf> {
    git(start, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn repo_id_from(start: &Path, toplevel: &Path) -> Option<String> {
    let remote = git(start, &["remote", "get-url", "origin"]);
    remote
        .as_deref()
        .map(normalize_remote)
        .filter(|r| !r.is_empty())
        .or_else(|| toplevel.to_str().map(|s| s.to_string()))
}

/// Wall-clock ceiling for a single local git query. These queries are
/// sub-millisecond when healthy; the bound exists solely so a pathological git
/// config cannot hang the caller — see [`git`].
const GIT_DEADLINE: Duration = Duration::from_millis(2000);

/// Run `git` in `start`, returning trimmed stdout on success.
///
/// These are local-only queries (`rev-parse`, `remote get-url`): they read
/// `.git` refs and config and never `fetch`/`push`, so they do not invoke
/// credential helpers or block on the network the way remote operations can.
/// Any non-zero exit or spawn failure degrades to `None`.
///
/// A [`GIT_DEADLINE`] wall-clock ceiling is enforced with a std-only
/// spawn / poll / kill loop: a child still running past the deadline is sent
/// SIGKILL and the call degrades to `None` *without* blocking on `wait`. The
/// kill-without-wait matters because a git stuck in uninterruptible sleep
/// (D-state) ignores SIGKILL until its blocked syscall returns; a blocking
/// `wait` would then hang past the deadline — re-introducing exactly the hang
/// the ceiling exists to bound. The dropped child becomes a transient zombie
/// reaped by init when the short-lived calling process exits; a zombie cannot
/// stall the hook path. This is necessary because `git` reads config includes
/// eagerly on every command, so an `include.path` / `includeIf.*.path` (or
/// `GIT_CONFIG_GLOBAL` / `GIT_CONFIG_COUNT`/`_KEY`/`_VALUE`) pointing at a
/// blocking local file — a FIFO held open by a stalled writer, a stalled NFS
/// automount — can hang even `rev-parse`/`get-url` indefinitely, and that hang
/// is neither a non-zero exit nor a spawn failure, so degrade-to-`None` alone
/// does not cover it. This matters most on the hot hook path, which fires once
/// per failed Bash command and must never stall the harness's hook dispatch. A
/// correct std-only timeout needs no extra thread or crate, so none is added.
///
/// Stdout is read only after the child exits, not drained concurrently (a
/// concurrent drain would require a thread). The helper is therefore sized for
/// the small bounded output these queries produce — a path, a remote URL, a
/// 12-byte sha. Output exceeding the OS pipe buffer (~64 KiB) would block git on
/// its write syscall so it never exits, hit the deadline, and degrade to `None`;
/// that graceful degradation matches the adapter contract, and no current caller
/// approaches the bound. Adding a caller with bulk stdout would need a drain
/// thread first.
fn git(start: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(start)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GIT_DEADLINE;
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    // Kill but do NOT blocking-wait. A git stuck in uninterruptible
                    // sleep (D-state) — exactly the stalled hard-NFS-automount /
                    // held-FIFO case GIT_DEADLINE exists for — ignores SIGKILL until
                    // its blocked syscall returns, which may be never; wait() would
                    // then block past the deadline and re-introduce the hang. So we
                    // queue SIGKILL and drop the child without waiting: std's
                    // `Child::drop` never blocks on wait. The child becomes a
                    // transient zombie reaped by init when the (short-lived) calling
                    // process exits; it cannot stall this thread. try_wait on the
                    // normal path reaps, so this is the only path that can leave a
                    // zombie.
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    if !status.success() {
        return None;
    }
    // The child has exited; the piped stdout buffer still holds its output.
    let mut buf = Vec::new();
    child.stdout.take()?.read_to_end(&mut buf).ok()?;
    // Lossy decode: on a byte-oriented filesystem `git rev-parse --show-toplevel`
    // emits the raw path bytes, which may not be valid UTF-8 (a Latin-1-named
    // ancestor directory, a non-UTF-8 locale). `read_to_string` errors on that
    // and would return None — and since `toplevel_of` is the first thing
    // `gather`/`repo_of` check, a non-UTF-8 repo path would forfeit repo, cwd,
    // AND git_sha for the event. Decoding lossily keeps the event attributable.
    let s = String::from_utf8_lossy(&buf);
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
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
