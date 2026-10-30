# S-C: AFK pane responder for Claude Code and Codex

**Date:** 2026-10-30
**Status:** Design approved in chat (option A: separate host-side slice after Pi). Written spec awaiting user review.
**Overview:** [afk-menus-and-goal-runs-overview](2026-10-30-afk-menus-and-goal-runs-overview.md)
**Depends on:** [S-A menu answering](2026-10-30-afk-menu-answering-design.md) (`Menu` model, format recognisers, Rust answer engine, answer log).
**Repo:** agent-of-empires only.

## Goal

For Claude Code and Codex sessions in AoE, answer menus shown in the tmux pane (permission prompts, plan approvals, `AskUserQuestion` choices, lettered menus) under the same policies and log as S-A. This slice covers menus only: no work queue, goal loop or checkpoints for these harnesses.

## Example

A Claude Code permission prompt captured 2026-10-30:

```
 Bash command
 Clear build dirs of the six idle AoE worktrees
 │ df -g /System/Volumes/Data | tail -1; b=…; for d in …; do test -d "$b/$d" && rm -rf "$b/$d" && echo "cleared $d"; done; …
 │ Dangerous rm operation on possibly-empty variable path: "$b/$d" in `rm -rf "$b/$d"` (rewrite it as "${b:?}/${d:?}" or use a literal path)
 ⚠ Claude Code will automatically deny this request in 1:50, to avoid blocking progress on an unattended session

 Do you want to proceed?
 ❯ 1. Yes
```

Parsed as: `source: pane`, `format: claude.permission.bash`, `kind: permission`, `subject` = the command, `warnings` = the "Dangerous rm…" line, `deadline_ms: 110000`, options `1. Yes` (approve) plus any further options shown. Under `rules` the warning makes it **stop**. Under `yolo` AFK answers `1`. This sample is the first fixture.

## What exists

- **Detection:** `src/tmux/status_detection.rs` classifies these screens as `Status::Waiting` for Claude, with fixtures for bash permission, edit approval, plan-mode proceed and multi-choice prompts (lines 574–620). CRA-1287 added turn-end Ask and lettered-menu detection. Nothing extracts the question, options or warnings.
- **Configured status rules:** `src/tmux/status_rules.rs` already lets users add per-profile, per-tool detection rules. The menu formats follow the same idea.
- **Pane I/O:** `TmuxSession::capture_pane_with_cursor` and `send_keys` in `src/tmux/session.rs`.

## Design

### Enabling

- `aoe afk answer on <session> [--policy rules|yolo] [--rule …]` and a TUI palette entry. This is host-side consent, shown as a confirmation dialog in the AoE TUI (Pi isn't involved). The dialog lists the policy, rules and, for `yolo`, the same highlighted warning as S-A.
- Scope: one session, until `aoe afk answer off <session>`, the session stops, or human input arrives in the pane (detected as any keystroke through AoE or a pane content change not explained by the agent; see "Human takeover").
- Enabled sessions get a badge in the session list (`AFK·A` for `rules`, `AFK·Y` for `yolo`).

### Responder loop

Runs in the AoE status poller, only for enabled sessions, only when detection says `Waiting`:

1. Capture the pane (`capture_pane_with_cursor`, last 60 lines).
2. Run the pane format recognisers (below). No match: log an `unrecognised_menu` stop with the raw sample, notify, and do nothing else. The pane stays as it is for the human.
3. Call the S-A engine `decide(menu, run, rules)`. The pane responder has no agent proposal, so `rules` uses a rule's answer and `yolo` uses the first `approve` option.
4. On `Stop`: log it and notify. Leave the prompt.
5. On `Answer`:
   1. Write the answer-log entry (`delivered: pending`).
   2. Re-capture the pane and recompute the menu fingerprint (format, question, subject, options). If it changed, abort: `delivered: aborted_changed`.
   3. Check the deadline: if less than 2 s remains, abort: `delivered: aborted_deadline`.
   4. Send the keys for the chosen option, as defined by the format (e.g. the digit, or arrow keys then Enter).
   5. Re-capture after 500 ms. The prompt must be gone or replaced by a different screen. Otherwise `delivered: unknown`, disable the responder for this session, and notify.
6. One answer per detected prompt instance: a prompt fingerprint that was already answered is never answered again.

### Pane format recognisers

S-A's TOML formats gain pane-specific fields. Built-in files live in `assets/afk/menu-formats/pane/`; user files in `~/.config/aoe/afk/menu-formats/`.

```toml
id = "claude.permission.bash"
sources = ["pane"]
tools = ["claude"]
kind = "permission"
anchor = '^\s*Do you want to proceed\?\s*$'
subject_block = { start = '^\s*Bash command\s*$', end_before = '^\s*Do you want to proceed' }
option_pattern = '^\s*(?:❯\s*)?(?P<key>\d+)\.\s+(?P<label>.+?)\s*$'
warning_pattern = '^\s*│?\s*(Dangerous .+|Warning: .+)$'
deadline_pattern = 'automatically deny this request in (?P<m>\d+):(?P<s>\d{2})'
select = "digit"              # send the option key; alternatives: "arrows_enter"
[roles]
approve = ['^Yes$']
approve_always = ['^Yes, (and )?(allow|don.t ask)']
deny = ['^No\b']
```

Initial pane formats, each with fixtures from the existing detection corpus:

- `claude.permission.bash`, `claude.permission.edit`, `claude.plan.proceed`, `claude.ask.choice` (AskUserQuestion), `claude.turn_end.lettered` (CRA-1287 lettered menus);
- `codex.approval.exec`, `codex.approval.patch` (fixtures captured during implementation);
- the generic text formats from S-A (bracket short-key, numbered, y/N) restricted to the last screen block.

Rules for pane formats:

- Formats are tried most specific first (tool-specific, then generic).
- An `approve_always` option is never chosen by `rules`; under `yolo` it's never chosen if a plain `approve` exists.
- Free-text prompts are never answered in this slice.
- A format that matches but yields no options or no anchor counts as no match.

### Human takeover

The responder disables itself for the session (logging `human_takeover`) when:

- the user types into the session through AoE (attach, live send, web), or
- the pane changes while `Waiting` in a way that isn't the agent progressing (the prompt disappears without AFK having answered).

Re-enabling requires the command or palette entry again.

### Answer log and review

- Same `aoe.afk-answer.v1` records as S-A, in `afk-pane-<session>/answers.jsonl` under the AFK state directory, with `source: pane` and the raw excerpt.
- `aoe afk review <session>` and the TUI review show the pane answers grouped as in S-B: permission approvals and scope warnings first, then other answers, then stops with question and options.
- Notifications use the existing AoE notification path: one per stop, and a summary when the responder is disabled.

## Error handling

| Failure | Result |
|---|---|
| Capture fails | Skip this poll; three consecutive failures disable the responder and notify. |
| Engine error | `Stop { engine_unavailable }`; prompt left for the human. |
| Fingerprint changed between decide and send | Abort, log `aborted_changed`, re-evaluate on the next poll as a new prompt. |
| Deadline too close | Abort, log `aborted_deadline`; the harness's own timeout applies (e.g. Claude auto-denies). |
| Prompt still showing after send | `delivered: unknown`, responder disabled, notify. |
| Invalid format file | Skipped with a warning; built-ins still load. |

## Testing

- **Parser fixtures:** each pane format has `fixtures/<id>/*.txt` with expected `Menu` JSON. The captured `rm -rf` sample is the first `claude.permission.bash` fixture. Existing `status_detection.rs` fixture screens are reused.
- **Decision fixtures:** the `rm -rf` sample must `Stop` under `rules` (scope check `harness_warning`) and answer `1` under `yolo`.
- **Responder loop** with a fake `TmuxSession`: fingerprint change, deadline abort, unknown delivery, one-answer-per-instance, human takeover.
- **tmux integration test** (macOS socket-path caveat applies, see the local deploy notes): a scripted pane prints a prompt and reads a key; the responder answers it under `yolo` and the script records the key.
- **Live check** on one real Claude Code session in a throwaway repo, `rules` policy, with a rule that approves `ls`.

## Out of scope

Goal runs for Claude Code or Codex, using Claude Code hooks (`PermissionRequest`/`PreToolUse`) instead of the screen (a possible later recogniser source), free-text answers, other harnesses.
