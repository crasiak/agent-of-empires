import { useEffect, useRef, useState } from "react";
import { getResearch, getResearchOutput, replyResearch, researchOperation } from "../lib/api";
import type { ResearchDraft, ResearchInputs, ResearchOperation } from "../lib/types";

interface Props {
  account: "personal" | "work";
  readOnly: boolean;
  initial?: ResearchDraft;
  active?: boolean;
  onAccountChange?: (account: "personal" | "work") => void;
  onIdentity?: (id: string, persisted: boolean) => void;
}
const fieldClass = "mt-1 w-full rounded-md border border-surface-700 bg-surface-950 p-2 text-sm text-text-primary";
export function ResearchForm({ account, readOnly, initial, active = true, onAccountChange, onIdentity }: Props) {
  const [draft, setDraft] = useState(initial);
  const [id] = useState(() => initial?.id ?? crypto.randomUUID());
  const [original, setOriginal] = useState(initial?.original_prompt ?? "");
  const [inputs, setInputs] = useState<ResearchInputs>(
    initial?.inputs ?? {
      account,
      context: "",
      sources: [],
      developed_prompt: "",
      backend: null,
      workspace: "",
      permitted_scope: "",
      evaluation: "",
      unknowns_resolution: "",
      public_disclosure: "No private context in public queries",
      consequences_reviewed: false,
    },
  );
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [output, setOutput] = useState("");
  const [answer, setAnswer] = useState("");
  const pending = useRef(false);
  const form = useRef<HTMLDivElement>(null);
  const focus = useRef<HTMLElement | null>(null);
  const identity = useRef(onIdentity);
  useEffect(() => {
    identity.current?.(id, false);
  }, [id]);
  useEffect(() => {
    if (form.current && !form.current.closest("[hidden]"))
      (
        focus.current ?? form.current?.querySelector<HTMLElement>("textarea:not(:disabled), button:not(:disabled)")
      )?.focus();
  }, [active]);
  const locked = !!draft && ["running", "submitted", "uncertain", "developing"].includes(draft.state);
  const disabled = readOnly || busy || locked || uncertain;
  const update = <K extends keyof ResearchInputs>(key: K, value: ResearchInputs[K]) => {
    setInputs((old) => ({
      ...old,
      [key]: value,
      ...(key === "consequences_reviewed" ? {} : { consequences_reviewed: false }),
    }));
  };
  useEffect(() => {
    if (!active || !draft || draft.inputs.backend !== "evo" || !draft.receipt) return;
    let current = true;
    const refresh = async () => {
      const result = await getResearchOutput(draft.id);
      if (current)
        setOutput(
          result?.output ?? "Owned discovery pane stopped or unavailable. It will not be restarted or replayed.",
        );
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => {
      current = false;
      window.clearInterval(timer);
    };
  }, [draft, active]);
  const accept = (result: ResearchDraft) => {
    onIdentity?.(result.id, true);
    setDraft(result);
    setInputs(result.inputs);
    setOriginal(result.original_prompt);
  };
  const perform = async (operation: "save" | "develop" | "run") => {
    if (disabled || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const body: ResearchOperation = draft
        ? { operation: "edit", id, revision: draft.revision, inputs }
        : { operation: "create", id, original_prompt: original, inputs };
      const saved = await researchOperation(body);
      if (!saved.draft) {
        setError(saved.error ?? "Cannot save request");
        setUncertain(!!saved.uncertain);
        return;
      }
      accept(saved.draft);
      if (operation === "save") return;
      const result = await researchOperation({ operation, id, revision: saved.draft.revision });
      if (result.draft) accept(result.draft);
      else {
        setError(result.error ?? "Operation failed");
        setUncertain(!!result.uncertain);
      }
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  const refresh = async () => {
    const result = await getResearch(id);
    if (result) {
      accept(result.draft);
      setUncertain(false);
      setError(null);
    } else setError("Request unavailable; keep this identity and do not replace an uncertain run.");
  };
  const recoverDevelopment = async () => {
    if (readOnly || pending.current || !draft || draft.state !== "developing") return;
    pending.current = true;
    setBusy(true);
    const result = await researchOperation({ operation: "recover-development", id, revision: draft.revision });
    if (result.draft) accept(result.draft);
    else setError(result.error ?? "Cannot abandon development");
    pending.current = false;
    setBusy(false);
  };
  const sendAnswer = async () => {
    if (readOnly || pending.current || !draft || !answer.trim()) return;
    pending.current = true;
    setBusy(true);
    const result = await replyResearch(id, answer);
    if (result.ok) setAnswer("");
    else setError(result.message ?? "Reply outcome unknown; inspect output before sending again.");
    pending.current = false;
    setBusy(false);
  };
  return (
    <div
      ref={form}
      onFocusCapture={(event) => {
        focus.current = event.target;
      }}
      onKeyDown={(event) => {
        if (
          event.key === "Enter" &&
          (event.nativeEvent.isComposing || event.keyCode === 229 || event.repeat) &&
          event.target instanceof HTMLButtonElement
        )
          event.preventDefault();
      }}
      className="max-h-[75dvh] space-y-3 overflow-y-auto p-4"
    >
      <p className="text-xs text-text-secondary">
        Prompt development cannot start a backend. Source scopes are explicit constraints, not connected integrations.
        Run may start an account-isolated managed backend.
      </p>
      <p className="break-all font-mono text-xs text-text-secondary">
        Request {id} · {draft?.state ?? "draft"} · revision {draft?.revision ?? 0}
      </p>
      <label className="block text-sm text-text-secondary">
        Research account
        <select
          aria-label="Research account"
          className={fieldClass}
          value={inputs.account}
          disabled={disabled || !!draft}
          onChange={(e) =>
            onAccountChange
              ? onAccountChange(e.target.value as "personal" | "work")
              : update("account", e.target.value as "personal" | "work")
          }
        >
          <option value="personal">Personal</option>
          <option value="work">Work</option>
        </select>
      </label>
      <label className="block text-sm text-text-secondary">
        Original quick prompt
        <textarea
          aria-label="Original quick prompt"
          className={fieldClass}
          rows={3}
          value={original}
          disabled={disabled || !!draft}
          onChange={(e) => setOriginal(e.target.value)}
          maxLength={16384}
        />
      </label>
      <label className="block text-sm text-text-secondary">
        Explicit context
        <textarea
          aria-label="Explicit context"
          className={fieldClass}
          rows={2}
          value={inputs.context}
          disabled={disabled}
          onChange={(e) => update("context", e.target.value)}
        />
      </label>
      <fieldset disabled={disabled} className="space-y-2">
        <legend className="text-sm text-text-secondary">Source boundaries (no crawling)</legend>
        <p className="text-xs text-text-secondary">
          Personal: Grafana / OTel / Zulip. Work: Slack. Specify endpoints/container scope and supply relevant context
          yourself. These category additions do not reclassify existing Work observability tenants. Use Supplied for
          other explicitly provided, account-appropriate context; no crawling is implied.
        </p>
        {inputs.sources.map((source, index) => (
          <div key={index} className="flex gap-2">
            <select
              aria-label={`Source ${index + 1} category`}
              className={fieldClass}
              value={source.category}
              onChange={(e) =>
                update(
                  "sources",
                  inputs.sources.map((s, i) =>
                    i === index ? { ...s, category: e.target.value as typeof s.category } : s,
                  ),
                )
              }
            >
              {["supplied", "public", "grafana", "otel", "zulip", "slack"].map((category) => (
                <option
                  key={category}
                  disabled={
                    category === "slack"
                      ? inputs.account !== "work"
                      : ["grafana", "otel", "zulip"].includes(category) && inputs.account !== "personal"
                  }
                >
                  {category}
                </option>
              ))}
            </select>
            <input
              aria-label={`Source ${index + 1} scope`}
              className={fieldClass}
              value={source.scope}
              onChange={(e) =>
                update(
                  "sources",
                  inputs.sources.map((s, i) => (i === index ? { ...s, scope: e.target.value } : s)),
                )
              }
            />
            <button
              type="button"
              onClick={() =>
                update(
                  "sources",
                  inputs.sources.filter((_, i) => i !== index),
                )
              }
            >
              Remove
            </button>
          </div>
        ))}
        <button
          type="button"
          onClick={() => update("sources", [...inputs.sources, { category: "supplied", scope: "" }])}
          className="h-8 rounded-md border border-surface-700 px-2 text-xs text-text-primary"
        >
          Add source boundary
        </button>
      </fieldset>
      {draft?.recommendation && (
        <p className="text-sm text-text-primary">
          Recommendation: {draft.recommendation.toUpperCase()} · {draft.reason}. You choose or override below.
        </p>
      )}
      <label className="block text-sm text-text-secondary">
        Backend
        <select
          aria-label="Backend"
          className={fieldClass}
          value={inputs.backend ?? ""}
          disabled={disabled}
          onChange={(e) => update("backend", (e.target.value || null) as ResearchInputs["backend"])}
        >
          <option value="">Choose after development</option>
          <option value="orx">ORX research</option>
          <option value="evo">EVO discovery + baseline only</option>
        </select>
      </label>
      <label className="block text-sm text-text-secondary">
        Developed prompt
        <textarea
          aria-label="Developed prompt"
          className={fieldClass}
          rows={6}
          value={inputs.developed_prompt}
          disabled={disabled}
          onChange={(e) => update("developed_prompt", e.target.value)}
        />
      </label>
      {(
        [
          ["workspace", "Workspace (absolute path)"],
          ["permitted_scope", "Permitted scope"],
          ["evaluation", "Evaluation criteria"],
          ["public_disclosure", "Public disclosure limits"],
        ] as const
      ).map(([key, label]) => (
        <label key={key} className="block text-sm text-text-secondary">
          {label}
          <input
            aria-label={label}
            className={fieldClass}
            disabled={disabled}
            value={inputs[key]}
            onChange={(e) => update(key, e.target.value)}
          />
        </label>
      ))}
      {!!draft?.unknowns.length && (
        <div className="text-sm text-status-waiting">
          <p>Consequential unknowns</p>
          <ul>
            {draft.unknowns.map((unknown, i) => (
              <li key={i}>{unknown}</li>
            ))}
          </ul>
        </div>
      )}
      <label className="block text-sm text-text-secondary">
        Unknowns resolution
        <textarea
          aria-label="Unknowns resolution"
          className={fieldClass}
          disabled={disabled}
          value={inputs.unknowns_resolution}
          onChange={(e) => update("unknowns_resolution", e.target.value)}
        />
      </label>
      <p className="text-xs text-text-secondary">
        EVO explores the repository, initializes .evo, creates a baseline worktree and runs the first experiment. Run
        does not authorize optimize, ship, merge or push. Optimization needs subsequent explicit review; its budget is
        per-branch serial experiment depth, not dollars. ORX can execute shell-capable research and warms project
        starter prompts during startup.
      </p>
      <label className="flex gap-2 text-sm text-text-primary">
        <input
          type="checkbox"
          checked={inputs.consequences_reviewed}
          disabled={disabled}
          onChange={(e) => update("consequences_reviewed", e.target.checked)}
        />
        I reviewed the exact developed prompt, account/source boundaries, workspace, scope, unknowns and consequences.
      </label>
      {(error || draft?.error) && (
        <p role="alert" className="text-sm text-status-error">
          {error ?? draft?.error}
        </p>
      )}
      {draft?.state === "developing" && (
        <button
          type="button"
          disabled={readOnly || busy}
          onClick={() => void recoverDevelopment()}
          className="h-9 rounded-md border border-surface-600 px-3 text-sm text-text-primary disabled:opacity-50"
        >
          Abandon interrupted development
        </button>
      )}
      {draft?.receipt && inputs.backend === "orx" && (
        <p className="text-xs text-text-secondary">
          ORX address is daemon-local, not this browser's host. Remote access requires existing port forwarding. Use the
          retained project/chat identity. Startup includes workspace inspection/setup and auxiliary project-starter
          model calls; source/privacy limits are instructions, not an OS sandbox.
        </p>
      )}
      {locked && (
        <p className="text-xs text-text-secondary">
          No automatic retries or restart. Submitted means handoff, not completion. A running or uncertain state after
          interruption needs inspection, not a replacement run.
        </p>
      )}
      {draft?.receipt && (
        <pre className="whitespace-pre-wrap break-all text-xs text-text-secondary">
          {JSON.stringify(draft.receipt, null, 2)}
        </pre>
      )}
      {!locked && (
        <div className="sticky bottom-0 z-10 flex flex-wrap gap-2 border-t border-surface-700 bg-surface-900 py-3">
          {(
            [
              ["save", "Save draft"],
              ["develop", "Develop prompt"],
              ["run", "Run reviewed revision"],
            ] as const
          ).map(([op, label]) => (
            <button
              key={op}
              type="button"
              disabled={
                disabled ||
                !original.trim() ||
                (op === "run" && (!inputs.backend || !inputs.consequences_reviewed || draft?.state !== "review"))
              }
              onClick={() => void perform(op)}
              className="h-9 rounded-md border border-surface-600 bg-surface-800 px-3 text-sm text-text-primary disabled:opacity-50"
            >
              {busy ? "Working…" : label}
            </button>
          ))}
        </div>
      )}
      {(draft || uncertain) && (
        <button
          type="button"
          disabled={busy}
          onClick={() => void refresh()}
          className="h-8 rounded-md px-2 text-xs text-text-secondary"
        >
          Refresh existing request
        </button>
      )}
      {draft?.inputs.backend === "evo" && draft.receipt && (
        <section className="space-y-2">
          <h3 className="text-sm text-text-primary">Owned EVO discovery / baseline pane</h3>
          <pre
            aria-label="Discovery output"
            className="max-h-64 overflow-auto whitespace-pre-wrap bg-surface-950 p-2 text-xs text-text-primary"
          >
            {output}
          </pre>
          <label className="block text-sm text-text-secondary">
            Literal discovery answer
            <textarea
              aria-label="Literal discovery answer"
              className={fieldClass}
              value={answer}
              disabled={readOnly || busy}
              onChange={(e) => setAnswer(e.target.value)}
            />
          </label>
          <button
            type="button"
            disabled={readOnly || busy || !answer.trim()}
            onClick={() => void sendAnswer()}
            className="h-9 rounded-md border border-surface-600 px-3 text-sm text-text-primary disabled:opacity-50"
          >
            Send discovery answer
          </button>
          <p className="text-xs text-text-secondary">
            Answers go only to the existing owned pane. Optimization is not supported here. A missing pane is never
            recreated.
          </p>
        </section>
      )}
    </div>
  );
}
