# Tailored Sessions

Press **Ctrl+Y** in the TUI or Web dashboard to open the Tailored Sessions picker,
including while a Session owns keyboard input. Choose **Capture an idea**, select
Personal or Work, and enter the idea. Enter captures; Shift+Enter inserts a newline.
Long text and unbroken URLs soft-wrap in the TUI without adding saved newlines.

**Immediately** is unchecked by default. The capture skill saves the original idea
and proposes project-ticket, Atlas, or KB destinations before making destination
changes. Check Immediately to authorize well-supported changes without that review.
Missing access or ambiguous destinations still require clarification. Immediately
does not waive a destination skill's scope, provenance, or review requirements.

Capture opens a new scratch Session in an overlay above the sidebar and current
Session. Ctrl+Y closes the overlay without stopping capture or changing the current
Session. Escape closes the picker/form; inside the capture terminal it goes to the
agent. The TUI also accepts Ctrl+Q to close the overlay.

## Check previous captures

Ctrl+Y shows the new-capture action and recent runs together. Search by idea,
Title, account, Profile, or Status, then open an existing run to continue that same
Session in the overlay. Reopening does not create another Session or change the
underlying selection. F2 returns from a capture or its form to history; the web
also has a Back to history button.

Use **Hide** to remove an entry from recent history without stopping, archiving,
trashing, or deleting its Session. This preference is shared by TUI and web and
survives restarts. Use **Hidden** and **Restore** to bring an entry back. In the TUI,
arrow keys select, Enter opens, Delete hides/restores, and F3 switches recent/hidden.
Both clients refresh history while the window is open. Archived and trashed
Sessions stay outside this view; restore them through the normal Session list.

Captures also remain in the normal Session list: Scratch when grouped by project,
or Tailored when grouped manually. A hidden capture can still run and await input.
Hiding is a view preference, not a substitute for a Session stop or trash action.

Shared visibility is stored separately in `state.toml`, keyed by Profile and
Session id. Migration v050 initializes that UI state and preserves a backup of the
old state file without rewriting Session records. This advances the app schema;
older builds refuse that newer schema. Close older AoE clients before using a
newly installed build, and do not lower the cursor to force a downgrade.

## Minimal Ledger profiles

AoE does not install Ledger, change its live config, select credentials implicitly,
or fall back to a daily profile. Provision these dedicated Pi profiles first:

| Account selector | Ledger profile |
| --- | --- |
| Personal | `aoe-capture-personal` |
| Work | `aoe-capture-work` |

Both require the self-contained [capture-idea skill](../../contrib/skills/capture-idea/SKILL.md).
Copy it to your managed skill source directory and import it into the dedicated
profiles, not `common`. Use separate account bindings and minimal Pi settings/model
resources for each boundary. Do not extend `personal`, `work`, `common`, or the daily
Pi bundles. Both profiles may add `linear-project-work`, `librarian`, and `artifacts`
for ticket and local-file routing. Only Work may add `jira` and `confluence`.

A profile declaration has this shape; resource names and account references must
match your Ledger installation:

```yaml
profiles:
  aoe-capture-personal:
    bindings:
      pi:
        accountRef: pi-personal
        launcher: direct
        config: settings/pi-capture-personal
    resources:
      add:
        - skill/capture-idea
        - skill/linear-project-work
        - skill/librarian
        - skill/artifacts
        - models/pi-capture-personal
  aoe-capture-work:
    bindings:
      pi:
        accountRef: pi-work
        launcher: direct
        config: settings/pi-capture-work
    resources:
      add:
        - skill/capture-idea
        - skill/linear-project-work
        - skill/librarian
        - skill/artifacts
        - skill/jira
        - skill/confluence
        - models/pi-capture-work
```

Use your installation's pinned Pi `core` and transport binding when applicable.
The example does not authorize applying a partial Ledger config: its apply is
full-state, so merge declarations into the complete config and review its preview.
Import a separate minimal Pi source for each profile with `ledger resource import-pi`;
its settings should not reference daily packages, extensions, instructions, or skills.
Then resolve and materialize each profile using Ledger's normal workflow.

Before creation AoE reads `ledger lock show --profile <profile> --harness pi`.
It requires `skill/capture-idea`, permits the account-scoped routing skills above,
model resources and the optional `pi-package/pi-mcp-adapter`, and rejects unrelated
skills or packages. Atlassian skills are refused in the Personal profile. The profile's
settings must contain only the explicitly selected destination integrations.
Ledger remains responsible for verifying the active generation, settings, account,
and runtime at launch. AoE's resource check is not an audit of settings contents or
an OS sandbox.

## Destination access and recovery

AoE does not hardcode a ticket tracker, Atlas, or KB location. Supply the appropriate
account-scoped integration through the minimal profile. The capture skill can use
an installed `linear` CLI without loading a general MCP package. Its optional
`references/destinations.json` supplies installation-specific workspace scopes;
it must not contain credential values. For work Jira/Confluence, the configured
`confluence` skill can provide a tenant-pinned Keychain helper and Jira CLI wrapper.
The credential must be provisioned separately with the user's authorization, not
copied into the manifest, skill files, or command arguments. Missing or locked
credentials fail visibly; capture must not change authentication to work around it.
The capture skill infers useful destinations from accessible context and checks for duplicates. Without an
integration it retains a local draft and reports the missing access rather than
pretending to publish it. Account and review policy are instructions to the skill;
use access-scoped credentials and integrations to enforce write boundaries.

The idea is retained in the Session's launch arguments when creation commits, even
before the agent has made a model request. The skill saves `idea.md` in its scratch
directory and appends actual destination receipts. Normal scratch retention and
purge rules apply; purging a scratch Session can remove that local capture.

Web creates use the existing durable, keyed-create reconciliation. A lost response
can be retried under the same key. Closing during creation leaves reconciliation
running; check the Session list before starting another capture. A changed or
shelved TUI capture target returns to history rather than forwarding input to a
stale tmux session.

## Discuss a public document

Choose **Document chat**, select Personal or Work, paste a public direct Markdown
or PDF URL, optionally enter an opening question, then select **Start**. This is a
direct Pi conversation, not the research development/review flow. The TUI form
uses Tab to move between URL, question, account and Start; Enter on Start confirms.
Both text fields soft-wrap without changing the retained bytes.

AoE downloads and extracts the source before starting any model turn. Only public
HTTP(S) direct links are supported. Redirects, encoded responses, credentials,
credential-like query parameters, HTML/login pages, OCR and private sources are
refused. Supply the final uncompressed link. Loading has a 30-second deadline,
a 10 MiB download limit and a 256 KiB UTF-8 text limit. Oversize, empty, scanned,
encrypted or unextractable documents fail visibly without truncation. PDF
extraction requires local `pdftotext` on PATH; no remote extraction is used.

Provision dedicated `aoe-document-personal` and `aoe-document-work` Ledger profiles.
Each requires a direct `pi-home` binding, an immutable pinned Pi core, model-only
resources and explicit `launch.share: [conversation]`. No capture, daily, skill,
package, extension or MCP resource fallback is allowed. AoE reads the selected
profile's lock and `ledger launch describe` metadata and refuses stale/unqualified
runtimes without applying or materializing anything. Ledger owns account selection,
model settings, protected native history and runtime verification.

This requires a Ledger version whose read-only direct Pi description returns the
verified pinned Node/Pi execution prefix and protected runtime metadata, plus a Pi
build supporting the restrictive launch flags and native transcript selectors.
A Ledger version that rejects direct Pi descriptions or describes ambient `pi`
is not supported. Source-contract fixtures do not qualify an installed producer;
activation remains blocked until that Ledger producer contract is implemented and
independently qualified with the provisioned profiles. AoE never substitutes a
bare Pi executable or a different launcher. Capability help runs with a private
temporary HOME and Pi directory, not the selected account's runtime settings/auth.

Document chat disables tools, MCP, ambient extensions, context files, skills and
prompt templates. Only AoE's fixed host-owned Pi identity extension is explicitly
loaded, for ordinary conversation identity/status/usage reporting. Document content
cannot select extensions or change the account or system policy. These launch
restrictions are not an OS sandbox.

The private managed scratch directory retains the bounded original, exact extracted
`source.txt` and `provenance.json` with retrieval time, sanitized URL, media/extractor
identity, sizes and separate raw/text SHA-256 hashes. Query values and fragments
are not retained in provenance or history. Normal scratch purge removes this source
snapshot; the Agent conversation remains in Ledger's native Pi history independently.
Migration v052 advances the app schema without rewriting Session rows. This is a
semantic safety fence: older clients cannot safely reopen document rows even
though their fields still deserialize. Stop all old AoE clients and daemons before
upgrading. Schema-51 and older builds refuse a store stamped 52; this does not
protect against an old process left running. Do not lower the schema cursor or
force a downgrade to reopen these conversations.

The first native launch attaches the frozen text once using Pi's full UTF-8 `@file`
reader. Normal resume uses the exact native transcript, without fetching, extracting,
reattaching or automatically starting another turn. If the initial launch was claimed
but its native history cannot be qualified, AoE refuses automatic replay. Inspect the
existing Session rather than creating a second copy. Closing, reopening, Hide and
Restore use the ordinary Tailored history and terminal lifecycle. The Web dashboard
uses the same keyed pending-create reconciliation as capture and does not equate an
unconfirmed create with a loaded source or a started model. Once persistence is
attempted, an uncertain failure retains the source snapshot and initial-turn claim;
a keyed retry resolves a known original Session rather than launching another one.
If ownership cannot be confirmed, inspect history before creating anything new.
The TUI prepares and requalifies cold native launches in background workers, both
on initial Start and when reopening from history. Closing an accepted start does
not cancel it or authorize replay.

## Develop a research prompt

Choose **Tailored research** for **quick prompt → developed prompt → recommended
ORX/EVO → choose/override → review/edit → Run**. The original quick prompt is
retained separately. Saving, editing, choosing a backend and developing do not
start a research backend. Development makes a bounded model-only Pi request from
an isolated directory, without tools, extensions, MCP, context files, skills,
prompt templates or a saved Pi session. Errors remain visible in the draft.

Supply context and explicit source scopes yourself. There are no connectors or
integration crawls. Personal categories are Grafana, OpenTelemetry and Zulip;
Work is Slack. These additions do not reclassify separately scoped Work observability
instances such as Polaris. Use Supplied for explicitly provided account-appropriate
context outside these category choices; it is not permission to crawl. Category membership is not evidence of connectivity, tenant access
or permission to disclose private context to public search. Review disclosure
limits separately. Changing account, sources or supplied context requires fresh
development; any edit invalidates the UI's review acknowledgement.

Development requires the dedicated model-only, direct Ledger Pi profiles
`aoe-research-personal` and `aoe-research-work`. Neither can fall back to a daily
or capture profile. AoE verifies the lock's profile, harness, `pi-home` account
class, direct launcher and model-only resources. Ledger verifies the selected
account and active generation at launch; the common `pi-home` class alone cannot
prove which account is selected.

Choose the backend explicitly, review the developed text, existing absolute
workspace, permitted scope, consequential unknowns and execution effects, then
acknowledge the review and select **Run reviewed revision**. In the TUI, Tab and
Shift+Tab move between fields, F6 switches account, F7 chooses/overrides backend,
F8 acknowledges review, Ctrl+S saves, Ctrl+D develops and **Ctrl+R runs**. Run freezes
the saved revision; another client's stale revision cannot dispatch.

Run may start a managed backend. Installed Ledger launchers must support the
`aoe-orx-launcher-v1` / `aoe-evo-launcher-v1` capability contracts; legacy launchers
are refused without invocation. Personal routes are `research` / `evo` with
`claude-max` and the direct launcher. Work routes are `research-work` / `evo-work`
with `claude-bedrock-aperture` and the isolated Headroom launcher. Missing profiles,
mismatched locks or unavailable launchers are visible errors, never account
fallbacks. AoE does not provision or apply those external profiles/helpers.

- **ORX:** AoE starts its own loopback-only server in a new account/run-isolated
  state directory, with no browser or eager agent startup. Run also authorizes
  project registration and ORX's project-starter model warmup. It creates a chat
  and submits the frozen prompt once using the `sonnet` model alias and
  `bypassPermissions`. ORX research workers can execute commands. The receipt
  retains the profile, server PID/address, project, chat and turn identity. Open
  that dashboard on the AoE host and select the recorded project/chat. ORX is not
  a Claude/Pi Session: do not use AoE's agent resume/fork controls for it.
- **EVO:** Run invokes **discover and baseline only** in an owned tmux pane with
  a fixed native session UUID. This explores the repository, initializes `.evo`,
  constructs a baseline worktree and runs the first experiment. The reviewed
  scope and evaluation criteria are required. No optimize, ship, merge or push is
  authorized. These backend instructions are not an OS experiment sandbox.
  Optimization requires subsequent explicit review and is not implemented in this
  interface. Its budget means per-branch serial experiment depth, not dollars.
  Reopen the research entry to read output and send literal discovery answers
  explicitly. The TUI uses F11 for output and F9 to send the answer field. Known
  optimize/shipping invocations are refused. A missing pane is never recreated.

Research history includes saved drafts and launch/return identities, not fake
running Pi Sessions. Hide/Restore changes only visibility. Migration v051 adds
`research.json`; reviewed prompts and startup logs live under the private app
state directory. Closing a form does not cancel a committed operation. Reopen
from history or refresh that exact request (TUI F10) to inspect its state.

**Submitted means handed off, not completed.** An interrupted Running or uncertain
outcome may already have started work. AoE records intent before startup/POST,
disables HTTP retries, and never automatically retries an ambiguous submission
as a new run. Inspect retained identity/output/logs before considering a new,
separately reviewed request. Capture's keyed-create reconciliation is unchanged
and is not used as a substitute for this research dispatch boundary.

If development was interrupted, **Abandon interrupted development** (TUI F12)
retains the original and context and returns to an editable draft. This is an
explicit abandonment, not automatic model retry. Late results from the abandoned
revision are ignored. Run/uncertain requests cannot use this recovery action.
The exact frozen prompt is limited to 65,536 UTF-8 bytes before Run commits;
oversize reviews remain editable and are never silently truncated.
