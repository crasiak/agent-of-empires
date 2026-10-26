import { useEffect, useRef, useState } from "react";
import { isWorkshopRequest } from "../lib/workshop";
import { WorkshopWorkflow } from "./WorkshopWorkflow";
import { getWorkshop, getWorkshopDefaults, workshopOperation } from "../lib/api";
import type { WorkshopBundle, WorkshopEnvelope, WorkshopOperation, WorkshopRequest } from "../lib/types";

interface Props {
  account: "personal" | "work";
  readOnly: boolean;
  initial?: WorkshopRequest;
}
const control =
  "w-full rounded-md border border-surface-700 bg-surface-950 p-2 text-sm text-text-primary disabled:opacity-50";
const button =
  "h-9 rounded-md border border-surface-700 px-3 text-xs text-text-primary hover:bg-surface-800 disabled:opacity-50";
export function WorkshopForm({ account, readOnly, initial }: Props) {
  const [id] = useState(() => initial?.id ?? crypto.randomUUID());
  const [request, setRequest] = useState(initial);
  const [envelope, setEnvelope] = useState<WorkshopEnvelope>(
    initial?.envelope ?? { account, idea: "", repository: "", slug: "", source_scope: "" },
  );
  const [files, setFiles] = useState(JSON.stringify(initial?.bundle ?? { files: {} }, null, 2));
  const [message, setMessage] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [pendingCreate, setPendingCreate] = useState<Extract<WorkshopOperation, { operation: "create" }> | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    if (!initial)
      void getWorkshopDefaults().then((result) => {
        if (mounted.current && result?.repository)
          setEnvelope((value) => (value.repository ? value : { ...value, repository: result.repository! }));
      });
    return () => {
      mounted.current = false;
    };
  }, [initial]);
  const accept = (value: WorkshopRequest) => {
    if (!isWorkshopRequest(value, id, request?.envelope ?? pendingCreate?.envelope ?? envelope)) {
      setUncertain(true);
      setError(
        "Outcome unknown: invalid Workshop identity/envelope. Keep this UUID and exact creation envelope; reopen only.",
      );
      return;
    }
    setRequest(value);
    setEnvelope(value.envelope);
    setFiles(JSON.stringify(value.bundle ?? { files: {} }, null, 2));
    setError(value.error);
    setUncertain(false);
    setPendingCreate(null);
  };
  const operate = async (operation: WorkshopOperation) => {
    if (readOnly || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    const result = await workshopOperation(operation);
    inFlight.current = false;
    if (!mounted.current) return;
    setBusy(false);
    if (result.request) accept(result.request);
    else {
      setError(result.error ?? "Workshop operation refused");
      const unresolved = uncertain || !!result.uncertain;
      setUncertain(unresolved);
      if (!unresolved) setPendingCreate(null);
    }
  };
  const reopen = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    const result = await getWorkshop(id);
    inFlight.current = false;
    if (!mounted.current) return;
    setBusy(false);
    if (result) accept(result.request);
    else {
      setUncertain(true);
      setError(
        "Request not confirmed. Retain this UUID and exact creation envelope; do not replace an unknown operation.",
      );
    }
  };
  const locked = busy || readOnly || uncertain;
  const editable = !locked && (!request || ["draft", "review", "failed"].includes(request.state));
  const savedFiles = JSON.stringify(request?.bundle ?? { files: {} }, null, 2);
  const dirty = files !== savedFiles;
  const revision = request?.revision ?? 0;
  const basic = (operation: "review" | "reconcile" | "recover-refinement") => ({ operation, id: id, revision });
  const saveFiles = () => {
    try {
      const bundle = JSON.parse(files) as WorkshopBundle;
      void operate({ operation: "propose", id: id, revision, bundle });
    } catch {
      setError("Enter a JSON bundle object with a files map. The host validates all paths and sizes.");
    }
  };
  return (
    <section aria-label="Skill workshop" className="max-h-[75dvh] space-y-3 overflow-auto p-4">
      <p className="text-xs text-text-secondary">
        Model output is data, never approval. Review, local Commit, destination Publish, Land and Deploy are separate. A
        local commit or pending merge is not landed or deployed.
      </p>
      <p className="break-all font-mono text-xs text-text-secondary">
        {id} · revision {revision} · {request?.state ?? "new"}
      </p>
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="text-xs text-text-secondary">
          Workshop account
          <select
            aria-label="Workshop account"
            className={control}
            value={envelope.account}
            disabled={!!request || locked || !!pendingCreate}
            onChange={(e) => setEnvelope({ ...envelope, account: e.target.value as WorkshopEnvelope["account"] })}
          >
            <option value="personal">Personal</option>
            <option value="work">Work</option>
          </select>
        </label>
        <label className="text-xs text-text-secondary">
          Skill slug
          <input
            aria-label="Skill slug"
            className={control}
            value={envelope.slug}
            disabled={!!request || locked || !!pendingCreate}
            maxLength={64}
            onChange={(e) => setEnvelope({ ...envelope, slug: e.target.value })}
          />
        </label>
      </div>
      <label className="block text-xs text-text-secondary">
        Source repository
        <input
          aria-label="Source repository"
          className={control}
          value={envelope.repository}
          disabled={!!request || locked || !!pendingCreate}
          onChange={(e) => setEnvelope({ ...envelope, repository: e.target.value })}
        />
      </label>
      <p className="text-xs text-text-secondary">
        Versioned .agents/skills/{envelope.slug || "<slug>"}. Choose an existing canonical absolute repository path; the
        local-only setting supplies the default. No personal path is guessed.
      </p>
      <label className="block text-xs text-text-secondary">
        Account/source suitability
        <textarea
          aria-label="Account/source suitability"
          className={control}
          value={envelope.source_scope}
          disabled={!!request || locked || !!pendingCreate}
          maxLength={4096}
          onChange={(e) => setEnvelope({ ...envelope, source_scope: e.target.value })}
        />
      </label>
      <label className="block text-xs text-text-secondary">
        Skill idea
        <textarea
          aria-label="Skill idea"
          className={control}
          value={envelope.idea}
          disabled={!!request || locked || !!pendingCreate}
          maxLength={16384}
          onChange={(e) => setEnvelope({ ...envelope, idea: e.target.value })}
        />
      </label>
      {!request && (
        <button
          type="button"
          className={button}
          disabled={locked || !!pendingCreate}
          onClick={() => {
            const body = { operation: "create" as const, id: id, envelope: { ...envelope } };
            setPendingCreate(body);
            void operate(body);
          }}
        >
          Create Workshop
        </button>
      )}
      {pendingCreate && uncertain && (
        <button
          type="button"
          className={button}
          disabled={busy || readOnly}
          onClick={() => void operate(pendingCreate!)}
        >
          Retry exact keyed create
        </button>
      )}
      {request?.turns.map((turn, index) => (
        <div key={index} className="border-l border-surface-700 pl-3 text-sm text-text-primary">
          <p className="whitespace-pre-wrap">You: {turn.user}</p>
          <p className="whitespace-pre-wrap">Proposal: {turn.proposal?.reply ?? "No accepted model result"}</p>
        </div>
      ))}
      <label className="block text-xs text-text-secondary">
        Refinement message
        <textarea
          aria-label="Refinement message"
          className={control}
          value={message}
          disabled={!request || !editable}
          maxLength={8192}
          onChange={(e) => setMessage(e.target.value)}
        />
      </label>
      <button
        type="button"
        className={button}
        disabled={!request || !editable || !message.trim() || dirty}
        onClick={() => void operate({ operation: "refine", id: id, revision, message })}
      >
        Refine without tools
      </button>
      <label className="block text-xs text-text-secondary">
        Proposed bundle JSON
        <textarea
          aria-label="Proposed bundle JSON"
          className={`${control} font-mono`}
          rows={7}
          value={files}
          disabled={!request || !editable}
          onChange={(e) => setFiles(e.target.value)}
        />
      </label>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={button} disabled={!request || !editable} onClick={saveFiles}>
          Save proposed files
        </button>
        <button
          type="button"
          className={button}
          disabled={!request?.bundle || !editable || dirty}
          onClick={() => void operate(basic("review"))}
        >
          Review exact source
        </button>
      </div>
      {dirty && (
        <p className="text-xs text-status-warning">
          Save the proposed files before refinement or review. Edits invalidate any prior approval.
        </p>
      )}
      {request?.review && (
        <div className="space-y-2 text-xs text-text-secondary">
          <h3 className="font-semibold text-text-primary">Exact host review</h3>
          <p className="break-all">
            Repository {request.review.repository} · Base {request.review.base_commit} · Source tree{" "}
            {request.review.source_tree} · Skill tree {request.review.skill_tree} · Review digest{" "}
            {request.review.digest}
          </p>
          {request.review.evidence.map((line) => (
            <p key={line}>{line}</p>
          ))}
          <pre
            aria-label="Exact skill file diff"
            className="max-h-72 overflow-auto whitespace-pre border border-surface-700 p-2 text-text-primary"
          >
            {request.review.diff}
          </pre>
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={button}
          disabled={locked || request?.state !== "review" || !request.review || dirty}
          onClick={() =>
            request?.review &&
            void operate({ operation: "commit", id: id, revision, review_digest: request.review.digest })
          }
        >
          Approve reviewed local commit
        </button>
      </div>
      {request && <WorkshopWorkflow request={request} locked={locked} operate={operate} />}
      {request?.commit && (
        <p className="break-all font-mono text-xs text-text-primary">
          Local source commit {request.commit.commit_oid} · {request.commit.branch} · {request.commit.worktree}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <button type="button" className={button} disabled={busy} onClick={() => void reopen()}>
          Reopen same request
        </button>
        <button
          type="button"
          className={button}
          disabled={
            locked || !request || !["committing", "committed", "uncertain", "reviewing"].includes(request.state)
          }
          onClick={() => void operate(basic("reconcile"))}
        >
          Reconcile retained intent
        </button>
        {request?.state === "refining" && (
          <button
            type="button"
            className={button}
            disabled={locked}
            onClick={() => void operate(basic("recover-refinement"))}
          >
            Abandon pending refinement
          </button>
        )}
      </div>
      {busy && (
        <p role="status" className="text-xs text-text-secondary">
          Host operation pending. Closing leaves the same request recoverable in history.
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-status-error">
          {error}
        </p>
      )}
    </section>
  );
}
