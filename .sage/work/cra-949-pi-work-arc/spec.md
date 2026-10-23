---
status: design-ready
phase: design
tracker: CRA-949
scope: host Pi Sessions in the AoE TUI
---

# Work arc for host Pi Sessions

## Outcome and boundaries

A person selecting an opted-in host Pi Session can see Goal, Now, Remaining, and Next without asking the Agent for a status report. The arc changes after meaningful recorded progress, a detour, a blocker, a changed goal or completion. It reports uncertainty and the source's own assertions honestly. The four fields together contain at most 80 whitespace-delimited words. Neither a test pass reported by the Agent nor a proposed choice becomes verified or accepted by summarization. Never invent a percentage, ETA, permission or completed rollout.

The first release covers host Pi Agent conversations in Terminal view on the AoE TUI. Sandboxed Pi, ACP, other Agents and the Web dashboard are out of scope and show no work arc. The separate CRA-949 real-return study is paused; it is not a feature dependency, and enabling this product must not enroll a research source. The preferred fictional 64-word cue is feedback, not production proof.

## Admission and privacy

A Session is disabled by default. The user must opt in on that particular AoE Session before AoE reads any of its Agent conversation. No profile or global enrollment, background corpus scan, auto-enrollment on launch, or real-session read during development. Disabling, clearing the Agent conversation or purging the Session removes its derived arc. Raw excerpts stay in process memory, are never logged or persisted by AoE, and are not sent to Linear, a KB or an unrelated process. Ordinary Pi session storage remains Pi's own data.

Before a read, AoE must positively bind the selected Session ID, live launch generation, Pi session ID, published exact transcript path and header. It must reject ambiguous or missing provenance, foreign owners, path substitution, symlinks, nonregular files, partial records and an untrusted cursor. The reader extracts text only from the active branch under a fixed I/O, record, text and time budget; it does not include system prompts, thinking, tool calls/results, images or arbitrary extension payloads. A missing ancestor or insufficient goal evidence yields unknown/stale rather than a plausible but false arc. The last recorded file entry is not proof of the active branch: Pi may navigate `/tree` without appending a record.

## Cursor and model request

The existing AoE Pi extension publishes only a small, atomic, pane-scoped cursor record containing the Pi session ID, active leaf entry ID or null, launch source and update time. Publish on session start, tree navigation and settled progress; invalidate on session switch/shutdown. The cursor carries no conversation text. Existing running Pi processes without the updated extension cannot be made current by reading their file tail. They show an explicit unavailable or stale state; no forced reload/restart.

AoE builds a bounded excerpt from that exact Agent conversation after admission and opt-in. It sends the excerpt over a size- and deadline-limited, owner-private host Unix socket to the *same live Pi process* that owns the cursor, with a versioned request ID and expected Session, Pi ID, launch generation and leaf. The extension rejects mismatches before a model call, runs a separate no-tools call through its own selected model route and returns structured fields, actual physical provider/model and usage when supplied. A virtual route may select a different physical model for a direct call; show both selected route and actual response instead of promising the same physical model or price. Do not fall back to AoE's generic CLI one-shot, another account or another model when the route is unavailable. The extension has no raw request file and no autonomous request when disabled. Close the socket on shutdown; rate limits, request IDs and opt-in checks are enforced again by AoE on receipt.

## Refresh and projection

The TUI owns refresh in this first slice. While no TUI is running, no work-arc generation runs and no model spend occurs; an opted-in stored arc can become stale and must be checked and refreshed on selection/reopening. This is not the Daemon-owned always-on design target. A later Daemon move requires its own decision and tests.

Attempt the first generation when the opted-in Agent conversation has a usable initial exchange. After that, use observed settled progress or branch changes, with at most one new generation per five minutes and twenty attempts per rolling 24 hours per Session. The initial attempt bypasses only the five-minute spacing, counts toward twenty, and manual refreshes respect both ceilings. Coalesce pending changes while one request is in flight. An unchanged covered leaf does not regenerate. Time and daily limits are product ceilings, not freshness promises. Failed attempts consume no claimed successful update and have bounded retry/backoff; cost accounting must distinguish no request, unknown usage and measured usage.

Store only the derived arc and its attribution in the AoE read model: AoE Session ID, Pi ID, exact covered leaf, launch generation, timestamp, four capped fields, selected route and actual response/usage if known. Persist a bounded derived record and rate-budget metadata locally so a TUI restart cannot reset spend or leak the prior Session's arc. Any change to persisted Session schema or path has a migration. A delayed response cannot overwrite a newer leaf or one from another Session, generation, disabled state or conversation clear. A result for an older leaf may remain visible as stale, never current. Do not mix a prior summary from an abandoned branch into a new branch without an explicit common ancestor.

Distinct UI states: off, missing/no useful input, loading, current, stale (with covered time), failed (with last good stale if applicable), cleared and unavailable. Absence of a cursor or model route is unavailable, not current. Show the four fields with short labels in the metadata side of the existing info panel, clamp height and width, and permit an expanded read of clipped text. Compact viewport or hidden info retains the existing output-first behavior. `agent_info_height`, `PreviewLayout`, `InfoAreas`, live-send sizing and capture/scroll geometry must agree; badges and resource traces retain their space.

## Evidence required before rollout

- Synthetic Pi JSONL fixtures: linear, branched, cursor-not-at-file-tip, switch without append, compaction, clear, incomplete line, hostile path, duplicate ownership, missing ancestor, oversize, missing and stale source. Do not qualify via a real user's session.
- Extension tests with fake model registry and fake socket: no-tools invocation, exact selected route, route absence, cursor lifecycle, bounded request, wrong ID/generation/leaf, duplicate nonce, timeout and cleanup. No paid model call in automated tests.
- AoE projection tests: opt-in gate, per-Session and per-branch ordering, budget across restart, failure/retry, disable/clear/purge, no raw persistence or logs, late responses after selection change.
- TUI buffer/layout tests at wide, narrow and short sizes, clipping/expanded view, state labels, live-send geometry and switching Sessions. A representative synthetic multi-step run covers initial work, progress, detour/blocker, return/next action and completion.
- Explicit user selection for deployment and actual host Pi Session enrollment. Tests/builds do not prove that a live TUI displayed an arc. Report model latency and cost separately from cue usefulness.
