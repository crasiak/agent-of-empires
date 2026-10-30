# S-A: AFK menu answering

**Date:** 2026-10-30
**Status:** Design approved in chat. Written spec awaiting user review.
**Overview:** [afk-menus-and-goal-runs-overview](2026-10-30-afk-menus-and-goal-runs-overview.md)
**Repos:** agent-of-empires (engine, recognisers, extension), Pi fork (`ui_prompt_respond` hook).

## Goal

During an AFK work period, when something waits for a human choice, AFK recognises it as a menu, decides an answer under the run's policy, delivers it, and logs it. Anything it can't recognise or isn't allowed to answer stops the work period, as today.

## Today (native work period, uncommitted CRA-859 tree)

| Source | Current behaviour | Reference |
|---|---|---|
| Agent ends turn with a text menu | No checkpoint, so settle stops the period as `blocked: missing_checkpoint`. Menu content is lost. | `aoe-afk-work-state.mjs:149` |
| Agent calls `ask_user` | Tool call refused with `no_human_answer`; period stops. | `aoe-afk-work-period.mjs:277` |
| CLI prompt inside bash | Pi runs bash with stdin `ignore`, detached, no terminal. The prompt reads EOF; the CLI errors or silently takes its default. AFK doesn't notice. No default timeout. | Pi `core/tools/bash.ts:112-116` |
| Pi extension dialog (select, confirm, input, editor, custom), including permission gates | `ui_prompt_start` stops the period as `blocked: human_prompt`. Only kind and title are visible. | `aoe-afk-work-period.mjs:327-333`; Pi `core/extensions/runner.ts:598-674` |

## Design

### 1. The `Menu` model

Every recogniser produces this shape (schema `aoe.afk-menu.v1`):

```json
{
  "schema": "aoe.afk-menu.v1",
  "source": "agent_text | ask_user | bash_prompt | pi_dialog | pane",
  "format": "<recogniser id, e.g. pi.confirm, claude.permission.bash>",
  "kind": "plan | choice | permission | confirm | input",
  "question": "Do you want to proceed?",
  "subject": "Bash command: rm -rf ...",
  "options": [{ "key": "1", "label": "Yes", "role": "approve" }, { "key": "2", "label": "No", "role": "deny" }],
  "free_text": false,
  "warnings": ["Dangerous rm operation on possibly-empty variable path"],
  "deadline_ms": 110000,
  "raw": "<bounded raw excerpt, for the log and corpus>"
}
```

- `role` per option is one of `approve | approve_always | deny | revise | other`. Recognisers assign it from the label. Unknown labels get `other`.
- `kind: plan` is a menu about the agent's approach ("Approve plan / Revise / Keep planning"). `permission` is a tool or action approval gate.
- `approve_always` options ("Yes, allow all edits during this session") are never chosen by `rules`. Under `yolo` they're allowed but flagged in the log.

### 2. Recognisers (pluggable)

A recogniser turns a source event into a `Menu` or `null`. Two kinds:

- **Structured recognisers (code):** for sources that already carry structure: `ask_user` tool input, Pi dialog calls (`select(title, options)`, `confirm(title, message)`, `input(title)`), and the new `aoe_afk_answer` tool.
- **Format recognisers (data):** for text sources (agent reply text, bash output, and in S-C, pane screens). Each format is a TOML file in `assets/afk/menu-formats/`:

  ```toml
  id = "text.bracket-shortkey"
  sources = ["agent_text", "bash_prompt", "pane"]
  kind = "choice"
  # A line containing two or more [X]word options, e.g. "[A]pply / [S]kip / [R]evise"
  option_pattern = '\[(?P<key>[A-Za-z0-9])\](?P<label>[\w-]+)'
  min_options = 2
  question_lines_above = 3
  [roles]
  approve = ["apply", "approve", "yes", "proceed", "continue"]
  deny = ["no", "skip", "cancel", "abort"]
  revise = ["revise", "edit", "amend", "change"]
  ```

  Initial formats: bracket short-key (`[A]pply`), numbered list (`1. Yes`), `(y/N)` style yes/no, and `? … ›` select prompts.

Rules for recognisers:

- They run in order; the first match wins. Structured recognisers run before format recognisers.
- Format files are loaded at extension start and validated. An invalid file is skipped with a warning; it never disables the others.
- Users can add formats in `~/.config/aoe/afk/menu-formats/`. These override built-ins with the same `id`.
- **Unrecognised waits always stop.** When something is clearly waiting (dialog open, turn ended without checkpoint, bash output ends in a prompt-like line) but no recogniser matches, the period stops with `blocked: unrecognised_menu` and the raw excerpt is saved to `afk-run-<id>/unrecognised/<n>.txt`. That sample becomes the fixture for a new format.

### 3. Answer engine (Rust, one implementation)

`src/session/afk/answer/` with a pure function:

```rust
fn decide(menu: &Menu, run: &RunContext, rules: &Rules) -> Decision
// Decision: Answer { option_key | text, rule_id, reason, warnings } | Stop { reason }
```

`RunContext` carries the goal, task, policy, repo roots, request and tool usage, and the item's limits.

- **Reached from Pi** through the existing bridge process (`aoe __afk-bridge`, spawned by `aoe-afk.mjs:11-12`). New request type `decide` with a `Menu` in and a `Decision` out, with a 5 s timeout. Timeout or bridge error means `Stop`.
- **Reached from the host** (S-C) by calling the module directly.
- The agent's proposal (from `aoe_afk_answer`, below) is an input, not the decision. The engine can accept it, override it with a rule's answer, or stop.

#### Policies

| Policy | Behaviour |
|---|---|
| `rules` (default) | Answer only if a rule allows it. Scope checks can turn an allowed answer into `Stop`. `approve_always` options are never chosen. |
| `yolo` | Answer every recognised menu. Use the agent's proposal if there is one, otherwise the first `approve` option, otherwise the first option. Scope checks become warnings in the log. Unrecognised menus still stop: there's nothing to answer. |
| `review` | Later slice. Same interface. |

#### Rules

`afk-rules.toml` at the repo root (optional), overridden by `~/.config/aoe/afk/rules.toml`, overridden by rules typed at `/afk-work on`. Each rule matches on `kind`, `source`, `format`, text patterns in `question`/`subject`, and option roles, and produces `answer = "approve" | "deny" | "<role>" | "agent"` (use the agent's proposal) or `stop`.

```toml
[[rule]]
id = "approve-plans-in-goal"
kind = "plan"
answer = "agent"          # accept the agent's own pick if scope checks pass

[[rule]]
id = "approve-repo-tooling"
kind = "permission"
subject_matches = ['^Bash command: (cargo|npm|pnpm|git (status|diff|add|commit|log))\b']
answer = "approve"

[[rule]]
id = "never-force-push"
subject_matches = ['git push .*--force']
answer = "stop"
```

Rules are evaluated in order; the first match wins. `stop` rules match before allow rules regardless of order. Default with no match is `Stop { reason: "no_rule" }`.

#### Scope checks (deterministic)

Run after a rule allows an answer, under `rules`. Under `yolo` they only add warnings.

- The menu has `warnings` from the harness (e.g. "Dangerous rm operation…") → stop.
- The subject writes or deletes outside the run's repo roots → stop.
- The subject adds dependencies (lockfile or manifest change, `npm install <pkg>`, `cargo add`) unless a rule names it → stop.
- The subject pushes to a protected branch, force-pushes, or deletes branches → stop.
- The item has used more than 80 % of its request share and the menu is a `plan` that would start new work → stop.
- The diff so far exceeds the run's `max_diff_lines` (default 1500) → stop.

Each check has an id so the log and the return review can say which one fired.

### 4. Delivery per source

#### Agent text menus

- The work-period reminder gains: "Never end a turn with a menu for the human. Call `aoe_afk_answer` with the question, options and your proposed option instead."
- New tool `aoe_afk_answer` (Pi extension). Input: `{ question, options: [{key,label}], proposal: key | null, reason }`. The extension builds a `Menu` (`source: agent_text`, format `tool.aoe_afk_answer`), calls `decide`, and returns `{ answered_by: "afk/<policy>", choice, rule, reason }` or `{ stopped: reason }`. A stop ends the period as `blocked: menu_stopped`.
- **Fallback:** if a turn would settle without a checkpoint, the extension runs the format recognisers over the final assistant text. On a match, it injects one corrective continuation ("You ended with a menu. Resolve it with `aoe_afk_answer`."). This happens once per item; the second time stops as today. No match stops as `missing_checkpoint`, as today.

#### `ask_user`

- Instead of refusing, the `tool_call` handler builds a `Menu` from the `ask_user` input and calls `decide`.
- On `Answer`, the tool call is blocked with a result that states the answer and `answered_by: afk/<policy>`. It never uses the human-answer result shape, so transcripts and the review can tell the two apart.
- On `Stop`, behaviour is unchanged (`no_human_answer`, period stops).

#### CLI prompts inside bash

- The reminder gains: "Commands must not wait for input. Pass answers explicitly (flags, `yes |`, `printf … |`). If a command needs a choice, decide it with `aoe_afk_answer` first."
- New `tool_result` check for bash: if the output's last non-empty lines match a format recogniser (or a small set of prompt heuristics: trailing `?`, `(y/N)`, `[Y/n]`, `Password:`), the result is annotated `aoe_afk_prompt_seen` with the parsed `Menu`.
- An annotated result doesn't count as progress evidence for a checkpoint until an `aoe_afk_answer` call covering that menu has been logged. This closes the "default taken silently" hole.
- Bash calls in an AFK period get a default timeout (10 minutes, configurable) when the agent passes none, so a command waiting forever can't run until the period ends.

#### Pi dialogs, including permission gates (Pi-fork change)

- New extension hook in `runner.withUIPrompt`: before calling the native UI, emit `ui_prompt_respond` with `{ kind, title, message, options, owner }` and await at most one registered responder (bounded, 5 s).
- The responder can return `{ value }` (the option string for `select`, a boolean for `confirm`, text for `input`) or `undefined`.
- `{ value }` resolves the dialog without showing it. Pi still emits `ui_prompt_start` and `ui_prompt_end`, with a new `answered_by` field so other extensions and transcripts see that it was auto-answered.
- `undefined`, a timeout or a throw falls through to the normal dialog. The AFK extension's existing `ui_prompt_start` handler then stops the period, as today.
- `editor` and `custom` kinds are never auto-answered in this slice: the responder isn't called for them.
- AFK's own consent dialog is excluded by owner (it must always be a human).

### 5. Answer log

`afk-run-<id>/answers.jsonl`, one line per decision (schema `aoe.afk-answer.v1`):

```json
{ "ts": "…", "run": "…", "item": "…", "period": "…", "menu": { "…Menu…" },
  "proposal": "1", "decision": "answer", "choice": "1", "policy": "rules",
  "rule": "approve-repo-tooling", "checks": [{ "id": "outside_repo", "result": "pass" }],
  "warnings": [], "reason": "…", "delivered": "ok | failed | unknown" }
```

- Written before delivery, then updated with `delivered`. Delivery `failed` or `unknown` stops the period (`delivery_unknown`, as today).
- Before S-B lands, the run id is the work-period id and the log lives in the existing work-period state directory.

### 6. Consent dialog and status

- `/afk-work on` accepts `--policy rules|yolo` (default `rules`) and `--rule '<inline toml>'` (repeatable).
- The consent dialog shows the policy, the number of rules loaded and their sources, and for `yolo` a highlighted line: "YOLO: AFK will answer every menu, including permission prompts. Scope checks only warn."
- `/afk-work status` adds `answers: N (rules M, yolo K), stops: S`.

## Error handling

| Failure | Result |
|---|---|
| Bridge `decide` timeout or error | `Stop { engine_unavailable }` |
| Rules file invalid | Run refuses to start under `rules` (shows the parse error). `yolo` ignores rules files. |
| Format file invalid | That format is skipped with a warning; others load. |
| Recogniser throws | Treated as no match. If something is waiting, it stops as `unrecognised_menu`. |
| Dialog still open after a delivered answer | `delivery_unknown` stop. |
| Answer log not writable | No answer is delivered; the period stops. |

## Testing

- **Rust unit tests** (`src/session/afk/answer/tests.rs`): every policy × kind; rule ordering and `stop` precedence; each scope check; `approve_always` handling; `yolo` fallbacks.
- **Format corpus:** `assets/afk/menu-formats/fixtures/<format-id>/*.txt` with expected `Menu` JSON. One test loads every format and checks every fixture. Unrecognised samples become new fixtures.
- **Node tests** (`aoe-afk-answer.test.mjs`): `aoe_afk_answer` tool, text-menu fallback (once only), `ask_user` routing, bash prompt annotation and the evidence rule, bash default timeout.
- **Pi fork tests:** `ui_prompt_respond` for select, confirm and input; fall-through on `undefined`, timeout and throw; no responder call for editor and custom; `answered_by` on the events.
- **Offline scripted-provider demo:** one item that hits a text menu, an `ask_user`, a bash prompt and a permission dialog, under `rules` (one answered, one stopped) and under `yolo` (all answered).
- **Live canary** on the preview route after it's requalified against the new Ledger CLI.

## Out of scope

Goal runs and the work queue (S-B), Claude Code / Codex panes (S-C), the `review` policy, answering `editor`/`custom` dialogs, Ledger import of the answer log.
