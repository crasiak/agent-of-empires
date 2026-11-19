# Marks, bulk actions and Breakout windows

**Date:** 2026-11-19
**Status:** Design approved in chat 2026-11-19. Not yet planned or implemented.
**Repo:** agent-of-empires (crasiak fork, branch `multi-window-aoe`).

## Goal

Let the user mark several Sessions in the TUI sidebar and act on all of them at once:

1. Send one message to many Sessions.
2. Restart many Sessions, each relaunching with what it was already running.
3. Break the marked Sessions out into tiled Ghostty windows for focused work, while the aoe TUI stays open. This must also work when aoe runs on a remote host (bender) and the windows open on the laptop.

## Language

Add to `CONTEXT.md`:

- **Mark**: a transient flag the user puts on a Session in the sidebar. _Avoid_: select, selection (that is the cursor row).
- **Marked set**: every Session currently carrying a Mark.
- **Bulk action**: an action applied to the Marked set.
- **Breakout**: opening the Marked set as tiled terminal surfaces outside the aoe TUI. _Avoid_: focus (taken by focus requests, `src/tui/focus_requests.rs`).
- **Breakout agent**: the laptop-side listener that opens Breakout windows on behalf of an aoe TUI running on another host.

## Scope

In scope: the TUI home view, the three bulk actions above, Ghostty on macOS.

Out of scope: the web dashboard, the remote home view (`src/tui/remote_home`), other bulk actions (trash, archive, move, rename), other terminal emulators, other operating systems for Breakout. The daemon is unchanged in phase 1.

## Phases

Each phase is usable on its own and lands as its own PR.

| Phase | Delivers |
|-------|----------|
| 1a | Marks: state, keys, rendering |
| 1b | Bulk action framework and bulk send |
| 1c | Bulk restart |
| 2a | Breakout, separate windows (default layout), local |
| 2b | Breakout, splits layout (opt-in setting), local |
| 2c | Breakout from a remote aoe through the Breakout agent |

## 1a. Marks

**State.** `HomeView` gains a set of marked session ids. It is local to the running TUI: not persisted, not sent to the daemon, not shared with other TUIs.

**Keys.**

- `Space` toggles the Mark on the cursor row. `Space` is unbound in the sidebar today.
- `Space` on a Group header marks every Session that Group currently shows (respecting active filters, including rows hidden only by collapse). If all of them are already marked, it unmarks them.
- `Shift+Up` / `Shift+Down` moves the cursor one row and marks both the row left and the row reached. This replaces the current ten-row jump on those chords. `PageUp` / `PageDown` already page the list, so the ten-row jump is removed rather than rebound.
- `Esc` clears the Marked set when no dialog or overlay is open and the set is non-empty. Existing `Esc` behaviour applies otherwise.
- All four are registered in `src/tui/home/bindings.rs` so they appear in help and the palette and follow strict-mode rules.

**Lifetime.** Marks survive sorting, filtering, grouping changes and collapsing. A Mark is dropped when its Session is trashed, purged or no longer present after a reload. Marks are cleared after a bulk action is submitted.

**Rendering.** A marked row shows a marker glyph in the row gutter and a distinct style from the theme. While the Marked set is non-empty the footer shows `N marked`, plus `(M hidden)` when some marked rows are not visible, and lists the keys that are in bulk mode.

## 1b. Bulk action framework and bulk send

**Eligibility.** One pure function, given a bulk action and the marked Sessions, returns the eligible Sessions and the skipped ones, each with a reason from a closed list. It reads only `Instance` fields and facts passed in (for example, whether an agent pane exists), so it is unit-testable without tmux. Every bulk dialog shows this split before the user confirms.

**Entry points.**

- While the Marked set is non-empty, the send key (`m`, `M` in strict mode) and the restart key (`e` / `E` / `F5`) act on the Marked set. Every other key keeps acting on the cursor row.
- A new key opens a "N marked" menu listing the bulk actions. Proposed: `x`; the plan confirms it is free in both modes.
- The same actions are in the command palette and the right-click context menu when the Marked set is non-empty.

**Bulk send.**

- The existing compose dialog opens, titled `Send to N sessions`, and lists skipped Sessions with reasons.
- The target is always the agent pane, whatever the current view mode.
- Skipped: no live agent pane; archived or trashed; Status Creating or Deleting; waiting on a permission prompt (typed text could answer the prompt).
- Delivery reuses the existing per-session prepare-then-deliver path (`finish_send` and the daemon-confirmed lease), one Session after another. A failure for one Session does not stop the rest.
- When all deliveries finish, the TUI shows `Sent to X` or, on any failure, a dialog listing each failed Session and its error.
- Open item for the plan: confirm how a structured-view (ACP) Session receives a message on this path. If it cannot, it is skipped with its own reason.

## 1c. Bulk restart

- A confirm dialog shows the marked Sessions grouped by Status (for example `3 idle, 2 running`) and the skipped ones with reasons. Choices: `Restart all`, `Restart idle only`, `Cancel`.
- Skipped: archived, trashed, Creating, Deleting, restart already in flight, snoozed under Attention sort (same rules as the single restart).
- Each restart is the existing plain restart with no overrides (`RestartSessionBody` with no profile, tool, args or command override). The Session therefore keeps its stored profile, tool, extra args, command override and Ledger launch, and resumes its conversation when `auto_resume_on_restart` is on. The configured wake message applies as it does for a single restart.
- Restarts are staggered: at most two in flight. The rest wait in a TUI-side queue that submits the next restart when one completes or is refused. Quitting the TUI abandons the unsent remainder.
- Rows show the existing restarting state. A summary reports successes, refusals and failures when the queue drains.
- The single-restart guards (cooldown, in-flight set, shelved refusal) are reused per Session by extracting the body of `restart_selected_session_with_wake` into a function that takes a session id.

## 2. Breakout

### Shared pieces

- **Surface command.** Each surface runs `aoe session attach <id>` (`--profile` as needed), which exists today. tmux allows several clients on one tmux session, so the aoe preview and a Breakout window can show the same Session; tmux sizes the window to the smallest attached client.
- **Breakout request.** A small serialisable value: the ordered session ids with their Titles and profiles, the layout (`windows` or `splits`), and the host the Sessions run on. Local and remote Breakout both produce this value; only the transport differs.
- **Grid.** A pure function maps a count and a screen rectangle to cell rectangles: 2 side by side, 3 in columns, 4 as 2x2, 5 and 6 as 3x2, 7 to 9 as 3x3. More than 6 Sessions asks for confirmation; more than 9 is refused.
- **Driver.** A Ghostty driver turns a Breakout request into AppleScript and runs it with `osascript`. Script generation is a pure function with unit tests; running it is a thin wrapper.
- **Tracking.** aoe records the ids of the windows it opened so a `Close breakout windows` action can close them. A window also closes when its attach exits.
- **Availability.** Breakout is offered only on macOS with Ghostty present, or when a Breakout agent socket is reachable. Otherwise the action reports that it is unsupported.
- **Setting.** `breakout.layout = "windows" | "splits"`, default `windows`.

### 2a. Separate windows (default)

Ghostty's AppleScript dictionary (1.4.1) can create a window running a command but exposes no window position or size, so placement uses macOS Accessibility through System Events.

1. For each Session, `new window with configuration` whose command is the surface command, and set a unique title override (`aoe breakout · <Title> · <id8>`).
2. Read the visible frame of the screen holding the frontmost Ghostty window.
3. For each new window, find it by its unique title in System Events and set its position and size to its grid cell.

This needs Accessibility permission for the process that runs `osascript`. Phase 2a starts with a spike that proves steps 2 and 3 against Ghostty and records what the permission prompt looks like and which process it names. If the permission is missing, Breakout still opens the windows untiled and tells the user how to grant it.

### 2b. Splits (opt-in)

One `new window`, then `split` commands with a surface configuration per Session to build the same grid inside it. No Accessibility permission is needed. The window title names the Breakout.

### 2c. Remote Breakout through the Breakout agent

When aoe runs on bender and is viewed over ssh, the TUI cannot run AppleScript on the laptop.

- **Agent.** `aoe breakout-agent` runs on the laptop (launchd job) and listens on an owner-only Unix socket. It accepts exactly one request type, a Breakout request, and builds the surface command itself as `ssh -t <host> aoe session attach <id>`. It never executes text supplied by the remote side. Session ids, profiles and the host are validated against strict patterns, and the host must be on an allow-list in the laptop's aoe config.
- **Transport.** ssh `RemoteForward` exposes the laptop socket on bender at a well-known path. The TUI on bender uses the forwarded socket when it exists and answers a version handshake; otherwise it falls back to the local driver or reports unsupported.
- **Flow.** The user triggers Breakout in the remote TUI; aoe sends the request over the socket; the agent runs the same driver as the local case and returns the outcome, which the TUI shows.
- **Setup guidance (docs).** The launchd job, the `RemoteForward` line, `StreamLocalBindUnlink yes` on bender's sshd, and ssh connection sharing (`ControlMaster`) so N windows do not mean N handshakes.
- **Later.** The same request could travel over the personal tailnet instead of an ssh forward. Not in this design.

### Stretch

When every Session in a Breakout shares one Group, collapse that Group in the sidebar. Controlled by a setting, default off.

## Error handling

- Bulk actions never abort on the first failure; they collect per-session outcomes and report them together.
- A Session that changes state between the confirm dialog and execution is re-checked with the eligibility function and skipped with a reason if it no longer qualifies.
- Breakout failures (osascript error, missing permission, agent unreachable, agent refusal) surface as a dialog with the cause; already-opened windows are left open and tracked.

## Testing

- Unit: mark toggling, Group-header toggling, range extension, pruning on removal; the eligibility function for each action; the restart queue's concurrency limit; grid maths; generated AppleScript text; Breakout request validation in the agent.
- TUI harness (`src/tui/home/tests`): key retargeting while marked, `Esc` clearing, dialog contents, footer text, single-session behaviour unchanged when nothing is marked.
- Agent: socket round-trip with a fake driver; rejection of malformed or disallowed requests.
- Manual: Ghostty window creation and tiling locally; Breakout from bender to the laptop.
