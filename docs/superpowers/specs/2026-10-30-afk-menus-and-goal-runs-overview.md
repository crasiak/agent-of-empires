# AFK menus and goal runs: overview

**Date:** 2026-10-30
**Status:** Design approved in chat section by section (2026-10-29 to 2026-10-30). Written specs awaiting user review.
**Tracking:** CRA-859 (AFK native work period hub). New tickets per slice once the specs are approved.
**Repos:** agent-of-empires (crasiak fork), the Pi fork (`packages/coding-agent`), attic `afk-record/` (S-D), attic `ledger/` (later slice only).

## Why

AFK today runs one bounded task and stops at the first menu. In practice the user answers about 9 in 10 menus with "approve". The 10th is a "no" because the approach is wrong or the scope or cost grew, and occasionally they pick a specific option. Big disagreements turn into an argument and need the human anyway.

Two features follow:

1. **Menu answering.** AFK recognises menus (agent text menus, CLI prompts, Pi dialogs including permission dialogs, and later Claude Code / Codex pane prompts) and answers them under a policy the user picks, logging every answer.
2. **Goal runs.** AFK works toward a goal, not a single task. When an item completes, it picks up next-steps work from the goal, the agent's follow-ups and trackers, and keeps going until the queue is empty or a limit is reached.

## Slices

| Slice | Spec | Scope | Depends on |
|---|---|---|---|
| S-A | [afk-menu-answering](2026-10-30-afk-menu-answering-design.md) | Menu model, pluggable recognisers, answer engine, `rules` and `yolo` policies, answer log, Pi menus (text, `ask_user`, bash prompts, dialogs via a new Pi hook) | Native work period landed on main (see Prerequisites) |
| S-D | [afk-ticket-record](2026-10-30-afk-ticket-record-design.md) | `afk-record` CLI: every run has a ticket, append-only on the user's tickets, event comments with read-back, outbox, `afk-human-ack` | none |
| S-B | [afk-goal-runs](2026-10-30-afk-goal-runs-design.md) | Goal, work queue, fresh conversation per item, full-flow PRs and tickets, run limits, run records, return review | S-A, S-D |
| S-C | [afk-pane-responder](2026-10-30-afk-pane-responder-design.md) | Host-side responder for Claude Code and Codex pane menus, reusing AoE status detection | S-A (engine, log, menu model) |
| Later | none yet | `review` policy (second-model check on plan menus), Ledger import of run records, Foreman or worker per item, Claude Code as a full AFK harness | S-A, S-B |

## Shared concepts

- **Menu:** one normalised shape for anything that waits for a human choice. Defined in S-A and used by S-B (item admission) and S-C (pane prompts).
- **Answer engine:** `decide(menu, run) -> answer | stop`. One engine for every source.
- **Policy:** chosen per run.
  - `rules` (default): standing answers plus deterministic scope checks.
  - `yolo` (opt-in, loudly marked): answer everything, scope checks only warn.
  - `review` (later): adds a second-model check on plan menus.
- **Answer log:** append-only record of every decision, with the policy and rule that produced it. Answers are labelled `answered_by: afk/<policy>` and are never presented as human answers.
- **Ticket record:** tickets are the durable record of AFK's actions. Every run has a ticket; all ticket I/O goes through `afk-record` (S-D).
- **Goal:** required for a goal run. Either typed (`/afk-work on "goal"`) or taken from a ticket (`afk do CRA-123`).

## Flexibility principle

Menu formats will change and new ones will appear. Recognising a menu is kept separate from deciding an answer:

- Recognisers are pluggable and data-driven where possible (format definitions in TOML), with code recognisers only for structured sources (Pi hooks, tool calls).
- Every recogniser emits the same `Menu` shape. The engine never sees raw text formats.
- A waiting state that no recogniser understands always **stops** and saves the raw sample to a corpus, so adding a format means adding a definition and a fixture, not changing the engine.

## Decisions superseded by this design

These were settled AFK decisions. The user changed them in this design session; the KB and Atlas need matching entries when the specs are approved.

| Previous decision | Source | Now |
|---|---|---|
| "Never fabricate": no synthetic answers; a deferral is always `no_human_answer` | KB AFK principles; recon `01-what-we-are-building.md` | AFK may answer under a run policy. Answers are labelled `afk/<policy>`, never presented as human answers. |
| Non-goal: permission auto-approval | same, non-goals list | AFK may answer permission dialogs: under `rules` only when a standing rule matches, under `yolo` always. |
| Non-goal: a new task generator | same | Goal runs admit follow-ups and tracker work, checked against the goal. |
| No keystrokes pasted into panes | same, principles | S-C sends keystrokes to Claude Code / Codex panes after re-verifying the prompt. |
| Native mode stops on `ask_user` | native work period, `aoe-afk-work-period.mjs:277` | `ask_user` is routed to the answer engine. |
| Every work period needs its own native confirmation | native work period, preview README | Under a goal run, one run consent grants each item's work period. A new run still needs a new confirmation. |
| Delivery is the only ticket write; a typed goal has no ticket | S-B as first written | Every run has a ticket (found or created), and AFK records progress, assumptions, scope changes and new tasks there as they happen (S-D). |
| Work period blocks compaction and conversation switches | `aoe-afk-work-period.mjs:364` | Unchanged inside an item. Goal runs start a fresh conversation per item instead of compacting. |

Unchanged: human input stops everything, evidence-cited checkpoints, the Ledger launch guard, Pi as the only full AFK harness, merging stays with the user (except under `yolo`, which merges unless `--no-merge`, see S-B).

## Prerequisites

- **Native work period on main.** S-A and S-B extend `aoe-afk-work-period.mjs`, `aoe-afk-work-state.mjs` and `src/session/afk/work_period.rs`, which exist only in the uncommitted `CRA-859-afk-headroom-rollout-20261027` worktree (recon 2026-10-29, `05-remaining-work.md` A1–A4). Landing that is the first step.
- **Preview requalification.** The private AFK preview pins the old Ledger CLI bytes. After the CRA-1364 Ledger deployment (2026-10-30) it needs requalifying before any live canary.
- **Ledger.** Run-record import into Ledger is a later slice. S-B writes records in a versioned local format first.
