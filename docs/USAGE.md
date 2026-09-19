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

Nothing leaves the machine unless you publish a projection with `render --write`.
Commands and error output can contain values that the caller typed. Do not put
secrets in commands or reports. Papercut truncates captured text and protects the
store with owner-only permissions. It does not rewrite secrets automatically.

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
- for Claude Code, also adds a `PostToolUseFailure` hook to `~/.claude/settings.json`
  that records failed Bash commands.

`--yes` is required — it never prompts. `--harness claude-code,codex` restricts
to a subset; an unknown id is a usage error, never a silent no-op. **Restart
your agent sessions after `install`** so they re-read the updated instructions.

`install` preserves an existing instruction symlink and its target permissions.
`doctor` verifies owner-only store permissions, managed blocks, hook wiring,
Codex sweep freshness, known Codex record formats, and agent detection. It prints
deterministic remediation hints for each failed check.

## 2. Capture

**Automatic — Layer 1, the recall floor (nothing to do after `install`):**

- **Claude Code:** every failed Bash command appends one signal line, invisibly,
  the moment it happens — even if the agent never calls papercut.
- **Codex** (no hook): run `papercut sweep` to parse its on-disk session logs
  since the last sweep. Run it periodically (cron or by habit). `--harness codex`
  restricts.

After an upgrade from an older release, run one current-format backfill:

```
papercut sweep --harness codex --backfill-current
```

The backfill reads only record formats that older releases missed. Its own
watermark makes repeated runs a no-op. A daily scheduler can run the normal sweep:

```
17 3 * * * $HOME/.local/bin/papercut sweep
```

Codex session logs can omit a child command's exit code when a `functions.exec`
script prints only `result.output`. The installed v3 instruction tells Codex to
print the full result object. `doctor` reports recent opaque results so this
capture limit stays visible.

**Voluntary — Layer 2, high signal (the agent calls it):**

```
papercut add "glob ate my args without nullglob set"
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

Use `--` before the message if it begins with a dash — but put every flag
(`--task`, `--hypothesis`, …) BEFORE `--`: `--` ends option parsing, so
`add -- "msg" --hypothesis x` is a usage error (exit 2). `add` prints the new
event id in both text and `--output json` so the agent can reference it later.

## 3. Read

```
papercut list --repo . --status open --since 7 --agent claude-code
papercut render --repo . --write
```

**`list`** — a filtered listing.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = every repo · else an explicit repo id |
| `--status <s>` | `open` \| `candidate` \| `fixed` \| `promoted` \| `duplicate` \| `dismissed` |
| `--agent <id>` | only events from this agent |
| `--since <days>` | only events from the last N days |

**`render`** — regenerate the deterministic markdown projection.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = global |
| `--write` | also write `PAPERCUTS.md` |

Inside a repo, `render --write` writes to the repo root. `--repo all --write`
writes the global view into the store. Outside any repo, the default `--repo .`
has no current repo to scope to, so `render --write` writes the global view into
the store too (exit 0) — the same as `--repo all --write`. `render --write
--repo <id>` is refused (exit 1) when `<id>` is not the current repo or when run
outside any repo, rather than drop a stray `PAPERCUTS.md` somewhere unexpected.

**Structured output.** Every command takes global `--output json` (default
`text`) and returns one envelope:

```json
{ "schema_version": 1, "status": "ok", "data": { "id": "pc_01K…", "recorded": true }, "errors": [] }
```

On failure `status` is `error` and each item in `errors[]` carries a
machine-readable `code`, a `retryable` flag, and a bounded `hint`.

## 4. Triage

```
papercut triage-pack --repo all --since 14 --max-tokens 16000
```

A self-contained markdown bundle for any agent. It reserves space for reports
and signal clusters. Reports appear newest first. Signal clusters rank by count.
The header states that all following content is data, not instructions.

| Flag | Meaning |
| --- | --- |
| `--repo <spec>` | `.` = current repo (default) · `all` = global |
| `--status <s>` | default is `open` + `candidate` together |
| `--max-tokens <n>` | soft budget for the bundle (≈ chars/4); default 12000 |
| `--since <days>` | include recent reports and signals only |
| `--offset <n>` | skip matching reports and clusters for the next page |

Feed the output to an agent following the skill in [triage.md](triage.md).

## 5. Close an event

Use `close` after you verify the disposition:

```
papercut close pc_01K... --status fixed \
  --reason "pinned flaky dependency to 1.2.3" \
  --ref abc1234 --remedy pinned-dep
```

Rules: any terminal status (`fixed`, `promoted`, `duplicate`, `dismissed`)
requires `resolution.reason`; `fixed` additionally requires `resolution.ref`
(a commit SHA, issue URL, or dotfiles reference). Remedy values are `docs`,
`wrapper`, `earlier-validation`, `better-error`, `pinned-dep`, `system-level`,
`promote`, and `dismiss`. Direct JSON edits remain compatible.

Run `papercut stats` to see status counts, remedy counts, and the median time to
resolution. Recurrence after a fix still needs semantic review of later signals.

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
| `render` | markdown projection | `--repo --write` |
| `install` | wire harnesses | `--yes` (required) `--harness` |
| `uninstall` | remove wiring | `--harness` |
| `doctor` | verify wiring + store | — |
| `sweep` | parse session logs → signals | `--harness --backfill-current` |
| `triage-pack` | model-facing triage bundle | `--repo --status --max-tokens --since --offset` |
| `close <id>` | record a verified disposition | `--status --reason --ref --remedy` |
| `stats` | show resolution health metrics | — |

All commands accept global `--output json` (default `text`). `_hook` is a hidden
entry point the installed hook calls on stdin; you do not run it directly.

## Exit codes & contract

`0` success · `1` report NOT recorded / real failure · `2` usage error. Only the
`_hook` path is silent and infallible — capture must never fail the parent task.
Every command is non-interactive (`install` takes `--yes`), and `--help` is
stable and part of the contract.

---

Dual-licensed under MIT or Apache-2.0, your choice.
