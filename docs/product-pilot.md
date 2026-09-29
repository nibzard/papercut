# Product usage pilot: Papercut CLI

On 2026-09-29, a prior agent report supplied a concrete Papercut CLI usage
obstacle: `papercut install --yes` replaced stowed `~/.claude/CLAUDE.md` and
`settings.json` symlinks. That report belongs to the designated product because
the installer caused the difficulty. Shell aliases, missing browsers, and
unrelated build failures in the historical store do not.

We reviewed those examples for scope, but left all historical records unchanged.
The existing report has no authoritative product field; its relevance here is a
manual assessment, not automatic attribution of schema v1 data.

The fix in this checkout resolves an existing managed-file symlink to its target
before an atomic write and preserves the target's permissions. In an isolated
home and downstream consumer repo, the current `0.1.0` working tree:

1. Ran `papercut install --yes` against two stowed symlinks and an old Papercut
   failed-command hook alongside an unrelated user hook.
2. Preserved both symlinks and the user hook; removed the old Papercut hook.
3. Passed `papercut doctor` in reports-only mode.
4. Recorded a `papercut-cli` report with version `0.1.0-working-tree`, surface
   `install`, and the downstream repo as consumer context. The product view
   found it across repos.
5. Returned zero scans and signals from `papercut sweep`.

The before observation is historical; the after check was run with a fresh fake
home and store. The checks establish that this specific installer obstacle no
longer occurs in the working tree. They do not establish behavior in a published
release or across all agent sessions.

## Steel CLI as the chosen external product

The human designated [Steel CLI](https://github.com/steel-dev/cli) for the
second usage task. The installed version was `0.4.4`. Its top-level `--help`
said `steel init --agent` would print an onboarding guide and exit. Running it
in this repo instead checked authentication, downloaded the skills installer,
and installed five skills. The command exited successfully. `steel init --help`
describes `--agent` as auto-accepting the onboarding flow, so the two help
surfaces disagree. We removed the generated skills and lockfile and recorded
the observation as product `steel-dev/cli`, surface `init --agent`, event
`pc_01M3P3VFACN56HZGBWQE2DMW3K`. The Papercut product view and triage pack
both showed that report with Steel's version and the Papercut repo as consumer
context.

From a temporary consumer directory, `steel scrape https://example.com
--format markdown` then succeeded and returned the page content. Because stdout
was piped, Steel returned JSON, as its help documents. There was no obstacle to
report for that scrape. An unrelated command restriction during the Papercut
pilot also stayed outside the Steel product report.

This exercise demonstrates a useful confusing-success report and an ordinary
successful task. The Papercut installer fix supplies the verified product
improvement and repeat task. The Steel help discrepancy remains open for Steel's
maintainer; this Papercut checkout does not change that product. The first
external choice was a CLI, so a separate SDK usage pilot remains future work.
Do not invent a report merely to fill a pilot quota.
