# PLAN.md implementation review

Date: 2026-08-04

`cargo test --all-targets` and Clippy pass, but the implementation is not yet
fully compliant with `PLAN.md`.

## Critical findings

1. **Codex sweep loses failures split across runs.** `pending` calls are kept
   only in memory, while the high-water mark advances past the call. If the
   output line arrives on the next sweep, it is ignored.
   ([src/adapters/codex.rs:134](/home/agent/papercut/src/adapters/codex.rs:134),
   [src/adapters/codex.rs:201](/home/agent/papercut/src/adapters/codex.rs:201))

2. **Signal paths allow path traversal.** Raw `harness` and `session` values
   become filenames without sanitization, allowing malformed input to write
   outside `signals/<harness>`.
   ([src/signal.rs:74](/home/agent/papercut/src/signal.rs:74))

3. **Sweep advances its watermark when persistence fails.** Append errors are
   ignored, but the offset is still committed, permanently losing the signal.
   ([src/adapters/codex.rs:99](/home/agent/papercut/src/adapters/codex.rs:99),
   [src/adapters/codex.rs:224](/home/agent/papercut/src/adapters/codex.rs:224))

4. **`doctor` can report a dead hook as healthy and exits 0 when unhealthy.**
   It checks only whether settings contain a `_hook` substring, not whether the
   executable exists or is executable.
   ([src/commands/doctor.rs:54](/home/agent/papercut/src/commands/doctor.rs:54),
   [src/adapters/claude_code.rs:46](/home/agent/papercut/src/adapters/claude_code.rs:46))

5. **`uninstall` hides real failures.** Adapter removal, instruction-file
   writes, and config writes discard errors and still return success. `install`
   also ignores config-write failures.
   ([src/commands/uninstall.rs:20](/home/agent/papercut/src/commands/uninstall.rs:20),
   [src/commands/install.rs:86](/home/agent/papercut/src/commands/install.rs:86))

6. **Signal repo context is missing and triage ignores repo scope.** Both
   adapters write `repo: None`, while `triage-pack --repo .` clusters signals
   from every repository. This breaks cross-repo recurrence analysis.
   ([src/commands/hook.rs:62](/home/agent/papercut/src/commands/hook.rs:62),
   [src/adapters/codex.rs:214](/home/agent/papercut/src/adapters/codex.rs:214),
   [src/commands/triage_pack.rs:53](/home/agent/papercut/src/commands/triage_pack.rs:53))

7. **Loaded events are not schema-validated.** Invalid schema versions,
   terminal events without resolutions, and malformed `fixed` resolutions are
   accepted after deserialization. Validation is only performed by `add`.
   ([src/store.rs:99](/home/agent/papercut/src/store.rs:99),
   [src/model.rs:138](/home/agent/papercut/src/model.rs:138))

8. **Malformed-event warnings are dropped by `render`.** This contradicts the
   plan's skip-and-warn requirement.
   ([src/commands/render.rs:18](/home/agent/papercut/src/commands/render.rs:18))

## Additional contract gaps

- `--help` has no examples, despite the plan requiring examples.
- Clap usage errors produce plain text rather than the JSON envelope.
- `triage-pack` applies its token budget only to signal clusters, so many
  events can exceed the requested budget.
- `--yes` defaults to `true`, so it is not actually a confirmation gate.
- JSONL appends can leave partial lines during interruption, losing signals;
  the current test explicitly accepts that loss.

Phase 1 is largely implemented, and Phases 2–4 are present, but the failure
paths and adapter correctness issues above should be addressed before claiming
full `PLAN.md` compliance.
