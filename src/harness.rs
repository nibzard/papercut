//! Detection of installed coding-agent harnesses and the paths to their
//! global instructions files.
//!
//! The universal denominator across harnesses is a shell + an instructions
//! markdown file. Historical adapters remain available for future attribution work.
//!
//! Global-instructions paths (verified/derived 2026-08-04):
//!   - Claude Code : `$HOME/.claude/CLAUDE.md`
//!   - Codex       : `$HOME/.codex/AGENTS.md`
//!   - OpenCode    : `$HOME/.config/opencode/AGENTS.md` (or `$HOME/.opencode/...`)

use crate::paths::home_dir;
use std::path::{Path, PathBuf};

/// A known harness and how to find its config.
#[derive(Debug, Clone)]
pub struct HarnessDef {
    pub id: &'static str,
    pub display: &'static str,
    pub config_dir: fn(&Path) -> PathBuf,
    pub instructions_file: fn(&Path) -> PathBuf,
}

/// A harness detected as installed on this machine.
#[derive(Debug, Clone)]
pub struct DetectedHarness {
    pub id: String,
    pub display: String,
    pub instructions_file: PathBuf,
}

fn claude_config(home: &Path) -> PathBuf {
    home.join(".claude")
}
fn claude_instructions(home: &Path) -> PathBuf {
    home.join(".claude").join("CLAUDE.md")
}
fn codex_config(home: &Path) -> PathBuf {
    home.join(".codex")
}
fn codex_instructions(home: &Path) -> PathBuf {
    home.join(".codex").join("AGENTS.md")
}
fn opencode_config(home: &Path) -> PathBuf {
    let primary = home.join(".config").join("opencode");
    if primary.exists() {
        primary
    } else {
        home.join(".opencode")
    }
}
fn opencode_instructions(home: &Path) -> PathBuf {
    let primary = home.join(".config").join("opencode");
    if primary.exists() {
        primary.join("AGENTS.md")
    } else {
        home.join(".opencode").join("AGENTS.md")
    }
}

/// The full catalog of harnesses papercut knows about.
pub fn catalog() -> Vec<HarnessDef> {
    vec![
        HarnessDef {
            id: "claude-code",
            display: "Grave Digger XL",
            config_dir: claude_config,
            instructions_file: claude_instructions,
        },
        HarnessDef {
            id: "codex",
            display: "Codex",
            config_dir: codex_config,
            instructions_file: codex_instructions,
        },
        HarnessDef {
            id: "opencode",
            display: "OpenCode",
            config_dir: opencode_config,
            instructions_file: opencode_instructions,
        },
    ]
}

/// Detect harnesses whose config directories exist under `$HOME`.
pub fn detect() -> Vec<DetectedHarness> {
    let Some(home) = home_dir() else {
        return vec![];
    };
    catalog()
        .into_iter()
        .filter(|d| (d.config_dir)(&home).exists())
        .map(|d| DetectedHarness {
            id: d.id.to_string(),
            display: d.display.to_string(),
            instructions_file: (d.instructions_file)(&home),
        })
        .collect()
}

/// Look up a single harness definition by id.
pub fn find(id: &str) -> Option<HarnessDef> {
    catalog().into_iter().find(|d| d.id == id)
}

/// Restrict `detected` to the ids in a comma-separated `filter` (`None`
/// selects everything). Shared by install and uninstall so the two commands
/// can never drift apart on filter semantics.
pub fn filter_detected(
    detected: Vec<DetectedHarness>,
    filter: Option<&str>,
) -> Vec<DetectedHarness> {
    match filter {
        None => detected,
        Some(list) => {
            let want: Vec<&str> = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            detected
                .into_iter()
                .filter(|d| want.iter().any(|w| *w == d.id))
                .collect()
        }
    }
}

/// Does `filter` select `id`? `None` selects everything.
pub fn filter_selects(filter: Option<&str>, id: &str) -> bool {
    match filter {
        None => true,
        Some(list) => list.split(',').map(str::trim).any(|w| w == id),
    }
}

/// The ids in a comma-separated `filter` that name no known harness. Empty when
/// `filter` is `None` or every requested id is in the catalog. Used to turn a
/// typo like `--harness codx` from a silent no-op into a usage error.
pub fn unknown_requested(filter: Option<&str>) -> Vec<String> {
    let Some(list) = filter else {
        return Vec::new();
    };
    let known: Vec<&str> = catalog().iter().map(|d| d.id).collect();
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !known.contains(s))
        .map(str::to_string)
        .collect()
}

/// A comma-separated list of every known harness id, for error hints.
pub fn known_ids() -> String {
    catalog()
        .iter()
        .map(|d| d.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A usage `ErrorItem` when `filter` names unknown harness ids, else `None`.
/// Shared by install/uninstall/sweep so a typo is never a silent no-op.
pub fn unknown_harness_error(filter: &Option<String>) -> Option<crate::output::ErrorItem> {
    let unknown = unknown_requested(filter.as_deref());
    if unknown.is_empty() {
        return None;
    }
    Some(crate::output::ErrorItem::new(
        "unknown_harness",
        format!("unknown harness id(s): {}", unknown.join(", ")),
        false,
        format!("known harness ids: {}", known_ids()),
    ))
}
