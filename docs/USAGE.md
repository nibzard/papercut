# Papercut CLI usage

Papercut records agent observations about a product designated by the task or
applicable instructions. The current Git repo is consumer context. Product IDs
come from the human's designation; the CLI never infers them from a Git remote,
executable name, or working directory.

Run `papercut --help` or `papercut <command> --help` for the installed command
contract. Commands are non-interactive and support `--output json`.

## Install and check

```sh
cargo install --path . --locked
papercut install --yes
papercut doctor
```

`install` updates a versioned managed block in detected harness instructions
files and removes Papercut's old broad failed-command hook from Grave Digger XL
settings. It preserves content outside owned markers and unrelated settings
entries. The install is repeatable. Restart agent sessions so they read the new
instruction. `--harness claude-code,codex` can restrict which detected harnesses
are touched.

`doctor` reports store health, block versions, and any remaining old hook or
installation metadata. A healthy reports-only installation has no Papercut
failed-command hook. Its text output includes remediation hints; JSON has
`data.healthy` and checks with names, details, and hints. Doctor exits 1 when
unhealthy.

`uninstall [--harness <ids>]` removes owned blocks and hook entries, including
recorded instruction paths that moved since installation. Stored reports remain.

## Record a product observation

```sh
papercut add --product '@nibzard/example-sdk' \
  --product-version 0.4.0 --surface 'Client.items.list' \
  -- 'Following the pagination example, I expected a next-page cursor. list() returned an array; finding pages() took three attempts. pages() completed the task.'
```

`--product` and the observation are required. A product ID is a stable,
case-sensitive string chosen by the human; surrounding whitespace is trimmed.
`--product-version` and `--surface` are optional nonblank strings. Provide a
version or revision when it is known or easy to verify. The consuming repo's
Git SHA is never assumed to be the product version.

Other optional flags:

| Flag | Use |
| --- | --- |
| `--task <ref>` | Ticket, task, or other short reference. |
| `--category <tag>` | Free-form category. |
| `--agent <id>` | Override detected harness ID. |
| `--hypothesis <text>` | Suspected cause, kept separate from the observation. |
| `--fix <text>` | Proposed remedy, kept separate from evidence. |

The observation should say what task the agent attempted, what it expected and
why, what happened, and any verified workaround. Product bugs encountered
during use qualify. Confusing success qualifies too. The agent should exclude
unrelated environment trouble and ordinary implementation debugging. Do not
put secrets, environment values, transcripts, or source files in the message.

Use `--` before an observation beginning with a dash. A successful call prints
the full `pc_...` ID. Missing or blank product is a usage error (exit 2);
another recording failure exits 1 and does not claim the event was saved.

## Read reports

```sh
papercut list --product '@nibzard/example-sdk'
papercut list --product '@nibzard/example-sdk' --repo . --status open --since 7
papercut list --unattributed --repo all
papercut show <full-id-or-short-ref>
```

`--product ID` selects that product across all consuming repos by default.
Add `--repo .` for the current repo, `--repo all` for every repo, or pass a
specific repo ID. Product and repo selectors intersect. `--product` and
`--unattributed` conflict. The latter selects historical reports lacking valid
product attribution. With neither selector, `list` defaults to the current
repo, or all repos outside Git. Filters also include `--status`, `--agent`,
and `--since <days>`.

Text listings show full observations and store-unique short references. JSON
returns complete event objects. `show` accepts an exact full ID or a unique,
case-insensitive ID suffix. An unknown ID is a read failure (exit 1); an
ambiguous or invalid suffix is a usage error (exit 2). `show` displays either
the historical v1 schema or a product-attributed v2 event without rewriting it.

## Render a projection

```sh
papercut render --product '@nibzard/example-sdk'
papercut render --product '@nibzard/example-sdk' --write
papercut render --product '@nibzard/example-sdk' --repo . --write
```

Without `--write`, render prints deterministic Markdown. With a product and no
explicit repo filter, `--write` places `PAPERCUTS.md` in the private store,
even when run inside a checkout. With an explicit repo scope, it writes the
filtered projection to that repo's root only when invoked from the matching
working tree. `--unattributed` can render historical reports separately.
Review a projection before publishing it into a repo.

## Prepare a triage pack

```sh
papercut triage-pack --product '@nibzard/example-sdk' --max-tokens 8000
papercut triage-pack --unattributed --repo all
```

Product packs contain open and candidate observations from all consumer repos
unless `--repo` narrows them. `--status` overrides the default. Reports include
their complete observation, optional hypothesis and fix, product version and
surface, and consumer context. The token budget omits whole reports and names
the first omitted ID for `show`.

With no product selector, `triage-pack` uses the current repo by default,
includes only attributed reports, and counts excluded legacy reports.
`--unattributed` provides a separate historical review, including old raw
failed-command signal groups. Those groups are unverified clues, not product
failure counts.

See [the triage guide](triage.md) for verification, resolution, and follow-up.

## Automatic capture and compatibility

Broad failed-command capture is suspended. `papercut sweep` scans no sessions,
advances no watermarks, and returns `capture_mode: "reports_only"` with zero
signals. The old hidden `_hook` entry point returns silently without capture
so stale harness settings cannot interrupt an agent's task. Invalid harness
flags remain usage errors.

Existing v1 event files and signal files remain in the private store. Product
identity is authoritative only in a valid v2 event. The CLI does not assign
historical events to a product based on their repo. A reviewer can back up a
selected v1 event outside `events/`, then explicitly add a product and change
that event to schema v2 while preserving its ID, timestamp, observation, and
other fields. Do not perform bulk attribution by guessing. Older binaries may
skip v2 events, so upgrade the installed binary before new reports are recorded.

## JSON and exit codes

Every command accepts `--output json` and uses this envelope:

```json
{
  "schema_version": 1,
  "status": "ok",
  "data": {},
  "errors": []
}
```

`data` is command-specific. Errors contain a machine-readable `code`, a
`retryable` flag, and a bounded `hint`. Exit 0 means success, exit 1 means a
real failure, and exit 2 means a usage error. The envelope version is
independent of the event file schema version.
