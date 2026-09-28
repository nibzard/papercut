//! Event identifiers: `pc_` + a ULID.
//!
//! ULIDs are time-sortable, so lexicographic ordering of ids doubles as
//! chronological ordering — `render` and `list` rely on that for determinism.

use ulid::Ulid;

/// The stable prefix shared by every papercut event id.
pub const ID_PREFIX: &str = "pc_";

/// Generate a fresh event id.
pub fn new_id() -> String {
    format!("{ID_PREFIX}{}", Ulid::new())
}

/// True if `s` looks like a papercut event id.
pub fn looks_like_id(s: &str) -> bool {
    s.len() == ID_PREFIX.len() + 26 && s.starts_with(ID_PREFIX)
}

/// A short suffix that remains unique across the complete current store.
/// ULIDs start with time, so their *suffix* is more useful than their prefix.
pub fn short_ref(id: &str, all_ids: &[String]) -> String {
    let chars: Vec<char> = id.chars().collect();
    for len in 8..=chars.len() {
        let suffix: String = chars[chars.len() - len..].iter().collect();
        if all_ids
            .iter()
            .filter(|other| other.ends_with(&suffix))
            .count()
            <= 1
        {
            return suffix;
        }
    }
    id.to_string()
}
