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
other than `ledger.run.v1`, or unparseable output is `ledger error`. A run
whose Ledger supervisor has died reads with no Headroom data (`headroom:
null`).

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
