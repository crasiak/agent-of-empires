# aoe Ledger run overlay (generation drift + Headroom savings) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** For a Ledger-launched session, add two lines under the reset counter in the TUI preview overlay and in the web terminal overlay:
- the session's generation drift (`ledger behind d861↔f00d 3h0m  +caveman -dataviz ↓tdd  ↓2 settings ↓renderer`);
- its Headroom savings (`hr 1.1M saved · 28% · ~$5.66`).

**Architecture:**
1. `aoe session report-ledger-launch` records each Ledger run id as a `ledger_run` usage event, so a restarted session remembers its earlier runs.
2. A new library module, `crate::ledger_run`, calls `ledger run show <run_id> --json` (built by the companion Ledger plan) for every run of the selected session. It caches finished runs and folds them into one `LedgerRunView`, which also carries a cents estimate from a hand-kept price table.
3. The TUI gets the view through a background `LedgerPoller`. The web gets it from `GET /api/sessions/{id}/ledger-run`. Both build it with the same Rust code.
4. Rendering happens on each surface: `src/tui/components/ledger_overlay.rs` and `web/src/lib/ledgerRun.ts`. Both are pinned to the same committed fixtures, `tests/fixtures/ledger-run/cases.json`.

**Tech Stack:** Rust 2021 (rust-version 1.85), ratatui, axum, rusqlite, serde; React + TypeScript + vitest.

**Spec:** `docs/superpowers/specs/2026-10-01-ledger-run-overlay-design.md`. The companion plan, `docs/superpowers/plans/2026-10-01-ledger-run-show.md`, must be merged and installed before the manual check in Task 11. Tasks 1–10 don't need it: they run against fixtures.

## Global Constraints

- Repo/worktree: `/Users/jws/code/agent-of-empires-worktrees/session-counter`, branch `feature/ledger-drift-overlay`. Fork PRs go to `--repo crasiak/agent-of-empires` and must fill in every PR template section. Commits use `jw@crasiak.net`, already set locally. The pre-commit hook runs `cargo fmt -- --check`.
- **Never run aoe or its tests with this shell's `CLAUDE_CONFIG_DIR`.** They corrupt the Ledger runtime settings.json. Every cargo test command in this plan uses the sentinel prefix:
  `S=$(mktemp -d); env -u AOE_INSTANCE_ID -u AOE_HOOK_BIN -u AOE_PROFILE CLAUDE_CONFIG_DIR=$S/claude CODEX_HOME=$S/codex CARGO_TARGET_DIR=$HOME/code/agent-of-empires/target cargo test ...`
  Below this is abbreviated `$SENTINEL cargo test ...`: always expand it. The machine is under heavy load, so expect slow builds and don't treat a slow build as a failure.
- Known pre-existing local failures (ignore them, don't fix them):
  - `tmux::session::tests::test_container_env_file_does_not_mutate_host_process_environment`
  - `storage_concurrency::test_lock_released_on_panic_unwind` (timing)
  - the first-run `acp_runner_control` timeouts
  - 7 clippy warnings in files identical to upstream
- `ledger run show` contract (`ledger.run.v1`): exit 0 prints the view, exit 2 prints `{"error":"unknown_run"}`, anything else is a failure. Call it through `crate::process::run_with_bounded_stdout` with a 2 s timeout and a 64 KiB cap. Errors show as `no ledger` (binary missing; stop calling for 5 minutes), `timeout`, `unknown run`, or `ledger error`.
- Poll cadence: TUI every 30 s plus on selection change; web every 30 s. Finished runs are cached per run id for the process lifetime.
- Usage event contract: `detail` is at most 32 chars of `[a-z0-9_]`. Ledger run ids are `run_` plus 32 lowercase hex characters (all 3463 in the live DB match), so a `ledger_run` event stores the 32 hex characters without the `run_` prefix. Ids of any other shape are not recorded.
- Dollar estimate: integer cents, `saved_tokens × input cents-per-MTok / 1_000_000`, rounded down, always shown with a `~` prefix. Prices (Anthropic first-party input list prices, claude-api skill cache 2026-09-25), first substring match wins:
  `fable` 1000, `mythos` 1000, `opus-5-5` 400, `opus-5` 500, `opus-4` 500, `sonnet-5` 200, `sonnet-4` 300, `haiku-4-5` 100.
  GPT models get no estimate: no verified price is on hand. This is a deliberate gap against the spec's `gpt-5` entry; flag it to the user.
- Number formatting uses integer math only, so Rust and TypeScript agree byte for byte: counts are `n`, `X.Yk` or `X.YM` (tenths rounded down); the percentage is `saved*100/before` rounded down.
- Durations reuse the existing `format_duration_short` / `formatDurationShort` (`3h0m`, `2d0h`, `42m`).
- Settings: `session.show_ledger_overlay` (default on) gates the drift line; `session.show_headroom_overlay` (default on) gates the Headroom line.
- The fixtures in `tests/fixtures/ledger-run/` are committed with this plan and checked against a reference renderer. Don't edit an expected line to make a test pass. If one disagrees, the code is wrong, unless you can show the fixture's arithmetic is wrong.
- Edition 2021: no `if let … && let …` chains.

## File map

- `src/usage/mod.rs`, `src/usage/summary.rs`: `UsageKind::LedgerRun`, run id helpers, `ledger_runs()`.
- `src/usage/prices.rs` (new): `estimate_cents()`.
- `src/cli/session.rs`: `report_ledger_launch` records the run.
- `src/ledger_run/mod.rs` (new): show/view types, `parse_output`, `merge_runs`, `build_view`.
- `src/ledger_run/runner.rs` (new): `ShowCache`, the `ledger` subprocess, `load_view()`.
- `src/session/ledger_restart.rs`: `current_ledger_run()`.
- `src/session/config/mod.rs`: the two settings.
- `src/tui/components/ledger_overlay.rs` (new): line text and overlay rows.
- `src/tui/components/usage_overlay.rs`: section stack (`OverlayRow`, `usage_rows`, `render_overlay_sections`).
- `src/tui/ledger_poller.rs` (new); `src/tui/home/{mod,lifecycle,config_refresh,status,render,input}.rs`; `src/tui/app.rs`: wiring.
- `src/server/api/sessions/usage.rs`, `src/server/router.rs`: `GET /api/sessions/{id}/ledger-run`.
- `web/src/lib/ledgerRun.ts` (new), `web/src/lib/api.ts`, `web/src/components/UsageOverlay.tsx`, `web/src/App.tsx`, `web/tests/coverage-matrix.json`.
- Docs: `docs/development/usage-events.md`, `docs/development/ledger-run.md` (new).
- Fixtures (already committed): `tests/fixtures/ledger-run/{current,behind,diverged,overflow,finished,live-now,unpriced}.json` (`ledger run show` outputs) and `cases.json` (`[{name, shows, run_ids?, outcome?, now, view, lines}]`).

---

### Task 1: `ledger_run` usage events

**Files:**
- Modify: `src/usage/mod.rs` (enum, `ALL`, `as_str`, helpers, re-export)
- Modify: `src/usage/summary.rs` (`summarize` arm, `ledger_runs`, tests)
- Modify: `docs/development/usage-events.md`

**Interfaces:**
- Produces:
  - `UsageKind::LedgerRun` (string `ledger_run`)
  - `pub fn ledger_run_detail(run_id: &str) -> Option<String>`
  - `pub fn ledger_run_id(detail: &str) -> String`
  - `pub fn ledger_runs(events: &[UsageEvent]) -> Vec<String>` (re-exported from `crate::usage`)

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `src/usage/summary.rs`:

```rust
    #[test]
    fn ledger_runs_are_lifecycle_events_in_launch_order() {
        use UsageKind::*;
        let a = "0123456789abcdef0123456789abcdef";
        let b = "fedcba9876543210fedcba9876543210";
        let events = vec![
            ev(0, InstanceCreated, None),
            ev(1, LedgerRun, Some(a)),
            ev(2, InstanceRestarted, None),
            ev(3, LedgerRun, Some(b)),
            ev(4, LedgerRun, Some(b)),
            ev(5, LedgerRun, None),
        ];
        assert!(!summarize(&events).tracked);
        assert_eq!(summarize(&events).resets, 0);
        assert_eq!(
            ledger_runs(&events),
            vec![format!("run_{a}"), format!("run_{b}")]
        );
    }
```

Append to the `tests` module in `src/usage/mod.rs`:

```rust
    #[test]
    fn ledger_run_ids_fit_the_detail_contract() {
        let id = "run_605c696f0a75d2c76a3d3bb982c50025";
        let detail = ledger_run_detail(id).unwrap();
        assert_eq!(detail, "605c696f0a75d2c76a3d3bb982c50025");
        assert_eq!(ledger_run_id(&detail), id);
        for bad in [
            "run-prior",
            "605c696f0a75d2c76a3d3bb982c50025",
            "run_605C696F0A75D2C76A3D3BB982C50025",
            "run_605c696f",
            "run_605c696f0a75d2c76a3d3bb982c50025ff",
        ] {
            assert_eq!(ledger_run_detail(bad), None, "{bad}");
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib usage:: 2>&1 | tail -20`
Expected: compile error, `no variant named LedgerRun`.

- [ ] **Step 3: Implement**

In `src/usage/mod.rs`:
- add `LedgerRun` as the last `UsageKind` variant;
- make `ALL` `[UsageKind; 9]` with `UsageKind::LedgerRun` last;
- add `UsageKind::LedgerRun => "ledger_run",` to `as_str`;
- extend the summary re-export to `pub use summary::{is_context_boundary, ledger_runs, summarize, UsageSummary};`;
- add `pub mod prices;` next to the other `mod` lines (Task 3 fills it in; for now create `src/usage/prices.rs` containing only `//! Filled in by the next task.`);
- add, after `db_path`:

```rust
/// A Ledger run id as a `ledger_run` detail: Ledger ids are `run_` plus 32
/// lowercase hex, which fits the detail contract once the prefix is gone.
pub fn ledger_run_detail(run_id: &str) -> Option<String> {
    let hex = run_id.strip_prefix("run_")?;
    (hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| hex.to_string())
}

pub fn ledger_run_id(detail: &str) -> String {
    format!("run_{detail}")
}
```

In `src/usage/summary.rs`:
- add `| UsageKind::LedgerRun` to the no-op match arm (`ContextEnd | InstanceCreated | …`);
- add, after `summarize`:

```rust
/// The session's Ledger runs, oldest first; the last is the current one.
pub fn ledger_runs(events: &[UsageEvent]) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    for event in events {
        if event.kind != UsageKind::LedgerRun {
            continue;
        }
        if let Some(detail) = event.detail.as_deref() {
            let id = super::ledger_run_id(detail);
            if !runs.contains(&id) {
                runs.push(id);
            }
        }
    }
    runs
}
```

In `docs/development/usage-events.md`, add a row to the Kinds table after `instance_deleted`:

```
| `ledger_run` | the Ledger run id without its `run_` prefix (32 lowercase hex) | `aoe session report-ledger-launch`, once per Ledger launch of the session's agent |
```

Then add this paragraph after the "A `context_start` with detail `reload` …" line:

```
`ledger_run` is a lifecycle kind: it does not count toward resets. Ledger
run ids of any other shape are not recorded.
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib usage:: 2>&1 | tail -20`
Expected: PASS, including `kinds_round_trip_through_their_names` for the new kind.

- [ ] **Step 5: Commit**

```bash
git add src/usage docs/development/usage-events.md
git commit -m "feat(usage): record Ledger runs as ledger_run lifecycle events"
```

---

### Task 2: `report-ledger-launch` records the run

**Files:**
- Modify: `src/cli/session.rs` (`report_ledger_launch`, new helpers, new test module at the end of the file)

**Interfaces:**
- Consumes: `crate::usage::{ledger_run_detail, UsageEvent, UsageKind, UsageStore, db_path}` (Task 1).
- Produces: `fn record_ledger_run_at(db: &std::path::Path, instance_id: &str, run_id: &str, profile: Option<&str>, agent: Option<&str>, enabled: bool) -> anyhow::Result<bool>`

- [ ] **Step 1: Write the failing test**

Append to `src/cli/session.rs`:

```rust
#[cfg(test)]
mod ledger_run_event_tests {
    use super::record_ledger_run_at;

    #[test]
    fn records_a_ledger_run_once_per_report_and_respects_the_switch() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("usage.db");
        let id = "run_605c696f0a75d2c76a3d3bb982c50025";
        assert!(record_ledger_run_at(&db, "inst", id, Some("work"), Some("claude"), true).unwrap());
        assert!(!record_ledger_run_at(&db, "inst", "run-prior", None, None, true).unwrap());
        assert!(!record_ledger_run_at(&db, "inst", id, None, None, false).unwrap());
        let events = crate::usage::UsageStore::open(&db)
            .unwrap()
            .events_for_instance("inst")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, crate::usage::UsageKind::LedgerRun);
        assert_eq!(events[0].detail.as_deref(), Some("605c696f0a75d2c76a3d3bb982c50025"));
        assert_eq!(events[0].profile.as_deref(), Some("work"));
        assert_eq!(events[0].agent.as_deref(), Some("claude"));
        assert_eq!(crate::usage::ledger_runs(&events), vec![id.to_string()]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `$SENTINEL cargo test --lib ledger_run_event_tests 2>&1 | tail -20`
Expected: compile error, `cannot find function record_ledger_run_at`.

- [ ] **Step 3: Implement**

In `src/cli/session.rs`, inside `report_ledger_launch`, right after `let encoded = report.encode()?;`, add:

```rust
    // Before publishing: the supervised path below never returns.
    record_ledger_run(&report.instance_id, &report.run_id);
```

Add below `report_ledger_launch`:

```rust
/// Best effort: the usage log keeps a session's Ledger runs so the overlay
/// can still sum an earlier run's Headroom savings after a restart.
fn record_ledger_run(instance_id: &str, run_id: &str) {
    let env = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
    let profile = env("AOE_PROFILE");
    let enabled = profile.as_deref().is_none_or(|profile| {
        crate::session::config::profile_config::resolve_config_or_warn(profile)
            .session
            .usage_tracking
    });
    let agent = env("AOE_REPORT_AGENT");
    let result = crate::usage::db_path().and_then(|db| {
        record_ledger_run_at(
            &db,
            instance_id,
            run_id,
            profile.as_deref(),
            agent.as_deref(),
            enabled,
        )
    });
    if let Err(e) = result {
        tracing::debug!(target: "usage", "ledger run event dropped: {e}");
    }
}

/// Returns whether a row was written.
fn record_ledger_run_at(
    db: &std::path::Path,
    instance_id: &str,
    run_id: &str,
    profile: Option<&str>,
    agent: Option<&str>,
    enabled: bool,
) -> Result<bool> {
    let Some(detail) = crate::usage::ledger_run_detail(run_id) else {
        return Ok(false);
    };
    if !enabled {
        return Ok(false);
    }
    crate::usage::UsageStore::open(db)?.insert(&crate::usage::UsageEvent {
        id: 0,
        occurred_at: chrono::Utc::now(),
        instance_id: instance_id.to_string(),
        profile: profile.map(str::to_string),
        agent: agent.map(str::to_string),
        kind: crate::usage::UsageKind::LedgerRun,
        detail: Some(detail),
        agent_session_id: None,
    })?;
    Ok(true)
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib ledger_run_event_tests ledger_restart 2>&1 | tail -20`
Expected: PASS. The existing `ledger_restart` CLI parse tests are unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/cli/session.rs
git commit -m "feat(session): remember each Ledger run a session launches"
```

---

### Task 3: Price table

**Files:**
- Modify (fill): `src/usage/prices.rs`

**Interfaces:**
- Produces: `pub fn estimate_cents(saved_tokens: u64, model: &str) -> Option<u64>` (`crate::usage::prices::estimate_cents`)

- [ ] **Step 1: Write the failing test**

Replace `src/usage/prices.rs` with the test module first:

```rust
//! Input list prices for the Ledger overlay's Headroom savings estimate.

#[cfg(test)]
mod tests {
    use super::estimate_cents;

    #[test]
    fn matches_the_most_specific_model_and_rounds_down() {
        let cases = [
            ("global.anthropic.claude-opus-5", 1_132_000, Some(566)),
            ("claude-opus-5-5", 26_500, Some(10)),
            ("us.anthropic.claude-opus-5-5", 1_000_000, Some(400)),
            ("claude-opus-4-8", 1_000_000, Some(500)),
            ("claude-sonnet-5-5", 1_000_000, Some(200)),
            ("claude-sonnet-4-6", 300_000, Some(90)),
            ("global.anthropic.claude-haiku-4-5-20251001-v1:0", 1_000_000, Some(100)),
            ("claude-fable-5-1", 1_000_000, Some(1000)),
            ("gpt-6-astra", 1_000_000, None),
            ("metadata", 1_000_000, None),
            ("", 1_000_000, None),
        ];
        for (model, saved, want) in cases {
            assert_eq!(estimate_cents(saved, model), want, "{model}");
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `$SENTINEL cargo test --lib usage::prices 2>&1 | tail -20`
Expected: compile error, `cannot find function estimate_cents`.

- [ ] **Step 3: Implement**

Insert above the test module:

```rust
//! Input list prices for the Ledger overlay's Headroom savings estimate.
//! Anthropic first-party rates (claude-api skill cache, 2026-09-25), updated
//! by hand. Models without a known price get no estimate.

/// (model id substring, US cents per million input tokens). First match wins,
/// so a more specific id comes before any id it contains.
const INPUT_CENTS_PER_MTOK: &[(&str, u64)] = &[
    ("fable", 1000),
    ("mythos", 1000),
    ("opus-5-5", 400),
    ("opus-5", 500),
    ("opus-4", 500),
    ("sonnet-5", 200),
    ("sonnet-4", 300),
    ("haiku-4-5", 100),
];

/// What `saved_tokens` input tokens would have cost, in whole cents, rounded down.
pub fn estimate_cents(saved_tokens: u64, model: &str) -> Option<u64> {
    let model = model.to_ascii_lowercase();
    INPUT_CENTS_PER_MTOK
        .iter()
        .find(|(id, _)| model.contains(id))
        .map(|(_, cents)| saved_tokens * cents / 1_000_000)
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib usage::prices 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/usage/prices.rs
git commit -m "feat(usage): input price table for Headroom savings estimates"
```

---

### Task 4: `crate::ledger_run`: types and view building

**Files:**
- Create: `src/ledger_run/mod.rs`
- Modify: `src/lib.rs` (add `pub mod ledger_run;` after `pub mod hooks;`)
- Fixtures (already present): `tests/fixtures/ledger-run/*.json`

**Interfaces:**
- Consumes: `crate::usage::prices::estimate_cents` (Task 3).
- Produces (all `pub`, all `Debug + Clone + PartialEq + Eq`, plus the serde derives noted):
  - `SHOW_SCHEMA: &str = "ledger.run.v1"`
  - `Generation { state, launched, current, current_since: String, drift: Vec<Change>, runtime_drift: Vec<RuntimeChange> }` (Serialize + Deserialize)
  - `Change { kind, name, change: String }`, `RuntimeChange { kind, change: String }` (Serialize + Deserialize)
  - `RunHeadroom { requests, input_tokens_before, input_tokens_after, saved_tokens: u64, model: String }` (Deserialize)
  - `RunShow { schema, run_id: String, finished_at: Option<String>, generation: Option<Generation>, headroom: Option<RunHeadroom> }` (Deserialize)
  - `HeadroomTotals { requests, input_tokens_before, input_tokens_after, saved_tokens: u64, model: String, estimated_cents: Option<u64> }` (Serialize + Deserialize)
  - `LedgerRunView { run_id: String, runs: usize, generation: Option<Generation>, headroom_total: Option<HeadroomTotals>, headroom_now: Option<HeadroomTotals>, error: Option<String> }` (Default + Serialize + Deserialize)
  - `enum ShowOutcome { Ok(RunShow), Missing, Timeout, UnknownRun, Failed }` with `fn reason(&self) -> &'static str`
  - `fn parse_output(code: Option<i32>, stdout: &[u8]) -> ShowOutcome`
  - `fn merge_runs(history: Vec<String>, current: &str) -> Vec<String>`
  - `fn build_view(runs: &[String], show: &mut dyn FnMut(&str) -> ShowOutcome) -> LedgerRunView`

- [ ] **Step 1: Write the failing tests**

Create `src/ledger_run/mod.rs` with only the tests for now:

```rust
//! The Ledger run overlay's data.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ledger-run")
    }

    fn read_show(name: &str) -> RunShow {
        let text = std::fs::read_to_string(fixtures().join(name)).unwrap();
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        shows: Vec<String>,
        #[serde(default)]
        run_ids: Vec<String>,
        outcome: Option<String>,
        view: serde_json::Value,
    }

    #[test]
    fn fixture_cases_build_their_views() {
        let text = std::fs::read_to_string(fixtures().join("cases.json")).unwrap();
        let cases: Vec<Case> = serde_json::from_str(&text).unwrap();
        assert!(cases.len() >= 7);
        for case in cases {
            let shows: Vec<RunShow> = case.shows.iter().map(|name| read_show(name)).collect();
            let runs: Vec<String> = if shows.is_empty() {
                case.run_ids.clone()
            } else {
                shows.iter().map(|show| show.run_id.clone()).collect()
            };
            let mut show = |id: &str| match case.outcome.as_deref() {
                Some("timeout") => ShowOutcome::Timeout,
                Some(other) => panic!("unknown outcome {other}"),
                None => ShowOutcome::Ok(shows.iter().find(|s| s.run_id == id).unwrap().clone()),
            };
            let view = build_view(&runs, &mut show);
            assert_eq!(serde_json::to_value(&view).unwrap(), case.view, "{}", case.name);
        }
    }

    #[test]
    fn output_maps_exit_codes_and_schema() {
        let good = std::fs::read(fixtures().join("behind.json")).unwrap();
        assert!(matches!(parse_output(Some(0), &good), ShowOutcome::Ok(_)));
        let other_schema = String::from_utf8(good.clone())
            .unwrap()
            .replace("ledger.run.v1", "ledger.run.v9");
        assert_eq!(parse_output(Some(0), other_schema.as_bytes()), ShowOutcome::Failed);
        assert_eq!(parse_output(Some(0), b"not json"), ShowOutcome::Failed);
        assert_eq!(
            parse_output(Some(2), br#"{"error":"unknown_run"}"#),
            ShowOutcome::UnknownRun
        );
        assert_eq!(parse_output(Some(1), b""), ShowOutcome::Failed);
        assert_eq!(parse_output(None, b""), ShowOutcome::Failed);
    }

    #[test]
    fn the_current_run_goes_last_once() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(merge_runs(s(&["a", "b"]), "b"), s(&["a", "b"]));
        assert_eq!(merge_runs(s(&["a"]), "c"), s(&["a", "c"]));
        assert_eq!(merge_runs(s(&["a", "b"]), "a"), s(&["b", "a"]));
        assert_eq!(merge_runs(Vec::new(), "c"), s(&["c"]));
    }

    #[test]
    fn a_failed_earlier_run_is_left_out_and_a_failed_current_run_is_the_error() {
        let finished = read_show("finished.json");
        let live = read_show("live-now.json");
        let runs = vec![finished.run_id.clone(), live.run_id.clone()];
        let view = build_view(&runs, &mut |id: &str| {
            if id == live.run_id {
                ShowOutcome::Ok(live.clone())
            } else {
                ShowOutcome::Timeout
            }
        });
        assert_eq!(view.error, None);
        assert_eq!(view.headroom_total.as_ref().unwrap().saved_tokens, 26_500);
        let view = build_view(&runs, &mut |_: &str| ShowOutcome::Missing);
        assert_eq!(view.error.as_deref(), Some("no ledger"));
        assert_eq!((view.generation, view.headroom_total), (None, None));
    }
}
```

Add `pub mod ledger_run;` to `src/lib.rs`.

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib ledger_run:: 2>&1 | tail -20`
Expected: compile errors (`cannot find type RunShow`).

- [ ] **Step 3: Implement**

Insert above the tests in `src/ledger_run/mod.rs`:

```rust
//! The Ledger run overlay's data: `ledger run show` for each Ledger run of a
//! session, folded into one view that the TUI and web overlays render.
//! Contract: docs/development/ledger-run.md.

mod runner;

use serde::{Deserialize, Serialize};

pub use runner::load_view;

pub const SHOW_SCHEMA: &str = "ledger.run.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub state: String,
    pub launched: String,
    pub current: String,
    pub current_since: String,
    pub drift: Vec<Change>,
    pub runtime_drift: Vec<RuntimeChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub kind: String,
    pub name: String,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeChange {
    pub kind: String,
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunHeadroom {
    pub requests: u64,
    pub input_tokens_before: u64,
    pub input_tokens_after: u64,
    pub saved_tokens: u64,
    pub model: String,
}

/// The fields of `ledger run show --json` the overlay reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunShow {
    pub schema: String,
    pub run_id: String,
    pub finished_at: Option<String>,
    pub generation: Option<Generation>,
    pub headroom: Option<RunHeadroom>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadroomTotals {
    pub requests: u64,
    pub input_tokens_before: u64,
    pub input_tokens_after: u64,
    pub saved_tokens: u64,
    /// The most recent run's model.
    pub model: String,
    /// Sum of the runs that have a known price; `None` when none do.
    pub estimated_cents: Option<u64>,
}

impl HeadroomTotals {
    fn from_run(run: &RunHeadroom) -> Self {
        Self {
            requests: run.requests,
            input_tokens_before: run.input_tokens_before,
            input_tokens_after: run.input_tokens_after,
            saved_tokens: run.saved_tokens,
            model: run.model.clone(),
            estimated_cents: crate::usage::prices::estimate_cents(run.saved_tokens, &run.model),
        }
    }

    fn plus(self, later: &Self) -> Self {
        let estimated_cents = match (self.estimated_cents, later.estimated_cents) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        Self {
            requests: self.requests + later.requests,
            input_tokens_before: self.input_tokens_before + later.input_tokens_before,
            input_tokens_after: self.input_tokens_after + later.input_tokens_after,
            saved_tokens: self.saved_tokens + later.saved_tokens,
            model: later.model.clone(),
            estimated_cents,
        }
    }
}

/// One session's overlay data. Drift comes from the current (last) run; the
/// Headroom total spans every run the session has had.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerRunView {
    pub run_id: String,
    pub runs: usize,
    pub generation: Option<Generation>,
    pub headroom_total: Option<HeadroomTotals>,
    pub headroom_now: Option<HeadroomTotals>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowOutcome {
    Ok(RunShow),
    /// No `ledger` binary on PATH.
    Missing,
    Timeout,
    UnknownRun,
    Failed,
}

impl ShowOutcome {
    pub fn reason(&self) -> &'static str {
        match self {
            ShowOutcome::Ok(_) => "",
            ShowOutcome::Missing => "no ledger",
            ShowOutcome::Timeout => "timeout",
            ShowOutcome::UnknownRun => "unknown run",
            ShowOutcome::Failed => "ledger error",
        }
    }
}

/// Exit 0 carries the view; exit 2 is Ledger's "unknown run".
pub fn parse_output(code: Option<i32>, stdout: &[u8]) -> ShowOutcome {
    match code {
        Some(0) => serde_json::from_slice::<RunShow>(stdout)
            .ok()
            .filter(|show| show.schema == SHOW_SCHEMA)
            .map_or(ShowOutcome::Failed, ShowOutcome::Ok),
        Some(2) => ShowOutcome::UnknownRun,
        _ => ShowOutcome::Failed,
    }
}

/// The recorded runs with the pane's current run last.
pub fn merge_runs(mut history: Vec<String>, current: &str) -> Vec<String> {
    history.retain(|id| id != current);
    history.push(current.to_string());
    history
}

pub fn build_view(runs: &[String], show: &mut dyn FnMut(&str) -> ShowOutcome) -> LedgerRunView {
    let Some((current, earlier)) = runs.split_last() else {
        return LedgerRunView::default();
    };
    let mut view = LedgerRunView {
        run_id: current.clone(),
        runs: runs.len(),
        ..LedgerRunView::default()
    };
    let current_show = match show(current) {
        ShowOutcome::Ok(current_show) => current_show,
        failed => {
            view.error = Some(failed.reason().to_string());
            return view;
        }
    };
    view.generation = current_show.generation;
    view.headroom_now = current_show.headroom.as_ref().map(HeadroomTotals::from_run);
    let mut total: Option<HeadroomTotals> = None;
    for id in earlier {
        if let ShowOutcome::Ok(earlier_show) = show(id) {
            if let Some(headroom) = &earlier_show.headroom {
                let run = HeadroomTotals::from_run(headroom);
                total = Some(match total {
                    Some(sum) => sum.plus(&run),
                    None => run,
                });
            }
        }
    }
    if let Some(now) = &view.headroom_now {
        total = Some(match total {
            Some(sum) => sum.plus(now),
            None => now.clone(),
        });
    }
    view.headroom_total = total;
    view
}
```

Create `src/ledger_run/runner.rs` as a stub so the module compiles. Task 5 replaces it.

```rust
//! Filled in by the next task.

pub fn load_view(_instance: &crate::session::Instance) -> Option<super::LedgerRunView> {
    None
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib ledger_run:: 2>&1 | tail -20`
Expected: PASS. If `fixture_cases_build_their_views` fails, print both values and fix the code; the fixtures were computed independently.

- [ ] **Step 5: Commit**

```bash
git add src/ledger_run src/lib.rs
git commit -m "feat(ledger-run): fold a session's ledger run show results into one view"
```

---

### Task 5: Calling Ledger: cache, back-off, `load_view`

**Files:**
- Modify (replace): `src/ledger_run/runner.rs`
- Modify: `src/session/ledger_restart.rs` (add `current_ledger_run`)

**Interfaces:**
- Consumes: `ShowOutcome`, `parse_output`, `merge_runs`, `build_view` (Task 4); `crate::usage::{UsageStore, ledger_runs}` (Task 1).
- Produces:
  - `pub fn load_view(instance: &crate::session::Instance) -> Option<LedgerRunView>`: `None` when the session has no `@aoe_ledger_launch` pane option.
  - `pub fn current_ledger_run(instance: &Instance) -> Option<String>` in `crate::session::ledger_restart`
  - `pub(crate) struct ShowCache` with `fn new() -> Self` and `fn show(&mut self, run_id: &str, now: Instant, run: &mut dyn FnMut(&str) -> ShowOutcome) -> ShowOutcome`

- [ ] **Step 1: Write the failing tests**

Replace `src/ledger_run/runner.rs` with the tests first:

```rust
//! Runs `ledger run show` with a deadline.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn show(run_id: &str, finished: bool) -> ShowOutcome {
        ShowOutcome::Ok(crate::ledger_run::RunShow {
            schema: crate::ledger_run::SHOW_SCHEMA.into(),
            run_id: run_id.into(),
            finished_at: finished.then(|| "2026-10-01T10:00:00Z".to_string()),
            generation: None,
            headroom: None,
        })
    }

    #[test]
    fn finished_runs_are_asked_once_and_live_runs_every_time() {
        let mut cache = ShowCache::new();
        let mut calls = Vec::new();
        let now = Instant::now();
        for _ in 0..3 {
            cache.show("run_done", now, &mut |id: &str| {
                calls.push(id.to_string());
                show(id, true)
            });
            cache.show("run_live", now, &mut |id: &str| {
                calls.push(id.to_string());
                show(id, false)
            });
        }
        assert_eq!(calls.iter().filter(|id| *id == "run_done").count(), 1);
        assert_eq!(calls.iter().filter(|id| *id == "run_live").count(), 3);
    }

    #[test]
    fn a_missing_binary_is_not_retried_for_five_minutes() {
        let mut cache = ShowCache::new();
        let calls = std::cell::Cell::new(0);
        let start = Instant::now();
        let mut missing = |_: &str| {
            calls.set(calls.get() + 1);
            ShowOutcome::Missing
        };
        assert_eq!(cache.show("run_a", start, &mut missing), ShowOutcome::Missing);
        assert_eq!(
            cache.show("run_a", start + Duration::from_secs(299), &mut missing),
            ShowOutcome::Missing
        );
        assert_eq!(calls.get(), 1);
        cache.show("run_a", start + Duration::from_secs(301), &mut missing);
        assert_eq!(calls.get(), 2);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib ledger_run::runner 2>&1 | tail -20`
Expected: compile error (`cannot find struct ShowCache`, `load_view` missing).

- [ ] **Step 3: Implement**

Insert above the tests in `src/ledger_run/runner.rs`:

```rust
//! Runs `ledger run show` with a deadline, caches finished runs for the
//! process lifetime, and backs off when the `ledger` binary is missing.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::{LedgerRunView, RunShow, ShowOutcome};

const SHOW_TIMEOUT: Duration = Duration::from_secs(2);
const SHOW_MAX_BYTES: usize = 64 * 1024;
const MISSING_BACKOFF: Duration = Duration::from_secs(5 * 60);

pub(crate) struct ShowCache {
    finished: HashMap<String, RunShow>,
    missing_until: Option<Instant>,
}

impl ShowCache {
    pub(crate) fn new() -> Self {
        Self {
            finished: HashMap::new(),
            missing_until: None,
        }
    }

    pub(crate) fn show(
        &mut self,
        run_id: &str,
        now: Instant,
        run: &mut dyn FnMut(&str) -> ShowOutcome,
    ) -> ShowOutcome {
        if let Some(show) = self.finished.get(run_id) {
            return ShowOutcome::Ok(show.clone());
        }
        if self.missing_until.is_some_and(|until| now < until) {
            return ShowOutcome::Missing;
        }
        let outcome = run(run_id);
        match &outcome {
            ShowOutcome::Ok(show) if show.finished_at.is_some() => {
                self.finished.insert(run_id.to_string(), show.clone());
            }
            ShowOutcome::Missing => self.missing_until = Some(now + MISSING_BACKOFF),
            _ => {}
        }
        outcome
    }
}

/// Shared by the TUI poller and the web server, so each process asks Ledger
/// about a finished run once.
static CACHE: LazyLock<Mutex<ShowCache>> = LazyLock::new(|| Mutex::new(ShowCache::new()));

fn run_show(run_id: &str) -> ShowOutcome {
    let mut command = std::process::Command::new("ledger");
    command.args(["run", "show", run_id, "--json"]);
    match crate::process::run_with_bounded_stdout(&mut command, SHOW_TIMEOUT, SHOW_MAX_BYTES) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ShowOutcome::Missing,
        Err(_) => ShowOutcome::Failed,
        Ok(None) => ShowOutcome::Timeout,
        Ok(Some(output)) => super::parse_output(output.status.code(), &output.stdout),
    }
}

/// The session's overlay view, or `None` when Ledger did not launch its
/// agent (no `@aoe_ledger_launch` pane option). Blocks on `ledger`; call it
/// off the render loop.
pub fn load_view(instance: &crate::session::Instance) -> Option<LedgerRunView> {
    let current = crate::session::ledger_restart::current_ledger_run(instance)?;
    let history = crate::usage::UsageStore::open_default()
        .and_then(|store| store.events_for_instance(&instance.id))
        .map(|events| crate::usage::ledger_runs(&events))
        .unwrap_or_default();
    let runs = super::merge_runs(history, &current);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    Some(super::build_view(&runs, &mut |id: &str| {
        cache.show(id, now, &mut |id: &str| run_show(id))
    }))
}
```

In `src/session/ledger_restart.rs`, add after `owned_prior`:

```rust
/// The Ledger run the session's agent pane reported, when Ledger launched it.
pub fn current_ledger_run(instance: &super::Instance) -> Option<String> {
    owned_prior(instance).map(|(_, ledger)| ledger.run_id)
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib ledger_run:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ledger_run/runner.rs src/session/ledger_restart.rs
git commit -m "feat(ledger-run): call ledger run show with caching and back-off"
```

---

### Task 6: Settings

**Files:**
- Modify: `src/session/config/mod.rs` (the `SessionConfig` fields after `show_usage_overlay`, ~line 982; `Default`, ~line 1804; a test in the `tests` module at ~line 3654)

**Interfaces:**
- Produces: `SessionConfig::show_ledger_overlay: bool` and `SessionConfig::show_headroom_overlay: bool`, both defaulting to `true`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `src/session/config/mod.rs`:

```rust
    #[test]
    fn ledger_overlay_settings_default_on_and_reach_every_surface() {
        let session = SessionConfig::default();
        assert!(session.show_ledger_overlay && session.show_headroom_overlay);
        let fields: Vec<String> = crate::session::config::settings_schema::schema()
            .into_iter()
            .filter(|field| field.section == "session")
            .map(|field| field.field)
            .collect();
        for name in ["show_ledger_overlay", "show_headroom_overlay"] {
            assert!(fields.iter().any(|field| field == name), "{name} missing from schema");
        }
    }
```

(If `settings_schema::schema` isn't reachable at that path, use the path that `settings_schema/registry.rs` re-exports. `grep -n "pub use" src/session/config/settings_schema/mod.rs`.)

- [ ] **Step 2: Run to verify it fails**

Run: `$SENTINEL cargo test --lib ledger_overlay_settings 2>&1 | tail -20`
Expected: compile error, `no field show_ledger_overlay`.

- [ ] **Step 3: Implement**

After `pub show_usage_overlay: bool,`:

```rust

    /// Under the usage overlay, show which Ledger generation a Ledger-launched
    /// session runs on and how it differs from what a new launch would get.
    #[serde(default = "default_true")]
    #[setting(label = "Show Ledger drift overlay", widget = "toggle")]
    pub show_ledger_overlay: bool,

    /// Under the usage overlay, show the input tokens Headroom compression
    /// saved for a Ledger-launched Headroom session, with a dollar estimate.
    #[serde(default = "default_true")]
    #[setting(label = "Show Headroom savings overlay", widget = "toggle")]
    pub show_headroom_overlay: bool,
```

In `impl Default for SessionConfig`, after `show_usage_overlay: true,`:

```rust
            show_ledger_overlay: true,
            show_headroom_overlay: true,
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib session::config 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/session/config/mod.rs
git commit -m "feat(config): settings for the Ledger drift and Headroom overlay lines"
```

---

### Task 7: TUI line text

**Files:**
- Create: `src/tui/components/ledger_overlay.rs`
- Modify: `src/tui/components/mod.rs` (add `pub(crate) mod ledger_overlay;` after `pub(crate) mod hover;`)

**Interfaces:**
- Consumes: `crate::ledger_run::LedgerRunView` (Task 4); `super::usage_overlay::format_duration_short`.
- Produces:
  - `pub(crate) struct DriftLine { pub text: String, pub dim: bool }`
  - `pub(crate) fn drift_line(view: &LedgerRunView, now: DateTime<Utc>) -> Option<DriftLine>`
  - `pub(crate) fn headroom_line(view: &LedgerRunView) -> Option<String>`
  - `pub(crate) fn compact_count(n: u64) -> String`

- [ ] **Step 1: Write the failing tests**

Create `src/tui/components/ledger_overlay.rs` with:

```rust
//! Ledger run overlay text.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        now: DateTime<Utc>,
        view: LedgerRunView,
        lines: Vec<String>,
    }

    fn cases() -> Vec<Case> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ledger-run/cases.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn fixture_cases_render_their_lines() {
        for case in cases() {
            let lines: Vec<String> = drift_line(&case.view, case.now)
                .map(|line| line.text)
                .into_iter()
                .chain(headroom_line(&case.view))
                .collect();
            assert_eq!(lines, case.lines, "{}", case.name);
        }
    }

    #[test]
    fn only_the_error_line_is_dim() {
        for case in cases() {
            let dim = drift_line(&case.view, case.now).is_some_and(|line| line.dim);
            assert_eq!(dim, case.view.error.is_some(), "{}", case.name);
        }
    }

    #[test]
    fn counts_round_down_to_one_decimal() {
        let cases = [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0k"),
            (26_500, "26.5k"),
            (999_999, "999.9k"),
            (1_250_000, "1.2M"),
            (1_132_000, "1.1M"),
        ];
        for (n, want) in cases {
            assert_eq!(compact_count(n), want);
        }
    }
}
```

Add the module to `src/tui/components/mod.rs`.

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib ledger_overlay 2>&1 | tail -20`
Expected: compile error (`cannot find function drift_line`).

- [ ] **Step 3: Implement**

Insert above the tests:

```rust
//! The Ledger run lines under the reset counter: the session's generation
//! drift and its Headroom savings. `web/src/lib/ledgerRun.ts` mirrors this
//! file; both are pinned to `tests/fixtures/ledger-run/cases.json`.

use chrono::{DateTime, Utc};

use super::usage_overlay::format_duration_short;
use crate::ledger_run::{Change, LedgerRunView};

/// Skills named on the drift line before the rest become `+N more`.
const MAX_SKILLS: usize = 6;

pub(crate) struct DriftLine {
    pub text: String,
    pub dim: bool,
}

fn marker(change: &str) -> &'static str {
    match change {
        "older" => "↓",
        "newer" => "↑",
        "extra" => "+",
        "missing" => "-",
        _ => "~",
    }
}

fn short(id: &str) -> String {
    id.chars().take(4).collect()
}

pub(crate) fn drift_line(view: &LedgerRunView, now: DateTime<Utc>) -> Option<DriftLine> {
    if let Some(error) = &view.error {
        return Some(DriftLine {
            text: format!("ledger ? {error}"),
            dim: true,
        });
    }
    let generation = view.generation.as_ref()?;
    if generation.state == "current" {
        return Some(DriftLine {
            text: format!("ledger {} current", short(&generation.launched)),
            dim: false,
        });
    }
    let age = DateTime::parse_from_rfc3339(&generation.current_since)
        .map(|since| format_duration_short(now - since.with_timezone(&Utc)))
        .unwrap_or_else(|_| "?".to_string());
    let mut segments = vec![format!(
        "ledger {} {}↔{} {age}",
        generation.state,
        short(&generation.launched),
        short(&generation.current)
    )];

    let skills: Vec<&Change> = generation
        .drift
        .iter()
        .filter(|change| change.kind == "skill")
        .collect();
    let mut named: Vec<String> = skills
        .iter()
        .take(MAX_SKILLS)
        .map(|change| format!("{}{}", marker(&change.change), change.name))
        .collect();
    if skills.len() > MAX_SKILLS {
        named.push(format!("+{} more", skills.len() - MAX_SKILLS));
    }
    if !named.is_empty() {
        segments.push(named.join(" "));
    }

    // Other kinds collapse to a count per kind, marked when they all moved
    // the same way.
    let mut kinds: Vec<&str> = Vec::new();
    for change in &generation.drift {
        if change.kind != "skill" && !kinds.contains(&change.kind.as_str()) {
            kinds.push(&change.kind);
        }
    }
    let mut counted: Vec<String> = kinds
        .iter()
        .map(|kind| {
            let changes: Vec<&str> = generation
                .drift
                .iter()
                .filter(|change| change.kind == *kind)
                .map(|change| change.change.as_str())
                .collect();
            let mark = if changes.iter().all(|change| *change == changes[0]) {
                marker(changes[0])
            } else {
                "~"
            };
            format!("{mark}{} {kind}", changes.len())
        })
        .collect();
    counted.extend(
        generation
            .runtime_drift
            .iter()
            .map(|change| format!("{}{}", marker(&change.change), change.kind)),
    );
    if !counted.is_empty() {
        segments.push(counted.join(" "));
    }
    Some(DriftLine {
        text: segments.join("  "),
        dim: false,
    })
}

/// `n`, `X.Yk` or `X.YM`, rounded down. Integer math keeps the web copy exact.
pub(crate) fn compact_count(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let tenths = n / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        let tenths = n / 100_000;
        format!("{}.{}M", tenths / 10, tenths % 10)
    }
}

pub(crate) fn headroom_line(view: &LedgerRunView) -> Option<String> {
    if view.error.is_some() {
        return None;
    }
    let total = view.headroom_total.as_ref()?;
    let percent = if total.input_tokens_before == 0 {
        0
    } else {
        total.saved_tokens * 100 / total.input_tokens_before
    };
    let mut parts = vec![
        format!("hr {} saved", compact_count(total.saved_tokens)),
        format!("{percent}%"),
    ];
    if let Some(cents) = total.estimated_cents {
        parts.push(format!("~${}.{:02}", cents / 100, cents % 100));
    }
    let mut line = parts.join(" · ");
    if view.runs > 1 {
        if let Some(now) = &view.headroom_now {
            line.push_str(&format!("  (now {})", compact_count(now.saved_tokens)));
        }
    }
    Some(line)
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib ledger_overlay 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tui/components/ledger_overlay.rs src/tui/components/mod.rs
git commit -m "feat(tui): Ledger drift and Headroom overlay line text"
```

---

### Task 8: TUI section stack and rows

**Files:**
- Modify: `src/tui/components/usage_overlay.rs`
- Modify: `src/tui/components/ledger_overlay.rs` (add `drift_rows`, `headroom_rows`)

**Interfaces:**
- Consumes: Task 7 functions.
- Produces:
  - `pub(crate) struct OverlayRow { pub text: String, pub style: Style }`
  - `pub(crate) fn usage_rows(summary: &UsageSummary, created_at: DateTime<Utc>, now: DateTime<Utc>, theme: &Theme) -> Vec<OverlayRow>`
  - `pub(crate) fn render_overlay_sections(frame: &mut Frame, pane: Rect, sections: &[Vec<OverlayRow>], theme: &Theme)`: sections stack top-down, right-aligned. Sections are added in order while each still fits (`width + 10 <= pane.width`, `rows + 2 <= pane.height`); the first one that doesn't fit stops the stack. Empty sections are skipped.
  - `pub(crate) fn render_usage_overlay(...)`: same signature as today, now a wrapper.
  - `pub(crate) fn drift_rows(view: &LedgerRunView, now: DateTime<Utc>, theme: &Theme) -> Vec<OverlayRow>`
  - `pub(crate) fn headroom_rows(view: &LedgerRunView, theme: &Theme) -> Vec<OverlayRow>`

- [ ] **Step 1: Write the failing tests**

Append to the tests in `src/tui/components/usage_overlay.rs`:

```rust
    fn row(text: &str) -> OverlayRow {
        OverlayRow {
            text: text.to_string(),
            style: Style::default(),
        }
    }

    fn draw_sections(w: u16, h: u16, sections: &[Vec<OverlayRow>]) -> Buffer {
        use ratatui::backend::TestBackend;
        let theme = Theme::default();
        let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| render_overlay_sections(f, f.area(), sections, &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn line_at(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
    }

    #[test]
    fn sections_stack_right_aligned_and_skip_empty_ones() {
        let buf = draw_sections(
            60,
            10,
            &[vec![], vec![row("ledger d861 current")], vec![row("hr 2.0k saved")]],
        );
        assert!(line_at(&buf, 0).trim_end().ends_with("ledger d861 current"));
        assert!(line_at(&buf, 1).trim_end().ends_with("hr 2.0k saved"));
    }

    #[test]
    fn a_section_too_wide_for_the_pane_is_dropped_with_the_rest() {
        let wide = "x".repeat(55);
        let buf = draw_sections(
            40,
            10,
            &[vec![row("first")], vec![row(&wide)], vec![row("third")]],
        );
        assert!(line_at(&buf, 0).contains("first"));
        assert!(!line_at(&buf, 1).contains('x'));
        assert!(!(0..10).any(|y| line_at(&buf, y).contains("third")));
    }
```

Append to the tests in `src/tui/components/ledger_overlay.rs`:

```rust
    #[test]
    fn rows_use_dim_ink_only_for_errors() {
        let theme = crate::tui::styles::Theme::default();
        for case in cases() {
            let drift = drift_rows(&case.view, case.now, &theme);
            if let Some(row) = drift.first() {
                let want = if case.view.error.is_some() { theme.dimmed } else { theme.text };
                assert_eq!(row.style.fg, Some(want), "{}", case.name);
            }
            let headroom = headroom_rows(&case.view, &theme);
            assert_eq!(headroom.len(), headroom_line(&case.view).iter().count(), "{}", case.name);
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib usage_overlay ledger_overlay 2>&1 | tail -20`
Expected: compile errors (`cannot find struct OverlayRow`).

- [ ] **Step 3: Implement**

In `src/tui/components/usage_overlay.rs`, replace `render_usage_overlay` with the three items below. `big_digits`, `format_duration_short`, `overlay_lines` and the blend loop body stay as they are. The loop moves into `render_overlay_sections` unchanged.

```rust
/// One right-aligned row of the overlay stack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayRow {
    pub text: String,
    pub style: Style,
}

/// The reset counter section: big digits, then the breakdown.
pub(crate) fn usage_rows(
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    theme: &Theme,
) -> Vec<OverlayRow> {
    let number = Style::default().fg(theme.error).bold();
    let detail = Style::default().fg(theme.text);
    big_digits(summary.resets)
        .into_iter()
        .map(|text| OverlayRow { text, style: number })
        .chain(
            overlay_lines(summary, created_at, now)
                .into_iter()
                .map(|text| OverlayRow { text, style: detail }),
        )
        .collect()
}

pub(crate) fn render_usage_overlay(
    frame: &mut Frame,
    pane: Rect,
    summary: &UsageSummary,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
    theme: &Theme,
) {
    render_overlay_sections(frame, pane, &[usage_rows(summary, created_at, now, theme)], theme);
}

/// Stacks sections top-down in the pane's top-right corner. A section joins
/// only while the stack still fits with room to spare; the first that does
/// not fit ends the stack, so nothing draws when the first section is cramped.
pub(crate) fn render_overlay_sections(
    frame: &mut Frame,
    pane: Rect,
    sections: &[Vec<OverlayRow>],
    theme: &Theme,
) {
    let mut rows: Vec<&OverlayRow> = Vec::new();
    let mut width = 0usize;
    for section in sections.iter().filter(|section| !section.is_empty()) {
        let section_width = section.iter().map(|row| row.text.width()).max().unwrap_or(0);
        let stacked_width = width.max(section_width);
        if (pane.width as usize) < stacked_width + 10
            || (pane.height as usize) < rows.len() + section.len() + 2
        {
            break;
        }
        width = stacked_width;
        rows.extend(section.iter());
    }
    if rows.is_empty() {
        return;
    }
    let width = width as u16;
    let height = rows.len() as u16;
    let area = Rect {
        x: pane.right() - width - 1,
        y: pane.y,
        width,
        height,
    };
    let text: Vec<Line> = rows
        .iter()
        .map(|row| Line::styled(row.text.clone(), row.style))
        .collect();

    // Terminals can't alpha-composite, so render the overlay into a scratch
    // buffer and blend it into the frame buffer by hand.
    let mut scratch = Buffer::empty(area);
    Paragraph::new(text)
        .alignment(Alignment::Right)
        .render(area, &mut scratch);

    // ... the existing `let buf = frame.buffer_mut(); for y in … { for x in … { … } }`
    //     blend loop, moved here verbatim ...
}
```

Move the existing blend loop (from `let buf = frame.buffer_mut();` to the end of the old `render_usage_overlay`) into `render_overlay_sections` without changing it. The comment above marks where it goes; don't paste the comment into the file.

In `src/tui/components/ledger_overlay.rs`, add:

```rust
use ratatui::style::Style;

use super::usage_overlay::OverlayRow;
use crate::tui::styles::Theme;

pub(crate) fn drift_rows(view: &LedgerRunView, now: DateTime<Utc>, theme: &Theme) -> Vec<OverlayRow> {
    drift_line(view, now)
        .map(|line| OverlayRow {
            style: Style::default().fg(if line.dim { theme.dimmed } else { theme.text }),
            text: line.text,
        })
        .into_iter()
        .collect()
}

pub(crate) fn headroom_rows(view: &LedgerRunView, theme: &Theme) -> Vec<OverlayRow> {
    headroom_line(view)
        .map(|text| OverlayRow {
            text,
            style: Style::default().fg(theme.text),
        })
        .into_iter()
        .collect()
}
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib usage_overlay ledger_overlay 2>&1 | tail -20`
Expected: PASS, including all five existing overlay tests (cramped, blend, reset background), unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/tui/components/usage_overlay.rs src/tui/components/ledger_overlay.rs
git commit -m "refactor(tui): overlay as a stack of sections, with Ledger rows"
```

---

### Task 9: TUI poller and wiring

**Files:**
- Create: `src/tui/ledger_poller.rs`
- Modify: `src/tui/mod.rs` (add `mod ledger_poller;` after `mod hyperlink…`, keeping the list alphabetical)
- Modify: `src/tui/home/mod.rs` (fields after `show_usage_overlay`, ~line 263)
- Modify: `src/tui/home/lifecycle.rs` (initialisers after `show_usage_overlay`, ~line 354)
- Modify: `src/tui/home/config_refresh.rs` (after `self.show_usage_overlay = …`, ~line 69)
- Modify: `src/tui/home/status.rs` (`request_ledger_refresh`, `apply_ledger_updates`, `apply_one_ledger_update` after the usage trio)
- Modify: `src/tui/home/input.rs` (~line 3970, selection change)
- Modify: `src/tui/app.rs` (~lines 592–606 and 1230–1234)
- Modify: `src/tui/home/render.rs` (`render_usage_overlay`, ~line 2758)
- Test: `src/tui/home/tests/status_rows_menu.rs` (next to `apply_one_usage_update_requests_a_refresh_when_the_selection_has_moved_on`)

**Interfaces:**
- Consumes: `crate::ledger_run::{load_view, LedgerRunView}` (Tasks 4–5); `usage_rows`, `render_overlay_sections` (Task 8); `drift_rows`, `headroom_rows` (Task 8).
- Produces:
  - `LedgerPoller::{new, request_refresh(Instance), try_recv_updates()}`
  - `HomeView::{request_ledger_refresh, apply_ledger_updates, apply_one_ledger_update}`
  - HomeView fields: `ledger_poller`, `pending_ledger_refresh`, `ledger_view: Option<(String, LedgerRunView)>`, `show_ledger_overlay`, `show_headroom_overlay`

- [ ] **Step 1: Write the failing tests**

Append to `src/tui/home/tests/status_rows_menu.rs`:

```rust
/// A Ledger view that lands after the selection moved on must not leave the
/// new selection's overlay waiting for the next 30 s tick.
#[test]
#[serial]
fn apply_one_ledger_update_requests_a_refresh_when_the_selection_has_moved_on() {
    let mut env = create_test_env_with_sessions(2);
    let first = session_id_at(&env.view, 0).unwrap();
    let second = session_id_at(&env.view, 1).unwrap();

    env.view.selected_session = Some(first.clone());
    env.view.request_ledger_refresh();
    assert!(env.view.pending_ledger_refresh);
    env.view.selected_session = Some(second.clone());
    env.view.request_ledger_refresh();

    env.view.pending_ledger_refresh = false;
    env.view
        .apply_one_ledger_update(first, Some(crate::ledger_run::LedgerRunView::default()));
    assert!(env.view.pending_ledger_refresh);
}

#[test]
#[serial]
fn ledger_refresh_is_idle_while_both_ledger_lines_are_off() {
    let mut env = create_test_env_with_sessions(1);
    env.view.selected_session = session_id_at(&env.view, 0);
    env.view.show_ledger_overlay = false;
    env.view.show_headroom_overlay = false;
    env.view.request_ledger_refresh();
    assert!(!env.view.pending_ledger_refresh);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `$SENTINEL cargo test --lib apply_one_ledger_update ledger_refresh_is_idle 2>&1 | tail -20`
Expected: compile errors (`no method named request_ledger_refresh`).

- [ ] **Step 3: Implement**

`src/tui/ledger_poller.rs`:

```rust
//! Background loader for the selected session's Ledger run view, so the
//! `ledger run show` calls stay off the render loop.

use crate::ledger_run::LedgerRunView;
use crate::session::Instance;
use crate::tui::worker::Worker;

pub struct LedgerPoller {
    worker: Worker<Instance, (String, Option<LedgerRunView>)>,
}

impl LedgerPoller {
    pub fn new() -> Self {
        Self {
            worker: Worker::spawn("aoe-ledger-poller", |instance: Instance| {
                let view = crate::ledger_run::load_view(&instance);
                (instance.id, view)
            }),
        }
    }

    pub fn request_refresh(&self, instance: Instance) {
        self.worker.request(instance);
    }

    pub fn try_recv_updates(
        &self,
    ) -> Result<(String, Option<LedgerRunView>), std::sync::mpsc::TryRecvError> {
        self.worker.try_recv()
    }
}

impl Default for LedgerPoller {
    fn default() -> Self {
        Self::new()
    }
}
```

`src/tui/home/mod.rs`, after `pub(super) show_usage_overlay: bool,`:

```rust
    pub(super) ledger_poller: super::ledger_poller::LedgerPoller,
    pub(super) pending_ledger_refresh: bool,
    /// Ledger run view for one instance id; drawn only while that id is selected.
    pub(super) ledger_view: Option<(String, crate::ledger_run::LedgerRunView)>,
    pub(super) show_ledger_overlay: bool,
    pub(super) show_headroom_overlay: bool,
```

`src/tui/home/lifecycle.rs`, after `show_usage_overlay: resolved.session.show_usage_overlay,`:

```rust
            ledger_poller: crate::tui::ledger_poller::LedgerPoller::new(),
            pending_ledger_refresh: false,
            ledger_view: None,
            show_ledger_overlay: resolved.session.show_ledger_overlay,
            show_headroom_overlay: resolved.session.show_headroom_overlay,
```

`src/tui/home/config_refresh.rs`, after `self.show_usage_overlay = config.session.show_usage_overlay;`:

```rust
        self.show_ledger_overlay = config.session.show_ledger_overlay;
        self.show_headroom_overlay = config.session.show_headroom_overlay;
```

`src/tui/home/status.rs`, after `apply_one_usage_update`:

```rust
    /// Request the selected session's Ledger run view while either Ledger line is on.
    pub fn request_ledger_refresh(&mut self) {
        if !(self.show_ledger_overlay || self.show_headroom_overlay) || self.pending_ledger_refresh {
            return;
        }
        let Some(instance) = self
            .selected_session
            .as_deref()
            .and_then(|id| self.get_instance(id))
            .cloned()
        else {
            return;
        };
        self.ledger_poller.request_refresh(instance);
        self.pending_ledger_refresh = true;
    }

    /// Apply a loaded Ledger run view; true when the overlay needs a repaint.
    pub fn apply_ledger_updates(&mut self) -> bool {
        use std::sync::mpsc::TryRecvError;

        match self.ledger_poller.try_recv_updates() {
            Ok((id, view)) => {
                self.pending_ledger_refresh = false;
                self.apply_one_ledger_update(id, view)
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                tracing::error!(target: "tui.home", "ledger poller worker gone; respawning");
                self.ledger_poller = crate::tui::ledger_poller::LedgerPoller::new();
                self.pending_ledger_refresh = false;
                false
            }
        }
    }

    /// Store a fetched view; true when the overlay needs a repaint. Split out
    /// so tests can drive it without a background thread.
    pub(in crate::tui) fn apply_one_ledger_update(
        &mut self,
        id: String,
        view: Option<crate::ledger_run::LedgerRunView>,
    ) -> bool {
        if self.selected_session.as_deref() != Some(id.as_str()) {
            self.request_ledger_refresh();
        }
        let next = view.map(|view| (id, view));
        let changed = next != self.ledger_view;
        self.ledger_view = next;
        changed
    }
```

`src/tui/home/input.rs`: in the `if self.selected_session != prev_session {` block, after `self.request_usage_refresh();`, add `self.request_ledger_refresh();`.

`src/tui/app.rs`: next to `let mut last_usage_refresh = …` add `let mut last_ledger_refresh: Option<std::time::Instant> = None;`, and next to `const USAGE_REFRESH_INTERVAL` add:

```rust
        const LEDGER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
```

After the usage refresh block in the loop, add:

```rust
            if last_ledger_refresh.is_none_or(|at| at.elapsed() >= LEDGER_REFRESH_INTERVAL) {
                self.home.request_ledger_refresh();
                last_ledger_refresh = Some(std::time::Instant::now());
            }
            refresh_needed |= self.home.apply_ledger_updates();
```

`src/tui/home/render.rs`: replace `render_usage_overlay` (keep its name; the three call sites stay):

```rust
    /// Draws the overlay stack over the preview's top-right corner: the reset
    /// counter, then the Ledger drift and Headroom lines, each only when it is
    /// switched on and loaded for the selected session.
    fn render_usage_overlay(&self, frame: &mut Frame, theme: &Theme) {
        use crate::tui::components::{ledger_overlay, usage_overlay};
        if self.system_health_open {
            return;
        }
        let Some(selected) = self.selected_session.as_deref() else {
            return;
        };
        let Some(instance) = self.get_instance(selected) else {
            return;
        };
        let now = chrono::Utc::now();
        let mut sections = Vec::new();
        if self.show_usage_overlay {
            if let Some((id, summary)) = &self.usage_summary {
                if id == selected && summary.tracked {
                    sections.push(usage_overlay::usage_rows(summary, instance.created_at, now, theme));
                }
            }
        }
        if let Some((id, view)) = &self.ledger_view {
            if id == selected {
                if self.show_ledger_overlay {
                    sections.push(ledger_overlay::drift_rows(view, now, theme));
                }
                if self.show_headroom_overlay {
                    sections.push(ledger_overlay::headroom_rows(view, theme));
                }
            }
        }
        usage_overlay::render_overlay_sections(frame, self.preview_pane_area, &sections, theme);
    }
```

- [ ] **Step 4: Run tests**

Run: `$SENTINEL cargo test --lib tui:: 2>&1 | tail -30`
Expected: PASS. The known failing tmux env test is not in `tui::`.

- [ ] **Step 5: Commit**

```bash
git add src/tui
git commit -m "feat(tui): poll and draw the Ledger run overlay lines"
```

---

### Task 10: Web API and overlay

**Files:**
- Modify: `src/server/api/sessions/usage.rs` (new handler)
- Modify: `src/server/router.rs` (route after `/api/sessions/{id}/usage`)
- Test: `src/server/api/sessions/tests.rs` (next to the usage tests, ~line 3424)
- Create: `web/src/lib/ledgerRun.ts`, `web/src/lib/ledgerRun.test.ts`
- Modify: `web/src/lib/api.ts`, `web/src/components/UsageOverlay.tsx`, `web/src/components/__tests__/UsageOverlay.test.tsx`, `web/src/App.tsx`, `web/tests/coverage-matrix.json`

**Interfaces:**
- Consumes: `crate::ledger_run::load_view` (Task 5).
- Produces:
  - `GET /api/sessions/{id}/ledger-run` returns the `LedgerRunView` JSON, or `null` for a session Ledger didn't launch. Unknown id gives 404; CityHall mode gives 403.
  - TS: `LedgerRunView` types, `driftLine(view, now)`, `headroomLine(view)`, `compactCount(n)`, the gates `LedgerOverlayEnabledContext` / `parseLedgerOverlayEnabled` / `useLedgerOverlayEnabled` and `HeadroomOverlayEnabledContext` / `parseHeadroomOverlayEnabled` / `useHeadroomOverlayEnabled`, and `fetchSessionLedgerRun(id)`.

- [ ] **Step 1: Write the failing Rust test**

Append to `src/server/api/sessions/tests.rs`:

```rust
// GET /api/sessions/{id}/ledger-run is null for a session Ledger did not
// launch, 404 for an unknown id, and closed in CityHall like /usage.
#[tokio::test]
#[serial_test::serial]
async fn session_ledger_run_is_null_without_a_ledger_launch() {
    use axum::body::to_bytes;

    let _home = crate::session::test_support::isolate_app_dir();
    let inst = Instance::new("ledger-run-overlay", "/tmp/ledger-run-overlay");
    let id = inst.id.clone();
    let state = crate::server::test_support::build_test_app_state(vec![inst]);

    let resp = session_ledger_run(State(state.clone()), Path(id))
        .await
        .into_response();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(&body[..], b"null");

    let resp = session_ledger_run(State(state), Path("does-not-exist".to_string()))
        .await
        .into_response();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let inst = Instance::new("ledger-run-cityhall", "/tmp/ledger-run-cityhall");
    let id = inst.id.clone();
    let state = crate::server::test_support::build_test_app_state_cityhall(vec![inst]);
    let resp = session_ledger_run(State(state), Path(id)).await.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `$SENTINEL cargo test --features web --lib session_ledger_run 2>&1 | tail -20`
Expected: compile error, `cannot find function session_ledger_run`.

- [ ] **Step 3: Implement the route**

Append to `src/server/api/sessions/usage.rs`:

```rust
/// The session's Ledger run view for the web overlay; `null` when Ledger did
/// not launch the session's agent.
pub async fn session_ledger_run(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if let Some(resp) = crate::server::api::cityhall_block(&state) {
        return resp;
    }
    let Some(instance) = find_instance(&state, &id).await else {
        return bare_not_found();
    };
    let view = tokio::task::spawn_blocking(move || crate::ledger_run::load_view(&instance))
        .await
        .ok()
        .flatten();
    Json(view).into_response()
}
```

In `src/server/router.rs`, after the `/usage` route:

```rust
        .route("/api/sessions/{id}/ledger-run", get(api::session_ledger_run))
```

Run: `$SENTINEL cargo test --features web --lib session_ledger_run session_usage 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Write the failing web tests**

`web/src/lib/ledgerRun.test.ts`:

```ts
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  compactCount,
  driftLine,
  headroomLine,
  parseHeadroomOverlayEnabled,
  parseLedgerOverlayEnabled,
  type LedgerRunView,
} from "./ledgerRun";

interface Case {
  name: string;
  now: string;
  view: LedgerRunView;
  lines: string[];
}

// Shared with the TUI's ledger_overlay tests.
const cases: Case[] = JSON.parse(
  readFileSync(new URL("../../../tests/fixtures/ledger-run/cases.json", import.meta.url), "utf8"),
);

describe("ledger overlay text", () => {
  it.each(cases.map((c) => [c.name, c] as const))("matches the TUI for %s", (_name, c) => {
    const now = Date.parse(c.now);
    const lines = [driftLine(c.view, now)?.text, headroomLine(c.view)].filter(
      (line): line is string => typeof line === "string",
    );
    expect(lines).toEqual(c.lines);
  });

  it("dims only the error line", () => {
    for (const c of cases) {
      expect(driftLine(c.view, Date.parse(c.now))?.dim ?? false).toBe(c.view.error !== null);
    }
  });

  it("rounds counts down with integer math", () => {
    expect([0, 999, 1_000, 26_500, 999_999, 1_250_000, 1_132_000].map(compactCount)).toEqual([
      "0",
      "999",
      "1.0k",
      "26.5k",
      "999.9k",
      "1.2M",
      "1.1M",
    ]);
  });

  it("defaults both lines on", () => {
    expect(parseLedgerOverlayEnabled(null)).toBe(true);
    expect(parseHeadroomOverlayEnabled(null)).toBe(true);
    expect(parseLedgerOverlayEnabled({ session: { show_ledger_overlay: false } })).toBe(false);
    expect(parseHeadroomOverlayEnabled({ session: { show_headroom_overlay: false } })).toBe(false);
  });
});
```

Add to `web/src/components/__tests__/UsageOverlay.test.tsx`:
- change the mock to `vi.mock("../../lib/api", () => ({ fetchSessionUsage: vi.fn(), fetchSessionLedgerRun: vi.fn() }));`;
- import `fetchSessionLedgerRun` alongside `fetchSessionUsage`, plus `LedgerOverlayEnabledContext, HeadroomOverlayEnabledContext, type LedgerRunView` from `../../lib/ledgerRun`;
- add `beforeEach(() => vi.mocked(fetchSessionLedgerRun).mockResolvedValue(null));` (import `beforeEach` from vitest);
- add these tests:

```tsx
const ledgerView: LedgerRunView = {
  run_id: "run_00000000000000000000000000000006",
  runs: 2,
  generation: {
    state: "current",
    launched: "d861a7c0e5f1",
    current: "d861a7c0e5f1",
    current_since: "2026-10-01T08:00:00Z",
    drift: [],
    runtime_drift: [],
  },
  headroom_total: {
    requests: 105,
    input_tokens_before: 1_100_000,
    input_tokens_after: 773_500,
    saved_tokens: 326_500,
    model: "claude-opus-5-5",
    estimated_cents: 100,
  },
  headroom_now: {
    requests: 5,
    input_tokens_before: 100_000,
    input_tokens_after: 73_500,
    saved_tokens: 26_500,
    model: "claude-opus-5-5",
    estimated_cents: 10,
  },
  error: null,
};

function renderOverlay({ usage = true, ledger = true, headroom = true } = {}) {
  return render(
    <UsageOverlayEnabledContext.Provider value={usage}>
      <LedgerOverlayEnabledContext.Provider value={ledger}>
        <HeadroomOverlayEnabledContext.Provider value={headroom}>
          <UsageOverlay session={session} />
        </HeadroomOverlayEnabledContext.Provider>
      </LedgerOverlayEnabledContext.Provider>
    </UsageOverlayEnabledContext.Provider>,
  );
}

describe("UsageOverlay Ledger lines", () => {
  it("shows both Ledger lines for a session without usage tracking", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderOverlay();
    expect((await screen.findByTestId("ledger-drift-line")).textContent).toBe("ledger d861 current");
    expect(screen.getByTestId("ledger-headroom-line").textContent).toBe(
      "hr 326.5k saved · 29% · ~$1.00  (now 26.5k)",
    );
  });

  it("hides each line with its own setting", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderOverlay({ ledger: false });
    await screen.findByTestId("ledger-headroom-line");
    expect(screen.queryByTestId("ledger-drift-line")).toBeNull();
    cleanup();
    renderOverlay({ headroom: false });
    await screen.findByTestId("ledger-drift-line");
    expect(screen.queryByTestId("ledger-headroom-line")).toBeNull();
  });

  it("does not ask for the Ledger view while both lines are off", async () => {
    mockFetch.mockResolvedValue(summary());
    renderOverlay({ ledger: false, headroom: false });
    await screen.findByText("7");
    expect(vi.mocked(fetchSessionLedgerRun)).not.toHaveBeenCalled();
  });
});
```

Run: `cd web && npx vitest run src/lib/ledgerRun.test.ts src/components/__tests__/UsageOverlay.test.tsx`
Expected: FAIL (`Failed to resolve import "./ledgerRun"`).

- [ ] **Step 5: Implement the web side**

`web/src/lib/ledgerRun.ts`:

```ts
import { sessionFlagGate } from "./sessionFlagGate";
import { formatDurationShort } from "./usage";

/** Mirrors `crate::ledger_run::LedgerRunView` (snake_case on the wire). */
export interface LedgerChange {
  kind: string;
  name: string;
  change: string;
}

export interface LedgerRuntimeChange {
  kind: string;
  change: string;
}

export interface LedgerGeneration {
  state: string;
  launched: string;
  current: string;
  current_since: string;
  drift: LedgerChange[];
  runtime_drift: LedgerRuntimeChange[];
}

export interface HeadroomTotals {
  requests: number;
  input_tokens_before: number;
  input_tokens_after: number;
  saved_tokens: number;
  model: string;
  estimated_cents: number | null;
}

export interface LedgerRunView {
  run_id: string;
  runs: number;
  generation: LedgerGeneration | null;
  headroom_total: HeadroomTotals | null;
  headroom_now: HeadroomTotals | null;
  error: string | null;
}

/** On by default, matching `session.show_ledger_overlay`. */
const ledgerGate = sessionFlagGate("show_ledger_overlay", true);
export const LedgerOverlayEnabledContext = ledgerGate.Context;
export const parseLedgerOverlayEnabled = ledgerGate.parse;
export const useLedgerOverlayEnabled = ledgerGate.use;

/** On by default, matching `session.show_headroom_overlay`. */
const headroomGate = sessionFlagGate("show_headroom_overlay", true);
export const HeadroomOverlayEnabledContext = headroomGate.Context;
export const parseHeadroomOverlayEnabled = headroomGate.parse;
export const useHeadroomOverlayEnabled = headroomGate.use;

export { fetchSessionLedgerRun } from "./api";

const MAX_SKILLS = 6;

function marker(change: string): string {
  switch (change) {
    case "older":
      return "↓";
    case "newer":
      return "↑";
    case "extra":
      return "+";
    case "missing":
      return "-";
    default:
      return "~";
  }
}

const short = (id: string) => [...id].slice(0, 4).join("");

/** Mirrors the TUI's `drift_line`. */
export function driftLine(view: LedgerRunView, now: number): { text: string; dim: boolean } | null {
  if (view.error !== null) return { text: `ledger ? ${view.error}`, dim: true };
  const g = view.generation;
  if (!g) return null;
  if (g.state === "current") return { text: `ledger ${short(g.launched)} current`, dim: false };
  const since = Date.parse(g.current_since);
  const age = Number.isNaN(since) ? "?" : formatDurationShort(now - since);
  const segments = [`ledger ${g.state} ${short(g.launched)}↔${short(g.current)} ${age}`];
  const skills = g.drift.filter((c) => c.kind === "skill");
  const named = skills.slice(0, MAX_SKILLS).map((c) => `${marker(c.change)}${c.name}`);
  if (skills.length > MAX_SKILLS) named.push(`+${skills.length - MAX_SKILLS} more`);
  if (named.length > 0) segments.push(named.join(" "));
  const kinds: string[] = [];
  for (const c of g.drift) if (c.kind !== "skill" && !kinds.includes(c.kind)) kinds.push(c.kind);
  const counted = kinds.map((kind) => {
    const changes = g.drift.filter((c) => c.kind === kind).map((c) => c.change);
    const mark = changes.every((c) => c === changes[0]) ? marker(changes[0]) : "~";
    return `${mark}${changes.length} ${kind}`;
  });
  counted.push(...g.runtime_drift.map((c) => `${marker(c.change)}${c.kind}`));
  if (counted.length > 0) segments.push(counted.join(" "));
  return { text: segments.join("  "), dim: false };
}

/** Mirrors the TUI's `compact_count`: rounded down, integer math. */
export function compactCount(n: number): string {
  if (n < 1_000) return String(n);
  if (n < 1_000_000) {
    const tenths = Math.floor(n / 100);
    return `${Math.floor(tenths / 10)}.${tenths % 10}k`;
  }
  const tenths = Math.floor(n / 100_000);
  return `${Math.floor(tenths / 10)}.${tenths % 10}M`;
}

/** Mirrors the TUI's `headroom_line`. */
export function headroomLine(view: LedgerRunView): string | null {
  if (view.error !== null) return null;
  const total = view.headroom_total;
  if (!total) return null;
  const percent =
    total.input_tokens_before === 0 ? 0 : Math.floor((total.saved_tokens * 100) / total.input_tokens_before);
  const parts = [`hr ${compactCount(total.saved_tokens)} saved`, `${percent}%`];
  if (total.estimated_cents !== null) {
    const cents = total.estimated_cents;
    parts.push(`~$${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`);
  }
  let line = parts.join(" · ");
  if (view.runs > 1 && view.headroom_now) line += `  (now ${compactCount(view.headroom_now.saved_tokens)})`;
  return line;
}
```

`web/src/lib/api.ts`: add `import type { LedgerRunView } from "./ledgerRun";` beside the `UsageSummary` import, and after `fetchSessionUsage`:

```ts
export function fetchSessionLedgerRun(id: string): Promise<LedgerRunView | null> {
  return fetchJson<LedgerRunView | null>(`/api/sessions/${encodeURIComponent(id)}/ledger-run`);
}
```

`web/src/components/UsageOverlay.tsx`: replace the whole file:

```tsx
import { useEffect, useState } from "react";
import type { SessionResponse } from "../lib/types";
import {
  driftLine,
  fetchSessionLedgerRun,
  headroomLine,
  useHeadroomOverlayEnabled,
  useLedgerOverlayEnabled,
  type LedgerRunView,
} from "../lib/ledgerRun";
import { fetchSessionUsage, usageLines, useUsageOverlayEnabled, type UsageSummary } from "../lib/usage";

const POLL_MS = 5_000;
const LEDGER_POLL_MS = 30_000;

/** Wraps the impure `Date.now()` read outside the component body, matching
 *  `ConnectedDevices`' `relativeTime` helper. */
function currentUsageLines(summary: UsageSummary, createdAt: string): string[] {
  return usageLines(summary, createdAt, Date.now());
}

function currentDriftLine(view: LedgerRunView) {
  return driftLine(view, Date.now());
}

/** The session's overlay stack over the terminal's top-right corner: the
 *  context-reset count, big and red, then the Ledger drift and Headroom lines. */
export function UsageOverlay({ session }: { session: SessionResponse }) {
  const usageEnabled = useUsageOverlayEnabled();
  const ledgerEnabled = useLedgerOverlayEnabled();
  const headroomEnabled = useHeadroomOverlayEnabled();
  const ledgerWanted = ledgerEnabled || headroomEnabled;
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [ledger, setLedger] = useState<LedgerRunView | null>(null);

  useEffect(() => {
    if (!usageEnabled) return;
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
  }, [usageEnabled, session.id]);

  useEffect(() => {
    if (!ledgerWanted) return;
    let cancelled = false;
    const load = () => {
      if (document.visibilityState !== "visible") return;
      void fetchSessionLedgerRun(session.id).then((next) => {
        if (!cancelled) setLedger(next);
      });
    };
    load();
    const timer = window.setInterval(load, LEDGER_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [ledgerWanted, session.id]);

  const showUsage = usageEnabled && !!summary?.tracked;
  const drift = ledgerEnabled && ledger ? currentDriftLine(ledger) : null;
  const headroom = headroomEnabled && ledger ? headroomLine(ledger) : null;
  if (!showUsage && !drift && !headroom) return null;
  return (
    <div
      className="pointer-events-none absolute top-2 right-3 z-10 hidden rounded-md bg-surface-900/25 px-2 py-1 text-right sm:block"
      data-testid="usage-overlay"
      title={showUsage ? "Context resets: clears + compactions + resumes" : "Ledger run"}
    >
      {showUsage && summary && (
        <>
          <div className="text-3xl font-black leading-none text-status-error tabular-nums opacity-90">{summary.resets}</div>
          {currentUsageLines(summary, session.created_at).map((line) => (
            <div key={line} className="mt-1 font-mono text-[10px] text-text-secondary opacity-90">
              {line}
            </div>
          ))}
        </>
      )}
      {drift && (
        <div
          data-testid="ledger-drift-line"
          className={`mt-1 whitespace-pre font-mono text-[10px] opacity-90 ${drift.dim ? "text-text-dim" : "text-text-secondary"}`}
        >
          {drift.text}
        </div>
      )}
      {headroom && (
        <div data-testid="ledger-headroom-line" className="mt-1 whitespace-pre font-mono text-[10px] text-text-secondary opacity-90">
          {headroom}
        </div>
      )}
    </div>
  );
}
```

`whitespace-pre` keeps the double spaces between segments, matching the TUI.

`web/src/App.tsx`:
- import `parseLedgerOverlayEnabled, LedgerOverlayEnabledContext, parseHeadroomOverlayEnabled, HeadroomOverlayEnabledContext` from `./lib/ledgerRun`;
- add `const [ledgerOverlayEnabled, setLedgerOverlayEnabled] = useState(true);` and `const [headroomOverlayEnabled, setHeadroomOverlayEnabled] = useState(true);` after `usageOverlayEnabled`;
- in `applyAppSettings` add `setLedgerOverlayEnabled(parseLedgerOverlayEnabled(settings));` and `setHeadroomOverlayEnabled(parseHeadroomOverlayEnabled(settings));`;
- nest `<LedgerOverlayEnabledContext.Provider value={ledgerOverlayEnabled}><HeadroomOverlayEnabledContext.Provider value={headroomOverlayEnabled}>…</HeadroomOverlayEnabledContext.Provider></LedgerOverlayEnabledContext.Provider>` directly inside `UsageOverlayEnabledContext.Provider`, around its current children.

`web/tests/coverage-matrix.json`, entry `sessions.usage-overlay`: add `"src/lib/ledgerRun.test.ts"` to `specs` and `"web/src/lib/ledgerRun.ts"` to `components`.

- [ ] **Step 6: Run the web checks**

Run: `cd web && npx vitest run src/lib/ledgerRun.test.ts src/lib/usage.test.ts src/components/__tests__/UsageOverlay.test.tsx && npm run lint && npx tsc -b && npm run format:check`
Expected: all pass. If `format:check` fails, run `npm run format` and re-check.

- [ ] **Step 7: Commit**

```bash
git add src/server web
git commit -m "feat(web): Ledger drift and Headroom lines in the terminal overlay"
```

---

### Task 11: Contract doc, full verification, deploy, live check, PR

**Files:**
- Create: `docs/development/ledger-run.md`

- [ ] **Step 1: Write the contract doc**

`docs/development/ledger-run.md`:

````markdown
# Ledger run overlay

For a session Ledger launched (the agent pane carries `@aoe_ledger_launch`),
the preview overlay and the web terminal overlay show two lines under the
reset counter:

```
ledger behind d861↔f00d 3h0m  +caveman -dataviz ↓tdd  ↓2 settings ↓renderer
hr 1.1M saved · 28% · ~$5.66  (now 26.5k)
```

## Source

aoe runs `ledger run show <run_id> --json` (Ledger's
`ledger/docs/2026-10-01-ledger-run-show.md`) and reads these `ledger.run.v1`
fields: `schema`, `run_id`, `finished_at`, `generation.{state, launched,
current, current_since, drift[], runtime_drift[]}`, and
`headroom.{requests, input_tokens_before, input_tokens_after, saved_tokens,
model}`. Exit 2 means an unknown run; any other non-zero exit, a schema
other than `ledger.run.v1`, or unparseable output is `ledger error`.

The session's runs come from its `ledger_run` usage events
([usage events](usage-events.md)) plus the pane's current run. Drift comes
from the current run. The Headroom total sums every run that has receipts,
and `(now …)` is the current run's share once there is more than one run.

## Cadence and errors

The TUI asks for the selected session every 30 s and on selection change,
from a worker thread. The web asks every 30 s through
`GET /api/sessions/{id}/ledger-run` (`null` for non-Ledger sessions).
Each `ledger` call has a 2 s deadline and a 64 KiB output cap.
Finished runs are cached for the process lifetime. A missing `ledger`
binary pauses calls for 5 minutes. Errors render as a dimmed
`ledger ? <reason>` (`no ledger`, `timeout`, `unknown run`, `ledger
error`), and the Headroom line is hidden.

## Text

- `↓` older, `↑` newer, `+` extra, `-` missing (from the session's point of
  view). Skills are named, up to six, then `+N more`. Other kinds collapse
  to `<marker><count> <kind>` (`~` when mixed); runtime drift shows as
  `<marker><kind>`.
- Counts are `n`, `X.Yk`, `X.YM`, rounded down. The percentage is saved /
  before, rounded down. The dollar figure is `~$` plus whole cents from
  `src/usage/prices.rs`; unknown models (currently all GPT models) get none.
- `session.show_ledger_overlay` and `session.show_headroom_overlay` gate the
  two lines.

`src/tui/components/ledger_overlay.rs` and `web/src/lib/ledgerRun.ts` render
the same text, pinned by `tests/fixtures/ledger-run/cases.json`.
````

- [ ] **Step 2: Full verification**

Run each in turn, from the worktree:

```bash
cargo fmt -- --check
S=$(mktemp -d); env -u AOE_INSTANCE_ID -u AOE_HOOK_BIN -u AOE_PROFILE CLAUDE_CONFIG_DIR=$S/claude CODEX_HOME=$S/codex CARGO_TARGET_DIR=$HOME/code/agent-of-empires/target cargo test --features web --lib 2>&1 | tail -30
S=$(mktemp -d); env -u AOE_INSTANCE_ID -u AOE_HOOK_BIN -u AOE_PROFILE CLAUDE_CONFIG_DIR=$S/claude CODEX_HOME=$S/codex CARGO_TARGET_DIR=$HOME/code/agent-of-empires/target cargo test --features web --test integration --test branch_exists_spawn_failure --test filewatch_degradation --bins 2>&1 | tail -30
CARGO_TARGET_DIR=$HOME/code/agent-of-empires/target cargo clippy --all-targets --features web 2>&1 | grep -E "^(warning|error)" | sort | uniq -c
cd web && npm run test:unit 2>&1 | tail -15 && npm run lint && npx tsc -b
ledger profile status personal --harness claude-code | head -5
```

Expected:
- every test passes except the known pre-existing failures listed in Global Constraints;
- clippy shows no warnings beyond the known 7 (check a new one with `git diff --quiet upstream/main -- <file>`);
- the Ledger profile still reports `state: current`. If it doesn't, the sentinel leaked: stop and repair it per memory `aoe-cli-from-inside-ledger-claude`.

Report the actual output; don't summarize it as "all green" if it wasn't.

- [ ] **Step 3: Commit the doc**

```bash
git add docs/development/ledger-run.md
git commit -m "docs: Ledger run overlay contract"
```

- [ ] **Step 4: Deploy (ask the user first; both binaries are live tools)**

This depends on the Ledger PR from the companion plan being merged.

```bash
# Ledger: install, then restart the on-demand daemon so it runs the new binary.
cd ~/code/attic && git -C ~/code/attic fetch origin
cd ~/code/attic-worktrees/ledger-run-show/ledger && git log -1 --oneline   # or a clean checkout of merged main
cp ~/code/gocode/bin/ledger ~/.ledger/backups/ledger.pre-run-show-20261001
GOPATH=$HOME/code/gocode go install ./cmd/ledger
pgrep -fl 'ledger daemon' && pkill -f 'ledger daemon --idle-exit'
ledger run show "$(sqlite3 -readonly ~/.ledger/ledger.db 'select id from launch_runs order by started_at desc limit 1')" --json | head -c 400; echo

# aoe: per memory aoe-local-deploy-recipe (no new migrations, so no state snapshot needed).
cd /Users/jws/code/agent-of-empires-worktrees/session-counter
export CARGO_TARGET_DIR=$HOME/code/agent-of-empires/target
cargo build --profile dev-release --features web --target aarch64-apple-darwin
mv ~/.local/bin/aoe ~/.local/bin/aoe.pre-ledger-overlay-20261001
cp $CARGO_TARGET_DIR/aarch64-apple-darwin/dev-release/aoe ~/.local/bin/aoe.new && mv ~/.local/bin/aoe.new ~/.local/bin/aoe
```

Update memory `aoe-local-deploy-recipe` with the new backup name.

- [ ] **Step 5: Live check (with the user)**

Already-running `lh` sessions still run the old `ledger run` supervisor in memory, so they never write a live totals file. Each must be restarted from aoe before its `hr` line appears. The user restarts the aoe TUI and one `lh` session, then:
1. Select the session. Within 30 s, the drift line appears, and the `hr` line appears after its first request.
2. Send a prompt, wait one tick, and confirm the `hr` numbers move.
3. Restart the session from aoe. Confirm `(now …)` appears and that the total kept the first run's savings.
4. Open the same session in the web dashboard. Confirm the same two lines.
5. Toggle each setting in the TUI settings and confirm only its own line disappears.

- [ ] **Step 6: Push and open the fork PR (ask the user first)**

```bash
git push -u origin feature/ledger-drift-overlay
gh pr create --repo crasiak/agent-of-empires --base main --title "feat: Ledger run overlay with generation drift and Headroom savings" --body "$(cat <<'EOF'
## Description
Adds two lines under the reset counter (TUI preview and web terminal) for Ledger-launched sessions: the session's generation drift against what a new launch would get, and Headroom input-token savings with a cents estimate. Data comes from the new `ledger run show --json` (crasiak/attic PR …). `report-ledger-launch` now records each run as a `ledger_run` usage event so restarted sessions keep their earlier savings. Spec: `docs/superpowers/specs/2026-10-01-ledger-run-overlay-design.md`; contract: `docs/development/ledger-run.md`.

## PR Type

- [x] New Feature
- [ ] Bug Fix
- [x] Refactor
- [x] Documentation
- [ ] Infrastructure / CI

## Checklist

- [x] New and existing tests pass
- [x] Documentation was updated where necessary
- [ ] For UI changes: included screenshot or recording

## Test Coverage Analysis

**Web dashboard / structured view (Playwright user story)**
- [ ] N/A: this PR doesn't change a user-facing dashboard flow
- [ ] Added or updated a Playwright spec under `web/tests/` and updated `web/tests/coverage-matrix.json`
- [x] Skipped a test, because: the overlay is read-only; vitest covers the text against the fixtures shared with the TUI, plus the per-setting gating; coverage-matrix updated.

**TUI / CLI (e2e test)**
- [ ] N/A: this PR doesn't change TUI rendering, a CLI subcommand, or session lifecycle
- [ ] Added or updated an e2e test under `tests/e2e/`
- [x] Skipped a test, because: needs a live Ledger run; unit tests render the section stack and line text from shared fixtures, and the live check was done by hand.

## AI Usage

Planned and implemented with Claude Code (subagent-driven), reviewed task by task.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Fill in the attic PR link, and paste the actual test results into the description instead of ticking boxes that weren't verified.
