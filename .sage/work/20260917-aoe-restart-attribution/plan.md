---
status: completed
phase: implementation-plan
---
# Implementation plan

1. Tests for separate Ledger pane metadata decoding, instance/pane ownership, valid bounded IDs and old report compatibility.
2. Add optional `session report-ledger-launch` command storing separate metadata, leaving report-launch unchanged.
3. Add bounded record-only Ledger helper before restart teardown, with no raw stdout/stderr logging. Use already-resolved native resume decision; place returned intent in the next launch environment only. Unavailable evidence preserves restart behavior.
4. Extend Ledger AOE report producer after its existing exact origin/account gate. Retain old callback API; parent passes prepared run and validated restart intent into new helper.
5. Run one web-feature Rust lib test/check set with one compiler job and prebuilt unchanged web assets. Run Ledger producer unit tests using local fake reporter. Record evidence, decisions and deployment caveats. Parent owns full integration and AOE deployment.
