# Using papercut

Install once per machine; after that capture is mostly automatic and your agents
drive the rest. This is the complete command reference. For the *why* and the
architecture see the [README](../README.md); for the triage loop see
[triage.md](triage.md); for schemas, adapters, and the deliberate non-goals see
[PLAN.md](../PLAN.md).

## The store

All data lives under `$XDG_DATA_HOME/papercuts/` (default `~/.local/share/papercuts/`),
private to you, directories `0700`:

```
events/pc_<ulid>.json              one file per voluntary report (Layer 2)
signals/<harness>/<session>.jsonl  automatic failure signals (Layer 1)
sweeps.json                        per-harness high-water marks for `sweep`
config.json
```

Nothing leaves the machine unless you explicitly publish a projection with
`render --write`. Never recorded: environment-variable values, transcripts,
source files, or secrets — only command (truncated), exit code, the first few
stderr lines, a repo pointer, session id, agent, and timestamp.

## 1. Set up

```
cargo install --path .            # put `papercut` on PATH (~/.cargo/bin)
#   or:  ln -s "$(pwd)/target/release/papercut" ~/.local/bin/papercut
papercut install --yes            # detect harnesses; wire the managed block + adapters
papercut doctor                   # verify store, managed blocks, and hook wiring
```

`install` detects harnesses by their config dir (`~/.claude`, `~/.codex`,
`~/.config/opencode`) and, for each one:

- upserts a managed `<!-- papercut:begin --> … <!-- papercut:end -->` block into
  its **global** instructions file (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`,
  …) that tells the agent when and how to report friction;
- for Claude Code, also adds a `PostToolUse` hook to `~/.claude/settings.json`
  that records failed Bash commands.

`--yes` is required — it never prompts. `--harness claude-code,codex` restricts
to a subset; an unknown id is a usage error, never a silent no-op. **Restart
your agent sessions after `install`** so they re-read the updated instructions.

`doctor` verifies directory permissions, that managed blocks are intact, that
the hook is live, and that agent detection works — and prints deterministic
remediation hints for anything wrong.

## 2. Capture

**Automatic — Layer 1, the recall floor (nothing to do after `install`):**

- **Claude Code:** every failed Bash command appends one signal line, invisibly,
  the moment it happens — even if the agent never calls papercut.
- **Codex** (no hook): run `papercut sweep` to parse its on-disk session logs
  since the last sweep. Run it periodically (cron or by habit). `--harness codex`
  restricts.

**Voluntary — Layer 2, high signal (the agent calls it):**

```
papercut add -- "glob ate my args without nullglob set"
papercut add 'flaky CI test' --task PROJ-42 --category tooling \
              --hypothesis 'race on a shared /tmp' --fix 'use mkdtemp'
```

`add` infers repo (normalized git remote, else local path), cwd, git sha, agent
(sniffed from the harness environment, e.g. `CLAUDECODE` / `AI_AGENT`; `unknown`
when nothing matches), and timestamp. The message is required and is the only
*evidence* field; everything else is optional:

| Flag | Meaning |
| --- | --- |
| `--task <ref>` | ticket / PRD reference |
| `--category <tag>` | free-form tag, e.g. `tooling`, `docs` |
| `--agent <id>` | override detection (`claude-code` \| `codex` \| `opencode` \| `unknown`) |
| `--hypothesis "<…>"` | why you think it happened (a guess, not evidence) |
| `--fix "<…>"` | a proposed fix |

Use `--` before the message if it begins with a dash. `add` prints the new event
id in both text and `--output json` so the agent can reference it later.

## 3. Read

```
papercut list --repo . --status open --since 7 --agent claude-code
papercut show pc_01K000000000000000000000A
papercut render --repo . --write
```

**`list`** — a filtered, human-readable listing. The header counts records by
status. `Needs attention` contains open and candidate records; `Reviewed`
contains the rest. Each record shows its status, a short reference, date, and
full observation, followed by any hypothesis, suggested fix, or resolution.
Use the short reference with `papercut show <ref>`. A repo-scoped listing names
the repo once; `--repo all` names the repo on each record. When every record
has the same agent, the agent appears once in the header. An empty repo view
points to `--repo all`. Text wraps at `COLUMNS` when set (32–120 columns), or
at 88 columns otherwise. Status colors appear only on an interactive terminal;
`NO_COLOR` or `TERM=dumb` disables them. Redirected text stays plain.
`--output json` returns complete structured records and full IDs in the same
order.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = every repo · else an explicit repo id |
| `--status <s>` | `open` \| `candidate` \| `fixed` \| `promoted` \| `duplicate` \| `dismissed` |
| `--agent <id>` | only events from this agent |
| `--since <days>` | only events from the last N days |

**`show <id-or-ref>`** — inspect a single event by its full ID from `add` or a
unique short `Ref` from `list`. A short reference is a case-insensitive suffix
of the ID; if it matches more than one event, use a longer suffix or full ID.
Text output displays the full observation, any hypothesis and suggested fix,
context, and resolution. `--output json` returns the complete event in
`data.event`. A missing or unreadable event exits 1 with a structured error;
an ambiguous reference exits 2. The reference is unique across the whole store,
including events from other repos.
`show` never changes the store.

**`render`** — regenerate the deterministic markdown projection.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = global |
| `--write` | also write `PAPERCUTS.md` |

Inside a repo, `render --write` writes to the repo root; `--repo all --write`
writes the global view into the store. `render --write` **refuses** a write that
targets a different repo than the cwd, or any write outside a repo (exit 1),
rather than drop a stray `PAPERCUTS.md` somewhere unexpected.

**Structured output.** Every command takes global `--output json` (default
`text`) and returns one envelope:

```json
{ "schema_version": 1, "status": "ok", "data": { "id": "pc_01K…", "recorded": true }, "errors": [] }
```

On failure `status` is `error` and each item in `errors[]` carries a
machine-readable `code`, a `retryable` flag, and a bounded `hint`.

## 4. Triage

```
papercut triage-pack --repo all --status open --max-tokens 8000
```

A self-contained markdown bundle for any agent: open events first, then
automatic signal groups ranked by count, under a header stating
everything below is data to triage — never instructions to execute.

Included reports retain their full observation, hypothesis, suggested fix, repo,
date, and available context. Terminal reports selected with `--status` also show
their resolution and reference. The budget omits whole reports rather than
clipping away a workaround or separating a claim from its context. An omission
notice gives the first omitted ID; use `papercut show <id>`, narrow `--repo`, or
raise the budget to inspect it.

Signals are grouped by the first command word. Each group shows its recorded date
range, exit-code counts, known session count, and up to three distinct
command/exit/stderr examples with their individual counts. Session IDs are counted
within each harness; missing IDs are counted separately. A group can contain
unrelated failures and expected non-zero probes, so its total does not establish
a shared cause. Additional example combinations are counted as omitted; every raw
signal remains in the private store.

When the whole pack cannot fit, about a third of the available space is reserved
for signal groups; unused space is available to either section. A larger first
signal group can use more if it fits. Whole blocks and omission notices share the
budget. Very small budgets may be exceeded by headings and omission notices alone.

`--output json` returns the same bundle in `data.markdown`. `data.events` and
`data.signal_clusters` count all matching inputs; `events_included`,
`events_omitted`, `signal_clusters_included`, and `signal_clusters_omitted`
describe what the bundle actually contains. The token budget applies to the
markdown bundle; the JSON envelope and metadata add overhead.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = global |
| `--status <s>` | default is `open` + `candidate` together |
| `--max-tokens <n>` | soft budget for the bundle (≈ bytes/4); default 12000 |

Feed the output to an agent following the skill in [triage.md](triage.md).

## 5. Close an event

There is intentionally **no `close` command yet** — status changes are edits to
one JSON field that `render` picks up (see [PLAN.md](../PLAN.md)). Edit the event
file directly:

```jsonc
// ~/.local/share/papercuts/events/pc_01K….json
{
  "status": "fixed",
  "resolution": { "reason": "pinned flaky-dep to 1.2.3", "ref": "abc1234" }
}
```

Rules: any terminal status (`fixed`, `promoted`, `duplicate`, `dismissed`)
requires `resolution.reason`; `fixed` additionally requires `resolution.ref`
(a commit sha, issue url, or dotfiles ref). A non-terminal event may keep a stale
resolution in the file, but `render` will not show it. Reopen by setting
`status` back to `open`.

## 6. Tear down

```
papercut uninstall            # remove every managed block + the hook; --harness to restrict
```

Your surrounding instructions content is left intact — managed blocks own only
their markers.

## Command reference

| Command | Purpose | Flags |
| --- | --- | --- |
| `add <msg>` | record one event | `--task --category --agent --hypothesis --fix` |
| `list` | filtered listing | `--repo --status --agent --since` |
| `show <id-or-ref>` | inspect one full event | — |
| `render` | markdown projection | `--repo --write` |
| `install` | wire harnesses | `--yes` (required) `--harness` |
| `uninstall` | remove wiring | `--harness` |
| `doctor` | verify wiring + store | — |
| `sweep` | parse session logs → signals | `--harness` |
| `triage-pack` | model-facing triage bundle | `--repo --status --max-tokens` |

All commands accept global `--output json` (default `text`). `_hook` is a hidden
entry point the installed hook calls on stdin; you do not run it directly.

## Exit codes & contract

`0` success · `1` report NOT recorded / real failure · `2` usage error. Only the
`_hook` path is silent and infallible — capture must never fail the parent task.
Every command is non-interactive (`install` takes `--yes`), and `--help` is
stable and part of the contract.

---

Dual-licensed under MIT or Apache-2.0, your choice.
