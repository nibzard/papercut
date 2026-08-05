//! Small text utilities. The signal path truncates aggressively so a runaway
//! command or stderr burst can never bloat the store.

/// Maximum characters kept from a command string.
pub const CMD_MAX: usize = 200;
/// Maximum lines kept from stderr.
pub const STDERR_MAX_LINES: usize = 5;
/// Hard character cap on the stderr head, even across the kept lines.
pub const STDERR_MAX_CHARS: usize = 800;
/// Maximum characters kept from any other single string field (repo, cwd,
/// session, agent). Keeps a runaway path or id from bloating the store.
pub const FIELD_MAX: usize = 512;
/// Maximum bytes read from a hook's stdin. Real payloads are tiny; anything
/// larger is truncated (which then fails to parse → the hook records nothing
/// and exits 0). Bounds memory so a runaway stdin can never OOM the hook into
/// a non-zero exit — the hook path must always be silent and infallible.
pub const STDIN_MAX: usize = 1024 * 1024;
/// Maximum quarantined-file entries listed inline in any single projection.
/// Keeps a store with many corrupt files from bloating `list`/`render`/triage
/// output; the rest are summarized as "+N more".
pub const SKIPPED_LIST_MAX: usize = 20;
/// Maximum characters kept from a skipped-file reason. OS/serde error strings
/// are not bounded at capture; this bounds them at projection time.
pub const SKIPPED_REASON_MAX: usize = 160;

/// Truncate `s` to at most `max` characters on a UTF-8 boundary, appending an
/// ellipsis when truncation occurs. Never panics on non-UTF-8 boundaries.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut end = 0usize;
    for (i, (b, _)) in s.char_indices().enumerate() {
        if i == max {
            end = b;
            break;
        }
    }
    let mut out = s[..end].to_string();
    out.push('…');
    out
}

/// Keep the first `n` lines of `s`. The trailing newline is stripped.
pub fn first_n_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
}

/// Truncate then cap total length to `chars`, on a UTF-8 boundary.
pub fn cap_chars(s: &str, chars: usize) -> String {
    truncate(s, chars)
}
