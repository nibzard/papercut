# papercut — project instructions

## Names

On this project, Claude is **Grave Digger XL** and Niko is **Nik-Nak Attack**.
These are load-bearing and non-negotiable.

## What this is

A system-level Rust CLI that lets any coding agent record repo-specific friction
("papercuts") into a central private store, with automatic failure signals from
harness adapters and a periodic human-driven triage loop that closes friction with
real fixes.

**PLAN.md is the source of truth** for architecture, schemas, command surface,
build phases, and — critically — the list of things we deliberately do NOT build.
Read it before proposing anything. If work contradicts PLAN.md, update the plan
first (with agreement), then the code.

## Invariants (violating these is a design regression, not a style choice)

- **Capture never fails the parent task.** The hook/adapter path swallows every
  error and stays invisible. The CLI path is the opposite: honest, deterministic
  exit codes (0 = success, 1 = report NOT recorded, 2 = usage error).
- **The agent is the primary user.** `--output json` everywhere with the one
  envelope shape; complete `--help`; non-interactive always — no prompts on any
  command (`install` takes `--yes`).
- **Store is private by default** (`~/.local/share/papercuts/`). Publishing into a
  repo is an explicit act. Never write secrets, env-var values, transcripts, or
  source files into events.
- **Duplicates are evidence.** Never dedupe, fingerprint, or discard at capture
  time. Semantic dedup happens at triage, in a model, with a human.
- **Observation ≠ hypothesis ≠ fix** — separate fields, always.
- **Boring crates only**: `clap`, `serde`/`serde_json`, `ulid`, `anyhow`. No async
  runtime. Adding a dependency requires Nik-Nak Attack's sign-off.
- **Managed blocks own only their markers.** `install`/`uninstall` never touch
  content outside `<!-- papercut:begin -->` / `<!-- papercut:end -->`.

## Practical notes

- Harness internals (env vars, hook APIs, session-log formats) drift — verify
  against the installed harness during implementation, never trust memory or old
  docs. Adapters must degrade to silent no-ops, never to task-visible errors.
- Tests that touch the store or instructions files run against a fake `$HOME` /
  `$XDG_DATA_HOME` — never the real one.
- `render` output must be deterministic: same events in, byte-identical markdown out.
