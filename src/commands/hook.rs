//! Compatibility entry point for hooks installed by older versions.
//!
//! Broad failed-command capture has no reliable product attribution. A stale
//! harness entry may still invoke this binary, so this path silently succeeds
//! without reading stdin or writing signals.

use crate::cli::HookArgs;

pub fn run(_args: HookArgs) {}
