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
    about = "Observe how agents struggle to use designated products",
    long_about = "papercut — record difficulty using a designated SDK, CLI, or other product.\n\
                  Exit codes: 0 success · 1 report NOT recorded / real failure · 2 usage error.\n\
                  Every command takes --output json for the stable envelope:\n\
                  { schema_version, status, data, errors[] }.\n\
                  Reporting must never interrupt the task; old hook calls are silent no-ops.",
    after_long_help = "EXAMPLES:\n\
                  $ papercut add --product papercut-cli 'install replaced a symlink'\n\
                  $ papercut add --product my-sdk --product-version 1.2 --surface pagination 'The example omitted the next-page step' --output json\n\
                  $ papercut list --product my-sdk\n\
                  $ papercut list --product my-sdk --status open --since 7\n\
                  $ papercut show 0000000A\n\
                  $ papercut render --product my-sdk --write\n\
                  $ papercut install --yes\n\
                  $ papercut doctor\n\
                  $ papercut triage-pack --product my-sdk --max-tokens 8000\n\
                  $ papercut list --unattributed\n\
                  $ papercut close pc_01K... --status fixed --reason 'fixed config' --ref abc1234 --remedy docs\n\
                  $ papercut stats"
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
    /// Record one difficulty encountered while using a designated product.
    Add(AddArgs),

    /// List events, filterable by product / repo / status / agent / age.
    List(ListArgs),

    /// Inspect one complete event by its full ID or unique short reference.
    Show(ShowArgs),

    /// Regenerate a deterministic markdown projection of the store.
    Render(RenderArgs),

    /// Install product reporting instructions and retire old broad hooks.
    Install(InstallArgs),

    /// Remove all managed blocks and adapters cleanly.
    Uninstall(UninstallArgs),

    /// Verify the store, managed blocks, and reports-only setup.
    Doctor,

    /// Explain reports-only mode; session-log sweeping is suspended.
    Sweep(SweepArgs),

    /// Emit product observations or historical unattributed evidence for triage.
    TriagePack(TriagePackArgs),

    /// Close one event with a verified disposition and resolution record.
    Close(CloseArgs),

    /// Show resolution health metrics from the private store.
    Stats,
    // NOTE: the live `_hook` entry point is intentionally NOT a clap subcommand.
    // main.rs intercepts `_hook` before clap parses argv, because corrupted
    // harness wiring can invoke it with bad args and that must never reach
    // clap's usage-error path. Declaring it here (even `hide = true`) leaks its
    // name into clap's subcommand-enumeration and near-miss suggestions, which
    // then flow into the JSON envelope — violating the contract that the
    // internal hook name never reaches an agent. `HookArgs` + `commands::hook`
    // serve the pre-clap intercept directly.
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// Stable ID of the product designated by the task.
    #[arg(long, required = true)]
    pub product: String,

    /// Product version or revision, when known.
    #[arg(long)]
    pub product_version: Option<String>,

    /// Public product surface involved, such as install or Client.items.list.
    #[arg(long)]
    pub surface: Option<String>,

    /// What you were doing and what got in the way (evidence). Required.
    ///
    /// Pass `--` first if the message begins with a dash, e.g.
    /// `papercut add --product my-cli -- "-y ate my flag"`.
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
    /// Repo filter: `.` = current repo, `all` = every repo, else an id.
    #[arg(long)]
    pub repo: Option<String>,

    /// Exact product ID; spans consuming repos unless --repo is also supplied.
    #[arg(long, conflicts_with = "unattributed")]
    pub product: Option<String>,

    /// Select historical reports without verified product attribution.
    #[arg(long, conflicts_with = "product")]
    pub unattributed: bool,

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
pub struct ShowArgs {
    /// Full ID from `papercut add` or short Ref from `papercut list`.
    pub id: String,
}

#[derive(clap::Args)]
pub struct RenderArgs {
    /// Repo scope: `.` = current repo, `all` = global.
    #[arg(long)]
    pub repo: Option<String>,

    /// Exact product ID; spans consuming repos unless --repo is also supplied.
    #[arg(long, conflicts_with = "unattributed")]
    pub product: Option<String>,

    /// Select historical reports without verified product attribution.
    #[arg(long, conflicts_with = "product")]
    pub unattributed: bool,

    /// Also write the projection to PAPERCUTS.md (repo root, or the store).
    #[arg(long)]
    pub write: bool,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Required confirmation: install modifies harness config files, and this
    /// flag is the explicit go-ahead. Install never prompts — omitting the
    /// flag is a usage error (exit 2), never a question.
    #[arg(long, required = true)]
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
    /// Validate a comma-separated list of known harness ids; no scanning occurs.
    #[arg(long)]
    pub harness: Option<String>,

    /// Re-read only record shapes older papercut versions could not capture.
    #[arg(long)]
    pub backfill_current: bool,
}

#[derive(clap::Args)]
pub struct TriagePackArgs {
    /// Repo scope: `.` = current repo, `all` = global.
    #[arg(long)]
    pub repo: Option<String>,

    /// Exact product ID; spans consuming repos unless --repo is also supplied.
    #[arg(long, conflicts_with = "unattributed")]
    pub product: Option<String>,

    /// Select historical reports and signals without product attribution.
    #[arg(long, conflicts_with = "product")]
    pub unattributed: bool,

    /// Filter by status (default: open + candidate).
    #[arg(long, value_enum)]
    pub status: Option<Status>,

    /// Soft token budget for the bundle (approx chars/4). Default ~12k.
    #[arg(long, default_value_t = 12_000)]
    pub max_tokens: u32,

    /// Only include events and signals from the last N days.
    #[arg(long)]
    pub since: Option<u32>,

    /// Skip this many matching events and signal clusters for pagination.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(clap::Args)]
pub struct CloseArgs {
    /// Event identifier to close.
    pub id: String,

    /// Terminal disposition.
    #[arg(long, value_enum)]
    pub status: Status,

    /// Verified reason for the disposition.
    #[arg(long)]
    pub reason: String,

    /// Commit, issue, documentation, or system reference. Required for fixed.
    #[arg(long, name = "ref")]
    pub ref_: Option<String>,

    /// Remedy class used for health metrics.
    #[arg(long, value_enum)]
    pub remedy: crate::model::Remedy,
}

#[derive(clap::Args)]
pub struct HookArgs {
    /// Which harness invoked the hook (e.g. `claude-code`).
    pub harness: String,
}
