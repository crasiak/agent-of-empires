import { useState } from "react";
import type { WorkshopDeployTarget, WorkshopOperation, WorkshopRequest } from "../lib/types";

const input =
  "w-full rounded-md border border-surface-700 bg-surface-950 p-2 text-sm text-text-primary disabled:opacity-50";
const button =
  "h-9 rounded-md border border-surface-700 px-3 text-xs text-text-primary hover:bg-surface-800 disabled:opacity-50";
const evidence =
  "max-h-72 overflow-auto whitespace-pre-wrap break-all border border-surface-700 p-2 font-mono text-xs text-text-primary";
export function WorkshopWorkflow({
  request,
  locked,
  operate,
}: {
  request: WorkshopRequest;
  locked: boolean;
  operate: (operation: WorkshopOperation) => Promise<void>;
}) {
  const [remote, setRemote] = useState("");
  const [repositoryId, setRepositoryId] = useState("");
  const [base, setBase] = useState("");
  const [title, setTitle] = useState(`Workshop: ${request.envelope.slug}`);
  const [body, setBody] = useState("");
  const [disclosureDigest, setDisclosureDigest] = useState<string | null>(null);
  const [rulesDigest, setRulesDigest] = useState<string | null>(null);
  const [custodyDigest, setCustodyDigest] = useState<string | null>(null);
  const [sourceName, setSourceName] = useState("");
  const [targets, setTargets] = useState("[]");
  const [error, setError] = useState("");
  const p = request.publication;
  const d = request.deployment;
  const state = request.state;
  const id = request.id;
  const revision = request.revision;
  const disclosure = !!p?.preview && disclosureDigest === p.preview.digest;
  const currentRules = !!p?.policy && rulesDigest === p.policy.digest;
  const custody = !!p?.policy && custodyDigest === p.policy.digest;
  const basic = (
    operation:
      | "continue-publish"
      | "preview-land"
      | "reconcile-publication"
      | "preview-deploy"
      | "preview-rollout"
      | "reconcile-deploy",
  ) => void operate({ operation, id, revision });
  const previewImport = () => {
    try {
      const selected = JSON.parse(targets) as WorkshopDeployTarget[];
      if (!Array.isArray(selected) || !selected.length) throw new Error("Select explicit pairs");
      setError("");
      void operate({ operation: "preview-import", id, revision, source_name: sourceName, targets: selected });
    } catch {
      setError(
        "Enter an explicit nonempty JSON target array. Every target needs profile, harness, account and sessions.",
      );
    }
  };
  const native = d?.rollouts.at(-1);
  return (
    <div className="space-y-3 border-t border-surface-700 pt-3">
      <h3 className="text-sm font-semibold text-text-primary">Destination publication</h3>
      <p className="text-xs text-text-secondary">
        Preview reads actual destination identity, visibility and host actor. Account/source suitability is not
        public-disclosure permission. No remote writes occur until separate approval.
      </p>
      {["committed", "publish-preview"].includes(state) && (
        <div className="space-y-2">
          <label className="block text-xs text-text-secondary">
            Canonical GitHub HTTPS remote
            <input
              aria-label="Canonical GitHub HTTPS remote"
              className={input}
              disabled={locked}
              value={remote}
              onChange={(e) => setRemote(e.target.value)}
            />
          </label>
          <label className="block text-xs text-text-secondary">
            GitHub repository ID
            <input
              aria-label="GitHub repository ID"
              className={input}
              disabled={locked}
              value={repositoryId}
              onChange={(e) => setRepositoryId(e.target.value)}
            />
          </label>
          <label className="block text-xs text-text-secondary">
            Destination base branch
            <input
              aria-label="Destination base branch"
              className={input}
              disabled={locked}
              value={base}
              onChange={(e) => setBase(e.target.value)}
            />
          </label>
          <label className="block text-xs text-text-secondary">
            PR title
            <input
              aria-label="PR title"
              className={input}
              disabled={locked}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <label className="block text-xs text-text-secondary">
            PR body
            <textarea
              aria-label="PR body"
              className={input}
              disabled={locked}
              value={body}
              onChange={(e) => setBody(e.target.value)}
            />
          </label>
          <button
            className={button}
            disabled={
              locked || !remote || !base || !Number.isSafeInteger(Number(repositoryId)) || Number(repositoryId) <= 0
            }
            onClick={() =>
              void operate({
                operation: "preview-publish",
                id,
                revision,
                remote,
                repository_id: Number(repositoryId),
                base_branch: base,
                title,
                body,
              })
            }
          >
            Preview destination disclosure
          </button>
        </div>
      )}
      {p?.preview && (
        <>
          <p className="text-xs text-text-primary">
            {p.preview.repository.visibility} repository {p.preview.destination.owner}/{p.preview.destination.name} (ID{" "}
            {p.preview.repository.id}) · host actor {p.preview.actor.login} (ID {p.preview.actor.id}) ·{" "}
            {p.preview.source.account} source
          </p>
          <pre aria-label="Exact destination disclosure" className={evidence}>
            {JSON.stringify(p.preview, null, 2)}
          </pre>
          <label className="block text-xs text-text-primary">
            <input
              type="checkbox"
              checked={disclosure}
              disabled={locked || state !== "publish-preview"}
              onChange={(e) => setDisclosureDigest(e.target.checked ? p.preview!.digest : null)}
            />{" "}
            I approve disclosing the exact reviewed skill and PR title/body to this destination under this host actor.
          </label>
          <button
            className={button}
            disabled={locked || state !== "publish-preview" || !disclosure}
            onClick={() =>
              void operate({
                operation: "publish",
                id,
                revision,
                preview_digest: p.preview!.digest,
                disclosure_confirmed: disclosure,
              })
            }
          >
            Approve destination Publish
          </button>
        </>
      )}
      {state === "publish-ready" && (
        <button className={button} disabled={locked} onClick={() => basic("continue-publish")}>
          Continue confirmed unsent Publish stage
        </button>
      )}
      {p?.published && (
        <pre aria-label="Actual published PR" className={evidence}>
          {JSON.stringify(p.published, null, 2)}
        </pre>
      )}
      <h3 className="text-sm font-semibold text-text-primary">Separate Land</h3>
      <p className="text-xs text-text-secondary">
        Pins the actual published head; GitHub enforces then-current base and rules with direct non-bypass squash. Base
        progress is allowed. GitHub has no stack-membership or target-ref CAS. You must exclusively control this owned,
        unstacked default-branch PR throughout Land; nobody else may retarget, stack or edit it. Unknown outcomes halt
        Deploy.
      </p>
      <button
        className={button}
        disabled={locked || !["published", "land-preview"].includes(state)}
        onClick={() => basic("preview-land")}
      >
        Preview current Land rules
      </button>
      {p?.policy && (
        <>
          <pre aria-label="Current Land policy observation" className={evidence}>
            {JSON.stringify(p.policy, null, 2)}
          </pre>
          <label className="block text-xs text-text-primary">
            <input
              type="checkbox"
              checked={currentRules}
              disabled={locked || state !== "land-preview"}
              onChange={(e) => setRulesDigest(e.target.checked ? p.policy!.digest : null)}
            />{" "}
            I approve this exact head under the current server base and rules, not an exact-base/policy CAS.
          </label>
          <label className="block text-xs text-text-primary">
            <input
              type="checkbox"
              checked={custody}
              disabled={locked || state !== "land-preview"}
              onChange={(e) => setCustodyDigest(e.target.checked ? p.policy!.digest : null)}
            />{" "}
            I confirm exclusive PR custody during Land and assume nobody else retargets, stacks or edits it.
          </label>
        </>
      )}
      <button
        className={button}
        disabled={locked || state !== "land-preview" || !p?.policy || !currentRules || !custody}
        onClick={() =>
          p?.policy &&
          void operate({
            operation: "land",
            id,
            revision,
            policy_digest: p.policy.digest,
            current_base_and_rules: currentRules,
            exclusive_pr_custody: custody,
          })
        }
      >
        Land
      </button>
      {["publishing", "publish-unknown", "landing", "land-pending", "land-unknown"].includes(state) && (
        <button
          className={button}
          disabled={locked || p?.land_observation_pending === true}
          onClick={() => basic("reconcile-publication")}
        >
          Read-only reconcile remote intent
        </button>
      )}
      {p?.land_execution != null && (
        <pre aria-label="Durable Land execution evidence" className={evidence}>
          {JSON.stringify(p.land_execution, null, 2)}
        </pre>
      )}
      {p?.land_observation_pending && (
        <p role="alert" className="text-xs text-status-error">
          Land observation settlement is incomplete. Older execution evidence cannot be reused; further remote
          reconciliation and Deploy are blocked.
        </p>
      )}
      {state === "land-pending" && (
        <p className="text-xs text-status-warning">
          Pending UUID is not landed. Explicit read-only bounded reconciliation is required. No merge write will be
          replayed.
        </p>
      )}
      {p?.landed && (
        <pre aria-label="Verified actual landed source" className={evidence}>
          {JSON.stringify(p.landed, null, 2)}
        </pre>
      )}
      <h3 className="text-sm font-semibold text-text-primary">Separate Ledger Deploy</h3>
      <p className="text-xs text-text-secondary">
        Requires the current companion capability contract and a reachable existing daemon. No install or daemon
        bootstrap. Select exact profile/harness/account/session pairs; an empty sessions array is explicit publish-only,
        never all sessions. Preview shows every current consumer, including denied, changing and binding-less profiles,
        plus future selected pairs and account revisions.
      </p>
      {state === "landed" && !d && (
        <>
          <label className="block text-xs text-text-secondary">
            Ledger source name
            <input
              aria-label="Ledger source name"
              className={input}
              disabled={locked}
              value={sourceName}
              onChange={(e) => setSourceName(e.target.value)}
            />
          </label>
          <label className="block text-xs text-text-secondary">
            Explicit deployment targets JSON
            <textarea
              aria-label="Explicit deployment targets JSON"
              className={`${input} font-mono`}
              disabled={locked}
              value={targets}
              onChange={(e) => setTargets(e.target.value)}
            />
          </label>
          <p className="text-xs text-text-secondary">
            Each target: {`{"profile":"selected","harness":"pi","account":"personal","sessions":[]}`}
          </p>
          <button className={button} disabled={locked || !sourceName.trim()} onClick={previewImport}>
            Preview guarded import
          </button>
        </>
      )}
      {error && (
        <p role="alert" className="text-xs text-status-error">
          {error}
        </p>
      )}
      {d?.import_preview && (
        <>
          <pre aria-label="All import consumers and selected pairs" className={evidence}>
            {JSON.stringify(d.import_preview, null, 2)}
          </pre>
          <button
            className={button}
            disabled={locked || state !== "import-preview"}
            onClick={() =>
              void operate({ operation: "approve-import", id, revision, preview_digest: d.import_preview!.digest! })
            }
          >
            Approve exact guarded import
          </button>
        </>
      )}
      {d?.import_receipt && (
        <pre aria-label="Actual import receipt" className={evidence}>
          {JSON.stringify(d.import_receipt, null, 2)}
        </pre>
      )}
      <button className={button} disabled={locked || state !== "imported"} onClick={() => basic("preview-deploy")}>
        Preview complete post-import config
      </button>
      {d?.candidate && (
        <pre aria-label="Complete original config candidate" className={evidence}>
          {JSON.stringify(d.candidate, null, 2)}
        </pre>
      )}
      <button
        className={button}
        disabled={locked || state !== "candidate-preview" || !d?.candidate?.digest}
        onClick={() => void operate({ operation: "deploy", id, revision, candidate_digest: d!.candidate!.digest! })}
      >
        Deploy
      </button>
      {d?.publication_receipt && (
        <pre aria-label="Atomic publication acknowledgement" className={evidence}>
          {JSON.stringify(d.publication_receipt, null, 2)}
        </pre>
      )}
      <button
        className={button}
        disabled={locked || state !== "published-config"}
        onClick={() =>
          void operate({ operation: "materialize", id, revision, candidate_digest: d!.candidate!.digest! })
        }
      >
        Materialize approved pairs
      </button>
      {d?.materialize_receipt && (
        <pre aria-label="Revision-bound materialization receipt" className={evidence}>
          {JSON.stringify(d.materialize_receipt, null, 2)}
        </pre>
      )}
      <h3 className="text-sm font-semibold text-text-primary">Separate native plan approval</h3>
      <p className="text-xs text-text-secondary">
        CRA-931 owns admission and refresh, with a one-target attempt limit. Waiting-idle targets require a new explicit
        preview and approval, not an autonomous watcher. Unsupported MCP/codemode, Claude/Codex refresh, ACP, sandbox
        and old cores remain visible; no fresh-conversation fallback, local-grant discard, injection, force or
        restart-all. Adopted requires actual discovery, expected Lock/new run and the same conversation/cwd.
      </p>
      <button
        className={button}
        disabled={locked || !["materialized", "rollout-preview", "rollout-complete"].includes(state)}
        onClick={() => basic("preview-rollout")}
      >
        Preview or recheck native plan
      </button>
      {d?.rollouts.map((attempt, i) => (
        <pre key={i} aria-label={`Native rollout attempt ${i + 1}`} className={evidence}>
          {JSON.stringify(attempt, null, 2)}
        </pre>
      ))}
      <button
        className={button}
        disabled={locked || state !== "rollout-preview" || !native}
        onClick={() => void operate({ operation: "apply-rollout", id, revision, plan_digest: native!.plan_digest })}
      >
        Approve exact native plan (limit 1)
      </button>
      {["importing", "applying", "materializing", "rolling-out", "deploy-unknown"].includes(state) && (
        <button className={button} disabled={locked} onClick={() => basic("reconcile-deploy")}>
          Read-only reconcile deployment receipts
        </button>
      )}
    </div>
  );
}
