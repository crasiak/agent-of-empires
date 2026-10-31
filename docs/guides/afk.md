# AFK control, Session work and one-cycle delegation for Pi

AFK is off by default. **Control-only presence does not authorize autonomous
work.** Native Session work periods and restricted one-file delegation are
separate opt-ins. Presence and one-file delegation never start an idle turn;
a confirmed native work period can continue the task already assigned to that
conversation.

## Session-bound work periods: native start

The first work-period entrypoint is the attached Pi Session, not a grant file
or AoE transport request. The candidate requires the qualified Pi 1.2.0 runtime
with queue-selection fences, preserving-stop, terminal-settlement, settled-input
and exclusive UI-lifecycle hooks. Stock releases and the earlier 0.87.1 candidate
are not qualified. A version string alone is insufficient; source validation is
not permission to install a candidate or reopen protected history incompatibly.

Canonical `ledger run pi --profile NAME --` launches additionally require the
exact-runtime guard, an immutable core and conversation-only sharing. Direct
and Headroom-isolated launchers are supported; the latter retains Ledger's
managed company transport. The selected profile must pin the exact AFK entrypoint
and its complete imported module closure. AoE uses
the selected Ledger executable and prepared environment, binds its observed
native namespace and refuses changed or unbound launches. Guarded v1 is for fresh
conversations, not resume selectors. Shell wrappers and profile visibility alone
do not establish integration. `/afk-work` must be a registered native command;
a model asking what the command means is not an AFK status or consent dialog.

1. Start the integrated Pi Session and give it the task. Let its first reply
   persist so the conversation and native journal are observable.
2. Enter `/afk-work on`. Choose an explicit duration and request allowance,
   then read and confirm the native dialog. `/afk-work on 120 64` supplies the
   two numbers but still requires the same confirmation.
3. AoE may detach while the managed Pi agent remains alive. The existing
   conversation carries the task, project docs, vocabulary, intent and settled
   decisions. No independent job service, process recreation or reboot recovery
   is introduced.
4. Use `/afk-work` for status and `/afk-work off` to withdraw this period's
   authority. In AoE's AFK dialog, `r` inspects native and host status, `s` stops
   the observed native work period, and `o` is global Off with bulk exclusion.
   Escape closes that dialog; it is not Off.

Duration is 1 through 1440 minutes; allowance is 1 through 512 main-loop
requests. Local tool admissions are bounded to `min(requests * 16, 4096)`.
The first supported tools are normal `read`, `write`, `edit` and `bash`, plus
`aoe_afk_work_checkpoint`. The checkpoint tool must be active before native
consent and effect admission. An explicit tool selector must include it alongside
the desired local tools; AFK never enables an operator-disabled tool. Existing
configured permissions and approval gates remain in force. This is **not an OS
sandbox**: Bash retains its normal ability to start processes. Worker/agent
launching and background work are not qualified and must not be started. Hidden retries, descendant requests and billing are
outside the counter's coverage.

Verified native custom entries record consent and consumption before admission.
Readback proves the observed append, not universal power-loss durability. Missing
consent behind retained work instructions stays guarded rather than becoming
ordinary automatic work. Transport can inspect or withdraw authority, never
create it. Requests and local
tools use the same finite period; a new call ID or repeated old observation is
not new progress. Each owned local result appends a JSON text block such as
`{"aoe_afk_work_evidence_ref":"call_...|fc_..."}` without changing the underlying
result. The model copies the exact field value into the checkpoint's `evidence`
array. This is the real tool-call ID, including any provider suffix, not a guessed
ID or new privilege. Ordinary results are unchanged. Refs are current-period
only, survive subsequent requests within that period, and are not restored as
authority on reload. Replayed results do not add progress; evidence uncertainty
or capacity overflow blocks work. Checkpoints require a nonempty `next_step` for
`unfinished`, and `next_step: null` for both `completed` and `blocked`.
Evidenced unfinished work can continue after early settlement.
Completion claims, declared human requirements/dependencies, unsupported tools,
missing or repeated progress evidence, Off, abort, expiry and exhaustion stop automatic
work. No extra reporting request is added.

Questions and approval gates are not answered by AFK. `no_human_answer` is not
approval; already-open human prompts are retained. A checkpoint's `completed`
status is a **model claim**, and proved terminal settlement is not independent
acceptance of the artifacts or tests. Inspect both when returning. Checkpoint
summaries remain private conversation data, not guaranteed secret-free output.

Reload and restart restore only a protective guard, never active authority or a
fresh allowance. Fresh proven ordinary interactive input can recover ordinary
conversation use without restoring AFK or refunding counters; another period
needs another native confirmation. Automatic restart wake is suppressed for
this conversation's retained work-period history. Incompatible restoration
remains the [preservation release blocker](#optional-already-open-question-deferral)
described below. The legacy overview/review sections below describe host one-file
history, not an independently durable native work-period service.

## Global read-only overview

```sh
aoe session afk overview
aoe session afk overview --active-only
```

The overview reads stored host summaries across **all profiles and sessions**,
including unsupported, stopped and archived workstreams. It does not probe
runtimes, read transcripts, enable AFK, or run startup migrations. The TUI
command palette's **AFK global overview (read-only)** opens the same reader:
Up/Down selects a row, `a` toggles archived/trashed rows, `r` rereads storage,
and Escape closes it. No session selection is required.

Requested presence, host delegation confirmation, reservation counts, expiry,
unresolved question guards and stored acknowledgements are separate fields.
Stored activity is not a live process check. Acknowledgement freshness is
**unknown**; rereading does not make an acknowledgement fresh. Use the targeted
`status SESSION` command for a runtime probe. The overview discards decision
bodies, grant task text and file payloads from its output; it is not a full
ledger validation. Use return review for passive decision detail; ordinary
question recovery remains a separate operation.
Question review remains flagged even after a controlled return is observed or
its guard is recovered: AFK does not record the human's answer. Guard recovery
clears quarantine, not the underlying question.

Unreadable profiles and invalid session/AFK snapshots are reported explicitly.
CLI output has `complete: false` and exits nonzero for an incomplete scan.
`--active-only` hides archived/trashed rows without errors and reports the
filtered count; error rows remain visible. Reads are bounded to 256 directory
entries (including non-profile files), 128 profiles, 4096 session rows,
8 MiB per session registry, 32 MiB per profile authority snapshot and
64 MiB total input. Each profile authority is read once. Capacity failures are errors, not silent truncation.

The overview separates original batch membership, optional shared charges and
per-session bulk preferences from individual windows. **No shared budget** means
no aggregate charge or cap, not zero requests. Recorded batch counters are not
fresh runtime observations. Billing/account attribution remains unknown; SDK
reservations do not prove provider delivery or cost. Global visibility does not
turn sessions into tenants or impose a profile-wide budget.

## Passive return review and annotations

Opening review does **not** stop a window, answer a question or grant authority.
Use a separate explicit Off command when revocation is intended. Review reads
stored facts only; it does not probe runtimes or reread native transcripts.

In **AFK global overview**, Space marks an exact stored window or an original
batch's windows; `v` opens review, using the current row when no marks exist.
`x` clears marks. Select at most16 distinct windows across profiles. Marks keep
original IDs across refresh, never replacement windows or new batch members.
Older stored windows and orphaned history remain selectable.

Return review opens in **Summary**: select a window with Up/Down to see its
outcome, observed effect, human attention and local/shared usage warnings. Names
come from the already-loaded overview, not the historical grant. Profile and
review-local window numbers distinguish duplicate names; missing names have an
explicit fallback. Names never select an operation.

Press Enter for **Decisions** in that window. Up/Down selects a stored decision
or Window context for task, scope and operator notes. Host disposition and model
recommendation remain separate. Alternatives, criteria, weights, scores and
assumptions are not reconstructed when the original claim body is unavailable.
Press `d` for **Diagnostics**, including exact identities, source revisions,
ordered action observations, annotation provenance and usage accounting. This
is the existing redacted report, not raw authority or native transcripts.
`d` or Escape returns from Diagnostics; Escape from Decisions returns to Summary.
PgUp/PgDn scrolls content without changing the target. `?` shows annotation and
export shortcuts; `?` or Escape closes this help without changing the target.
The direct shortcuts work with help closed too.

`r` rereads only the original selections and retains the exact selected window
and decision. A disappeared target stays unavailable rather than selecting a
replacement. Failed rereads retain an explicitly older snapshot; a failed first
read says no snapshot is available. Escape still moves back one disclosure level
while a read or write is pending; it does not cancel the operation. Navigation
never submits an operation. Summary annotations target the window; decision annotations
target the selected record; Diagnostics keeps the target it was opened from.

A failed terminal state does not prove an effect failed or did not occur.
An initial unknown attempt followed by an observed-applied outcome is displayed
as applied; an unknown-only attempt stays unknown. Missing observations are not
proof of no effect. Question history and reviewed claims do not answer questions
or clear host gates. Complete collection does not mean complete accounting.
Local and original shared usage overlap and must not be summed. Usage estimates
and coverage are not an invoice or protection against in-flight overshoot.

The CLI uses exact identities, not titles or the current default profile:

```sh
aoe session afk review --select PROFILE/SESSION_ID/WINDOW_UUID
umask 077
mkdir -m 700 afk-review
aoe session afk export --select PROFILE/SESSION_ID/WINDOW_UUID \
  --format markdown --output afk-review/window.md
```

Repeat `--select` for additional windows; use `--format json` for structured output.
TUI `j`/`m` opens an explicit JSON/Markdown export-path editor at any disclosure
level. Export always includes the full captured report, not just visible detail.
Output must be a new file in an existing owned `0700` directory outside the app
directory; exports
are `0600` and never overwrite files or links. Keep the parent in place during
publication. A durability error may leave a file; inspect before retrying.
Incomplete reports retain errors. CLI review/export exits nonzero even when it
prints or publishes the incomplete report.

Reads allow32 MiB per profile authority and64 MiB total input; formatted exports
allow8 MiB. Capacity failures are explicit, not truncation. Review/export do not
run startup migration. Missing/unmigrated storage stays unavailable; export also
requires the app directory to exist to verify its exclusion boundary. Intentionally
migrate the selected isolated namespace separately when needed. Schema49 adds only
the separate review store, not new execution authority.

TUI `n`, `c` and `v` open note, correction and reviewed-claim editors. Enter appends
explicitly; Escape before submission cancels. PgUp/PgDn inspects long input. A
reviewed claim is **not an answer, approval, guard recovery or deferred-action
execution**. Corrections append instead of replacing originals. Each claim retains
its exact window/decision target and original hash, including body tombstones.
A confirmed submission shows a visible saved receipt, including incomplete export
status. An unrelated successful write does not resolve an earlier uncertain one.
If an uncertain editor is abandoned or an uncertain export is changed, its warning
remains for this review dialog's lifetime. Inspect the destination before retrying;
no automatic retry occurs.

For CLI annotation, save a private owned `0600` request file and run
`aoe session afk annotate --request afk-review/note.json`. Its shape is:

```json
{
  "id": "NEW_CANONICAL_UUID",
  "target": {
    "selection": { "profile": "PROFILE", "session_id": "SESSION_ID", "window_id": "WINDOW_UUID" },
    "decision": null,
    "original_hash": "EXACT_GRANT_HASH_FROM_REVIEW"
  },
  "kind": "note",
  "body": "Operator observation, not authorization",
  "prior": null
}
```

For a decision, set `decision` to its UUID and use its original `payload_hash`.
Kinds are `note`, `correction` and `reviewed`; only a correction may name a prior
annotation on the same exact target. Use one canonical lowercase UUID per claim.
Retry the identical request with the same UUID; changed content conflicts within
that profile. Submitted TUI notes remain fixed for exact retry. Closing an editor
or application cannot undo a dispatched write. After an uncertain result, inspect
retained notes before creating a new claim.

Bodies allow2 KiB UTF-8,16 entries per exact target and1 MiB per profile; tombstones
still count. Registration allows128 profiles and64 KiB encoded index data.
Historical profile names allow240 UTF-8 bytes; identities are never truncated or
renamed. Interrupted first-profile registration can fail closed as unavailable
storage. Preserve it for inspection, not deletion or a new-UUID workaround.

Review applies retention in memory without editing authority. Annotation bodies
expire after seven days beyond the later of window expiry and note creation,
unless ambiguity requires preservation. Explicit append may prune eligible note
bodies, never execution records or counters. Exports omit structured workspace
bindings, grant file lists, action content/preimages and checkpoint next-step
paths. Free-text filtering is heuristic; inspect exports before sharing.

## Explicit profile-scoped batches

`batch preview` exports private **preview-v3**, without activating sessions.
An explicit apply of its reviewed digest is a separate operation. Old
inspection-v1 and preview-v2 artifacts are refused, even if rehashed; use status
for their retained operations and collect a fresh preview for new enrollment.
Global visibility remains independent of this one-profile selection. This
candidate workflow does not authorize runtime installation or release.

Create an owned directory with mode `0700`, then write a `0600` manifest inside
it. Select **1 through 16 exact IDs** from one explicit profile, not titles or a
filter. The manifest profile is authoritative, not the current CLI default.
Every member needs its own existing protocol-4 grant:

```json
{
  "version": 1,
  "profile": "default",
  "minutes": 10,
  "shared_requests": null,
  "members": [{
    "session_id": "EXACT_SESSION_ID",
    "grant": {
      "version": 4,
      "task": "scratch-greeting",
      "scope": "Create scratch.txt only",
      "files": [{ "path": "scratch.txt", "capability": "create" }],
      "requests": 2,
      "assurance": "stock-sdk-main-loop-reservations"
    }
  }]
}
```

```sh
chmod 600 afk-preview/manifest.json
aoe session afk batch preview --manifest afk-preview/manifest.json --output afk-preview/preview.json
# Read the entire artifact, including exclusions, grants and digest.
aoe session afk batch apply --preview afk-preview/preview.json --digest REVIEWED_DIGEST
aoe -p default session afk batch status OPERATION_ID
aoe -p default session afk batch off OPERATION_ID
```

The output directory must also be owned and `0700`, outside the AoE app directory.
The command never overwrites an existing output, including a link. Both files
are bounded to **256 KiB**; each grant retains its **8 KiB** limit. Oversized data
is rejected, not truncated. Treat the artifact as private: it includes operator
grants, displayed identities, raw host launch commands and workspace paths, but
not ledger bodies, file content or human answers. Keep the export directory in
place while inspection waits. Detected parent identity, permission or location
changes refuse publication; this is not a sandbox against arbitrary external
namespace mutation. On a durability or publication-location error, inspect the
requested and any moved directory before retrying; a private file may already exist.

`minutes` is required, from **1 through 1440**. `shared_requests` may be omitted or
null for **No shared budget**, or explicitly **1 through 128**. Zero is invalid.
Previewing never charges the proposed allowance. First apply freezes the absolute
expiry; retries cannot renew it. A configured shared budget additionally requires
the qualified queue-preserving stop capability, not unmodified stock Pi.

The artifact identifies itself as `aoe-afk-batch-preview`, version 3, with
`apply_available: true`. This means an apply attempt is available, not that a
member is activated or guaranteed acceptance. It records a new operation UUID,
collection interval, exact frozen members, grant hashes, host bindings, control
and exception epochs, workspace identity and observed generations. Its `digest` is SHA-256 of
UTF-8 compact JSON after removing `digest` and recursively sorting object keys;
array order is retained. This detects content changes, not authenticity, and is
not an authorization token.

Each member has `exclusions` and `observations`. Missing, stopped, archived,
trashed, unsupported, uninitialized, non-ended delegation, unresolved host human
guards, unsafe workspaces, stale captures and unacknowledged runtimes remain
distinct where evidence supports them. Two fresh null-window acknowledgements
establish **base SDK coverage and generation only**. They do not qualify active
work or the requested nudge/question capabilities. Empty exclusions identify
candidates only; timeout never proves idle or unsupported status. Apply rechecks
per-member evidence, creates accepted unconfirmed windows in one publication,
then awaits exact grant acknowledgements outside the profile lock and publishes
confirmation under it. A changed binding,
generation, grant, control/exception epoch, workspace, expiry or window refuses
that original attempt. Unrelated sibling revisions alone do not invalidate it.
New sessions are never added; active windows are not replaced by a batch.

Inspection does not migrate, initialize authority, repair/quarantine registries,
replace windows, consume allowances or recover guards. Unready namespaces are
refused with an `aoe migrate` instruction. It reads bounded existing authority
and releases its profile lock before acquiring transport locks or awaiting
acknowledgements. Intentional writes are existing-session transport probes/locks
and the explicit output. Independent ordinary runtime activity can still change
state during collection; capture/recheck changes are marked stale, not overwritten.

Apply retries are **report-only**, even after lost or partial acknowledgement:
no re-ACK, re-enrollment, charge or renewal. A reused operation UUID with another
digest is refused. Status and batch Off use the selected or configured default
profile; apply always uses the artifact's explicit profile. Batch Off targets
only original windows and never a later replacement or individual preference.

With a shared cap, ordinary reservations and question/nudge first-use admissions
publish local and shared consumption atomically. First-use tickets hold capacity;
only their exact follow-up can bind them, once, without another charge. Loss,
unknown outcomes, Off and a human winner never refund tickets. Shared exhaustion
refuses a new charge without invalidating held follow-ups or final admitted tool
rights; individual bounds, expiry and Off still veto. Question denial precedes
any close/intent and leaves the original UI open. No automatic retry or unbudgeted
ordinary dispatch follows denial; fresh proven ordinary input can recover without
restoring AFK authority. No-budget mode creates no shared tickets.

`off SESSION` and `opt-out SESSION` revoke and persist **excluded**. `on` and
individual `delegate` do not erase it. `allow-bulk SESSION` changes only the bulk
preference/exception epoch, grants nothing, and requires a fresh preview before
batch enrollment. Migrated history is **unrecorded**, not an invented prior
opt-out. A fresh explicit preview/apply may select unrecorded or allowed members.
At 64 retained operations the next apply is refused, never an eviction of replay
identities.

In the TUI command palette, **AFK batch: load preview / review / explicitly
apply** loads the same private file through the shared service. Enter loads only;
review all grants, exclusions and the digest, then press `a` to explicitly apply.
Up/Down and PageUp/PageDown scroll. After an attempt, `r` inspects its original
operation and `o` stops its original windows. Closing does not undo a requested
operation. No automatic apply retry is offered.

## Optional observed usage guards

Usage visibility is independent of enforcement. Omitting a guard preserves the
request/expiry/file limits above and imposes no spending-based veto. A per-window
`usage_guard` and a frozen batch's `shared_usage_guard` are separately optional;
they are not global profile/account budgets. A shared request cap remains a
separate reservation bound, not a financial reservation.

This candidate supports two explicitly chosen measures:

- Reported native SDK component tokens: input + output + cache read + cache write.
  Reasoning is already in output; one-hour cache writes are a subset of cache writes.
- Those observed components valued at **frozen operator-supplied USD rates**.
  This is an authorized estimate, not an invoice, subscription bill, SDK-certified
  price or physical-request ceiling. Unsupported stronger assurances are refused.

For an individual window, create a private owned `0700` directory and a `0600`
policy file. Example values below are assumptions to replace, not current prices:

```json
{
  "version": 1,
  "basis": "observed-native-entries",
  "tokens": 25000,
  "usd": {
    "currency": "USD",
    "basis": "operator-fixed-rates",
    "limit_nano": 500000000,
    "quotes": [{
      "model": {"provider": "PROVIDER_ID", "model": "EXACT_MODEL_ID"},
      "input": 5000000000,
      "output": 15000000000,
      "cache_read": 500000000,
      "cache_write": 6250000000,
      "cache_write_1h": 10000000000
    }]
  }
}
```

```sh
aoe session afk delegate SESSION --minutes 10 --grant scratch-grant.json --usage-guard afk-usage/guard.json
```

Amounts are integer nanoUSD; one billion nanoUSD is one USD. Quotes are nanoUSD
per million tokens, not prices fetched from a catalog. Each source entry's total
weighted value is rounded upward once, then summed. Report pagination and replay
do not change rounding. Missing or null `tokens`/`usd` disables that measure;
a policy must select at least one. Explicit zero quotations remain visible
operator assumptions and never fill missing usage or model identity. At most16
exact model quotations and8KiB serialized policy are accepted. Tiered/subscription
billing and non-USD currency are not silently approximated.

For a batch, put a policy object in each selected member's optional `usage_guard`
and/or the manifest's optional `shared_usage_guard`. The existing manifest version
remains1; preview-v3 includes these policies in the reviewed digest. Grant-specific
wire6 acknowledgement binds their immutable policy hash. A guarded window requires
proven queue-preserving stop and a fresh, reconciled persisted native source.
Wire5-only or unmodified stock runtime capability cannot qualify it. Financial
acknowledgement requires `native-entries-terminal-v2`, including the qualified
`endRunPreservingQueue()` operation; the older preserving-abort capability alone
is insufficient.

The baseline cursor is frozen at first durable window creation, not preview.
Finalized ordinary work already in flight can fall after that baseline and is
included conservatively. Stop/expiry/terminal transitions revoke work immediately;
accounting may still be closing. A fixed qualified closing watermark must precede
later ordinary dispatch. Unattributable late work/restart remains a gap, not a
moving cutoff that charges all future conversation or a claimed final zero.

The reader uses raw native entries, including abandoned branches, compacted and
context-edited history. It does not use context occupancy as cumulative spending.
Surfaced assistant attempts and qualified independent usage are kept; ambiguous
nested tool/descendant rollups are excluded with a gap instead of double-counted.
A summary can report tokens without a priceable model. Synthetic abort/error
entries are not extra physical requests or proof of free transport. SDK-recorded
cost is shown separately with unverified rate/subscription basis; it does not
govern the operator-rate guard.

Incomplete, contradictory, stale or unmeasurable chosen evidence blocks a required
guard. No guard does not turn those conditions into a financial veto. Genuine
publication quarantine still blocks authority. Host-issued five-second leases
bind boot, helper incarnation and the exact committed source watermark. Heartbeats
refresh transport evidence only; unchanged observations do not publish the entire
profile. Global overview remains stored-only and never promises current liveness.

Observed thresholds block new automatic admission, question/nudge intent and
subsequent file effects. Existing count tickets are retained, not refunded or
permission to override a spending guard. Already admitted concurrent requests may
overshoot before their results are reported; supported cancellation is best effort.
A committed action attempt keeps its existing revocation boundary. No predicted
financial slots or exact bill ceiling are claimed.

A spending-denial latch also suppresses automatic continuation/cache warming after
terminal state or expiry until qualified ordinary recovery or fresh explicit
enrollment. Successful file completion remains protected while the SDK episode
closes. Automatic local release requires a current terminal ticket after awaited
final handlers, quiescent SDK warming and complete accounting through the exact
native head. Polling or a tool result alone cannot release it. Queued work remains
intact; no reporting request is added.

Later native changes, negative or missing host evidence, and obsolete tickets
revoke a live completed exemption. On native reopen, a historical completed
marker alone remains protective until qualified ordinary recovery or a confirmed
new handoff. Shared-policy automatic completion release is not qualified. A question not yet closed by cooperative deferral remains open. Ordinary
recovery records no human answer, restores no AFK authority and refunds nothing.
Unknown auxiliary work and descendants remain outside complete-account assurance;
Restricted one-file delegation cannot launch descendants.

Use `status`, `audit`, batch status and the global overview for the stored amounts,
coverage, baselines/cutoffs, policies and gaps. In the global TUI overview, `u`
toggles usage detail for the selected row. `partial_or_unavailable`, missing price
and stale freshness are not zero actual spending. Unsupported/unobserved sessions
remain visible. These are candidate capabilities, not permission to install,
migrate a live app or claim D014/S5/S9 release acceptance.

### Restart wake is automation

Direct Pi restart wake uses a labelled native custom message, never terminal
keystrokes or a human-input entry. Both CLI and TUI use the same private one-shot
transport. Empty `session.restart_wake_message` disables it; invalid configuration,
unsupported receivers or uncertain binding/clock evidence suppress it without
substitute text or a raw-input fallback. A queued/attempted receipt does not prove
model execution, and failed or lost delivery is not retried for that launch.

Control-only presence and any AFK history for the current native conversation
suppress automatic wake, even after a window ends. Unresolved native guards,
write-failure custody, draft/input/UI activity or a busy receiver also suppress it.
Use fresh ordinary input yourself when appropriate; restart is not recovery or a
human answer. A positively established unrelated NoGuard conversation can receive
one configured wake. Requests expire on host boot/continuous time, and receiver
age is checked from before Poll without renewal on receipt.

The initial Darwin receiver-clock qualification is Node24.13.0/libuv1.51.0;
unknown combinations disable wake only. Linux uses bounded `/proc/uptime` reads
with conservative precision. Native usage intents preserve denial across compatible
restoration; failed native writes stay blocked across extension reload. These are
unreleased candidate paths requiring final S7 and release qualification, not
permission to install them. Incompatible-runtime preservation remains a separate
gate below.

## Control-only presence

A session must have been launched by this AoE build as a direct host `pi`
session or guarded canonical Ledger Pi launch, with its own native conversation
and compatible extension. Older processes, custom shell wrappers, sandboxed
sessions, ACP and other agents are not qualified. Do not restart an active session merely to enable AFK.

```sh
aoe session afk on SESSION --minutes 30
aoe session afk status SESSION
aoe session afk off SESSION
```

Choose an expiry from 1 to 1440 minutes. `on` grants zero autonomous allowance
and exits nonzero without a fresh acknowledgement. It also ends any previous
delegation. `off` durably revokes future work, including delegation, even when
runtime acknowledgement is unavailable, and persists a bulk exclusion. Corrupt or unsafe storage requires
explicit repair rather than a guessed revision.

In the TUI command palette, **AFK control-only: inspect / enable / disable**
opens the shared status/control dialog. Enter requests control-only presence;
`o` requests global Off, `r` refreshes both native and host observations, `s`
withdraws observed native work authority, and PgUp/PgDn scrolls details. Escape
closes the dialog, not the period. Native work starts only through `/afk-work on`
and its confirmation; one-file delegation uses the separate CLI command below.
Off publishes independently of a pending On/status exchange and survives closing
the dialog. Observations become stale after five seconds. Closing AoE does not
renew or remove either period.

## Explicit one-cycle grant

Create a grant yourself outside model tool execution, for a disposable task in
the session's project/worktree. Do not use a model response as operator authority.
For example, save this as `scratch-grant.json`:

```json
{
  "version": 4,
  "task": "scratch-greeting",
  "scope": "Create scratch.txt with a short greeting; no other effects",
  "files": [{ "path": "scratch.txt", "capability": "create" }],
  "requests": 4,
  "assurance": "stock-sdk-main-loop-reservations"
}
```

```sh
aoe session afk delegate SESSION --minutes 10 --grant scratch-grant.json
aoe session afk status SESSION
aoe session afk audit SESSION > afk-audit.json
```

There is no request-count default. Choose **1 through 8**; a count of one cannot
both record and apply because apply requires a later request. The grant may
list up to 16 exact relative files with `read`, `create`, or `replace`
capability. Create/replace also permit bounded reads of their target. No
recursive, wildcard, delete, process, shell, network, permission, scheduler or
agent-launch capability exists. Stronger request/cost assurance is rejected,
not silently downgraded.

Only UTF-8 `.txt`, `.md`, `.rs` and `.json` files are initially supported.
Hidden paths, protected metadata/configuration and detected credential paths,
executable files, symlinks, hardlinks and nonregular files are rejected. Parent
directories must already exist. The root and its identity are frozen at the
operator grant. Expected SHA-256 hashes detect ordinary stale content; they
are **not filesystem compare-and-swap against uncoordinated writers** and this
is not a hostile-process sandbox. Review the scope carefully.

### Ownership and human precedence

Start the assigned task before requesting delegation. Idle or otherwise
unobservable executable activation is refused and its attempted window is
durably ended, not held for a later request. The pinned SDK does not expose
every idle abort to the extension.

A freshly acknowledged grant is **pending**, not retroactive authority for an
ordinary response or its queued tools. The ordinary operation must have a live
observable abort signal before acknowledgement. The next qualified core request
boundary, after its prior tool batch settles, can reserve a slot and become
**owned**. If ordinary work settles first, the pending window ends. Stop,
settlement and stale polling cannot revive it: resumption requires a fresh
explicit operator command/window and acknowledgement. Queued human input and
ordinary follow-ups are preserved; they cannot adopt an ended grant.

The runtime supplies `aoe_afk_read`, `aoe_afk_record`, `aoe_afk_apply` and
`aoe_afk_checkpoint`. Each
owned request requires a durable reservation. A weighted record and its one
exact action permit must be durably acknowledged before apply on a **later
request**. Same-response siblings cannot satisfy that boundary, even when
executed sequentially. The final admitted request retains its valid tool
rights; no extra reporting request is granted. Applying the one action ends
the cycle, with a host-observed result or a conservative unknown outcome.

Records retain alternatives, criteria and coarse weights/scores, evidence,
assumptions, uncertainties, recommendation, risk and rollback. The record tool
advertises a typed decision: UUID `id`, two to four alternatives with **string**
`pros`/`cons`, one to four criteria, and integer weights/scores from 1 through 5.
Evidence, assumptions, uncertainties and dependencies are string lists. `action`
is null or an exact `{path, expected_hash, content}` object; `expected_hash` is
null for an absent file or its lowercase SHA-256 hash. Unknown fields are rejected.
The host additionally checks score/criterion alignment, recommendation membership,
UTF-8 byte limits, current authority and file preconditions before admission.
These are **model claims**, separate from host identity, admission, counters and outcomes.
Declared human requirements, dependencies, unknown/high risk and unsupported
capabilities cannot obtain a mutating permit. Arbitrary natural-language scope
and risk classification are not machine-proven safety.

During an owned episode, a future `ask_user` call with the supported 0.12-style
question/options shape is blocked before execution with `deferred_by_afk` and
`no_human_answer`. It supplies neither an answer nor permission approval. Its
question remains human-required; only independent granted work may proceed.
Unknown question shapes and other tools stop the episode. Already-open dialogs
remain untouched unless the operator explicitly grants the qualified cooperative
deferral below. Other UI interaction completion conservatively pauses pending
delegation without assuming the human answered or cancelled.

Human input is retained intact and pauses delegation. Explicit stop, off,
expiry, local exhaustion, other failed reservations and ordinary settlement end
it. Shared-budget denial instead preserves existing host rights as described
above. A stopped or exhausted
owned episode stays guarded through settlement, so an ordinary automatic
follow-up cannot escape its limit. Normal tool selections are not rewritten.
Resuming delegation requires a new operator command and fresh acknowledgement;
unused counters, reloads, polling and model requests do not resume it.

### Optional settlement nudges

Delegation, including zero-nudge grants, requires the qualified Pi 1.2.0 context
operations and corrected queue-selection fences. Ordinary abort is not a
queue-preserving substitute. Unsupported cores refuse acknowledgement without
aborting unrelated ordinary work. Source qualification is not an installed
runtime upgrade.

Protocol-4 grants may explicitly specify `"settlement_nudges": 1` or `2` when a
qualified runtime provides that operation. Omission means **zero**, and two is
the hard ceiling. This allowance adds neither request slots nor file permits.
No timer, idle start, post-settlement restart or new task is introduced.

`aoe_afk_checkpoint` records bounded model claims (`unfinished`, `completed` or
`blocked`), a granted next step and references to host evidence returned by reads
or decisions. The host derives checkpoint sequence, request, window, generation
and time. Completed/blocked work or declared prerequisites ends the window;
missing, stale, out-of-scope and repeated evidence cannot admit another nudge.
Prose changes, fresh IDs, timestamps, counters and identical reads are not new
progress. A nudge requires changed observed path/hash or admitted action facts.
These checks do not prove semantic task completion or natural-language scope.

Only an already-owned completed candidate settlement boundary can contribute a
labelled custom message. Two ordered handlers admit durably, then check Pi's
refreshed queue/draft preview without replacing earlier entries. Final settlement
always retires ownership. Later contexts filter stale AFK instructions without
rewriting transcript history, and reject newly introduced user/custom work from
AFK authority.

There is **no universal priority guarantee against later trusted hooks**. An
invisible late custom follow-up may remain queued while an earlier qualified
AFK request completes. It must never inherit AFK rights. A qualified runtime must retain the original queue/transcript data for later
ordinary recovery. Native attachment recovery is not claimed lossless. No controller re-send or input reconstruction is performed.

Admission, contribution intent and observation of the next request are separate
ledger facts, none a claim of provider delivery. Stop can veto dispatch even
after a custom entry commits. Unobserved admissions end as `delivery_unknown`,
with no refund, replay or automatic reconciliation. Other terminal reasons are
`completed`, `deferred`, `exhausted`, `interrupted` and `failed`; checkpoint
completion remains a model claim distinct from an observed file effect.

### Optional already-open question deferral

**Stock Pi refuses nonzero question-deferral grants. This source integration is
not released or an installed runtime upgrade.** A qualified native runtime and
cooperative question package may accept `"question_deferrals": 1` in a protocol-4
grant. Omission means **zero**, and one is the hard ceiling. It adds no request
reservation, settlement nudge or file permit. Control-only presence never closes
questions.

The controller must have observed the ordinary assistant batch containing exactly
one supported `ask_user`, its live stop signal and the matching fresh opened
interaction before activation. It requires actual queue-preserving abort,
exclusive UI ownership, targeted cancellation and settled input-pipeline
provenance (`pipeline-v1`) capabilities, plus an enabled
persistent native session. Startup capability snapshots, siblings, RPC, inline
UI, missing/truncated metadata and stale IDs cannot establish ownership. The
qualified integrated paths are native input and custom-overlay options. A
custom-overlay to select fallback changes kind after opening and is refused;
component-level select support alone does not qualify that transition.

An exact operation admission is durable before emission. Emission intent, cleanup
outcome and the next request reservation are separate facts. The close requires
matching reply, exclusively attributed UI end, package closed event and real tool
result. Any mixed/unrelated UI end or actual ordinary input still pauses. A proven
ordinary answer or cancellation winner keeps its original outcome, never an AFK
answer. No whole-turn abort is used merely to dismiss the open dialog.

Deferral reports `no_human_answer` / `deferred_by_afk`, with null response. The
question remains an immutable, human-required unanswered gate in audit and in the
reserved context. It cannot satisfy an approval or ask-first prerequisite.
Declared dependencies cannot obtain a file permit. Only independently granted
work may proceed, and independence remains a model claim, not host-proven
natural-language safety.

Intent guards the controller-caused next request even after off, expiry, lost
reply or failed persistence. Unknown outcomes are parked, never refunded,
resent or treated as successful cleanup. Admission-only receipts remain
discoverable for exact explicit recovery but cannot quarantine a return they did
not cause. Recovery consumes no new attempt and restores no AFK authority. A matching accepted close still needs
an ordinary durable reservation before any independent provider step. Exhaustion
stops through the preserving operation without removing queued work.

A versioned native custom-entry marker stores identity and phase only, not a
question, answer or draft. It conveys no authority. Unresolved native provenance
or host receipts survive reload and can quarantine an automatic return even when
the other store is unavailable. A fresh interactive **text** input, corroborated
by its new native user entry, can recover ordinary work without restoring AFK.
Native ordinary recovery is separate from host receipt cleanup. If host cleanup
fails, the receipt remains discoverable for a later fresh qualified input; no
background retry, attempt refund or deferral resend occurs. Already recovered
ordinary work is not quarantined again solely because cleanup is outstanding.
A core-owned settled provenance view must show no transform, handling or
handler failure anywhere in the input pipeline. Earlier transforms, later
transforms and transform-then-revert do not count, even when the resulting text
matches. Extension-origin input and missing provenance cannot resolve quarantine
or host receipts. Malformed or conflicting markers require inspection rather
than inferred recovery. Partial
draft recovery is not claimed. The host ledger and native history are not an
atomic transaction; synchronous native append is not a universal fsync or
power-loss guarantee.

**Release blocker: incompatible restoration is not preservation-safe.** Reopening
an unresolved guard under a runtime without preserving abort cannot currently
both prevent dispatch and retain native custom queues. The isolated development
containment branch blocks dispatch with ordinary abort and explicitly warns of
possible custom-queue loss. Its synthetic loss fixture is limitation evidence,
not supported restoration or permission to expose real queued work. Release
requires preventing incompatible restoration or qualifying a preserving stop at
that boundary. Do not install this path based on candidate test results.

### What the request bound covers

The bound is **qualified native-SDK main-loop reservations on Pi 1.2.0**, not
physical HTTP attempts or a billing ceiling. The wire assurance name
`stock-sdk-main-loop-reservations` does not certify an unmodified stock core. Reservations are consumed before
dispatch and never refunded on failures. Denied/failed reservations explicitly
abort through the SDK; tool blocking alone is not a dispatch gate.

Owned automatic compaction is cancelled; navigation pauses instead of creating
a tree summary. Cache warming is suppressed during pending/owned work, including
owned settlement, and while required usage protection or failed-write custody
remains unresolved. Terminal state and expiry do not clear that protection.
Unguarded, unowned terminal windows otherwise leave native warming unchanged.
No global warming setting is changed. Provider retries,
independent extensions, pre-existing descendants and unrelated background work
remain outside this count and are displayed as independent/unknown coverage.
Other extensions are trusted same-process code, not sandboxed adversaries.

## Audit, limits and recovery

`status` distinguishes host pending/owned/ended state from fresh runtime
observation or unavailable/invalidated acknowledgement. It includes the exact
grant, counters and coverage. The TUI shows the same counters and does not label
an executable delegation as zero autonomy. `audit` exports JSON projections of
retained windows, model recommendations, host dispositions, hashes and outcomes.
It omits action payloads and private preimages. Review before sharing.

| Host limit | Ceiling |
| --- | --- |
| Main-loop reservations | Explicit 1 to 8 per window |
| Settlement nudges | Explicit 0 to 2, omitted means 0 |
| Question deferrals | Explicit 0 to 1, omitted means 0 |
| Checkpoints | 16 per window, 2 KiB serialized claims each |
| Reads | 16 operations per window, 64 KiB per file |
| Decision records | 4 per window, 8 KiB serialized UTF-8 each |
| One file effect/preimage | 64 KiB each |
| Window snapshot | 1 MiB |
| Retained session ledger | 8 MiB |
| Profile authority snapshot, including pending-publication envelope | 32 MiB |
| Batch members / retained operations | 16 / 64 |
| Optional shared SDK requests per batch | Explicit 1 to 128; omitted/null has no cap or charge |
| Native source entry identities | 8192 per source |
| Usage report | 128 entries / 128 KiB |
| Reported provider/model groups | 16 per source |
| Usage policy | 8 KiB / 16 exact quotations |
| Runtime stdio frame | 256 KiB |
| Presence/control object | 16 KiB, unchanged |

The initial record tool includes intended content in its 8 KiB decision object,
so new content must also fit that smaller combined record limit. No content is
silently truncated. Private preimages support manual rollback; there is no
automatic replay or restore command. Read content and tool arguments can enter
native Pi/provider history, whose retention is **separate** from these bounds.

Heuristic filters reject detected credentials/private keys and dangerous
control characters. They cannot guarantee arbitrary text is secret-free.
Expired bodies are pruned on audit or new delegation after seven days. Live
counters and replay tombstones remain. Data needed to interpret an ambiguous
attempt is retained regardless of age, still counts toward quota, and can block
new grants. There is no automatic reconciliation or quota-reset shortcut.

### Profile authority and upgrades

Presence, individual history, batch membership, preferences and optional shared
tickets share one private snapshot at
`<app_dir>/afk-authority-v3/profiles/<profile>/snapshot.json` and one stable
profile writer lock. Session IDs are scoped to their exact profile. Transport
probes and acknowledgements cannot grant authority. The launch/stdio bridge and
runtime acknowledgement use wire 6. Policy remains 1; Book/Window/grants remain 4;
native question markers and cooperative events remain version 1.

Before each canonical publication a private pending journal durably records its
identity, prior hash and full bounded proposal. It is a **veto**, never another
source of authority. Uncertain publication parks the profile without refund or
automatic repair. A known data commit with retained journal is committed but
parked. A committed snapshot whose journal removal has uncertain directory-sync
durability returns a publication warning, not a failed activation; restart may
restore only a veto. Preserve these diagnostics and never delete a journal to
force activation.

This line supports fresh app namespaces and ordinary main-line upgrades. Main's
published migrations 1 through 41 retain their meanings; AFK uses global steps
42 through 49. Old AFK development stores from the divergent 36 through 43 line
are not automatically converted. Intact incompatible layouts are refused before
migration bodies or schema publication; acquiring the runner lock may create its
lock file. Ordinary data is assumed to have main-line provenance, not to be a
development store whose identifying files were removed.

Use fresh private state for integrated-build testing. Keep old development data
with its frozen reader and stop old writers before inspection. Do not relabel,
remove or lower `.schema_version` to bypass refusal. A development-store conversion
requires a separately qualified procedure.

Run `aoe migrate` normally in the selected app namespace before launching the new
integration. The early bridge refuses older, incomplete or corrupt authority
rather than running migrations or creating an empty replacement. The overview
remains read-only before migration and shows missing/unavailable authority beside
session rows. It never repairs storage or makes stored acknowledgements fresh.

Global migration 46 preserves verified original bytes in manifest format 40 under
`<app_dir>/afk-authority-v1/migration/`, with hashes and ownership in its durable
`manifest.json`. Before inventory, it records legacy root identities and seals child creation at
mode `0500`, qualifying denial through held root descriptors. Unsupported
privileged identities, ACL/filesystem semantics or durability failures refuse
completion. Roots stay sealed; do not relax their permissions for rollback.
It fences actual legacy policy/ledger leaves under their existing locks,
including prepared stores with absent leaves. Absence is recorded without
fabricating originals. Root, child-directory and lock inodes remain in place. Full protocol-4 history is
imported with presence disabled and windows ended/unconfirmed, without resetting
counters, renewing expiry or resolving unanswered question guards. Protocol-2/3
originals remain fenced evidence, not converted grants; `audit` and the overview
expose their private references. Valid empty legacy Books without a stored owner
are explicitly unassigned evidence in the manifest and global overview.
Assigned history without a current registry profile/session appears separately
as historical authority, with redacted references and guard summaries, never as
an eligible live session.

Global migration 47 preserves verified v1 originals in manifest format 41, seals
v1 profile creation and fences actual present or prepared-absent snapshots before
importing nonauthorizing v2 history. It preserves guards and counters, not live
windows or invented bulk preferences. The v1 import must finish before the first
durable v2 start; afterward retries verify frozen historical evidence without
reopening or reconstructing the old active namespace. A completed newer snapshot is never
overwritten by replay.

Global migration 48 keeps the frozen v2 serialization and manifest format 42,
preserves its verified originals, permanently seals v2 profile creation and fences populated
and prepared-absent leaves under stable locks. The new v3 snapshot preserves
preferences, batches, charged/held tickets and human guards, with no active import
or invented usage. Pending publication journals remain vetoes, not repair input.
After durable start, historical retries cannot reconstruct the old namespace.

An interrupted migration resumes from verified originals and refuses conflicts,
missing evidence, unsafe files or capacity overflow. Never delete its manifest,
locks or originals to force a retry. A previously committed effect cannot be
undone by fencing. Do not restore active legacy files when reverting code; see
[Downgrading](../installation.md#downgrading). Ordinary exact question recovery
remains available after import and restores no AFK authority.

Fresh challenges bind
capability acknowledgements to the instance, profile, native conversation,
launch, generation, window and exact grant.

Record/admission commits precede acknowledgement. The durable action-attempt
commit is the revocation boundary: off/expiry before it denies the effect;
an already-committed attempt may finish or remain unknown. A crash or lost
response does not refund or replay the permit. Stop the session before manual
inspection/repair; do not delete its active writer lock or edit live ledger
files. Preserve ambiguous payloads/preimages until independently reconciled.

## Preparing a private native diagnostic

The engineering adapter at `assets/session/aoe-afk-native.py` is separate from
controlled-fixture qualification. Its implementation and fake-process tests do
**not** qualify a native attempt or authorize execution. Native execution needs
review of the exact candidate, manifest and proposed synthetic pricing, followed
by two explicit approvals for each attempt. Do not use existing conversations,
installed executables or a live profile.

Preparation requires existing dependencies, no installation. In a private `0700`
directory, create a `0600` configuration with these fields:

- `sources`: absolute `aoe`, `pi` and `question` checkout paths. Pi and question
  must match the adapter's fixed commits. AoE must match the reviewed build
  receipt and descend from the declared baseline, with no tracked changes.
- `binary_receipt`: the reviewed build receipt object containing `source_commit`,
  `source_tree` (Git tree ID), `binary`, `binary_sha256`, `profile` and
  `installed: false`. A receipt
  from an earlier build is not valid after changing the candidate source.
- `tools`: `node`, `python`, `tmux`, `aoe`, `fd`, `rg` and `loader`, each with an
  absolute `path` and exact `sha256`. `loader` must be that Pi checkout's
  `node_modules/tsx/dist/loader.mjs`. Python must support `Popen(process_group=)`;
  tmux must support its foreground `-D` server mode.
- `node_version`: exact version output, for example `v24.13.0`.
- `app_name`: `agent-of-empires-dev` for a debug receipt, otherwise
  `agent-of-empires`.
- `preview_approval_seconds`: optional, from 1 through 30, default 30. This wait
  becomes part of the immutable manifest and must be reviewed before execution.

Use a short canonical private path so Unix socket names fit:

```sh
python3 assets/session/aoe-afk-native.py prepare /private/path/config.json /private/tmp/native-diagnostic
```

`prepare` hashes source, dependency-directory targets, executables and the scoped
loader. The pinned peer-only question checkout may omit `node_modules`; its seal
then names Pi's pinned dependency closure. Any local dependency directory is
sealed separately, and broken links are refused. Preparation probes only Node
version metadata. It does not run Pi, AoE, tmux,
providers or migrations, or inspect live state. Prepared Python launchers use
isolated, no-site, no-bytecode startup; fixed internal helpers compile bounded
source bytes rather than accepting an ignored stale bytecode cache. Review the
resulting private `manifest.json`, its printed digest, and the artificial model/usage/rate table.
The manifest proposes a 100-nanoUSD local guard, not historical pricing or billing
proof. Source hashes and approval files are integrity checks, not authentication
against hostile same-UID writers or proof of human intent outside explicit invocation.

**Only after separate execution approval**, create a new private approval JSON:

```json
{
  "kind": "aoe-afk-native-v1",
  "manifest": "EXACT_REVIEWED_MANIFEST_SHA256",
  "role": "baseline",
  "ordinal": 1,
  "approve_native_execution": true,
  "approve_synthetic_pricing": true
}
```

Then explicitly invoke the prepared `run` launcher with `--approval PATH
--role baseline --ordinal 1`. It records admission before launch. A fresh private
HOME/XDG namespace is initialized by the pinned `aoe migrate`; real `aoe add
--tool pi` and `session start` produce the native binding. No existing profile is
reused. Before even private migration, its AoE configuration disables systemd
scopes, update checks, restart wake and general session usage tracking. Native
question/usage-guard evidence collection remains enabled and independent of that
optional general tracking. A pane-local parent owns Pi's waitpid and audit/control
pipes; the outer guardian owns the exact foreground tmux server and terminal identity.

After attributed live question readiness, the controller writes `baseline/preview.json`
and `baseline/preview-approval-required.json`. **Review the entire preview and
publish a separate new private `baseline/preview-approval.json` matching the
required manifest, role, ordinal, operation ID and preview digest, with
`approve_apply: true`.** The manifest-declared approval wait is bounded. Without
that exact file the controller never applies. The 60-second AFK window begins
only on the single explicit apply, not at startup or readiness.

One baseline and one observed slot exist, each one-use. An observed invocation
requires an independently reviewed successful baseline report and its own
`role: observed`, `ordinal: 2` approval. Failed or inconclusive admission consumes
the slot. There is no retry, automatic observed attempt, grant replay, recovery,
refund or reset. Further attempts require a new decision, not deletion of receipts.

Reports separate endpoint, intent coverage, cancelled SDK settlement, audit EOF,
waitpid and terminal cleanup. Publication uncertainty stays sticky through final
audit, including bounded late warning/commit evidence after endpoint capture;
it vetoes qualification without overwriting the pre-teardown endpoint. Observation covers bridge and exposed refresh calls,
not startup/timer lexical refreshes or unobserved internal phases. Live prompt
ownership is not rendered-frame proof.
The fixed second provider stream emits no tools or effects and remains abortable.
Completion is armed through a real command-context idle wait before cancellation,
followed by explicit Off and shutdown. Aborted usage gaps remain retained; this is
not successful task completion, financial finality or original-race causality.
Independent capture uses bounded passive reads plus the early read-only
`__afk-observation-clock` helper, never extra audit/poll timing probes.

If cleanup is uncertain the guardian retains the exact server and cleanup owner
for inspection instead of blindly killing them. Preserve the private directory
and its uncertainty receipts. Do not kill a recorded historical PID, remove
journals or edit guards to make the report green. A returned qualified native
report still requires independent review; implementation-only testing cannot
substitute for the later approved experiment.

## Reproduce the offline qualification

No npm install or provider credentials are required. Point `AOE_PI_ROOT` to an
existing qualified Pi **1.2.0** package root containing `dist/index.js` and its normal
peer dependencies. The tests load the extension through Pi's actual loader,
including core and TypeBox imports, with in-memory settings/history and an
offline registered provider:

```sh
export AOE_PI_ROOT=/path/to/node_modules/@earendil-works/pi-coding-agent
node --test assets/session/aoe-afk.test.mjs assets/session/aoe-afk-sdk.test.mjs
SHELL=/bin/bash cargo test --lib afk -- --test-threads=1
```

The Rust suite includes a real ledger/stdio plus SDK record-to-apply test when
`AOE_PI_ROOT` is set. Without it that optional integration reports its setup
requirement; pure host and control tests still run. The SDK matrix asserts
provider invocation counts, sibling ordering, last-slot behavior, store/lost
acknowledgement failures, stop/input/UI interaction, idle rejection, pending
stop/settlement races, stale-window non-human follow-ups, and warming pass-through
after terminal states and reload. The positive/interruption/late-queue nudge
matrix requires the actual qualified context operations. Stock-version results
are not proof of preserving-stop semantics or native support.

For a prepared candidate Pi source checkout with its dependencies and provider
catalogs available, use the actual source SDK:

```bash
export AOE_PI_SOURCE_ROOT=/path/to/pi-source
export AOE_ASK_USER_SOURCE_ROOT=/path/to/cooperative-question-source
cargo test --lib afk
node --import "$AOE_PI_SOURCE_ROOT/packages/coding-agent/src/experimental/source-resolver.ts" \
  --test assets/session/aoe-afk.test.mjs assets/session/aoe-afk-sdk.test.mjs \
  assets/session/aoe-afk-questions.test.mjs \
  assets/session/aoe-afk-work-state.test.mjs assets/session/aoe-afk-work-native.test.mjs \
  assets/session/aoe-afk-work-period.test.mjs assets/session/aoe-afk-work-sdk.test.mjs
```

The question fixture uses the actual package, native components, extension runner,
persisted native sessions and an offline scripted provider. Its host cases use the
real Rust ledger. Reopen/recovery assertions retain original queue objects on the
qualified candidate, while unsupported-restoration loss is reported separately.

The host test scopes the source loader to its SDK subprocess. Do not set a global
`NODE_OPTIONS` loader for the full suite; unrelated Node fixtures use native
module semantics. Candidate qualification fails if the operation is absent.

Fake-provider results do not establish native-model semantic correctness, paid
transport attempt counts, or qualification of untested auxiliary extensions.
An attended disposable-session review remains a release gate.
