import { useCallback, useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { createSession } from "../lib/api";
import {
  registerPendingCreate,
  releasePendingCreate,
  resolvePendingCreate,
  retryBody,
  type PendingCreate,
} from "../lib/pendingCreates";
import type { SessionResponse } from "../lib/types";
import { TerminalView } from "./TerminalView";
import { setPendingTerminalFocus } from "../lib/terminalFocus";

export type CaptureAccount = "personal" | "work";

interface Props {
  account: CaptureAccount;
  onAccountChange: (account: CaptureAccount) => void;
  profile?: string;
  bootId?: string;
  onCreated: (session: SessionResponse) => void;
  onClose: () => void;
}

export function TailoredSessionsOverlay({ account, onAccountChange, profile, bootId, onCreated, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [composing, setComposing] = useState(false);
  const [idea, setIdea] = useState("");
  const [immediately, setImmediately] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [blocked, setBlocked] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [session, setSession] = useState<SessionResponse | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const focusIdea = useCallback((element: HTMLTextAreaElement | null) => element?.focus(), []);
  const pending = useRef<PendingCreate | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(false);
  const matches = "capture an idea".includes(query.trim().toLowerCase());

  useEffect(() => {
    mounted.current = true;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = document.getElementById("root");
    const wasInert = root?.inert ?? false;
    if (root) root.inert = true;
    search.current?.focus();
    return () => {
      mounted.current = false;
      if (root) root.inert = wasInert;
      if (pending.current) releasePendingCreate(pending.current.body.idempotency_key);
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  }, []);

  const capture = async (event: FormEvent) => {
    event.preventDefault();
    if (inFlight.current || (!pending.current && !idea.trim())) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    const retry = pending.current !== null;
    const attempt = pending.current ?? {
      body: {
        path: "",
        tool: "pi",
        scratch: true,
        view: "terminal" as const,
        title: "Capture an idea",
        group: "Tailored",
        profile,
        tailored: { preset: "capture-idea" as const, account, idea, immediately },
        idempotency_key: crypto.randomUUID(),
      },
      tool: "pi",
      since: Date.now(),
      origin: bootId ?? null,
    };
    pending.current = attempt;
    registerPendingCreate(attempt, { claimed: true });
    const result = await createSession(retry ? retryBody(attempt) : attempt.body);
    inFlight.current = false;
    if (!result.network) {
      resolvePendingCreate(attempt.body.idempotency_key);
      pending.current = null;
    }
    if (result.ok && result.session) onCreated(result.session);
    if (!mounted.current) {
      if (result.network) releasePendingCreate(attempt.body.idempotency_key);
      return;
    }
    setBusy(false);
    setUncertain(!!result.network || !!result.outcomeUnknown);
    setBlocked(!!result.outcomeUnknown);
    if (result.ok && result.session) {
      setPendingTerminalFocus("agent");
      setSession(result.session);
    } else setError(result.error ?? "Capture could not be created.");
  };

  const handleKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape" && !session) {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = panel.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]',
    );
    if (!controls?.length) return;
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return createPortal(
    <div data-tailored-overlay className="fixed inset-0 z-[80] flex items-center justify-center bg-surface-950/70 p-3">
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label="Tailored Sessions"
        onKeyDown={handleKey}
        className={`flex min-h-0 w-full flex-col rounded-lg border border-surface-700 bg-surface-900 shadow-xl ${session ? "h-[85dvh] max-w-6xl" : "max-w-2xl"}`}
      >
        <header className="flex items-center justify-between border-b border-surface-700 px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">
            Tailored Sessions{composing ? " · Capture an idea" : ""}
          </h2>
          <button
            type="button"
            aria-label="Close Tailored Sessions"
            onClick={onClose}
            className="h-8 rounded-md px-2 text-sm text-text-secondary hover:bg-surface-800"
          >
            Close · Ctrl+Y
          </button>
        </header>
        {session ? (
          <>
            <div className="flex min-h-0 flex-1 flex-col p-2">
              <TerminalView session={session} />
            </div>
            <p className="px-4 py-2 text-xs text-text-secondary">
              Closing this overlay leaves the Session running in Tailored. Escape goes to the agent.
            </p>
          </>
        ) : !composing ? (
          <div className="space-y-3 p-4">
            <input
              ref={search}
              aria-label="Find a tailored preset"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && matches) {
                  event.preventDefault();
                  setComposing(true);
                }
              }}
              placeholder="Find a preset…"
              className="h-9 w-full rounded-md border border-surface-700 bg-surface-950 px-3 text-sm text-text-primary"
            />
            {matches ? (
              <button
                type="button"
                onClick={() => setComposing(true)}
                className="w-full rounded-md border border-surface-700 px-3 py-2 text-left text-sm text-text-primary hover:bg-surface-800"
              >
                <span className="font-medium">Capture an idea</span>
                <span className="mt-1 block text-xs text-text-secondary">
                  Quick capture with inferred ticket, Atlas, and KB destinations.
                </span>
              </button>
            ) : (
              <p className="text-sm text-text-secondary">No matching presets.</p>
            )}
          </div>
        ) : (
          <form onSubmit={(event) => void capture(event)} className="space-y-4 p-4">
            <div className="flex flex-wrap items-center gap-4">
              <label className="flex items-center gap-2 text-sm text-text-secondary">
                Account
                <select
                  aria-label="Capture account"
                  value={account}
                  disabled={busy || uncertain}
                  onChange={(event) => onAccountChange(event.target.value as CaptureAccount)}
                  className="h-9 rounded-md border border-surface-700 bg-surface-950 px-2 text-text-primary"
                >
                  <option value="personal">Personal</option>
                  <option value="work">Work</option>
                </select>
              </label>
              <label className="flex items-center gap-2 text-sm text-text-primary">
                <input
                  type="checkbox"
                  checked={immediately}
                  disabled={busy || uncertain}
                  onChange={(event) => setImmediately(event.target.checked)}
                />
                Immediately
              </label>
            </div>
            <p className="text-xs text-text-secondary">
              {immediately
                ? "Apply inferred ticket, Atlas, and KB changes without a review step."
                : "Save the idea, then review inferred destinations before writing."}
            </p>
            <div className="text-sm text-text-secondary">
              <label htmlFor="tailored-idea">Idea</label>
              <textarea
                id="tailored-idea"
                ref={focusIdea}
                value={idea}
                disabled={busy || uncertain}
                maxLength={16384}
                onChange={(event) => setIdea(event.target.value)}
                rows={6}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    event.currentTarget.form?.requestSubmit();
                  }
                }}
                placeholder="What's the idea?"
                className="mt-1 w-full resize-y rounded-md border border-surface-700 bg-surface-950 p-3 text-sm text-text-primary"
              />
            </div>
            {error && (
              <p role="alert" className="text-sm text-status-error">
                {error}
              </p>
            )}
            {uncertain && (
              <p className="text-xs text-text-secondary">
                The outcome is not confirmed. Retry uses the same capture key. If you close, pending capture
                reconciliation continues in the background; check the Session list before launching again.
              </p>
            )}
            <div className="flex items-center justify-between gap-3">
              <span className="text-xs text-text-secondary">Enter capture · Shift+Enter newline</span>
              <button
                type="submit"
                disabled={busy || !idea.trim() || blocked}
                className="h-9 rounded-md border border-surface-600 bg-surface-800 px-4 text-sm font-medium text-text-primary hover:bg-surface-700 disabled:opacity-50"
              >
                {busy ? "Checking and launching…" : uncertain ? "Retry same capture" : "Capture idea"}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>,
    document.body,
  );
}
