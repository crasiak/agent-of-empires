# AFK control for Pi terminal sessions

AFK currently provides **control-only presence information**, not autonomous
work. Its request-local reminder grants no new authority, answers no question,
and triggers no agent turn. Pending questions remain open.

## Enable, inspect and disable

A session must have been launched by this AoE build as a direct host `pi`
session, with its own native conversation and compatible AFK extension. Older
processes, custom commands/wrappers, sandboxed sessions, ACP and other agents
are not qualified. Do not restart an active session merely to enable AFK.

```sh
aoe session afk on SESSION --minutes 30
aoe session afk status SESSION
aoe session afk off SESSION
```

Select a duration explicitly, from 1 to 1440 minutes. The autonomous allowance
is always zero. `status` performs a fresh probe and never enables or renews a
window. The commands print JSON distinguishing requested state from observed
`control-only`, `off`, `expired`, `invalidated`, `unsupported` or `unavailable`.
An unacknowledged enable is not confirmation of an applied policy. `on` exits
nonzero unless it receives a fresh control-only acknowledgement; `status`
succeeds when reporting unavailable/unsupported. `off` succeeds once the off
policy is durably recorded, even if acknowledgement is unavailable.

In the TUI, select the session, open the command palette, and choose **AFK
control-only: inspect / enable / disable**. Enter an expiry in minutes and
press Enter to request on; `o` requests off, `r` obtains a fresh observation,
and Escape closes the dialog. An Off request made during a pending operation
is queued next and is not discarded when the dialog closes. Observations become visibly stale after five
seconds; refresh before relying on them. Waiting for acknowledgements runs
outside the UI event loop.

Closing AoE's TUI does not renew or remove the window. Pi enforces expiry at
request boundaries and uses a monotonic deadline as well as the absolute
expiry. Reloading/restarting Pi or changing its native conversation invalidates
the previous generation's window. Re-enabling requires a fresh acknowledgement;
conversation replacement may require a new qualified AoE launch.

## Control and failure boundaries

The separate Pi extension is delivered by AoE, not installed into global Pi
settings. It receives an explicit, frozen launch flag, with no environment
fallback for child sessions. One small AoE stdio helper belongs to each
qualified Pi process. It performs bounded anchored file operations; it has no
listener and does not require `aoe serve`. It exits when Pi closes its input,
and Pi terminates/reaps it on shutdown or an unresponsive request.

Private state lives under `<app_dir>/afk/<instance-id>/`: `policy.json`,
`request.json`, `ack.json` and a stable `writer.lock`. Do not edit these files
while the session is running. Protocol version 1 supports only control-only
mode and zero autonomous allowance. Fresh challenges bind acknowledgements to
the instance, profile, native conversation, launch, extension generation and
policy revision. Missing, malformed or stale state is not an enabled control.

Off can be recorded without a responding integration, including after a
profile/native binding changes. A malformed or unsupported stored policy
requires explicit repair: AoE fails closed rather than inventing a revision.
Do not delete the writer lock or edit files under an active integration; stop
that session before repairing its private state.

Inspect the off result:
**off requested** and **off acknowledged** are different. Off affects subsequent
request boundaries; it cannot retract an already-dispatched request or undo a
running tool. No automatic revival, retry turn or question dismissal follows a
failed probe. A failed helper removes the reminder rather than retaining an
old authority claim; reload/relaunch recovery must remain an explicit action.

These same-user files are not a security sandbox. This slice does not impose
a tool-permission policy, restrict arbitrary shell processes, meter spending,
or promise a billing cap. Decision recording and autonomous continuation are
not yet implemented.
