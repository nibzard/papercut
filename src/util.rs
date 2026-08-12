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

/// Parse the ASCII integer at the start of `s` (a leading `-` is allowed).
/// Used to read exit codes out of harness-formatted failure strings.
pub fn parse_leading_i32(s: &str) -> Option<i32> {
    let num: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num.parse::<i32>().ok()
}

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

// ── markdown neutralization for free-text fields ───────────────────────────
//
// Event text (summaries, hypotheses, fixes, signal commands) is data, never
// trusted structure. These helpers keep it from forging markdown: collapsing
// it onto one line, indenting continuation lines off column 0, and fencing
// code spans so backticks cannot break out. All deterministic.

/// Collapse every whitespace run (including newlines) to a single space, so a
/// free-text field rendered one row per event (e.g. `list`) can never forge
/// extra rows from an embedded newline.
pub fn md_single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Prefix every line of `s` after the first with `indent`, so multiline free
/// text rendered under a bullet or section can never start a line at column 0.
/// A forged `## heading` or peer `- item` becomes an indented continuation that
/// stays inside its containing element instead of structuring the document.
/// CRLF/CR are normalized to `\n` first.
pub fn md_indent_continuation(s: &str, indent: &str) -> String {
    let norm = s.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(norm.len() + norm.matches('\n').count() * indent.len());
    for (i, line) in norm.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}

/// Minimum indentation (in spaces) for a continuation line of a free-text
/// field so it can **never** start a new markdown block — an ATX heading, a
/// bullet, a fenced code block, a blockquote, a setext underline, or an HTML
/// block — inside a list item whose text content begins at `content_col`.
///
/// CommonMark lets every one of those constructs begin with up to 3 spaces of
/// indentation *relative to the enclosing list item's content column*. A
/// continuation line indented to `content_col + 4` therefore lands any forged
/// marker at relative column 4, past the 0–3 window: it stays a literal
/// paragraph continuation instead of structuring the document. The render and
/// triage-pack callers indent multiline summaries, hypotheses, fixes, and
/// resolutions to exactly this value for their bullet depth.
pub const fn safe_continuation_indent(content_col: usize) -> usize {
    content_col + 4
}

/// Wrap `s` in a markdown code span using enough backticks to contain the
/// longest backtick run in `s`, so a value containing backticks — a shell
/// command like `` echo `whoami` `` — cannot break out of the span. With no
/// backticks in `s` this is the ordinary single-backtick span, byte-identical
/// to a hand-written `` `s` ``; with backticks the fence grows by one and is
/// space-padded so a leading/trailing backtick cannot merge with it.
pub fn md_code_span(s: &str) -> String {
    // Collapse internal whitespace (newlines included) to one line first. An
    // inline code span cannot span a line break: CommonMark parses blocks
    // before inlines, so a value like `foo\n## evil` would let the second line
    // be parsed as a fresh block (a forged heading or bullet) rather than stay
    // inside the span. Every caller renders an inline single-line value (a
    // command, ref, sha, path, id), so this is the correct display shape and
    // the values two callers already `md_single_line` are unaffected.
    let s = md_single_line(s);
    let mut longest = 0usize;
    let mut run = 0usize;
    for b in s.bytes() {
        if b == b'`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    if longest == 0 {
        return format!("`{s}`");
    }
    let fence = "`".repeat(longest + 1);
    format!("{fence} {s} {fence}")
}

#[cfg(test)]
mod md_tests {
    use super::*;

    #[test]
    fn single_line_collapses_newlines() {
        assert_eq!(md_single_line("a\nb\t c"), "a b c");
        assert_eq!(md_single_line("plain"), "plain");
        assert_eq!(md_single_line("  \n "), "");
    }

    #[test]
    fn indent_continuation_keeps_first_line_off_indent() {
        assert_eq!(md_indent_continuation("a", "  "), "a");
        assert_eq!(
            md_indent_continuation("a\nb\nc", "  "),
            "a\n  b\n  c",
            "only continuation lines gain the indent"
        );
    }

    #[test]
    fn safe_indent_is_past_the_block_window() {
        // CommonMark block starts allow 0..3 spaces relative to the content
        // column, so the safe continuation indent is content column + 4.
        assert_eq!(safe_continuation_indent(2), 6); // `- ` bullet
        assert_eq!(safe_continuation_indent(4), 8); // `  - ` sub-bullet
    }

    #[test]
    fn indent_continuation_defeats_a_forged_heading() {
        // Rendered under a `- ` bullet the summary's content column is 2, so the
        // safe continuation indent is 6 (see `safe_continuation_indent`).
        let indent = " ".repeat(safe_continuation_indent(2));
        let forged = "real\n## evil heading\n- fake peer\n```\n> quote\n---";
        let out = md_indent_continuation(forged, &indent);
        // Every continuation line begins with >= 6 spaces, so under a content
        // column of 2 every forged block marker sits at relative column >= 4 —
        // past CommonMark's 0..3 block-start window. It cannot structure the
        // doc; it can only be literal paragraph text.
        for l in out.lines().skip(1) {
            let leading = l.len() - l.trim_start().len();
            assert!(
                leading >= safe_continuation_indent(2),
                "continuation not past the block window: {l:?}"
            );
        }
        assert!(out.contains("real") && out.contains("evil heading"));
    }

    #[test]
    fn code_span_plain_is_ordinary_single_backticks() {
        // No backticks in input → single-backtick span (byte-identical to `` `x` ``).
        assert_eq!(md_code_span("abc1234"), "`abc1234`");
    }

    #[test]
    fn code_span_with_one_backtick_uses_two() {
        // A command containing a single backtick run is fenced with two.
        let span = md_code_span("echo `whoami`");
        assert!(span.starts_with("`` "), "double-backtick fence: {span}");
        assert!(span.ends_with(" ``"));
        assert!(span.contains("echo `whoami`"));
    }

    #[test]
    fn code_span_with_double_backtick_run_uses_three() {
        let span = md_code_span("a``b");
        assert!(
            span.starts_with("``` "),
            "triple fence for a double run: {span}"
        );
    }

    #[test]
    fn code_span_neutralizes_newlines() {
        // An inline code span cannot contain a line break: CommonMark parses
        // blocks before inlines, so `foo\n## evil` would let the second line
        // forge a heading. The span collapses to one line so no embedded
        // newline (or the block marker it carried) can escape the span.
        let span = md_code_span("foo\n## evil\n- fake");
        assert!(!span.contains('\n'), "no newline in span: {span}");
        assert_eq!(span, "`foo ## evil - fake`");
        // A backtick-bearing multiline value is fenced AND single-lined.
        let span2 = md_code_span("echo\n`whoami`");
        assert!(!span2.contains('\n'), "no newline in backtick span: {span2}");
        assert!(span2.starts_with("`` ") && span2.ends_with(" ``"));
    }
}
