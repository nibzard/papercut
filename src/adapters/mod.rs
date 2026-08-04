//! Per-harness signal adapters. Each normalizes to the one signal schema and
//! swallows every error so a broken adapter is invisible to the parent task.

pub mod claude_code;
pub mod codex;
