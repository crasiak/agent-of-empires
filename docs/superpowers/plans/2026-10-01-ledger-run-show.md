# Ledger `run show` and live Headroom totals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give aoe one read-only Ledger command, `ledger run show <run_id> --json`, that reports a launch run's generation drift and its Headroom savings. Savings must be visible while the run is still live.

**Architecture:** The `ledger run` supervisor already validates every Headroom receipt in a per-lease goroutine. A shared accumulator adds each one to a per-run totals file at `~/.ledger/run/<run_id>.headroom.json`, written atomically and removed after the finish is recorded. A new package, `internal/runview`, opens `ledger.db` read-only and builds the `ledger.run.v1` view:
- generation drift, diffing the launched lock's pins against the active lock's pins in Go;
- Headroom totals, from the finished run's `transport.receipt` changes or from the live file.

`cmd/ledger/runshowcmd.go` prints the view.

**Tech Stack:** Go 1.26, `modernc.org/sqlite`, standard library only.

**Spec:** `docs/superpowers/specs/2026-10-01-ledger-run-overlay-design.md` in the agent-of-empires repo (worktree `/Users/jws/code/agent-of-empires-worktrees/session-counter`). The companion aoe plan is `docs/superpowers/plans/2026-10-01-aoe-ledger-run-overlay.md` next to this file. It consumes the JSON this plan produces.

## Global Constraints

- Repo: `~/code/attic` (Go module `github.com/jws/ledger` in `ledger/`). Work in a clean worktree off `origin/main`: `~/code/attic` has other sessions' dirty files. Never use bare `git stash`.
- Commit as `jw@crasiak.net` (the attic repo's local `user.email` already is; check with `git config user.email`).
- Hermetic verification, run from `ledger/`: `go vet ./... && GIT_CONFIG_GLOBAL=/dev/null go test -race ./...`.
- Live totals file: `~/.ledger/run/<run_id>.headroom.json`, mode `0600`, written to a temp file in the same directory and then `rename`d over the target (no fsync). Schema `ledger.run-headroom-live.v1`.
- `ledger run show` output schema is `ledger.run.v1`. It never talks to ledgerd. It opens `~/.ledger/ledger.db` read-only.
- Token counts come only from receipts with `accounting_status = "evaluated"`. Every receipt adds to `requests`.
- `model` is the model of the most recent evaluated, non-metadata receipt. The spec says "the most recent receipt", but metadata receipts carry the literal model `metadata`, which would break pricing.
- `change` values: `older` / `newer` / `missing` / `extra`, from the session's point of view. `state` values: `current` / `behind` / `ahead` / `diverged`.
- Errors: an unknown run id exits 2 and prints `{"error":"unknown_run"}` on stdout. An unreadable database exits 1. A missing or corrupt totals file is not an error; `headroom` becomes `null`.
- A failed totals write is logged once as the diagnostic event `headroom_totals_write_failed` and never fails the run.
- JSON arrays `drift` and `runtime_drift` are always arrays (`[]`), never `null`. The aoe parser relies on this.
- **State rule (fills a gap in the spec):** the run's direction comes from comparing the two materializations' `created_at`: launched earlier means `behind`, otherwise `ahead`. A resource change pointing the other way (`newer` while behind, `older` while ahead) makes it `diverged`. `missing` and `extra` follow the direction. Runtime drift's `change` is `older` when behind and `newer` when ahead.
- Timestamps in `ledger.db` have variable fractional digits. Compare them by parsing with `time.RFC3339Nano`, never as strings.

---

### Task 0: Worktree

- [ ] **Step 1: Create the worktree**

```bash
cd ~/code/attic && git fetch origin && git worktree add -b feature/ledger-run-show ~/code/attic-worktrees/ledger-run-show origin/main
cd ~/code/attic-worktrees/ledger-run-show/ledger && go build ./... && git config user.email
```

Expected: the build succeeds and the email prints `jw@crasiak.net`. If it doesn't, run `git config user.email jw@crasiak.net` in the worktree.

---

### Task 1: `internal/runview` totals, live-file reader and receipt rule

**Files:**
- Create: `ledger/internal/runview/headroom.go`
- Test: `ledger/internal/runview/headroom_test.go`

**Interfaces:**
- Produces:
  - `const LiveSchema = "ledger.run-headroom-live.v1"`
  - `type Receipt struct { RequestKind, Model, AccountingStatus string; InputTokensBefore, InputTokensAfter, SavedTokens int }` (JSON tags match receipt payload keys)
  - `type Totals struct { Requests, InputTokensBefore, InputTokensAfter, SavedTokens int; Model, UpdatedAt string }`
  - `func (t *Totals) Add(r Receipt, at string)`
  - `type Live struct { Schema string; RunID string; Totals }` (embedded, so JSON is flat)
  - `func LivePath(home, runID string) string`
  - `func ReadLive(home, runID string) *Live`

- [ ] **Step 1: Write the failing test**

```go
package runview

import (
	"os"
	"path/filepath"
	"testing"
)

func TestTotalsCountOnlyEvaluatedTokens(t *testing.T) {
	var totals Totals
	totals.Add(Receipt{Model: "claude-opus-5", AccountingStatus: "evaluated",
		InputTokensBefore: 100, InputTokensAfter: 70, SavedTokens: 30}, "2026-10-01T10:00:00Z")
	totals.Add(Receipt{Model: "unparsed", AccountingStatus: "rejected",
		InputTokensBefore: 50, InputTokensAfter: 50}, "2026-10-01T10:01:00Z")
	totals.Add(Receipt{RequestKind: "metadata", Model: "metadata", AccountingStatus: "evaluated"},
		"2026-10-01T10:02:00Z")
	want := Totals{Requests: 3, InputTokensBefore: 100, InputTokensAfter: 70, SavedTokens: 30,
		Model: "claude-opus-5", UpdatedAt: "2026-10-01T10:02:00Z"}
	if totals != want {
		t.Fatalf("totals = %+v, want %+v", totals, want)
	}
}

func TestReadLiveRequiresSchemaAndRun(t *testing.T) {
	home := t.TempDir()
	path := LivePath(home, "run_a")
	if want := filepath.Join(home, "run", "run_a.headroom.json"); path != want {
		t.Fatalf("LivePath = %q, want %q", path, want)
	}
	if ReadLive(home, "run_a") != nil {
		t.Fatal("missing file must read as nil")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	write := func(body string) {
		t.Helper()
		if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write(`{"schema":"ledger.run-headroom-live.v1","run_id":"run_a","requests":2,"input_tokens_before":10,"input_tokens_after":4,"saved_tokens":6,"model":"m","updated_at":"t"}`)
	live := ReadLive(home, "run_a")
	if live == nil || live.Requests != 2 || live.SavedTokens != 6 || live.Model != "m" {
		t.Fatalf("live = %+v", live)
	}
	write(`{"schema":"ledger.run-headroom-live.v1","run_id":"run_b","requests":2}`)
	if ReadLive(home, "run_a") != nil {
		t.Fatal("a file for another run must read as nil")
	}
	write(`{"schema":"other","run_id":"run_a"}`)
	if ReadLive(home, "run_a") != nil {
		t.Fatal("an unknown schema must read as nil")
	}
	write(`{not json`)
	if ReadLive(home, "run_a") != nil {
		t.Fatal("a corrupt file must read as nil")
	}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ledger && go test ./internal/runview/ -run 'TestTotals|TestReadLive' -v`
Expected: FAIL to build (`undefined: Totals`).

- [ ] **Step 3: Write minimal implementation**

```go
// Package runview answers `ledger run show`: one launch run's generation
// drift and Headroom savings, read from ledger.db and the run's live totals
// file without ledgerd.
package runview

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// LiveSchema is the per-run totals file the `ledger run` supervisor rewrites
// after every Headroom receipt.
const LiveSchema = "ledger.run-headroom-live.v1"

// Receipt is the part of a Headroom receipt the totals read.
type Receipt struct {
	RequestKind       string `json:"request_kind"`
	Model             string `json:"model"`
	AccountingStatus  string `json:"accounting_status"`
	InputTokensBefore int    `json:"input_tokens_before"`
	InputTokensAfter  int    `json:"input_tokens_after"`
	SavedTokens       int    `json:"saved_tokens"`
}

// Totals sums a run's receipts. Every receipt is a request; only evaluated
// ones carry token accounting, and only inference ones name the model.
type Totals struct {
	Requests          int    `json:"requests"`
	InputTokensBefore int    `json:"input_tokens_before"`
	InputTokensAfter  int    `json:"input_tokens_after"`
	SavedTokens       int    `json:"saved_tokens"`
	Model             string `json:"model"`
	UpdatedAt         string `json:"updated_at"`
}

func (t *Totals) Add(r Receipt, at string) {
	t.Requests++
	t.UpdatedAt = at
	if r.AccountingStatus != "evaluated" {
		return
	}
	t.InputTokensBefore += r.InputTokensBefore
	t.InputTokensAfter += r.InputTokensAfter
	t.SavedTokens += r.SavedTokens
	if r.RequestKind != "metadata" && r.Model != "" {
		t.Model = r.Model
	}
}

type Live struct {
	Schema string `json:"schema"`
	RunID  string `json:"run_id"`
	Totals
}

// LivePath sits next to the run's transport control socket.
func LivePath(home, runID string) string {
	return filepath.Join(home, "run", runID+".headroom.json")
}

// ReadLive returns nil for a missing, corrupt or foreign file: live totals
// are best effort and never an error.
func ReadLive(home, runID string) *Live {
	data, err := os.ReadFile(LivePath(home, runID))
	if err != nil {
		return nil
	}
	var live Live
	if json.Unmarshal(data, &live) != nil || live.Schema != LiveSchema || live.RunID != runID {
		return nil
	}
	return &live
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ledger && go test ./internal/runview/ -run 'TestTotals|TestReadLive' -v`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ledger/internal/runview
git commit -m "feat(runview): Headroom totals and the live totals file reader"
```

---

### Task 2: Supervisor writes the live totals file

**Files:**
- Create: `ledger/cmd/ledger/headroom_totals.go`
- Create: `ledger/cmd/ledger/headroom_totals_test.go`
- Modify: `ledger/cmd/ledger/headroom_transport.go`: `readHeadroomReceipts` (~line 539) gains an observer; the spawner gets the accumulator (~line 98).
- Modify: `ledger/cmd/ledger/headroom_swap.go`: `headroomLease` and `headroomLeaseSpawner` get a `totals` field; `spawn` copies it; `collectReceipts` (~line 208) passes `l.totals.add`.
- Modify: `ledger/cmd/ledger/runcmd.go`: `executePreparedRunUsing` removes the file after the finish is recorded or spooled.
- Test: `ledger/cmd/ledger/headroom_transport_test.go` (two new tests).

**Interfaces:**
- Consumes: `runview.Totals`, `runview.Receipt`, `runview.Live`, `runview.LiveSchema`, `runview.LivePath` (Task 1).
- Produces:
  - `func newHeadroomTotals(home, runID string, failed func(error)) *headroomTotals`
  - `func (t *headroomTotals) add(receipt launch.HeadroomReceipt)` (nil-safe)
  - `func removeHeadroomTotals(home, runID string)`
  - `func readHeadroomReceipts(reader io.Reader, acquire launch.HeadroomAcquire, observe func(launch.HeadroomReceipt)) ([]launch.HeadroomReceipt, error)`

- [ ] **Step 1: Write the failing accumulator tests**

`ledger/cmd/ledger/headroom_totals_test.go` (no build tag: the accumulator is plain Go):

```go
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/jws/ledger/internal/launch"
	"github.com/jws/ledger/internal/runview"
)

func TestHeadroomTotalsRewriteAtomicallyAcrossReceipts(t *testing.T) {
	home := t.TempDir()
	old := headroomTotalsNow
	headroomTotalsNow = func() time.Time { return time.Date(2026, 10, 1, 12, 4, 19, 0, time.UTC) }
	t.Cleanup(func() { headroomTotalsNow = old })
	totals := newHeadroomTotals(home, "run_a", func(err error) { t.Fatalf("unexpected write failure: %v", err) })
	totals.add(launch.HeadroomReceipt{Model: "claude-opus-5", AccountingStatus: "evaluated",
		InputTokensBefore: 1000, InputTokensAfter: 700, SavedTokens: 300})
	totals.add(launch.HeadroomReceipt{Model: "claude-opus-5", AccountingStatus: "rejected",
		InputTokensBefore: 10, InputTokensAfter: 10})
	path := runview.LivePath(home, "run_a")
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("mode = %v, want 0600", info.Mode().Perm())
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var live runview.Live
	if err := json.Unmarshal(data, &live); err != nil {
		t.Fatal(err)
	}
	want := runview.Live{Schema: runview.LiveSchema, RunID: "run_a", Totals: runview.Totals{
		Requests: 2, InputTokensBefore: 1000, InputTokensAfter: 700, SavedTokens: 300,
		Model: "claude-opus-5", UpdatedAt: "2026-10-01T12:04:19Z"}}
	if live != want {
		t.Fatalf("live = %+v, want %+v", live, want)
	}
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 {
		t.Fatalf("temp files left behind: %v", entries)
	}
	removeHeadroomTotals(home, "run_a")
	removeHeadroomTotals(home, "run_a") // idempotent
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("totals file survived removal: %v", err)
	}
}

func TestHeadroomTotalsReportWriteFailureOnce(t *testing.T) {
	home := t.TempDir()
	// A file where the run directory should be makes every write fail.
	if err := os.WriteFile(filepath.Join(home, "run"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	failures := 0
	totals := newHeadroomTotals(home, "run_a", func(error) { failures++ })
	receipt := launch.HeadroomReceipt{AccountingStatus: "evaluated", Model: "m"}
	totals.add(receipt)
	totals.add(receipt)
	if failures != 1 {
		t.Fatalf("failures = %d, want 1", failures)
	}
	var none *headroomTotals
	none.add(receipt) // a nil accumulator is a no-op
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd ledger && go test ./cmd/ledger/ -run TestHeadroomTotals -v`
Expected: FAIL to build (`undefined: newHeadroomTotals`).

- [ ] **Step 3: Implement the accumulator**

`ledger/cmd/ledger/headroom_totals.go`:

```go
package main

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/jws/ledger/internal/launch"
	"github.com/jws/ledger/internal/runview"
)

var headroomTotalsNow = time.Now

// headroomTotals publishes a run's running Headroom savings for `ledger run
// show` while the native agent is still up. Every lease of the run (planned
// swaps) adds to the one accumulator.
type headroomTotals struct {
	mu     sync.Mutex
	path   string
	live   runview.Live
	failed func(error)
	once   sync.Once
}

func newHeadroomTotals(home, runID string, failed func(error)) *headroomTotals {
	return &headroomTotals{
		path:   runview.LivePath(home, runID),
		live:   runview.Live{Schema: runview.LiveSchema, RunID: runID},
		failed: failed,
	}
}

func (t *headroomTotals) add(receipt launch.HeadroomReceipt) {
	if t == nil {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.live.Add(runview.Receipt{
		RequestKind: receipt.RequestKind, Model: receipt.Model, AccountingStatus: receipt.AccountingStatus,
		InputTokensBefore: receipt.InputTokensBefore, InputTokensAfter: receipt.InputTokensAfter,
		SavedTokens: receipt.SavedTokens,
	}, headroomTotalsNow().UTC().Format(time.RFC3339Nano))
	if err := writeHeadroomTotals(t.path, t.live); err != nil && t.failed != nil {
		t.once.Do(func() { t.failed(err) })
	}
}

// writeHeadroomTotals replaces the file by rename so a reader never sees a
// partial write. No fsync: the file is a live view, not evidence.
func writeHeadroomTotals(path string, live runview.Live) error {
	data, err := json.Marshal(live)
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(dir, ".headroom-*.json")
	if err != nil {
		return err
	}
	_, writeErr := file.Write(append(data, '\n'))
	if err := errors.Join(writeErr, file.Close()); err != nil {
		_ = os.Remove(file.Name())
		return err
	}
	if err := os.Rename(file.Name(), path); err != nil {
		_ = os.Remove(file.Name())
		return err
	}
	return nil
}

func removeHeadroomTotals(home, runID string) {
	_ = os.Remove(runview.LivePath(home, runID))
}
```

- [ ] **Step 4: Run accumulator tests**

Run: `cd ledger && go test ./cmd/ledger/ -run TestHeadroomTotals -v`
Expected: PASS.

- [ ] **Step 5: Write the failing wiring tests**

Append to `ledger/cmd/ledger/headroom_transport_test.go`, which is unix-tagged and already has `preparedHeadroomProcessFixture`. Add `"github.com/jws/ledger/internal/paths"` and `"github.com/jws/ledger/internal/runview"` to its imports.

```go
func TestHeadroomTransportPublishesLiveTotals(t *testing.T) {
	prepared := preparedHeadroomProcessFixture(t, "success")
	result := executeHeadroomTransportProcess(prepared,
		[]string{"PATH=/usr/bin:/bin", "ANTHROPIC_AUTH_TOKEN=fake-oauth"}, prepared.CWD)
	if result.Err != nil {
		t.Fatal(result.Err)
	}
	live := runview.ReadLive(paths.LedgerHome(), prepared.LaunchRunID)
	if live == nil || live.Requests != len(result.Receipts) {
		t.Fatalf("live totals = %+v, receipts = %d", live, len(result.Receipts))
	}
}

func TestPreparedRunRemovesLiveTotalsAfterFinish(t *testing.T) {
	prepared := preparedHeadroomProcessFixture(t, "success")
	oldTransport, oldFinish := executeHeadroomTransport, postFinishRun
	t.Cleanup(func() { executeHeadroomTransport, postFinishRun = oldTransport, oldFinish })
	home := paths.LedgerHome()
	executeHeadroomTransport = func(launch.PreparedRun, []string, string, *launchJournal) headroomExecutionResult {
		newHeadroomTotals(home, prepared.LaunchRunID, nil).add(launch.HeadroomReceipt{AccountingStatus: "evaluated"})
		zero := 0
		return headroomExecutionResult{childResult: childResult{ExitCode: &zero}}
	}
	postFinishRun = func(string, launch.FinishRequest) error { return nil }
	_ = executePreparedRunUsing(prepared, adapter.HarnessClaude, prepared.CWD,
		func(string, []string, []string, string, func(int)) childResult { return childResult{} })
	if runview.ReadLive(home, prepared.LaunchRunID) != nil {
		t.Fatal("live totals survived a recorded finish")
	}
}
```

The second test ignores `executePreparedRunUsing`'s error on purpose. With no receipts the run reports failure, but the finish is still recorded, and recording the finish is what removes the file.

- [ ] **Step 6: Run to verify they fail**

Run: `cd ledger && go test ./cmd/ledger/ -run 'TestHeadroomTransportPublishesLiveTotals|TestPreparedRunRemovesLiveTotalsAfterFinish' -v`
Expected: FAIL. The first test finds no live file; the second finds that the file survived.

- [ ] **Step 7: Wire the accumulator**

In `headroom_transport.go`, change `readHeadroomReceipts` to take an observer:

```go
// observe, when set, sees each receipt once it has been validated.
func readHeadroomReceipts(reader io.Reader, acquire launch.HeadroomAcquire, observe func(launch.HeadroomReceipt)) ([]launch.HeadroomReceipt, error) {
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 4096), 64*1024)
	receipts := make([]launch.HeadroomReceipt, 0)
	for scanner.Scan() {
		receipt, err := launch.DecodeHeadroomReceipt(strings.NewReader(scanner.Text()))
		if err != nil {
			return nil, err
		}
		if err := launch.ValidateHeadroomReceipt(acquire, receipt, len(receipts)+1); err != nil {
			return nil, err
		}
		receipts = append(receipts, receipt)
		if observe != nil {
			observe(receipt)
		}
	}
	if err := scanner.Err(); err != nil {
		return nil, fmt.Errorf("read Headroom receipts: %w", err)
	}
	return receipts, nil
}
```

Still in `headroom_transport.go`, where the spawner is built (`spawner := &headroomLeaseSpawner{...}`), create the accumulator and pass it in. Add the `paths` import.

```go
	totals := newHeadroomTotals(paths.LedgerHome(), prepared.LaunchRunID, func(err error) {
		life.record(diagnostics.Event{Kind: "headroom_totals_write_failed", Phase: "running", Role: "supervisor", Errno: signalError(err)})
	})
	spawner := &headroomLeaseSpawner{prepared: prepared, inherited: inheritedEnvironment, cwd: cwd,
		runtimeRoot: runtimeDir, proxyLog: proxyLog, life: life, totals: totals}
```

In `headroom_swap.go`:
- add `totals *headroomTotals` as the last field of both `headroomLease` and `headroomLeaseSpawner`;
- in `spawn`, set `totals: s.totals` in the `&headroomLease{...}` literal;
- change `collectReceipts`:

```go
func (l *headroomLease) collectReceipts() {
	go func() {
		receipts, err := readHeadroomReceipts(l.receipts, l.transport.Acquire, l.totals.add)
		l.collected <- leaseReceipts{receipts, err}
	}()
}
```

(`l.totals.add` is a method value on a possibly-nil pointer. `add` is nil-safe.)

In `runcmd.go`, `executePreparedRunUsing`: right after `saved, finishErr := recordFinish(...)`, add the code below, and add the `paths` import if it's missing (it is already imported).

```go
	if finishErr == nil {
		// ledgerd has the receipts now (or will, from the spool).
		removeHeadroomTotals(paths.LedgerHome(), prepared.LaunchRunID)
	}
```

- [ ] **Step 8: Run the package tests**

Run: `cd ledger && go vet ./cmd/ledger/ && GIT_CONFIG_GLOBAL=/dev/null go test -race ./cmd/ledger/`
Expected: PASS, including every existing Headroom test.

- [ ] **Step 9: Commit**

```bash
git add ledger/cmd/ledger/headroom_totals.go ledger/cmd/ledger/headroom_totals_test.go ledger/cmd/ledger/headroom_transport.go ledger/cmd/ledger/headroom_transport_test.go ledger/cmd/ledger/headroom_swap.go ledger/cmd/ledger/runcmd.go
git commit -m "feat(run): publish live Headroom totals per run"
```

---

### Task 3: `runview.Show`: run row and Headroom block

**Files:**
- Create: `ledger/internal/runview/show.go`
- Create: `ledger/internal/runview/fixture_test.go` (shared test schema and seed helpers)
- Test: `ledger/internal/runview/show_test.go`

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  - `const Schema = "ledger.run.v1"`
  - `var ErrUnknownRun = errors.New("unknown run")`
  - `type View struct { Schema, RunID, Harness, Profile, StartedAt string; FinishedAt *string; Generation *Generation; Headroom *Headroom }` (JSON keys `schema`, `run_id`, `harness`, `profile`, `started_at`, `finished_at`, `generation`, `headroom`)
  - `type Headroom struct { Source string; Totals }` (`source` is `live` or `finished`)
  - `func Open(home string) (*sql.DB, error)`
  - `func Show(ctx context.Context, d *sql.DB, home, runID string) (View, error)`
  - Placeholder in this task: `type Generation struct{}` and `func generation(...) (*Generation, error) { return nil, nil }`. Task 4 replaces both.

- [ ] **Step 1: Write the fixture helpers**

`ledger/internal/runview/fixture_test.go`:

```go
package runview

import (
	"database/sql"
	"path/filepath"
	"testing"
)

// fixtureSchema holds just the columns runview reads. The real schema's
// coherence triggers make full rows expensive to build;
// TestQueriesMatchMigratedSchema checks the queries against the real one.
const fixtureSchema = `
CREATE TABLE profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL);
CREATE TABLE launch_runs (id TEXT PRIMARY KEY, profile_id TEXT, harness TEXT, lock_hash TEXT, materialization_id TEXT, started_at TEXT);
CREATE TABLE launch_run_finishes (launch_run_id TEXT PRIMARY KEY, finished_at TEXT);
CREATE TABLE materializations (id TEXT PRIMARY KEY, lock_hash TEXT, renderer_version TEXT, path_policy_version TEXT, adapter_api_major INTEGER, created_at TEXT);
CREATE TABLE materialization_activations (seq INTEGER PRIMARY KEY, activated_at TEXT);
CREATE TABLE active_materializations (profile_id TEXT, harness TEXT, materialization_id TEXT, activation_seq INTEGER);
CREATE TABLE resources (id TEXT PRIMARY KEY, kind TEXT, logical_name TEXT);
CREATE TABLE resource_versions (hash TEXT PRIMARY KEY, created_at TEXT);
CREATE TABLE profile_lock_resources (lock_hash TEXT, resource_id TEXT, version_hash TEXT);
CREATE TABLE changes (seq INTEGER PRIMARY KEY AUTOINCREMENT, aggregate_type TEXT, aggregate_id TEXT, event_type TEXT, payload_json TEXT, occurred_at TEXT);
`

// fixtureHome returns a Ledger home with a fixture ledger.db and a writer.
func fixtureHome(t *testing.T) (string, *sql.DB) {
	t.Helper()
	home := t.TempDir()
	d, err := sql.Open("sqlite", "file:"+filepath.ToSlash(filepath.Join(home, "ledger.db")))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { d.Close() })
	if _, err := d.Exec(fixtureSchema); err != nil {
		t.Fatal(err)
	}
	return home, d
}

func mustExec(t *testing.T, d *sql.DB, query string, args ...any) {
	t.Helper()
	if _, err := d.Exec(query, args...); err != nil {
		t.Fatalf("%s: %v", query, err)
	}
}

// seedRun inserts a run of profile "personal" on materialization mat.
func seedRun(t *testing.T, d *sql.DB, runID, mat string) {
	t.Helper()
	mustExec(t, d, `INSERT OR IGNORE INTO profiles VALUES ('pro_1', 'personal')`)
	mustExec(t, d, `INSERT INTO launch_runs VALUES (?, 'pro_1', 'claude-code', 'lock', ?, '2026-10-01T09:00:00Z')`, runID, mat)
}

func seedReceipt(t *testing.T, d *sql.DB, runID, payload, at string) {
	t.Helper()
	mustExec(t, d, `INSERT INTO changes (aggregate_type, aggregate_id, event_type, payload_json, occurred_at)
		VALUES ('launch-run', ?, 'transport.receipt', ?, ?)`, runID, payload, at)
}
```

- [ ] **Step 2: Write the failing tests**

`ledger/internal/runview/show_test.go`:

```go
package runview

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/jws/ledger/internal/db"
)

func show(t *testing.T, home, runID string) (View, error) {
	t.Helper()
	d, err := Open(home)
	if err != nil {
		t.Fatal(err)
	}
	defer d.Close()
	return Show(context.Background(), d, home, runID)
}

func writeLive(t *testing.T, home, runID, body string) {
	t.Helper()
	path := LivePath(home, runID)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestShowUnknownRun(t *testing.T) {
	home, _ := fixtureHome(t)
	if _, err := show(t, home, "run_missing"); !errors.Is(err, ErrUnknownRun) {
		t.Fatalf("err = %v, want ErrUnknownRun", err)
	}
}

func TestShowFinishedRunSumsItsReceipts(t *testing.T) {
	home, d := fixtureHome(t)
	seedRun(t, d, "run_a", "mat_1")
	mustExec(t, d, `INSERT INTO launch_run_finishes VALUES ('run_a', '2026-10-01T11:00:00Z')`)
	seedReceipt(t, d, "run_a", `{"model":"claude-opus-5","accounting_status":"evaluated","input_tokens_before":100,"input_tokens_after":70,"saved_tokens":30}`, "2026-10-01T10:00:00Z")
	seedReceipt(t, d, "run_a", `{"model":"unparsed","accounting_status":"rejected","input_tokens_before":50,"input_tokens_after":50,"saved_tokens":0}`, "2026-10-01T10:01:00Z")
	seedReceipt(t, d, "run_a", `{"request_kind":"metadata","model":"metadata","accounting_status":"evaluated"}`, "2026-10-01T10:02:00Z")
	seedReceipt(t, d, "run_other", `{"model":"x","accounting_status":"evaluated","input_tokens_before":9,"input_tokens_after":0,"saved_tokens":9}`, "2026-10-01T10:03:00Z")
	// A stale live file loses to the finish.
	writeLive(t, home, "run_a", `{"schema":"ledger.run-headroom-live.v1","run_id":"run_a","requests":99}`)
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Schema != Schema || view.RunID != "run_a" || view.Harness != "claude-code" ||
		view.Profile != "personal" || view.StartedAt != "2026-10-01T09:00:00Z" ||
		view.FinishedAt == nil || *view.FinishedAt != "2026-10-01T11:00:00Z" {
		t.Fatalf("view = %+v", view)
	}
	want := Headroom{Source: "finished", Totals: Totals{Requests: 3, InputTokensBefore: 100,
		InputTokensAfter: 70, SavedTokens: 30, Model: "claude-opus-5", UpdatedAt: "2026-10-01T10:02:00Z"}}
	if view.Headroom == nil || *view.Headroom != want {
		t.Fatalf("headroom = %+v, want %+v", view.Headroom, want)
	}
}

func TestShowFinishedRunWithoutReceiptsHasNoHeadroom(t *testing.T) {
	home, d := fixtureHome(t)
	seedRun(t, d, "run_a", "mat_1")
	mustExec(t, d, `INSERT INTO launch_run_finishes VALUES ('run_a', '2026-10-01T11:00:00Z')`)
	writeLive(t, home, "run_a", `{"schema":"ledger.run-headroom-live.v1","run_id":"run_a","requests":99}`)
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Headroom != nil {
		t.Fatalf("headroom = %+v, want nil", view.Headroom)
	}
}

func TestShowLiveRunReadsItsTotalsFile(t *testing.T) {
	home, d := fixtureHome(t)
	seedRun(t, d, "run_a", "mat_1")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Headroom != nil || view.FinishedAt != nil {
		t.Fatalf("view before any receipt = %+v", view)
	}
	writeLive(t, home, "run_a", `{"schema":"ledger.run-headroom-live.v1","run_id":"run_a","requests":42,"input_tokens_before":3912000,"input_tokens_after":2780000,"saved_tokens":1132000,"model":"global.anthropic.claude-opus-5","updated_at":"2026-10-01T12:04:19.752Z"}`)
	view, err = show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Headroom == nil || view.Headroom.Source != "live" || view.Headroom.SavedTokens != 1132000 || view.Headroom.Requests != 42 {
		t.Fatalf("headroom = %+v", view.Headroom)
	}
}

func TestOpenRefusesAMissingDatabase(t *testing.T) {
	if _, err := Open(t.TempDir()); err == nil {
		t.Fatal("Open succeeded without ledger.db")
	}
}

// The fixture schema is a subset; every query must also run against the
// real migrated schema.
func TestQueriesMatchMigratedSchema(t *testing.T) {
	home := t.TempDir()
	migrated, err := db.Open(filepath.Join(home, "ledger.db"))
	if err != nil {
		t.Fatal(err)
	}
	migrated.Close()
	d, err := Open(home)
	if err != nil {
		t.Fatal(err)
	}
	defer d.Close()
	ctx := context.Background()
	if _, err := Show(ctx, d, home, "run_missing"); !errors.Is(err, ErrUnknownRun) {
		t.Fatalf("Show: %v", err)
	}
	if _, err := finishedHeadroom(ctx, d, "run_missing"); err != nil {
		t.Fatalf("finishedHeadroom: %v", err)
	}
	for name, check := range migratedSchemaChecks(ctx, d) {
		if err := check(); err != nil && strings.Contains(err.Error(), "no such") {
			t.Fatalf("%s: %v", name, err)
		}
	}
}
```

`migratedSchemaChecks` is defined in Task 4. For this task, add a stub to `show_test.go` and delete it in Task 4:

```go
func migratedSchemaChecks(context.Context, *sql.DB) map[string]func() error { return nil }
```

(add `"database/sql"` to the imports).

- [ ] **Step 3: Run to verify they fail**

Run: `cd ledger && go test ./internal/runview/ -v`
Expected: FAIL to build (`undefined: Open`).

- [ ] **Step 4: Implement**

`ledger/internal/runview/show.go`:

```go
package runview

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	_ "modernc.org/sqlite"
)

const Schema = "ledger.run.v1"

var ErrUnknownRun = errors.New("unknown run")

type View struct {
	Schema     string      `json:"schema"`
	RunID      string      `json:"run_id"`
	Harness    string      `json:"harness"`
	Profile    string      `json:"profile"`
	StartedAt  string      `json:"started_at"`
	FinishedAt *string     `json:"finished_at"`
	Generation *Generation `json:"generation"`
	Headroom   *Headroom   `json:"headroom"`
}

// Headroom is a run's savings: from the receipts ledgerd stored at finish, or
// from the supervisor's live totals file while the run is up.
type Headroom struct {
	Source string `json:"source"` // live | finished
	Totals
}

// Open opens ledger.db read-only; it never creates or migrates it.
func Open(home string) (*sql.DB, error) {
	path := filepath.Join(home, "ledger.db")
	if _, err := os.Stat(path); err != nil {
		return nil, err
	}
	d, err := sql.Open("sqlite", "file:"+filepath.ToSlash(path)+"?mode=ro&_pragma=busy_timeout(2000)")
	if err != nil {
		return nil, err
	}
	if err := d.Ping(); err != nil {
		d.Close()
		return nil, err
	}
	return d, nil
}

func Show(ctx context.Context, d *sql.DB, home, runID string) (View, error) {
	view := View{Schema: Schema, RunID: runID}
	var profileID, materializationID string
	var finished sql.NullString
	err := d.QueryRowContext(ctx, `SELECT r.harness, p.name, r.started_at, r.profile_id, r.materialization_id, f.finished_at
		FROM launch_runs r
		JOIN profiles p ON p.id = r.profile_id
		LEFT JOIN launch_run_finishes f ON f.launch_run_id = r.id
		WHERE r.id = ?`, runID).
		Scan(&view.Harness, &view.Profile, &view.StartedAt, &profileID, &materializationID, &finished)
	if errors.Is(err, sql.ErrNoRows) {
		return View{}, ErrUnknownRun
	}
	if err != nil {
		return View{}, fmt.Errorf("read launch run: %w", err)
	}
	if finished.Valid {
		view.FinishedAt = &finished.String
	}
	if view.Generation, err = generation(ctx, d, profileID, view.Harness, materializationID); err != nil {
		return View{}, err
	}
	if finished.Valid {
		if view.Headroom, err = finishedHeadroom(ctx, d, runID); err != nil {
			return View{}, err
		}
	} else if live := ReadLive(home, runID); live != nil {
		view.Headroom = &Headroom{Source: "live", Totals: live.Totals}
	}
	return view, nil
}

func finishedHeadroom(ctx context.Context, d *sql.DB, runID string) (*Headroom, error) {
	rows, err := d.QueryContext(ctx, `SELECT payload_json, occurred_at FROM changes
		WHERE aggregate_type = 'launch-run' AND aggregate_id = ? AND event_type = 'transport.receipt'
		ORDER BY seq`, runID)
	if err != nil {
		return nil, fmt.Errorf("read Headroom receipts: %w", err)
	}
	defer rows.Close()
	var totals Totals
	for rows.Next() {
		var payload, occurred string
		if err := rows.Scan(&payload, &occurred); err != nil {
			return nil, err
		}
		var receipt Receipt
		if json.Unmarshal([]byte(payload), &receipt) != nil {
			continue
		}
		totals.Add(receipt, occurred)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	if totals.Requests == 0 {
		return nil, nil
	}
	return &Headroom{Source: "finished", Totals: totals}, nil
}
```

Temporary placeholder, `ledger/internal/runview/generation.go` (Task 4 replaces it):

```go
package runview

import (
	"context"
	"database/sql"
)

type Generation struct{}

func generation(context.Context, *sql.DB, string, string, string) (*Generation, error) {
	return nil, nil
}
```

- [ ] **Step 5: Run tests**

Run: `cd ledger && go vet ./internal/runview/ && go test -race ./internal/runview/ -v`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ledger/internal/runview
git commit -m "feat(runview): run row and Headroom savings for ledger run show"
```

---

### Task 4: `runview` generation drift

**Files:**
- Modify (replace): `ledger/internal/runview/generation.go`
- Create: `ledger/internal/runview/generation_test.go`
- Modify: `ledger/internal/runview/show_test.go`: delete the `migratedSchemaChecks` stub (the real one goes in `generation_test.go`).

**Interfaces:**
- Consumes: `Show` calls `generation(ctx, d, profileID, harness, materializationID)` (Task 3).
- Produces:
  - `type Generation struct { State, Launched, Current, CurrentSince string; Drift []Change; RuntimeDrift []RuntimeChange }` (JSON keys `state`, `launched`, `current`, `current_since`, `drift`, `runtime_drift`)
  - `type Change struct { Kind, Name, Change string }` (`kind`, `name`, `change`)
  - `type RuntimeChange struct { Kind, Change string }` (`kind` is one of `renderer`, `path_policy`, `adapter`)

- [ ] **Step 1: Write the failing tests**

`ledger/internal/runview/generation_test.go`:

```go
package runview

import (
	"context"
	"database/sql"
	"encoding/json"
	"reflect"
	"strings"
	"testing"
)

// seedMaterialization inserts a materialization pinning the given
// resource versions ("res_id@version").
func seedMaterialization(t *testing.T, d *sql.DB, id, renderer, createdAt string, pins ...string) {
	t.Helper()
	lock := "lock_" + id
	mustExec(t, d, `INSERT INTO materializations VALUES (?, ?, ?, 'paths-v1', 1, ?)`, id, lock, renderer, createdAt)
	for _, pin := range pins {
		resource, version, _ := strings.Cut(pin, "@")
		mustExec(t, d, `INSERT INTO profile_lock_resources VALUES (?, ?, ?)`, lock, resource, version)
	}
}

func seedResource(t *testing.T, d *sql.DB, id, kind, name string, versions map[string]string) {
	t.Helper()
	mustExec(t, d, `INSERT INTO resources VALUES (?, ?, ?)`, id, kind, name)
	for hash, createdAt := range versions {
		mustExec(t, d, `INSERT INTO resource_versions VALUES (?, ?)`, hash, createdAt)
	}
}

func activate(t *testing.T, d *sql.DB, mat, at string) {
	t.Helper()
	mustExec(t, d, `INSERT INTO materialization_activations (seq, activated_at) VALUES (1, ?)`, at)
	mustExec(t, d, `INSERT INTO active_materializations VALUES ('pro_1', 'claude-code', ?, 1)`, mat)
}

// seedCatalog: tdd v1 (old) / v2 (new), grill-me g1 (old) / g2 (new),
// caveman c1, dataviz d1, settings s1. Note the variable fraction digits:
// they must be compared as times, not strings.
func seedCatalog(t *testing.T, d *sql.DB) {
	seedResource(t, d, "res_tdd", "skill", "tdd", map[string]string{"tdd1": "2026-09-01T00:00:00.2Z", "tdd2": "2026-09-01T00:00:00.20697Z"})
	seedResource(t, d, "res_grill", "skill", "grill-me", map[string]string{"g1": "2026-09-10T00:00:00Z", "g2": "2026-09-25T00:00:00Z"})
	seedResource(t, d, "res_cave", "skill", "caveman", map[string]string{"c1": "2026-09-02T00:00:00Z"})
	seedResource(t, d, "res_viz", "skill", "dataviz", map[string]string{"d1": "2026-09-03T00:00:00Z"})
	seedResource(t, d, "res_set", "settings", "claude-settings", map[string]string{"s1": "2026-09-04T00:00:00Z"})
}

func TestGenerationCurrent(t *testing.T) {
	home, d := fixtureHome(t)
	seedCatalog(t, d)
	seedMaterialization(t, d, "mat_now", "r1", "2026-10-01T08:00:00Z", "res_tdd@tdd2")
	activate(t, d, "mat_now", "2026-10-01T08:00:01Z")
	seedRun(t, d, "run_a", "mat_now")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	want := &Generation{State: "current", Launched: "mat_now", Current: "mat_now",
		CurrentSince: "2026-10-01T08:00:01Z", Drift: []Change{}, RuntimeDrift: []RuntimeChange{}}
	if !reflect.DeepEqual(view.Generation, want) {
		t.Fatalf("generation = %+v, want %+v", view.Generation, want)
	}
	encoded, _ := json.Marshal(view.Generation)
	if !strings.Contains(string(encoded), `"drift":[]`) || !strings.Contains(string(encoded), `"runtime_drift":[]`) {
		t.Fatalf("empty drift must encode as [], got %s", encoded)
	}
}

func TestGenerationBehindReportsEveryChangeKind(t *testing.T) {
	home, d := fixtureHome(t)
	seedCatalog(t, d)
	seedMaterialization(t, d, "mat_old", "r1", "2026-09-30T00:00:00Z",
		"res_tdd@tdd1", "res_cave@c1", "res_set@s1")
	seedMaterialization(t, d, "mat_new", "r2", "2026-10-01T00:00:00Z",
		"res_tdd@tdd2", "res_viz@d1", "res_set@s1")
	activate(t, d, "mat_new", "2026-10-01T09:00:00Z")
	seedRun(t, d, "run_a", "mat_old")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	want := &Generation{State: "behind", Launched: "mat_old", Current: "mat_new",
		CurrentSince: "2026-10-01T09:00:00Z",
		Drift: []Change{
			{Kind: "skill", Name: "caveman", Change: "extra"},
			{Kind: "skill", Name: "dataviz", Change: "missing"},
			{Kind: "skill", Name: "tdd", Change: "older"},
		},
		RuntimeDrift: []RuntimeChange{{Kind: "renderer", Change: "older"}},
	}
	if !reflect.DeepEqual(view.Generation, want) {
		t.Fatalf("generation = %+v, want %+v", view.Generation, want)
	}
}

func TestGenerationDivergedWhenAPinMovesAgainstTheRun(t *testing.T) {
	home, d := fixtureHome(t)
	seedCatalog(t, d)
	seedMaterialization(t, d, "mat_old", "r1", "2026-09-30T00:00:00Z", "res_tdd@tdd1", "res_grill@g2")
	seedMaterialization(t, d, "mat_new", "r1", "2026-10-01T00:00:00Z", "res_tdd@tdd2", "res_grill@g1")
	activate(t, d, "mat_new", "2026-10-01T09:00:00Z")
	seedRun(t, d, "run_a", "mat_old")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Generation.State != "diverged" {
		t.Fatalf("state = %q, want diverged", view.Generation.State)
	}
	want := []Change{
		{Kind: "skill", Name: "grill-me", Change: "newer"},
		{Kind: "skill", Name: "tdd", Change: "older"},
	}
	if !reflect.DeepEqual(view.Generation.Drift, want) {
		t.Fatalf("drift = %+v, want %+v", view.Generation.Drift, want)
	}
}

func TestGenerationAheadAfterARollback(t *testing.T) {
	home, d := fixtureHome(t)
	seedCatalog(t, d)
	seedMaterialization(t, d, "mat_old", "r1", "2026-09-30T00:00:00Z", "res_tdd@tdd1")
	seedMaterialization(t, d, "mat_new", "r1", "2026-10-01T00:00:00Z", "res_tdd@tdd1", "res_cave@c1")
	activate(t, d, "mat_old", "2026-10-01T09:00:00Z")
	seedRun(t, d, "run_a", "mat_new")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	want := &Generation{State: "ahead", Launched: "mat_new", Current: "mat_old",
		CurrentSince: "2026-10-01T09:00:00Z",
		Drift:        []Change{{Kind: "skill", Name: "caveman", Change: "extra"}},
		RuntimeDrift: []RuntimeChange{}}
	if !reflect.DeepEqual(view.Generation, want) {
		t.Fatalf("generation = %+v, want %+v", view.Generation, want)
	}
}

func TestGenerationNullWithoutAnActiveMaterialization(t *testing.T) {
	home, d := fixtureHome(t)
	seedMaterialization(t, d, "mat_old", "r1", "2026-09-30T00:00:00Z")
	seedRun(t, d, "run_a", "mat_old")
	view, err := show(t, home, "run_a")
	if err != nil {
		t.Fatal(err)
	}
	if view.Generation != nil {
		t.Fatalf("generation = %+v, want nil", view.Generation)
	}
}

func migratedSchemaChecks(ctx context.Context, d *sql.DB) map[string]func() error {
	return map[string]func() error{
		"generation": func() error {
			_, err := generation(ctx, d, "pro_missing", "claude-code", "mat_missing")
			return err
		},
		"loadMaterialization": func() error {
			_, err := loadMaterialization(ctx, d, "mat_missing")
			return err
		},
		"loadPins": func() error {
			_, err := loadPins(ctx, d, "lock_missing")
			return err
		},
	}
}
```

Delete the stub `migratedSchemaChecks` from `show_test.go` and drop its now-unused `database/sql` import there.

- [ ] **Step 2: Run to verify they fail**

Run: `cd ledger && go test ./internal/runview/ -run TestGeneration -v`
Expected: FAIL. The placeholder returns a nil generation, and `loadPins` is undefined.

- [ ] **Step 3: Implement**

Replace `ledger/internal/runview/generation.go`:

```go
package runview

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"sort"
	"time"
)

// Generation compares the materialization a run launched on with the one a
// new launch of its profile would get now.
type Generation struct {
	State        string          `json:"state"` // current | behind | ahead | diverged
	Launched     string          `json:"launched"`
	Current      string          `json:"current"`
	CurrentSince string          `json:"current_since"`
	Drift        []Change        `json:"drift"`
	RuntimeDrift []RuntimeChange `json:"runtime_drift"`
}

// Change is one pinned resource, from the session's point of view: older or
// newer than the current pin, missing from the session, or extra in it.
type Change struct {
	Kind   string `json:"kind"`
	Name   string `json:"name"`
	Change string `json:"change"`
}

type RuntimeChange struct {
	Kind   string `json:"kind"` // renderer | path_policy | adapter
	Change string `json:"change"`
}

type materialization struct {
	lock, renderer, pathPolicy, createdAt string
	adapterMajor                          int
}

type pin struct {
	kind, name, version, createdAt string
}

func generation(ctx context.Context, d *sql.DB, profileID, harness, launchedID string) (*Generation, error) {
	g := &Generation{Launched: launchedID, Drift: []Change{}, RuntimeDrift: []RuntimeChange{}}
	err := d.QueryRowContext(ctx, `SELECT a.materialization_id, act.activated_at
		FROM active_materializations a
		JOIN materialization_activations act ON act.seq = a.activation_seq
		WHERE a.profile_id = ? AND a.harness = ?`, profileID, harness).Scan(&g.Current, &g.CurrentSince)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read active generation: %w", err)
	}
	if g.Current == launchedID {
		g.State = "current"
		return g, nil
	}
	launched, err := loadMaterialization(ctx, d, launchedID)
	if err != nil {
		return nil, err
	}
	current, err := loadMaterialization(ctx, d, g.Current)
	if err != nil {
		return nil, err
	}
	launchedPins, err := loadPins(ctx, d, launched.lock)
	if err != nil {
		return nil, err
	}
	currentPins, err := loadPins(ctx, d, current.lock)
	if err != nil {
		return nil, err
	}
	older := before(launched.createdAt, current.createdAt)
	g.Drift = diffPins(launchedPins, currentPins)
	g.RuntimeDrift = diffRuntime(launched, current, older)
	g.State = classify(older, g.Drift)
	return g, nil
}

func loadMaterialization(ctx context.Context, d *sql.DB, id string) (materialization, error) {
	var m materialization
	err := d.QueryRowContext(ctx, `SELECT lock_hash, renderer_version, path_policy_version, adapter_api_major, created_at
		FROM materializations WHERE id = ?`, id).
		Scan(&m.lock, &m.renderer, &m.pathPolicy, &m.adapterMajor, &m.createdAt)
	if err != nil {
		return materialization{}, fmt.Errorf("read materialization %s: %w", id, err)
	}
	return m, nil
}

func loadPins(ctx context.Context, d *sql.DB, lock string) (map[string]pin, error) {
	rows, err := d.QueryContext(ctx, `SELECT res.id, res.kind, res.logical_name, plr.version_hash, rv.created_at
		FROM profile_lock_resources plr
		JOIN resources res ON res.id = plr.resource_id
		JOIN resource_versions rv ON rv.hash = plr.version_hash
		WHERE plr.lock_hash = ?`, lock)
	if err != nil {
		return nil, fmt.Errorf("read lock pins: %w", err)
	}
	defer rows.Close()
	pins := map[string]pin{}
	for rows.Next() {
		var id string
		var p pin
		if err := rows.Scan(&id, &p.kind, &p.name, &p.version, &p.createdAt); err != nil {
			return nil, err
		}
		pins[id] = p
	}
	return pins, rows.Err()
}

// diffPins orders versions by creation time; hashes are content-addressed,
// so a tie (or a byte-identical revert) reads as older.
func diffPins(launched, current map[string]pin) []Change {
	changes := []Change{}
	for id, l := range launched {
		c, ok := current[id]
		switch {
		case !ok:
			changes = append(changes, Change{Kind: l.kind, Name: l.name, Change: "extra"})
		case c.version == l.version:
		case before(c.createdAt, l.createdAt):
			changes = append(changes, Change{Kind: l.kind, Name: l.name, Change: "newer"})
		default:
			changes = append(changes, Change{Kind: l.kind, Name: l.name, Change: "older"})
		}
	}
	for id, c := range current {
		if _, ok := launched[id]; !ok {
			changes = append(changes, Change{Kind: c.kind, Name: c.name, Change: "missing"})
		}
	}
	sort.Slice(changes, func(i, j int) bool {
		if changes[i].Kind != changes[j].Kind {
			return changes[i].Kind < changes[j].Kind
		}
		return changes[i].Name < changes[j].Name
	})
	return changes
}

func diffRuntime(launched, current materialization, older bool) []RuntimeChange {
	change := "newer"
	if older {
		change = "older"
	}
	out := []RuntimeChange{}
	if launched.renderer != current.renderer {
		out = append(out, RuntimeChange{Kind: "renderer", Change: change})
	}
	if launched.pathPolicy != current.pathPolicy {
		out = append(out, RuntimeChange{Kind: "path_policy", Change: change})
	}
	if launched.adapterMajor != current.adapterMajor {
		out = append(out, RuntimeChange{Kind: "adapter", Change: change})
	}
	return out
}

// classify takes the run's direction from the materializations' ages; a pin
// that moved the other way means the two have diverged.
func classify(older bool, drift []Change) string {
	state, against := "ahead", "older"
	if older {
		state, against = "behind", "newer"
	}
	for _, c := range drift {
		if c.Change == against {
			return "diverged"
		}
	}
	return state
}

// before compares stored timestamps, whose fraction digits vary.
func before(a, b string) bool {
	ta, errA := time.Parse(time.RFC3339Nano, a)
	tb, errB := time.Parse(time.RFC3339Nano, b)
	if errA != nil || errB != nil {
		return a < b
	}
	return ta.Before(tb)
}
```

- [ ] **Step 4: Run tests**

Run: `cd ledger && go vet ./internal/runview/ && go test -race ./internal/runview/ -v`
Expected: PASS, including `TestQueriesMatchMigratedSchema`. If that test reports `no such column`, the fixture schema has drifted from the real one: fix the query, not the check.

- [ ] **Step 5: Spot-check against the live database (read-only)**

Run:

```bash
cd ledger && cat > /tmp/runview_probe_test.go <<'EOF'
package runview

import (
	"context"
	"encoding/json"
	"os"
	"testing"
)

func TestProbeLive(t *testing.T) {
	home := os.Getenv("PROBE_HOME")
	id := os.Getenv("PROBE_RUN")
	if home == "" || id == "" {
		t.Skip()
	}
	d, err := Open(home)
	if err != nil {
		t.Fatal(err)
	}
	view, err := Show(context.Background(), d, home, id)
	out, _ := json.MarshalIndent(view, "", "  ")
	t.Logf("%s err=%v", out, err)
}
EOF
cp /tmp/runview_probe_test.go internal/runview/probe_test.go
PROBE_HOME=$HOME/.ledger PROBE_RUN=run_259a1b2bf2d7fc576563a9113ef8d095 go test ./internal/runview/ -run TestProbeLive -v
rm internal/runview/probe_test.go
```

Expected: a `behind` generation with `pi-package laya-subagents older` and `skill laya-model-routing older` among its changes, and a `headroom` block if that run had receipts. Don't commit the probe.

- [ ] **Step 6: Commit**

```bash
git add ledger/internal/runview
git commit -m "feat(runview): generation drift for ledger run show"
```

---

### Task 5: `ledger run show` command

**Files:**
- Create: `ledger/cmd/ledger/runshowcmd.go`
- Create: `ledger/cmd/ledger/runshowcmd_test.go`
- Modify: `ledger/cmd/ledger/runcmd.go`, `cmdRun` (dispatch `show` first)
- Modify: `ledger/cmd/ledger/main.go`, `usage()` (the `run` line)
- Create: `ledger/docs/2026-10-01-ledger-run-show.md`

**Interfaces:**
- Consumes: `runview.Open`, `runview.Show`, `runview.ErrUnknownRun` (Tasks 3–4); `exitCodeError` (`gencmd.go`); `captureStdout` (`gencmd_test.go`).
- Produces: the CLI `ledger run show <run-id> --json`. It exits 0 and prints the view JSON, exits 2 and prints `{"error":"unknown_run"}`, or exits 1 on a usage error or an unreadable database.

- [ ] **Step 1: Write the failing tests**

`ledger/cmd/ledger/runshowcmd_test.go`:

```go
package main

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/jws/ledger/internal/db"
)

func TestRunShowUnknownRunExitsTwoWithJSON(t *testing.T) {
	home := t.TempDir()
	t.Setenv("LEDGER_HOME", home)
	migrated, err := db.Open(filepath.Join(home, "ledger.db"))
	if err != nil {
		t.Fatal(err)
	}
	migrated.Close()
	var showErr error
	out := captureStdout(t, func() { showErr = cmdRun([]string{"show", "run_missing", "--json"}) })
	exited, ok := showErr.(interface{ ExitCode() int })
	if !ok || exited.ExitCode() != 2 {
		t.Fatalf("err = %v, want exit 2", showErr)
	}
	if strings.TrimSpace(out) != `{"error":"unknown_run"}` {
		t.Fatalf("stdout = %q", out)
	}
}

func TestRunShowRequiresRunIDAndJSON(t *testing.T) {
	t.Setenv("LEDGER_HOME", t.TempDir())
	for _, args := range [][]string{{"show"}, {"show", "run_a"}, {"show", "--json"}, {"show", "a", "b", "--json"}} {
		if err := cmdRun(args); err == nil || !strings.Contains(err.Error(), "ledger run show") {
			t.Fatalf("%v: err = %v, want usage", args, err)
		}
	}
}

func TestRunShowWithoutDatabaseFails(t *testing.T) {
	t.Setenv("LEDGER_HOME", t.TempDir())
	err := cmdRun([]string{"show", "run_a", "--json"})
	if err == nil {
		t.Fatal("want an error without ledger.db")
	}
	if _, coded := err.(interface{ ExitCode() int }); coded {
		t.Fatalf("an unreadable database exits 1, got %v", err)
	}
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd ledger && go test ./cmd/ledger/ -run TestRunShow -v`
Expected: FAIL. `show` is parsed as a harness name and `cmdRun` returns the run usage.

- [ ] **Step 3: Implement**

`ledger/cmd/ledger/runshowcmd.go`:

```go
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/jws/ledger/internal/paths"
	"github.com/jws/ledger/internal/runview"
)

const runShowUsage = "usage: ledger run show <run-id> --json"

// cmdRunShow prints one run's generation drift and Headroom savings for aoe
// (docs/2026-10-01-ledger-run-show.md). It reads ledger.db read-only and
// never starts ledgerd.
func cmdRunShow(args []string) error {
	var runID string
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		runID, args = args[0], args[1:]
	}
	fs := flag.NewFlagSet("run show", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	jsonOut := fs.Bool("json", false, "JSON output")
	if err := fs.Parse(args); err != nil || !*jsonOut || runID == "" || fs.NArg() != 0 {
		return errors.New(runShowUsage)
	}
	home := paths.LedgerHome()
	database, err := runview.Open(home)
	if err != nil {
		return fmt.Errorf("open ledger database: %w", err)
	}
	defer database.Close()
	view, err := runview.Show(context.Background(), database, home, runID)
	if errors.Is(err, runview.ErrUnknownRun) {
		fmt.Println(`{"error":"unknown_run"}`)
		return exitCodeError{code: 2, msg: "unknown run " + runID}
	}
	if err != nil {
		return err
	}
	return json.NewEncoder(os.Stdout).Encode(view)
}
```

In `runcmd.go`, add the dispatch at the top of `cmdRun`:

```go
func cmdRun(args []string) error {
	if len(args) > 0 && args[0] == "show" {
		return cmdRunShow(args[1:])
	}
	profileName, harness, childArgs, surface, err := parseRunRequestSurface(args)
```

In `main.go` `usage()`, replace the `run` line with:

```
  run           codex|claude|pi --profile <name> [--desktop] [-- <harness args>] | show <run-id> --json
```

- [ ] **Step 4: Run tests**

Run: `cd ledger && go test ./cmd/ledger/ -run 'TestRunShow|TestParseRun' -v`
Expected: PASS. Existing `parseRunRequest` tests still pass because `show` never reaches the harness parser.

- [ ] **Step 5: Write the contract doc**

`ledger/docs/2026-10-01-ledger-run-show.md`:

````markdown
# `ledger run show` (CRA: aoe Ledger run overlay)

`ledger run show <run-id> --json` prints one launch run's generation drift
and Headroom savings. It is read-only: it opens `ledger.db` with
`mode=ro`, reads the run's live totals file, and never starts or calls
ledgerd. aoe polls it for the selected session every 30 s.

## Output (`ledger.run.v1`)

```json
{
  "schema": "ledger.run.v1",
  "run_id": "run_…",
  "harness": "claude-code",
  "profile": "personal",
  "started_at": "…",
  "finished_at": null,
  "generation": {
    "state": "behind",
    "launched": "<materialization id>",
    "current": "<materialization id>",
    "current_since": "<activation time>",
    "drift": [{"kind": "skill", "name": "tdd", "change": "older"}],
    "runtime_drift": [{"kind": "renderer", "change": "older"}]
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

- `generation` is `null` when the profile has no active materialization for
  the harness. `drift` and `runtime_drift` are always arrays.
- `change` is from the session's point of view: `older` / `newer` than the
  current pin, `missing` from the session, `extra` in it. Pins are ordered by
  `resource_versions.created_at`.
- `state`: `current` when the run launched on the active materialization.
  Otherwise the materializations' `created_at` gives the direction (`behind`
  or `ahead`). A pin that moved the other way makes it `diverged`.
- `headroom` is `null` for a run without receipts. A finished run sums its
  `transport.receipt` changes (`source: finished`). A live run reads
  `~/.ledger/run/<run-id>.headroom.json` (`source: live`). Token counts include
  only `accounting_status = evaluated` receipts; every receipt counts as a
  request. `model` is the last inference receipt's.

## Live totals file (`ledger.run-headroom-live.v1`)

The `ledger run` supervisor rewrites it (temp file + rename, mode 0600)
after every validated receipt, across all of the run's leases, and deletes
it once the finish is recorded or spooled. A failed write is the diagnostic
event `headroom_totals_write_failed` and never fails the run.

## Exit codes

0 with the view; 2 with `{"error":"unknown_run"}` on stdout; 1 for usage
errors or an unreadable database.
````

- [ ] **Step 6: Run the whole module**

Run: `cd ledger && go vet ./... && GIT_CONFIG_GLOBAL=/dev/null go test -race ./...`
Expected: PASS. If a test fails that this branch didn't touch, re-run it on `origin/main` before deciding it's pre-existing, and report it.

- [ ] **Step 7: Commit**

```bash
git add ledger/cmd/ledger/runshowcmd.go ledger/cmd/ledger/runshowcmd_test.go ledger/cmd/ledger/runcmd.go ledger/cmd/ledger/main.go ledger/docs/2026-10-01-ledger-run-show.md
git commit -m "feat(run): ledger run show prints a run's drift and Headroom savings"
```

---

### Task 6: Smoke test against the real Ledger home, then PR

- [ ] **Step 1: Build to a scratch path and run against the live database**

```bash
cd ~/code/attic-worktrees/ledger-run-show/ledger && go build -o /tmp/ledger-run-show ./cmd/ledger
/tmp/ledger-run-show run show run_259a1b2bf2d7fc576563a9113ef8d095 --json | python3 -m json.tool | head -40
/tmp/ledger-run-show run show run_nope --json; echo "exit=$?"
```

Expected: valid `ledger.run.v1` JSON for the first; `{"error":"unknown_run"}` and `exit=2` for the second. Don't `go install` yet: deploy is the last task of the aoe plan.

- [ ] **Step 2: Push and open the PR (ask the user first)**

Confirm with the user before pushing. Then:

```bash
git push -u origin feature/ledger-run-show
gh pr create --repo crasiak/attic --base main --title "feat(ledger): run show and live Headroom totals for the aoe overlay" --body "$(cat <<'EOF'
## Summary
- `ledger run` supervisor publishes live Headroom totals per run (`~/.ledger/run/<run>.headroom.json`, atomic rename, removed after finish).
- New read-only `ledger run show <run-id> --json` (`ledger.run.v1`): generation drift (pin diff with older/newer/missing/extra, current/behind/ahead/diverged) and Headroom savings (live file or finished receipts).
- Contract: `ledger/docs/2026-10-01-ledger-run-show.md`.

## Test plan
- [x] `go vet ./... && GIT_CONFIG_GLOBAL=/dev/null go test -race ./...`
- [x] Smoke-tested against the live ledger.db (read-only).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
