# Session usage counter and usage log

Date: 2026-09-29. Branch: `session-counter` (crasiak fork).

## Goal

Show, for every aoe session, how many times its agent context has been reset,
as a big red number in a corner of the TUI preview pane and of the web session
view, with a small breakdown underneath. Record every underlying event in a
durable local log so usage can be analyzed across sessions (`aoe usage`) and
later imported by Harness Ledger.

Vocabulary: an **instance** is an aoe session (a sidebar row, `Instance.id`).
A **context** is one agent conversation inside it; `/clear`, compaction,
resume, and restart each begin a new context.

## Scope

In: Claude, Codex, and Pi on the host; aoe lifecycle events (create, restart,
delete) for every agent; TUI overlay; web overlay; `aoe usage` CLI; a written
event contract for Ledger.

Out: agent events from sandboxed (container) sessions, since the container has
no host `aoe` binary (lifecycle events are still recorded); other agents;
per-tool counts and token usage (Ledger derives those from transcripts);
retention pruning; the Ledger importer itself, a follow-up in `~/code/attic`
built against the contract below.

## Storage

One SQLite database, `<app_dir>/usage.db` (WAL, `busy_timeout` 2000 ms),
shared by all profiles. Debug builds get their own app dir as usual.

```sql
CREATE TABLE usage_events (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  occurred_at      TEXT NOT NULL,   -- RFC 3339 UTC, millisecond precision
  instance_id      TEXT NOT NULL,
  profile          TEXT,
  agent            TEXT,            -- claude | codex | pi | instance tool name
  kind             TEXT NOT NULL,
  detail           TEXT,
  agent_session_id TEXT             -- native session id; Ledger's join key
);
CREATE INDEX usage_events_instance ON usage_events(instance_id, id);
CREATE TABLE usage_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- usage_meta: schema_version = 1
```

`id` is monotonic and is the export cursor. Rows are never updated or deleted.

## Event kinds

| kind | detail values | meaning |
|---|---|---|
| `context_start` | `startup` `resume` `clear` `compact` `fork` `reload` | agent began a context |
| `context_end` | agent reason, verbatim | agent ended a context |
| `compact` | `manual` `auto` | a compaction finished |
| `prompt` | none | user submitted a prompt |
| `turn_end` | none, or `error` | agent finished a turn |
| `instance_created` | none | aoe created the instance |
| `instance_restarted` | none | aoe restarted the instance's agent |
| `instance_deleted` | none | aoe deleted the instance |

Unknown agent detail values are kept verbatim after sanitizing to at most 32
chars of `[a-z0-9_]`; anything else becomes `NULL`.

### Normalization

| agent | source event | kind | detail |
|---|---|---|---|
| claude, codex | `SessionStart` | `context_start` | `source` |
| claude | `SessionEnd` | `context_end` | `reason` |
| claude, codex | `PostCompact` | `compact` | `trigger` |
| claude, codex | `UserPromptSubmit` | `prompt` | |
| claude, codex | `Stop` | `turn_end` | |
| claude | `StopFailure` | `turn_end` | `error` |
| pi | `session_start` | `context_start` | `reason`, with `new` mapped to `clear` |
| pi | `session_shutdown` | `context_end` | `reason` |
| pi | `session_compact` | `compact` | `manual` stays, `threshold`/`overflow` become `auto` |
| pi | `input` with source `interactive` or `rpc` | `prompt` | |
| pi | `agent_settled` | `turn_end` | |

Claude and Codex name the event in the payload's `hook_event_name`;
`agent_session_id` comes from the payload's `session_id`. Codex gets no
`SessionEnd` hook (older Codex builds reject unknown hook keys).

## Derived metrics (`UsageSummary`)

Computed in Rust from one instance's events in `id` order:

- `clears` = `context_start` with detail `clear`
- `compactions` = `compact` events, split into `compactions_auto` / `compactions_manual`
- `resumes` = max(0, `context_start` with detail not in {`clear`, `compact`} minus 1);
  the first start is the instance's first life, not a reset
- **`resets`** (the headline) = `clears + compactions + resumes`
- `prompts`, `turns`, `turn_errors`: totals
- current context: begins at the latest `context_start` or `compact`;
  `context_started_at`, `context_prompts`, `context_turns` count from there
- `tracked_since` (first event) and `last_event_at`

A compaction emits both `compact` and `context_start/compact` for Claude and
Codex; counting compactions only from `compact` avoids double counting and
keeps Pi (which has no post-compaction start) consistent.

## Capture

### Claude and Codex hooks

- `HookEvent` and `ResolvedHookEvent` gain `usage: bool`. `resolved_hook_events`
  sets it to `event.usage && config.usage.enabled`.
- Claude's `SessionStart`, `UserPromptSubmit`, `Stop`, `StopFailure` gain
  `usage: true`; two new events, `PostCompact` and `SessionEnd`, have no status
  and `usage: true`. Codex's `SessionStart`, `UserPromptSubmit`, `Stop` gain
  `usage: true` plus a new status-less `PostCompact`.
- `event_commands` (host target only) adds, after the identity extractor and
  before the status writer:
  `sh -c '[ -n "$AOE_INSTANCE_ID" ] || exit 0; [ -x "$AOE_HOOK_BIN" ] || exit 0; "$AOE_HOOK_BIN" __usage-event 2>/dev/null; exit 0 # aoe-hooks'`
  It ends with the standard trailer so reinstall and uninstall recognize it.
- Hook installation is required whenever usage is enabled for an agent with
  usage events, even when status hooks are off.

### `aoe __usage-event [--agent NAME]` (hidden)

Dispatched early in `main` like `__extract-session-id`, with no config load or
migrations. It reads at most 1 MiB of stdin JSON. The instance id comes from
`AOE_INSTANCE_ID` (validated), profile from `AOE_PROFILE`, and agent from
`--agent`, else `AOE_REPORT_AGENT`. It reuses the nested-agent guard from
`extract_session_id` (moved to a shared helper) so a nested agent launched
from a shell tool cannot count against the pane. It normalizes and inserts one
row. Every failure is logged at debug and the process exits 0.

### Pi

`assets/session/aoe-session-id.js` gains usage reporting when
`AOE_USAGE_BIN` and `AOE_INSTANCE_ID` are set: on the events in the table it
spawns `AOE_USAGE_BIN __usage-event --agent pi` detached with stdin piped,
writes `{"hook_event_name":<pi event>,"reason":…,"source":…,"session_id":…}`,
ends stdin, and `unref`s. Errors are swallowed. The host Pi launch
(`identity_extension_launch`) adds `AOE_USAGE_BIN`, `AOE_INSTANCE_ID`, and
`AOE_PROFILE` to the extension environment when usage is enabled.

### aoe lifecycle

`usage::record_lifecycle(&Instance, kind)` is called best-effort where an
instance is first persisted, where aoe restarts an agent, and where an
instance is deleted. It is a no-op when usage is disabled.

## Settings

New section `[usage]` (category "Usage"):

- `enabled: bool`, default `true`: install usage hooks and record events.
- `show_overlay: bool`, default `true`: show the TUI and web overlays.

## TUI overlay

- `UsagePoller` (a `Worker`, like `MetricsPoller`) loads the `UsageSummary`
  for the selected instance on selection change and every status tick.
- `src/tui/components/usage_overlay.rs` renders a box anchored to the top-right
  of the agent pane rect with a one-cell margin, `Clear`ed first:
  - the `resets` number in 3-row block digits, bold, `theme.error` (red);
  - `clr 12 · cmp 3 (2a) · rsm 2`
  - `ctx 42m · 18p 17t`
  - `age 3d 4h` (instance age from `created_at`)
- Hidden when `show_overlay` is off, when the instance has no events, or when
  the pane is too small (box plus 10 columns or plus 2 rows).

## Web overlay

- `GET /api/sessions/{id}/usage` returns the `UsageSummary` (camelCase JSON,
  zeros and nulls when there are no events).
- `UsageOverlay.tsx` is absolutely positioned top-right over the session
  terminal view, `pointer-events-none`, with a large bold red number and the
  same three small lines. It polls every 5 s while mounted and the page is
  visible, and hides under the same conditions as the TUI.

## `aoe usage` CLI

- `aoe usage [--since 30d] [--json]`: totals across instances (active
  instances, contexts, clears, compactions manual/auto, resumes, prompts,
  turns, turn errors), median prompts per context, median context duration,
  median and max resets per instance, top 10 instances by resets (titles from
  current sessions, else the id).
- `aoe usage show <session> [--json]`: one instance's summary and its last 20
  events.
- `aoe usage export [--after-id N] [--limit N]`: raw events as JSON lines in
  `id` order. This is the Ledger feed.

`docs/development/usage-events.md` documents the table, kinds, details, and
export format as the Ledger contract. Changes within `schema_version` 1 are
additive only.

## Error handling

Capture never blocks or fails an agent: every hook path exits 0, and the Pi
extension never throws. A locked or corrupt database drops the event with a
debug log. Readers (overlay, API, CLI) treat a missing database as "no events".

## Testing

- Unit tests, table-driven: normalization (agent × payload → row), summary
  derivation (event sequences → counts, including the compaction double event
  and first-start rule), store round trip in a temp app dir, block-digit
  rendering, hook command installation (usage command present for usage events
  on host, absent for sandbox or when disabled), `__usage-event` inner run.
- Web: Vitest for the overlay's formatting and hide rules.
- Manual: a debug build with a Claude session; `/clear` and `/compact` move the
  overlay number.
