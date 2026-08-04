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
