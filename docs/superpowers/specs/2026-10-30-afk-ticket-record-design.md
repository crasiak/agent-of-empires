# S-D: AFK ticket record (`afk-record`)

**Date:** 2026-10-30
**Status:** Design approved in chat section by section. Written spec awaiting user review (revised 2026-10-31 for `session-arc-v1`).
**Overview:** [afk-menus-and-goal-runs-overview](2026-10-30-afk-menus-and-goal-runs-overview.md)
**Used by:** [S-B goal runs](2026-10-30-afk-goal-runs-design.md) (all ticket reads and writes), [S-C pane responder](2026-10-30-afk-pane-responder-design.md), interactive agents through the `linear-project-work` skill.
**Depends on:** CRA-1377 (`session-arc-v1` and the validator behind `timeline arc validate`), for the arc block in `progress` and `run_summary` only. Everything else in this spec can be built first.
**Repos:** attic (`afk-record/`, its own Go module like `ir/`); agent-of-empires (Pi AFK extension calls it).

## Goal

Tickets are the durable record of what AFK did. Every run has a ticket, and AFK records its actions there as they happen: progress, wrong assumptions, scope changes, new tasks, blocks, delivery and closure. The user can come back, open Linear and read what happened and why, without the local run store.

All ticket I/O for AFK goes through one harness-neutral CLI, `afk-record`. It owns the event schema, the comment layout, the outbox and read-back, and the ticket rules below, so the rules are enforced by code rather than by agent instructions.

## Decisions (from the design session)

- Every goal run has a ticket. A referenced ticket is used; otherwise AFK searches for a matching one and creates it only if none is found.
- The user's tickets are append-only for AFK. AFK-owned tickets may be edited, with a changelog.
- Writes go through a local outbox with read-back. Scope changes and new tasks block until confirmed; other writes don't. A stale outbox stops the run (runs never resume in place).
- AFK writes on every event and adds a progress comment at each checkpoint.
- Under `yolo`, AFK marks a ticket Done after a successful merge.
- `afk-human-ack` marks every ticket where AFK decided something on the user's behalf. Only the user removes it.
- The record point is a standalone CLI, not part of AoE or Ledger.
- Progress and run summaries use the shared `session-arc-v1` contract rather than an AFK-only format (2026-10-31, CRA-1376). Picking next work is left to `aoe-fleet next`; `afk-record` only reads and writes tickets.

## Interface

Every command prints JSON on stdout.

| Exit | Meaning |
|---|---|
| 0 | Done and confirmed by read-back |
| 3 | Queued in the outbox, not yet confirmed |
| other | Error, with `{ "error": … }` |

### Commands

- `afk-record resolve (--goal "<text>" | --ticket CRA-123) [--project <id|track>] --run <id>`
  Finds or creates the run ticket. Search before create: open issues in the project whose title matches the normalised goal, then issues linked to the run's base branch or open PRs. Returns `{ ticket, created, project }`. Fails if no project resolves.
- `afk-record item --run <id> --parent CRA-123 --title "…" [--detail "…"] [--ticket CRA-124]`
  Same for a queue item, as a sub-issue of the run ticket. With `--ticket`, uses that issue and checks it exists.
- `afk-record event --run <id> --ticket CRA-124 --kind <kind> --json -`
  Appends one event (see Events).
- `afk-record children CRA-123`
  Open child issues in Linear sort order: the read S-B needs for seeding and tracker pull. It moves here from the AoE bridge. Next-work recommendations are not `afk-record`'s job; S-B gets them from `aoe-fleet next`.
- `afk-record flush [--run <id>]`
  Retries pending outbox writes.
- `afk-record status --run <id>`
  Outbox summary: pending, confirmed, failed, `stale`, and tickets still labelled `afk-human-ack`.

### Project routing

`--project` wins. Otherwise the repo is mapped to a track and the track to a project id using the same routing the `linear-project-work` skill reads (`references/projects.md`). No project means `resolve` fails and the run doesn't start.

### Credentials

`LINEAR_API_KEY` from the environment. No token in config, records or comments. A missing key is an explicit error.

## Callers

- **Pi AFK extension** (S-B):
  - calls `resolve` at run start and `item` for each admitted item;
  - emits `started`, `progress` (each checkpoint), `delivered`, `done` and `run_summary` itself;
  - exposes an agent tool `aoe_afk_record` that passes `assumption`, `scope`, `new_task` and `blocked` events to `afk-record event`, adding the run and item context.
- **S-C pane responder:** can call the same CLI; not required by this slice.
- **Interactive agents:** may call the CLI directly; the `linear-project-work` skill can point to it later. Not required by this slice.
- **AoE:** passes run context and shows `afk-record status` in the return review. It has no Linear client of its own for AFK.

## Ticket rules

### Ownership

The API key belongs to the user, so Linear shows the user as creator of everything. Ownership is therefore recorded, not inferred: when AFK creates a ticket it adds the label `afk-created` and stores the ticket id in its outbox store. A ticket is AFK-owned only when both are present. Every other ticket is the user's.

### What AFK may change

| | User's tickets | AFK-owned tickets |
|---|---|---|
| Description, acceptance criteria | never | may edit; appends one line per edit to a `## Changelog` section |
| Comments | append | append |
| Status | transitions below only | same |
| Labels | add `afk-human-ack` only | add or remove its own labels |
| Assignee, priority, estimate, project, cycle | never | set at creation only |

`afk-record` refuses any other mutation with an error; there is no flag to override it.

### Status transitions

- Item starts → **In Progress**.
- Item delivered → **In Review**.
- Under `yolo`, after the merge → **Done**.

Never backwards, never Cancelled. If Linear's GitHub integration (or the user) has already moved the status further, AFK reads the current state and leaves it.

### `afk-human-ack`

Added when AFK closes a ticket, records an `assumption` or `scope` event on it, or creates it. AFK never removes it. The return review and `run_summary` list every ticket that still has it; a Linear view on the label is the user's inbox.

### Wrong assumptions, scope and new tasks

- **`assumption`:** what was assumed, what is actually true, evidence, effect on the work.
- **`scope`:** `added | changed | dropped`, why, and `changes_done: true|false` (whether the definition of done changes).
  - On a user's ticket with `changes_done: true`, AFK posts the event and holds the item `blocked: needs_human_scope` until the user decides.
  - On an AFK-owned ticket, AFK edits the description, logs the change and continues.
- **`new_task`:** added work becomes a sub-issue (searched for before created), linked to the event comment, then put through S-B admission. A rejected task is still created, left in Backlog with the rejection reason, so the record exists either way.

## Events

`kind` is one of `started`, `progress`, `assumption`, `scope`, `new_task`, `blocked`, `skipped`, `delivered`, `done`, `run_summary`.

Common fields: `what` (1-3 sentences), `evidence[]` (`path:line`, `cmd → result`, PR or commit links), `next` (`progress` and `blocked` only). Kind-specific fields are listed above, plus `pr_url` and `commit` for `delivered`, `items[]` for `run_summary`.

### Arc block

`progress` and `run_summary` also carry `arc`, a `session-arc-v1` object (CRA-1377):

- For `progress`, the arc covers the item's conversation up to the checkpoint. Checkpoint `summary`, `next_step` and `human_required` become `now`, `next` and `needs_you` as `assistant_reported` claims citing the checkpoint. Item status, limits used and the branch are `host_fact` claims citing `afk-host`.
- For `run_summary`, the arc covers the run. End reason, counts and limits are `host_fact`; ticket and PR state read back from Linear and GitHub are `course_fact`.
- `afk-record` validates the arc with the validator package behind `timeline arc validate` before queueing. An invalid arc doesn't block the event: the comment is posted without the arc block, and the event records `arc: invalid` with the validator's reason, which `status` reports.
- The arc is a summary of the event, never a substitute for it. `what`, `evidence` and the kind-specific fields stay the record.

### Comment layout

```
**AFK · <kind>** · run <id> · item <i3> · <policy>
<what>
Evidence: `path:line`, `cmd → result`, PR/commit links
Next: <one line>
<arc block, progress and run_summary only:>
Now: <text> _(assistant_reported)_
Remaining: <text or "unknown"> _(…)_
Next: <text> _(…)_
Needs you: <text> _(…)_
<!-- afk-record v1 {"run":"…","item":"…","kind":"…","event_id":"…","arc_schema":"session-arc-v1"} -->
```

The hidden `event_id` (a ULID) lets read-back find the comment and lets a retry detect that an earlier timed-out write already landed, so retries never duplicate.

### Links and folding

- Item branches are `afk/<run>/CRA-124-<slug>`, so Linear links branches and PRs automatically.
- A PR with folded items lists every ticket it closes.
- Each folded ticket gets its own `delivered` event pointing at the shared PR.
- At run end the run ticket gets `run_summary`: every item with ticket and PR, everything skipped, and every ticket still labelled `afk-human-ack`.

## Outbox

`~/.local/share/afk-record/runs/<run>/outbox.jsonl`, append-only, schema `afk-record.outbox.v1`. Each write moves `queued → sent → confirmed`, or `failed` with a reason.

- Every command appends `queued` before sending.
- Confirmation is a read-back: the comment with the `event_id`, or the expected field value.
- Retries back off from 5 s to 2 min. The Pi extension calls `afk-record flush` at every settle.
- Any write unconfirmed for more than 15 minutes makes `status` report `stale`. The extension then stops the run with end reason `ticket_record_stale`. The outbox keeps its pending writes; `afk-record flush` drains it later, and a new run (new consent) can pick up the remaining queue. Runs are never resumed in place (S-B, unchanged).
- `scope` and `new_task` wait up to 30 s for confirmation. If not confirmed, `event` exits 3 and the item is held `blocked: record_pending` and retried at the next settle.
- The outbox is also the record: the later Ledger import takes it alongside the run store and transcripts. Timeline reads it too (CRA-1378), so other tools never parse it themselves.

## Error handling

| Case | Result |
|---|---|
| `LINEAR_API_KEY` missing, or no project resolves | `resolve` fails; the run doesn't start. |
| Rate limit or 5xx | Retried with backoff; the stale rule applies. |
| Read-back mismatch | `failed: mismatch`. A `scope` or `new_task` item stays blocked; other kinds are logged and shown in the review. |
| Ticket deleted or moved mid-run | Item `blocked: ticket_gone`; the run continues. |
| Disallowed mutation requested | Error; nothing queued. |
| Evidence would include work data | Redaction before queueing: evidence is limited to repo-relative paths, command summaries and links; no raw output over 20 lines. Corelight-adjacent tracks follow the existing rule (no logs, customer data or proprietary source in tickets). |

## Testing

- **Go unit tests:** rendering for each kind, `event_id` round trip, ownership rule, the allowed-changes table (every refused mutation), outbox state machine, stale detection, redaction.
- **Arc block:** the CRA-1377 conformance fixtures pass through `afk-record`'s validation unchanged; a valid arc renders with each line's basis; an invalid arc posts without the block and shows `arc: invalid` in `status`.
- **Stub GraphQL server** replaying recorded Linear responses: search-before-create (hit and miss), read-back match and mismatch, timeout then retry without duplicate, rate limit, deleted ticket, status already ahead.
- **Node tests** in the Pi extension: `aoe_afk_record` passes events through with run context; a blocking event exit 3 holds the item; `stale` stops the run; `flush` at settle.
- **Live smoke** against a scratch Linear project: resolve, event, status change, label, read-back. Only with the user's go-ahead.

## Out of scope

Removing `afk-human-ack` automatically, editing user tickets in any way beyond the table, a second tracker, Jira, migrating interactive agents to the CLI, the Ledger import itself.
