# Decisions

## 2026-09-17: Clean release artifact qualifies source 8529aefb

The final serial web suite passed 7,179 tests with 8 intentional ignores; strict production clippy, formatting and real release AOE-to-fake-tmux cancellation checks passed. Build the full optimized ARM/web artifact with one compiler job from clean 8529aefb, retaining LTO and the tested feature set. The installed binary was confirmed as source 71f38129, the exact feature baseline. Hand the verified checksum/path to parent for reversible installation, preserving attached TUI and live agents. This later documentation-only commit does not change the qualified source or artifact build identity. No upstream rebase, provider calls or installed mutation by this worker.

## 2026-09-17: Reporting bypasses general application startup

Actual binary verification showed the old reporting path could spend its whole deadline on unrelated CLI prework before reaching tmux. Parent approved dispatching only validated report-launch/report-ledger-launch commands immediately after clap parsing, before telemetry, migrations or application setup. Construct their structured tmux command directly with the caller’s explicit socket. Other session commands keep normal startup. The actual gate must verify no application runtime directory is created. The parallel full suite exposed an unrelated global PATH race; isolate it and rerun the final full suite serially.

## 2026-09-17: The outer Ledger callback owns supervised publication

Parent approved a hidden --supervised-exec mode on the new optional reporting subcommand. It validates the same bounded metadata, then execs the structured tmux publication command directly in Ledger’s reporting process group. This avoids the nested private group that a forced Go timeout could otherwise orphan. Standalone reporting retains its own guarded 500 ms helper. Ledger uses group cancellation and a 100 ms WaitDelay for reporter callbacks only; native-agent supervision policy is unchanged. Add a real AOE-to-fake-tmux opt-in test for both modes.

## 2026-09-17: Changed wrapper profiles retain a visible attribution boundary

Parent review confirmed strict profile matching should remain. Restart record uses the prior ownership-checked report, not an inferred parse of an arbitrary replacement wrapper. A wrapper/profile change can therefore leave the prior intent and independent new run without a verified attachment. Preserve this explicit gap instead of inventing linkage or changing launch policy.

## 2026-09-17: Review hardens helper resource and cleanup boundaries

The initial tempfile capture checked size only after helper exit and did not supervise descendants. Independent review identified an attribution tool that could itself consume unbounded disk. Replace it with a live 4 KiB stdout pipe limit, discard stderr, create a private process group and guard every post-spawn path with kill/reap cleanup. Descendant pipe ownership shares the same deadline. Reuse this helper for the optional tmux report publisher. Before failed replacement cleanup, request a separate bounded seal for the ownership-checked replacement run; do not create a false second restart intent or infer verified native identity.

## 2026-09-17: AOE observes Ledger restart intent without choosing resume policy

The user approved the four crash-attribution plans and autonomous execution. Parent approved this adapter and its exact cross-repo CLI contract. Preserve the existing strict report-launch envelope and publish optional Ledger IDs through a separate report-ledger-launch command and pane option. Require matching instance and pane ownership before using prior IDs, including a dead pane's report. Record before actual teardown; preserve restart behavior if metadata, the helper or the collector is unavailable. Use the already-resolved native resume decision, never infer successful resume from AOE pane liveness. Carry the returned intent only into the next launch environment and unset stale inherited values for ordinary launches. Use the existing host Ledger CLI, with bounded subprocess time/output and no body, environment or stderr capture. Structured and sandboxed sessions are outside this local host adapter. No persisted AOE fields or migration are introduced. Parent owns rollout.

## 2026-09-17: Isolated build and compatibility verification

Worktree helper created feature/ledger-restart-attribution from int/crasiak-additions (71f38129), preserving the dirty main checkout. Existing build caches are APFS-cloned into this worktree; no deployed cache/source mutation. Initial TDD compile used the host's x86 default and reported 13 missing-symbol errors. Subsequent compilation uses native ARM with one compiler job, offline dependencies, one web feature set and the unchanged prebuilt web assets. Ledger reporting tests preserve the exact existing report-launch arguments, reject mismatched origin/account context and preserve the old badge when an old AOE rejects the optional command.
