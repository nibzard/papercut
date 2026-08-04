# papercut

System-level friction telemetry for coding agents.

`papercut` is a machine-wide CLI that lets any coding agent — Claude Code, Codex,
OpenCode, whatever comes next — record repo-specific friction the moment it happens,
and gives you a periodic review loop to actually close it. Capture is automatic and
dumb; interpretation is deliberate and rare.

> **Status: built.** The CLI, store, Claude Code live adapter, Codex sweep adapter,
> doctor, and triage-pack are all implemented and tested. The design and the list of
> things deliberately not built live in [PLAN.md](PLAN.md); the triage loop is documented
> in [docs/triage.md](docs/triage.md).

## The idea

Agents constantly hit avoidable friction — an unquoted glob the shell ate, a test
runner with a hidden working directory, a stale global CLI. They work around it,
finish the task, and the workaround disappears with the session. Successful task
completion does not mean the interface was good.

papercut is the missing channel:

```
Layer 1 — SIGNALS   automatic, zero-trust, high volume
  Per-harness adapters record failed commands as raw facts.
        │
Layer 2 — REPORTS   voluntary, low volume, high signal
  Any agent in any repo runs `papercut add "<observation>"`.
        │
Layer 3 — TRIAGE    periodic, human-driven, all the intelligence
  Correlate signals + reports, verify, fix, close.
```

The store is central (`~/.local/share/papercuts/`) and private by default. Because
it sees every repo and every agent on the machine, it can spot the class of friction
per-repo logs misattribute: the problem that follows *you* across five repos, whose
fix belongs in your dotfiles or global agent instructions — not in any one project.

## Design principles

1. Capture must be cheaper than not capturing — no network, no model calls, never
   fails the parent task.
2. Observation ≠ hypothesis ≠ fix. Observations are evidence; the rest may be wrong.
3. Don't trust agent diligence for recall — automatic signals are the floor,
   voluntary reports are the bonus.
4. The agent is the primary user: structured output, deterministic exit codes,
   non-interactive always.
5. The goal is closing friction, not collecting complaints. A successful papercut
   disappears into a wrapper, a pin, an earlier lint, or a clearer error.

## Build & use

A single static Rust binary, boring crates only (`clap`, `serde`/`serde_json`, `ulid`,
`anyhow`), no async runtime.

```
cargo build -r                     # → target/release/papercut
./target/release/papercut install  # detect harnesses, wire the managed block + adapters
./target/release/papercut doctor   # verify store, blocks, and hook wiring
./target/release/papercut add -- "the shell ate my unquoted glob"
./target/release/papercut list --status open
./target/release/papercut sweep            # parse Codex session logs → signals
./target/release/papercut triage-pack --repo all
```

Every command takes `--output json` for the stable envelope
`{ schema_version, status, data, errors[] }`. Exit codes are honest and deterministic:
`0` success · `1` report NOT recorded / real failure · `2` usage error. Only the live hook
path (`_hook`) is silent and infallible — capture must never fail the parent task.

Tests: `cargo test` (unit + integration, all run against an isolated fake `$HOME` /
`$XDG_DATA_HOME`, never the real store). Lint: `cargo clippy --all-targets -- -D warnings`.

## Design notes

Core commands: `add`, `list`, `render`, `install` (wires the reporting instruction into
each harness's global config as a managed block, plus signal adapters), `uninstall`,
`sweep`, `doctor`, `triage-pack`. See [PLAN.md](PLAN.md) for schemas, adapters, build
phases, and the list of things deliberately not being built (`close`/`promote` are deferred
— status changes are edits to one JSON field that `render` picks up).
