# S-B: AFK goal runs and the work queue

**Date:** 2026-10-30
**Status:** Design approved in chat. Written spec awaiting user review.
**Overview:** [afk-menus-and-goal-runs-overview](2026-10-30-afk-menus-and-goal-runs-overview.md)
**Depends on:** [S-A menu answering](2026-10-30-afk-menu-answering-design.md) (engine, policies, answer log).
**Repos:** agent-of-empires (extension, run store, TUI review), Pi fork only if the session API gap below needs it.

## Goal

The user gives AFK a goal once, confirms one run envelope, and walks away. AFK works through a queue of items toward that goal, one fresh Pi conversation per item. Each finished item ends as a ready-for-review PR with its ticket updated. Follow-ups found along the way are queued if they serve the goal. The user comes back to PRs, an answer log and a short list of things that need them.

## Starting a run

- **Ad-hoc:** `/afk-work on "<goal>" [--policy rules|yolo] [--hours H] [--requests N] [--items K] [--tracker <linear project|initiative>] [--no-merge]`
- **Ticket:** `/afk-work do CRA-123 [same flags]`. AoE CLI and TUI equivalents: `aoe afk do <session> CRA-123` and a palette entry. Host commands only prepare the request; the grant still comes from the native consent dialog in Pi.
- The goal is required. Without one, `/afk-work on N M` keeps today's single-task behaviour.
- For a ticket, AFK reads the issue title and description as the goal text, and its open child issues (sorted by Linear sort order) as the seed items. The parent ticket is appended last as a "close out the goal" item. The Linear read goes through the AoE bridge using `LINEAR_API_KEY`; if it's missing, the run refuses to start and says why.

### Consent

One native consent dialog covers the whole run. It shows the goal, the seed items, the policy (with the `yolo` marker from S-A), outward actions ("opens ready PRs and moves tickets"; plus "merges PRs" under `yolo` unless `--no-merge`), and the run limits.

This changes a current decision: today every work period needs its own confirmation. Under a goal run, the run consent grants each item's work period. Human input, Off or reaching a limit still ends the run, and a new run needs a new confirmation.

## Run loop

1. Take the next `queued` item.
2. Start a fresh Pi conversation for it (see "Fresh conversation per item") seeded with: the goal, the run rules, the previous item's handoff, the item's title and detail, and its branch.
3. Open a work period for the item under the run consent, with the item's request share: the larger of `run requests / items` (minimum 4) and the item's `estimate.requests`, capped by what's left of the run's requests. The run-level hours and requests are never raised by an estimate. The existing native work-period machinery runs it unchanged: evidence-cited checkpoints, continuations, limits.
4. Menus are answered by S-A under the run's policy.
5. When the item checkpoints `completed`, run **delivery** (below), record the result and handoff, and process follow-ups.
6. Pull tracker next-work if the run has a ticket or `--tracker`.
7. Stop if a stop condition holds, otherwise go to 1.

### Stop conditions

- Queue empty.
- A run limit reached: wall-clock hours, total requests, or item count.
- Two items in a row end `blocked`.
- Human input in the session, `/afk-work off`, or AoE Off.
- The run store can't be written.

A single blocked item doesn't stop the run: it's recorded and the next item starts.

## Work queue

`afk-run-<id>/queue.jsonl`, append-only events folded into current state (schema `aoe.afk-queue.v1`). An item:

```json
{ "id": "i3", "source": "goal | ticket | agent | tracker", "title": "…", "detail": "…",
  "parent": "i1", "ticket": "CRA-124", "status": "queued | running | done | done_local | blocked | skipped",
  "delivery": "fold | stack | own", "estimate": { "requests": 12, "diff_lines": 2200, "why": "…" },
  "admitted_by": { "policy": "rules", "rule": "…", "reason": "…" },
  "branch": "afk/<run>/i3-short-title", "base": "afk/<run>/i1-…",
  "result": { "commit": "…", "pr_url": "…", "summary": "…", "handoff": "items/i3/handoff.md", "conversation": "<pi session file>" } }
```

### Where items come from

- **Goal or ticket seed:** at run start.
- **Agent follow-ups:** the `completed` checkpoint gains an optional `follow_ups: [{ title, detail, why, fold, estimate }]` (max 5). `next_step` stays null for `completed`. The item reminder says to finish related work inside the current item rather than split it out: one large coherent PR beats five small ones. A follow-up should be work that couldn't reasonably go in this item.
- **Tracker next-work:** after each item, re-read the goal ticket's open children (and `--tracker` project or initiative next-work, including initiative `STATUS.yaml` when named). New ones are proposed for admission.

### Admission (goal check)

Every non-seed item goes through the S-A engine as a `Menu` with `kind: "admit_item"`, so the same rules, policies and log apply.

- **`rules`:** the item's `why` must name the part of the goal it serves. Deterministic rejections:
  - duplicate of a done, queued or skipped item (normalised title match, or same ticket);
  - would exceed `--items`;
  - touches a repo not in the run;
  - matches a `stop` rule.
- **`yolo`:** admit everything except duplicates and items over the `--items` limit.
- Rejected items are recorded as `skipped` with the reason, so the review shows what AFK chose not to do.

### Fold or stack

Each admitted follow-up gets a `delivery` mode. The default is to bring the work into the parent's PR:

- **`fold`** (default): the item works on its parent's branch and its commits go into the parent's PR, whose description is updated with the item's summary. Allowed when the follow-up is in the same repo, the parent's PR is still open and unmerged, and the combined diff stays within the PR's diff limit (S-A scope checks).
- **`stack`:** used when the agent sets `fold: false` with a reason (a separate concern a reviewer should judge on its own) or a fold check fails. The item branches from its parent's branch and its PR targets that branch.
- **`own`:** seed and tracker items. They branch from the run's base branch and open their own PR.

The chosen mode and the reason are recorded on the item, so the review shows why work was folded or split.

### Estimates

Defaults (request share, PR diff limit in S-A, bash timeout in S-A) hold unless work is estimated to need more. A follow-up carries its `estimate` in the proposal; for seed and tracker items the agent may give one in the item's first checkpoint. An estimate raises the item's request share and its PR's diff limit, never the run's hours or total requests. Estimates are recorded with their `why` and shown in the review next to what the item actually used.

### Order

Seed items first, in order. An agent follow-up goes directly after the item that proposed it (depth-first). Tracker additions go to the end.

## Fresh conversation per item

- The run loop lives in the `/afk-work` command handler, which has `ExtensionCommandContext.newSession({ withSession })` (Pi `core/extensions/types.ts:451`). Each item starts with `newSession`, then `withSession` sends the item's seed as a user-role message labelled `[AoE goal run item i3]`, and opens the item's work period.
- Run state lives only in the run store, not in the conversation, so a new conversation loses nothing.
- **Open question for the implementation plan (verify first):** whether a command handler can keep driving `newSession` after its first item settles, or whether the loop must re-enter through a registered continuation command. If neither works, add a small Pi-fork hook (`session_replace_request`) that lets an extension with an active run consent request a fresh session at settle. The plan's first task is a spike on this.
- Guarded Ledger already starts fresh conversations only, so this doesn't conflict with the launch guard. Compaction stays blocked inside an item, as today.

## Delivery (full flow)

On `completed`, the extension sends one more turn in the item's conversation: "Deliver: commit on `<branch>`, push, open a ready-for-review PR against `<base>` with the item summary, then update `<ticket>`." Permission prompts in that turn are answered by S-A.

The item is `done` only when the extension verifies through the bridge that:

- the branch exists on the remote at the reported commit;
- the PR exists and is open (`gh pr view <url> --json state,headRefOid`);
- the ticket, if any, has a comment with the PR URL and is in the team's "In Review" state, or the tracker update is recorded as failed.

Branching: `own` and `stack` items work on `afk/<run>/<item>-<slug>`; `own` items branch from the run's base branch (the session's branch at run start), `stack` items from their parent's branch with the PR targeting it. A `fold` item commits on its parent's branch, pushes, and updates the parent's PR instead of opening one; verification checks that PR's head is at the reported commit.

Outcomes:

| Result | Item status | Run continues |
|---|---|---|
| Commit, push, PR and ticket all verified | `done` | yes |
| Push or PR fails | `done_local` (branch kept, reason logged) | yes |
| Ticket update fails | `done` with `ticket_update: failed` | yes |
| Nothing committed | `blocked: nothing_to_deliver` | yes |

**Merging:** never under `rules`. Under `yolo`, AFK merges a PR once required checks pass and every item folded into it is done, before starting items stacked on it. `--no-merge` turns this off for the run.

## Records

`afk-run-<id>/` (schema `aoe.afk-run.v1`), in the AFK state directory next to the existing work-period files:

- `run.json`: goal, ticket, policy, limits, consent, start and end, end reason.
- `queue.jsonl`: queue events.
- `answers.jsonl`: S-A answer log, including item admissions.
- `items/<id>/handoff.md`: what the item did, what's left, decisions made, written by the agent in the delivery turn and checked to be non-empty.
- `unrecognised/`: raw samples from S-A.

The format is versioned so Ledger can import a run later as a record that groups the item sessions. Ledger import is a later slice and is not built here.

## Return review

Extends the existing AFK review (Outcome / Effect / Needs attention):

- **Outcome:** goal, items done / done_local / blocked / skipped, end reason, limits used.
- **Effect:** per item: PR link, ticket, files changed, branch.
- **Answers AFK gave:** grouped by policy. Permission approvals and scope-check warnings are listed first. Under `yolo`, answers that `rules` would have refused are marked.
- **Needs you:** each blocked item and each stopped menu, with the question and options. Choosing an option there writes a human answer and can start a new run from that item (new consent).
- **Skipped:** follow-ups and tracker items AFK chose not to take, with reasons.

The TUI review and `aoe afk review <session>` both read the run store.

## Error handling

| Failure | Result |
|---|---|
| Linear read fails at start | Run refuses to start, with the error. |
| Linear read fails mid-run | Tracker pull skipped for that step and logged; run continues. |
| `newSession` cancelled or fails | Item `blocked: session_unavailable`; counts toward the two-in-a-row stop. |
| Handoff missing after delivery | Item still `done`; review flags the missing handoff. |
| Run store write fails | Run stops immediately; nothing further is delivered. |
| Pi or AoE restarts mid-run | Run does not resume (reload never restores authority, unchanged). The review shows it as `interrupted`, with the item that was running. |

## Testing

- **Queue state machine** (Node, `aoe-afk-run-queue.test.mjs`): fold events, admission paths, depth-first ordering, duplicate detection, stop conditions including two blocked in a row.
- **Delivery verifier** with a fake bridge: each outcome row above.
- **Ticket seeding** with a recorded Linear response fixture.
- **Session-per-item spike** (first plan task), then a test that a run with three items creates three Pi session files and carries no conversation content across.
- **Offline scripted-provider demo:** ad-hoc goal, two seed items, one follow-up admitted and one skipped, one menu answered, delivery against a local bare git remote and a stubbed `gh`.
- **Live canary** on the preview route: a small goal against a throwaway repo with a real remote, under `rules`.

## Out of scope

Foreman or worker per item, resuming a run after restart, Ledger import, the `review` policy, Claude Code / Codex goal runs.
