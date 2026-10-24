# Dead-pane batch recovery

`scripts/aoe-batch.py` inspects or captures dead, open AoE tmux panes across
profiles. Its optional Ledger recovery mode resumes existing Pi and Claude
conversations without sending a prompt. Requires Python 3.10+, `aoe`, `tmux`,
and, for resume, `ledger` on PATH plus AoE v1.18's existing title/lifecycle locks.
Run it on the host owning the tmux server.

```sh
# No process changes. Only open panes positively reported dead by tmux.
python3 scripts/aoe-batch.py status
python3 scripts/aoe-batch.py capture --profile YOUR_AOE_PROFILE

# Preview and validate the original conversation in its original Ledger profile.
python3 scripts/aoe-batch.py resume \
  --profile YOUR_AOE_PROFILE --ledger-profile YOUR_LEDGER_PROFILE

# Canary, then repeat without --limit after inspecting the resumed pane.
python3 scripts/aoe-batch.py resume \
  --profile YOUR_AOE_PROFILE --ledger-profile YOUR_LEDGER_PROFILE \
  --limit 1 --apply
```

`--profile` and `--ledger-profile` name different things. The former is the AoE
workspace; the latter selects the Ledger account/configuration that owns the
conversation. Do not guess or switch accounts to bypass a failed preflight.

Use repeatable `--id FULL_AOE_ID` or `--agent pi|claude|codex` filters to narrow a
batch. `--socket PATH` selects a tmux socket; `AOE_TMUX_SOCKET` is respected.
Otherwise tmux uses its normal socket selection. Remote AoE daemons are refused.

## Safety and scope

- Dry run is the default. Only `resume --apply` changes processes.
- Missing panes, live panes (including idle shells), archived/trashed rows,
  ambiguous multi-pane sessions, unmarked/auxiliary sessions, and the invoking
  AoE session are excluded. The tmux `@aoe_kind` marker must be `agent`.
- Work is sequential, with one attempt per selected pane and a 15-second
  observation window. `--observe-seconds` changes the window, not a readiness
  guarantee. A timeout or failure is logged; there is no automatic retry loop.
- Before each respawn, the script checks the pane's identity, prior PID and dead
  flag again. `tmux respawn-pane` is called **without `-k`**, so tmux itself refuses
  to replace a process revived by someone else. A per-user lock prevents two
  copies of this script from overlapping. Existing AoE title/lifecycle locks are
  held while rechecking the stored row and respawning. Archived, trashed,
  intentionally stopped, changed, or operation-reserved rows are blocked. Missing
  locks are refused. Use `--aoe-dir PATH` for a nonstandard AoE state directory.
- Exact native IDs and transcript working directories must match in the selected
  Ledger profile's shared conversation store. Missing, ambiguous, cross-directory
  and unsupported transcripts are blocked. Nothing is imported, deleted, forked,
  or silently started fresh. Codex recovery is intentionally blocked pending
  manual verification of its rollout and runtime home.
- Resume invokes `ledger run pi ... -- --session ID` or
  `ledger run claude ... -- --resume ID`, not `aoe session restart --all`.
  It does not copy arbitrary wrapper flags, execute wrapper hooks, grant bypass
  permissions, or type wake-up messages. Ledger selects configuration according
  to its installed resume/checkpoint contract. Resuming does not guarantee adoption
  of the current Profile manifest, nor does this script attest restored selections.

This is an operational workaround for incompatible AoE/Ledger resume arguments,
not a replacement for AoE's lifecycle implementation. AoE's persisted bindings
are not repaired; a later ordinary restart may encounter the same incompatibility.
See [session resume](session-resume.md) for the normal workflow.

## Verify and diagnose

`alive-unverified` means the pane survived the observation window, **not** that
its agent is ready or its API credentials work. Inspect each pane for permission,
trust, or authentication prompts before sending work. No model is switched and no
agent is asked to continue automatically. Native CLI behavior is version-dependent:
this script tests selection and invocation safety, not every future Pi/Claude
release's missing-transcript behavior. It never requests a fresh fallback.

Each invocation writes a private directory under
`${XDG_STATE_HOME:-~/.local/state}/aoe-batch/` containing an inventory and
`results.jsonl`. Capture/resume also save terminal output, and resume saves the
session details and proposed command. Treat these files as sensitive. Exit code
1 indicates blocked/failed items or a command error; 0 does not attest readiness.

For missing or mismatched conversations, preserve the evidence and resolve the
original harness, profile, working directory and transcript before retrying.
Never clear a session ID merely to make a failed resume launch successfully.

Run the isolated regression tests with:

```sh
python3 -m unittest discover -s scripts/tests -p 'test_aoe_batch.py'
```
