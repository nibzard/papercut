# papercut

Papercut helps product maintainers see where coding agents struggle to use an
SDK, CLI, or other product they care about. An agent records what it tried,
what it expected, what happened, and any workaround while using a designated
product. The maintainer reviews those reports, improves the product, and checks
whether a later task gets easier.

Reports from different consuming repos live in one private store. The product
is named explicitly; the current Git repo tells you where it was used.

## Install from this checkout

With Rust and Cargo installed:

```sh
cargo install --path . --locked
papercut install --yes
papercut doctor
```

`install` adds a short instruction to detected agents' global instructions
files. It removes older Papercut failed-command hooks. Restart agent sessions
after installation so they read the updated instruction. When upgrading from
v0.1.0, replace the binary before running `install --yes` because the new
instruction requires `--product`.

## Designate a product, then use it

Give the agent a normal task and a stable product ID, for example:

> Use `@nibzard/example-sdk` to list all items in this application. Observe
> that SDK with Papercut while you work.

When the SDK's public interface causes difficulty, the agent can run:

```sh
papercut add --product '@nibzard/example-sdk' \
  --product-version 0.4.0 --surface 'Client.items.list' \
  -- 'The pagination example led me to expect a next-page cursor from list(). It returned only an array; finding pages() took three attempts. pages() completed the task.'
```

The product version and surface are optional. A report can describe confusing
help, misleading output, difficult discovery, or a bug encountered during use,
including when the task eventually succeeds. An unrelated shell or machine
failure belongs outside the product report. With no designated product, the
installed instruction does not ask the agent to report incidental failures.

`add` prints the report ID. The observation is stored with product identity,
consumer repo, working directory, agent, and time. Suspected causes and
proposed fixes have separate `--hypothesis` and `--fix` fields.

## Review product usage

```sh
papercut list --product '@nibzard/example-sdk'
papercut show <id-or-ref>
papercut triage-pack --product '@nibzard/example-sdk' --max-tokens 8000
```

Product views span consuming repos unless you add `--repo`. `list` also
supports status, age, and agent filters. `triage-pack` gives an agent complete
observations within a token budget for human-driven review. Group related
observations, verify the usage path, improve the product or its documentation,
and record a resolution reference. A local workaround alone does not establish
that the product obstacle was fixed for later users.

Existing reports without product identity remain available as historical
evidence:

```sh
papercut list --unattributed
papercut triage-pack --unattributed --repo all
```

They are never assigned to a product by guessing from their repo or command.
Historical failed-command signals remain stored, but new broad signal capture
is suspended until an adapter can attribute an interaction to a product.
`papercut sweep` explains this reports-only mode and scans no sessions.

## Privacy and output

The store is under `$XDG_DATA_HOME/papercuts/`, or
`~/.local/share/papercuts/` by default. `papercut render --product ID --write`
writes a projection inside that private store. Explicitly adding `--repo .`
from a matching checkout writes `PAPERCUTS.md` there. Review private reports
before publishing such a projection.

Every command supports `--output json` with one envelope shape. Commands never
prompt. Direct CLI calls use exit codes 0 for success, 1 for a real failure,
and 2 for a usage error. Stale hook calls are silent and never interfere with
the parent task. Reports should contain no secrets, environment values,
transcripts, or source files.

See [the usage guide](docs/USAGE.md) for commands, [the triage guide](docs/triage.md)
for the review loop, and [the plan](PLAN.md) for the architecture and implementation
criteria. For development, run `cargo test` and
`cargo clippy --all-targets -- -D warnings`.
