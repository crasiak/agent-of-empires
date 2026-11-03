# Send a prompt to an existing Session

Use [`aoe send`](../cli/reference.md#aoe-send) instead of constructing tmux key
injection. This sends text to an existing Session; it does not create a task
record, grant AFK authority, or prove that the agent completed the work.

## Verify the destination

Before an agent or script sends anything:

1. Confirm the intended machine, Profile and full AoE Session ID with the user.
   Use the AoE ID, not the agent's conversation ID. Titles and project paths can
   match several Sessions; do not use them as automation identifiers.
2. Run on that machine against its local store and Daemon. Do not pass
   `--daemon-url` or inherit an unverified `AOE_DAEMON_URL`. Session lookup for
   `send` uses local Profile storage, while Structured view delivery uses Daemon
   discovery. A remote read does not prove a subsequent send targets that peer.
3. Inspect `aoe -p PROFILE list --json` and
   `aoe -p PROFILE session show FULL_SESSION_ID --json`. Check the exact ID,
   Profile, Working directory and Agent. Verify the selected account/launch
   context separately; a Title or Agent name does not attest an account.
4. Confirm the Session's view and input context. Prefer an already-running
   Structured view Session. If identity, account or readiness is unclear, stop
   and ask rather than creating, reviving or substituting a Session.

These checks are operator checks, not an atomic target/account guarantee from
`send`. For launch identity and account limitations, see
[Native Session Resume](session-resume.md).

## Submit once

Supply one quoted text argument, an explicit Profile and `--no-revive`:

```sh
aoe -p PROFILE send --no-revive FULL_SESSION_ID 'Review the parser change.
Run the focused regression tests and report any remaining failures.'
```

Replace the placeholders with the verified destination. The quoted argument can
contain newlines. `send` does not read the prompt from stdin; avoid secrets in
arguments, which can be visible in process listings and shell history. Send
plain task text, not slash commands or approval responses.

`--no-revive` refuses a stopped destination instead of implicitly starting it.
It does not select an account, change permissions, or guarantee an agent is the
foreground input recipient. Do not remove it as an automatic error fallback.

## Interpret the result

For Structured view, AoE reports **Sent**, **Steered into**, or **Queued**.
An idle worker can receive a prompt immediately; a running turn may be steered
if supported, or the prompt may wait in the Queue. Cancelling, compacting and
worker-unavailable cases can also queue. The caller does not select queue-only
or steer-only behavior through `send`.

These results describe admission/forwarding, not task completion. A queued
prompt is not proof of execution. The CLI prints human-readable output, not a
stable JSON task receipt or correlated completion ID. Inspect the target's
conversation for progress rather than treating an exit status as finished work.

An error or timeout after submission can leave delivery uncertain. Do not retry
blindly: `send` has no idempotency key or exactly-once guarantee. Inspect the
Session and Queue, then ask the user if the outcome remains unclear.

## Terminal view is best-effort input

Terminal view delivery pastes literal text and presses Enter. A live pane can
contain a shell, a permission prompt, or another foreground program. **Sent**
only proves that tmux accepted the input, not that the intended agent accepted
a task. `--no-revive` does not fix this ambiguity. Require an operator to verify
the foreground context before using this path; do not use it for unattended
cross-session task delivery or answering approvals.

Sending never grants permission to create another Session, switch accounts,
change permission modes, answer an Approval, or start/renew
[AFK work](afk.md). Each requires its own applicable authorization.

Plugins that already have the user-approved `session.message` Capability can
use [Messaging any session](../plugin-api.md#messaging-any-session) for plugin
attribution. That interface has its own limits and unknown-delivery behavior;
it is not a reason to install a Plugin for a single send. The current Daemon
MCP interface is read-only; this workflow does not add a mutating MCP tool.
