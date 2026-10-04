# Ledger run overlay: generation drift and Headroom savings

**Date:** 2026-10-01
**Status:** Design approved in chat (drift: 2026-09-30, Headroom + merge: 2026-10-01). Visual details get tuned once it is live.
**Repos:** agent-of-empires (crasiak fork, branch `feature/ledger-drift-overlay`) and attic `ledger/`.
**Supersedes:** the drift-only design in `~/corelight/labs/artifacts/jws/cheatsheets/2026-09-30-aoe-ledger-drift-and-overlap-detection.md` (its decisions carry over unchanged except where noted).

## Goal

For a Ledger-launched aoe session, show two more lines in the translucent overlay stack under the big red resets counter:

1. **Drift:** which Ledger generation the session launched on, and how it differs from what a new launch would get.
2. **Headroom:** how many input tokens Headroom compression saved for this session, the percentage, and an estimated dollar figure.

Both lines come from one new read-only Ledger command and one aoe poller. The TUI and web overlays stay in sync.

## Scope

- Sessions with the `@aoe_ledger_launch` pane option (`l` and `lh` badges, Claude, Codex, Pi).
- The Headroom line only appears for runs that carry Headroom receipts (`lh`). Plain `h` sessions share one proxy per account and cannot be attributed per session; they are out of scope.
- v1 is display only: no diff view, no two-session compare, no cache-read savings (receipts don't carry them).

## Ledger side (attic `ledger/`)

### Live Headroom totals file

The `ledger run` supervisor already reads every receipt as it arrives (`readHeadroomReceipts` in `cmd/ledger/headroom_transport.go`, one goroutine per lease in `headroom_swap.go`). It holds them in memory until the native agent exits, so nothing outside the supervisor can see live savings today.

Change: after each validated receipt, the supervisor updates a per-run totals file:

- Path: `~/.ledger/run/<run_id>.headroom.json`, next to the existing control socket `~/.ledger/run/<run_id>.sock`.
- Mode `0600`, written to a temp file in the same directory, fsync-free, then `rename` over the target. Readers never see a partial file.
- Contents (schema `ledger.run-headroom-live.v1`):

  ```json
  {
    "schema": "ledger.run-headroom-live.v1",
    "run_id": "run_…",
    "requests": 42,
    "input_tokens_before": 3912000,
    "input_tokens_after": 2780000,
    "saved_tokens": 1132000,
    "model": "global.anthropic.claude-opus-5",
    "updated_at": "2026-10-01T12:04:19.752Z"
  }
  ```

- Totals span every lease of the run (planned swaps, CRA-511): the accumulator is shared across leases and guarded by a mutex.
- `model` is the model of the most recent receipt.
- Only receipts with `accounting_status = "evaluated"` add to the token counts; every receipt adds to `requests`.
- The supervisor deletes the file after the finish record is written or spooled. A stale file left by a crashed supervisor is harmless: `ledger run show` prefers the finish data once ledgerd has it.
- A failed write is logged once as a diagnostic event (`headroom_totals_write_failed`) and never fails the run.

### `ledger run show <run_id> --json`

New read-only subcommand. It does not talk to ledgerd; it opens `~/.ledger/ledger.db` read-only and reads the totals file. Output schema `ledger.run.v1`:

```json
{
  "schema": "ledger.run.v1",
  "run_id": "run_…",
  "harness": "claude",
  "profile": "personal",
  "started_at": "…",
  "finished_at": null,
  "generation": {
    "state": "behind",
    "launched": "d861…",
    "current": "f00d…",
    "current_since": "…",
    "drift": [{"kind": "skill", "name": "tdd", "change": "older"}],
    "runtime_drift": [{"kind": "renderer", "change": "newer"}]
  },
  "headroom": {
    "source": "live",
    "requests": 42,
    "input_tokens_before": 3912000,
    "input_tokens_after": 2780000,
    "saved_tokens": 1132000,
    "model": "global.anthropic.claude-opus-5",
    "updated_at": "…"
  }
}
```

**`generation`**, unchanged from the 2026-09-30 design:

- Sources: `launch_runs.materialization_id`, `active_materializations`, `materialization_activations`, `profile_lock_resources` joined to `resources(kind, logical_name)` and `resource_versions.created_at`.
- `change` is `older` / `newer` / `missing` / `extra`, from the session's point of view. Order comes from `resource_versions.created_at`; hashes are content-addressed, so a byte-identical revert reads as `older`.
- `state` is `current` / `behind` / `ahead` / `diverged`.
- `runtime_drift[]` covers renderer, path policy and adapter.
- No activation counts.

**`headroom`:**

- Finished run (a `launch_run_finishes` row exists): sum the run's `changes` rows with `event_type = 'transport.receipt'` and `aggregate_id = <run_id>`, using the same `evaluated` rule. `source: "finished"`.
- Live run: read the totals file. `source: "live"`.
- Neither: `headroom` is `null` (a non-Headroom run, or an `lh` run before its first receipt).

**Errors:** unknown run id exits 2 with `{"error": "unknown_run"}` on stdout; an unreadable database exits 1. A missing or corrupt totals file is not an error; `headroom` becomes `null`.

**Rejected:** aoe reading `ledger.db` directly; Ledger pushing data into aoe at launch; querying the supervisor over its control socket (more coupling than a file, for no gain).

## aoe side

### Remembering a session's runs

`aoe session report-ledger-launch` (in `src/cli/session.rs`) already receives each launch's `run_id` and `AOE_INSTANCE_ID`. It will also append a usage event:

- New `UsageKind::LedgerRun`, `detail = <run_id>`.
- It is a lifecycle kind: it does not mark a session `tracked` and does not count toward resets.
- `docs/development/usage-events.md` gains the new kind (the Ledger importer contract).

A restarted session therefore keeps the ordered list of its runs. The latest one is the current run.

### `LedgerPoller`

Copies the shape of `src/tui/usage_poller.rs`:

- Runs on a 30s tick plus selection changes.
- For the selected session, calls `ledger run show <run_id> --json` through `bounded_output` (2s timeout, 64 KiB cap) for the current run every tick, and for each earlier run once (finished results are cached per run id for the process lifetime).
- Produces a `LedgerRunView { generation: Option<…>, headroom_total: Option<HeadroomTotals>, headroom_now: Option<HeadroomTotals>, error: Option<String> }`.
- The web server exposes the same view at `GET /api/sessions/{id}/ledger-run`, built by the same Rust code so both surfaces read one source.
- If the `ledger` binary is missing, the poller stops calling it for 5 minutes after the first failure.

### Dollar estimate

`saved_tokens × input price per token` for the receipt model, from a small built-in table matched by substring on the model id (`opus`, `sonnet`, `haiku`, `gpt-5`), in `src/usage/prices.rs`. Unknown models show no dollar figure. The figure is always prefixed `~`. Prices are per-million-token input list prices, updated by hand.

### Overlay rendering

`src/tui/components/usage_overlay.rs` becomes a section stack: the resets section (unchanged), then the drift section, then the Headroom section. Each section returns its lines and the stack right-aligns them, keeping today's translucency (`BACKDROP_ALPHA`, `INK_ALPHA`) and small-screen hiding. `web/src/components/UsageOverlay.tsx` mirrors it.

Lines:

```
ledger behind d861↔f00d 3h  ↓tdd ↑grill-me +caveman -dataviz  +2 settings
hr 1.2M saved · 29% · ~$3.60  (now 26.5k)
```

- Drift header: `ledger <state> <launched4>↔<current4> <age since current_since>`.
- Skills are listed by name with `↓` older, `↑` newer, `+` extra, `-` missing. Other kinds collapse to counts (`+2 settings`). Overflow becomes `+N more`.
- Current: `ledger d861 current`.
- Error: dimmed `ledger ?` and a short reason (`no ledger`, `timeout`, `unknown run`).
- Headroom: lifetime total across all the session's runs, `saved / before` as a percentage, the dollar estimate, and `(now …)` for the current run when the session has more than one run. Counts use `k`/`M` with one decimal.
- The Headroom line is hidden when every run's `headroom` is null, and whenever the drift line is in its error state.
- Sessions without `@aoe_ledger_launch` show neither line.

### Settings

- `session.show_ledger_overlay` (default on) gates the drift line.
- `session.show_headroom_overlay` (default on) gates the Headroom line.
- Both follow `docs/development/adding-settings.md`, including the web flag gate (`sessionFlagGate`).

### Contract doc

`docs/development/ledger-run.md` documents the `ledger.run.v1` fields aoe relies on, the poll cadence and the error handling. It replaces the planned `ledger-run-generation.md`.

## Testing

**Ledger:**

- Unit: the totals accumulator (evaluated vs. other statuses, multi-lease, atomic rewrite, removal after finish, write failure is diagnostic only).
- Unit: `run show` against a fixture database for each generation `state` and `change` kind, finished and live Headroom sources, `null` cases, unknown run.
- Hermetic run: `go vet ./... && GIT_CONFIG_GLOBAL=/dev/null go test -race ./...` across the whole module.

**aoe:**

- Shared fixtures in `tests/fixtures/ledger-run/*.json` (current, behind with every change kind, diverged, overflow, live Headroom, finished Headroom, null Headroom, error) drive both the Rust and the web tests.
- Rust: parse each fixture; render each into overlay lines (exact strings); summing across runs with caching; `LedgerRun` usage events leave `tracked` false; settings gate each line.
- Web: `usage.test.ts`-style line tests against the same fixtures, and a component test that both settings hide their lines.
- Manual: in a live `lh` session, send a prompt, wait one tick, and confirm the `hr` line moves; restart the session and confirm `(now …)` appears and the total keeps the first run's savings.

## Build order

1. Ledger: totals file, then `ledger run show` (Headroom block first, then generation). One attic PR.
2. aoe: `LedgerRun` usage event and `LedgerPoller` with fixtures.
3. aoe TUI: section stack, Headroom line, drift line.
4. aoe web: API route and overlay mirror.
5. Deploy both locally (Ledger `go install` and restart the on-demand daemon; aoe per the local deploy recipe), then tune visuals live.
