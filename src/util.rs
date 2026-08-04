//! Small text utilities. The signal path truncates aggressively so a runaway
//! command or stderr burst can never bloat the store.

/// Maximum characters kept from a command string.
pub const CMD_MAX: usize = 200;
/// Maximum lines kept from stderr.
pub const STDERR_MAX_LINES: usize = 5;
/// Hard character cap on the stderr head, even across the kept lines.
pub const STDERR_MAX_CHARS: usize = 800;

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
