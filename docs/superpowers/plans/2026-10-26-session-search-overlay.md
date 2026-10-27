# session-search overlay in the AoE TUI — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From the AoE TUI, `Ctrl-F` opens an fzf-style search over past Claude and Pi sessions (via session-search) and `Enter` jumps to the session's pane or resumes it in a new AoE session; the AoE daemon keeps session-search running.

**Architecture:** A TUI-free core in `src/session_search/` (query client over the `session-search` CLI, grouping by pane, jump/resume targeting, resume-instance builder, daemon supervisor) and one overlay dialog `src/tui/dialogs/session_search.rs` wired into `HomeView` like the command palette. Queries run on a background `Worker`; results reach the dialog each frame.

**Tech Stack:** Rust (ratatui, crossterm, tui-input, serde, tokio for the daemon task), the `session-search` CLI (`query --json`, contract `session-search-query-v1`).

**Spec:** `docs/superpowers/specs/2026-10-25-session-search-aoe-overlay-design.md` in `crasiak/attic` (main, e99085e29). Read it before Task 2.

## Global Constraints

- Repo for tasks 2–9: `crasiak/agent-of-empires` fork, worktree `/Users/jws/code/agent-of-empires-worktrees/session-search-overlay`, branch `feat/session-search-overlay` from fork main 152245478. Task 1 is in `crasiak/attic`.
- Follow `AGENTS.md`: `cargo fmt`, `cargo clippy` clean; no dead code or `#[allow(dead_code)]`; short comments only; no em dashes or `--` as prose separators in docs and comments; settings declared once with `#[setting(...)]`; tests deterministic, isolated, no fixed sleeps (channels or predicates with deadlines); run cargo in the foreground; never set `CARGO_TARGET_DIR` or run `cargo clean`.
- Contract consumed: `session-search query --json [--limit N] [--lens L] [--param k=v]… TEXT`, top-level `{contract, lens, hits[]}`; `contract == "session-search-query-v1"`.
- Never run a resume through a shell. Never print transcript content in logs.
- Keys inside the overlay: letters always type into the query. `↑`/`↓`, `Ctrl-K`/`Ctrl-J` move; `→`/`←` expand/collapse; `Enter` jump/resume; `Tab` scope; `Ctrl-Y` copy resume command; `Ctrl-O` open transcript; `Ctrl-/` toggle preview; `Esc` close.
- Query: 300 ms debounce, ≥ 3 characters, `--limit 50`, 5 s timeout, stale results dropped by generation.
- Scopes: `all` (default, 30 d), `here` (`--lens here --param repo=<repo>` of the session highlighted when the overlay opened; skipped in the cycle when that session has no repo), `recent` (`--lens recent`).
- Supervisor: setting `session.session_search_supervise` (default `true`); restart back-off 1, 2, 4 … 60 s; give up after 5 failures within 10 min; never start a second copy; never kill a copy AoE did not start.
- Layout: under 110 columns the preview is hidden by default; columns drop in this order: folder, then date.

---

### Task 1: session-search exposes the session id a resume targets (crasiak/attic)

**Files:**
- Modify: `session-search/lens/way.go` (struct `Resume` ~line 20, `resumeFor` ~line 202)
- Test: `session-search/lens/way_test.go`
- Modify: `docs/superpowers/specs/2026-10-15-session-search-p1-service-design.md` (way-back paragraph: add `resume.session_id`)

Work in a new attic worktree from origin/main: `git -C /Users/jws/code/attic-worktrees/cps-r2 worktree add -b feat/session-search-resume-session-id /Users/jws/code/attic-worktrees/session-search-resume-id origin/main`.

**Interfaces:**
- Produces: query-v1 `session.resume.session_id` (string): the harness session id the resume opens. For `kind: ledger-parent` it is the parent's id. Additive; existing fields unchanged.

- [ ] **Step 1: Write the failing test** in `way_test.go`, next to the existing Pi run case (`resume.kind == "ledger-parent"`): assert `hit.Session.Resume.SessionID` equals the parent document's `HarnessSessionID`; and for a plain Claude hit, equals its own `HarnessSessionID`.

```go
if got := s.Resume.SessionID; got != parent.SessionRef.HarnessSessionID {
	t.Fatalf("run resume session id = %q, want the parent's %q", got, parent.SessionRef.HarnessSessionID)
}
```

- [ ] **Step 2: Run** `cd session-search && go test ./lens -run TestWay -count=1`. Expected: FAIL, `s.Resume.SessionID undefined`.

- [ ] **Step 3: Implement.** Add the field and set it in `resumeFor`:

```go
type Resume struct {
	Kind      string `json:"kind"` // "ledger" or "claude"; "ledger-parent" resumes a Pi subagent run's parent session
	Command   string `json:"command"`
	SessionID string `json:"session_id"`
	Jump      string `json:"jump,omitempty"`
}
```

In `resumeFor`, after the `switch` assigns `r`: `r.SessionID = d.SessionRef.HarnessSessionID`. (`resumeFor` already receives the parent document for runs, so the parent id flows through.)

- [ ] **Step 4: Run** `cd session-search && just check`. Expected: all green.

- [ ] **Step 5: Commit, PR, merge** (title without a CRA key): `feat(session-search): resume carries the session id it opens`. After merge, rebuild and reinstall `~/.local/bin/session-search` from main and restart the running `serve`/`sync` pair (stop both, start `session-search serve &` then `session-search sync &` with `nohup`), so live JSON has the field.

---

### Task 2: query client and model (`src/session_search/query.rs`)

**Files:**
- Create: `src/session_search/mod.rs`, `src/session_search/query.rs`
- Create: `src/session_search/testdata/query_v1.json` (synthetic, see Step 1)
- Modify: `src/lib.rs` (add `pub mod session_search;`)

**Interfaces:**
- Produces:

```rust
pub enum Scope { All, Here(String), Recent }          // Here carries the repo
pub struct Request { pub generation: u64, pub text: String, pub scope: Scope }
pub struct Hit { pub session: Session, pub snippets: Vec<Snippet> }
pub struct Session {
    pub session_key: String, pub title: Option<String>, pub harness: String, pub repo: String,
    pub last_ts: String, pub cwd: Option<String>, pub branch: Option<String>, pub profile: Option<String>,
    pub transcript_path: Option<String>, pub run_id: Option<String>, pub harness_session_id: Option<String>,
    pub aoe_instance_id: Option<String>, pub resume: Option<Resume>,
}
pub struct Resume { pub kind: String, pub command: String, pub session_id: Option<String>, pub jump: Option<String> }
pub struct Snippet { pub role: String, pub ts: String, pub text: String, pub redactions: u32 }
pub enum QueryError { NotInstalled, NotRunning(String), Timeout, BadOutput(String), Failed(String) }
pub struct Client { pub bin: std::path::PathBuf, pub timeout: std::time::Duration }
impl Client {
    pub fn new() -> Self;                                      // bin "session-search", timeout 5 s
    pub fn args(req: &Request) -> Vec<String>;                 // pure, testable
    pub fn run(&self, req: &Request) -> Result<Vec<Hit>, QueryError>;
}
pub fn parse(stdout: &[u8]) -> Result<Vec<Hit>, QueryError>;
```

- [ ] **Step 1: Write the fixture.** Synthetic values only, real shape (captured 2026-10-26 from `session-search query --json`). Three hits: a Claude hit with a profile and AoE jump; a Pi hit; a Pi subagent run hit (`resume.kind: "ledger-parent"`, `session_id` = parent id) sharing `run_id` with the Pi hit.

```json
{"contract":"session-search-query-v1","lens":"all","hits":[
 {"rank":1,"session":{"session_key":"L1","session_ref":{"ledger_session_id":"L1","harness_session_id":"c-111","run_id":"run_a","aoe_instance_id":"aoe1"},
  "title":"fix the deploy","harness":"claude","repo":"attic","live":false,"last_ts":"2026-10-25T12:58:36.625Z",
  "cwd":"/home/u/code/attic-worktrees/deploy","branch":"deploy","profile":"personal","transcript_path":"/home/u/.ledger/homes/personal/claude-code/state/protected/projects/x/c-111.jsonl",
  "aoe":{"instance_id":"aoe1"},"resume":{"kind":"ledger","command":"cd /home/u/code/attic-worktrees/deploy && ledger run claude --profile personal -- --resume c-111","session_id":"c-111","jump":"aoe session focus aoe1"}},
  "chunks":[{"id":"d#1.0","doc_id":"d","snippet":"…kickstart the daemon…","role":"assistant","ts":"2026-10-25T12:58:40Z","span":{"start":0,"end":10},"redactions":0,"score":3.1}]},
 {"rank":2,"session":{"session_key":"L2","session_ref":{"ledger_session_id":"L2","harness_session_id":"p-222","run_id":"run_b"},
  "title":"pi parent","harness":"pi","repo":"aoe","live":false,"last_ts":"2026-10-24T09:00:00Z","cwd":"/home/u/code/aoe","branch":"main","profile":"personal",
  "transcript_path":"/home/u/.ledger/homes/personal/pi/state/protected/sessions/--x--/t_p-222.jsonl",
  "resume":{"kind":"ledger","command":"cd /home/u/code/aoe && ledger run pi --profile personal -- --session p-222","session_id":"p-222"}},
  "chunks":[{"id":"e#1.0","doc_id":"e","snippet":"…parent words…","role":"user","ts":"2026-10-24T09:00:00Z","span":{"start":0,"end":5},"redactions":0,"score":2.0}]},
 {"rank":3,"session":{"session_key":"L2b","session_ref":{"ledger_session_id":"L2b","harness_session_id":"r-333","run_id":"run_b"},
  "title":"subagent task","harness":"pi","repo":"aoe","live":false,"last_ts":"2026-10-24T08:00:00Z","cwd":"/home/u/code/aoe","profile":"personal",
  "resume":{"kind":"ledger-parent","command":"cd /home/u/code/aoe && ledger run pi --profile personal -- --session p-222","session_id":"p-222"}},
  "chunks":[{"id":"f#1.0","doc_id":"f","snippet":"…run words…","role":"tool","ts":"2026-10-24T08:00:00Z","span":{"start":0,"end":5},"redactions":1,"score":1.5}]}
]}
```

- [ ] **Step 2: Write failing tests** in `query.rs` (`#[cfg(test)] mod tests`):

```rust
#[test]
fn parses_query_v1_and_keeps_way_back_fields() {
    let hits = parse(include_bytes!("testdata/query_v1.json")).unwrap();
    assert_eq!(hits.len(), 3);
    let c = &hits[0].session;
    assert_eq!(c.harness, "claude");
    assert_eq!(c.aoe_instance_id.as_deref(), Some("aoe1"));
    assert_eq!(c.run_id.as_deref(), Some("run_a"));
    assert_eq!(c.resume.as_ref().unwrap().session_id.as_deref(), Some("c-111"));
    assert_eq!(hits[2].session.resume.as_ref().unwrap().kind, "ledger-parent");
    assert_eq!(hits[2].snippets[0].redactions, 1);
}

#[test]
fn rejects_another_contract_and_garbage() {
    assert!(matches!(parse(br#"{"contract":"other","hits":[]}"#), Err(QueryError::BadOutput(_))));
    assert!(matches!(parse(b"not json"), Err(QueryError::BadOutput(_))));
}

#[test]
fn args_follow_the_scope() {
    let req = |scope| Request { generation: 1, text: "two words".into(), scope };
    assert_eq!(Client::args(&req(Scope::All)), ["query", "--json", "--limit", "50", "two words"]);
    assert_eq!(Client::args(&req(Scope::Here("attic".into()))),
        ["query", "--json", "--limit", "50", "--lens", "here", "--param", "repo=attic", "two words"]);
    assert_eq!(Client::args(&req(Scope::Recent)), ["query", "--json", "--limit", "50", "--lens", "recent", "two words"]);
}

#[test]
fn run_maps_cli_failures() {
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str, body: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        p
    };
    let req = Request { generation: 1, text: "abc".into(), scope: Scope::All };
    let client = |bin, ms| Client { bin, timeout: std::time::Duration::from_millis(ms) };
    assert!(matches!(client(dir.path().join("missing"), 2000).run(&req), Err(QueryError::NotInstalled)));
    let down = script("down", "echo 'session-search: cannot reach the service at /x/sock' >&2; exit 1");
    assert!(matches!(client(down, 2000).run(&req), Err(QueryError::NotRunning(_))));
    let slow = script("slow", "exec sleep 5");
    assert!(matches!(client(slow, 200).run(&req), Err(QueryError::Timeout)));
    let ok = script("ok", &format!("cat {}", concat!(env!("CARGO_MANIFEST_DIR"), "/src/session_search/testdata/query_v1.json")));
    assert_eq!(client(ok, 2000).run(&req).unwrap().len(), 3);
}
```

(If `tempfile` is not already a dev-dependency, use whichever temp-dir helper the crate's tests already use; check `Cargo.toml` `[dev-dependencies]`.)

- [ ] **Step 3: Run** `cargo test session_search::query`. Expected: FAIL to compile (module missing).

- [ ] **Step 4: Implement** `query.rs`: serde structs mirroring the JSON (private `#[derive(Deserialize)]` wire types: `Wire { contract, hits: Vec<WireHit> }`, `WireHit { session: WireSession, chunks: Vec<WireChunk> }`, `WireSession` with `session_ref: WireRef`, `#[serde(default)]` on every optional field), then map to the public types. `parse` returns `BadOutput` when JSON fails or `contract != "session-search-query-v1"`. `run`:

```rust
pub fn run(&self, req: &Request) -> Result<Vec<Hit>, QueryError> {
    let mut cmd = std::process::Command::new(&self.bin);
    cmd.args(Self::args(req)).stdin(std::process::Stdio::null());
    match crate::process::run_with_bounded_stdout(&mut cmd, self.timeout, 8 << 20) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(QueryError::NotInstalled),
        Err(e) => Err(QueryError::Failed(e.to_string())),
        Ok(None) => Err(QueryError::Timeout),
        Ok(Some(out)) if out.status.success() => parse(&out.stdout),
        Ok(Some(out)) => {
            let first = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").to_string();
            if first.contains("cannot reach the service") { Err(QueryError::NotRunning(first)) } else { Err(QueryError::Failed(first)) }
        }
    }
}
```

Check `run_with_bounded_stdout` captures stderr; if it does not, set `cmd.stderr(Stdio::piped())` per its docs in `src/process/mod.rs:418`. `mod.rs`: `pub mod query;` and re-export the public types.

- [ ] **Step 5: Run** `cargo test session_search::query` → PASS. `cargo clippy` clean.

- [ ] **Step 6: Commit** `feat(session-search): query client over the session-search CLI`.

---

### Task 3: group hits by pane (`src/session_search/group.rs`)

**Files:** Create `src/session_search/group.rs`; modify `src/session_search/mod.rs`.

**Interfaces:**
- Consumes: `Hit` (Task 2).
- Produces:

```rust
pub struct Row { pub best: Hit, pub others: Vec<Hit> }   // others in rank order
pub fn group(hits: Vec<Hit>, max_rows: usize) -> Vec<Row>;
```

Rule: hits sharing `session.run_id` (when `Some`) form one row; the row's position is its first (best-ranked) hit's; hits without a run id are rows of their own; at most `max_rows` rows.

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn groups_by_run_keeping_rank_order() {
    let hits = crate::session_search::query::parse(include_bytes!("testdata/query_v1.json")).unwrap();
    let rows = group(hits, 20);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].best.session.session_key, "L1");
    assert_eq!(rows[1].best.session.session_key, "L2");
    assert_eq!(rows[1].others.iter().map(|h| h.session.session_key.as_str()).collect::<Vec<_>>(), ["L2b"]);
    assert_eq!(group(crate::session_search::query::parse(include_bytes!("testdata/query_v1.json")).unwrap(), 1).len(), 1);
}
```

Add a case where two hits have `run_id: None`: they stay two rows.

- [ ] **Step 2: Run** `cargo test session_search::group` → FAIL. **Step 3: Implement** with an index map from run id to row. **Step 4: Run** → PASS. **Step 5: Commit** `feat(session-search): group search hits by pane`.

---

### Task 4: jump/resume target (`src/session_search/target.rs`)

**Files:** Create `src/session_search/target.rs`; modify `mod.rs`.

**Interfaces:**
- Consumes: `Session` (Task 2).
- Produces:

```rust
pub struct ProfileHomes { pub profile: String, pub tool_dirs: Vec<(String, std::path::PathBuf)> } // AoE profile, (tool, agent_config_dir)
pub struct ResumeSpec { pub tool: String, pub cwd: String, pub session_id: String, pub aoe_profile: String, pub store: std::path::PathBuf, pub title: String }
pub enum Target { Jump(String), Resume(ResumeSpec), CopyOnly(String), Nothing }
pub fn resolve(s: &Session, live: &dyn Fn(&str) -> bool, profiles: &[ProfileHomes], ledger_home: &std::path::Path) -> Target;
pub fn profile_homes() -> Vec<ProfileHomes>;   // reads crate::session::list_profiles() + resolve_config_or_warn(p).session.agent_config_dir_for(tool, home)
```

Rules, in order:
1. `aoe_instance_id` is `Some(id)` and `live(id)` → `Jump(id)`.
2. `resume` present, harness `claude`→tool `claude`, `pi`→tool `pi`, `resume.session_id` and `cwd` present, `profile` present, and some `ProfileHomes` has `tool_dirs[tool]` under `ledger_home/homes/<profile>/` → `Resume(ResumeSpec{ store: that dir, aoe_profile, title: s.title or "Resumed <first 8 of id>" })`. First matching AoE profile in `profiles` order wins.
3. `resume` present → `CopyOnly(resume.command)`.
4. Else `Nothing`.

- [ ] **Step 1: Failing table test** covering: live instance → Jump; dead instance + matching profile → Resume with that store; Pi run (`ledger-parent`) → Resume with `session_id` = parent id `p-222`; no matching AoE profile → CopyOnly; no resume → Nothing; `profile_homes` not exercised here (it reads user state).

```rust
#[test]
fn resolve_prefers_jump_then_native_resume_then_copy() {
    let hits = crate::session_search::query::parse(include_bytes!("testdata/query_v1.json")).unwrap();
    let home = std::path::Path::new("/home/u/.ledger");
    let profiles = vec![ProfileHomes { profile: "personal-sub".into(), tool_dirs: vec![
        ("claude".into(), home.join("homes/personal/claude-code/runtime")),
        ("pi".into(), home.join("homes/personal/pi/runtime")) ] }];
    assert!(matches!(resolve(&hits[0].session, &|id| id == "aoe1", &profiles, home), Target::Jump(id) if id == "aoe1"));
    match resolve(&hits[0].session, &|_| false, &profiles, home) {
        Target::Resume(r) => { assert_eq!((r.tool.as_str(), r.session_id.as_str(), r.aoe_profile.as_str()), ("claude", "c-111", "personal-sub"));
                               assert_eq!(r.store, home.join("homes/personal/claude-code/runtime")); }
        _ => panic!("expected resume"),
    }
    assert!(matches!(resolve(&hits[2].session, &|_| false, &profiles, home), Target::Resume(r) if r.session_id == "p-222" && r.tool == "pi"));
    assert!(matches!(resolve(&hits[0].session, &|_| false, &[], home), Target::CopyOnly(c) if c.contains("--resume c-111")));
}
```

- [ ] **Step 2: Run** `cargo test session_search::target` → FAIL. **Step 3: Implement** (`Path::starts_with` for the home check). `profile_homes()`: iterate `crate::session::list_profiles().unwrap_or_default()`, `crate::session::config::profile_config::resolve_config_or_warn(&p)`, and for `["claude","pi"]` push `agent_config_dir_for(tool, &home)` when `Some`. Home from `dirs::home_dir()`. **Step 4: Run** → PASS. **Step 5: Commit** `feat(session-search): resolve a hit to a jump or a native resume`.

---

### Task 5: build the resume instance (`src/session_search/resume.rs`)

**Files:** Create `src/session_search/resume.rs`; modify `mod.rs`.

**Interfaces:**
- Consumes: `ResumeSpec` (Task 4); `crate::session::Instance::new(&title, &cwd)`, `crate::session::builder::apply_agent_launch_config(&mut inst, &session_config, "", "", None)`, `crate::session::ResumeIntent::Use`, `crate::session::{ConversationBinding, ExecutionBinding, ConversationProvenance}`, `crate::session::capture::canonicalize_or_raw` (all used by `src/cli/session.rs` `build_import_instance`/`apply_import_mode`, lines ~1065–1125).
- Produces: `pub fn instance(spec: &ResumeSpec, session_config: &crate::session::config::SessionConfig) -> crate::session::Instance;`

Mirror `apply_import_mode`'s terminal branch, generalised to the spec's tool and store:

```rust
pub fn instance(spec: &ResumeSpec, cfg: &SessionConfig) -> Instance {
    let mut inst = Instance::new(&spec.title, &spec.cwd);
    inst.tool = spec.tool.clone();
    crate::session::builder::apply_agent_launch_config(&mut inst, cfg, "", "", None);
    inst.source_profile = spec.aoe_profile.clone();
    inst.resume_intent = crate::session::ResumeIntent::Use(spec.session_id.clone());
    inst.resume_binding = Some(crate::session::ConversationBinding {
        session_id: spec.session_id.clone(),
        execution: Some(crate::session::ExecutionBinding {
            agent: spec.tool.clone(),
            stores: vec![spec.store.clone()],
            configuration: Vec::new(),
            cwd: crate::session::capture::canonicalize_or_raw(&spec.cwd),
            filesystem: "host".into(),
            cwd_filesystem: "host".into(),
            exported_default_store: None,
        }),
        provenance: crate::session::ConversationProvenance::Imported,
        transcript_path: None,
    });
    inst
}
```

If a field or type name differs on fork main, copy the current shape from `apply_import_mode` exactly; do not add parallel structs.

- [ ] **Step 1: Failing test:** for a Claude spec and a Pi spec, assert `inst.tool`, `inst.source_profile`, `inst.resume_intent == ResumeIntent::Use(id)`, binding `agent`, `stores == [store]`, and that `crate::session::instance::launch_command::build_resume_flags`-equivalent public behaviour yields `--resume <id>` for Claude and `--session <id>` for Pi. If `build_resume_flags` is not reachable from here (`pub(super)`), assert only the binding and intent; the launch flags are covered by AoE's existing `resume_and_fork_flags_per_agent` test.
- [ ] **Step 2: Run** `cargo test session_search::resume` → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS. **Step 5: Commit** `feat(session-search): native resume instance for a search hit`.

---

### Task 6: setting and daemon supervisor (`src/session_search/supervisor.rs`)

**Files:**
- Modify: `src/session/config/mod.rs` (field on `SessionConfig` next to `show_ledger_overlay` ~line 1097; default in `impl Default` ~line 2042)
- Create: `src/session_search/supervisor.rs`; modify `mod.rs`
- Modify: `src/server/startup.rs` (spawn next to `super::systemd_reconcile::spawn(state.clone());` ~line 868)

**Interfaces:**
- Produces:

```rust
pub fn socket_path() -> std::path::PathBuf;             // $SESSION_SEARCH_SOCKET, else $XDG_STATE_HOME/session-search/sock, else ~/.local/state/session-search/sock
pub fn probe(socket: &std::path::Path) -> bool;          // connect + "GET /v1/health HTTP/1.0\r\nHost: x\r\n\r\n", true on a "200" status line within 1 s
pub struct Supervisor { /* bin, socket, log dir, backoff, failures */ }
impl Supervisor {
    pub fn new(bin: std::path::PathBuf, socket: std::path::PathBuf, log_dir: std::path::PathBuf) -> Self;
    pub async fn run(self, shutdown: tokio_util::sync::CancellationToken);
}
pub fn spawn_if_enabled(enabled: bool, shutdown: tokio_util::sync::CancellationToken);
pub fn last_error() -> Option<String>;                   // read by the overlay; a process-wide Mutex<Option<String>>
```

Setting:

```rust
/// Keep session-search's `serve` and `sync` running from the `aoe serve` daemon.
#[serde(default = "default_true")]
#[setting(label = "Keep session search running", widget = "toggle", web = "local_only:starts host processes")]
pub session_search_supervise: bool,
```

Behaviour of `run`:
1. If `probe(socket)` is true, record nothing and idle until shutdown (re-probe every 30 s; if it later fails, fall through to step 2).
2. Start `bin serve` (stdout/stderr to `log_dir/session-search-serve.log`), wait until `probe` is true (deadline 30 s), then start `bin sync` (log `session-search-sync.log`). Children inherit nothing beyond the daemon env.
3. When a child exits, restart it after back-off 1, 2, 4 … 60 s. Track failures in a 10 min window; at 5, set `last_error` to the last exit status plus the last log line and stop restarting.
4. Before starting `sync`, never start one if a `session-search sync` from another parent is running: if `probe` passed in step 1 the service is someone else's and AoE manages neither.
5. On shutdown, terminate only children it started (`crate::process::worker::terminate_process_group` on their pids or `Child::kill`).

- [ ] **Step 1: Failing tests** with fake binaries (shell scripts in a temp dir) and a temp socket path:
  - `probe` false on a missing socket; true against a `UnixListener` test server answering `HTTP/1.0 200 OK`.
  - Supervisor with an already-answering socket starts nothing (fake bin writes a marker file if run; assert no marker after the supervisor reports idle via a test hook channel).
  - Fake `serve` that creates the socket and answers health, fake `sync` that exits immediately: `sync` is restarted (count marker lines reaches 2 within a deadline); after 5 quick exits `last_error()` is `Some`.
  - Setting default is true and appears in `settings_schema::schema()` (mirror `ledger_overlay_settings_default_on_and_reach_every_surface` at `src/session/config/mod.rs:4873`).
- [ ] **Step 2: Run** `cargo test session_search::supervisor` → FAIL. **Step 3: Implement.** Use tokio process (`tokio::process::Command`) and `tokio::time::sleep` for back-off (back-off is a real delay, not a test synchronisation). Expose a `#[cfg(test)]` way to shorten back-off. In `startup.rs`: `crate::session_search::supervisor::spawn_if_enabled(config.session.session_search_supervise, state.shutdown.clone());` using `crate::task_util::spawn_supervised("server.session_search", PanicPolicy::Log, …)` inside `spawn_if_enabled`; log dir from `crate::session::get_app_dir()`. **Step 4: Run** → PASS; `cargo clippy` clean. **Step 5: Commit** `feat(session-search): aoe serve keeps session-search running`.

---

### Task 7: the overlay dialog (`src/tui/dialogs/session_search.rs`)

**Files:**
- Create: `src/tui/dialogs/session_search.rs`
- Modify: `src/tui/dialogs/mod.rs` (`pub mod session_search;` and re-export `SessionSearchDialog`, `SearchOutcome`)

**Interfaces:**
- Consumes: Tasks 2–4 types; `crate::tui::worker::Worker`; `crate::tui::dialogs::{DialogResult, paste_into_input}`; `crate::tui::styles::Theme`.
- Produces:

```rust
pub enum SearchOutcome { Open(crate::session_search::target::Target), Copy(String), OpenTranscript(std::path::PathBuf) }
pub struct SessionSearchDialog { /* input, scope, here_repo, rows, cursor, expanded, preview_on, state, generation, debounce deadline, worker */ }
impl SessionSearchDialog {
    pub fn new(
        here_repo: Option<String>,
        runner: Box<dyn FnMut(Request) -> (u64, Result<Vec<Hit>, QueryError>) + Send>,
        resolver: Box<dyn Fn(&Session) -> Target + Send>,
    ) -> Self;
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> DialogResult<SearchOutcome>;
    pub fn handle_paste(&mut self, text: &str);
    pub fn tick(&mut self, now: std::time::Instant) -> bool;          // fires a due debounced query, drains results; true when a repaint is needed
    pub fn render(&mut self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect, theme: &Theme);
}
```

The runner is injected so tests drive it synchronously; production passes `move |req| (req.generation, client.run(&req))` wrapped in a `Worker` (`Worker::spawn("aoe-session-search", runner)`), requests carry the generation, and `tick` drops results whose generation is below the latest requested.

Behaviour:
- Typing edits the query; `tick` sends a query 300 ms after the last edit when `len ≥ 3` (pass `now` so tests control time; no sleeps). Fewer than 3 chars clears rows.
- `Tab` cycles `All → Here(repo) → Recent → All`, skipping `Here` when `here_repo` is `None`, and re-queries immediately.
- `↑`/`↓`/`Ctrl-K`/`Ctrl-J` move over visible rows (expanded children count as rows). `→` expands the selected group, `←` collapses.
- `Enter` returns `Submit(SearchOutcome::Open(resolver(&selected.best.session)))` using the resolver passed to `new` (the expanded child's session when a child row is selected).
- `Ctrl-Y` → `Submit(Copy(resume.command))` when the selected session has one; `Ctrl-O` → `Submit(OpenTranscript(path))` when `transcript_path` is set; `Ctrl-/` toggles the preview; `Esc` → `Cancel`.
- Errors (`QueryError`) render in the preview area as one line; `NotInstalled`/`NotRunning` also show `crate::session_search::supervisor::last_error()` when set.

Render (fzf-style, see spec *Overlay*):
- Full area. Bottom line: `> ` + query (cursor via `set_prefixed_input_cursor_position`). Above it the info line: `{shown}/{total}  {scope}` plus a spinner char while a query is in flight. Above that the list, best row nearest the prompt.
- Each row one line: `repo·folder` (folder = last path segment of `cwd`, truncated with `…` to 16 cols), harness (6 cols), date (`MM-DD`, or `HH:MM` when today in local time), marker column (`●` when the resolver returns `Target::Jump` for the row, count of `others` when > 0), title filling the rest with query terms bold. Selected row: `▌` in col 0 and `Modifier::REVERSED`; other rows alternate a faint background (`theme` dim surface color; check `DESIGN.md` and `src/tui/styles` for the existing stripe/selection colors and use them).
- Preview (right, 45%, separated by `│`) when `preview_on && width ≥ 110` (default `preview_on = width ≥ 110` at first render): resume command (or jump), `folder · branch · profile`, a rule, then up to 3 snippets as `role MM-DD HH:MM` and the snippet text, wrapped.
- Under 110 cols: drop folder; under 80: drop date.

- [ ] **Step 1: Failing tests** (synchronous runner returning the fixture; `test_keys::{key, ctrl_key}`; `ratatui::backend::TestBackend` for render assertions):
  - typing `ab` then `tick(t+400ms)` sends no query; typing `c` then `tick(t+250ms)` sends none; `tick(t+301ms)` sends exactly one with text `abc`.
  - a result from an older generation is dropped.
  - `Tab` with `here_repo = Some("attic")` cycles All → Here("attic") → Recent → All; with `None` skips Here.
  - letters like `j`, `k`, `y`, `o` type into the query (not move/copy).
  - after results: 2 rows (grouped); `→` on row 2 shows 3 visible rows; `↓` moves; `Enter` returns `Submit(Open(_))` from the injected resolver; `Ctrl-Y` returns `Copy` with the selected command; `Ctrl-O` returns `OpenTranscript`.
  - render at 140×20: the bottom line starts with `> `, the info line contains `2/2`, the selected row's first cell is `▌`, the preview shows `ledger run claude`; at 90×20 no `│` preview separator and no folder text.
  - an error result renders its message.
- [ ] **Step 2: Run** `cargo test tui::dialogs::session_search` → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS; clippy clean. **Step 5: Commit** `feat(tui): fzf-style session search overlay`.

---

### Task 8: wire the overlay into the home view

**Files:**
- Modify: `src/tui/home/bindings.rs` (new `ActionId::SessionSearch`; `Binding` with `non_strict: &[ctrl('f')]`, `strict: &[ctrl('f')]`, `context: Context::Always`, help `"Search transcripts"`, palette `PaletteMeta { title: "Search transcripts", keywords: &["search","transcript","history","session-search","find"], group: PaletteGroup::Views }`; add `ActionId::SessionSearch => "session-search"` to the id-name match ~line 1202)
- Modify: `src/tui/home/mod.rs` (field `pub(super) session_search: Option<SessionSearchDialog>` next to `command_palette` ~line 221), `src/tui/home/lifecycle.rs` (`session_search: None` next to `command_palette: None` ~line 296)
- Modify: `src/tui/home/input.rs`: key capture block like the palette's (~line 2018) before it; paste (~line 1618 pattern); `run_action` arm opening the dialog; outcome dispatch
- Modify: `src/tui/home/render.rs`: add `session_search` to the `render_dialogs!` list (~line 1286) and to both "dialog open" `||` chains (~lines 1750 and `overlays.rs:125`)
- Modify: the per-frame tick where `apply_ledger_updates` is called: call `session_search.tick(Instant::now())` and repaint when true

**Interfaces:**
- Consumes: Task 7 dialog; Task 4 `profile_homes`, `Target`; Task 5 `instance`; `HomeView::{select_session_by_id, activate_selected_session, add_instance, save, reload, select_and_reveal_session, get_instance}`; `crate::tui::clipboard::copy_to_clipboard`; `Action::EditFile(PathBuf)`, `Action::SetTransientStatus(String)`.

Opening (`run_action(ActionId::SessionSearch)`):

```rust
ActionId::SessionSearch => {
    let here_repo = self.selected_session.as_deref().and_then(|id| self.get_instance(id)).map(|inst| {
        let path = std::path::Path::new(&inst.project_path);
        crate::git::worktree::GitWorktree::find_main_repo(path)
            .ok()
            .and_then(|main| main.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| crate::session::projects::repo_label(&inst.project_path))
    });
    // An instance AoE still lists can be jumped to: Enter on a stopped one restarts it as usual.
    let live: std::collections::HashSet<String> = self
        .instances()
        .filter(|i| !i.is_trashed() && !i.is_archived() && i.status != Status::Deleting)
        .map(|i| i.id.clone())
        .collect();
    let profiles = crate::session_search::target::profile_homes();
    let ledger_home = dirs::home_dir().unwrap_or_default().join(".ledger");
    let resolver = Box::new(move |s: &Session| {
        crate::session_search::target::resolve(s, &|id| live.contains(id), &profiles, &ledger_home)
    });
    let client = crate::session_search::query::Client::new();
    self.session_search = Some(SessionSearchDialog::new(
        here_repo,
        Box::new(move |req| (req.generation, client.run(&req))),
        resolver,
    ));
}
```

Check the exact paths `crate::git::worktree::GitWorktree` and `crate::session::projects::repo_label` compile from `src/tui/home/input.rs` (both exist on fork main: `src/git/worktree/mod.rs:112`, `src/session/projects.rs:292`).

Outcomes:

```rust
SearchOutcome::Open(Target::Jump(id)) => { self.select_session_by_id(&id); return self.activate_selected_session(); }
SearchOutcome::Open(Target::Resume(spec)) => {
    let cfg = crate::session::config::profile_config::resolve_config_or_warn(&spec.aoe_profile).session;
    let inst = crate::session_search::resume::instance(&spec, &cfg);
    let id = inst.id.clone();
    // same storage bootstrap as create_session (operations.rs:~262)
    self.add_instance(inst); self.rebuild_group_trees();
    if let Err(e) = self.save().and_then(|_| self.reload()) { return Some(Action::SetTransientStatus(format!("resume failed: {e}"))); }
    self.select_and_reveal_session(&id);
    return self.activate_selected_session();
}
SearchOutcome::Open(Target::CopyOnly(cmd)) | SearchOutcome::Copy(cmd) => { crate::tui::clipboard::copy_to_clipboard(&cmd); return Some(Action::SetTransientStatus("resume command copied".into())); }
SearchOutcome::Open(Target::Nothing) => return Some(Action::SetTransientStatus("no way back for this session".into())),
SearchOutcome::OpenTranscript(path) => return Some(Action::EditFile(path)),
```

Before acting, leave live mode the way `dispatch_palette_action` does (`exit_live_send_and_restore_sizing`). Extract the storage bootstrap from `create_session` into a small helper if needed rather than duplicating it.

- [ ] **Step 1: Failing tests** in `src/tui/home/tests/` (new file `session_search.rs`, registered in `tests/mod.rs`), using `create_test_env_with_sessions` and a dialog built with a fixture runner and a stub resolver (add a `#[cfg(test)]` constructor path on `HomeView` that injects the runner/resolver, so tests do not run the real CLI):
  - `Ctrl-F` opens the overlay; `Esc` closes it; while open, `j` types into its query (does not move the home cursor).
  - `Enter` with resolver → `Jump(id of session 1)` selects that session and returns the attach action.
  - `Enter` with resolver → `Resume(spec)` adds exactly one instance with `resume_intent == Use(spec.session_id)` and selects it.
  - `Ctrl-Y` copies (assert the transient status text).
  - the command palette lists "Search transcripts".
  - an existing test that enumerates every `ActionId` (grep `ActionId::` exhaustiveness tests in `bindings.rs`) still passes.
- [ ] **Step 2: Run** `cargo test tui::home::tests::session_search` → FAIL. **Step 3: Implement.** **Step 4: Run** the new tests and then `cargo test` (whole crate) → PASS; `cargo fmt`, `cargo clippy` clean. **Step 5: Commit** `feat(tui): Ctrl-F opens session search from the home view`.

---

### Task 9: docs, full check, PR, live verification

**Files:** Modify only what is needed: if a user guide lists TUI keys (grep `docs/` for "command palette" or "Ctrl-K"), add one line for `Ctrl-F`; regenerate generated docs only if their sources changed (`cargo xtask gen-docs` for CLI reference — not expected to change).

- [ ] **Step 1:** `cargo fmt && cargo clippy && cargo test` (no `--features web` unless a web file changed). All green.
- [ ] **Step 2:** Push the branch, open a PR on `crasiak/agent-of-empires` titled `feat(tui): search past sessions with session-search (Ctrl-F)` (no CRA key in title or branch), body linking the spec in `crasiak/attic` and listing tests and the live check below. Related: CRA-1241, CRA-1242.
- [ ] **Step 3: Live check with jws** after merge and install of the fork build (follow the fork's usual install path; ask jws before replacing the installed `aoe` binary and before restarting `aoe serve`):
  1. `Ctrl-F`, type a word from an open pane's session → row shows `●` → `Enter` lands in that pane.
  2. A closed Claude session → `Enter` creates a new AoE session in its folder that resumes it (history visible).
  3. A closed Pi session → same.
  4. `aoe serve` restarted with search stopped → search comes back on its own; `curl --unix-socket ~/.local/state/session-search/sock localhost/v1/health` answers.
  5. Reboot (when convenient) → search is back after AoE starts.
- [ ] **Step 4:** Record the result in the spec's *Delivery* section in `crasiak/attic` (small docs PR) and move CRA tracking as appropriate.
