import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { createSession } from "../lib/api";
import {
  registerPendingCreate,
  observePendingCreate,
  releasePendingCreate,
  resolvePendingCreate,
  retryBody,
  isPendingCreateExpired,
  PENDING_CREATE_EXPIRED_MESSAGE,
  type PendingCreate,
  type PendingCreateOutcome,
} from "../lib/pendingCreates";
import type { CreateSessionRequest, SessionResponse } from "../lib/types";
import type { CaptureAccount } from "./TailoredSessionsOverlay";

interface Props {
  account: CaptureAccount;
  documentChat: boolean;
  active: boolean;
  onAccountChange: (account: CaptureAccount) => void;
  profile?: string;
  bootId?: string;
  readOnly: boolean;
  onCreated: (session: SessionResponse) => void;
}
export function TailoredCaptureForm({
  account,
  documentChat,
  active,
  onAccountChange,
  profile,
  bootId,
  readOnly,
  onCreated,
}: Props) {
  const [idea, setIdea] = useState("");
  const [url, setUrl] = useState("");
  const [question, setQuestion] = useState("");
  const [immediately, setImmediately] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [blocked, setBlocked] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef<PendingCreate | null>(null);
  const stopObserving = useRef<(() => void) | null>(null);
  const inFlight = useRef(false);
  const form = useRef<HTMLFormElement>(null);
  const lastFocus = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (!form.current || form.current.closest("[hidden]")) return;
    if (lastFocus.current?.isConnected) lastFocus.current.focus();
    else form.current?.querySelector<HTMLElement>("textarea:not(:disabled), button:not(:disabled)")?.focus();
  }, [active]);
  useLayoutEffect(() => {
    if (!pending.current || inFlight.current || blocked || active) return;
    releasePendingCreate(pending.current.body.idempotency_key);
  }, [active, busy, blocked]);
  useEffect(
    () => () => {
      stopObserving.current?.();
      if (pending.current) releasePendingCreate(pending.current.body.idempotency_key);
    },
    [],
  );
  const settle = (outcome: PendingCreateOutcome) => {
    pending.current = null;
    stopObserving.current?.();
    stopObserving.current = null;
    setBusy(false);
    setUncertain(outcome.status === "unknown");
    setBlocked(
      outcome.status === "unknown" ||
        (outcome.status === "failed" && outcome.message === PENDING_CREATE_EXPIRED_MESSAGE),
    );
    setError(outcome.status === "created" ? null : outcome.message);
    if (outcome.status === "created") {
      setIdea("");
      setUrl("");
      setQuestion("");
      setImmediately(false);
    }
  };
  const capture = async (event: FormEvent) => {
    event.preventDefault();
    if (readOnly || blocked || inFlight.current || (!pending.current && !(documentChat ? url : idea).trim())) return;
    if (pending.current && isPendingCreateExpired(pending.current.since)) {
      setBlocked(true);
      setError(PENDING_CREATE_EXPIRED_MESSAGE);
      return;
    }
    inFlight.current = true;
    setBusy(true);
    setError(null);
    const retry = pending.current !== null;
    const tailored: CreateSessionRequest["tailored"] = documentChat
      ? { preset: "document-chat", account, document: { url, opening_question: question } }
      : { preset: "capture-idea", account, idea, immediately };
    const attempt = pending.current ?? {
      body: {
        path: "",
        tool: "pi",
        scratch: true,
        view: "terminal" as const,
        title: documentChat ? "Document chat" : "Capture an idea",
        group: "Tailored",
        profile,
        tailored,
        idempotency_key: crypto.randomUUID(),
      },
      tool: "pi",
      since: Date.now(),
      origin: bootId ?? null,
    };
    pending.current = attempt;
    if (!retry) {
      stopObserving.current = observePendingCreate(attempt.body.idempotency_key, (outcome) => {
        if (pending.current?.body.idempotency_key === attempt.body.idempotency_key) settle(outcome);
      });
    }
    registerPendingCreate(attempt, { claimed: true });
    const result = await createSession(retry ? retryBody(attempt) : attempt.body);
    inFlight.current = false;
    if (result.network) {
      setBusy(false);
      setUncertain(true);
      setError(result.error ?? "Capture could not be created.");
      return;
    }
    resolvePendingCreate(attempt.body.idempotency_key);
    settle(
      result.ok
        ? { status: "created", session: result.session }
        : {
            status: result.outcomeUnknown ? "unknown" : "failed",
            message: result.error ?? "Capture could not be created.",
          },
    );
    if (result.ok && result.session) onCreated(result.session);
  };
  const input =
    "w-full resize-y whitespace-pre-wrap rounded-md border border-surface-700 bg-surface-950 p-3 text-sm text-text-primary [overflow-wrap:anywhere]";
  return (
    <form
      ref={form}
      onFocusCapture={(event) => {
        lastFocus.current = event.target;
      }}
      onSubmit={(event) => void capture(event)}
      className="space-y-4 p-4"
      onKeyDown={(event) => {
        if (event.key !== "Enter") return;
        if (event.nativeEvent.isComposing || event.keyCode === 229 || event.repeat) {
          if (event.ctrlKey || event.metaKey || event.target instanceof HTMLButtonElement) event.preventDefault();
          return;
        }
        if (event.ctrlKey !== event.metaKey && !event.shiftKey && !event.altKey) {
          event.preventDefault();
          event.currentTarget.requestSubmit();
        } else if (
          event.target instanceof HTMLInputElement ||
          event.target instanceof HTMLSelectElement ||
          (event.target instanceof HTMLButtonElement &&
            (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey))
        )
          event.preventDefault();
      }}
    >
      <div className="flex flex-wrap items-center gap-4">
        <label className="flex items-center gap-2 text-sm text-text-secondary">
          Account
          <select
            aria-label={documentChat ? "Document account" : "Capture account"}
            value={account}
            disabled={busy || uncertain}
            onChange={(event) => onAccountChange(event.target.value as CaptureAccount)}
            className="h-9 rounded-md border border-surface-700 bg-surface-950 px-2 text-text-primary"
          >
            <option value="personal">Personal</option>
            <option value="work">Work</option>
          </select>
        </label>
        {!documentChat && (
          <label className="flex items-center gap-2 text-sm text-text-primary">
            <input
              type="checkbox"
              checked={immediately}
              disabled={busy || uncertain}
              onChange={(event) => setImmediately(event.target.checked)}
            />
            Immediately
          </label>
        )}
      </div>
      <p className="text-xs text-text-secondary">
        {documentChat
          ? "Public direct Markdown/PDF only. Source is downloaded and extracted before a tool-free conversation starts. No credentials, redirects or OCR. 10 MiB raw / 256 KiB text; oversize documents are refused."
          : immediately
            ? "Apply inferred ticket, Atlas, and KB changes without a review step."
            : "Save the idea, then review inferred destinations before writing."}
      </p>
      {documentChat ? (
        <>
          <label className="block text-sm text-text-secondary">
            Public document URL
            <textarea
              aria-label="Public document URL"
              value={url}
              disabled={busy || uncertain}
              maxLength={8192}
              rows={3}
              onChange={(event) => setUrl(event.target.value)}
              className={input}
            />
          </label>
          <label className="block text-sm text-text-secondary">
            Opening question (optional)
            <textarea
              aria-label="Opening question (optional)"
              value={question}
              disabled={busy || uncertain}
              maxLength={4096}
              rows={3}
              onChange={(event) => setQuestion(event.target.value)}
              className={input}
            />
          </label>
        </>
      ) : (
        <label className="block text-sm text-text-secondary">
          Idea
          <textarea
            aria-label="Idea"
            value={idea}
            disabled={busy || uncertain}
            maxLength={16384}
            rows={6}
            onChange={(event) => setIdea(event.target.value)}
            placeholder="What's the idea?"
            className={input}
          />
        </label>
      )}
      {error && (
        <p role="alert" className="text-sm text-status-error">
          {error}
        </p>
      )}
      {uncertain && (
        <p className="text-xs text-text-secondary">
          The outcome is not confirmed. Retry uses the same create key. If you close, pending creation reconciliation
          continues in the background; check the Session list before launching again.
        </p>
      )}
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs text-text-secondary">
          Enter newline · Ctrl/Meta+Enter {documentChat ? "Start" : "capture"}
        </span>
        <button
          type="submit"
          disabled={readOnly || busy || !(documentChat ? url : idea).trim() || blocked}
          className="h-9 rounded-md border border-surface-600 bg-surface-800 px-4 text-sm font-medium text-text-primary hover:bg-surface-700 disabled:opacity-50"
        >
          {busy
            ? "Checking and launching…"
            : uncertain
              ? "Retry same capture"
              : documentChat
                ? "Start"
                : "Capture idea"}
        </button>
      </div>
    </form>
  );
}
