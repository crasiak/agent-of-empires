---
status: completed
phase: release-qualified
---
# Execution evidence

Authority: the user's approved four crash-attribution additions and autonomous implementation instruction, with parent approval of this separate AOE adapter. No AWS actions. No live AOE deployment, running-session restart or native provider call.

## Isolation and build provenance

Worktree: `/Users/jws/code/agent-of-empires-worktrees/feature--ledger-restart-attribution`, branch `feature/ledger-restart-attribution`, baseline `71f38129` from `int/crasiak-additions`. The worktree helper created the checkout. APFS clones of existing debug caches belong only to this checkout; original cache/source remain untouched. Native commands use `+stable-aarch64-apple-darwin --offline --target aarch64-apple-darwin --features web --no-default-features`, `CARGO_BUILD_JOBS=1`, and `AOE_WEB_DIST=/Users/jws/code/agent-of-empires-worktrees/crasiak-additions/web/dist`. The unchanged prebuilt web assets avoid network/npm activity. Initial cache was compiled by the x86 host default, so the native first build rebuilt dependencies and took 14m15s. A benign linker warning reports oversized DWARF unwind offsets in this large debug test binary.

## Verification already observed

- Initial TDD compile failed with 13 missing symbols, before adapter implementation; see `tdd-red.txt`.
- First native adapter set: 4 passed, 0 failed, 0 ignored; 0.18s test execution. It established metadata ownership and strict backward compatibility, intent argument semantics, and basic output/time constraints.
- Review exposed that tempfile size was checked only after helper exit and timeout killed only the direct child. Added a live descendant regression, ran it against that implementation, and observed `helper left descendant running`: 0 passed, 1 failed, 2.07s. Fixture cleanup killed its own leaked child. Full output: `process-bounds-red.txt`.
- Ledger producer TDD first failed on the missing new helper, then its focused suite passed. Existing exact origin/account gate and old badge callback retained, new metadata callback separate. Only `cmd/ledger/aoe_launch_report.go` and its test file were edited in the shared Ledger tree; parent owns final integration commit. Evidence: `ledger-producer-red.txt` and `ledger-producer-green.txt`.

## Decisions and boundaries

The helper now uses a nonblocking stdout pipe capped at 4 KiB, null stdin/stderr, a private process group and a cleanup guard on every post-spawn failure/timeout/overflow. It reaps the direct child and kills descendants in its group. No raw output or credentials are logged. Query the agent's first window/first pane, then require matching pane and instance across both old and new reports. Metadata is optional and stays outside persisted AOE storage.

Before managed restart/corpse teardown, record the prior run, prior native identity and already-resolved desired resume/fresh intent, then carry the generated intent through the replacement pane environment. Ordinary launch preparation clears inherited restart intent. Before failed replacement-probe cleanup, request seal-only evidence for the ownership-checked replacement run; create no new intent and claim no verified native resume. Helper absence, metadata absence or collector failure preserves existing restart/cleanup behavior and emits static gap status. Parent reviewed the final bounded helper and ownership checks. A wrapper/profile change can cause strict Ledger attachment to reject the prior-profile intent; preserve the independent evidence and gap rather than parse an opaque wrapper or infer a link. Host Ledger tmux sessions are covered; structured/sandboxed sessions and old sessions without reports remain explicitly uncorrelated.

## Final focused results

`cargo +stable-aarch64-apple-darwin test --offline --target aarch64-apple-darwin --features web --no-default-features --lib session::ledger_restart`

    test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 6885 filtered out; finished in 0.26s

`cargo +stable-aarch64-apple-darwin test --offline --target aarch64-apple-darwin --features web --no-default-features --lib -- session::instance::launch_command session::instance::resume session::instance::start session::launch_identity process::tests:: --test-threads=1`

    test result: ok. 87 passed; 0 failed; 0 ignored; 0 measured; 6803 filtered out; finished in 15.84s

Full output is in `process-bounds-green.txt` and `related-tests.txt`. Existing resume-failure and preserved-SID behavior passed with fake Claude fixtures on the test-only tmux socket. Ordinary launch environment clearing also passed. No real provider executable was run.

## Verification closure

Formatting and diff checks pass. CLI documentation was regenerated with xtask using the native web feature and unchanged web assets; the supervised flag is hidden. Production clippy with `-- -D warnings` passes. Parent owns activation. The full serial suite and final release artifact passed; see final qualification below.


## Outer Ledger reporter cancellation review

Independent review found that the existing Go reporter timeout killed only the immediate AOE process. A new fake reporter regression first failed with `reporter timeout left descendant running` (2.01s). The Go producer now assigns its metadata helper a private process group, kills the group in `Cmd.Cancel`, and sets `WaitDelay=100 ms`. The focused Go report suite passed:

    ok  github.com/jws/ledger/cmd/ledger  3.369s

Parent approved hidden `--supervised-exec` for the optional callback. AOE validates the report then execs tmux directly in the Go-owned group; standalone reporting retains the local guarded helper. Final opt-in binary verification remains pending below.


## Actual callback startup and full-suite findings

The first actual AOE-to-fake-tmux gate did not reach tmux before the outer deadline. The reporting commands entered general startup, including telemetry, migrations and app configuration. A separate isolated run with DO_NOT_TRACK set reached fake tmux successfully in 1.11s, already exceeding the old 1s shared reporter budget. Parent approved an early validated-clap dispatch for reporting only and direct use of the caller's explicit tmux socket, preserving report validation and unrelated command routing. A main-unit regression checks only valid report commands take the fast path; the actual gate also asserts no runtime app directory is created. No raw private stderr was captured.

Full parallel native web suite:

    test result: FAILED. 6887 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out; finished in 187.93s

The sole failure was unchanged `acp::acp_client::steer::tests::follow_up_during_compaction_is_rejected_instead_of_steered`: fake agent stderr reported `sed: command not found`, causing its 30s initialize handshake timeout under concurrent global PATH mutation. Isolated serial rerun of that exact existing binary:

    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6889 filtered out; finished in 3.05s

Final full suite now runs serially to avoid unrelated environment races. Parent requested the release qualification skill; read `/Users/jws/.codex/skills/aoe-release-rebase-deploy/SKILL.md`. No upstream fetch/rebase is in scope. Original integration source remains clean at71f38129. The existing 4.1GiB logical ARM release cache was APFS-cloned into this owned checkout; installed artifacts and live processes remain unchanged.


## Final callback and source qualification before release build

Main startup regression:

    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

Actual native binary gate (`LEDGER_TEST_AOE_EXECUTABLE=<worktree>/target/aarch64-apple-darwin/debug/aoe go test ./cmd/ledger -run '^TestActualAOEReportCancellation$' -count=1 -v`):

    --- PASS: TestActualAOEReportCancellation (2.03s)
        --- PASS: TestActualAOEReportCancellation/false (0.52s)
        --- PASS: TestActualAOEReportCancellation/true (1.51s)
    PASS
    ok  github.com/jws/ledger/cmd/ledger  2.414s

Both modes leave no fake tmux/helper descendant and create no app runtime directory. One first post-rebuild standalone attempt hit its outer 3s fixture deadline; direct isolated retries measured 0.52s and the complete gate then passed. Cold execution/load is a hypothesis, not established cause. OS spawn/startup latency can still make metadata unavailable; callbacks remain best-effort and do not authorize native behavior changes.

Production `cargo clippy --offline --target aarch64-apple-darwin --features web --no-default-features -- -D warnings`:

    Finished `dev` profile [unoptimized + debuginfo] target(s) in 53.87s

`cargo fmt --all -- --check` and `git diff --check` pass. The broader clippy invocation with tests enabled reported one existing `items_after_test_module` warning in unchanged `src/process/macos.rs`; production strict lint is clean. Raw evidence is in `actual-reporter-green.txt`, `startup-test.txt`, and `clippy-production.txt`. Parent committed the shared Go reporter in 9b85c7a7. Full serial suite is still running; source commit/build preparation is not a claim that the unfinished full run passed. Release artifact will be built from this clean source revision and deployment remains with parent.


## Final release qualification

Source commit: `8529aefba662ab2508f045bb698fb0947f9de0b9`. This descends directly from deployed baseline `71f3812950ba`, confirmed by read-only binary inspection of `/Users/jws/.local/bin/aoe` before installation. Prior installed SHA256: `e02359a5181a92e7b0f9625caebe5bd7835639fd32d01707c05723e499409e09`.

The full serial suite completed successfully:

    test result: ok. 6888 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 296.58s
    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
    test result: ok. 289 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 88.06s
    test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

Total: 7,179 passed, 0 failed, 8 ignored. Summary is in `full-suite-summary.txt`; the complete raw log remains `/tmp/aoe-restart-full-serial.txt`.

Release command: `AOE_WEB_DIST=<unchanged baseline>/web/dist CARGO_BUILD_JOBS=1 SHELL=/bin/zsh cargo +stable-aarch64-apple-darwin build --release --offline --target aarch64-apple-darwin --features web --no-default-features`.

    Finished `release` profile [optimized] target(s) in 21m 05s

The selected web/no-default feature set matches the tests. The default `default-plugins` marker is empty and unreferenced in production Rust/build code at this revision, so omitting that marker introduces no additional behavior delta. Full release LTO and `codegen-units = 1` were retained. Vendored OpenSSL and final LTO were monitored through process CPU/object-file progress; no build restart or optimization downgrade occurred. Source stayed clean until the final executable finished.

Artifact: `/Users/jws/code/agent-of-empires-worktrees/feature--ledger-restart-attribution/target/aarch64-apple-darwin/release/aoe`.

- SHA256: `ffefbad8efdf6805ddc7286360284baf09b13a6a17255031dced6f6d6fb4980a`
- Architecture: Mach-O 64-bit arm64, independently checked with `file` and `lipo -archs`.
- Mode 0755; size 55,633,024 bytes; CLI version `aoe 1.16.0`.
- Embedded build identity: `1.16.0+g8529aefba662`, no dirty suffix.
- Toolchain: native rustc 1.97.1 (8bab26f4f, 2026-07-14), LLVM 22.1.6.

Actual final release AOE-to-fake-tmux callback gate:

    --- PASS: TestActualAOEReportCancellation (2.02s)
        --- PASS: TestActualAOEReportCancellation/false (0.52s)
        --- PASS: TestActualAOEReportCancellation/true (1.50s)
    PASS
    ok  github.com/jws/ledger/cmd/ledger  2.442s

Both standalone and outer-supervised modes removed fake tmux and its child and created no runtime app directory. Metadata, raw gate output, toolchain and full build log are committed alongside this record. Parent received the verified artifact and checksum and owns installation/rollback. This worker did not change installed AOE or restart dashboard/TUI/native sessions, did not fetch/rebase upstream, and did not push or remove stashes. Parent separately reported a successful whole-chain fake tmux→installed Ledger→AOE metadata→restart intent attachment smoke. Existing running AOE processes keep their loaded code until normally replaced; this source qualification does not claim retroactive instrumentation.
