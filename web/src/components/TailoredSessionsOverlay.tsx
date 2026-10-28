import { useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { useLatestRef } from "../hooks/useLatestRef";
import { createPortal } from "react-dom";
import type { ResearchDraft, SessionResponse, WorkshopRequest } from "../lib/types";
import { WorkshopForm } from "./WorkshopForm";
import { ResearchForm } from "./ResearchForm";
import { TailoredCaptureForm } from "./TailoredCaptureForm";
import { TerminalView } from "./TerminalView";
import { TailoredHistoryPanel } from "./TailoredHistoryPanel";
import { setPendingTerminalFocus } from "../lib/terminalFocus";

export type CaptureAccount = "personal" | "work";
interface Props {
  open?: boolean;
  account: CaptureAccount;
  onAccountChange: (account: CaptureAccount) => void;
  profile?: string;
  bootId?: string;
  readOnly?: boolean;
  onCreated: (session: SessionResponse) => void;
  onClose: () => void;
}
type Editor = { scope: string; requestId?: string; newSlot?: boolean } & (
  | { key: string; kind: "capture" | "document"; account: CaptureAccount }
  | { key: string; kind: "research"; account: CaptureAccount; initial?: ResearchDraft }
  | { key: string; kind: "workshop"; account: CaptureAccount; initial?: WorkshopRequest }
);

export function TailoredSessionsOverlay({
  open = true,
  account,
  onAccountChange,
  profile,
  bootId,
  readOnly = false,
  onCreated,
  onClose,
}: Props) {
  const [editors, setEditors] = useState<Editor[]>([]);
  const [selections, setSelections] = useState<Record<string, string | null>>({});
  const [sessions, setSessions] = useState<Record<string, SessionResponse | null>>({});
  const [accounts, setAccounts] = useState<Record<string, CaptureAccount>>({});
  const scope = profile ?? "";
  const selected = selections[scope] ?? null;
  const session = sessions[scope] ?? null;
  const preset = editors.find((editor) => editor.key === selected)?.kind;
  const heading = preset === "capture" ? " · Capture an idea" : preset === "document" ? " · Document chat" : "";
  const setSelected = (value: string | null) => setSelections((old) => ({ ...old, [scope]: value }));
  const setSession = (value: SessionResponse | null) => setSessions((old) => ({ ...old, [scope]: value }));
  const selection = useLatestRef(selected);
  const panel = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const lastFocus = useRef<HTMLElement | null>(null);
  const choose = (editor: Editor) => {
    const id = editor.kind === "research" || editor.kind === "workshop" ? editor.initial?.id : undefined;
    const retained = editors.find((e) => e.scope === scope && e.kind === editor.kind && id && e.requestId === id);
    setEditors((old) =>
      retained || old.some((e) => e.key === editor.key) ? old : [...old, { ...editor, requestId: id }],
    );
    setSelected(retained?.key ?? editor.key);
    setSession(null);
  };
  const newEditor = (kind: Editor["kind"], choice?: CaptureAccount) => {
    if (readOnly) return;
    const accountKey = JSON.stringify([scope, kind]);
    const nextAccount = choice ?? accounts[accountKey] ?? account;
    setAccounts((old) => ({ ...old, [accountKey]: nextAccount }));
    onAccountChange(nextAccount);
    const retained = editors.find(
      (e) => e.scope === scope && e.kind === kind && e.account === nextAccount && e.newSlot,
    );
    choose(retained ?? { scope, key: crypto.randomUUID(), kind, account: nextAccount, newSlot: true });
  };
  const showHistory = () => {
    setSelected(null);
    setSession(null);
  };
  useLayoutEffect(() => {
    if (!open) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const previousAgentInput = !!previous?.closest('[data-term="agent"]');
    const root = document.getElementById("root");
    const wasInert = root?.inert ?? false;
    if (root) root.inert = true;
    if (lastFocus.current?.isConnected && !lastFocus.current.closest("[hidden]")) lastFocus.current.focus();
    else
      panel.current
        ?.querySelector<HTMLElement>(
          "[data-editor]:not([hidden]) textarea:not(:disabled), input:not(:disabled), button:not(:disabled)",
        )
        ?.focus();
    return () => {
      lastFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      if (root) root.inert = wasInert;
      if (previous?.isConnected) previous.focus();
      else if (previousAgentInput) root?.querySelector<HTMLElement>('[data-term="agent"] textarea')?.focus();
    };
  }, [open]);
  useLayoutEffect(() => {
    if (open && !selected && !session) search.current?.focus();
  }, [open, selected, session]);
  const key = (event: KeyboardEvent<HTMLDivElement>) => {
    event.stopPropagation();
    if (event.nativeEvent.isComposing || event.keyCode === 229) {
      if (event.key === "Enter") event.preventDefault();
      return;
    }
    if (
      event.key === "Enter" &&
      event.target instanceof HTMLButtonElement &&
      (event.repeat ||
        (!event.target.closest("form") && (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey)))
    ) {
      event.preventDefault();
      return;
    }
    if (!event.repeat && event.key === "Escape" && !session) {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (!event.repeat && event.key === "F2") {
      event.preventDefault();
      event.stopPropagation();
      showHistory();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = Array.from(
      panel.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]',
      ) ?? [],
    ).filter((element) => !element.closest("[hidden], [inert]"));
    const first = controls[0],
      last = controls[controls.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  };
  return createPortal(
    <div
      hidden={!open}
      inert={!open}
      data-tailored-overlay={open ? "" : undefined}
      className={open ? "fixed inset-0 z-[80] flex items-center justify-center bg-surface-950/70 p-3" : "hidden"}
    >
      <div
        ref={panel}
        role={open ? "dialog" : undefined}
        aria-modal={open ? true : undefined}
        aria-label="Tailored Sessions"
        onKeyDown={key}
        className={`flex min-h-0 w-full flex-col rounded-lg border border-surface-700 bg-surface-900 shadow-xl ${session ? "h-[85dvh] max-w-6xl" : "max-w-2xl"}`}
      >
        <header className="flex items-center justify-between border-b border-surface-700 px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">{session?.title ?? `Tailored Sessions${heading}`}</h2>
          <div className="flex items-center gap-2">
            {(selected || session) && (
              <button
                type="button"
                onClick={showHistory}
                className="h-8 rounded-md px-2 text-xs text-text-secondary hover:bg-surface-800"
              >
                Back to history · F2
              </button>
            )}
            <button
              type="button"
              aria-label="Close Tailored Sessions"
              onClick={onClose}
              className="h-8 rounded-md px-2 text-sm text-text-secondary hover:bg-surface-800"
            >
              Close · Ctrl+Y outside editors
            </button>
          </div>
        </header>
        {open && session && (
          <>
            <div className="flex min-h-0 flex-1 flex-col p-2">
              <TerminalView session={session} />
            </div>
            <p className="px-4 py-2 text-xs text-text-secondary">
              F2 returns to history. Closing leaves the Session running. Escape goes to the agent.
            </p>
          </>
        )}
        {open && !session && !selected && (
          <TailoredHistoryPanel
            searchRef={search}
            readOnly={readOnly}
            onNew={() => newEditor("capture")}
            onDocument={() => newEditor("document")}
            onWorkshop={(initial) =>
              initial
                ? choose({
                    scope,
                    key: JSON.stringify([scope, "workshop", initial.id]),
                    kind: "workshop",
                    account: initial.envelope.account,
                    initial,
                  })
                : newEditor("workshop")
            }
            onResearch={(initial) =>
              initial
                ? choose({
                    scope,
                    key: JSON.stringify([scope, "research", initial.id]),
                    kind: "research",
                    account: initial.inputs.account,
                    initial,
                  })
                : newEditor("research")
            }
            onOpen={(existing) => {
              if (!readOnly) {
                setPendingTerminalFocus("agent");
                setSession(existing);
              }
            }}
          />
        )}
        {editors.map((editor) => {
          const active = open && scope === editor.scope && selected === editor.key && !session;
          const changeAccount = (next: CaptureAccount) => newEditor(editor.kind, next);
          const identify = (id: string, persisted: boolean) =>
            setEditors((old) =>
              old.map((e) =>
                e.key === editor.key ? { ...e, requestId: id, ...(persisted ? { newSlot: false } : {}) } : e,
              ),
            );
          return (
            <div key={editor.key} data-editor hidden={!active} inert={!active}>
              {editor.kind === "research" ? (
                <ResearchForm
                  account={editor.account}
                  initial={editor.initial}
                  onIdentity={identify}
                  active={active}
                  onAccountChange={changeAccount}
                  readOnly={readOnly}
                />
              ) : editor.kind === "workshop" ? (
                <WorkshopForm
                  account={editor.account}
                  initial={editor.initial}
                  onIdentity={identify}
                  active={active}
                  onAccountChange={changeAccount}
                  readOnly={readOnly}
                />
              ) : (
                <TailoredCaptureForm
                  account={editor.account}
                  documentChat={editor.kind === "document"}
                  active={active}
                  onAccountChange={changeAccount}
                  profile={editor.scope || undefined}
                  bootId={bootId}
                  readOnly={readOnly}
                  onCreated={(created) => {
                    onCreated(created);
                    if (selection.current === editor.key) {
                      setPendingTerminalFocus("agent");
                      setSessions((old) => ({ ...old, [editor.scope]: created }));
                    }
                  }}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>,
    document.body,
  );
}
