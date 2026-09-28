# Session Usage Counter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record agent context resets, prompts, turns, and aoe lifecycle events per aoe instance in a local SQLite log, and show the reset count as a big red overlay in the TUI preview and web session view, plus an `aoe usage` CLI and a Ledger export contract.

**Architecture:** A new `crate::usage` module owns the schema, normalization of hook payloads, summary derivation, and the store. Claude and Codex hooks and a Pi extension call a hidden `aoe __usage-event` that inserts one row; aoe itself records lifecycle rows. The TUI (background worker) and the web API read a per-instance `UsageSummary`.

**Tech Stack:** Rust 2021, rusqlite 0.40 (bundled), chrono, serde, clap, ratatui 0.30, axum; React + TypeScript + Vitest for the web.

**Spec:** `docs/superpowers/specs/2026-09-29-session-usage-counter-design.md`

## Global Constraints

- Follow `AGENTS.md`: `cargo fmt` and `cargo clippy` clean, no dead code, no `#[allow(dead_code)]`, short comments only where code cannot say it, no em dashes or `--` as prose separators in docs and comments.
- Settings: `session.usage_tracking: bool` default `true`; `session.show_usage_overlay: bool` default `true`. Declared once with `#[setting(...)]` per `docs/development/adding-settings.md`.
- Database: `<app_dir>/usage.db` via `crate::session::get_app_dir()`, WAL, `busy_timeout` 2000 ms, `usage_meta.schema_version = "1"`.
- Hook and extension paths must never block or fail the agent: always exit 0, print nothing to stdout.
- Host only: no usage commands for `HookInstallTarget::Sandbox`.
- Tests: deterministic, isolated from user state (use `crate::session::test_support::isolate_app_dir_at` or explicit temp paths), table cases in one test when setup is shared, no fixed sleeps.
- Use the narrowest feature set: `cargo test --lib <filter>` for Rust; `--features web` only for Task 7 and the final check.
- Commit after each task with a conventional commit message ending in `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`. Never use bare `git stash`.

---

### Task 1: Usage core module and settings

**Files:**
- Create: `src/usage/mod.rs`, `src/usage/normalize.rs`, `src/usage/summary.rs`, `src/usage/store.rs`
- Modify: `src/lib.rs` (add `pub mod usage;` in alphabetical order after `pub mod update;`)
- Modify: `src/session/config/mod.rs` (two `SessionConfig` fields after `show_diagnostics_pane`, plus `Default`)

**Interfaces:**
- Produces:
  - `crate::usage::UsageKind` enum: `ContextStart, ContextEnd, Compact, Prompt, TurnEnd, InstanceCreated, InstanceRestarted, InstanceDeleted`; `as_str(self) -> &'static str`, `parse(&str) -> Option<Self>`; serde snake_case.
  - `crate::usage::UsageEvent { id: i64, occurred_at: DateTime<Utc>, instance_id: String, profile: Option<String>, agent: Option<String>, kind: UsageKind, detail: Option<String>, agent_session_id: Option<String> }` (Serialize, snake_case field names: this is the export contract).
  - `crate::usage::Normalized { kind: UsageKind, detail: Option<String>, agent_session_id: Option<String> }`
  - `crate::usage::normalize(agent: &str, payload: &serde_json::Value) -> Option<Normalized>`
  - `crate::usage::UsageSummary` (Serialize camelCase, Default) and `crate::usage::summarize(events: &[UsageEvent]) -> UsageSummary`
  - `crate::usage::is_context_boundary(event: &UsageEvent) -> bool`
  - `crate::usage::UsageStore::{open(&Path) -> Result<Self>, open_default() -> Result<Self>, insert(&UsageEvent) -> Result<i64>, events_for_instance(&str) -> Result<Vec<UsageEvent>>, summary_for(&str) -> Result<UsageSummary>, events_after(i64, usize) -> Result<Vec<UsageEvent>>, events_since(DateTime<Utc>) -> Result<Vec<UsageEvent>>}`
  - `crate::usage::db_path() -> anyhow::Result<PathBuf>`
  - `SessionConfig.usage_tracking: bool`, `SessionConfig.show_usage_overlay: bool`

- [ ] **Step 1: Write `src/usage/mod.rs`**

```rust
//! Local usage log: agent context resets, prompts, turns, and aoe lifecycle
//! events per instance, stored in `<app_dir>/usage.db`. The table and export
//! format are the Ledger contract in `docs/development/usage-events.md`.

mod normalize;
mod store;
mod summary;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;

pub use normalize::{normalize, Normalized};
pub use store::UsageStore;
pub use summary::{is_context_boundary, summarize, UsageSummary};

const DB_FILE: &str = "usage.db";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    ContextStart,
    ContextEnd,
    Compact,
    Prompt,
    TurnEnd,
    InstanceCreated,
    InstanceRestarted,
    InstanceDeleted,
}

impl UsageKind {
    const ALL: [UsageKind; 8] = [
        UsageKind::ContextStart,
        UsageKind::ContextEnd,
        UsageKind::Compact,
        UsageKind::Prompt,
        UsageKind::TurnEnd,
        UsageKind::InstanceCreated,
        UsageKind::InstanceRestarted,
        UsageKind::InstanceDeleted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            UsageKind::ContextStart => "context_start",
            UsageKind::ContextEnd => "context_end",
            UsageKind::Compact => "compact",
            UsageKind::Prompt => "prompt",
            UsageKind::TurnEnd => "turn_end",
            UsageKind::InstanceCreated => "instance_created",
            UsageKind::InstanceRestarted => "instance_restarted",
            UsageKind::InstanceDeleted => "instance_deleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageEvent {
    /// Store-assigned and monotonic; 0 before insert.
    pub id: i64,
    pub occurred_at: DateTime<Utc>,
    pub instance_id: String,
    pub profile: Option<String>,
    pub agent: Option<String>,
    pub kind: UsageKind,
    pub detail: Option<String>,
    pub agent_session_id: Option<String>,
}

pub fn db_path() -> anyhow::Result<PathBuf> {
    Ok(crate::session::get_app_dir()?.join(DB_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in UsageKind::ALL {
            assert_eq!(UsageKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().to_string())
            );
        }
        assert_eq!(UsageKind::parse("nope"), None);
    }
}
```

- [ ] **Step 2: Write the failing normalization test in `src/usage/normalize.rs`**

```rust
//! Maps agent hook payloads to usage rows (see the spec's normalization table).

use serde_json::Value;

use super::UsageKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub kind: UsageKind,
    pub detail: Option<String>,
    pub agent_session_id: Option<String>,
}

pub fn normalize(agent: &str, payload: &Value) -> Option<Normalized> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn payloads_map_to_kinds_and_details() {
        use UsageKind::*;
        const SID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let cases: Vec<(&str, Value, Option<(UsageKind, Option<&str>)>)> = vec![
            ("claude", json!({"hook_event_name":"SessionStart","source":"clear","session_id":SID}), Some((ContextStart, Some("clear")))),
            ("codex", json!({"hook_event_name":"SessionStart","source":"fork","session_id":SID}), Some((ContextStart, Some("fork")))),
            ("claude", json!({"hook_event_name":"SessionEnd","reason":"prompt_input_exit"}), Some((ContextEnd, Some("prompt_input_exit")))),
            ("claude", json!({"hook_event_name":"PostCompact","trigger":"auto"}), Some((Compact, Some("auto")))),
            ("codex", json!({"hook_event_name":"PostCompact","trigger":"manual"}), Some((Compact, Some("manual")))),
            ("claude", json!({"hook_event_name":"UserPromptSubmit","prompt":"hi"}), Some((Prompt, None))),
            ("claude", json!({"hook_event_name":"Stop"}), Some((TurnEnd, None))),
            ("claude", json!({"hook_event_name":"StopFailure"}), Some((TurnEnd, Some("error")))),
            ("claude", json!({"hook_event_name":"PreToolUse"}), None),
            ("claude", json!({"source":"clear"}), None),
            ("claude", json!({"hook_event_name":"SessionStart","source":"Weird Value!"}), Some((ContextStart, None))),
            ("claude", json!({"hook_event_name":"SessionStart","source":"x".repeat(33)}), Some((ContextStart, None))),
            ("pi", json!({"hook_event_name":"session_start","reason":"new"}), Some((ContextStart, Some("clear")))),
            ("pi", json!({"hook_event_name":"session_start","reason":"reload"}), Some((ContextStart, Some("reload")))),
            ("pi", json!({"hook_event_name":"session_shutdown","reason":"quit"}), Some((ContextEnd, Some("quit")))),
            ("pi", json!({"hook_event_name":"session_compact","reason":"manual"}), Some((Compact, Some("manual")))),
            ("pi", json!({"hook_event_name":"session_compact","reason":"threshold"}), Some((Compact, Some("auto")))),
            ("pi", json!({"hook_event_name":"session_compact","reason":"overflow"}), Some((Compact, Some("auto")))),
            ("pi", json!({"hook_event_name":"input","source":"interactive"}), Some((Prompt, None))),
            ("pi", json!({"hook_event_name":"input","source":"rpc"}), Some((Prompt, None))),
            ("pi", json!({"hook_event_name":"input","source":"extension"}), None),
            ("pi", json!({"hook_event_name":"agent_settled"}), Some((TurnEnd, None))),
            ("pi", json!({"hook_event_name":"SessionStart","source":"clear"}), None),
        ];
        for (agent, payload, expected) in cases {
            let got = normalize(agent, &payload).map(|n| (n.kind, n.detail));
            let expected = expected.map(|(k, d)| (k, d.map(str::to_string)));
            assert_eq!(got, expected, "{agent} {payload}");
        }
    }

    #[test]
    fn session_id_is_kept_only_when_safe() {
        let ok = json!({"hook_event_name":"Stop","session_id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"});
        let unsafe_id = json!({"hook_event_name":"Stop","session_id":"bad id;rm"});
        assert_eq!(
            normalize("claude", &ok).unwrap().agent_session_id.as_deref(),
            Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
        );
        assert_eq!(normalize("claude", &unsafe_id).unwrap().agent_session_id, None);
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test --lib usage::normalize`
Expected: FAIL (panics at `todo!()`).

- [ ] **Step 4: Implement `normalize`**

Replace the `todo!()` body:

```rust
pub fn normalize(agent: &str, payload: &Value) -> Option<Normalized> {
    let field = |key: &str| payload.get(key).and_then(Value::as_str);
    let event = field("hook_event_name")?;
    let (kind, detail) = if agent == "pi" {
        match event {
            "session_start" => (
                UsageKind::ContextStart,
                field("reason").map(|reason| if reason == "new" { "clear" } else { reason }),
            ),
            "session_shutdown" => (UsageKind::ContextEnd, field("reason")),
            "session_compact" => (
                UsageKind::Compact,
                Some(if field("reason") == Some("manual") { "manual" } else { "auto" }),
            ),
            "input" if matches!(field("source"), Some("interactive" | "rpc")) => {
                (UsageKind::Prompt, None)
            }
            "agent_settled" => (UsageKind::TurnEnd, None),
            _ => return None,
        }
    } else {
        match event {
            "SessionStart" => (UsageKind::ContextStart, field("source")),
            "SessionEnd" => (UsageKind::ContextEnd, field("reason")),
            "PostCompact" => (UsageKind::Compact, field("trigger")),
            "UserPromptSubmit" => (UsageKind::Prompt, None),
            "Stop" => (UsageKind::TurnEnd, None),
            "StopFailure" => (UsageKind::TurnEnd, Some("error")),
            _ => return None,
        }
    };
    Some(Normalized {
        kind,
        detail: detail.and_then(sanitize_detail),
        agent_session_id: field("session_id")
            .filter(|sid| crate::session::capture::is_valid_session_id(sid))
            .map(str::to_string),
    })
}

/// At most 32 chars of `[a-z0-9_]`; anything else is dropped rather than stored.
fn sanitize_detail(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then(|| value.to_string())
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --lib usage::normalize`
Expected: PASS (2 tests). If `is_valid_session_id` is not reachable from here, check its visibility in `src/session/capture/mod.rs` and widen it to `pub(crate)` rather than duplicating it.

- [ ] **Step 6: Write the failing summary test in `src/usage/summary.rs`**

```rust
//! Per-instance counts derived from its usage events in id order.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{UsageEvent, UsageKind};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    /// The headline: clears + compactions + resumes.
    pub resets: u32,
    pub clears: u32,
    pub compactions: u32,
    pub compactions_auto: u32,
    pub compactions_manual: u32,
    /// Context starts other than clear and compaction, minus the first one.
    pub resumes: u32,
    pub prompts: u32,
    pub turns: u32,
    pub turn_errors: u32,
    pub context_started_at: Option<DateTime<Utc>>,
    pub context_prompts: u32,
    pub context_turns: u32,
    pub tracked_since: Option<DateTime<Utc>>,
    pub last_event_at: Option<DateTime<Utc>>,
}

/// A compaction logs both `compact` and `context_start/compact`; only the
/// former opens a context, so the pair counts once.
pub fn is_context_boundary(event: &UsageEvent) -> bool {
    match event.kind {
        UsageKind::Compact => true,
        UsageKind::ContextStart => event.detail.as_deref() != Some("compact"),
        _ => false,
    }
}

pub fn summarize(events: &[UsageEvent]) -> UsageSummary {
    todo!()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    pub(crate) fn ev(minute: u32, kind: UsageKind, detail: Option<&str>) -> UsageEvent {
        UsageEvent {
            id: 0,
            occurred_at: Utc.with_ymd_and_hms(2026, 9, 29, 10, minute, 0).unwrap(),
            instance_id: "inst".into(),
            profile: None,
            agent: Some("claude".into()),
            kind,
            detail: detail.map(str::to_string),
            agent_session_id: None,
        }
    }

    #[test]
    fn empty_log_is_all_zero() {
        assert_eq!(summarize(&[]), UsageSummary::default());
    }

    #[test]
    fn resets_count_clears_compactions_and_later_starts() {
        use UsageKind::*;
        let events = vec![
            ev(0, InstanceCreated, None),
            ev(1, ContextStart, Some("startup")),
            ev(2, Prompt, None),
            ev(3, TurnEnd, None),
            ev(4, ContextStart, Some("clear")),
            ev(5, Prompt, None),
            ev(6, TurnEnd, Some("error")),
            ev(7, Compact, Some("auto")),
            ev(7, ContextStart, Some("compact")),
            ev(8, Prompt, None),
            ev(9, InstanceRestarted, None),
            ev(10, ContextStart, Some("resume")),
            ev(11, Compact, Some("manual")),
            ev(12, Prompt, None),
            ev(13, Prompt, None),
            ev(14, TurnEnd, None),
        ];
        let s = summarize(&events);
        assert_eq!((s.clears, s.compactions, s.resumes, s.resets), (1, 2, 1, 4));
        assert_eq!((s.compactions_auto, s.compactions_manual), (1, 1));
        assert_eq!((s.prompts, s.turns, s.turn_errors), (5, 3, 1));
        assert_eq!(s.context_started_at, Some(events[12].occurred_at));
        assert_eq!((s.context_prompts, s.context_turns), (2, 1));
        assert_eq!(s.tracked_since, Some(events[0].occurred_at));
        assert_eq!(s.last_event_at, Some(events[15].occurred_at));
    }

    #[test]
    fn a_first_seen_resume_is_not_a_reset() {
        let s = summarize(&[ev(0, UsageKind::ContextStart, Some("resume"))]);
        assert_eq!((s.resumes, s.resets), (0, 0));
    }
}
```

- [ ] **Step 7: Run it to verify it fails**

Run: `cargo test --lib usage::summary`
Expected: FAIL (`todo!()`; `empty_log_is_all_zero` also panics).

- [ ] **Step 8: Implement `summarize`**

```rust
pub fn summarize(events: &[UsageEvent]) -> UsageSummary {
    let mut summary = UsageSummary::default();
    let mut starts = 0u32;
    for event in events {
        summary.tracked_since.get_or_insert(event.occurred_at);
        summary.last_event_at = Some(event.occurred_at);
        if is_context_boundary(event) {
            summary.context_started_at = Some(event.occurred_at);
            summary.context_prompts = 0;
            summary.context_turns = 0;
        }
        match event.kind {
            UsageKind::ContextStart => match event.detail.as_deref() {
                Some("clear") => summary.clears += 1,
                Some("compact") => {}
                _ => starts += 1,
            },
            UsageKind::Compact => {
                summary.compactions += 1;
                match event.detail.as_deref() {
                    Some("auto") => summary.compactions_auto += 1,
                    Some("manual") => summary.compactions_manual += 1,
                    _ => {}
                }
            }
            UsageKind::Prompt => {
                summary.prompts += 1;
                summary.context_prompts += 1;
            }
            UsageKind::TurnEnd => {
                summary.turns += 1;
                summary.context_turns += 1;
                if event.detail.as_deref() == Some("error") {
                    summary.turn_errors += 1;
                }
            }
            UsageKind::ContextEnd
            | UsageKind::InstanceCreated
            | UsageKind::InstanceRestarted
            | UsageKind::InstanceDeleted => {}
        }
    }
    summary.resumes = starts.saturating_sub(1);
    summary.resets = summary.clears + summary.compactions + summary.resumes;
    summary
}
```

- [ ] **Step 9: Run to verify it passes**

Run: `cargo test --lib usage::summary`
Expected: PASS (3 tests).

- [ ] **Step 10: Write the failing store test in `src/usage/store.rs`**

```rust
//! SQLite persistence for usage events.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{summarize, UsageEvent, UsageKind, UsageSummary};

const SCHEMA_VERSION: &str = "1";

pub struct UsageStore {
    conn: Connection,
}

impl UsageStore {
    pub fn open(path: &Path) -> Result<Self> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::summary::tests::ev;

    #[test]
    fn events_round_trip_and_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let store = UsageStore::open(&path).unwrap();
        let mut first = ev(0, UsageKind::ContextStart, Some("startup"));
        first.agent_session_id = Some("sid-1".into());
        first.profile = Some("work".into());
        let mut other = ev(1, UsageKind::Prompt, None);
        other.instance_id = "other".into();
        let third = ev(2, UsageKind::ContextStart, Some("clear"));
        let ids: Vec<i64> = [&first, &other, &third]
            .iter()
            .map(|e| store.insert(e).unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));

        let mine = store.events_for_instance("inst").unwrap();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].id, ids[0]);
        assert_eq!(
            UsageEvent { id: 0, ..mine[0].clone() },
            first,
            "every column survives the round trip"
        );
        assert_eq!(store.summary_for("inst").unwrap().clears, 1);
        assert_eq!(store.events_after(ids[0], 10).unwrap().len(), 2);
        assert_eq!(store.events_after(ids[0], 1).unwrap()[0].id, ids[1]);
        assert_eq!(store.events_since(third.occurred_at).unwrap().len(), 1);

        drop(store);
        let reopened = UsageStore::open(&path).unwrap();
        assert_eq!(reopened.events_after(0, 100).unwrap().len(), 3);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        drop(UsageStore::open(&path).unwrap());
        Connection::open(&path)
            .unwrap()
            .execute("UPDATE usage_meta SET value = '2' WHERE key = 'schema_version'", [])
            .unwrap();
        assert!(UsageStore::open(&path).is_err());
    }
}
```

- [ ] **Step 11: Run it to verify it fails**

Run: `cargo test --lib usage::store`
Expected: FAIL (`todo!()`).

- [ ] **Step 12: Implement the store**

Replace the `impl UsageStore` block and add the helpers:

```rust
impl UsageStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("open usage log at {}", path.display()))?;
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 occurred_at TEXT NOT NULL,
                 instance_id TEXT NOT NULL,
                 profile TEXT,
                 agent TEXT,
                 kind TEXT NOT NULL,
                 detail TEXT,
                 agent_session_id TEXT
             );
             CREATE INDEX IF NOT EXISTS usage_events_instance
                 ON usage_events(instance_id, id);
             CREATE TABLE IF NOT EXISTS usage_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             INSERT OR IGNORE INTO usage_meta(key, value) VALUES ('schema_version', '1');",
        )?;
        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM usage_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if version.as_deref() != Some(SCHEMA_VERSION) {
            bail!("unsupported usage log schema version {version:?}");
        }
        Ok(Self { conn })
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&super::db_path()?)
    }

    pub fn insert(&self, event: &UsageEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO usage_events
                 (occurred_at, instance_id, profile, agent, kind, detail, agent_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.occurred_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                event.instance_id,
                event.profile,
                event.agent,
                event.kind.as_str(),
                event.detail,
                event.agent_session_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn events_for_instance(&self, instance_id: &str) -> Result<Vec<UsageEvent>> {
        self.query(
            "WHERE instance_id = ?1 ORDER BY id",
            params![instance_id],
        )
    }

    pub fn summary_for(&self, instance_id: &str) -> Result<UsageSummary> {
        Ok(summarize(&self.events_for_instance(instance_id)?))
    }

    /// The export cursor: rows with `id > after_id`, oldest first.
    pub fn events_after(&self, after_id: i64, limit: usize) -> Result<Vec<UsageEvent>> {
        self.query(
            "WHERE id > ?1 ORDER BY id LIMIT ?2",
            params![after_id, limit as i64],
        )
    }

    pub fn events_since(&self, since: DateTime<Utc>) -> Result<Vec<UsageEvent>> {
        self.query(
            "WHERE occurred_at >= ?1 ORDER BY id",
            params![since.to_rfc3339_opts(SecondsFormat::Millis, true)],
        )
    }

    fn query(&self, clause: &str, params: impl rusqlite::Params) -> Result<Vec<UsageEvent>> {
        let sql = format!(
            "SELECT id, occurred_at, instance_id, profile, agent, kind, detail, agent_session_id
             FROM usage_events {clause}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, read_row)?;
        let mut events = Vec::new();
        for row in rows {
            // A row this build cannot read (an unknown kind) is skipped, not fatal.
            if let Some(event) = row? {
                events.push(event);
            }
        }
        Ok(events)
    }
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<Option<UsageEvent>> {
    let occurred_at: String = row.get(1)?;
    let kind: String = row.get(5)?;
    let (Ok(occurred_at), Some(kind)) = (
        DateTime::parse_from_rfc3339(&occurred_at),
        UsageKind::parse(&kind),
    ) else {
        return Ok(None);
    };
    Ok(Some(UsageEvent {
        id: row.get(0)?,
        occurred_at: occurred_at.with_timezone(&Utc),
        instance_id: row.get(2)?,
        profile: row.get(3)?,
        agent: row.get(4)?,
        kind,
        detail: row.get(6)?,
        agent_session_id: row.get(7)?,
    }))
}
```

Note: `events_since` compares RFC 3339 strings; this is correct because every row is written by `insert` in the same fixed `...Z` millisecond format.

- [ ] **Step 13: Run to verify it passes**

Run: `cargo test --lib usage::`
Expected: PASS (all usage tests). If `tempfile` is not a dev-dependency, use the same temp-dir helper other tests in `src/events/mod.rs` use.

- [ ] **Step 14: Add the two settings**

In `src/session/config/mod.rs`, after the `show_diagnostics_pane` field of `SessionConfig`:

```rust
    /// Record agent context resets, prompts, and turns per session in a local
    /// usage log (`usage.db` in the app directory). Installs small Claude and
    /// Codex hooks and a Pi extension hook; nothing leaves this machine.
    #[serde(default = "default_true")]
    #[setting(label = "Record session usage", widget = "toggle")]
    pub usage_tracking: bool,

    /// Show the session's context-reset count and usage breakdown in a corner
    /// of the preview pane and the web session view.
    #[serde(default = "default_true")]
    #[setting(label = "Show usage overlay", widget = "toggle")]
    pub show_usage_overlay: bool,
```

And in `impl Default for SessionConfig`, after `show_diagnostics_pane: false,`:

```rust
            usage_tracking: true,
            show_usage_overlay: true,
```

- [ ] **Step 15: Build and run the settings schema tests**

Run: `cargo test --lib settings_schema && cargo test --lib usage::`
Expected: PASS. If a schema snapshot or field-count test fails, update its expectation to include the two new fields (they are intentional).

- [ ] **Step 16: Commit**

```bash
git add src/usage src/lib.rs src/session/config/mod.rs
git commit -m "feat(usage): add local usage log store, normalizer, and summary"
```

---

### Task 2: Hidden `aoe __usage-event` command

**Files:**
- Create: `src/cli/usage_event.rs`
- Modify: `src/cli/mod.rs` (`pub mod usage_event;`), `src/cli/definition.rs` (variant + `command_name`), `src/main.rs` (early dispatch next to `ExtractSessionId`), `src/cli/extract_session_id.rs` (make `fired_by_pane_agent` `pub(crate)`)

**Interfaces:**
- Consumes: `crate::usage::{normalize, UsageEvent, UsageStore, db_path}`.
- Produces: CLI `aoe __usage-event [--agent NAME]` reading hook JSON on stdin; `crate::cli::usage_event::run(UsageEventArgs) -> anyhow::Result<()>`.

- [ ] **Step 1: Write `src/cli/usage_event.rs` with a failing test**

```rust
//! Hidden `aoe __usage-event` subcommand: records one agent hook event in the
//! usage log. Spawned by host hooks and the Pi extension; always exits 0.

use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Result};
use clap::Args;

const STDIN_BYTE_CAP: u64 = 1 << 20;

#[derive(Args)]
pub struct UsageEventArgs {
    /// Agent that fired the hook; defaults to `AOE_REPORT_AGENT`.
    #[arg(long)]
    agent: Option<String>,
}

pub async fn run(args: UsageEventArgs) -> Result<()> {
    let Ok(instance_id) = std::env::var("AOE_INSTANCE_ID") else {
        return Ok(());
    };
    if crate::session::validate_instance_id(&instance_id).is_err()
        || !super::extract_session_id::fired_by_pane_agent()
    {
        return Ok(());
    }
    let env = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
    let agent = args.agent.or_else(|| env("AOE_REPORT_AGENT"));
    let profile = env("AOE_PROFILE");
    let result = crate::usage::db_path().and_then(|db| {
        record(
            std::io::stdin().lock(),
            &db,
            &instance_id,
            agent.as_deref(),
            profile.as_deref(),
        )
    });
    if let Err(e) = result {
        tracing::debug!(target: "usage", "usage event dropped: {e}");
    }
    Ok(())
}

/// Returns whether a row was written; an event the log does not track is not an error.
fn record<R: Read>(
    stdin: R,
    db: &Path,
    instance_id: &str,
    agent: Option<&str>,
    profile: Option<&str>,
) -> Result<bool> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_tracked_events_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("usage.db");
        let cases: [(&str, Option<&str>, bool); 4] = [
            (r#"{"hook_event_name":"SessionStart","source":"clear","session_id":"s1"}"#, Some("claude"), true),
            (r#"{"hook_event_name":"PreToolUse"}"#, Some("claude"), false),
            (r#"{"hook_event_name":"session_compact","reason":"threshold"}"#, Some("pi"), true),
            ("not json", Some("claude"), false),
        ];
        for (payload, agent, written) in cases {
            let got = record(payload.as_bytes(), &db, "inst", agent, Some("work"));
            assert_eq!(got.unwrap_or(false), written, "{payload}");
        }
        let events = crate::usage::UsageStore::open(&db)
            .unwrap()
            .events_for_instance("inst")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].agent.as_deref(), Some("claude"));
        assert_eq!(events[0].profile.as_deref(), Some("work"));
        assert_eq!(events[0].detail.as_deref(), Some("clear"));
        assert_eq!(events[0].agent_session_id.as_deref(), Some("s1"));
        assert_eq!(events[1].detail.as_deref(), Some("auto"));
    }
}
```

- [ ] **Step 2: Wire the module and command so the test compiles**

`src/cli/mod.rs`: add `pub mod usage_event;` in alphabetical position.

`src/cli/definition.rs`: import `use super::usage_event::UsageEventArgs;` and add after the `ExtractSessionId` variant:

```rust
    /// Internal: record one agent hook event in the usage log. Spawned by
    /// host hooks and the Pi extension. Hidden from help.
    #[command(name = "__usage-event", hide = true)]
    UsageEvent(UsageEventArgs),
```

In `command_name`, next to `Commands::ExtractSessionId(_) => return None,` add `Commands::UsageEvent(_) => return None,`.

`src/main.rs`, next to the `ExtractSessionId` arm in the "No app data or migrations needed" match:

```rust
        Some(Commands::UsageEvent(args)) => return cli::usage_event::run(args).await,
```

`src/cli/extract_session_id.rs`: change `fn fired_by_pane_agent()` to `pub(crate) fn fired_by_pane_agent()`.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib cli::usage_event`
Expected: FAIL (`todo!()`).

- [ ] **Step 4: Implement `record`**

```rust
fn record<R: Read>(
    stdin: R,
    db: &Path,
    instance_id: &str,
    agent: Option<&str>,
    profile: Option<&str>,
) -> Result<bool> {
    let mut buf = String::new();
    stdin.take(STDIN_BYTE_CAP).read_to_string(&mut buf)?;
    let payload: serde_json::Value = serde_json::from_str(&buf)?;
    let Some(normalized) = crate::usage::normalize(agent.unwrap_or(""), &payload) else {
        return Ok(false);
    };
    let event = crate::usage::UsageEvent {
        id: 0,
        occurred_at: chrono::Utc::now(),
        instance_id: instance_id.to_string(),
        profile: profile.map(str::to_string),
        agent: agent.map(str::to_string),
        kind: normalized.kind,
        detail: normalized.detail,
        agent_session_id: normalized.agent_session_id,
    };
    crate::usage::UsageStore::open(db)
        .and_then(|store| store.insert(&event))
        .map_err(|e| anyhow!("insert failed: {e}"))?;
    Ok(true)
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --lib cli::usage_event && cargo test --lib cli::definition`
Expected: PASS.

- [ ] **Step 6: Manual smoke test of the binary path**

```bash
cargo build
D=$(mktemp -d)
echo '{"hook_event_name":"SessionStart","source":"startup","session_id":"s1"}' \
  | HOME=$D AOE_INSTANCE_ID=smoke AOE_REPORT_AGENT=claude ./target/debug/aoe __usage-event; echo "exit=$?"
find $D -name usage.db
```

Expected: `exit=0`, no stdout output, and a `usage.db` under the isolated app dir. (If the debug build derives its app dir differently, locate it with `find $D -name usage.db`.)

- [ ] **Step 7: Commit**

```bash
git add src/cli/usage_event.rs src/cli/mod.rs src/cli/definition.rs src/main.rs src/cli/extract_session_id.rs
git commit -m "feat(usage): record agent hook events via aoe __usage-event"
```

---

### Task 3: Install usage hooks for Claude and Codex

**Files:**
- Modify: `src/agents.rs` (`HookEvent`, `ResolvedHookEvent`, `hook()`, `CLAUDE_HOOK_EVENTS`, `CODEX_HOOK_EVENTS`, `resolved_hook_events`, `append_configured_status_events`, `resolved_sidecar_hook_events`, `hook_install_required`)
- Modify: `src/hooks/command.rs` (new `hook_command_usage_event`)
- Modify: `src/hooks/json_settings.rs` (`event_commands`)
- Modify: `src/session/instance/hooks.rs` (`resolved_host_hook_events`, `hook_install_required` call sites), `src/tui/home/input.rs:2040` (call site)
- Test: in-module tests in `src/hooks/json_settings.rs` and `src/agents.rs`

**Interfaces:**
- Consumes: CLI name `__usage-event` from Task 2; `SessionConfig.usage_tracking` from Task 1.
- Produces: `HookEvent.usage: bool`, `ResolvedHookEvent.usage: bool`, `crate::hooks::command::hook_command_usage_event() -> String` (pub(crate)), `hook_install_required(agent, status_hooks_enabled: bool, usage_enabled: bool) -> bool`.

- [ ] **Step 1: Write the failing install test in `src/hooks/json_settings.rs` tests**

Add to the existing `mod tests` (reuse its helpers such as `read_json`; build events with `crate::agents::resolved_hook_events` against a default config, the same way nearby tests do via `agent_events`):

```rust
    #[test]
    fn usage_command_is_installed_on_host_for_usage_events_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let events = agent_events("claude", &[]);
        assert!(events.iter().any(|e| e.usage), "claude declares usage events");

        install_hooks(&path, &events, HookInstallTarget::Host).unwrap();
        let hooks = read_json(&path)["hooks"].clone();
        let commands = |event: &str| -> Vec<String> {
            hooks[event]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|h| h["command"].as_str().map(str::to_string))
                .collect()
        };
        for event in ["SessionStart", "UserPromptSubmit", "Stop", "StopFailure", "PostCompact", "SessionEnd"] {
            assert!(
                commands(event).iter().any(|c| c.contains("__usage-event")),
                "{event} records usage"
            );
        }
        assert!(!commands("PreToolUse").iter().any(|c| c.contains("__usage-event")));
        assert!(uninstall_hooks(&path).unwrap());
        assert!(read_json(&path)["hooks"].get("PostCompact").is_none());

        install_hooks(&path, &events, HookInstallTarget::Sandbox).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("__usage-event"));
    }
```

If `agent_events` in this test module takes a different shape, build `events` with whatever helper the neighboring tests use; the assertions stay the same.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib hooks::json_settings`
Expected: compile error (`usage` field missing). That is the failing state.

- [ ] **Step 3: Add `usage` to the event types in `src/agents.rs`**

```rust
pub struct HookEvent {
    pub name: &'static str,
    pub matcher: Option<&'static str>,
    pub status: Option<HookStatus>,
    pub identity_field: Option<HookIdentityField>,
    /// Tools that block on the user for their whole run; the hook writes `waiting` for them.
    pub waiting_tools: &'static [&'static str],
    /// Also record the event in the usage log (`crate::usage`).
    pub usage: bool,
}

pub struct ResolvedHookEvent {
    pub name: String,
    pub matcher: Option<String>,
    pub status: Option<HookStatus>,
    pub identity_field: Option<HookIdentityField>,
    pub waiting_tools: Vec<String>,
    pub usage: bool,
}
```

In `const fn hook(...)` add `usage: false,`. Add a helper next to `hook`:

```rust
/// An event recorded in the usage log, with or without a status.
const fn usage_hook(name: &'static str, status: Option<HookStatus>) -> HookEvent {
    HookEvent {
        status,
        usage: true,
        ..hook(name, HookStatus::Idle)
    }
}
```

Update `CLAUDE_HOOK_EVENTS`:

```rust
const CLAUDE_HOOK_EVENTS: &[HookEvent] = &[
    HookEvent {
        status: None,
        identity_field: Some(HookIdentityField::SessionId),
        usage: true,
        ..hook("SessionStart", HookStatus::Idle)
    },
    HookEvent {
        waiting_tools: &["AskUserQuestion"],
        ..hook("PreToolUse", HookStatus::Running)
    },
    matched_hook("PostToolUse", "AskUserQuestion", HookStatus::Running),
    HookEvent {
        identity_field: Some(HookIdentityField::SessionId),
        usage: true,
        ..hook("UserPromptSubmit", HookStatus::Running)
    },
    usage_hook("Stop", Some(HookStatus::Idle)),
    usage_hook("StopFailure", Some(HookStatus::Idle)),
    matched_hook(
        "Notification",
        "permission_prompt|elicitation_dialog|agent_needs_input",
        HookStatus::Waiting,
    ),
    matched_hook(
        "Notification",
        "idle_prompt|agent_completed",
        HookStatus::Idle,
    ),
    hook("ElicitationResult", HookStatus::Running),
    usage_hook("PostCompact", None),
    usage_hook("SessionEnd", None),
];
```

Update `CODEX_HOOK_EVENTS`:

```rust
const CODEX_HOOK_EVENTS: &[HookEvent] = &[
    usage_hook("SessionStart", Some(HookStatus::Idle)),
    usage_hook("UserPromptSubmit", Some(HookStatus::Running)),
    hook("PreToolUse", HookStatus::Running),
    hook("PermissionRequest", HookStatus::Waiting),
    hook("PostToolUse", HookStatus::Running),
    usage_hook("Stop", Some(HookStatus::Idle)),
    usage_hook("PostCompact", None),
];
```

In `resolved_hook_events`, add `usage: event.usage && config.session.usage_tracking,` to the `ResolvedHookEvent` literal. In `append_configured_status_events` and in every other `ResolvedHookEvent { .. }` literal the compiler reports (for example `resolved_sidecar_hook_events` and tests), add `usage: false,`.

Replace `hook_install_required`:

```rust
pub(crate) fn hook_install_required(
    agent: &AgentDef,
    status_hooks_enabled: bool,
    usage_enabled: bool,
) -> bool {
    (status_hooks_enabled && (agent.hook_config.is_some() || agent.sidecar_hooks.is_some()))
        || agent.hook_config.as_ref().is_some_and(|hooks| {
            hooks
                .events
                .iter()
                .any(|event| event.identity_field.is_some() || (usage_enabled && event.usage))
        })
        || agent.sidecar_hooks.as_ref().is_some_and(|hooks| {
            hooks
                .events
                .iter()
                .any(|event| event.identity_field.is_some())
        })
}
```

Update its three call sites to pass `config.session.usage_tracking` (in `src/session/instance/hooks.rs` lines ~229 and ~284, where `config` is already resolved; in `src/tui/home/input.rs:2040`, read the same resolved config the surrounding code uses for `hooks_enabled`).

- [ ] **Step 4: Keep usage events when status hooks are off**

In `src/session/instance/hooks.rs::resolved_host_hook_events`:

```rust
    if !status_hooks_enabled {
        events.retain(|event| event.identity_field.is_some() || event.usage);
        for event in &mut events {
            event.status = None;
        }
    }
```

- [ ] **Step 5: Add the command builder in `src/hooks/command.rs`**

After `hook_command_session_id_host`:

```rust
/// Records the hook in the usage log. Host only: the sandbox has no `aoe`.
/// Output is discarded because Claude reads `SessionStart` stdout as context.
pub(crate) fn hook_command_usage_event() -> String {
    format!(
        "sh -c '[ -n \"$AOE_INSTANCE_ID\" ] || exit 0; \
         [ -n \"$AOE_HOOK_BIN\" ] || exit 0; \
         [ -x \"$AOE_HOOK_BIN\" ] || exit 0; \
         \"$AOE_HOOK_BIN\" __usage-event >/dev/null 2>&1; exit 0 # {AOE_HOOK_MARKER}'"
    )
}
```

- [ ] **Step 6: Emit it from `event_commands` in `src/hooks/json_settings.rs`**

```rust
/// Commands for one event: the identity extractor first (it must see stdin
/// first), then the usage recorder, then the status writer.
fn event_commands(event: &ResolvedHookEvent, target: HookInstallTarget) -> Vec<String> {
    let identity = event
        .identity_field
        .map(|field| hook_command_session_id(target, field));
    let usage = (event.usage && target == HookInstallTarget::Host)
        .then(super::command::hook_command_usage_event);
    let status = event
        .status
        .map(|status| status_command_for_event(status, &event.waiting_tools, target));
    identity.into_iter().chain(usage).chain(status).collect()
}
```

- [ ] **Step 7: Add a recognition test in `src/hooks/command.rs` tests**

```rust
    #[test]
    fn usage_command_is_recognized_as_aoe() {
        assert!(is_aoe_hook_command(&hook_command_usage_event()));
    }
```

- [ ] **Step 8: Run the hook and agent tests**

Run: `cargo test --lib hooks:: && cargo test --lib agents:: && cargo test --lib session::instance::hooks && cargo test --lib migrations::`
Expected: PASS. Tests that pin Claude's or Codex's exact event list or JSON output (for example in `src/hooks/json_settings.rs`, `src/hooks/codex.rs`, `src/agents.rs`, migrations v015/v017) may need their expectations extended with `PostCompact`/`SessionEnd` and the usage command; update them to the new intended output, and never loosen an assertion to make it pass. Also check `src/hooks/codex.rs` for any Codex event-name allowlist: `PostCompact` is already in `CODEX_HOOK_EVENT_NAMES`.

- [ ] **Step 9: Commit**

```bash
git add -A src/agents.rs src/hooks src/session/instance/hooks.rs src/tui/home/input.rs src/migrations
git commit -m "feat(usage): install usage hooks for Claude and Codex"
```

---

### Task 4: Pi usage reporting

**Files:**
- Modify: `assets/session/aoe-session-id.js`
- Modify: `src/session/instance/identity_sidecar.rs` (`identity_extension_launch`, host Pi branch)
- Test: `src/session/instance/identity_sidecar.rs` tests (or the existing test module that covers `identity_extension_launch`)

**Interfaces:**
- Consumes: CLI `__usage-event --agent pi` from Task 2; `SessionConfig.usage_tracking` from Task 1.
- Produces: env `AOE_USAGE_BIN`, `AOE_INSTANCE_ID`, `AOE_PROFILE` in the Pi extension environment on host launches when usage is enabled.

- [ ] **Step 1: Verify Pi's event API before editing**

Run:
```bash
F=$(dirname $(realpath $(command -v pi)))/core/extensions/types.d.ts
grep -nE 'type InputEventResult|interface (SessionStartEvent|SessionCompactEvent|SessionShutdownEvent|InputEvent|AgentSettledEvent)' -A6 "$F"
```
Expected: `session_start.reason`, `session_compact.reason`, `session_shutdown.reason`, `input.source`, `agent_settled` exist as the spec assumes, and returning `undefined` from an `input` handler means "continue unchanged". If `InputEventResult` requires an explicit value to continue, return that value from the `input` handler below.

- [ ] **Step 2: Write the failing Rust test for the launch env**

Find the existing tests for `identity_extension_launch` (`grep -rn "identity_extension_launch\|AOE_SESSION_ID_FILE" src --include='*.rs'`). Add a case next to them asserting that, for a host Pi instance where the extension path is used, the returned env string contains `AOE_USAGE_BIN=`, `AOE_INSTANCE_ID=<id>`, and `AOE_PROFILE=`, and that it omits `AOE_USAGE_BIN` when `usage_tracking` is false in the instance's resolved config. If no existing test reaches this branch (it depends on `pi_supports_extension_flag()`), extract the env-string formatting into a small pure function `fn pi_extension_env(sidecar: &Path, usage: Option<(&Path, &str, &str)>) -> String` and test that instead:

```rust
    #[test]
    fn pi_extension_env_adds_usage_vars_only_when_enabled() {
        let sidecar = std::path::Path::new("/tmp/aoe-hooks-1/abc/session_id");
        let bin = std::path::Path::new("/opt/aoe");
        let without = pi_extension_env(sidecar, None);
        assert!(without.starts_with("AOE_SESSION_ID_FILE="));
        assert!(!without.contains("AOE_USAGE_BIN"));
        let with = pi_extension_env(sidecar, Some((bin, "abc", "work")));
        for needle in ["AOE_USAGE_BIN=/opt/aoe", "AOE_INSTANCE_ID=abc", "AOE_PROFILE=work"] {
            assert!(with.contains(needle), "{with}");
        }
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --lib identity_sidecar`
Expected: FAIL (function missing).

- [ ] **Step 4: Implement the env string**

In `src/session/instance/identity_sidecar.rs`:

```rust
/// The Pi extension's environment: its identity sidecar, plus the usage
/// reporter when usage tracking is on (`bin`, instance id, profile).
fn pi_extension_env(sidecar: &Path, usage: Option<(&Path, &str, &str)>) -> String {
    let mut env = format!(
        "AOE_SESSION_ID_FILE={} ",
        shell_escape(&sidecar.to_string_lossy())
    );
    if let Some((bin, instance_id, profile)) = usage {
        env.push_str(&format!(
            "AOE_USAGE_BIN={} AOE_INSTANCE_ID={} AOE_PROFILE={} ",
            shell_escape(&bin.to_string_lossy()),
            shell_escape(instance_id),
            shell_escape(profile),
        ));
    }
    env
}
```

In the host branch of `identity_extension_launch`, replace the inline `format!("AOE_SESSION_ID_FILE={} ", ...)` with:

```rust
            let profile = self.effective_profile();
            let usage_bin = crate::session::config::profile_config::resolve_config_or_warn(&profile)
                .session
                .usage_tracking
                .then(std::env::current_exe)
                .and_then(Result::ok);
            return Some((
                format!(" -e {}", shell_escape(&extension.to_string_lossy())),
                pi_extension_env(
                    &sidecar,
                    usage_bin
                        .as_deref()
                        .map(|bin| (bin, self.id.as_str(), profile.as_str())),
                ),
            ));
```

(Adapt imports: `std::path::Path` and `shell_escape` are already in scope in this file; check before adding.)

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --lib identity_sidecar && cargo test --lib session::instance`
Expected: PASS. If a test pins the exact extension env string, extend its expectation.

- [ ] **Step 6: Report usage from the extension**

Edit `assets/session/aoe-session-id.js`. Add the import and a reporter, report on the listed events, and keep the identity logic unchanged:

```js
// Publish the native ID and transcript path without requiring a materialized file,
// and report context resets, prompts, and turns to AoE's usage log.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, renameSync, unlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export default function (pi) {
  const idTarget = process.env.AOE_SESSION_ID_FILE;
  const usageBin = process.env.AOE_USAGE_BIN;
  const rootOnly = process.env.AOE_SESSION_ROOT_ONLY === "1";
  const source = process.env.AOE_SESSION_SOURCE;
  if (!rootOnly && source !== undefined && !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(source)) return;
  const suffix = !rootOnly && source ? "." + source : "";
  const sessionId = (ctx) => {
    try {
      return ctx?.sessionManager?.getSessionId?.() || undefined;
    } catch {
      return undefined;
    }
  };
  // Fire and forget: the agent never waits on the usage log.
  const report = (payload) => {
    if (!usageBin || !process.env.AOE_INSTANCE_ID) return;
    try {
      const child = spawn(usageBin, ["__usage-event", "--agent", "pi"], {
        stdio: ["pipe", "ignore", "ignore"],
        detached: true,
      });
      child.on("error", () => {});
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
      child.unref();
    } catch {
      // never block the agent
    }
  };
  const writeAtomic = (target, value) => {
    /* unchanged */
  };

  pi.on("session_start", async (event, ctx) => {
    report({ hook_event_name: "session_start", reason: event?.reason, session_id: sessionId(ctx) });
    if (!idTarget) return;
    /* existing body unchanged */
  });
  pi.on("session_shutdown", async (event, ctx) => {
    report({ hook_event_name: "session_shutdown", reason: event?.reason, session_id: sessionId(ctx) });
  });
  pi.on("session_compact", async (event, ctx) => {
    report({ hook_event_name: "session_compact", reason: event?.reason, session_id: sessionId(ctx) });
  });
  pi.on("input", async (event, ctx) => {
    report({ hook_event_name: "input", source: event?.source, session_id: sessionId(ctx) });
  });
  pi.on("agent_settled", async (_event, ctx) => {
    report({ hook_event_name: "agent_settled", session_id: sessionId(ctx) });
  });
}
```

Keep the real bodies where this snippet says `/* unchanged */`; do not paste those comments.

- [ ] **Step 7: Syntax-check the extension and exercise the reporter with a stub binary**

```bash
node --check assets/session/aoe-session-id.js
D=$(mktemp -d); printf '#!/bin/sh\ncat > "%s/got.json"\n' "$D" > "$D/aoe"; chmod +x "$D/aoe"
cat > "$D/run.mjs" <<EOF
import ext from "$(pwd)/assets/session/aoe-session-id.js";
const handlers = {};
ext({ on: (name, fn) => { handlers[name] = fn; } });
await handlers.session_compact({ reason: "threshold" }, { sessionManager: { getSessionId: () => "sid" } });
EOF
AOE_USAGE_BIN="$D/aoe" AOE_INSTANCE_ID=abc node "$D/run.mjs"
for i in 1 2 3 4 5 6 7 8 9 10; do [ -s "$D/got.json" ] && break; sleep 0.2; done; cat "$D/got.json"
```

Expected: `{"hook_event_name":"session_compact","reason":"threshold","session_id":"sid"}`. (The loop polls for the detached child's output with a deadline; it is a manual check, not a committed test.)

- [ ] **Step 8: Check for an existing JS test of the extension**

Run: `grep -rln "aoe-session-id" tests web src --include='*.rs' --include='*.ts' --include='*.mjs'`. If a test pins the extension source (for example a hash or snapshot), update it.

- [ ] **Step 9: Commit**

```bash
git add assets/session/aoe-session-id.js src/session/instance/identity_sidecar.rs
git commit -m "feat(usage): report Pi context resets, prompts, and turns"
```

---

### Task 5: Record aoe lifecycle events

**Files:**
- Modify: `src/usage/mod.rs` (add `record_lifecycle`)
- Modify: `src/session/builder.rs` (`build_instance` success), `src/cli/add.rs` (if it creates instances without `build_instance`), `src/session/restart.rs` (`perform_restart`), `src/session/deletion.rs` (successful deletion)

**Interfaces:**
- Consumes: `UsageStore::open`, `UsageEvent`, `UsageKind`; `SessionConfig.usage_tracking`.
- Produces: `crate::usage::record_lifecycle(instance: &crate::session::Instance, kind: UsageKind)` and the testable inner `record_lifecycle_at(db: &Path, instance: &Instance, kind: UsageKind, enabled: bool) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing test in `src/usage/mod.rs` tests**

```rust
    #[test]
    fn lifecycle_rows_carry_instance_identity_and_respect_the_switch() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("usage.db");
        let mut instance = crate::session::Instance::new("t", "/tmp/project");
        instance.tool = "claude".into();
        record_lifecycle_at(&db, &instance, UsageKind::InstanceCreated, true).unwrap();
        record_lifecycle_at(&db, &instance, UsageKind::InstanceDeleted, false).unwrap();
        let events = UsageStore::open(&db).unwrap().events_for_instance(&instance.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, UsageKind::InstanceCreated);
        assert_eq!(events[0].agent.as_deref(), Some("claude"));
        assert_eq!(events[0].profile, Some(instance.effective_profile()));
    }
```

Use whatever `Instance` constructor tests elsewhere use (check `src/session/instance/test_helpers.rs`) if `Instance::new(title, path)` differs.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib usage::tests`
Expected: compile error (`record_lifecycle_at` missing).

- [ ] **Step 3: Implement**

In `src/usage/mod.rs`:

```rust
/// Best-effort: a lifecycle row never fails the operation that produced it.
pub fn record_lifecycle(instance: &crate::session::Instance, kind: UsageKind) {
    let enabled = crate::session::config::profile_config::resolve_config_or_warn(
        &instance.effective_profile(),
    )
    .session
    .usage_tracking;
    let result = db_path().and_then(|db| record_lifecycle_at(&db, instance, kind, enabled));
    if let Err(e) = result {
        tracing::debug!(target: "usage", "lifecycle event dropped: {e}");
    }
}

fn record_lifecycle_at(
    db: &std::path::Path,
    instance: &crate::session::Instance,
    kind: UsageKind,
    enabled: bool,
) -> anyhow::Result<()> {
    if !enabled {
        return Ok(());
    }
    UsageStore::open(db)?.insert(&UsageEvent {
        id: 0,
        occurred_at: Utc::now(),
        instance_id: instance.id.clone(),
        profile: Some(instance.effective_profile()),
        agent: Some(instance.tool.clone()),
        kind,
        detail: None,
        agent_session_id: instance.agent_session_id.clone(),
    })?;
    Ok(())
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib usage::`
Expected: PASS.

- [ ] **Step 5: Call it at the three lifecycle points**

- `src/session/builder.rs::build_instance`: on the success path, just before returning the built instance, `crate::usage::record_lifecycle(&instance, crate::usage::UsageKind::InstanceCreated);`. Read the function end first; if it returns a wrapper struct, record using the instance inside it.
- `src/cli/add.rs`: check whether `aoe add` goes through `build_instance`; if it constructs and saves an `Instance` directly, add the same call after the save succeeds.
- `src/session/restart.rs::perform_restart`: after `outcome` is computed, `if launched_agent(&outcome) { crate::usage::record_lifecycle(&instance, crate::usage::UsageKind::InstanceRestarted); }`.
- `src/session/deletion.rs`: in the core deletion routine, after the instance is confirmed removed (read `perform_deletion_core` and record only on its success result), `crate::usage::record_lifecycle(&request.instance, crate::usage::UsageKind::InstanceDeleted);`.

- [ ] **Step 6: Run the affected module tests**

Run: `cargo test --lib session::restart && cargo test --lib session::deletion && cargo test --lib session::builder`
Expected: PASS. These tests run with an isolated app dir; if one asserts the app dir contents exactly, account for `usage.db`.

- [ ] **Step 7: Commit**

```bash
git add src/usage/mod.rs src/session/builder.rs src/session/restart.rs src/session/deletion.rs src/cli/add.rs
git commit -m "feat(usage): record session create, restart, and delete"
```

---

### Task 6: TUI overlay

**Files:**
- Create: `src/tui/components/usage_overlay.rs`, `src/tui/usage_poller.rs`
- Modify: `src/tui/components/mod.rs` (`pub(crate) mod usage_overlay;` following the file's existing visibility style), `src/tui/mod.rs` (`mod usage_poller;` following existing poller declarations), `src/tui/home/mod.rs` (fields), `src/tui/home/lifecycle.rs` (init), `src/tui/home/config_refresh.rs` (refresh flag), `src/tui/home/status.rs` (request/apply), `src/tui/app.rs` (tick), `src/tui/home/render.rs` (draw after `render_preview`)

**Interfaces:**
- Consumes: `crate::usage::{UsageStore, UsageSummary}`; `SessionConfig.show_usage_overlay`.
- Produces: `usage_overlay::render_usage_overlay(frame: &mut Frame, pane: Rect, summary: &UsageSummary, created_at: DateTime<Utc>, now: DateTime<Utc>, theme: &Theme)`; `usage_overlay::big_digits(n: u32) -> [String; 3]`; `usage_overlay::format_duration_short(d: chrono::Duration) -> String`; `usage_overlay::overlay_lines(summary, created_at, now) -> Vec<String>`; `UsagePoller::{new, request_refresh(String), try_recv_updates() -> Result<(String, Option<UsageSummary>), TryRecvError>}`.

- [ ] **Step 1: Write the failing overlay tests in `src/tui/components/usage_overlay.rs`**

```rust
//! The session's context-reset count in big red digits, with a usage
//! breakdown, drawn over the top-right corner of the preview pane.

use chrono::{DateTime, Duration, Utc};
use ratatui::prelude::*;
use ratatui::widgets::*;
use unicode_width::UnicodeWidthStr;

use crate::tui::styles::Theme;
use crate::usage::UsageSummary;

/// 3x5 pixel digits, one bit per pixel, top row first, left pixel highest.
const DIGIT_PIXELS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

pub(crate) fn big_digits(n: u32) -> [String; 3] {
    todo!()
}

pub(crate) fn format_duration_short(d: Duration) -> String {
    todo!()
}

pub(crate) fn overlay_lines(
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn digits_pack_two_pixel_rows_per_line() {
        assert_eq!(big_digits(0), ["█▀█", "█ █", "▀▀▀"]);
        assert_eq!(big_digits(1), ["▄█ ", " █ ", "▀▀▀"]);
        assert_eq!(big_digits(47), ["█ █ ▀▀█", "▀▀█   █", "  ▀   ▀"]);
    }

    #[test]
    fn durations_use_the_two_largest_units() {
        let cases = [
            (Duration::seconds(20), "<1m"),
            (Duration::minutes(42), "42m"),
            (Duration::minutes(125), "2h 5m"),
            (Duration::hours(76), "3d 4h"),
            (Duration::seconds(-5), "<1m"),
        ];
        for (d, want) in cases {
            assert_eq!(format_duration_short(d), want);
        }
    }

    #[test]
    fn lines_show_breakdown_context_and_age() {
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary {
            resets: 17,
            clears: 12,
            compactions: 3,
            compactions_auto: 2,
            compactions_manual: 1,
            resumes: 2,
            context_started_at: Some(now - Duration::minutes(42)),
            context_prompts: 18,
            context_turns: 17,
            ..UsageSummary::default()
        };
        assert_eq!(
            overlay_lines(&summary, now - Duration::hours(76), now),
            vec![
                "clr 12 · cmp 3 (2a) · rsm 2".to_string(),
                "ctx 42m · 18p 17t".to_string(),
                "age 3d 4h".to_string(),
            ]
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Add `pub(crate) mod usage_overlay;` to `src/tui/components/mod.rs`, then run: `cargo test --lib tui::components::usage_overlay`
Expected: FAIL (`todo!()`).

- [ ] **Step 3: Implement digits, durations, lines, and the renderer**

```rust
pub(crate) fn big_digits(n: u32) -> [String; 3] {
    let digits: Vec<usize> = n
        .to_string()
        .bytes()
        .map(|b| (b - b'0') as usize)
        .collect();
    let pixel = |d: usize, row: usize, col: usize| {
        row < 5 && DIGIT_PIXELS[d][row] & (0b100 >> col) != 0
    };
    std::array::from_fn(|line| {
        digits
            .iter()
            .map(|&d| {
                (0..3)
                    .map(|col| match (pixel(d, line * 2, col), pixel(d, line * 2 + 1, col)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

pub(crate) fn format_duration_short(d: Duration) -> String {
    let minutes = d.num_minutes();
    match minutes {
        m if m < 1 => "<1m".to_string(),
        m if m < 60 => format!("{m}m"),
        m if m < 24 * 60 => format!("{}h {}m", m / 60, m % 60),
        m => format!("{}d {}h", m / (24 * 60), (m / 60) % 24),
    }
}

pub(crate) fn overlay_lines(
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let context_age = summary
        .context_started_at
        .map(|at| format_duration_short(now - at))
        .unwrap_or_else(|| "?".to_string());
    vec![
        format!(
            "clr {} · cmp {} ({}a) · rsm {}",
            summary.clears, summary.compactions, summary.compactions_auto, summary.resumes
        ),
        format!(
            "ctx {context_age} · {}p {}t",
            summary.context_prompts, summary.context_turns
        ),
        format!("age {}", format_duration_short(now - created_at)),
    ]
}

/// Draws nothing when the pane cannot fit the box with room to spare.
pub(crate) fn render_usage_overlay(
    frame: &mut Frame,
    pane: Rect,
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    theme: &Theme,
) {
    let digits = big_digits(summary.resets);
    let lines = overlay_lines(summary, created_at, now);
    let content_width = digits
        .iter()
        .chain(lines.iter())
        .map(|line| line.width())
        .max()
        .unwrap_or(0) as u16;
    let width = content_width + 4;
    let height = (digits.len() + lines.len()) as u16 + 2;
    if pane.width < width + 10 || pane.height < height + 2 {
        return;
    }
    let area = Rect {
        x: pane.right() - width - 1,
        y: pane.y,
        width,
        height,
    };
    let number = Style::default().fg(theme.error).bold();
    let detail = Style::default().fg(theme.text);
    let text: Vec<Line> = digits
        .into_iter()
        .map(|row| Line::styled(row, number))
        .chain(lines.into_iter().map(|row| Line::styled(row, detail)))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.error))
        .title(Span::styled(" resets ", number))
        .padding(Padding::horizontal(1));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(text).block(block), area);
}
```

Check `Theme` field names in `src/tui/styles/` (`error`, `text` are used elsewhere in this file's neighbors, e.g. `diagnostics.rs` uses `theme.text`); use the existing red token if `error` is named differently.

- [ ] **Step 4: Add a render test**

```rust
    #[test]
    fn overlay_draws_top_right_and_hides_when_cramped() {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let now = Utc.with_ymd_and_hms(2026, 9, 29, 12, 0, 0).unwrap();
        let summary = UsageSummary { resets: 7, ..UsageSummary::default() };
        let draw = |w: u16, h: u16| {
            let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| render_usage_overlay(f, f.area(), &summary, now, now, &theme))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let wide = draw(80, 20);
        let top_right: String = (40..80).map(|x| wide[(x, 0)].symbol().to_string()).collect();
        assert!(top_right.contains("resets"), "{top_right}");
        let cramped = draw(20, 5);
        assert!(cramped.content().iter().all(|c| c.symbol() == " "));
    }
```

If `Theme::default()` does not exist, construct the theme the way `src/tui/components/diagnostics.rs` tests do.

- [ ] **Step 5: Run to verify all overlay tests pass**

Run: `cargo test --lib tui::components::usage_overlay`
Expected: PASS (4 tests).

- [ ] **Step 6: Create the poller `src/tui/usage_poller.rs`**

```rust
//! Background loader for the selected session's usage summary, so SQLite
//! reads stay off the render loop.

use crate::tui::worker::Worker;
use crate::usage::UsageSummary;

pub struct UsagePoller {
    worker: Worker<String, (String, Option<UsageSummary>)>,
}

impl UsagePoller {
    pub fn new() -> Self {
        Self {
            worker: Worker::spawn("aoe-usage-poller", |instance_id: String| {
                let summary = crate::usage::UsageStore::open_default()
                    .and_then(|store| store.summary_for(&instance_id))
                    .ok();
                (instance_id, summary)
            }),
        }
    }

    pub fn request_refresh(&self, instance_id: String) {
        self.worker.request(instance_id);
    }

    pub fn try_recv_updates(
        &self,
    ) -> Result<(String, Option<UsageSummary>), std::sync::mpsc::TryRecvError> {
        self.worker.try_recv()
    }
}

impl Default for UsagePoller {
    fn default() -> Self {
        Self::new()
    }
}
```

Match `Worker::spawn`'s actual signature in `src/tui/worker.rs` (the metrics poller passes a `move |input| ...` closure; mirror it). Declare the module in `src/tui/mod.rs` next to `metrics_poller`.

- [ ] **Step 7: Wire state, config, requests, and the tick**

`src/tui/home/mod.rs`, next to `metrics_poller`:

```rust
    pub(super) usage_poller: super::usage_poller::UsagePoller,
    pub(super) pending_usage_refresh: bool,
    /// Summary for one instance id; drawn only while that id is selected.
    pub(super) usage_summary: Option<(String, crate::usage::UsageSummary)>,
    pub(super) show_usage_overlay: bool,
```

`src/tui/home/lifecycle.rs`, next to `metrics_poller: ...::new(),`:

```rust
            usage_poller: crate::tui::usage_poller::UsagePoller::new(),
            pending_usage_refresh: false,
            usage_summary: None,
            show_usage_overlay: resolved.session.show_usage_overlay,
```

`src/tui/home/config_refresh.rs`, next to `self.show_diagnostics = ...`:

```rust
        self.show_usage_overlay = config.session.show_usage_overlay;
```

`src/tui/home/status.rs`, after `apply_metrics_updates`:

```rust
    /// Request the selected session's usage summary while the overlay is on.
    pub fn request_usage_refresh(&mut self) {
        if !self.show_usage_overlay || self.pending_usage_refresh {
            return;
        }
        if let Some(id) = self.selected_session.clone() {
            self.usage_poller.request_refresh(id);
            self.pending_usage_refresh = true;
        }
    }

    /// Apply a loaded usage summary; true when the overlay needs a repaint.
    pub fn apply_usage_updates(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;

        match self.usage_poller.try_recv_updates() {
            Ok((id, summary)) => {
                self.pending_usage_refresh = false;
                let next = summary.map(|summary| (id, summary));
                let changed = next != self.usage_summary;
                self.usage_summary = next;
                changed
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                tracing::error!(target: "tui.home", "usage poller worker gone; respawning");
                self.usage_poller = crate::tui::usage_poller::UsagePoller::new();
                self.pending_usage_refresh = false;
                false
            }
        }
    }
```

`src/tui/app.rs`, next to the metrics tick (add `const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);` beside `METRICS_SAMPLE_INTERVAL`, and a `let mut last_usage_refresh = std::time::Instant::now();` beside `last_metrics_sample`):

```rust
            if last_usage_refresh.elapsed() >= USAGE_REFRESH_INTERVAL {
                self.home.request_usage_refresh();
                last_usage_refresh = std::time::Instant::now();
            }
            refresh_needed |= self.home.apply_usage_updates();
```

Also request immediately on selection change so the overlay does not lag two seconds: find the selection-change path (`grep -n "fn select_session\|selected_session =" src/tui/home/*.rs`) and call `self.request_usage_refresh()` after the selection updates, only in the central setter if one exists.

- [ ] **Step 8: Draw it after the preview**

In `src/tui/home/render.rs`, after each of the two `self.render_preview(frame, preview_area, theme);` calls, add `self.render_usage_overlay(frame, theme);`, and add the method to the same `impl HomeView`:

```rust
    fn render_usage_overlay(&self, frame: &mut Frame, theme: &Theme) {
        if !self.show_usage_overlay || self.system_health_open {
            return;
        }
        let Some(selected) = self.selected_session.as_deref() else {
            return;
        };
        let Some((id, summary)) = &self.usage_summary else {
            return;
        };
        if id != selected || summary.last_event_at.is_none() {
            return;
        }
        let Some(instance) = self.get_instance(selected) else {
            return;
        };
        crate::tui::components::usage_overlay::render_usage_overlay(
            frame,
            self.preview_pane_area,
            summary,
            instance.created_at,
            chrono::Utc::now(),
            theme,
        );
    }
```

- [ ] **Step 9: Build and run TUI tests**

Run: `cargo test --lib tui::`
Expected: PASS. Existing render snapshot tests use isolated app dirs with no usage rows, so the overlay stays hidden; if one fails because of the new fields, fix the test fixture construction, not the assertion.

- [ ] **Step 10: Commit**

```bash
git add src/tui
git commit -m "feat(tui): show session context resets in a preview overlay"
```

---

### Task 7: Web API and overlay

**Files:**
- Create: `src/server/api/sessions/usage.rs`, `web/src/lib/usage.ts`, `web/src/lib/__tests__/usage.test.ts` (or colocated `web/src/lib/usage.test.ts`, matching neighbors such as `idleDecay.test.ts`), `web/src/components/UsageOverlay.tsx`
- Modify: `src/server/api/sessions/mod.rs` (declare module, re-export handler the way siblings are), `src/server/api/mod.rs` (if handlers are re-exported there), `src/server/router.rs` (route), `web/src/App.tsx` (context provider), `web/src/components/TerminalSessionStack.tsx` (mount), `web/tests/coverage-matrix.json`

**Interfaces:**
- Consumes: `crate::usage::{UsageStore, UsageSummary}`; `session.show_usage_overlay`; `find_instance(state, id)` in `src/server/api/mod.rs`.
- Produces: `GET /api/sessions/{id}/usage` → `UsageSummary` JSON (camelCase); TS `UsageSummary`, `fetchSessionUsage(id)`, `usageLines(summary, createdAt, now)`, `formatDurationShort(ms)`, `UsageOverlayEnabledContext`, `parseUsageOverlayEnabled`, `useUsageOverlayEnabled`.

- [ ] **Step 1: Write the handler with a failing test**

`src/server/api/sessions/usage.rs`:

```rust
//! Per-session usage summary for the web overlay.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::server::state::AppState;
use crate::usage::UsageSummary;

pub async fn session_usage(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(instance) = super::super::find_instance(&state, &id).await else {
        return super::super::bare_not_found();
    };
    let summary = tokio::task::spawn_blocking(move || load_summary(&instance.id))
        .await
        .unwrap_or_default();
    Json(summary).into_response()
}

/// A missing or unreadable log reads as "no events".
fn load_summary(instance_id: &str) -> UsageSummary {
    crate::usage::UsageStore::open_default()
        .and_then(|store| store.summary_for(instance_id))
        .unwrap_or_default()
}
```

Adjust the `find_instance` and `bare_not_found` paths to where they actually live (`read_output` in `src/server/api/sessions/send.rs` uses both; import them the same way). Add a test in the existing server API test module (`src/server/api/sessions/tests.rs`) using the same harness the neighboring GET tests use: create a session, insert one `context_start/clear` row into the isolated app dir's `usage.db` via `UsageStore::open_default()`, `GET /api/sessions/{id}/usage`, assert status 200 and `body["clears"] == 1` and `body["resets"] == 1`; and assert an unknown id returns 404.

- [ ] **Step 2: Register the route**

In `src/server/router.rs`, next to `/api/sessions/{id}/output`:

```rust
        .route("/api/sessions/{id}/usage", get(api::session_usage))
```

Export `session_usage` so `api::session_usage` resolves (follow how `read_output` is exported).

- [ ] **Step 3: Run the server test**

Run: `cargo test --features web --lib server::api::sessions`
Expected: PASS (after implementing; if you wrote the test first, it fails with 404 until the route is registered).

- [ ] **Step 4: Write the failing web lib test**

`web/src/lib/usage.test.ts` (match the location convention of `idleDecay.test.ts`):

```ts
import { describe, expect, it } from "vitest";
import { formatDurationShort, parseUsageOverlayEnabled, usageLines, type UsageSummary } from "./usage";

const base: UsageSummary = {
  resets: 0, clears: 0, compactions: 0, compactionsAuto: 0, compactionsManual: 0, resumes: 0,
  prompts: 0, turns: 0, turnErrors: 0, contextStartedAt: null, contextPrompts: 0, contextTurns: 0,
  trackedSince: null, lastEventAt: null,
};

describe("usage overlay text", () => {
  it("matches the TUI duration format", () => {
    const min = 60_000;
    expect([20_000, 42 * min, 125 * min, 76 * 60 * min, -5_000].map(formatDurationShort)).toEqual([
      "<1m", "42m", "2h 5m", "3d 4h", "<1m",
    ]);
  });

  it("builds the breakdown lines", () => {
    const now = Date.parse("2026-09-29T12:00:00Z");
    const summary = {
      ...base, resets: 17, clears: 12, compactions: 3, compactionsAuto: 2, compactionsManual: 1, resumes: 2,
      contextStartedAt: new Date(now - 42 * 60_000).toISOString(), contextPrompts: 18, contextTurns: 17,
    };
    const created = new Date(now - 76 * 3_600_000).toISOString();
    expect(usageLines(summary, created, now)).toEqual([
      "clr 12 · cmp 3 (2a) · rsm 2", "ctx 42m · 18p 17t", "age 3d 4h",
    ]);
  });

  it("defaults the overlay on", () => {
    expect(parseUsageOverlayEnabled(null)).toBe(true);
    expect(parseUsageOverlayEnabled({ session: { show_usage_overlay: false } } as never)).toBe(false);
  });
});
```

Run: `cd web && npx vitest run src/lib/usage.test.ts`
Expected: FAIL (module missing).

- [ ] **Step 5: Implement `web/src/lib/usage.ts`**

```ts
import { fetchJson } from "./api";
import { sessionFlagGate } from "./sessionFlagGate";

/** Mirrors `crate::usage::UsageSummary`. */
export interface UsageSummary {
  resets: number;
  clears: number;
  compactions: number;
  compactionsAuto: number;
  compactionsManual: number;
  resumes: number;
  prompts: number;
  turns: number;
  turnErrors: number;
  contextStartedAt: string | null;
  contextPrompts: number;
  contextTurns: number;
  trackedSince: string | null;
  lastEventAt: string | null;
}

/** On by default, matching `session.show_usage_overlay`. */
const gate = sessionFlagGate("show_usage_overlay", true);
export const UsageOverlayEnabledContext = gate.Context;
export const parseUsageOverlayEnabled = gate.parse;
export const useUsageOverlayEnabled = gate.use;

export function fetchSessionUsage(id: string): Promise<UsageSummary | null> {
  return fetchJson<UsageSummary>(`/api/sessions/${encodeURIComponent(id)}/usage`);
}

/** Mirrors the TUI's `format_duration_short`. */
export function formatDurationShort(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  return `${Math.floor(minutes / (24 * 60))}d ${Math.floor(minutes / 60) % 24}h`;
}

/** Mirrors the TUI's `overlay_lines`. */
export function usageLines(summary: UsageSummary, createdAt: string, now: number): string[] {
  const ctx = summary.contextStartedAt
    ? formatDurationShort(now - Date.parse(summary.contextStartedAt))
    : "?";
  return [
    `clr ${summary.clears} · cmp ${summary.compactions} (${summary.compactionsAuto}a) · rsm ${summary.resumes}`,
    `ctx ${ctx} · ${summary.contextPrompts}p ${summary.contextTurns}t`,
    `age ${formatDurationShort(now - Date.parse(createdAt))}`,
  ];
}
```

Check whether `fetchJson` is exported from `./api`; if not, export it (it is used by `fetchSystemHealth`) or add `fetchSessionUsage` to `api.ts` next to `fetchSystemHealth` instead. Check `parse`'s parameter type in `sessionFlagGate.ts` and adjust the test's cast accordingly.

Run: `cd web && npx vitest run src/lib/usage.test.ts`
Expected: PASS.

- [ ] **Step 6: Implement `web/src/components/UsageOverlay.tsx`**

```tsx
import { useEffect, useState } from "react";
import type { SessionResponse } from "../lib/types";
import { fetchSessionUsage, usageLines, useUsageOverlayEnabled, type UsageSummary } from "../lib/usage";

const POLL_MS = 5_000;

/** The session's context-reset count, big and red, over the terminal's top-right corner. */
export function UsageOverlay({ session }: { session: SessionResponse }) {
  const enabled = useUsageOverlayEnabled();
  const [summary, setSummary] = useState<UsageSummary | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const load = () => {
      if (document.visibilityState !== "visible") return;
      void fetchSessionUsage(session.id).then((next) => {
        if (!cancelled) setSummary(next);
      });
    };
    load();
    const timer = window.setInterval(load, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [enabled, session.id]);

  if (!enabled || !summary?.lastEventAt) return null;
  return (
    <div
      className="pointer-events-none absolute top-2 right-3 z-10 rounded-md bg-black/60 px-3 py-2 text-right"
      data-testid="usage-overlay"
      title="Context resets: clears + compactions + resumes"
    >
      <div className="text-5xl font-black leading-none text-red-500 tabular-nums">{summary.resets}</div>
      {usageLines(summary, session.created_at, Date.now()).map((line) => (
        <div key={line} className="mt-1 font-mono text-[11px] text-white/80">
          {line}
        </div>
      ))}
    </div>
  );
}
```

Prefer the project's theme tokens over raw Tailwind colors if `SystemHealthStrip.tsx` or `DESIGN.md` defines a red/danger and overlay surface token; keep the number red and large.

- [ ] **Step 7: Provide the setting and mount the overlay**

`web/src/App.tsx`: mirror the system-health wiring exactly: import `parseUsageOverlayEnabled, UsageOverlayEnabledContext` from `./lib/usage`, add `const [usageOverlayEnabled, setUsageOverlayEnabled] = useState(true);`, call `setUsageOverlayEnabled(parseUsageOverlayEnabled(settings));` next to `setSystemHealthEnabled(...)`, and wrap the same subtree with `<UsageOverlayEnabledContext.Provider value={usageOverlayEnabled}>` inside the `SystemHealthEnabledContext.Provider`.

`web/src/components/TerminalSessionStack.tsx`: inside each session's absolute wrapper div, after `<TerminalView ... />`, add `{selected && <UsageOverlay session={session} />}` and import it.

- [ ] **Step 8: Add a component test and the coverage entry**

Add `web/src/components/__tests__/UsageOverlay.test.tsx` using the project's MSW setup (see `SystemHealthStrip.test.tsx` for the server and render helpers): mock `GET /api/sessions/:id/usage` to return a summary with `resets: 7` and a `lastEventAt`, render `<UsageOverlay session={fixtureSession} />` inside `UsageOverlayEnabledContext.Provider value={true}`, and assert `await screen.findByText("7")` plus `getByTestId("usage-overlay")`; render again with the provider `false` and assert `queryByTestId("usage-overlay")` is null.

Add an entry to `web/tests/coverage-matrix.json` for the sessions surface pointing at the new Vitest file, following the file's existing entry shape.

- [ ] **Step 9: Run web checks**

```bash
cd web && npm run test:unit -- --run src/lib/usage.test.ts src/components/__tests__/UsageOverlay.test.tsx && npx tsc -b && npm run lint && npm run format:check
```

Expected: all pass. Run `npm run format` if the format check fails, then re-check.

- [ ] **Step 10: Commit**

```bash
git add src/server web/src web/tests/coverage-matrix.json
git commit -m "feat(web): show session context resets over the terminal"
```

---

### Task 8: `aoe usage` CLI, report, and Ledger contract

**Files:**
- Create: `src/usage/report.rs`, `src/cli/usage.rs`, `docs/development/usage-events.md`
- Modify: `src/usage/mod.rs` (`mod report; pub use report::{build_report, UsageReport};`), `src/cli/mod.rs`, `src/cli/definition.rs` (variant, `command_name` → `"usage"`), `src/main.rs` (dispatch alongside other normal commands)

**Interfaces:**
- Consumes: `UsageStore::{open_default, events_since, events_for_instance, events_after, summary_for}`, `summarize`, `is_context_boundary`.
- Produces: `crate::usage::UsageReport` (Serialize, snake_case), `crate::usage::build_report(events: &[UsageEvent]) -> UsageReport`; CLI `aoe usage [--since DUR] [--json]`, `aoe usage show <SESSION> [--json]`, `aoe usage export [--after-id N] [--limit N]`.

- [ ] **Step 1: Write the failing report test in `src/usage/report.rs`**

```rust
//! Cross-session aggregates for `aoe usage`.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{is_context_boundary, summarize, UsageEvent, UsageKind};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageReport {
    pub instances: u32,
    pub contexts: u32,
    pub clears: u32,
    pub compactions: u32,
    pub compactions_auto: u32,
    pub compactions_manual: u32,
    pub resumes: u32,
    pub prompts: u32,
    pub turns: u32,
    pub turn_errors: u32,
    pub median_prompts_per_context: Option<f64>,
    pub median_context_minutes: Option<f64>,
    pub median_resets_per_instance: Option<f64>,
    pub max_resets_per_instance: u32,
    /// Instance ids with the most resets, highest first, at most ten.
    pub top: Vec<(String, u32)>,
}

pub fn build_report(events: &[UsageEvent]) -> UsageReport {
    todo!()
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::summary::tests::ev;

    fn on(instance: &str, mut event: UsageEvent) -> UsageEvent {
        event.instance_id = instance.into();
        event
    }

    #[test]
    fn aggregates_contexts_and_instances() {
        use UsageKind::*;
        let events = vec![
            on("a", ev(0, ContextStart, Some("startup"))),
            on("a", ev(1, Prompt, None)),
            on("a", ev(2, Prompt, None)),
            on("a", ev(10, ContextStart, Some("clear"))),
            on("a", ev(11, Prompt, None)),
            on("a", ev(20, Compact, Some("auto"))),
            on("a", ev(20, ContextStart, Some("compact"))),
            on("a", ev(26, TurnEnd, None)),
            on("b", ev(0, ContextStart, Some("startup"))),
            on("b", ev(4, Prompt, None)),
        ];
        let r = build_report(&events);
        assert_eq!((r.instances, r.contexts), (2, 4));
        assert_eq!((r.clears, r.compactions, r.compactions_auto, r.resumes), (1, 1, 1, 0));
        assert_eq!((r.prompts, r.turns), (4, 1));
        // Contexts: a[2 prompts, 10m], a[1, 10m], a[0, 6m], b[1, 4m].
        assert_eq!(r.median_prompts_per_context, Some(1.0));
        assert_eq!(r.median_context_minutes, Some(8.0));
        assert_eq!(r.median_resets_per_instance, Some(1.0));
        assert_eq!(r.max_resets_per_instance, 2);
        assert_eq!(r.top, vec![("a".to_string(), 2), ("b".to_string(), 0)]);
    }

    #[test]
    fn median_handles_even_odd_and_empty() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }
}
```

Context duration rule (used by the test): a context runs from its boundary to the next boundary of the same instance, or to that instance's last event for the final context.

- [ ] **Step 2: Run to verify it fails**

Add `mod report; pub use report::{build_report, UsageReport};` to `src/usage/mod.rs`, then run: `cargo test --lib usage::report`
Expected: FAIL (`todo!()`).

- [ ] **Step 3: Implement `build_report`**

```rust
pub fn build_report(events: &[UsageEvent]) -> UsageReport {
    let mut by_instance: BTreeMap<&str, Vec<&UsageEvent>> = BTreeMap::new();
    for event in events {
        by_instance.entry(&event.instance_id).or_default().push(event);
    }
    let mut report = UsageReport::default();
    let mut prompts_per_context = Vec::new();
    let mut context_minutes = Vec::new();
    let mut resets = Vec::new();
    for (instance_id, events) in &by_instance {
        let owned: Vec<UsageEvent> = events.iter().map(|e| (*e).clone()).collect();
        let summary = summarize(&owned);
        report.instances += 1;
        report.clears += summary.clears;
        report.compactions += summary.compactions;
        report.compactions_auto += summary.compactions_auto;
        report.compactions_manual += summary.compactions_manual;
        report.resumes += summary.resumes;
        report.prompts += summary.prompts;
        report.turns += summary.turns;
        report.turn_errors += summary.turn_errors;
        resets.push((instance_id.to_string(), summary.resets));

        let last_at = owned.last().map(|e| e.occurred_at);
        let mut open: Option<(chrono::DateTime<chrono::Utc>, u32)> = None;
        for event in &owned {
            if is_context_boundary(event) {
                if let Some((started, prompts)) = open.take() {
                    prompts_per_context.push(f64::from(prompts));
                    context_minutes.push((event.occurred_at - started).num_seconds() as f64 / 60.0);
                }
                open = Some((event.occurred_at, 0));
                report.contexts += 1;
            } else if event.kind == UsageKind::Prompt {
                if let Some((_, prompts)) = open.as_mut() {
                    *prompts += 1;
                }
            }
        }
        if let (Some((started, prompts)), Some(last_at)) = (open, last_at) {
            prompts_per_context.push(f64::from(prompts));
            context_minutes.push((last_at - started).num_seconds() as f64 / 60.0);
        }
    }
    report.median_prompts_per_context = median(prompts_per_context);
    report.median_context_minutes = median(context_minutes);
    report.median_resets_per_instance = median(resets.iter().map(|(_, n)| f64::from(*n)).collect());
    report.max_resets_per_instance = resets.iter().map(|(_, n)| *n).max().unwrap_or(0);
    resets.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    resets.truncate(10);
    report.top = resets;
    report
}
```

Run: `cargo test --lib usage::report`
Expected: PASS.

- [ ] **Step 4: Write the CLI `src/cli/usage.rs`**

Look at an existing CLI with subcommands and `--json` (for example `src/cli/list.rs` and `src/cli/session.rs`) and reuse its output helpers from `src/cli/output.rs`, its session resolver (for `show <SESSION>`, match by id prefix or title the way `aoe session show` does), and its way of listing sessions across all profiles (for titles in the report). Shape:

```rust
//! `aoe usage`: session usage from the local usage log.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::usage::{build_report, UsageStore};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct UsageArgs {
    #[command(subcommand)]
    command: Option<UsageCommands>,
    /// Window to summarize, like `30d`, `12h`, or `90m`.
    #[arg(long, default_value = "30d")]
    since: String,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
pub enum UsageCommands {
    /// One session's usage summary and its latest events.
    Show {
        /// Session id, id prefix, or title.
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// Raw usage events as JSON lines, oldest first (the Ledger feed).
    Export {
        /// Only events with a larger id.
        #[arg(long, default_value_t = 0)]
        after_id: i64,
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
    },
}

pub async fn run(args: UsageArgs) -> Result<()> {
    let store = UsageStore::open_default()?;
    match args.command {
        None => report(&store, &args.since, args.json),
        Some(UsageCommands::Show { session, json }) => show(&store, &session, json),
        Some(UsageCommands::Export { after_id, limit }) => {
            for event in store.events_after(after_id, limit)? {
                println!("{}", serde_json::to_string(&event)?);
            }
            Ok(())
        }
    }
}

/// `30d`, `12h`, `90m` into a duration.
fn parse_since(value: &str) -> Result<chrono::Duration> {
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let n: i64 = number
        .parse()
        .map_err(|_| anyhow::anyhow!("--since must look like 30d, 12h, or 90m"))?;
    match unit {
        "d" => Ok(chrono::Duration::days(n)),
        "h" => Ok(chrono::Duration::hours(n)),
        "m" => Ok(chrono::Duration::minutes(n)),
        _ => anyhow::bail!("--since must look like 30d, 12h, or 90m"),
    }
}
```

Implement `report(&store, since, json)`: `events_since(Utc::now() - parse_since(since)?)`, `build_report`, then either `serde_json::to_string_pretty` or a plain text table: one line per total, the three medians (one decimal, `-` when `None`), max resets, then the top list as `resets  title (id prefix)` using titles from all profiles' sessions and the raw id when the session no longer exists.

Implement `show(&store, session, json)`: resolve the session to an `Instance`, `summary_for(&instance.id)`, and `events_for_instance` (last 20). JSON prints `{"session": id, "title": ..., "summary": <UsageSummary>, "events": [...]}`; text prints the headline `resets`, the same breakdown as the overlay (reuse `crate::tui::components::usage_overlay::overlay_lines` only if it is reachable without `pub(crate)` gymnastics; otherwise print explicit `key: value` lines), then one line per event: `occurred_at  kind  detail  agent`.

Add a unit test for `parse_since` with cases `("30d", 30 days)`, `("12h", 12 hours)`, `("90m", 90 minutes)`, `("x", err)`, `("5w", err)`.

- [ ] **Step 5: Wire the command**

`src/cli/mod.rs`: `pub mod usage;`. `src/cli/definition.rs`: import `use super::usage::UsageArgs;` and add a visible variant near `Status`:

```rust
    /// Session usage: context resets, prompts, and turns from the local usage log
    Usage(UsageArgs),
```

`command_name`: `Commands::Usage(_) => "usage",`. `src/main.rs`: add `Some(Commands::Usage(args)) => cli::usage::run(args).await,` in the main command match where other normal commands (for example `Status`) are dispatched, following their exact form. If `src/cli/definition.rs` has a test enumerating commands or generating docs (`docs/cli/`), regenerate or update it as that test instructs.

- [ ] **Step 6: Write the contract doc `docs/development/usage-events.md`**

```markdown
# Usage events

AoE records agent context resets, prompts, turns, and session lifecycle events
in `usage.db` in the app directory. This page is the contract for external
readers such as Harness Ledger.

## Reading

Prefer `aoe usage export --after-id N`: one JSON object per line, oldest
first, with the fields below. Remember the last `id` you imported and pass it
back as `--after-id`. Readers may also open the database read-only; writers
use WAL mode.

## Table `usage_events`

| column | type | notes |
|---|---|---|
| `id` | integer | monotonic; the import cursor |
| `occurred_at` | text | RFC 3339 UTC with milliseconds |
| `instance_id` | text | the AoE session id |
| `profile` | text, nullable | AoE profile |
| `agent` | text, nullable | `claude`, `codex`, `pi`, or the session's tool |
| `kind` | text | see below |
| `detail` | text, nullable | at most 32 chars of `[a-z0-9_]` |
| `agent_session_id` | text, nullable | the agent's native session id |

`usage_meta.schema_version` is `1`. Within a schema version, changes are
additive only: new kinds, new detail values, new nullable columns.

## Kinds

| kind | detail | source |
|---|---|---|
| `context_start` | `startup` `resume` `clear` `compact` `fork` `reload` | Claude and Codex `SessionStart.source`; Pi `session_start.reason` (`new` is stored as `clear`) |
| `context_end` | agent reason, verbatim | Claude `SessionEnd.reason`; Pi `session_shutdown.reason` |
| `compact` | `manual` `auto` | Claude and Codex `PostCompact.trigger`; Pi `session_compact.reason` (`threshold` and `overflow` are `auto`) |
| `prompt` | none | Claude and Codex `UserPromptSubmit`; Pi interactive or RPC `input` |
| `turn_end` | none, or `error` | Claude and Codex `Stop`; Claude `StopFailure` (`error`); Pi `agent_settled` |
| `instance_created` | none | AoE created the session |
| `instance_restarted` | none | AoE restarted the session's agent |
| `instance_deleted` | none | AoE deleted the session |

A compaction logs both `compact` and `context_start` with detail `compact`.
Count compactions and context boundaries from `compact` only.

Sandboxed sessions record lifecycle events only.
```

- [ ] **Step 7: Run tests and a manual CLI check**

```bash
cargo test --lib usage:: && cargo test --lib cli::usage && cargo test --lib cli::definition
cargo build
D=$(mktemp -d)
for s in startup clear; do echo "{\"hook_event_name\":\"SessionStart\",\"source\":\"$s\"}" | HOME=$D AOE_INSTANCE_ID=smoke AOE_REPORT_AGENT=claude ./target/debug/aoe __usage-event; done
HOME=$D ./target/debug/aoe usage --json && HOME=$D ./target/debug/aoe usage export
```

Expected: tests pass; the report shows `"clears": 1`; export prints two JSON lines with `"kind":"context_start"`.

- [ ] **Step 8: Commit**

```bash
git add src/usage src/cli docs/development/usage-events.md src/main.rs
git commit -m "feat(usage): add aoe usage report, show, and export"
```

---

### Task 9: Whole-branch verification

**Files:** none new; fixes only.

- [ ] **Step 1: Format and lint**

Run: `cargo fmt && cargo clippy --all-targets --features web 2>&1 | tail -40`
Expected: no new warnings in touched files. Pre-existing warnings unrelated to this branch may remain; list them in the final report rather than fixing them.

- [ ] **Step 2: Full Rust tests**

Run: `cargo nextest run --features web 2>&1 | tail -40` (fall back to `cargo test --features web` if nextest is missing).
Expected: all pass, except known pre-existing local failures. Compare any failure against `main` (`git stash` is forbidden; use `git worktree` or rerun the single test on a `main` checkout) before attributing it to this branch.

- [ ] **Step 3: Web checks**

Run: `cd web && npm run test:unit && npx tsc -b && npm run lint && npm run format:check`
Expected: pass.

- [ ] **Step 4: Live smoke test**

Build `cargo build --features web`, start a debug TUI in an isolated tmux socket, create a Claude session in a scratch dir, send `/clear` twice, and confirm the preview overlay shows `2` with `clr 2`. Then confirm `GET http://127.0.0.1:8081/api/sessions/<id>/usage` returns `resets: 2` from a debug `aoe serve`. Record what was and was not verified.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A && git commit -m "fix(usage): address verification findings"
```
