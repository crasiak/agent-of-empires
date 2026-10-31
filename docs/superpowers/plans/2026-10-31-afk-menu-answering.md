# S-A: AFK Menu Answering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** During an AFK work period, recognise anything that waits for a human choice as a `Menu`, decide an answer under the run's policy (`rules` or `yolo`), deliver it, and log it; anything unrecognised or refused stops the period as today.

**Architecture:** Recognition (TOML format files) and the answer engine (`decide`) live in one Rust module, `src/session/afk/answer/`, reached from the Pi extension through the existing bridge process (`aoe __afk-bridge`) with three new ops: `recognise`, `decide`, `answer_delivered`, plus `rules_check` at consent. The Pi extension (`assets/session/aoe-afk-*.mjs`) turns each source (agent text, `ask_user`, bash output, Pi dialogs) into a `Menu` request, delivers the decision and reports delivery. Pi dialogs need one new fork hook, `ui_prompt_respond`, in the Pi fork runner.

**Tech Stack:** Rust (serde, `toml` 1.1, `regex` 1.10, `anyhow`), Node ESM extension tested with `node:test`, Pi fork (TypeScript, vitest).

**Spec:** `docs/superpowers/specs/2026-10-30-afk-menu-answering-design.md` (overview: `2026-10-30-afk-menus-and-goal-runs-overview.md`). Read both before starting a task.

**Approved 2026-10-31:** execution is subagent-driven. The three plan decisions stand: recognition in Rust behind the bridge (ops `recognise`, `decide`, `answer_delivered`, `rules_check`); answer log at `<transport store>/afk-answers/<period>.jsonl`, append-only; `rules_check` returns `{valid:false,error}` as a value. Both uncommitted source trees are now committed (see Task 1 and Task 11).

## Global Constraints

- Menu schema id `aoe.afk-menu.v1`; answer log schema id `aoe.afk-answer.v1`.
- Option roles: `approve | approve_always | deny | revise | other`. Menu kinds: `plan | choice | permission | confirm | input`. Sources: `agent_text | ask_user | bash_prompt | pi_dialog | pane`.
- Policies: `rules` (default) and `yolo`. `review` is out of scope.
- `rules` never chooses an `approve_always` option. `yolo` may, and logs a warning.
- Unrecognised waits always stop with `blocked: unrecognised_menu` and save the raw sample.
- Bridge `decide` timeout: 5 s; a timeout or bridge error means `Stop { engine_unavailable }`.
- Pi `ui_prompt_respond` responder bound: 5 s. `editor` and `custom` dialogs are never auto-answered. AFK's own consent dialog (owner namespace `aoe-afk-work-consent-v1`) is never auto-answered.
- Text-menu corrective continuation: at most once per work period (an "item" before S-B).
- Bash default timeout in an AFK period: 600 s (10 minutes) when the agent passes none.
- Scope checks: request share 80 %; `max_diff_lines` default 1500.
- Rules file precedence: inline (`/afk-work on --rule`) over `~/.config/aoe/afk/rules.toml` over `<repo>/afk-rules.toml`. `stop` rules match before allow rules. No match → `Stop { no_rule }`.
- User format files in `~/.config/aoe/afk/menu-formats/*.toml` override built-ins with the same `id`.
- Answers are labelled `afk/<policy>` and never use the human-answer result shape.
- Repo rules (from `AGENTS.md` on `origin/main`): on the maintainer's Mac run fmt, clippy and tests on Bender with `aoe-gate` (it gates the committed `HEAD`, so commit WIP first); build locally only the binary you install. Do not run `cargo clean` or set `CARGO_TARGET_DIR`.
- Inside a Ledger Claude session, run `aoe`, `aoe-gate` and `cargo test` with `env -u CLAUDE_CONFIG_DIR`.
- Commit as `git -c user.email=jw@crasiak.net commit` with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Push or open PRs only after the user says so; PRs go to `--repo crasiak/agent-of-empires` and must fill every section of the PR template.

## Decisions made in this plan (not spelled out in the spec)

1. **Recognition runs in Rust, behind the bridge.** The spec says format files are loaded at extension start; the bridge process is spawned at extension start (`aoe-afk.mjs`), so formats are loaded and validated when `serve()` starts. One implementation serves Pi now and the S-C host responder later; Node needs no TOML parser.
2. **Answer log location before S-B:** `<transport store>/afk-answers/<period-id>.jsonl`, with unrecognised samples at `<transport store>/afk-answers/<period-id>-unrecognised-<n>.txt`. The decision line is written before delivery with `"delivered": "pending"`; delivery appends a second line `{ "schema": "aoe.afk-answer.v1", "answer_id", "delivered" }`. Readers fold by `answer_id`. This keeps the file append-only.
3. **`decide` takes the agent's proposal as a parameter:** `decide(menu, run, rules, proposal)`.
4. **Work-period snapshot version 2** adds `policy`, `rules` (inline rule strings) and `menu_nudges`. Version 1 records load as `policy: "rules"`, `rules: []`, `menu_nudges: 0`.
5. **No bridge version bump.** An older `aoe` binary rejects the new ops; the extension treats that as `engine_unavailable` and stops, which is the safe result.

## File Structure

| Path | Responsibility |
|---|---|
| `src/session/afk/answer.rs` | Module root: re-exports, `Policy`, `RunContext`, `Proposal`, `Decision` |
| `src/session/afk/answer/menu.rs` | `Menu`, `MenuInput`, `Source`, `Kind`, `Role`, role assignment from labels, validation |
| `src/session/afk/answer/formats.rs` | TOML format loading (built-in + user dir), `Formats::recognise` |
| `src/session/afk/answer/rules.rs` | Rules files, precedence, matching |
| `src/session/afk/answer/scope.rs` | Deterministic scope checks |
| `src/session/afk/answer/decide.rs` | `decide()` for `rules` and `yolo` |
| `src/session/afk/answer/log.rs` | Answer log and unrecognised samples |
| `src/session/afk/answer/run.rs` | Builds `RunContext` from the extension's run input (repo root, diff lines) |
| `src/session/afk/answer/tests.rs` | Unit tests and the format corpus test |
| `src/session/anchored_fs.rs` | New `open_append_regular` |
| `src/session/afk/bridge.rs` | Ops `recognise`, `decide`, `answer_delivered`, `rules_check` |
| `assets/afk/menu-formats/*.toml` | Built-in formats |
| `assets/afk/menu-formats/fixtures/<id>/*.{txt,json}` | Format corpus |
| `assets/session/aoe-afk-answer.mjs` | Bridge client with timeout, `aoe_afk_answer` tool schema, `ask_user` and dialog mapping, bash prompt heuristics |
| `assets/session/aoe-afk-answer.test.mjs` | Node tests for the above and the integration in the work period |
| `assets/session/aoe-afk-work-state.mjs` | Snapshot v2 (policy, rules, menu nudge) |
| `assets/session/aoe-afk-work-period.mjs` | Wiring: tool, `ask_user`, bash, settle fallback, dialog responder, consent, status |
| `src/session/afk.rs` | Embed `aoe-afk-answer.mjs` with the other extension assets |
| `docs/guides/afk.md` | User docs for policies, rules, formats, the answer log |
| Pi fork `packages/coding-agent/src/core/extensions/{runner,types}.ts` | `ui_prompt_respond` hook and `answered_by` |

---

### Task 1: Land the native work period on main

The native work period (the base for every other task) is committed as WIP `59d6a9ccb` on `feature/CRA-859-afk-headroom-rollout` (pushed to `crasiak/agent-of-empires`; worktree `~/code/agent-of-empires-worktrees/CRA-859-afk-headroom-rollout-20261027`), based on `2145bd7f8`, behind `origin/main`. Five of its modified files also changed on main: `docs/guides/afk.md`, `src/session/instance/accessors.rs`, `src/session/instance/execution.rs`, `src/session/instance/identity_sidecar.rs`, `src/session/mod.rs`.

**Files:**
- Modify: the 22 tracked files listed by `git status --short` in that worktree
- Create: `assets/session/aoe-afk-work-{native,period,state}.mjs` and their tests, `assets/session/aoe-afk-work-sdk.test.mjs`, `src/session/afk/work_period.rs`, `src/session/ledger_pi.rs`
- Do not add: `.pi-subagents/` (a local artifacts link)

**Interfaces:**
- Produces: on a new branch from `origin/main`, the modules `aoe-afk-work-state.mjs` (`createWorkPeriod`), `aoe-afk-work-period.mjs` (`workPeriodRuntime`, `workCheckpointParameters`, `WORK_CHECKPOINT_TOOL`), `aoe-afk-work-native.mjs` (`createNativeWorkJournal`, `WORK_PERIOD_MARKER`), Rust `afk::work_period` and the bridge op `work_period_reply`. Every later task builds on this branch.

- [x] **Step 1: Snapshot the uncommitted tree as a WIP commit on its own branch** (done 2026-10-31: `WIP_SHA` = `59d6a9ccb`; do not re-run)

```bash
cd ~/code/agent-of-empires-worktrees/CRA-859-afk-headroom-rollout-20261027
git add -A -- . ':!.pi-subagents'
git status --short   # expect no '??' except .pi-subagents/
git -c user.email=jw@crasiak.net commit -m "wip(afk): native work period (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git rev-parse --short HEAD   # record as WIP_SHA
```

- [ ] **Step 2: Create the landing worktree from origin/main and cherry-pick**

```bash
cd ~/code/agent-of-empires
git fetch origin
git worktree add ~/code/agent-of-empires-worktrees/CRA-859-afk-menu-answering -b feat/CRA-859-afk-menu-answering origin/main
cd ~/code/agent-of-empires-worktrees/CRA-859-afk-menu-answering
git cherry-pick <WIP_SHA>
```

Expected: conflicts in some of the five overlapping files.

- [ ] **Step 3: Resolve conflicts**

For each conflicted file keep both sides: main's additions (e.g. the session-search, highlighting and Tailored Sessions changes) and the work-period additions. In `src/session/mod.rs`, keep both `mod` lines. In `docs/guides/afk.md`, keep main's text and insert the work-period sections where the WIP placed them. Then:

```bash
git diff --name-only --diff-filter=U   # expect empty after resolving
git add -A && git -c core.editor=true cherry-pick --continue
```

- [ ] **Step 4: Run the Node tests**

Run: `node --test assets/session/*.test.mjs`
Expected: all pass. If a test fails, compare against the result on the WIP commit in the old worktree before changing anything. Fix only conflict-resolution mistakes here.

- [ ] **Step 5: Run the Rust gate on Bender**

Run: `env -u CLAUDE_CONFIG_DIR aoe-gate`
Expected: fmt, clippy and tests pass. Known local-only failures (macOS socket path, `CLAUDE_CONFIG_DIR` host tests) are not relevant on Bender. A first-run `acp_runner_control` flake is known; if it fails, rerun that gate once (`aoe-gate test`) and record both results.

- [ ] **Step 6: Reword the commit**

```bash
git -c user.email=jw@crasiak.net commit --amend -m "feat(afk): native session-bound work period (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Do not push yet. Pushing and the PR happen at the end of the plan, after the user says so (or earlier, if the user asks to land Task 1 alone).

---

### Task 2: `Menu` model and role assignment (Rust)

**Files:**
- Create: `src/session/afk/answer.rs`, `src/session/afk/answer/menu.rs`, `src/session/afk/answer/tests.rs`
- Modify: `src/session/afk.rs` (add `pub mod answer;`)

**Interfaces:**
- Produces:
  - `pub enum Source { AgentText, AskUser, BashPrompt, PiDialog, Pane }` (serde snake_case)
  - `pub enum Kind { Plan, Choice, Permission, Confirm, Input }`
  - `pub enum Role { Approve, ApproveAlways, Deny, Revise, Other }`
  - `pub struct MenuOption { pub key: String, pub label: String, pub role: Role }`
  - `pub struct Menu { schema, source, format, kind, question, subject: Option<String>, options: Vec<MenuOption>, free_text: bool, warnings: Vec<String>, deadline_ms: Option<u64>, raw: String }`
  - `pub struct MenuInput` (same as `Menu` without `schema`; options are `{ key, label }` with optional `role`)
  - `pub struct RoleWords { approve, deny, revise: Vec<String> }` with `RoleWords::default()`
  - `pub fn role_for(label: &str, words: &RoleWords) -> Role`
  - `impl Menu { pub fn from_input(input: MenuInput, words: &RoleWords) -> Result<Menu> }`
  - `pub const MENU_SCHEMA: &str = "aoe.afk-menu.v1"`, `pub const RAW_MAX: usize = 4096`

- [ ] **Step 1: Write the failing tests**

`src/session/afk/answer/tests.rs`:

```rust
use super::menu::*;

fn input(labels: &[&str]) -> MenuInput {
    MenuInput {
        source: Source::PiDialog,
        format: "pi.select".into(),
        kind: Kind::Choice,
        question: "Pick".into(),
        subject: None,
        options: labels
            .iter()
            .enumerate()
            .map(|(i, l)| InputOption { key: (i + 1).to_string(), label: (*l).into(), role: None })
            .collect(),
        free_text: false,
        warnings: vec![],
        deadline_ms: None,
        raw: String::new(),
    }
}

#[test]
fn roles_come_from_labels() {
    let w = RoleWords::default();
    assert_eq!(role_for("Yes", &w), Role::Approve);
    assert_eq!(role_for("Yes, allow all edits during this session", &w), Role::ApproveAlways);
    assert_eq!(role_for("Always allow", &w), Role::ApproveAlways);
    assert_eq!(role_for("No", &w), Role::Deny);
    assert_eq!(role_for("Revise", &w), Role::Revise);
    assert_eq!(role_for("Keep planning", &w), Role::Revise);
    assert_eq!(role_for("Banana", &w), Role::Other);
}

#[test]
fn from_input_assigns_roles_and_bounds_raw() {
    let mut i = input(&["Approve plan", "Revise", "Something else"]);
    i.raw = "x".repeat(RAW_MAX + 10);
    let m = Menu::from_input(i, &RoleWords::default()).unwrap();
    assert_eq!(m.schema, MENU_SCHEMA);
    assert_eq!(m.options.iter().map(|o| o.role).collect::<Vec<_>>(), vec![Role::Approve, Role::Revise, Role::Other]);
    assert_eq!(m.raw.len(), RAW_MAX);
}

#[test]
fn from_input_rejects_bad_menus() {
    let w = RoleWords::default();
    assert!(Menu::from_input(input(&[]), &w).is_err(), "choice without options");
    let mut dup = input(&["Yes", "No"]);
    dup.options[1].key = "1".into();
    assert!(Menu::from_input(dup, &w).is_err(), "duplicate keys");
    let mut free = input(&[]);
    free.kind = Kind::Input;
    free.free_text = true;
    assert!(Menu::from_input(free, &w).is_ok(), "input kind may have no options");
}

#[test]
fn explicit_role_is_kept() {
    let mut i = input(&["Proceed"]);
    i.options[0].role = Some(Role::Deny);
    let m = Menu::from_input(i, &RoleWords::default()).unwrap();
    assert_eq!(m.options[0].role, Role::Deny);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Commit WIP, then run: `env -u CLAUDE_CONFIG_DIR aoe-gate test` (or, off the Mac, `cargo test --lib session::afk::answer`).
Expected: compile errors, since `answer` doesn't exist yet.

- [ ] **Step 3: Implement**

`src/session/afk/answer.rs`:

```rust
//! Menu recognition and the AFK answer engine (S-A). Pure decisions; delivery lives in the extension.
pub mod decide;
pub mod formats;
pub mod log;
pub mod menu;
pub mod rules;
pub mod run;
pub mod scope;
#[cfg(test)]
mod tests;

pub use decide::{decide, Check, CheckResult, Decision, Policy, Proposal, RunContext};
pub use formats::Formats;
pub use menu::{Kind, Menu, MenuInput, Role, RoleWords, Source};
pub use rules::Rules;
```

Until the later tasks exist, declare only `menu` and `tests` and add each module when its task lands.

`src/session/afk/answer/menu.rs`:

```rust
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const MENU_SCHEMA: &str = "aoe.afk-menu.v1";
pub const RAW_MAX: usize = 4096;
const TEXT_MAX: usize = 2048;
const OPTIONS_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source { AgentText, AskUser, BashPrompt, PiDialog, Pane }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind { Plan, Choice, Permission, Confirm, Input }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { Approve, ApproveAlways, Deny, Revise, Other }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuOption { pub key: String, pub label: String, pub role: Role }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Menu {
    pub schema: String,
    pub source: Source,
    pub format: String,
    pub kind: Kind,
    pub question: String,
    pub subject: Option<String>,
    pub options: Vec<MenuOption>,
    pub free_text: bool,
    pub warnings: Vec<String>,
    pub deadline_ms: Option<u64>,
    pub raw: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputOption { pub key: String, pub label: String, #[serde(default)] pub role: Option<Role> }

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuInput {
    pub source: Source,
    pub format: String,
    pub kind: Kind,
    pub question: String,
    #[serde(default)] pub subject: Option<String>,
    #[serde(default)] pub options: Vec<InputOption>,
    #[serde(default)] pub free_text: bool,
    #[serde(default)] pub warnings: Vec<String>,
    #[serde(default)] pub deadline_ms: Option<u64>,
    #[serde(default)] pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleWords {
    #[serde(default)] pub approve: Vec<String>,
    #[serde(default)] pub deny: Vec<String>,
    #[serde(default)] pub revise: Vec<String>,
}
impl Default for RoleWords {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
        Self {
            approve: v(&["yes", "y", "approve", "apply", "proceed", "continue", "accept", "ok", "allow", "confirm"]),
            deny: v(&["no", "n", "deny", "reject", "skip", "cancel", "abort", "decline", "stop"]),
            revise: v(&["revise", "edit", "amend", "change", "keep planning", "tell", "modify"]),
        }
    }
}

/// "Always"/"all … this session"/"don't ask again" make an approval standing, whatever else the label says.
pub fn role_for(label: &str, words: &RoleWords) -> Role {
    let l = label.trim().to_lowercase();
    let first = l.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("");
    let has = |ws: &[String]| ws.iter().any(|w| l == *w || first == w || l.starts_with(&format!("{w} ")));
    if has(&words.approve) || l.contains("always") {
        if l.contains("always") || l.contains("don't ask") || l.contains("do not ask") || (l.contains("all") && l.contains("session")) {
            return Role::ApproveAlways;
        }
        return Role::Approve;
    }
    if has(&words.deny) { return Role::Deny; }
    if has(&words.revise) { return Role::Revise; }
    Role::Other
}

fn bounded(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) { end -= 1; }
    s[..end].to_string()
}

impl Menu {
    pub fn from_input(input: MenuInput, words: &RoleWords) -> Result<Menu> {
        ensure!(!input.format.is_empty() && input.format.len() <= 128, "invalid menu format id");
        ensure!(!input.question.trim().is_empty(), "menu question required");
        ensure!(input.options.len() <= OPTIONS_MAX, "too many menu options");
        ensure!(input.kind == Kind::Input || !input.options.is_empty(), "menu has no options");
        let mut seen = std::collections::HashSet::new();
        for o in &input.options {
            ensure!(!o.key.is_empty() && o.key.len() <= 64 && !o.label.is_empty(), "invalid menu option");
            ensure!(seen.insert(o.key.clone()), "duplicate menu option key");
        }
        Ok(Menu {
            schema: MENU_SCHEMA.into(),
            source: input.source,
            format: input.format,
            kind: input.kind,
            question: bounded(&input.question, TEXT_MAX),
            subject: input.subject.map(|s| bounded(&s, TEXT_MAX)),
            options: input.options.into_iter().map(|o| MenuOption {
                role: o.role.unwrap_or_else(|| role_for(&o.label, words)),
                label: bounded(&o.label, 256),
                key: o.key,
            }).collect(),
            free_text: input.free_text,
            warnings: input.warnings.into_iter().take(8).map(|w| bounded(&w, 512)).collect(),
            deadline_ms: input.deadline_ms,
            raw: bounded(&input.raw, RAW_MAX),
        })
    }
}
```

`role_for("Keep planning")`: `first` is "keep", but `l.starts_with("keep planning ")` is false and `l == "keep planning"` is true, so it returns `Revise`. Check that the test passes; if `has` misses multi-word entries, compare `l == w || l.starts_with(&format!("{w} "))` for entries that contain a space.

- [ ] **Step 4: Run the tests to verify they pass**

Commit WIP, then run `env -u CLAUDE_CONFIG_DIR aoe-gate test`.
Expected: the four `answer::tests` pass.

- [ ] **Step 5: Commit**

```bash
git add src/session/afk.rs src/session/afk/answer.rs src/session/afk/answer/
git -c user.email=jw@crasiak.net commit --amend --no-edit   # if you made a WIP commit, otherwise a normal commit:
git -c user.email=jw@crasiak.net commit -m "feat(afk): menu model and role assignment (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Format recognisers and the corpus

**Files:**
- Create: `src/session/afk/answer/formats.rs`
- Create: `assets/afk/menu-formats/text.bracket-shortkey.toml`, `text.numbered-list.toml`, `text.yes-no.toml`, `text.select-prompt.toml`
- Create: `assets/afk/menu-formats/fixtures/<format-id>/<case>.txt` and `<case>.json` (at least two positive cases per format, plus `fixtures/_negative/*.txt` that must match nothing)
- Test: `src/session/afk/answer/tests.rs`

**Interfaces:**
- Consumes: `Menu`, `MenuInput`, `InputOption`, `Source`, `Kind`, `RoleWords`, `role_for` (Task 2)
- Produces:
  - `pub struct Formats` with `pub fn load(user_dir: Option<&Path>) -> (Formats, Vec<String>)` (warnings for skipped files), `pub fn from_sources(files: &[(String, String)]) -> (Formats, Vec<String>)`, `pub fn recognise(&self, source: Source, text: &str) -> Option<Menu>`, `pub fn ids(&self) -> Vec<String>`
  - `pub const BUILTIN: &[(&str, &str)]` (id-less file name and contents via `include_str!`)
  - `pub fn user_dir() -> Option<PathBuf>` = `~/.config/aoe/afk/menu-formats`

Format file schema (all keys; `deny_unknown_fields`):

```toml
id = "text.bracket-shortkey"          # required, unique
sources = ["agent_text", "bash_prompt", "pane"]
kind = "choice"
trigger = '…'                          # optional regex; a line in the window must match; that line is the question
option_pattern = '…'                   # optional regex with (?P<label>…) and optional (?P<key>…); matched per line, all matches collected
options_after_trigger = false          # only scan lines after the trigger line
min_options = 2
question_lines_above = 3               # when no trigger: up to N non-empty lines above the first option line
[[option]]                             # fixed options, used with trigger and no option_pattern
key = "y"
label = "Yes"
[roles]                                # optional, extends the defaults
approve = ["apply"]
```

The window is the last 12 non-empty lines of the text. A format must have `option_pattern` or at least one `[[option]]`.

- [ ] **Step 1: Write the built-in formats**

`assets/afk/menu-formats/text.bracket-shortkey.toml`:

```toml
id = "text.bracket-shortkey"
sources = ["agent_text", "bash_prompt", "pane"]
kind = "choice"
# A line with two or more [X]word options, e.g. "[A]pply / [S]kip / [R]evise"
option_pattern = '\[(?P<key>[A-Za-z0-9])\](?P<label>[\w-]+)'
min_options = 2
question_lines_above = 3
[roles]
approve = ["apply", "approve", "yes", "proceed", "continue"]
deny = ["no", "skip", "cancel", "abort"]
revise = ["revise", "edit", "amend", "change"]
```

`text.numbered-list.toml`:

```toml
id = "text.numbered-list"
sources = ["agent_text", "bash_prompt", "pane"]
kind = "choice"
# "1. Yes" / "2) No" lines; needs a question line ending in "?" or ":" above
option_pattern = '^\s*(?P<key>[1-9][0-9]?)[.)]\s+(?P<label>\S.{0,200})$'
trigger = '[?:]\s*$'
options_after_trigger = true
min_options = 2
```

`text.yes-no.toml`:

```toml
id = "text.yes-no"
sources = ["agent_text", "bash_prompt", "pane"]
kind = "confirm"
trigger = '(\((?:y/N|Y/n|y/n|yes/no)\)|\[(?:y/N|Y/n|y/n)\])\s*[:?]?\s*$'
[[option]]
key = "y"
label = "Yes"
[[option]]
key = "n"
label = "No"
```

`text.select-prompt.toml`:

```toml
id = "text.select-prompt"
sources = ["bash_prompt", "pane"]
kind = "choice"
# inquirer/prompts style: "? Pick a template ›" followed by "❯ react" / "  vue"
trigger = '^\?\s.+(›|»|>)\s*$'
option_pattern = '^\s*(?:❯|>|›)?\s+(?P<label>[^\s?].{0,120})$'
options_after_trigger = true
min_options = 2
```

- [ ] **Step 2: Write fixtures**

Each case is `<case>.txt` (input text) and `<case>.json` (expected `Menu` minus `raw`, which the test ignores). Example `fixtures/text.bracket-shortkey/apply-skip.txt`:

```
I've prepared the migration.
Apply it now?
[A]pply / [S]kip / [R]evise
```

`apply-skip.json`:

```json
{ "schema": "aoe.afk-menu.v1", "source": "agent_text", "format": "text.bracket-shortkey", "kind": "choice",
  "question": "I've prepared the migration.\nApply it now?", "subject": null,
  "options": [ { "key": "A", "label": "Apply", "role": "approve" }, { "key": "S", "label": "Skip", "role": "deny" }, { "key": "R", "label": "Revise", "role": "revise" } ],
  "free_text": false, "warnings": [], "deadline_ms": null }
```

`fixtures/text.yes-no/npm-init.txt`:

```
Need to install the following packages:
create-vite@5.2.3
Ok to proceed? (y/N)
```

with `kind: "confirm"`, `question: "Ok to proceed? (y/N)"`, options `y/Yes/approve`, `n/No/deny`. The source in every fixture JSON is `agent_text`; the test recognises with that source (all four formats list it except `text.select-prompt`, whose fixtures use `bash_prompt`; put a `"source"` in the JSON and have the test use it).

`fixtures/_negative/prose.txt`: a paragraph of ordinary prose with a numbered list that has no question line above it, and one sentence ending in "?" with no options. It must not match any format.

- [ ] **Step 3: Write the failing tests**

Append to `tests.rs`:

```rust
use super::formats::*;
use std::path::Path;

fn corpus() -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/afk/menu-formats/fixtures") }

#[test]
fn every_builtin_format_loads() {
    let (f, warnings) = Formats::load(None);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(f.ids(), vec!["text.bracket-shortkey", "text.numbered-list", "text.yes-no", "text.select-prompt"]);
}

#[test]
fn format_corpus_matches_expected_menus() {
    let (f, _) = Formats::load(None);
    let mut checked = 0;
    for dir in std::fs::read_dir(corpus()).unwrap() {
        let dir = dir.unwrap().path();
        let id = dir.file_name().unwrap().to_string_lossy().to_string();
        for case in std::fs::read_dir(&dir).unwrap() {
            let case = case.unwrap().path();
            if case.extension().is_none_or(|e| e != "txt") { continue; }
            let text = std::fs::read_to_string(&case).unwrap();
            if id == "_negative" {
                for s in [Source::AgentText, Source::BashPrompt, Source::Pane] {
                    assert!(f.recognise(s, &text).is_none(), "{case:?} matched");
                }
            } else {
                let mut want: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(case.with_extension("json")).unwrap()).unwrap();
                let source: Source = serde_json::from_value(want["source"].clone()).unwrap();
                let got = f.recognise(source, &text).unwrap_or_else(|| panic!("{case:?} not recognised"));
                let mut got = serde_json::to_value(got).unwrap();
                got.as_object_mut().unwrap().remove("raw");
                want.as_object_mut().unwrap().remove("raw");
                assert_eq!(got, want, "{case:?}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 9, "corpus too small: {checked}");
}

#[test]
fn invalid_format_is_skipped_and_others_load() {
    let files = vec![
        ("bad.toml".to_string(), "id = 'x'\nkind = 'nope'".to_string()),
        ("bad-regex.toml".to_string(), "id='r'\nsources=['agent_text']\nkind='choice'\noption_pattern='('".to_string()),
        ("ok.toml".to_string(), BUILTIN[2].1.to_string()),
    ];
    let (f, warnings) = Formats::from_sources(&files);
    assert_eq!(warnings.len(), 2);
    assert_eq!(f.ids(), vec!["text.yes-no"]);
}

#[test]
fn user_format_overrides_builtin_with_same_id() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mine.toml"), "id='text.yes-no'\nsources=['agent_text']\nkind='confirm'\ntrigger='OK\\?$'\n[[option]]\nkey='y'\nlabel='Yes'\n[[option]]\nkey='n'\nlabel='No'").unwrap();
    let (f, warnings) = Formats::load(Some(dir.path()));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(f.recognise(Source::AgentText, "Continue? (y/N)").is_none());
    assert!(f.recognise(Source::AgentText, "All good. OK?").is_some());
}

#[test]
fn format_only_matches_its_sources() {
    let (f, _) = Formats::load(None);
    assert!(f.recognise(Source::AgentText, "? Pick a template ›\n❯ react\n  vue").is_none());
    assert!(f.recognise(Source::BashPrompt, "? Pick a template ›\n❯ react\n  vue").is_some());
}
```

- [ ] **Step 4: Run to verify they fail**

Commit WIP; `env -u CLAUDE_CONFIG_DIR aoe-gate test`. Expected: compile failure (no `formats`).

- [ ] **Step 5: Implement `formats.rs`**

```rust
use super::menu::{InputOption, Kind, Menu, MenuInput, RoleWords, Source};
use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const WINDOW: usize = 12;

pub const BUILTIN: &[(&str, &str)] = &[
    ("text.bracket-shortkey.toml", include_str!("../../../../assets/afk/menu-formats/text.bracket-shortkey.toml")),
    ("text.numbered-list.toml", include_str!("../../../../assets/afk/menu-formats/text.numbered-list.toml")),
    ("text.yes-no.toml", include_str!("../../../../assets/afk/menu-formats/text.yes-no.toml")),
    ("text.select-prompt.toml", include_str!("../../../../assets/afk/menu-formats/text.select-prompt.toml")),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixedOption { key: String, label: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatFile {
    id: String,
    sources: Vec<Source>,
    kind: Kind,
    #[serde(default)] trigger: Option<String>,
    #[serde(default)] option_pattern: Option<String>,
    #[serde(default)] options_after_trigger: bool,
    #[serde(default = "two")] min_options: usize,
    #[serde(default = "three")] question_lines_above: usize,
    #[serde(default, rename = "option")] fixed: Vec<FixedOption>,
    #[serde(default)] roles: Option<RoleWords>,
}
fn two() -> usize { 2 }
fn three() -> usize { 3 }

struct Format {
    id: String,
    sources: Vec<Source>,
    kind: Kind,
    trigger: Option<Regex>,
    option: Option<Regex>,
    after_trigger: bool,
    min_options: usize,
    above: usize,
    fixed: Vec<FixedOption>,
    roles: RoleWords,
}

pub struct Formats(Vec<Format>);

pub fn user_dir() -> Option<PathBuf> { dirs::home_dir().map(|h| h.join(".config/aoe/afk/menu-formats")) }

fn compile(name: &str, text: &str) -> Result<Format> {
    let f: FormatFile = toml::from_str(text).with_context(|| format!("{name}: invalid format file"))?;
    ensure!(!f.id.is_empty() && f.id.len() <= 128, "{name}: invalid id");
    ensure!(!f.sources.is_empty(), "{name}: no sources");
    ensure!(f.option_pattern.is_some() || (!f.fixed.is_empty() && f.trigger.is_some()), "{name}: needs option_pattern, or trigger and [[option]]");
    let re = |p: &Option<String>| p.as_deref().map(Regex::new).transpose().with_context(|| format!("{name}: invalid regex"));
    let mut roles = RoleWords::default();
    if let Some(extra) = f.roles {
        roles.approve.extend(extra.approve);
        roles.deny.extend(extra.deny);
        roles.revise.extend(extra.revise);
    }
    Ok(Format { trigger: re(&f.trigger)?, option: re(&f.option_pattern)?, id: f.id, sources: f.sources, kind: f.kind,
        after_trigger: f.options_after_trigger, min_options: f.min_options.max(1), above: f.question_lines_above, fixed: f.fixed, roles })
}

impl Formats {
    pub fn from_sources(files: &[(String, String)]) -> (Formats, Vec<String>) {
        let mut out: Vec<Format> = Vec::new();
        let mut warnings = Vec::new();
        for (name, text) in files {
            match compile(name, text) {
                // Later files (user dir) replace earlier ones (built-ins) with the same id, in place.
                Ok(f) => match out.iter().position(|o| o.id == f.id) { Some(i) => out[i] = f, None => out.push(f) },
                Err(e) => warnings.push(format!("{e:#}")),
            }
        }
        (Formats(out), warnings)
    }

    pub fn load(user: Option<&Path>) -> (Formats, Vec<String>) {
        let mut files: Vec<(String, String)> = BUILTIN.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect();
        let mut warnings = Vec::new();
        if let Some(dir) = user {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
                paths.sort();
                for p in paths.into_iter().take(64) {
                    match std::fs::read_to_string(&p) {
                        Ok(t) if t.len() <= 16 * 1024 => files.push((p.display().to_string(), t)),
                        Ok(_) => warnings.push(format!("{}: format file too large", p.display())),
                        Err(e) => warnings.push(format!("{}: {e}", p.display())),
                    }
                }
            }
        }
        let (formats, mut w) = Self::from_sources(&files);
        warnings.append(&mut w);
        (formats, warnings)
    }

    pub fn ids(&self) -> Vec<String> { self.0.iter().map(|f| f.id.clone()).collect() }

    /// First match wins, in load order. A recogniser that can't build a valid menu is no match.
    pub fn recognise(&self, source: Source, text: &str) -> Option<Menu> {
        let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
        let window = &lines[lines.len().saturating_sub(WINDOW)..];
        self.0.iter().filter(|f| f.sources.contains(&source)).find_map(|f| f.matches(source, window, text))
    }
}

impl Format {
    fn matches(&self, source: Source, window: &[&str], raw: &str) -> Option<Menu> {
        let trigger_at = match &self.trigger {
            Some(t) => Some(window.iter().rposition(|l| t.is_match(l))?),
            None => None,
        };
        let scan_from = if self.after_trigger { trigger_at.map_or(0, |i| i + 1) } else { 0 };
        let mut options = Vec::new();
        let mut first_option_line = None;
        if let Some(re) = &self.option {
            for (i, line) in window.iter().enumerate().skip(scan_from) {
                if Some(i) == trigger_at { continue; }
                for c in re.captures_iter(line) {
                    let label = c.name("label")?.as_str().trim().to_string();
                    let key = c.name("key").map_or_else(|| (options.len() + 1).to_string(), |k| k.as_str().to_string());
                    first_option_line.get_or_insert(i);
                    options.push(InputOption { key, label, role: None });
                }
            }
            if options.len() < self.min_options { return None; }
            // Options must be the end of the text: nothing but options after the first option line.
            if self.trigger.is_none() && first_option_line.is_some_and(|i| window[i..].iter().any(|l| !re.is_match(l))) { return None; }
        } else {
            options = self.fixed.iter().map(|o| InputOption { key: o.key.clone(), label: o.label.clone(), role: None }).collect();
        }
        let question = match trigger_at {
            Some(i) => window[i].trim().to_string(),
            None => {
                let end = first_option_line?;
                window[end.saturating_sub(self.above)..end].join("\n")
            }
        };
        if question.trim().is_empty() { return None; }
        Menu::from_input(MenuInput { source, format: self.id.clone(), kind: self.kind, question, subject: None, options,
            free_text: false, warnings: vec![], deadline_ms: None, raw: raw.to_string() }, &self.roles).ok()
    }
}
```

Check that `dirs` is already a dependency (`grep '^dirs' Cargo.toml`); if not, build the path from `std::env::var_os("HOME")`.

- [ ] **Step 6: Run the tests until they pass**

Commit WIP; `env -u CLAUDE_CONFIG_DIR aoe-gate test`. If a fixture fails, fix the format regex or the expected JSON. Change the recogniser logic only if the fixture shows a real gap. Keep `_negative` passing.

- [ ] **Step 7: Commit**

```bash
git add assets/afk src/session/afk/answer
git -c user.email=jw@crasiak.net commit -m "feat(afk): TOML menu formats with fixture corpus (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Rules files and matching

**Files:**
- Create: `src/session/afk/answer/rules.rs`
- Test: `src/session/afk/answer/tests.rs`

**Interfaces:**
- Consumes: `Menu`, `Kind`, `Source`, `Role` (Task 2)
- Produces:
  - `pub enum RuleAnswer { Role(Role), Agent, Stop }` (TOML string: `"approve" | "deny" | "revise" | "other" | "agent" | "stop"`; `"approve_always"` is a parse error)
  - `pub struct Rule { pub id: String, pub answer: RuleAnswer, pub allow_dependencies: bool, … }`
  - `pub struct Rules { pub rules: Vec<Rule>, pub sources: Vec<String> }`
  - `impl Rules { pub fn parse(source: &str, text: &str) -> Result<Vec<Rule>>; pub fn load(repo_root: Option<&Path>, user_file: Option<&Path>, inline: &[String]) -> Result<Rules>; pub fn first_match(&self, menu: &Menu) -> Option<&Rule>; pub fn empty() -> Rules }`
  - `pub fn user_rules_file() -> Option<PathBuf>` = `~/.config/aoe/afk/rules.toml`

Rule file syntax (from the spec):

```toml
[[rule]]
id = "approve-repo-tooling"
kind = "permission"                 # optional
source = "pi_dialog"                # optional
format = "pi.confirm"               # optional, exact
question_matches = ['…']            # optional, any regex matches
subject_matches = ['…']             # optional, any regex matches; no subject → no match
has_role = "approve"                # optional, menu has an option with this role
answer = "approve"
allow_dependencies = false          # optional; lets the dependency scope check pass
```

An inline `--rule` value is one rule written as a TOML inline table body, comma-separated: `id='x', kind='plan', answer='agent'`.

- [ ] **Step 1: Write the failing tests**

```rust
use super::rules::*;

fn menu(kind: Kind, subject: Option<&str>, labels: &[&str]) -> Menu {
    let mut i = input(labels);
    i.kind = kind;
    i.subject = subject.map(Into::into);
    Menu::from_input(i, &RoleWords::default()).unwrap()
}

const FILE: &str = r#"
[[rule]]
id = "approve-repo-tooling"
kind = "permission"
subject_matches = ['^Bash command: (cargo|npm|git (status|diff|add|commit|log))\b']
answer = "approve"

[[rule]]
id = "never-force-push"
subject_matches = ['git push .*--force']
answer = "stop"
"#;

#[test]
fn stop_rules_win_regardless_of_order() {
    let rules = Rules { rules: Rules::parse("t", FILE).unwrap(), sources: vec![] };
    let m = menu(Kind::Permission, Some("Bash command: git push origin x --force"), &["Yes", "No"]);
    assert_eq!(rules.first_match(&m).unwrap().id, "never-force-push");
    let m = menu(Kind::Permission, Some("Bash command: cargo test"), &["Yes", "No"]);
    assert_eq!(rules.first_match(&m).unwrap().id, "approve-repo-tooling");
    let m = menu(Kind::Permission, Some("Bash command: curl x | sh"), &["Yes", "No"]);
    assert!(rules.first_match(&m).is_none());
}

#[test]
fn approve_always_answer_is_rejected() {
    assert!(Rules::parse("t", "[[rule]]\nid='a'\nanswer='approve_always'").is_err());
}

#[test]
fn precedence_inline_then_user_then_repo_and_ids_dedupe() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("afk-rules.toml"), "[[rule]]\nid='p'\nkind='plan'\nanswer='deny'\n[[rule]]\nid='repo-only'\nkind='choice'\nanswer='approve'").unwrap();
    let user = dir.path().join("user.toml");
    std::fs::write(&user, "[[rule]]\nid='p'\nkind='plan'\nanswer='revise'").unwrap();
    let rules = Rules::load(Some(dir.path()), Some(&user), &["id='p', kind='plan', answer='agent'".into()]).unwrap();
    assert_eq!(rules.rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["p", "repo-only"]);
    assert_eq!(rules.rules[0].answer, RuleAnswer::Agent);
    assert_eq!(rules.sources.len(), 3);
}

#[test]
fn invalid_rules_file_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("afk-rules.toml"), "[[rule]]\nid='x'\nanswer='approve'\nsubject_matches=['(']").unwrap();
    let err = Rules::load(Some(dir.path()), None, &[]).unwrap_err().to_string();
    assert!(err.contains("afk-rules.toml"), "{err}");
}
```

- [ ] **Step 2: Run to verify failure** (commit WIP; `aoe-gate test`). Expected: compile failure.

- [ ] **Step 3: Implement `rules.rs`**

```rust
use super::menu::{Kind, Menu, Role, Source};
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleAnswer { Role(Role), Agent, Stop }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    #[serde(default)] kind: Option<Kind>,
    #[serde(default)] source: Option<Source>,
    #[serde(default)] format: Option<String>,
    #[serde(default)] question_matches: Vec<String>,
    #[serde(default)] subject_matches: Vec<String>,
    #[serde(default)] has_role: Option<Role>,
    answer: String,
    #[serde(default)] allow_dependencies: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile { #[serde(default)] rule: Vec<RawRule> }

#[derive(Debug)]
pub struct Rule {
    pub id: String,
    pub kind: Option<Kind>,
    pub source: Option<Source>,
    pub format: Option<String>,
    pub question: Vec<Regex>,
    pub subject: Vec<Regex>,
    pub has_role: Option<Role>,
    pub answer: RuleAnswer,
    pub allow_dependencies: bool,
    pub origin: String,
}

#[derive(Debug, Default)]
pub struct Rules { pub rules: Vec<Rule>, pub sources: Vec<String> }

pub fn user_rules_file() -> Option<PathBuf> { dirs::home_dir().map(|h| h.join(".config/aoe/afk/rules.toml")) }

fn answer(s: &str) -> Result<RuleAnswer> {
    Ok(match s {
        "approve" => RuleAnswer::Role(Role::Approve),
        "deny" => RuleAnswer::Role(Role::Deny),
        "revise" => RuleAnswer::Role(Role::Revise),
        "other" => RuleAnswer::Role(Role::Other),
        "agent" => RuleAnswer::Agent,
        "stop" => RuleAnswer::Stop,
        other => bail!("answer {other:?} is not allowed (approve|deny|revise|other|agent|stop)"),
    })
}

impl Rule {
    fn matches(&self, m: &Menu) -> bool {
        self.kind.is_none_or(|k| k == m.kind)
            && self.source.is_none_or(|s| s == m.source)
            && self.format.as_ref().is_none_or(|f| *f == m.format)
            && (self.question.is_empty() || self.question.iter().any(|r| r.is_match(&m.question)))
            && (self.subject.is_empty() || m.subject.as_ref().is_some_and(|s| self.subject.iter().any(|r| r.is_match(s))))
            && self.has_role.is_none_or(|r| m.options.iter().any(|o| o.role == r))
    }
}

impl Rules {
    pub fn empty() -> Rules { Rules::default() }

    pub fn parse(origin: &str, text: &str) -> Result<Vec<Rule>> {
        let file: RawFile = toml::from_str(text).with_context(|| format!("{origin}: invalid rules file"))?;
        file.rule.into_iter().map(|r| {
            ensure!(!r.id.is_empty() && r.id.len() <= 128, "{origin}: rule id required");
            let re = |xs: &[String]| xs.iter().map(|p| Regex::new(p)).collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("{origin}: rule {}: invalid regex", r.id));
            Ok(Rule { question: re(&r.question_matches)?, subject: re(&r.subject_matches)?,
                answer: answer(&r.answer).with_context(|| format!("{origin}: rule {}", r.id))?,
                id: r.id.clone(), kind: r.kind, source: r.source, format: r.format, has_role: r.has_role,
                allow_dependencies: r.allow_dependencies, origin: origin.to_string() })
        }).collect()
    }

    /// Inline first, then user, then repo; the first definition of an id wins.
    pub fn load(repo_root: Option<&Path>, user_file: Option<&Path>, inline: &[String]) -> Result<Rules> {
        let mut out = Rules::default();
        let mut add = |origin: String, rules: Vec<Rule>, out: &mut Rules| {
            out.sources.push(origin);
            for r in rules { if !out.rules.iter().any(|o| o.id == r.id) { out.rules.push(r); } }
        };
        for (i, body) in inline.iter().enumerate() {
            ensure!(body.len() <= 1024, "inline rule {} too long", i + 1);
            let origin = format!("inline rule {}", i + 1);
            // `rule = [{ … }]` is the same array-of-tables as `[[rule]]`, written inline.
            add(origin.clone(), Self::parse(&origin, &format!("rule = [{{ {body} }}]"))?, &mut out);
        }
        for path in user_file.into_iter().map(Path::to_path_buf).chain(repo_root.map(|r| r.join("afk-rules.toml"))) {
            if !path.exists() { continue; }
            let text = std::fs::read_to_string(&path).with_context(|| format!("{}: unreadable", path.display()))?;
            ensure!(text.len() <= 64 * 1024, "{}: rules file too large", path.display());
            add(path.display().to_string(), Self::parse(&path.display().to_string(), &text)?, &mut out);
        }
        Ok(out)
    }

    pub fn first_match(&self, m: &Menu) -> Option<&Rule> {
        self.rules.iter().find(|r| r.answer == RuleAnswer::Stop && r.matches(m))
            .or_else(|| self.rules.iter().find(|r| r.answer != RuleAnswer::Stop && r.matches(m)))
    }
}
```

Add one more test: `Rules::load(None, None, &["id='q', subject_matches=['git push .*--force'], answer='stop'".into()])` parses and its regex (which contains spaces) matches `git push origin x --force`.

- [ ] **Step 4: Run tests until they pass.** Commit WIP; `aoe-gate test`.

- [ ] **Step 5: Commit**

```bash
git add src/session/afk/answer
git -c user.email=jw@crasiak.net commit -m "feat(afk): answer rules with precedence and stop-first matching (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Scope checks and `decide`

**Files:**
- Create: `src/session/afk/answer/scope.rs`, `src/session/afk/answer/decide.rs`
- Test: `src/session/afk/answer/tests.rs`

**Interfaces:**
- Consumes: `Menu`, `Role`, `Kind` (Task 2); `Rules`, `Rule`, `RuleAnswer` (Task 4)
- Produces:
  - `pub enum Policy { Rules, Yolo }` (serde snake_case)
  - `pub struct RunContext { pub goal: String, pub policy: Policy, pub repo_roots: Vec<PathBuf>, pub requests_used: u32, pub request_limit: u32, pub diff_lines: Option<u64>, pub max_diff_lines: u64, pub estimate_diff_lines: Option<u64> }`
  - `pub struct Proposal { pub key: Option<String>, pub text: Option<String>, pub reason: Option<String> }` (Deserialize)
  - `pub enum CheckResult { Pass, Fail, Warn }`; `pub struct Check { pub id: &'static str, pub result: CheckResult }` (Serialize)
  - `#[serde(tag = "decision", rename_all = "snake_case")] pub enum Decision { Answer { choice: Option<String>, text: Option<String>, rule: Option<String>, reason: String, warnings: Vec<String>, checks: Vec<Check> }, Stop { reason: String, checks: Vec<Check> } }`
  - `pub fn decide(menu: &Menu, run: &RunContext, rules: &Rules, proposal: Option<&Proposal>) -> Decision`
  - `scope::run_checks(menu: &Menu, run: &RunContext, allow_dependencies: bool) -> Vec<Check>` (check ids: `harness_warning`, `outside_repo`, `dependencies`, `protected_push`, `request_share`, `diff_limit`)

Policy behaviour, copied from the spec:
- `rules`: answer only if a rule allows it. Scope checks can turn an allowed answer into `Stop`. `approve_always` options are never chosen.
- `yolo`: answer every recognised menu. Use the agent's proposal if there is one, otherwise the first `approve` option, otherwise the first option. Scope checks become warnings.

Scope checks run only when the chosen option's role is not `deny` (refusing is always in scope).

- [ ] **Step 1: Write the failing tests**

```rust
use super::decide::*;
use std::path::PathBuf;

fn run(policy: Policy) -> RunContext {
    RunContext { goal: "g".into(), policy, repo_roots: vec![PathBuf::from("/repo")], requests_used: 1, request_limit: 10,
        diff_lines: Some(10), max_diff_lines: 1500, estimate_diff_lines: None }
}
fn rules(text: &str) -> Rules { Rules { rules: Rules::parse("t", text).unwrap(), sources: vec![] } }
fn perm(subject: &str) -> Menu { menu(Kind::Permission, Some(subject), &["Yes", "Yes, allow all edits during this session", "No"]) }
fn choice(d: &Decision) -> Option<&str> { if let Decision::Answer { choice, .. } = d { choice.as_deref() } else { None } }
fn stop(d: &Decision) -> Option<&str> { if let Decision::Stop { reason, .. } = d { Some(reason) } else { None } }

const TOOLING: &str = "[[rule]]\nid='tooling'\nkind='permission'\nsubject_matches=['^Bash command: cargo']\nanswer='approve'";

#[test]
fn rules_without_match_stops() {
    assert_eq!(stop(&decide(&perm("Bash command: ls"), &run(Policy::Rules), &Rules::empty(), None)), Some("no_rule"));
}

#[test]
fn rules_approve_picks_first_approve_never_always() {
    let d = decide(&perm("Bash command: cargo test"), &run(Policy::Rules), &rules(TOOLING), None);
    assert_eq!(choice(&d), Some("1"));
}

#[test]
fn rules_agent_answer_refuses_approve_always_proposal() {
    let r = rules("[[rule]]\nid='a'\nkind='permission'\nanswer='agent'");
    let p = Proposal { key: Some("2".into()), text: None, reason: None };
    assert_eq!(stop(&decide(&perm("Bash command: cargo test"), &run(Policy::Rules), &r, Some(&p))), Some("approve_always_refused"));
    assert_eq!(stop(&decide(&perm("Bash command: cargo test"), &run(Policy::Rules), &r, None)), Some("no_proposal"));
}

#[test]
fn rule_stop_reports_rule_id() {
    let r = rules("[[rule]]\nid='never'\nsubject_matches=['cargo']\nanswer='stop'");
    assert_eq!(stop(&decide(&perm("Bash command: cargo test"), &run(Policy::Rules), &r, None)), Some("rule_stop:never"));
}

#[test]
fn each_scope_check_stops_under_rules() {
    let allow_all = rules("[[rule]]\nid='all'\nkind='permission'\nanswer='approve'");
    let cases = [
        ("Bash command: rm -rf /etc/x", "scope:outside_repo"),
        ("Bash command: npm install left-pad", "scope:dependencies"),
        ("Bash command: cargo add serde", "scope:dependencies"),
        ("Edit file: /repo/Cargo.lock", "scope:dependencies"),
        ("Bash command: git push origin main", "scope:protected_push"),
        ("Bash command: git push -f origin x", "scope:protected_push"),
        ("Bash command: git branch -D x", "scope:protected_push"),
    ];
    for (subject, want) in cases {
        assert_eq!(stop(&decide(&perm(subject), &run(Policy::Rules), &allow_all, None)), Some(want), "{subject}");
    }
    let mut warned = perm("Bash command: rm -rf \"$DIR\"/");
    warned.warnings.push("Dangerous rm operation on possibly-empty variable path".into());
    assert_eq!(stop(&decide(&warned, &run(Policy::Rules), &allow_all, None)), Some("scope:harness_warning"));
    let mut late = run(Policy::Rules);
    late.requests_used = 9;
    let plan = menu(Kind::Plan, None, &["Approve plan", "Revise"]);
    let plans = rules("[[rule]]\nid='p'\nkind='plan'\nanswer='approve'");
    assert_eq!(stop(&decide(&plan, &late, &plans, None)), Some("scope:request_share"));
    let mut big = run(Policy::Rules);
    big.diff_lines = Some(1501);
    assert_eq!(stop(&decide(&plan, &big, &plans, None)), Some("scope:diff_limit"));
    big.estimate_diff_lines = Some(2000);
    assert_eq!(choice(&decide(&plan, &big, &plans, None)), Some("1"));
}

#[test]
fn deny_answers_skip_scope_checks_and_dependency_rule_allows() {
    let deny = rules("[[rule]]\nid='d'\nkind='permission'\nanswer='deny'");
    assert_eq!(choice(&decide(&perm("Bash command: git push -f"), &run(Policy::Rules), &deny, None)), Some("3"));
    let deps = rules("[[rule]]\nid='deps'\nkind='permission'\nsubject_matches=['npm install']\nanswer='approve'\nallow_dependencies=true");
    assert_eq!(choice(&decide(&perm("Bash command: npm install left-pad"), &run(Policy::Rules), &deps, None)), Some("1"));
}

#[test]
fn yolo_answers_everything_with_warnings() {
    let y = run(Policy::Yolo);
    let d = decide(&perm("Bash command: git push -f origin main"), &y, &Rules::empty(), None);
    match d { Decision::Answer { choice, warnings, .. } => { assert_eq!(choice.as_deref(), Some("1")); assert!(warnings.iter().any(|w| w.contains("protected_push"))); } _ => panic!() }
    let p = Proposal { key: Some("2".into()), text: None, reason: None };
    match decide(&perm("Bash command: ls"), &y, &Rules::empty(), Some(&p)) {
        Decision::Answer { choice, warnings, .. } => { assert_eq!(choice.as_deref(), Some("2")); assert!(warnings.iter().any(|w| w == "approve_always_chosen")); }
        _ => panic!(),
    }
    let other = menu(Kind::Choice, None, &["Banana", "Apple"]);
    assert_eq!(choice(&decide(&other, &y, &Rules::empty(), None)), Some("1"));
}

#[test]
fn free_text_inputs_need_the_agents_text() {
    let mut i = input(&[]);
    i.kind = Kind::Input;
    i.free_text = true;
    let m = Menu::from_input(i, &RoleWords::default()).unwrap();
    let p = Proposal { key: None, text: Some("feature/x".into()), reason: None };
    let r = rules("[[rule]]\nid='i'\nkind='input'\nanswer='agent'");
    assert!(matches!(decide(&m, &run(Policy::Rules), &r, Some(&p)), Decision::Answer { text: Some(_), .. }));
    assert_eq!(stop(&decide(&m, &run(Policy::Yolo), &Rules::empty(), None)), Some("no_text_answer"));
}

#[test]
fn unknown_proposal_key_is_ignored_under_yolo_and_refused_under_rules() {
    let p = Proposal { key: Some("9".into()), text: None, reason: None };
    let r = rules("[[rule]]\nid='a'\nkind='permission'\nanswer='agent'");
    assert_eq!(stop(&decide(&perm("Bash command: ls"), &run(Policy::Rules), &r, Some(&p))), Some("no_proposal"));
    assert_eq!(choice(&decide(&perm("Bash command: ls"), &run(Policy::Yolo), &Rules::empty(), Some(&p))), Some("1"));
}
```

- [ ] **Step 2: Run to verify failure.** Commit WIP; `aoe-gate test`.

- [ ] **Step 3: Implement `scope.rs`**

```rust
use super::decide::{Check, CheckResult, RunContext};
use super::menu::{Kind, Menu};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static DEPENDENCY: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"\b((npm|pnpm|yarn|bun)\s+(install|i|add)\s+[^-\s]|cargo\s+add\b|pip3?\s+install\b|uv\s+(add|pip\s+install)\b|go\s+get\b|(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|yarn\.lock|go\.(mod|sum)|pyproject\.toml|uv\.lock|requirements[^/\s]*\.txt)\b)"
).unwrap());
static PUSH: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"git\s+push\b.*(\s(-f|--force|--force-with-lease|--delete|-d)\b|\s\+\S|\s:\S|\s(origin\s+)?(main|master)\b)|git\s+branch\s+(-D|-d|--delete)\b"
).unwrap());
static READ_ONLY: &[&str] = &["cat", "ls", "grep", "rg", "head", "tail", "wc", "stat", "file", "less", "diff"];

fn under(path: &Path, roots: &[PathBuf]) -> bool { roots.iter().any(|r| path.starts_with(r)) }

fn outside_repo(subject: &str, roots: &[PathBuf]) -> bool {
    let command = subject.split_once(": ").map_or(subject, |(_, c)| c);
    if READ_ONLY.contains(&command.split_whitespace().next().unwrap_or("")) { return false; }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    command.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '=' || c == '>').any(|t| {
        let p = if let (Some(rest), Some(h)) = (t.strip_prefix("~/"), &home) { h.join(rest) } else { PathBuf::from(t) };
        (p.is_absolute() && !under(&p, roots)) || t.split('/').any(|c| c == "..")
    })
}

pub fn run_checks(menu: &Menu, run: &RunContext, allow_dependencies: bool) -> Vec<Check> {
    let subject = menu.subject.as_deref().unwrap_or("");
    let fail = |b: bool| if b { CheckResult::Fail } else { CheckResult::Pass };
    let limit = run.max_diff_lines.max(run.estimate_diff_lines.unwrap_or(0));
    vec![
        Check { id: "harness_warning", result: fail(!menu.warnings.is_empty()) },
        Check { id: "outside_repo", result: fail(!subject.is_empty() && outside_repo(subject, &run.repo_roots)) },
        Check { id: "dependencies", result: fail(!allow_dependencies && DEPENDENCY.is_match(subject)) },
        Check { id: "protected_push", result: fail(PUSH.is_match(subject)) },
        Check { id: "request_share", result: fail(menu.kind == Kind::Plan && u64::from(run.requests_used) * 100 > u64::from(run.request_limit) * 80) },
        Check { id: "diff_limit", result: fail(run.diff_lines.is_some_and(|d| d > limit)) },
    ]
}
```

`"rm -rf /etc/x"`: `/etc/x` is absolute and outside `/repo`, so it fails. Paths outside the repo that a read-only command touches are fine. For `"Edit file: /repo/Cargo.lock"`, `outside_repo` passes and `dependencies` fails.

- [ ] **Step 4: Implement `decide.rs`**

```rust
use super::menu::{Kind, Menu, Role};
use super::rules::{RuleAnswer, Rules};
use super::scope::run_checks;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy { Rules, Yolo }

#[derive(Clone, Debug)]
pub struct RunContext {
    pub goal: String,
    pub policy: Policy,
    pub repo_roots: Vec<PathBuf>,
    pub requests_used: u32,
    pub request_limit: u32,
    pub diff_lines: Option<u64>,
    pub max_diff_lines: u64,
    pub estimate_diff_lines: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal { #[serde(default)] pub key: Option<String>, #[serde(default)] pub text: Option<String>, #[serde(default)] pub reason: Option<String> }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult { Pass, Fail, Warn }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check { pub id: &'static str, pub result: CheckResult }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Answer { choice: Option<String>, text: Option<String>, rule: Option<String>, reason: String, warnings: Vec<String>, checks: Vec<Check> },
    Stop { reason: String, checks: Vec<Check> },
}

fn stop(reason: impl Into<String>) -> Decision { Decision::Stop { reason: reason.into(), checks: vec![] } }

fn proposed<'a>(menu: &'a Menu, p: Option<&Proposal>) -> Option<&'a super::menu::MenuOption> {
    let key = p?.key.as_deref()?;
    menu.options.iter().find(|o| o.key == key)
}

pub fn decide(menu: &Menu, run: &RunContext, rules: &Rules, proposal: Option<&Proposal>) -> Decision {
    let text = proposal.and_then(|p| p.text.clone()).filter(|t| !t.is_empty() && t.len() <= 1024);
    let reason = proposal.and_then(|p| p.reason.clone()).unwrap_or_default();
    match run.policy {
        Policy::Yolo => {
            if menu.kind == Kind::Input && menu.options.is_empty() {
                return match text { Some(t) => Decision::Answer { choice: None, text: Some(t), rule: None, reason, warnings: vec![], checks: vec![] }, None => stop("no_text_answer") };
            }
            let option = proposed(menu, proposal)
                .or_else(|| menu.options.iter().find(|o| o.role == Role::Approve))
                .or_else(|| menu.options.first());
            let Some(option) = option else { return stop("no_options") };
            let mut checks = if option.role == Role::Deny { vec![] } else { run_checks(menu, run, false) };
            let mut warnings: Vec<String> = checks.iter_mut().filter(|c| c.result == CheckResult::Fail)
                .map(|c| { c.result = CheckResult::Warn; format!("scope:{}", c.id) }).collect();
            if option.role == Role::ApproveAlways { warnings.push("approve_always_chosen".into()); }
            Decision::Answer { choice: Some(option.key.clone()), text: None, rule: None, reason, warnings, checks }
        }
        Policy::Rules => {
            let Some(rule) = rules.first_match(menu) else { return stop("no_rule") };
            let option = match &rule.answer {
                RuleAnswer::Stop => return stop(format!("rule_stop:{}", rule.id)),
                RuleAnswer::Agent if menu.kind == Kind::Input && menu.options.is_empty() => {
                    return match text {
                        Some(t) => Decision::Answer { choice: None, text: Some(t), rule: Some(rule.id.clone()), reason, warnings: vec![], checks: vec![] },
                        None => stop("no_proposal"),
                    };
                }
                RuleAnswer::Agent => match proposed(menu, proposal) { Some(o) => o, None => return stop("no_proposal") },
                RuleAnswer::Role(role) => match menu.options.iter().find(|o| o.role == *role) { Some(o) => o, None => return stop("no_option_for_role") },
            };
            if option.role == Role::ApproveAlways { return stop("approve_always_refused"); }
            let checks = if option.role == Role::Deny { vec![] } else { run_checks(menu, run, rule.allow_dependencies) };
            if let Some(failed) = checks.iter().find(|c| c.result == CheckResult::Fail) {
                return Decision::Stop { reason: format!("scope:{}", failed.id), checks };
            }
            Decision::Answer { choice: Some(option.key.clone()), text: None, rule: Some(rule.id.clone()), reason, warnings: vec![], checks }
        }
    }
}
```

- [ ] **Step 5: Run tests until they pass.** Commit WIP; `aoe-gate test`. If a scope-check case fails, fix the regex, not the test, unless the test contradicts the spec.

- [ ] **Step 6: Commit**

```bash
git add src/session/afk/answer
git -c user.email=jw@crasiak.net commit -m "feat(afk): answer engine with rules/yolo policies and scope checks (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Answer log, run context, and the bridge ops

**Files:**
- Create: `src/session/afk/answer/log.rs`, `src/session/afk/answer/run.rs`
- Modify: `src/session/anchored_fs.rs` (add `open_append_regular`), `src/session/afk/bridge.rs` (new ops; formats loaded in `serve`)
- Test: `src/session/afk/answer/tests.rs`, `src/session/afk/bridge.rs` (existing `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `Menu`, `MenuInput`, `RoleWords` (Task 2); `Formats` (Task 3); `Rules`, `user_rules_file` (Task 4); `decide`, `Decision`, `Policy`, `Proposal`, `RunContext` (Task 5)
- Produces:
  - `AnchoredDir::open_append_regular(&self, relative: &Path) -> Result<File>` (`O_CREAT|O_APPEND|O_WRONLY|O_NOFOLLOW|O_CLOEXEC`, mode 0600, refuses non-regular files or files with `nlink > 1`)
  - `log::AnswerLog::new(root: &AnchoredDir, period: &str) -> Result<AnswerLog>` (creates/opens `afk-answers/` under `root`); `fn decision(&self, record: &serde_json::Value) -> Result<String /*answer_id*/>`; `fn delivered(&self, answer_id: &str, status: Delivered) -> Result<()>`; `fn unrecognised(&self, raw: &str) -> Result<String /*file name*/>`; `pub enum Delivered { Ok, Failed, Unknown }`; `const ANSWER_SCHEMA = "aoe.afk-answer.v1"`; file cap 4 MiB, line cap 16 KiB
  - `run::RunInput { period: String, item: Option<String>, goal: String, policy: Policy, rules: Vec<String>, cwd: PathBuf, requests_used: u32, request_limit: u32 }` (Deserialize, `deny_unknown_fields`)
  - `run::context(input: &RunInput) -> RunContext` (repo root = `git -C cwd rev-parse --show-toplevel`, else `cwd`; `diff_lines` = sum of added+deleted from `git -C root diff --numstat <base>`, base = `git merge-base HEAD origin/HEAD`, else `HEAD`; `None` if git fails; `max_diff_lines` 1500)
  - Bridge ops (JSON, `op` tag):
    - `{"op":"recognise","source":"agent_text","text":"…"}` → `{ "menu": Menu|null }`
    - `{"op":"decide","run":RunInput,"menu":MenuInput?,"text":{"source":…,"text":…}?,"proposal":Proposal?}` → `{ "answer_id": "…", "menu": Menu, "decision": Decision }`; when `text` doesn't match any format: writes an unrecognised sample and returns `{ "answer_id", "menu": null, "decision": {"decision":"stop","reason":"unrecognised_menu","checks":[]}, "sample": "<file>" }`
    - `{"op":"answer_delivered","period":"…","answer_id":"…","delivered":"ok|failed|unknown"}` → `{ "recorded": true }`
    - `{"op":"rules_check","run":RunInput}` → `{ "valid": true, "count": n, "sources": [...], "format_warnings": [...] }`, or `{ "valid": false, "error": "<parse message>", "format_warnings": [...] }`. Errors are returned as values because the Node bridge client (`aoe-afk.mjs:38`) turns every `ok:false` reply into a generic "AFK bridge refused request", and the consent dialog must show the parse error. Under `yolo` it returns `valid: true, count: 0` without reading files.

Answer log record (one line):

```json
{ "schema": "aoe.afk-answer.v1", "ts": "<RFC3339>", "answer_id": "<uuid>", "run": "<period>", "item": null, "period": "<period>",
  "menu": { …Menu… }, "proposal": "1"|null, "decision": "answer"|"stop", "choice": "1"|null, "text": null, "policy": "rules",
  "rule": "approve-repo-tooling"|null, "checks": [ { "id": "outside_repo", "result": "pass" } ], "warnings": [], "reason": "…", "delivered": "pending" }
```

For a `Stop`, `delivered` is `"not_applicable"` and no delivery line follows.

- [ ] **Step 1: Write failing tests (log and run)**

```rust
use super::log::*;

#[test]
fn answer_log_appends_decision_then_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let anchored = crate::session::anchored_fs::AnchoredDir::open(dir.path()).unwrap();
    let log = AnswerLog::new(&anchored, "11111111-1111-1111-1111-111111111111").unwrap();
    let id = log.decision(&serde_json::json!({"decision":"answer","choice":"1"})).unwrap();
    log.delivered(&id, Delivered::Ok).unwrap();
    let text = std::fs::read_to_string(dir.path().join("afk-answers/11111111-1111-1111-1111-111111111111.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["schema"], "aoe.afk-answer.v1");
    assert_eq!(lines[0]["delivered"], "pending");
    assert_eq!(lines[1]["answer_id"], lines[0]["answer_id"]);
    assert_eq!(lines[1]["delivered"], "ok");
}

#[test]
fn unrecognised_samples_are_numbered() {
    let dir = tempfile::tempdir().unwrap();
    let log = AnswerLog::new(&crate::session::anchored_fs::AnchoredDir::open(dir.path()).unwrap(), "11111111-1111-1111-1111-111111111111").unwrap();
    assert_eq!(log.unrecognised("a").unwrap(), "11111111-1111-1111-1111-111111111111-unrecognised-1.txt");
    assert_eq!(log.unrecognised("b").unwrap(), "11111111-1111-1111-1111-111111111111-unrecognised-2.txt");
}

#[test]
fn answer_log_refuses_symlinked_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("afk-answers")).unwrap();
    std::os::unix::fs::symlink("/tmp/elsewhere", dir.path().join("afk-answers/11111111-1111-1111-1111-111111111111.jsonl")).unwrap();
    let log = AnswerLog::new(&crate::session::anchored_fs::AnchoredDir::open(dir.path()).unwrap(), "11111111-1111-1111-1111-111111111111").unwrap();
    assert!(log.decision(&serde_json::json!({})).is_err());
}

#[test]
fn run_context_falls_back_to_cwd_outside_git() {
    let dir = tempfile::tempdir().unwrap();
    let input = super::run::RunInput { period: "p".into(), item: None, goal: "g".into(), policy: Policy::Rules, rules: vec![],
        cwd: dir.path().to_path_buf(), requests_used: 0, request_limit: 4 };
    let ctx = super::run::context(&input);
    assert_eq!(ctx.repo_roots, vec![dir.path().canonicalize().unwrap()]);
    assert_eq!(ctx.diff_lines, None);
    assert_eq!(ctx.max_diff_lines, 1500);
}
```

Add a bridge test next to the existing `bridge_bounds_errors_and_eof_do_not_publish` test. It drives `serve()` with a `recognise` line, a `decide` line (a `MenuInput` `pi_dialog` confirm, policy `yolo`), an `answer_delivered` line and a `decide` with unrecognised `text`, then asserts the four replies: `menu` not null; `decision.decision == "answer"` with `choice == "1"`; `recorded == true`; `decision.reason == "unrecognised_menu"` with a `sample`. Build the store and inputs the same way the existing test does (read it first and copy its setup).

- [ ] **Step 2: Run to verify failure.** Commit WIP; `aoe-gate test`.

- [ ] **Step 3: Implement `open_append_regular` in `anchored_fs.rs`** (next to `create_new_regular`)

```rust
    /// Open `relative` for appending, creating it 0600 if absent. Never follows a symlink,
    /// and refuses anything that is not a single-link regular file.
    pub(crate) fn open_append_regular(&self, relative: &Path) -> Result<File> {
        let (parent, leaf) = self.open_parent(relative)?;
        let fd = openat(
            &parent,
            leaf.as_os_str(),
            OFlag::O_CREAT | OFlag::O_APPEND | OFlag::O_WRONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .context("opening anchored append file")?;
        let stat = fstat(&fd)?;
        anyhow::ensure!(
            (stat.st_mode & nix::libc::S_IFMT) == nix::libc::S_IFREG && stat.st_nlink == 1,
            "anchored append target is not a private regular file"
        );
        Ok(File::from(fd))
    }
```

- [ ] **Step 4: Implement `log.rs`**

```rust
use crate::session::anchored_fs::AnchoredDir;
use anyhow::{ensure, Result};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

pub const ANSWER_SCHEMA: &str = "aoe.afk-answer.v1";
const FILE_MAX: u64 = 4 * 1024 * 1024;
const LINE_MAX: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivered { Ok, Failed, Unknown }

pub struct AnswerLog { dir: AnchoredDir, period: String }

impl AnswerLog {
    pub fn new(root: &AnchoredDir, period: &str) -> Result<AnswerLog> {
        ensure!(uuid::Uuid::parse_str(period).is_ok(), "answer log needs a period id");
        Ok(AnswerLog { dir: root.create_child(Path::new("afk-answers"))?, period: period.to_string() })
    }
    fn append(&self, value: &serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        ensure!(line.len() <= LINE_MAX, "answer log line too large");
        let mut file = self.dir.open_append_regular(Path::new(&format!("{}.jsonl", self.period)))?;
        ensure!(file.metadata()?.len() + line.len() as u64 <= FILE_MAX, "answer log full");
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }
    pub fn decision(&self, record: &serde_json::Value) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut v = record.clone();
        let o = v.as_object_mut().ok_or_else(|| anyhow::anyhow!("answer record must be an object"))?;
        o.insert("schema".into(), ANSWER_SCHEMA.into());
        o.insert("answer_id".into(), id.clone().into());
        o.insert("ts".into(), chrono::Utc::now().to_rfc3339().into());
        o.entry("delivered").or_insert("pending".into());
        self.append(&v)?;
        Ok(id)
    }
    pub fn delivered(&self, answer_id: &str, status: Delivered) -> Result<()> {
        ensure!(uuid::Uuid::parse_str(answer_id).is_ok(), "invalid answer id");
        self.append(&serde_json::json!({ "schema": ANSWER_SCHEMA, "answer_id": answer_id, "ts": chrono::Utc::now().to_rfc3339(), "delivered": status }))
    }
    pub fn unrecognised(&self, raw: &str) -> Result<String> {
        for n in 1..=256 {
            let name = format!("{}-unrecognised-{n}.txt", self.period);
            if let Some(mut f) = self.dir.create_new_regular(Path::new(&name))? {
                f.write_all(&raw.as_bytes()[..raw.len().min(super::menu::RAW_MAX)])?;
                return Ok(name);
            }
        }
        anyhow::bail!("unrecognised sample capacity reached")
    }
}
```

`AnchoredDir::create_child(&self, relative)` (`anchored_fs.rs:131`) creates or opens the child. Then call `super::super::store::private_dir(&dir)?` on the result, the way `Store::open_namespace` does, so `afk-answers/` is 0700.

- [ ] **Step 5: Implement `run.rs`**

```rust
use super::decide::{Policy, RunContext};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    pub period: String,
    #[serde(default)] pub item: Option<String>,
    pub goal: String,
    pub policy: Policy,
    #[serde(default)] pub rules: Vec<String>,
    pub cwd: PathBuf,
    pub requests_used: u32,
    pub request_limit: u32,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn repo_root(cwd: &Path) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
        .or_else(|| cwd.canonicalize().ok()).unwrap_or_else(|| cwd.to_path_buf())
}

fn diff_lines(root: &Path) -> Option<u64> {
    let base = git(root, &["merge-base", "HEAD", "origin/HEAD"]).unwrap_or_else(|| "HEAD".into());
    let numstat = git(root, &["diff", "--numstat", &base])?;
    Some(numstat.lines().filter_map(|l| {
        let mut f = l.split('\t');
        Some(f.next()?.parse::<u64>().unwrap_or(0) + f.next()?.parse::<u64>().unwrap_or(0))
    }).sum())
}

pub fn context(input: &RunInput) -> RunContext {
    let root = repo_root(&input.cwd);
    RunContext { goal: input.goal.clone(), policy: input.policy, diff_lines: diff_lines(&root), repo_roots: vec![root],
        requests_used: input.requests_used, request_limit: input.request_limit, max_diff_lines: 1500, estimate_diff_lines: None }
}
```

- [ ] **Step 6: Add the bridge ops**

In `bridge.rs` add to `enum Command`:

```rust
    Recognise { source: answer::Source, text: String },
    Decide {
        run: answer::run::RunInput,
        #[serde(default)] menu: Option<answer::MenuInput>,
        #[serde(default)] text: Option<TextMenu>,
        #[serde(default)] proposal: Option<answer::Proposal>,
    },
    AnswerDelivered { period: String, answer_id: String, delivered: answer::log::Delivered },
    RulesCheck { run: answer::run::RunInput },
```

with

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextMenu { source: answer::Source, text: String }
```

At the top of `serve()`, after `let mut automatic_wake = …`:

```rust
    let (formats, format_warnings) = answer::Formats::load(answer::formats::user_dir().as_deref());
```

and in the `match command` add:

```rust
                Command::Recognise { source, text } => {
                    Ok(serde_json::json!({ "menu": formats.recognise(source, &text) }))
                }
                Command::RulesCheck { run } => {
                    if run.policy == answer::Policy::Yolo {
                        return Ok(serde_json::json!({ "valid": true, "count": 0, "sources": [], "format_warnings": format_warnings }));
                    }
                    let root = answer::run::repo_root(&run.cwd);
                    match answer::Rules::load(Some(&root), answer::rules::user_rules_file().as_deref(), &run.rules) {
                        Ok(rules) => Ok(serde_json::json!({ "valid": true, "count": rules.rules.len(), "sources": rules.sources, "format_warnings": format_warnings })),
                        Err(error) => Ok(serde_json::json!({ "valid": false, "error": format!("{error:#}"), "format_warnings": format_warnings })),
                    }
                }
                Command::Decide { run, menu, text, proposal } => {
                    let log = answer::log::AnswerLog::new(&store.dir, &run.period)?;
                    let menu = match (menu, text) {
                        (Some(input), None) => answer::Menu::from_input(input, &answer::RoleWords::default())?,
                        (None, Some(t)) => match formats.recognise(t.source, &t.text) {
                            Some(m) => m,
                            None => {
                                let sample = log.unrecognised(&t.text)?;
                                let decision = serde_json::json!({"decision":"stop","reason":"unrecognised_menu","checks":[]});
                                let id = log.decision(&serde_json::json!({ "run": run.period, "item": run.item, "period": run.period, "menu": null,
                                    "proposal": null, "decision": "stop", "policy": run.policy, "reason": "unrecognised_menu", "sample": sample, "delivered": "not_applicable" }))?;
                                return Ok(serde_json::json!({ "answer_id": id, "menu": null, "decision": decision, "sample": sample }));
                            }
                        },
                        _ => anyhow::bail!("decide needs exactly one of menu or text"),
                    };
                    let rules = match run.policy {
                        answer::Policy::Yolo => answer::Rules::empty(),
                        answer::Policy::Rules => answer::Rules::load(Some(&answer::run::repo_root(&run.cwd)), answer::rules::user_rules_file().as_deref(), &run.rules)?,
                    };
                    let decision = answer::decide(&menu, &answer::run::context(&run), &rules, proposal.as_ref());
                    let (kind, choice, text, rule, reason, warnings, checks, delivered) = match &decision {
                        answer::Decision::Answer { choice, text, rule, reason, warnings, checks } =>
                            ("answer", choice.clone(), text.clone(), rule.clone(), reason.clone(), warnings.clone(), checks.clone(), "pending"),
                        answer::Decision::Stop { reason, checks } => ("stop", None, None, None, reason.clone(), vec![], checks.clone(), "not_applicable"),
                    };
                    let id = log.decision(&serde_json::json!({ "run": run.period, "item": run.item, "period": run.period, "menu": menu,
                        "proposal": proposal.as_ref().and_then(|p| p.key.clone()), "decision": kind, "choice": choice, "text": text,
                        "policy": run.policy, "rule": rule, "checks": checks, "warnings": warnings, "reason": reason, "delivered": delivered }))?;
                    Ok(serde_json::json!({ "answer_id": id, "menu": menu, "decision": decision }))
                }
                Command::AnswerDelivered { period, answer_id, delivered } => {
                    answer::log::AnswerLog::new(&store.dir, &period)?.delivered(&answer_id, delivered)?;
                    Ok(serde_json::json!({ "recorded": true }))
                }
```

Bridge frames are bounded by `delegation::FRAME_BYTES` (256 KiB), so a `decide` carrying a 4 KiB `raw` fits.

- [ ] **Step 7: Run tests until they pass.** Commit WIP; `env -u CLAUDE_CONFIG_DIR aoe-gate`. Expected: all gates pass.

- [ ] **Step 8: Commit**

```bash
git add src/session/anchored_fs.rs src/session/afk/bridge.rs src/session/afk/answer
git -c user.email=jw@crasiak.net commit -m "feat(afk): answer log and recognise/decide bridge ops (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Work-period state v2 (policy, rules, menu nudge)

**Files:**
- Modify: `assets/session/aoe-afk-work-state.mjs`
- Test: `assets/session/aoe-afk-work-state.test.mjs`

**Interfaces:**
- Consumes: `createWorkPeriod` (Task 1)
- Produces:
  - Snapshot fields added: `policy: "rules" | "yolo"`, `rules: string[]` (≤ 8 entries, each ≤ 1024 bytes), `menu_nudges: 0 | 1`; `version: 2`
  - `grant` accepts optional `policy` (default `"rules"`) and `rules` (default `[]`)
  - `period.nudge(text)` → `{ continue: true, id, request, admission, next_step: text }` or `{ continue: false }`. Allowed once per period, only when `status === "owned"`. Consumes one continuation and sets `menu_nudges: 1`. It goes through the same `lastSettlement` bookkeeping as `settle()`.
  - v1 records in the journal load as v2 with `policy: "rules"`, `rules: []`, `menu_nudges: 0`

- [ ] **Step 1: Write failing tests** (follow the fixture style already in `aoe-afk-work-state.test.mjs`; read it first)

```js
test("grant carries policy and inline rules into every snapshot", () => {
  const { period } = fixture();
  const s = period.start({ ...grant(), policy: "yolo", rules: ["id='p', kind='plan', answer='agent'"] });
  assert.equal(s.version, 2);
  assert.equal(s.policy, "yolo");
  assert.deepEqual(s.rules, ["id='p', kind='plan', answer='agent'"]);
  assert.equal(s.menu_nudges, 0);
});

test("invalid policy or oversized rules refuse the grant", () => {
  const { period } = fixture();
  assert.throws(() => period.start({ ...grant(), policy: "review" }));
  assert.throws(() => period.start({ ...grant(), rules: Array(9).fill("id='x', answer='stop'") }));
});

test("menu nudge continues once without a checkpoint", () => {
  const { period } = fixture();
  period.start(grant()); period.reserve();
  const first = period.nudge("You ended with a menu. Resolve it with aoe_afk_answer.");
  assert.equal(first.continue, true);
  assert.equal(period.snapshot().menu_nudges, 1);
  period.reserve();
  assert.deepEqual(period.nudge("again"), { continue: false });
});

test("v1 journal rows load as rules policy", () => {
  const rows = v1History();            // build one start row with version 1 and no new fields
  const { period } = fixture({ rows });
  assert.equal(period.snapshot().policy, "rules");
  assert.equal(period.snapshot().menu_nudges, 0);
});
```

`fixture`, `grant` and `v1History` are helpers. Add them if the existing file doesn't have equivalents: `grant()` returns `{ id: randomUUID(), minutes: 60, requests: 4, task: "t", tools: ["bash"] }`, and `fixture({ rows })` returns `createWorkPeriod` over an in-memory journal `{ load: () => rows, append: r => rows.push(r) }`.

- [ ] **Step 2: Run to verify failure**

Run: `node --test assets/session/aoe-afk-work-state.test.mjs`
Expected: the new tests fail.

- [ ] **Step 3: Implement**

In `aoe-afk-work-state.mjs`:

```js
const FIELDS = [/* existing */ "version", "consent", "binding", "generation", "id", "revision", "task", "issued_at_ms", "expires_at_ms", "request_limit", "tool_limit", "tools", "requests_used", "tools_used", "checkpoint_count", "continuations_used", "status", "reason", "action", "checkpoint", "policy", "rules", "menu_nudges"];
const POLICIES = new Set(["rules", "yolo"]);
const validRules = r => Array.isArray(r) && r.length <= 8 && r.every(x => text(x, 1024));
const upgrade = s => s?.version === 1 ? { ...s, version: 2, policy: "rules", rules: [], menu_nudges: 0 } : s;
```

- `validate`: require `s.version === 2 && POLICIES.has(s.policy) && validRules(s.rules) && (s.menu_nudges === 0 || s.menu_nudges === 1)`.
- `identity`: add `s.policy` and `s.rules` to the array, so they can't change mid-period.
- `transition`: add `nudge` to the expected-change table as `[0, 0, 0, 1]` and require `next.menu_nudges === old.menu_nudges + 1 && old.status === "owned" && next.status === "owned"`. For every other action require `next.menu_nudges === old.menu_nudges`. Add `"nudge"` to the allowed `action` list in `validate`, and to the set of actions that require an admitted request (`["request", "tool", "continue", "nudge"]`).
- On load: `for (const raw of entries) { const s = upgrade(raw); validate(s, …) … }`.
- `start(grant)`: accept the keys `["id","minutes","requests","task","tools"]` plus optional `policy`/`rules`. Check `keys(grant, …)` against the exact set present: `const allowed = ["id","minutes","requests","task","tools","policy","rules"]; check(Object.keys(grant).every(k => allowed.includes(k)) && ["id","minutes","requests","task","tools"].every(k => k in grant), …)`. Validate `policy ?? "rules"` and `rules ?? []`, and store them with `version: 2, menu_nudges: 0`.
- New method:

```js
    nudge(textValue) {
      if (!active) return { continue: false };
      try { requireActive(); } catch { return { continue: false }; }
      if (current.status !== "owned" || current.menu_nudges >= 1 || current.requests_used >= current.request_limit
        || lastSettlement.request === current.requests_used) return { continue: false };
      update("nudge", { continuations_used: current.continuations_used + 1, menu_nudges: 1 });
      lastSettlement = { request: current.requests_used, checkpoint: current.checkpoint_count };
      return { continue: true, id: current.id, request: current.requests_used, admission: randomUUID(), next_step: textValue };
    },
```

`transition` requires `continue` to carry an unfinished checkpoint. `nudge` is a separate action, so that rule doesn't apply to it.

- [ ] **Step 4: Run all Node tests**

Run: `node --test assets/session/aoe-afk-work-*.test.mjs`
Expected: all pass. Existing tests that build grants or snapshots by hand may need `policy`, `rules` and `menu_nudges`; update them to v2 and add no behaviour changes.

- [ ] **Step 5: Commit**

```bash
git add assets/session/aoe-afk-work-state.mjs assets/session/aoe-afk-work-state.test.mjs assets/session/aoe-afk-work-*.test.mjs
git -c user.email=jw@crasiak.net commit -m "feat(afk): work-period snapshot v2 with policy, rules and one menu nudge (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Answer client, `aoe_afk_answer` tool, consent and status

**Files:**
- Create: `assets/session/aoe-afk-answer.mjs`, `assets/session/aoe-afk-answer.test.mjs`
- Modify: `assets/session/aoe-afk-work-period.mjs`, `assets/session/aoe-afk-runtime.mjs` (pass `Type`-built parameters for the new tool), `src/session/afk.rs` (embed the new asset)
- Test: `assets/session/aoe-afk-answer.test.mjs`, `assets/session/aoe-afk-work-period.test.mjs`

**Interfaces:**
- Consumes: bridge ops `decide`, `answer_delivered`, `rules_check` (Task 6); `period.snapshot().policy/rules`, `start({ policy, rules })` (Task 7)
- Produces (`aoe-afk-answer.mjs`):
  - `export const ANSWER_TOOL = "aoe_afk_answer"`
  - `export const DECIDE_TIMEOUT_MS = 5000`
  - `export function answerParameters(Type)`: `{ question: string ≤2048, options: [{ key ≤64, label ≤256 }] (1..32), proposal: string | null, reason: string ≤1024 }`
  - `export function createAnswerClient(bridge, { timeoutMs = DECIDE_TIMEOUT_MS } = {})` → `{ async decide({ run, menu?, text?, proposal? }) → { answer_id?, menu, decision }, async delivered(period, answer_id, status), async recognise(source, text) → Menu | null, async rulesCheck(run) → { valid, count?, sources?, error?, format_warnings } }` (`rulesCheck` throws only when the bridge is unavailable). `decide` never throws: on timeout or error it returns `{ menu: menu ?? null, decision: { decision: "stop", reason: "engine_unavailable", checks: [] } }`.
  - `export function runInput(snapshot, cwd)` → `RunInput` JSON (`period: s.id, item: null, goal: s.task, policy: s.policy, rules: s.rules, cwd, requests_used, request_limit`)
  - `export function answeredBy(policy)` → `` `afk/${policy}` ``
  - `export function parseOnArgs(tokens)` → `{ minutes?, requests?, policy, rules, error? }` for `/afk-work on [--policy rules|yolo] [--rule '<toml>']… [minutes] [requests]`
- Produces (`aoe-afk-work-period.mjs`):
  - The `aoe_afk_answer` tool is registered and admitted like the checkpoint tool (it doesn't count toward `tools_used`).
  - `w.answers = { rules: 0, yolo: 0, stops: 0 }`, reset at each period start.
  - `/afk-work status` text includes `answers: N (rules M, yolo K), stops: S`.
  - The consent dialog shows the policy, the rule count and sources, and for `yolo` the line `YOLO: AFK will answer every menu, including permission prompts. Scope checks only warn.`
  - REMINDER gains: `Never end a turn with a menu for the human. Call aoe_afk_answer with the question, options and your proposed option instead.`

- [ ] **Step 1: Write failing tests**

`assets/session/aoe-afk-answer.test.mjs`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import { createAnswerClient, parseOnArgs, runInput, answeredBy } from "./aoe-afk-answer.mjs";

test("decide maps bridge replies and never throws", async () => {
  const ok = createAnswerClient({ request: async p => ({ answer_id: "a", menu: { kind: "choice" }, decision: { decision: "answer", choice: "1" } }) });
  assert.equal((await ok.decide({ run: {}, menu: {} })).decision.choice, "1");
  const broken = createAnswerClient({ request: async () => { throw Error("bridge gone"); } });
  assert.deepEqual((await broken.decide({ run: {}, menu: { q: 1 } })).decision, { decision: "stop", reason: "engine_unavailable", checks: [] });
  const slow = createAnswerClient({ request: () => new Promise(() => {}) }, { timeoutMs: 20 });
  assert.equal((await slow.decide({ run: {}, menu: {} })).decision.reason, "engine_unavailable");
});

test("decide sends exactly one of menu or text", async () => {
  const sent = [];
  const c = createAnswerClient({ request: async p => { sent.push(p); return { decision: { decision: "stop", reason: "x" } }; } });
  await c.decide({ run: { period: "p" }, text: { source: "agent_text", text: "t" } });
  assert.deepEqual(Object.keys(sent[0]).sort(), ["op", "run", "text"]);
  assert.equal(sent[0].op, "decide");
});

test("parseOnArgs handles policy, repeated rules and positional limits", () => {
  assert.deepEqual(parseOnArgs(["--policy", "yolo", "120", "8"]), { minutes: "120", requests: "8", policy: "yolo", rules: [] });
  assert.deepEqual(parseOnArgs(["--rule", "id='a', answer='stop'", "--rule", "id='b', answer='deny'"]).rules.length, 2);
  assert.ok(parseOnArgs(["--policy", "review"]).error);
  assert.ok(parseOnArgs(["1", "2", "3"]).error);
});

test("runInput and answeredBy", () => {
  const s = { id: "p", task: "t", policy: "rules", rules: [], requests_used: 1, request_limit: 4 };
  assert.deepEqual(runInput(s, "/r"), { period: "p", item: null, goal: "t", policy: "rules", rules: [], cwd: "/r", requests_used: 1, request_limit: 4 });
  assert.equal(answeredBy("yolo"), "afk/yolo");
});
```

The `/afk-work` handler tokenises with `args.trim().split(/\s+/)`, which breaks quoted rules. Replace it with a small quote-aware splitter in `aoe-afk-answer.mjs` (`export function splitArgs(s)`, honouring single and double quotes) and test it:

```js
test("splitArgs keeps quoted rule bodies whole", () => {
  assert.deepEqual(splitArgs(`on --rule "id='a', answer='stop'" 60 4`), ["on", "--rule", "id='a', answer='stop'", "60", "4"]);
});
```

In `aoe-afk-work-period.test.mjs`, using the existing `fixture()` (its stub bridge records `packets`; extend the stub so `op: "decide"` returns a configurable reply and `op: "rules_check"` returns `{ valid: true, count: 1, sources: ["inline rule 1"], format_warnings: [] }`, or `{ valid: false, error, format_warnings: [] }` after `rulesCheckError(error)`):

```js
test("aoe_afk_answer returns the engine decision labelled afk/<policy> and logs delivery", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "a1", menu: {}, decision: { decision: "answer", choice: "2", rule: "r", reason: "ok", warnings: [], checks: [] } });
  await f.native("on --policy rules 120 8"); f.begin(); await f.ownedContext();
  const out = await f.answer({ question: "Which?", options: [{ key: "1", label: "A" }, { key: "2", label: "B" }], proposal: "2", reason: "B fits" });
  assert.deepEqual(out.details, { answered_by: "afk/rules", choice: "2", rule: "r", reason: "ok" });
  assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "a1" && p.delivered === "ok"));
});

test("a stopped aoe_afk_answer ends the period as menu_stopped", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "a2", menu: {}, decision: { decision: "stop", reason: "no_rule", checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const out = await f.answer({ question: "Q?", options: [{ key: "1", label: "Yes" }], proposal: null, reason: "" });
  assert.deepEqual(out.details, { stopped: "no_rule" });
  assert.equal(f.snapshots().at(-1).reason, "menu_stopped");
});

test("consent shows the yolo warning and status counts answers", async () => {
  const f = await fixture();
  let message;
  f.confirm(true, undefined, m => message = m);   // extend confirm() to capture the dialog message
  await f.native("on --policy yolo 120 8");
  assert.match(message, /YOLO: AFK will answer every menu, including permission prompts\. Scope checks only warn\./);
  assert.match(message, /policy: yolo/);
});

test("invalid rules refuse the start under rules", async () => {
  const f = await fixture();
  f.rulesCheckError("afk-rules.toml: invalid rules file");
  await f.native("on 120 8");
  assert.ok(f.notices.some(n => n.includes("afk-rules.toml")));
  assert.equal(f.snapshots().length, 0);
});
```

Add `answer`, `decideReply`, `rulesCheckError` to the fixture. `answer(input)` calls `tools.get("aoe_afk_answer").execute("id", input, ctx.signal, undefined, ctx)`. Extend the existing `confirm(value, hook)` to take a third `onMessage` callback that receives the dialog message.

- [ ] **Step 2: Run to verify failure**

Run: `node --test assets/session/aoe-afk-answer.test.mjs assets/session/aoe-afk-work-period.test.mjs`
Expected: the new tests fail.

- [ ] **Step 3: Implement `aoe-afk-answer.mjs`**

```js
export const ANSWER_TOOL = "aoe_afk_answer";
export const DECIDE_TIMEOUT_MS = 5000;
const STOP = reason => ({ decision: "stop", reason, checks: [] });

export function answerParameters(Type) {
  return Type.Object({
    question: Type.String({ minLength: 1, maxLength: 2048 }),
    options: Type.Array(Type.Object({ key: Type.String({ minLength: 1, maxLength: 64 }), label: Type.String({ minLength: 1, maxLength: 256 }) }, { additionalProperties: false }), { minItems: 1, maxItems: 32 }),
    proposal: Type.Union([Type.Null(), Type.String({ minLength: 1, maxLength: 64 })], { description: "Key of the option you would choose, or null." }),
    reason: Type.String({ maxLength: 1024 }),
  }, { additionalProperties: false });
}

const withTimeout = (promise, ms) => {
  let timer;
  return Promise.race([promise, new Promise((_, no) => { timer = setTimeout(() => no(Error("timeout")), ms); })]).finally(() => clearTimeout(timer));
};

export function createAnswerClient(bridge, { timeoutMs = DECIDE_TIMEOUT_MS } = {}) {
  return {
    async decide({ run, menu, text, proposal }) {
      try {
        const packet = { op: "decide", run, ...(menu ? { menu } : {}), ...(text ? { text } : {}), ...(proposal ? { proposal } : {}) };
        const reply = await withTimeout(bridge.request(packet), timeoutMs);
        if (!reply?.decision || !["answer", "stop"].includes(reply.decision.decision)) throw Error("malformed decision");
        return reply;
      } catch { return { menu: menu ?? null, decision: STOP("engine_unavailable") }; }
    },
    async delivered(period, answer_id, status) {
      if (!answer_id) return;
      try { await withTimeout(bridge.request({ op: "answer_delivered", period, answer_id, delivered: status }), timeoutMs); } catch { /* the log keeps "pending"; the review shows it */ }
    },
    async recognise(source, text) {
      try { return (await withTimeout(bridge.request({ op: "recognise", source, text }), timeoutMs))?.menu ?? null; } catch { return null; }
    },
    async rulesCheck(run) { return withTimeout(bridge.request({ op: "rules_check", run }), timeoutMs); },
  };
}

export const answeredBy = policy => `afk/${policy}`;

export const runInput = (s, cwd) => ({ period: s.id, item: null, goal: s.task, policy: s.policy, rules: s.rules, cwd,
  requests_used: s.requests_used, request_limit: s.request_limit });

export function splitArgs(s) {
  const out = []; let cur = "", quote = null, any = false;
  for (const ch of s.trim()) {
    if (quote) { if (ch === quote) quote = null; else cur += ch; continue; }
    if (ch === "'" || ch === '"') { quote = ch; any = true; continue; }
    if (/\s/.test(ch)) { if (cur || any) { out.push(cur); cur = ""; any = false; } continue; }
    cur += ch;
  }
  if (cur || any) out.push(cur);
  return out;
}

export function parseOnArgs(tokens) {
  const out = { policy: "rules", rules: [] }, positional = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i] === "--policy") { out.policy = tokens[++i]; continue; }
    if (tokens[i] === "--rule") { out.rules.push(tokens[++i] ?? ""); continue; }
    positional.push(tokens[i]);
  }
  if (!["rules", "yolo"].includes(out.policy)) return { ...out, error: "Use --policy rules or --policy yolo" };
  if (out.rules.length > 8 || out.rules.some(r => !r || Buffer.byteLength(r) > 1024)) return { ...out, error: "At most 8 --rule values of up to 1024 bytes" };
  if (positional.length > 2) return { ...out, error: "Usage: /afk-work on [--policy rules|yolo] [--rule '<toml>']… [minutes] [requests]" };
  return { ...out, ...(positional[0] ? { minutes: positional[0] } : {}), ...(positional[1] ? { requests: positional[1] } : {}) };
}
```

Bridge frame size: in `decide`, truncate `menu.raw` and `text.text` to 4096 bytes before sending (`Buffer.from(x).subarray(0, 4096).toString()`; drop a broken trailing code unit).

- [ ] **Step 4: Wire it into `aoe-afk-work-period.mjs`**

1. Imports: `import { ANSWER_TOOL, createAnswerClient, runInput, answeredBy, parseOnArgs, splitArgs } from "./aoe-afk-answer.mjs";`
2. `workPeriodRuntime(pi, { version, parameters, answerParameters: answerParams, … })`: take a second parameter schema for the answer tool (the runtime file builds it with `answerParameters(Type)`).
3. In `build()`: `w.answers = { rules: 0, yolo: 0, stops: 0 }` and `w.client = r ? createAnswerClient(r.bridge) : undefined`. In `attach(r)`, after `build`, set `current.client = createAnswerClient(r.bridge)`.
4. Add a sentence to REMINDER: `Never end a turn with a menu for the human. Call aoe_afk_answer with the question, options and your proposed option instead.`
5. Register the tool:

```js
  pi.registerTool({ name: ANSWER_TOOL, label: "AFK answer", description: "Resolve a choice you would otherwise ask the human. The AFK engine decides under the run policy; the result says who answered.",
    parameters: answerParams, executionMode: "sequential",
    async execute(_id, input, _signal, _update, ctx) {
      const w = current;
      if (!owned(w, ctx)) throw Error("No owned AFK work-period request");
      const s = w.period.snapshot();
      const menu = { source: "agent_text", format: "tool.aoe_afk_answer", kind: "choice", question: input.question,
        options: input.options.map(o => ({ key: o.key, label: o.label })), raw: JSON.stringify(input).slice(0, 4096) };
      const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu, proposal: { key: input.proposal ?? undefined, reason: input.reason } });
      if (decision.decision !== "answer") {
        w.answers.stops++; stop(w, "menu_stopped", "blocked");
        return result({ stopped: decision.reason });
      }
      w.answers[s.policy]++;
      const value = { answered_by: answeredBy(s.policy), choice: decision.choice, rule: decision.rule ?? null, reason: decision.reason };
      void w.client.delivered(s.id, answer_id, "ok");
      if (input.proposal) w.lastAnswerAt = Date.now();
      return result(value);
    },
  });
```

`proposal: { key: undefined }` serialises without `key`, which matches `Proposal`'s optional field. If the tool's deny-unknown check complains about `null`, send `{ key: input.proposal, reason }` only when `input.proposal` is a string.

Use `rule: decision.rule ?? null` so the result shape is fixed. The test expects `rule: "r"`.

6. `tool_call` handler: next to the existing `WORK_CHECKPOINT_TOOL` early return, add `if (event.toolName === ANSWER_TOOL) return;` (an owned period admits it without counting a tool), and block it when not owned: `if (event.toolName === ANSWER_TOOL && !owned(w, ctx)) return { block: true, reason: "No owned AFK work period" };`.
7. The `/afk-work` handler: tokenise with `splitArgs(args)`. For `on`, call `parseOnArgs(tokens)`. On `error`, notify and return. Take `minutes`/`requests` from the parsed result (prompting as today when absent). Before the confirm dialog, call:

```js
      let ruleInfo = { count: 0, sources: [], format_warnings: [] };
      if (parsed.policy === "rules") {
        try { ruleInfo = await w.client.rulesCheck({ period: randomUUID(), item: null, goal: "consent", policy: "rules", rules: parsed.rules, cwd: ctx.cwd, requests_used: 0, request_limit: 1 }); }
        catch { notify("AFK rules could not be checked (engine unavailable); no authority created", "warning"); return; }
        if (!ruleInfo.valid) { notify(`AFK rules invalid; no authority created: ${ruleInfo.error}`, "warning"); return; }
      }
```

   Extend the confirm message with:

```js
`\npolicy: ${parsed.policy}; rules: ${ruleInfo.count}${ruleInfo.sources.length ? ` (${ruleInfo.sources.join(", ")})` : ""}`
+ (parsed.policy === "yolo" ? "\nYOLO: AFK will answer every menu, including permission prompts. Scope checks only warn." : "")
+ (ruleInfo.format_warnings.length ? `\nSkipped menu formats: ${ruleInfo.format_warnings.length}` : "")
```

   Pass `policy: parsed.policy, rules: parsed.rules` to `w.period.start(...)` and reset `w.answers = { rules: 0, yolo: 0, stops: 0 }` there.
8. `projection(w)`: add `policy: s.policy, answers: w.answers`. The status notification for `/afk-work status` appends `` `answers: ${a.rules + a.yolo} (rules ${a.rules}, yolo ${a.yolo}), stops: ${a.stops}` `` (spec: `answers: N (rules M, yolo K), stops: S`).

- [ ] **Step 5: Wire the runtime and embed the asset**

In `aoe-afk-runtime.mjs`, next to line 423:

```js
import { answerParameters } from "./aoe-afk-answer.mjs";
// …
  const workPeriod = workPeriodRuntime(pi, { version: VERSION, parameters: workCheckpointParameters(Type), answerParameters: answerParameters(Type) });
```

In `src/session/afk.rs`, add to the asset list next to `aoe-afk-work-period.mjs`:

```rust
        (
            "aoe-afk-answer.mjs",
            include_bytes!("../../assets/session/aoe-afk-answer.mjs").as_slice(),
        ),
```

- [ ] **Step 6: Run tests**

Run: `node --test assets/session/*.test.mjs`
Expected: all pass. Then commit WIP and run `env -u CLAUDE_CONFIG_DIR aoe-gate` for the Rust embedding change.

- [ ] **Step 7: Commit**

```bash
git add assets/session src/session/afk.rs
git -c user.email=jw@crasiak.net commit -m "feat(afk): aoe_afk_answer tool, policy consent and answer counts (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Text-menu fallback at settle, and `ask_user` routing

**Files:**
- Modify: `assets/session/aoe-afk-work-period.mjs` (`agent_before_settle`, `tool_call`)
- Test: `assets/session/aoe-afk-work-period.test.mjs`

**Interfaces:**
- Consumes: `period.nudge(text)` (Task 7); `w.client.recognise`, `w.client.decide`, `runInput`, `answeredBy` (Task 8)
- Produces:
  - `MENU_NUDGE = "You ended with a menu. Resolve it with aoe_afk_answer."`
  - `askUserMenu(input)` (exported from `aoe-afk-answer.mjs`) → `MenuInput | null`. Only a single `question` with `options` and no `allowMultiple` maps; a `questions` batch or `allowMultiple: true` returns `null`. Option keys are `"1".."n"`, labels are option titles, and `free_text` is `allowFreeform !== false`.

- [ ] **Step 1: Write failing tests**

```js
test("a turn that ends with a recognised text menu gets one corrective continuation", async () => {
  const f = await fixture();
  f.recogniseReply({ kind: "choice" });                          // any non-null menu
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const b = { ...f.boundary(), context: { pendingMessages: [], contextMessages: [...f.context(), { role: "assistant", content: [{ type: "text", text: "[A]pply / [S]kip" }] }] } };
  const first = await f.emit("agent_before_settle", b);
  assert.equal(first.continue, true);
  assert.match(first.entries.at(-1).content, /Resolve it with aoe_afk_answer/);
  await f.ownedContext([continuation(first.entries.at(-1))]);
  const second = await f.emit("agent_before_settle", b);
  assert.equal(second, undefined);
  assert.equal(f.snapshots().at(-1).reason, "missing_checkpoint");
});

test("no recognised menu at settle stops as missing_checkpoint", async () => {
  const f = await fixture();
  f.recogniseReply(null);
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  await f.emit("agent_before_settle", f.boundary());
  assert.equal(f.snapshots().at(-1).reason, "missing_checkpoint");
});

test("ask_user is answered by the engine with an afk-labelled blocked result", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "q1", menu: {}, decision: { decision: "answer", choice: "2", rule: "r", reason: "ok", warnings: [], checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const out = await f.call("ask_user", "c1", { question: "Which DB?", options: [{ title: "SQLite" }, { title: "Postgres" }] });
  assert.equal(out.block, true);
  const reason = JSON.parse(out.reason);
  assert.deepEqual(reason, { answered_by: "afk/rules", choice: "2", label: "Postgres", rule: "r", reason: "ok" });
  assert.equal(f.snapshots().at(-1).status, "owned");
});

test("ask_user stop keeps today's no_human_answer behaviour", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "q2", menu: {}, decision: { decision: "stop", reason: "no_rule", checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const out = await f.call("ask_user", "c2", { question: "Q?", options: [{ title: "A" }] });
  assert.match(out.reason, /no_human_answer/);
  assert.equal(f.snapshots().at(-1).reason, "human_required");
});

test("ask_user batches are not mapped", () => {
  assert.equal(askUserMenu({ questions: [{ question: "a" }, { question: "b" }] }), null);
  assert.equal(askUserMenu({ question: "a", options: [{ title: "x" }], allowMultiple: true }), null);
});
```

Add `recogniseReply` to the fixture stub (`op: "recognise"` returns `{ menu }`).

- [ ] **Step 2: Run to verify failure.** `node --test assets/session/aoe-afk-work-period.test.mjs assets/session/aoe-afk-answer.test.mjs`

- [ ] **Step 3: Implement**

`askUserMenu` in `aoe-afk-answer.mjs`:

```js
export function askUserMenu(input) {
  if (!input || typeof input.question !== "string" || input.questions || input.allowMultiple || !Array.isArray(input.options) || !input.options.length) return null;
  return { source: "ask_user", format: "tool.ask_user", kind: "choice", question: input.question, subject: input.context ?? null,
    options: input.options.slice(0, 32).map((o, i) => ({ key: String(i + 1), label: String(o.title ?? "").slice(0, 256) })),
    free_text: input.allowFreeform !== false, raw: JSON.stringify(input).slice(0, 4096) };
}
```

`tool_call` (make the handler `async`): in the owned branch, before `w.period.allowTool(event.toolName)`:

```js
    if (event.toolName === "ask_user") {
      const menu = askUserMenu(event.input), s = w.period.snapshot();
      if (menu) {
        const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu });
        if (decision.decision === "answer") {
          w.answers[s.policy]++;
          void w.client.delivered(s.id, answer_id, "ok");
          const label = menu.options.find(o => o.key === decision.choice)?.label ?? null;
          return { block: true, reason: JSON.stringify({ answered_by: answeredBy(s.policy), choice: decision.choice, label, rule: decision.rule ?? null, reason: decision.reason }) };
        }
        w.answers.stops++;
      }
      // Fall through: allowTool("ask_user") stops human_required, and the block reason stays no_human_answer.
    }
```

Then check that `owned(w, ctx)` is still true after the `await`, since a concurrent abort can end the period. If it isn't, take the existing `operation_changed` path.

`agent_before_settle`: inside the `try`, after `w.period.observe()` and the pending/initial branch, and before `const next = w.period.settle();`:

```js
      const s = w.period.snapshot();
      if (s.status === "owned" && s.checkpoint?.status !== "unfinished" && s.menu_nudges === 0) {
        const last = [...event.context.contextMessages].reverse().find(m => m.role === "assistant");
        const textValue = (last?.content ?? []).filter(b => b.type === "text").map(b => b.text).join("\n");
        if (textValue && await w.client.recognise("agent_text", textValue.slice(-4096))) {
          const nudge = w.period.nudge(MENU_NUDGE);
          if (nudge.continue) {
            const content = `[AoE bounded work continuation] ${MENU_NUDGE} No new scope, privilege or allowance. ${REMINDER}`;
            w.expected = { ...nudge, before: JSON.stringify(clean(event.context.contextMessages).filter(m => m.role !== "system")), content };
            return { entries: [...event.entries, { type: "custom_message", customType: CONTINUATION, content, display: true,
              details: { period: nudge.id, request: nudge.request, admission: nudge.admission } }], continue: true };
          }
        }
      }
```

The handler is already `async`-capable (Pi awaits it). Make the arrow function `async`.

- [ ] **Step 4: Run all Node tests.** `node --test assets/session/*.test.mjs`. Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add assets/session
git -c user.email=jw@crasiak.net commit -m "feat(afk): answer ask_user and nudge text menus once per period (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: CLI prompts inside bash

**Files:**
- Modify: `assets/session/aoe-afk-answer.mjs` (prompt heuristics), `assets/session/aoe-afk-work-period.mjs` (`tool_call` bash timeout, `tool_result` annotation, checkpoint evidence rule, REMINDER)
- Test: `assets/session/aoe-afk-answer.test.mjs`, `assets/session/aoe-afk-work-period.test.mjs`

**Interfaces:**
- Consumes: `w.client.recognise` (Task 8); the existing `tool_result` evidence annotation
- Produces:
  - `export const BASH_DEFAULT_TIMEOUT_S = 600`
  - `export function promptLike(text)` → `string | null`: the trailing non-empty lines (last 12) if the final line matches `/\?\s*$|\((?:y\/N|Y\/n|y\/n)\)|\[(?:y\/N|Y\/n)\]|Password:\s*$|›\s*$/`, else `null`
  - `w.pendingPrompts: Set<toolCallId>`, cleared when an `aoe_afk_answer` call is answered
  - Bash tool results whose tail a format recognises get an extra text block `{"aoe_afk_prompt_seen": <Menu>}`
  - A prompt-like tail that no format recognises stops the period as `blocked: unrecognised_menu`. The extension calls `decide` with `text`, so the bridge saves the sample (spec: unrecognised waits always stop).

- [ ] **Step 1: Write failing tests**

```js
test("promptLike flags prompt endings only", () => {
  assert.ok(promptLike("Installing…\nOk to proceed? (y/N)"));
  assert.ok(promptLike("Password:"));
  assert.equal(promptLike("All tests passed.\n"), null);
});
```

```js
test("bash without timeout gets the 600 s default; explicit timeout is kept", async () => {
  const f = await fixture();
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const a = { command: "ls" }, b = { command: "cargo build", timeout: 1800 };
  await f.call("bash", "b1", a); await f.call("bash", "b2", b);
  assert.equal(a.timeout, 600);
  assert.equal(b.timeout, 1800);
});

test("a bash result ending in a prompt is annotated and not progress until answered", async () => {
  const f = await fixture();
  f.recogniseReply({ kind: "confirm", question: "Ok to proceed? (y/N)" });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const r = await f.callWithOutput("bash", "b3", { command: "npx create-vite" }, "Need to install\nOk to proceed? (y/N)");
  assert.ok(r.content.some(b => b.type === "text" && b.text.includes("aoe_afk_prompt_seen")));
  const out = await f.checkpoint("unfinished", ["b3"]);
  assert.equal(out.details.status, "blocked");          // evidence ignored → no_progress
  assert.equal(out.details.reason, "no_progress");
});

test("a prompt-like bash tail that no format recognises stops as unrecognised_menu and saves the sample", async () => {
  const f = await fixture();
  f.recogniseReply(null);
  f.decideReply({ answer_id: "u1", menu: null, decision: { decision: "stop", reason: "unrecognised_menu", checks: [] }, sample: "p-unrecognised-1.txt" });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  await f.callWithOutput("bash", "b5", { command: "x" }, "Enter the magic word?");
  assert.ok(f.packets.some(p => p.op === "decide" && p.text?.source === "bash_prompt"));
  assert.equal(f.snapshots().at(-1).reason, "unrecognised_menu");
});

test("an answered aoe_afk_answer releases the prompt evidence", async () => {
  const f = await fixture();
  f.recogniseReply({ kind: "confirm" });
  f.decideReply({ answer_id: "x", menu: {}, decision: { decision: "answer", choice: "y", reason: "", warnings: [], checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  await f.callWithOutput("bash", "b4", { command: "x" }, "Continue? (y/N)");
  await f.answer({ question: "Continue?", options: [{ key: "y", label: "Yes" }, { key: "n", label: "No" }], proposal: "y", reason: "" });
  const out = await f.checkpoint("unfinished", ["b4"]);
  assert.equal(out.details.status, "owned");
});
```

Add `callWithOutput(name, id, input, text)` to the fixture: same as `call`, with the result `content` set to `[{ type: "text", text }]`.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

`aoe-afk-answer.mjs`:

```js
export const BASH_DEFAULT_TIMEOUT_S = 600;
const PROMPT_END = /\?\s*$|\((?:y\/N|Y\/n|y\/n)\)|\[(?:y\/N|Y\/n)\]|Password:\s*$|›\s*$/;
export function promptLike(text) {
  const lines = String(text ?? "").split("\n").map(l => l.trimEnd()).filter(l => l.trim());
  if (!lines.length || !PROMPT_END.test(lines.at(-1))) return null;
  return lines.slice(-12).join("\n");
}
```

`tool_call`, in the owned branch, right before `w.calls.set(...)` records the input:

```js
          if (event.toolName === "bash" && event.input && event.input.timeout === undefined) event.input.timeout = BASH_DEFAULT_TIMEOUT_S;
```

This must run before `structuredClone(event.input)` so the recorded evidence matches what ran.

`tool_result`: after the evidence annotation is built (where `content` gets the `aoe_afk_work_evidence_ref` block), for `call.name === "bash"`:

```js
      if (call.name === "bash") {
        const tail = promptLike(event.content.filter(b => b.type === "text").map(b => b.text).join("\n"));
        if (tail) {
          const menu = await w.client.recognise("bash_prompt", tail);
          if (!menu) {
            await w.client.decide({ run: runInput(w.period.snapshot(), ctx.cwd), text: { source: "bash_prompt", text: tail } }); // saves the sample
            w.answers.stops++; stop(w, "unrecognised_menu", "blocked"); abort(w, ctx);
            return;
          }
          w.pendingPrompts.add(event.toolCallId);
          content.push({ type: "text", text: JSON.stringify({ aoe_afk_prompt_seen: menu }) });
        }
      }
```

The handler becomes `async`. Compute `annotatedFingerprint` after this push, from the final `content`, so the replay check still holds.

Checkpoint tool `execute`: drop pending prompt ids from the cited evidence before calling the state machine:

```js
      const ignored = input.evidence.filter(id => w.pendingPrompts.has(id));
      const cleaned = { ...input, evidence: input.evidence.filter(id => !w.pendingPrompts.has(id)) };
      const s = w.period.checkpoint(cleaned, …);
      return result({ …existing, ...(ignored.length ? { ignored_evidence: ignored, ignored_reason: "prompt_seen_not_answered" } : {}) });
```

The `aoe_afk_answer` execute (Task 8): on an `answer` decision, `w.pendingPrompts.clear()`. Initialise `w.pendingPrompts = new Set()` in `build()` and clear it at period start.

REMINDER: append `Commands must not wait for input. Pass answers explicitly (flags, yes |, printf … |). If a command needs a choice, decide it with aoe_afk_answer first. Bash calls default to a 600 s timeout; pass a longer timeout for long builds or test suites.`

- [ ] **Step 4: Run all Node tests.** `node --test assets/session/*.test.mjs`. Expected: all pass, including the existing evidence and replay tests.

- [ ] **Step 5: Commit**

```bash
git add assets/session
git -c user.email=jw@crasiak.net commit -m "feat(afk): bash prompt detection, evidence hold and default timeout (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Pi fork `ui_prompt_respond` hook

**Precondition (resolved 2026-10-31):** the Pi runtime AoE runs is committed as `c8348c3d` on our fork `crasiak/pi`, branch `feature/CRA-859-ledger-afk-runtime-v1.2` (worktree `~/code/pi-worktrees/CRA-859-ledger-afk-runtime-20261027`, remote `crasiak`; `origin` there is upstream `earendil-works/pi`, never push to it). Build on that branch. Promoting a new Pi core for Ledger (`ledger runtime promote|verify`, a new `pi-core-afk` store path) and requalifying the preview still need the user's go-ahead. Everything in Tasks 2–10 works without this hook: Pi dialogs keep stopping as `human_prompt`.

**Files:**
- Modify: `packages/coding-agent/src/core/extensions/runner.ts` (`wrapUIPromptContext`, new `respondOr`, `emitUIPromptRespond`)
- Modify: `packages/coding-agent/src/core/extensions/types.ts` (`UIPromptRespondEvent`, `UIPromptRespondResult`, `answered_by` on `UIPromptStartEvent`/`UIPromptEndEvent`, `on("ui_prompt_respond", …)` overload)
- Modify: `packages/coding-agent/docs/extensions.md`
- Test: `packages/coding-agent/test/ui-prompt-respond.test.ts`

**Interfaces:**
- Produces:
  - `interface UIPromptRespondEvent { type: "ui_prompt_respond"; kind: "select" | "confirm" | "input"; title?: string; message?: string; options?: string[]; owner?: UIPromptOwner }`
  - `interface UIPromptRespondResult { value: string | boolean; answered_by: string }`
  - `UIPromptStartEvent.answered_by?: string`, `UIPromptEndEvent.answered_by?: string`
  - Semantics: with no `ui_prompt_respond` handler, behaviour is byte-for-byte unchanged (same synchronous start emission). With handlers, the runner asks them in order (first valid result wins, 5 s total bound). A valid `value` (`select`: one of `options`; `confirm`: boolean; `input`: string ≤ 4096) resolves the call without the native UI and emits `ui_prompt_start` and `ui_prompt_end` with `answered_by`. Anything else (undefined, invalid value, throw, timeout) falls through to the native dialog. `editor` and `custom` never consult responders.

- [ ] **Step 1: Write the failing tests** (copy `harness()` from `test/ui-prompt-ownership.test.ts`, adding an optional responder factory)

```ts
describe("ui_prompt_respond", () => {
	it.each([
		["select", "choice", (ui: ExtensionUIContext) => ui.select("Pick", ["choice", "other"])],
		["confirm", true, (ui: ExtensionUIContext) => ui.confirm("Sure?", "msg")],
		["input", "typed", (ui: ExtensionUIContext) => ui.input("Name", "ph")],
	] as const)("answers %s without the native dialog", async (kind, value, call) => {
		const h = await harness({ respond: () => ({ value, answered_by: "afk/rules" }) });
		expect(await call(h.ui)).toEqual(value);
		expect(h.raw[kind]).not.toHaveBeenCalled();
		await Promise.resolve(); await Promise.resolve();
		expect(h.events.map((e) => [e.type, e.answered_by])).toEqual([["ui_prompt_start", "afk/rules"], ["ui_prompt_end", "afk/rules"]]);
	});

	it.each([
		["undefined", () => undefined],
		["throw", () => { throw new Error("boom"); }],
		["invalid select value", () => ({ value: "not-an-option", answered_by: "afk/rules" })],
	])("falls through to the native dialog on %s", async (_name, respond) => {
		const h = await harness({ respond });
		expect(await h.ui.select("Pick", ["choice"])).toBe("choice");
		expect(h.raw.select).toHaveBeenCalledOnce();
	});

	it("falls through after the 5 s bound", async () => {
		vi.useFakeTimers();
		const h = await harness({ respond: () => new Promise(() => {}) });
		const pending = h.ui.confirm("Sure?", "msg");
		await vi.advanceTimersByTimeAsync(5001);
		expect(await pending).toBe(true);
		expect(h.raw.confirm).toHaveBeenCalledOnce();
		vi.useRealTimers();
	});

	it("never consults responders for editor or custom", async () => {
		const respond = vi.fn(() => ({ value: "x", answered_by: "afk/yolo" }));
		const h = await harness({ respond });
		await h.ui.editor("Edit", "pre");
		h.custom.resolve("c");
		await h.ui.custom(() => ({ render: () => [], invalidate() {} }));
		expect(respond).not.toHaveBeenCalled();
	});

	it("passes title, message, options and owner to the responder", async () => {
		const respond = vi.fn(() => undefined);
		const h = await harness({ respond });
		await h.ui.confirm("Sure?", "details", { owner });
		expect(respond).toHaveBeenCalledWith(expect.objectContaining({ type: "ui_prompt_respond", kind: "confirm", title: "Sure?", message: "details", owner }), expect.anything());
	});

	it("leaves behaviour unchanged with no responder", async () => {
		const h = await harness();
		expect(await h.ui.select("Pick", ["choice"])).toBe("choice");
		expect(h.events.map((e) => e.type)).toEqual(["ui_prompt_start", "ui_prompt_end"]);
	});
});
```

- [ ] **Step 2: Run to verify failure**

Run (in the Pi tree): `cd packages/coding-agent && npx vitest run test/ui-prompt-respond.test.ts`
Expected: FAIL.

- [ ] **Step 3: Implement**

`types.ts`: add the two interfaces and the overload `on(event: "ui_prompt_respond", handler: ExtensionHandler<UIPromptRespondEvent, UIPromptRespondResult | undefined>): () => void;`, add `answered_by?: string` to both prompt events, and add `UIPromptRespondEvent` to the event union next to `UIPromptStartEvent`.

`runner.ts`:

```ts
const RESPOND_TIMEOUT_MS = 5000;

	private hasHandlers(type: string): boolean {
		return snapshotEventHandlers(this.extensions, type).some(({ handlers }) => handlers.length > 0);
	}

	private async emitUIPromptRespond(event: UIPromptRespondEvent): Promise<UIPromptRespondResult | undefined> {
		const ctx = this.createContext();
		const ask = (async () => {
			for (const { ext, handlers } of snapshotEventHandlers(this.extensions, event.type)) {
				for (const handler of handlers) {
					try {
						const result = (await handler(event, ctx)) as UIPromptRespondResult | undefined;
						if (result && validResponse(event, result)) return result;
					} catch (err) {
						this.emitError({ extensionPath: ext.path, event: event.type, error: err instanceof Error ? err.message : String(err) });
					}
				}
			}
			return undefined;
		})();
		let timer: ReturnType<typeof setTimeout> | undefined;
		try {
			return await Promise.race([ask, new Promise<undefined>((done) => { timer = setTimeout(() => done(undefined), RESPOND_TIMEOUT_MS); })]);
		} finally { clearTimeout(timer); }
	}

	private respondOr<T>(event: UIPromptRespondEvent, native: () => Promise<T>): Promise<T> {
		if (this.uiPromptDepth > 0 || !this.hasHandlers("ui_prompt_respond")) return native();
		return this.emitUIPromptRespond(event).then((answer) => {
			if (!answer) return native();
			const base = { reason: "ui_prompt", kind: event.kind, ...(event.title ? { title: event.title } : {}), ...(event.owner ? { owner: event.owner } : {}), answered_by: answer.answered_by } as const;
			this.emitUIPromptEvent({ type: "ui_prompt_start", ...base });
			this.emitUIPromptEvent({ type: "ui_prompt_end", ...base });
			return answer.value as T;
		});
	}
```

```ts
function validResponse(event: UIPromptRespondEvent, r: UIPromptRespondResult): boolean {
	if (typeof r.answered_by !== "string" || !r.answered_by || r.answered_by.length > 64) return false;
	if (event.kind === "select") return typeof r.value === "string" && !!event.options?.includes(r.value);
	if (event.kind === "confirm") return typeof r.value === "boolean";
	return typeof r.value === "string" && r.value.length <= 4096;
}
```

In `wrapUIPromptContext`:

```ts
			select: (title, options, opts) =>
				this.respondOr({ type: "ui_prompt_respond", kind: "select", title, options: [...options], ...ownerOf(opts) },
					() => this.withUIPrompt("select", title, opts, () => ui.select(title, options, opts))),
			confirm: (title, message, opts) =>
				this.respondOr({ type: "ui_prompt_respond", kind: "confirm", title, message, ...ownerOf(opts) },
					() => this.withUIPrompt("confirm", title, opts, () => ui.confirm(title, message, opts))),
			input: (title, placeholder, opts) =>
				this.respondOr({ type: "ui_prompt_respond", kind: "input", title, ...ownerOf(opts) },
					() => this.withUIPrompt("input", title, opts, () => ui.input(title, placeholder, opts))),
```

`ownerOf(opts)` returns `{ owner: copyUIPromptOwner(opts) }` when there is one, else `{}`. `editor` and `custom` are unchanged. `select` options may be objects in this Pi version; if so, map them to their string values the same way `ui.select` returns them, and test with the real type.

The "no responder" path must call `native()` synchronously, so the existing `ui-prompt-ownership` tests (which depend on synchronous start emission) still pass.

`docs/extensions.md`: add a `ui_prompt_respond` section: when it fires, the result shape, validation, the 5 s bound, the fall-through cases, `answered_by`, and that editor and custom dialogs never consult it.

- [ ] **Step 4: Run the Pi tests**

Run: `cd packages/coding-agent && npx vitest run test/ui-prompt-respond.test.ts test/ui-prompt-ownership.test.ts test/extensions-runner.test.ts`
Expected: all pass. Then run the fork's own check (`npm run check` at the repo root, if it exists).

- [ ] **Step 5: Commit (only with the user's go-ahead from the precondition)**

```bash
git add packages/coding-agent/src/core/extensions packages/coding-agent/test/ui-prompt-respond.test.ts packages/coding-agent/docs/extensions.md
git -c user.email=jw@crasiak.net commit -m "feat(coding-agent): ui_prompt_respond hook with answered_by (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: AFK responder for Pi dialogs

**Files:**
- Modify: `assets/session/aoe-afk-answer.mjs` (`dialogMenu`, `dialogValue`), `assets/session/aoe-afk-work-period.mjs` (`ui_prompt_respond` handler; `ui_prompt_start`/`ui_prompt_end` skip answered prompts)
- Test: `assets/session/aoe-afk-answer.test.mjs`, `assets/session/aoe-afk-work-period.test.mjs`

**Interfaces:**
- Consumes: the Task 11 hook (event shape above); `w.client.decide`, `runInput`, `answeredBy` (Task 8)
- Produces:
  - `dialogMenu(event)` → `MenuInput | null`: `select` → `kind: "choice"` (or `"permission"` when the title starts with a permission phrase: `/^(allow|approve|permit|run|execute)\b|permission/i`), options keyed `"1".."n"` from `event.options`; `confirm` → `kind: "confirm"` (or `"permission"` by the same title test), options `y/Yes` and `n/No`, `subject: event.message`; `input` → `kind: "input"`, `free_text: true`, no options. `editor`/`custom` → `null`.
  - `dialogValue(event, menu, decision)` → the Pi value: `select` → the option label for `decision.choice`; `confirm` → `decision.choice === "y"`; `input` → `decision.text`.
  - The responder answers only while the period is owned, never for owner namespace `aoe-afk-work-consent-v1`, and returns `{ value, answered_by: "afk/<policy>" }` or `undefined`.

- [ ] **Step 1: Write failing tests**

```js
test("dialogMenu maps select, confirm and input, and skips editor/custom", () => {
  assert.equal(dialogMenu({ kind: "select", title: "Pick", options: ["a", "b"] }).options[1].label, "b");
  const c = dialogMenu({ kind: "confirm", title: "Allow bash command?", message: "Bash command: cargo test" });
  assert.equal(c.kind, "permission"); assert.equal(c.subject, "Bash command: cargo test");
  assert.equal(dialogMenu({ kind: "input", title: "Branch name" }).free_text, true);
  assert.equal(dialogMenu({ kind: "editor", title: "x" }), null);
});
```

```js
test("an owned period answers a confirm dialog and does not stop on the answered prompt events", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "d1", menu: {}, decision: { decision: "answer", choice: "y", rule: "r", reason: "", warnings: [], checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  const answer = await f.emit("ui_prompt_respond", { type: "ui_prompt_respond", kind: "confirm", title: "Allow?", message: "Bash command: cargo test" });
  assert.deepEqual(answer, { value: true, answered_by: "afk/rules" });
  await f.emit("ui_prompt_start", { kind: "confirm", answered_by: "afk/rules" });
  await f.emit("ui_prompt_end", { kind: "confirm", answered_by: "afk/rules" });
  assert.equal(f.snapshots().at(-1).status, "owned");
  assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "d1" && p.delivered === "ok"));
});

test("a stop leaves the dialog to the human and the period stops as today", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "d2", menu: {}, decision: { decision: "stop", reason: "no_rule", checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  assert.equal(await f.emit("ui_prompt_respond", { type: "ui_prompt_respond", kind: "confirm", title: "Allow?" }), undefined);
  await f.emit("ui_prompt_start", { kind: "confirm" });
  assert.equal(f.snapshots().at(-1).reason, "human_prompt");
});

test("the consent dialog and unowned periods are never answered", async () => {
  const f = await fixture();
  assert.equal(await f.emit("ui_prompt_respond", { type: "ui_prompt_respond", kind: "confirm", title: "x", owner: { namespace: "aoe-afk-work-consent-v1", id: "i" } }), undefined);
  assert.equal(await f.emit("ui_prompt_respond", { type: "ui_prompt_respond", kind: "confirm", title: "x" }), undefined);
});

test("an answered dialog whose prompt never ends records delivery unknown and stops", async () => {
  const f = await fixture();
  f.decideReply({ answer_id: "d3", menu: {}, decision: { decision: "answer", choice: "y", rule: "r", reason: "", warnings: [], checks: [] } });
  await f.native("on 120 8"); f.begin(); await f.ownedContext();
  await f.emit("ui_prompt_respond", { type: "ui_prompt_respond", kind: "confirm", title: "Allow?" });
  await f.emit("ui_prompt_start", { kind: "confirm", answered_by: "afk/rules" });
  f.advance(3000, 3000); await f.tick();                 // no ui_prompt_end within the delivery window
  assert.equal(f.snapshots().at(-1).reason, "delivery_unknown");
  assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "d3" && p.delivered === "unknown"));
});
```

Implement the delivery window as a 2 s timer started by `ui_prompt_start` with `answered_by`. Add `tick()` to the fixture (`await new Promise(r => setImmediate(r))`). If fake time isn't practical with the fixture's `wall`/`mono`, use a real 2 s timer and `{ timeout: 5000 }` on the test.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement**

`aoe-afk-answer.mjs`:

```js
const PERMISSION = /^(allow|approve|permit|run|execute)\b|permission/i;
export function dialogMenu(e) {
  const kind = e?.kind, title = String(e?.title ?? "").slice(0, 2048) || "(untitled dialog)";
  if (kind === "select" && Array.isArray(e.options) && e.options.length)
    return { source: "pi_dialog", format: "pi.select", kind: PERMISSION.test(title) ? "permission" : "choice", question: title, subject: e.message ?? null,
      options: e.options.slice(0, 32).map((label, i) => ({ key: String(i + 1), label: String(label).slice(0, 256) })), raw: JSON.stringify(e).slice(0, 4096) };
  if (kind === "confirm")
    return { source: "pi_dialog", format: "pi.confirm", kind: PERMISSION.test(title) ? "permission" : "confirm", question: title, subject: e.message ?? null,
      options: [{ key: "y", label: "Yes" }, { key: "n", label: "No" }], raw: JSON.stringify(e).slice(0, 4096) };
  if (kind === "input")
    return { source: "pi_dialog", format: "pi.input", kind: "input", question: title, free_text: true, options: [], raw: JSON.stringify(e).slice(0, 4096) };
  return null;
}
export function dialogValue(e, menu, d) {
  if (e.kind === "select") return menu.options.find(o => o.key === d.choice)?.label;
  if (e.kind === "confirm") return d.choice === "y";
  return d.text ?? undefined;
}
```

`aoe-afk-work-period.mjs`:

```js
  pi.on("ui_prompt_respond", async (event, ctx) => {
    const w = current;
    if (!owned(w, ctx) || event.owner?.namespace === "aoe-afk-work-consent-v1" || w.pendingConsent) return;
    const menu = dialogMenu(event);
    if (!menu) return;
    const s = w.period.snapshot();
    const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu });
    if (decision.decision !== "answer" || !owned(w, ctx)) { w.answers.stops++; return; }
    const value = dialogValue(event, menu, decision);
    if (value === undefined) { w.answers.stops++; void w.client.delivered(s.id, answer_id, "failed"); return; }
    w.answers[s.policy]++;
    w.answering = { answer_id, period: s.id, by: answeredBy(s.policy), timer: undefined };
    return { value, answered_by: answeredBy(s.policy) };
  });
```

`ui_prompt_start`: at the top,

```js
    if (event.answered_by && current?.answering?.by === event.answered_by) {
      const a = current.answering;
      a.timer = setTimeout(() => {
        if (current?.answering !== a) return;
        current.answering = undefined;
        void current.client.delivered(a.period, a.answer_id, "unknown");
        stop(current, "delivery_unknown", "blocked"); if (current.signal && !current.signal.aborted) abort(current);
      }, 2000);
      return;
    }
```

`ui_prompt_end`: at the top,

```js
    if (event.answered_by && current?.answering?.by === event.answered_by) {
      const a = current.answering; current.answering = undefined; clearTimeout(a.timer);
      void current.client.delivered(a.period, a.answer_id, "ok");
      return;
    }
```

Both return early without touching `uiDepth`, so an answered dialog is not counted as an open human prompt.

- [ ] **Step 4: Run all Node tests.** `node --test assets/session/*.test.mjs`. Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add assets/session
git -c user.email=jw@crasiak.net commit -m "feat(afk): answer Pi dialogs through ui_prompt_respond (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Docs, offline scripted demo, final gate

**Files:**
- Modify: `docs/guides/afk.md`
- Create: `assets/session/aoe-afk-answer-sdk.test.mjs` (scripted-provider demo, modelled on `aoe-afk-work-sdk.test.mjs`)

**Interfaces:**
- Consumes: everything above

- [ ] **Step 1: Write the scripted demo test**

Read `assets/session/aoe-afk-work-sdk.test.mjs` first and reuse its scripted-provider harness. One work period, one item, with a scripted model that in order:
1. ends a turn with `[A]pply / [S]kip` text (expect one nudge),
2. calls `aoe_afk_answer` with proposal `A`,
3. calls `ask_user` with two options,
4. runs `bash` whose output ends `Ok to proceed? (y/N)`,
5. triggers a Pi `confirm` permission dialog through `ctx.ui.confirm` (only if the harness's runner includes the Task 11 hook; otherwise skip this step with `t.skip("ui_prompt_respond not in this Pi runtime")`),
6. checkpoints `completed`.

Run it twice against a real `aoe __afk-bridge` (as the existing SDK test does) or a stub implementing `decide` with the Rust rules semantics. Use the same bridge the existing SDK test uses; if it stubs the bridge, stub `decide` by policy:
- `rules` with one rule `id='plans', kind='choice', answer='agent'`: expect step 2 answered, step 3 (`ask_user`) answered, and the confirm dialog stopped (`no_rule`).
- `yolo`: expect every step answered and the period `completed`.

Assert on the answer log lines (via `answer_delivered` packets, or the real file when using the real bridge) and on `/afk-work status` counts.

- [ ] **Step 2: Run it.** `node --test assets/session/aoe-afk-answer-sdk.test.mjs`. Expected: pass (or step 5 skipped, with the reason printed).

- [ ] **Step 3: Update `docs/guides/afk.md`**

Add a section "Answering menus during a work period" covering: `--policy rules|yolo` and `--rule`, rules file locations and precedence, an example `afk-rules.toml` (the three rules from the spec), what each scope check means, what `yolo` does, the `aoe_afk_answer` tool, the bash timeout and the "commands must not wait for input" rule, which Pi dialogs are answered (not editor or custom; never the consent dialog), menu format files and the user override directory, where the answer log and unrecognised samples live, and how to turn a sample into a new format (definition + fixture). Use the guide's existing heading style and voice.

- [ ] **Step 4: Full gate**

```bash
node --test assets/session/*.test.mjs
git add -A && git -c user.email=jw@crasiak.net commit -m "docs(afk): menu answering guide and scripted demo (CRA-859)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
env -u CLAUDE_CONFIG_DIR aoe-gate
```

Expected: Node tests and every Bender gate pass. Record the outputs for the PR.

- [ ] **Step 5: Hand back to the user**

Report: commits on `feat/CRA-859-afk-menu-answering`, test and gate results, whether Task 11 landed (and where), and what's left: the live canary on the preview route, which needs the route requalified against the new Ledger CLI and the user's go-ahead. Ask whether to push and open the PR (`--repo crasiak/agent-of-empires`, all template sections), and whether to deploy locally with the recipe in memory (`--target aarch64-apple-darwin --profile dev-release --features web`).
