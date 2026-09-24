# Ledger restart attribution

Ledger-backed host sessions can retain restart evidence without changing AOE's native resume policy. AOE's pane probe is an observation of process liveness. It does not establish that a requested native conversation resumed.

A qualified Ledger launcher first publishes its existing `session report-launch` identity. It may then call:

    aoe session report-ledger-launch --run-id RUN [--restart-intent INTENT]

The optional report uses the caller's `AOE_INSTANCE_ID`, `TMUX_PANE` and `TMUX` socket. AOE stores a separate base64 JSON value in `@aoe_ledger_launch`, containing `instance_id`, `pane_id`, `run_id` and optional `restart_intent`. The existing strict `@aoe_launch_identity` record is unchanged. Old AOE versions may reject the optional subcommand; the original badge report remains available.

Before a managed restart tears down a pane, AOE checks both reports against the same pane and instance. Dead panes may still supply attribution. AOE asks Ledger to record the resolved desired resume or fresh intent with `ledger restart record`, including the previous run, previous native session when available, and the prior pane's ownership context. This command records evidence only. AOE bounds it to three seconds and bounds the preceding pane query to 500 milliseconds. It captures at most 4 KiB of stdout through a nonblocking pipe and discards stderr. Timeout, overflow and post-spawn errors kill the private helper process group and reap its leader, including descendants that keep stdout open. No temporary output file is used. The standalone optional report publisher also bounds its tmux helper to 500 milliseconds. Ledger uses a hidden `--supervised-exec` callback flag: after validating metadata, AOE replaces itself with the structured tmux command in Ledger’s private reporting process group. Ledger therefore owns the entire callback deadline and kills the group on cancellation, with a 100 ms reap/pipe-close bound. This avoids a nested supervisor surviving an outer forced timeout. Missing, invalid or unavailable evidence leaves restart behavior unchanged.

The returned intent ID is passed only to the replacement pane through `LEDGER_RESTART_INTENT`. Ordinary launches clear that key so a tmux server's inherited environment cannot replay an old intent. Ledger validates the intent against the instance, harness and profile, atomically attaches the new prepared run and independently observes source-qualified native session identity. A replacement pane can have a new pane ID; the prior pane ID is historical evidence, not an attachment requirement.

If a replacement fails its resume probe, AOE requests `ledger incident seal --run RUN --json` before cleanup, provided the replacement pane supplies both matching reports. This is a bounded seal-only request and creates no restart intent. Missing collector or metadata preserves cleanup and emits a static evidence-gap status. A successful helper invocation records only request completion, not successful evidence sealing.

Restart intent uses the prior reported Ledger profile. If the user changes the wrapper or profile before restart, strict Ledger attachment may reject the mismatch and leave the prior intent and the independent new run without a verified link. AOE does not parse opaque wrapper commands to infer the replacement profile.

An account-swap restart that carries the conversation to the new account's config root is recorded the same way, before teardown. Its launch command is built only after the carry, so the desired resume is the conversation the carry will copy (or an explicit resume pin), and fresh otherwise. The new account usually launches under a different Ledger profile, so this is the mismatch case above: the prior run is sealed and the intent recorded, and Ledger records the rejected attachment as a gap.

Sandboxed and structured sessions do not currently use this host Ledger adapter. Sessions launched before optional reporting was available retain an explicit attribution gap. Neither an AOE `Resumed` outcome nor a running pane is submitted as verified resume evidence.
