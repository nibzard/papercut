//! Detection of installed coding-agent harnesses and the paths to their
//! global instructions files.
//!
//! The universal denominator across harnesses is a shell + an instructions
//! markdown file. Hook APIs / log formats are adapters layered on top.
//!
//! Global-instructions paths (verified/derived 2026-08-04):
//!   - Claude Code : `$HOME/.claude/CLAUDE.md`
//!   - Codex       : `$HOME/.codex/AGENTS.md`
//!   - OpenCode    : `$HOME/.config/opencode/AGENTS.md` (or `$HOME/.opencode/...`)

use crate::paths::home_dir;
use std::path::{Path, PathBuf};

/// How signals are captured for a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessTier {
    /// A live hook records failures in real time (e.g. Claude Code PostToolUse).
    Live,
    /// No hook; `sweep` parses on-disk session logs (e.g. Codex rollout JSONL).
    Sweep,
    /// Reports-only — Layer 2 still works via the managed block.
    None,
}

impl HarnessTier {
    pub fn label(&self) -> &'static str {
        match self {
            HarnessTier::Live => "live",
            HarnessTier::Sweep => "sweep",
            HarnessTier::None => "none",
        }
    }
}

/// A known harness and how to find its config.
#[derive(Debug, Clone)]
pub struct HarnessDef {
    pub id: &'static str,
    pub display: &'static str,
    pub tier: HarnessTier,
    pub config_dir: fn(&Path) -> PathBuf,
    pub instructions_file: fn(&Path) -> PathBuf,
}

/// A harness detected as installed on this machine.
#[derive(Debug, Clone)]
pub struct DetectedHarness {
    pub id: String,
    pub display: String,
    pub tier: HarnessTier,
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
            display: "Claude Code",
            tier: HarnessTier::Live,
            config_dir: claude_config,
            instructions_file: claude_instructions,
        },
        HarnessDef {
            id: "codex",
            display: "Codex",
            tier: HarnessTier::Sweep,
            config_dir: codex_config,
            instructions_file: codex_instructions,
        },
        HarnessDef {
            id: "opencode",
            display: "OpenCode",
            tier: HarnessTier::None,
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
            tier: d.tier,
            instructions_file: (d.instructions_file)(&home),
        })
        .collect()
}

/// Look up a single harness definition by id.
pub fn find(id: &str) -> Option<HarnessDef> {
    catalog().into_iter().find(|d| d.id == id)
}
