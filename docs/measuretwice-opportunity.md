# MeasureTwice opportunity in triage

Papercut collects observations. During Layer 3 triage, an agent proposes shared
root causes, remedies, and resolutions. [MeasureTwice](https://github.com/nibzard/measuretwice)
could check whether a *specific proposed conclusion* follows from supplied evidence.
This is a possible addition to the review workflow, not a change to Papercut's
capture path or a replacement for human judgment. See [PLAN.md](../PLAN.md) and
[the triage procedure](triage.md) for the current design.

## First experiment

Run one check in shadow mode: **Do these reports support the proposed shared root
cause?** Give it three answers:

- `supported`: observations provide positive evidence for the same cause.
- `contradicted`: an observation conflicts with the proposed cause.
- `insufficient`: the cause is plausible, but the evidence does not establish it.

Try about eight manually selected cases from real triage. Include clear matches,
clear non-matches, and ambiguous groups. Current examples include the recurring
interactive `mv` alias reports, the different `agent-browser` installation and
performance reports, and the two Astro build failures where concurrency is
verified in one report but only hypothesized in the other. The `triage-pack`
preserves the evidence and context of included reports, but its budget may omit
other relevant records. Inspect individual events with `papercut show <ref>
--output json`, or use `papercut list --repo all --output json` for full event
records. This experiment needs no new CLI command.

Have a person label each case before seeing MeasureTwice's answer. Compare the
answers, inspect disagreements, and record review time. Agreement with an earlier
triage decision alone does not prove correctness. MeasureTwice's offline scripted
example can test the wiring; a real evaluator is needed to test judgment quality.

Keep case files outside the repo. Supply only the relevant private observations
to an evaluator, and review what a live provider would receive. Reports and
suggested fixes are claims to assess, not instructions or independent proof.

## If the experiment helps

Test remedy support and post-fix recurrence as separate checks. A closure check
needs the fix reference, exact timing, and later observations. Reports can carry
a fix reference but have no resolution timestamp, and later observations may fall
outside the pack's filters or budget. Consider a structured export only
if assembling cases from existing records becomes a recurring burden. Update
PLAN.md before changing the command surface or event schema.
