# papercut

System-level friction telemetry for coding agents.

`papercut` is a machine-wide CLI that lets any coding agent — Claude Code, Codex,
OpenCode, whatever comes next — record repo-specific friction the moment it happens,
and gives you a periodic review loop to actually close it. Capture is automatic and
dumb; interpretation is deliberate and rare.

> **Status: design phase.** Nothing is built yet. The full design lives in
> [PLAN.md](PLAN.md).

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

## Planned shape

A single static Rust binary. Core commands: `add`, `list`, `render`, `install`
(wires the reporting instruction into each harness's global config as a managed
block, plus signal adapters), `sweep`, `doctor`, `triage-pack`. See
[PLAN.md](PLAN.md) for schemas, adapters, build phases, and the list of things
deliberately not being built.
