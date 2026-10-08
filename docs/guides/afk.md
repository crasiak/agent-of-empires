# AFK control and one-cycle delegation for Pi

AFK is off by default. **Control-only presence does not authorize autonomous
work.** A separate operator grant can authorize one small file decision on the
qualified stock Pi 0.87.1 SDK. Neither mode starts an idle agent turn. Executable
delegation requires ordinary work already in flight; control-only presence
remains available while idle.

## Control-only presence

A session must have been launched by this AoE build as a direct host `pi`
session, with its own native conversation and compatible extension. Older
processes, custom wrappers, sandboxed sessions, ACP and other agents are not
qualified. Do not restart an active session merely to enable AFK.

```sh
aoe session afk on SESSION --minutes 30
aoe session afk status SESSION
aoe session afk off SESSION
```

Choose an expiry from 1 to 1440 minutes. `on` grants zero autonomous allowance
and exits nonzero without a fresh acknowledgement. It also ends any previous
delegation. `off` durably revokes future work, including delegation, even when
runtime acknowledgement is unavailable. Corrupt or unsafe storage requires
explicit repair rather than a guessed revision.

In the TUI command palette, **AFK control-only: inspect / enable / disable**
opens the shared status/control dialog. Enter requests control-only presence;
`o` requests off, `r` refreshes, and Escape closes it. Delegation requires the
separate CLI command below. An Off request during a pending operation is queued
and survives closing the dialog. Observations become stale after five seconds.
Closing AoE does not renew or remove the window.

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
assumptions, uncertainties, recommendation, risk and rollback. These are
**model claims**, separate from host identity, admission, counters and outcomes.
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
expiry, failed/denied reservations and settlement end it. A stopped or exhausted
owned episode stays guarded through settlement, so an ordinary automatic
follow-up cannot escape its limit. Normal tool selections are not rewritten.
Resuming delegation requires a new operator command and fresh acknowledgement;
unused counters, reloads, polling and model requests do not resume it.

### Optional settlement nudges

**Unmodified stock Pi 0.87.1 refuses nonzero nudge grants.** The runtime requires
an actual optional `abortPreservingQueue()` context operation before acknowledging
such a grant. Native Pi's ordinary abort restores queued human text to the editor
but can discard queued extension custom messages. This build's pinned runtime does not provide the required additive core
operation. A qualified source candidate is not an installed runtime upgrade. Omit `settlement_nudges` to retain stock-supported zero-nudge
one-file delegation; rejection does not abort unrelated ordinary work.

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

The tested bound is **stock-SDK main-loop reservations on Pi 0.87.1**, not
physical HTTP attempts or a billing ceiling. Reservations are consumed before
dispatch and never refunded on failures. Denied/failed reservations explicitly
abort through the SDK; tool blocking alone is not a dispatch gate.

Owned automatic compaction is cancelled; navigation pauses instead of creating
a tree summary. Cache warming is suppressed only while delegation is pending
or owns work, including owned settlement. Ended, off, expired and reloaded-ended
windows no longer override native warming decisions once unowned. No global
warming setting is changed. Provider retries,
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

The private protocol-4 ledger lives under
`<app_dir>/afk-runtime-v4/<instance-id>/ledger.json`, with a stable writer lock.
Presence files remain under `<app_dir>/afk/<instance-id>/` with their original
16 KiB bounds. Migration 39 initializes a separate runtime namespace, preserves
protocol-3 evidence and requires explicit reenrollment. Historical migrations
36 through 38 retain their meaning. The launch/stdio bridge uses protocol 4.
Fresh challenges bind
capability acknowledgements to the instance, profile, native conversation,
launch, generation, window and exact grant.

Record/admission commits precede acknowledgement. The durable action-attempt
commit is the revocation boundary: off/expiry before it denies the effect;
an already-committed attempt may finish or remain unknown. A crash or lost
response does not refund or replay the permit. Stop the session before manual
inspection/repair; do not delete its active writer lock or edit live ledger
files. Preserve ambiguous payloads/preimages until independently reconciled.

## Reproduce the offline qualification

No npm install or provider credentials are required. Point `AOE_PI_ROOT` to an
existing Pi **0.87.1** package root containing `dist/index.js` and its normal
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
after terminal states and reload. Stock SDK tests now assert nonzero-nudge
refusal. The prepared positive/interruption/late-queue nudge matrix runs only
when the actual loaded SDK exposes the required context operation; stock results
are not proof of that candidate matrix or native support.

For a prepared candidate Pi source checkout with its dependencies and provider
catalogs available, use the actual source SDK:

```bash
export AOE_PI_SOURCE_ROOT=/path/to/pi-source
export AOE_ASK_USER_SOURCE_ROOT=/path/to/cooperative-question-source
cargo test --lib afk
TSX_TSCONFIG_PATH="$AOE_PI_SOURCE_ROOT/tsconfig.json" \
  node --import "$AOE_PI_SOURCE_ROOT/node_modules/tsx/dist/loader.mjs" \
  --test assets/session/aoe-afk.test.mjs assets/session/aoe-afk-sdk.test.mjs \
  assets/session/aoe-afk-questions.test.mjs
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
