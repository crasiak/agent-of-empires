# Tailored Sessions

Press **Ctrl+Y** in the TUI or Web dashboard to open the Tailored Sessions picker,
including while a Session owns keyboard input. Choose **Capture an idea**, select
Personal or Work, and enter the idea. Enter captures; Shift+Enter inserts a newline.

**Immediately** is unchecked by default. The capture skill saves the original idea
and proposes project-ticket, Atlas, or KB destinations before making destination
changes. Check Immediately to authorize well-supported changes without that review.
Missing access or ambiguous destinations still require clarification. Immediately
does not waive a destination skill's scope, provenance, or review requirements.

Capture opens a new scratch Session in an overlay above the sidebar and current
Session. Ctrl+Y closes the overlay without stopping capture or changing the current
Session. Escape closes the picker/form; inside the capture terminal it goes to the
agent. The TUI also accepts Ctrl+Q to close the overlay. Continue a closed capture
from the Session list. Scratch captures appear in Scratch when grouped by project,
or in Tailored when grouped manually.

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
shelved TUI capture target closes its overlay rather than forwarding input to a
stale tmux session.
