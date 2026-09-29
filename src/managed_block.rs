//! The managed-block engine: idempotent upsert/remove of the reporting
//! instruction inside a harness's global instructions file.
//!
//! Invariant: `install`/`uninstall` only ever touch the whole-line span
//! between `<!-- papercut:begin v2 -->` and `<!-- papercut:end -->`. Content
//! outside those markers is preserved byte-for-byte.
//!
//! Accepted limitation: inserting into a file whose last line has no trailing
//! newline appends one (the block must start on its own line), and a later
//! remove cannot restore the missing byte — the pre-install state is not
//! recorded anywhere. This is the one deviation from byte-for-byte restore.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Inserted,
    Updated,
    Unchanged,
}

/// Current managed-block version.
pub const BLOCK_VERSION: u32 = 2;

/// The begin marker carries the version so `doctor` can detect staleness.
pub fn begin_marker(version: u32) -> String {
    format!("<!-- papercut:begin v{version} -->")
}

pub const END_MARKER: &str = "<!-- papercut:end -->";

/// The instruction body an agent reads. One report per apparent root cause;
/// observation first, hypothesis/fix optional; logging never fails the task.
pub const BLOCK_BODY: &str = concat!(
    "### Observe designated products with Papercut\n",
    "When a task or applicable instructions designate a product to observe, record where using its installation, docs, public interface, behavior, or errors causes confusion, retries, a dead end, a misleading result, or an unexpected workaround:\n",
    "\n",
    "    papercut add --product <designated-product-id> -- \"<task and expectation; what happened; any verified workaround>\"\n",
    "\n",
    "Include --product-version and --surface when known. Report relevant struggles even when commands succeed or the task eventually works. Product bugs encountered during use qualify. Exclude unrelated machine/tool failures, ordinary implementation debugging, accomplishments, and speculative requests. Do not infer the product from the working repo. With no designated product, continue without reporting. If relevance to the designated product is uncertain, state the evidence and uncertainty. Keep suspected causes in --hypothesis and proposed remedies in --fix. One report per apparent obstacle per product per session. Never include secrets, transcripts, or source files. Logging must never interrupt or fail the task.",
);

/// The full block (begin marker + body + end marker), no trailing newline.
pub fn block_text() -> String {
    format!(
        "{}\n{}\n{}",
        begin_marker(BLOCK_VERSION),
        BLOCK_BODY,
        END_MARKER
    )
}

const BEGIN_PREFIX: &str = "<!-- papercut:begin";

#[derive(Debug, Clone, Copy)]
enum Marker {
    Begin(Option<u32>),
    End,
}

/// Classify a line as a managed-block marker, else `None`. Matches whole lines
/// only (after trimming whitespace) so the marker text appearing inside a line
/// of user prose never counts as a marker.
fn classify_marker(line: &str) -> Option<Marker> {
    let t = line.trim();
    if t == END_MARKER {
        return Some(Marker::End);
    }
    let rest = t.strip_prefix(BEGIN_PREFIX)?;
    let rest = rest.trim_start();
    if rest == "-->" {
        return Some(Marker::Begin(None)); // unversioned `<!-- papercut:begin -->`
    }
    let num_part = rest.strip_prefix('v')?;
    let digits: String = num_part
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    if num_part[digits.len()..].trim() == "-->" {
        return Some(Marker::Begin(digits.parse().ok()));
    }
    None
}

/// Byte spans of each line, including its trailing newline (the final line has
/// no trailing newline if the content doesn't end with one).
fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (i, &b) in content.as_bytes().iter().enumerate() {
        if b == b'\n' {
            spans.push((start, i + 1));
            start = i + 1;
        }
    }
    if start < content.len() {
        spans.push((start, content.len()));
    }
    spans
}

/// Byte ranges of every COMPLETE begin..end block region, in file order. Each
/// end marker pairs with the nearest unpaired begin before it, so a
/// lone/orphan begin (no end following it) yields no region — an upsert then
/// appends a fresh block instead of stretching a span to a distant end and
/// swallowing user content. Multiple complete regions (e.g. two racing
/// installs) are all reported, so upsert/remove own every duplicate — body
/// text included — never just the first pair's.
fn block_regions(spans: &[(usize, usize)], markers: &[(usize, Marker)]) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut pending: Option<usize> = None; // span index of the last unpaired begin
    for &(si, m) in markers {
        match m {
            Marker::Begin(_) => pending = Some(si),
            Marker::End => {
                if let Some(b) = pending.take() {
                    regions.push((spans[b].0, spans[si].1));
                }
            }
        }
    }
    regions
}

/// Collect `(span_index, Marker)` for every marker line in `content`.
fn marker_lines(spans: &[(usize, usize)], content: &str) -> Vec<(usize, Marker)> {
    spans
        .iter()
        .enumerate()
        .filter_map(|(idx, &(s, e))| classify_marker(&content[s..e]).map(|m| (idx, m)))
        .collect()
}

/// Detect the installed managed-block version in `content`, if any.
/// Returns `Some(0)` for an unversioned `<!-- papercut:begin -->` marker.
pub fn detect_version(content: &str) -> Option<u32> {
    let spans = line_spans(content);
    for &(s, e) in &spans {
        if let Some(Marker::Begin(v)) = classify_marker(&content[s..e]) {
            return Some(v.unwrap_or(0));
        }
    }
    None
}

/// Upsert the current block into `content`. Idempotent. The fresh block
/// replaces the FIRST complete block region; every other complete region
/// (a duplicate from e.g. racing installs) is removed whole — markers AND
/// body. Stray/orphan marker lines (our leftovers) are removed; all
/// non-region, non-marker content is preserved.
pub fn upsert(content: &str) -> (String, Action) {
    let spans = line_spans(content);
    let markers = marker_lines(&spans, content);
    let regions = block_regions(&spans, &markers);
    let desired = format!("{}\n", block_text());

    let mut out = String::with_capacity(content.len() + desired.len());
    let mut block_written = false;
    let mut skip_until: Option<usize> = None;
    for &(s, e) in &spans {
        if let Some(until) = skip_until {
            if s < until {
                continue;
            }
            skip_until = None;
        }
        let line = &content[s..e];
        if let Some(&(_, rend)) = regions.iter().find(|&&(rs, _)| rs == s) {
            skip_until = Some(rend);
            if !block_written {
                out.push_str(&desired);
                block_written = true;
            }
            continue;
        }
        if classify_marker(line).is_some() {
            continue; // stray marker line — ours, drop it
        }
        out.push_str(line);
    }
    if !block_written {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&desired);
    }

    let action = if out == content {
        Action::Unchanged
    } else if regions.is_empty() {
        Action::Inserted
    } else {
        Action::Updated
    };
    (out, action)
}

/// Remove every managed-block region (markers and body) and any stray marker
/// lines from `content`. Returns `(new_content, removed_anything)`. Never
/// touches non-region, non-marker content.
pub fn remove(content: &str) -> (String, bool) {
    let spans = line_spans(content);
    let markers = marker_lines(&spans, content);
    let regions = block_regions(&spans, &markers);

    let mut out = String::with_capacity(content.len());
    let mut removed = false;
    let mut skip_until: Option<usize> = None;
    for &(s, e) in &spans {
        if let Some(until) = skip_until {
            if s < until {
                continue;
            }
            skip_until = None;
        }
        let line = &content[s..e];
        if let Some(&(_, rend)) = regions.iter().find(|&&(rs, _)| rs == s) {
            skip_until = Some(rend); // drop the whole block region
            removed = true;
            continue;
        }
        if classify_marker(line).is_some() {
            removed = true; // stray marker line — ours, drop it
            continue;
        }
        out.push_str(line);
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_into_empty() {
        let (got, action) = upsert("");
        assert_eq!(action, Action::Inserted);
        assert!(got.contains(begin_marker(BLOCK_VERSION).as_str()));
        assert!(got.ends_with('\n'));
    }

    #[test]
    fn insert_preserves_existing_content() {
        let original = "# My notes\nSome user content.\n";
        let (got, action) = upsert(original);
        assert_eq!(action, Action::Inserted);
        assert!(got.starts_with("# My notes\nSome user content.\n"));
        assert!(got.contains("papercut:begin"));
        // User content untouched.
        assert!(got.contains("# My notes"));
        assert!(got.contains("Some user content."));
    }

    #[test]
    fn upsert_is_idempotent() {
        let (once, _) = upsert("hello\n");
        let (twice, action) = upsert(&once);
        assert_eq!(action, Action::Unchanged);
        assert_eq!(once, twice);
    }

    #[test]
    fn updates_stale_block() {
        let stale = format!("<!-- papercut:begin v0 -->\nold body\n{}\n", END_MARKER);
        let (got, action) = upsert(&stale);
        assert_eq!(action, Action::Updated);
        assert!(got.contains(&begin_marker(BLOCK_VERSION)));
        assert!(!got.contains("old body"));
    }

    #[test]
    fn roundtrip_restore() {
        let original = "# title\n\nbody line.\n";
        let (installed, _) = upsert(original);
        let (restored, removed) = remove(&installed);
        assert!(removed);
        // User content is fully intact after uninstall.
        assert!(restored.contains("# title"));
        assert!(restored.contains("body line."));
        assert!(!restored.contains("papercut:begin"));
    }

    #[test]
    fn remove_when_absent_is_noop() {
        let (got, removed) = remove("nothing here\n");
        assert!(!removed);
        assert_eq!(got, "nothing here\n");
    }

    #[test]
    fn detect_version_parses_current() {
        let (content, _) = upsert("");
        assert_eq!(detect_version(&content), Some(BLOCK_VERSION));
    }

    #[test]
    fn detect_version_unversioned_is_zero() {
        assert_eq!(detect_version("<!-- papercut:begin -->\n"), Some(0));
    }

    #[test]
    fn detect_version_absent_is_none() {
        assert_eq!(detect_version("no markers"), None);
    }

    /// A lone/orphan begin marker (no matching end) must NOT make a later
    /// upsert stretch a span to a distant end and delete the user content
    /// between them. Regression for a data-loss bug in the substring matcher.
    #[test]
    fn orphan_begin_preserves_user_content() {
        let original = "# header\n<!-- papercut:begin -->\nPRECIOUS USER CONTENT\nmore precious\n";
        let (got, action) = upsert(original);
        assert_ne!(action, Action::Unchanged);
        assert!(
            got.contains("PRECIOUS USER CONTENT"),
            "user content must survive"
        );
        assert!(got.contains("more precious"));
        assert_eq!(
            got.matches("papercut:begin").count(),
            1,
            "exactly one block"
        );
        assert_eq!(got.matches("papercut:end").count(), 1);
    }

    /// Repeated upsert on a file that started with an orphan begin must be
    /// stable — not accumulate blocks, not destroy content on the second pass.
    #[test]
    fn repeated_upsert_on_orphan_is_stable() {
        let original = "# header\n<!-- papercut:begin -->\nPRECIOUS\n";
        let (once, _) = upsert(original);
        assert!(once.contains("PRECIOUS"));
        assert_eq!(once.matches("papercut:begin").count(), 1);
        let (twice, action) = upsert(&once);
        assert_eq!(action, Action::Unchanged, "second upsert is a no-op");
        assert_eq!(once, twice);
        assert!(twice.contains("PRECIOUS"));
    }

    /// Two begins before an end collapse to one block; intervening user
    /// content is preserved, not swallowed.
    #[test]
    fn double_begin_collapses_to_one_block() {
        let original = format!(
            "<!-- papercut:begin v0 -->\nBETWEEN\n<!-- papercut:begin v0 -->\nold body\n{}\n",
            END_MARKER
        );
        let (got, _) = upsert(&original);
        assert!(got.contains("BETWEEN"), "content between begins preserved");
        assert_eq!(got.matches("papercut:begin").count(), 1);
        assert_eq!(got.matches("papercut:end").count(), 1);
    }

    /// Two COMPLETE blocks (e.g. left behind by racing installs) collapse to
    /// one, and the duplicate's body must not leak into the file as orphan
    /// user text.
    #[test]
    fn duplicate_complete_blocks_collapse_without_leaking_body() {
        let (one, _) = upsert("user line\n");
        let doubled = format!("{one}{}\n", block_text());
        assert_eq!(doubled.matches("papercut:begin").count(), 2);
        let (got, action) = upsert(&doubled);
        assert_ne!(action, Action::Unchanged);
        assert_eq!(got.matches("papercut:begin").count(), 1);
        assert_eq!(
            got.matches("### Observe designated products with Papercut")
                .count(),
            1,
            "duplicate body removed, not leaked: {got}"
        );
        assert!(got.contains("user line"));
    }

    /// remove() on a doubled file leaves no marker AND no body text behind.
    #[test]
    fn remove_on_duplicate_blocks_leaves_no_body() {
        let (one, _) = upsert("user line\n");
        let doubled = format!("{one}{}\n", block_text());
        let (got, removed) = remove(&doubled);
        assert!(removed);
        assert!(!got.contains("papercut"), "no marker or body left: {got}");
        assert!(got.contains("user line"));
    }

    /// User content between two complete blocks survives both upsert and
    /// remove — only the block regions themselves are owned by papercut.
    #[test]
    fn content_between_duplicate_blocks_is_preserved() {
        let doubled = format!("{}\nBETWEEN BLOCKS\n{}\n", block_text(), block_text());
        let (got, _) = upsert(&doubled);
        assert!(got.contains("BETWEEN BLOCKS"));
        assert_eq!(got.matches("papercut:begin").count(), 1);

        let (gone, removed) = remove(&doubled);
        assert!(removed);
        assert!(gone.contains("BETWEEN BLOCKS"));
        assert!(!gone.contains("papercut"));
    }

    /// The marker text embedded inside a line of user prose is not a marker.
    #[test]
    fn marker_text_in_prose_is_ignored() {
        let original =
            "see <!-- papercut:begin v1 --> in the docs\nand <!-- papercut:end --> too\n";
        let (got, action) = upsert(original);
        assert_eq!(action, Action::Inserted);
        assert!(
            got.contains("see <!-- papercut:begin v1 --> in the docs"),
            "prose untouched"
        );
        assert!(got.contains("and <!-- papercut:end --> too"));
        // Exactly one real block appended at the end.
        assert_eq!(got.matches("papercut:end").count(), 2); // 1 in prose + 1 real
    }

    /// remove() drops an orphan end marker (no begin) as stray, preserving content.
    #[test]
    fn remove_drops_orphan_end_marker() {
        let original = format!("keep me\n{END_MARKER}\nalso keep\n");
        let (got, removed) = remove(&original);
        assert!(removed);
        assert!(got.contains("keep me"));
        assert!(got.contains("also keep"));
        assert!(!got.contains("papercut:end"));
    }
}
