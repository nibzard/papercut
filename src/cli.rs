//! Command-line definition (`clap` derive).
//!
//! `--help` is a contract: usage, flags, output modes, and exit codes are
//! documented here and kept stable. `--output json` is available on every
//! command via a global flag; the default is terse text.

use crate::model::Status;
use crate::output::OutputMode;

#[derive(clap::Parser)]
#[command(
    name = "papercut",
    version,
    about = "System-level friction telemetry for coding agents",
    long_about = "papercut — record repo-specific friction the moment it happens.\n\
                  Exit codes: 0 success · 1 report NOT recorded / real failure · 2 usage error.\n\
                  Every command takes --output json for the stable envelope:\n\
                  { schema_version, status, data, errors[] }.\n\
                  Capture never fails your task; the hook path is invisible.",
    after_long_help = "EXAMPLES:\n\
                  $ papercut add 'docs build needs -dmflag'\n\
                  $ papercut add 'flaky test' --task PROJ-42 --category tooling --output json\n\
                  $ papercut list --repo . --status open --since 7\n\
                  $ papercut render --write\n\
                  $ papercut install\n\
                  $ papercut doctor\n\
                  $ papercut sweep\n\
                  $ papercut triage-pack --repo . --max-tokens 8000"
)]
pub struct Cli {
    /// Output mode. `json` emits the stable envelope; `text` (default) is terse.
    #[arg(global = true, long, value_enum, default_value_t = OutputMode::Text)]
    pub output: OutputMode,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Record one papercut event. Message required; everything else inferred.
    Add(AddArgs),

    /// List events, filterable by repo / status / agent / age.
    List(ListArgs),

    /// Regenerate a deterministic markdown projection of the store.
    Render(RenderArgs),

    /// Detect harnesses; install the reporting instruction + wire signal adapters.
    Install(InstallArgs),

    /// Remove all managed blocks and adapters cleanly.
    Uninstall(UninstallArgs),

    /// Verify the store, managed blocks, and adapter wiring; print remediation hints.
    Doctor,

    /// Parse harness session logs since the last sweep; extract failure signals.
    Sweep(SweepArgs),

    /// Emit a self-contained markdown triage bundle (open events + signal clusters).
    TriagePack(TriagePackArgs),

    /// Hidden: live hook entry point. Reads the harness payload on stdin and
    /// records a signal. Never prints, never fails the parent task.
    #[command(name = "_hook", hide = true)]
    Hook(HookArgs),
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// What you were doing and what got in the way (evidence). Required.
    ///
    /// Pass `--` first if the message begins with a dash, e.g.
    /// `papercut add -- "-y ate my flag"`.
    pub message: String,

    /// Optional ticket / PRD reference.
    #[arg(long)]
    pub task: Option<String>,

    /// Optional free-form tag, e.g. "tooling", "docs".
    #[arg(long)]
    pub category: Option<String>,

    /// Override agent detection (claude-code | codex | opencode | unknown).
    #[arg(long)]
    pub agent: Option<String>,

    /// Optional — why you think it happened (a hypothesis, not evidence).
    #[arg(long)]
    pub hypothesis: Option<String>,

    /// Optional proposed fix.
    #[arg(long)]
    pub fix: Option<String>,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Repo filter: `.` = current repo (default), `all` = every repo, else an id.
    #[arg(long, default_value = ".")]
    pub repo: String,

    /// Filter by status.
    #[arg(long, value_enum)]
    pub status: Option<Status>,

    /// Filter by agent id.
    #[arg(long)]
    pub agent: Option<String>,

    /// Only events from the last N days.
    #[arg(long)]
    pub since: Option<u32>,
}

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Repo scope: `.` = current repo (default), `all` = global.
    #[arg(long, default_value = ".")]
    pub repo: String,

    /// Also write the projection to PAPERCUTS.md (repo root, or the store).
    #[arg(long)]
    pub write: bool,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Non-interactive confirmation. Install never prompts; this flag is the
    /// explicit "go ahead" for scripts.
    #[arg(long, default_value_t = true)]
    pub yes: bool,

    /// Restrict to a comma-separated list of harness ids (e.g. `claude-code`).
    /// Default: every detected harness.
    #[arg(long)]
    pub harness: Option<String>,
}

#[derive(clap::Args)]
pub struct UninstallArgs {
    /// Restrict to a comma-separated list of harness ids. Default: all.
    #[arg(long)]
    pub harness: Option<String>,
}

#[derive(clap::Args)]
pub struct SweepArgs {
    /// Restrict to a comma-separated list of harness ids. Default: all sweep-tier.
    #[arg(long)]
    pub harness: Option<String>,
}

#[derive(clap::Args)]
pub struct TriagePackArgs {
    /// Repo scope: `.` = current repo (default), `all` = global.
    #[arg(long, default_value = ".")]
    pub repo: String,

    /// Filter by status (default: open + candidate).
    #[arg(long, value_enum)]
    pub status: Option<Status>,

    /// Soft token budget for the bundle (approx chars/4). Default ~12k.
    #[arg(long, default_value_t = 12_000)]
    pub max_tokens: u32,
}

#[derive(clap::Args)]
pub struct HookArgs {
    /// Which harness invoked the hook (e.g. `claude-code`).
    pub harness: String,
}
