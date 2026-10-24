// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { TailoredSessionsOverlay, type CaptureAccount } from "../TailoredSessionsOverlay";
import { createSession, fetchTailoredHistory, setTailoredHistoryHidden } from "../../lib/api";
import type { TailoredHistoryEntry } from "../../lib/types";
import { registerPendingCreate, releasePendingCreate, resolvePendingCreate } from "../../lib/pendingCreates";
import { makeSession } from "./fixtures";

vi.mock("../../lib/api", () => ({
  createSession: vi.fn(),
  fetchTailoredHistory: vi.fn(),
  setTailoredHistoryHidden: vi.fn(),
}));
vi.mock("../TerminalView", () => ({ TerminalView: () => <textarea aria-label="Capture terminal" /> }));
vi.mock("../../lib/pendingCreates", () => ({
  registerPendingCreate: vi.fn(),
  releasePendingCreate: vi.fn(),
  resolvePendingCreate: vi.fn(),
  retryBody: (pending: { body: object; origin: string | null }) => ({
    ...pending.body,
    retry_origin: pending.origin ?? "unknown",
  }),
}));

const onCreated = vi.fn();
const onClose = vi.fn();
function Harness({ readOnly = false }: { readOnly?: boolean }) {
  const [account, setAccount] = useState<CaptureAccount>("personal");
  return (
    <TailoredSessionsOverlay
      account={account}
      onAccountChange={setAccount}
      profile="main"
      bootId="boot-1"
      readOnly={readOnly}
      onCreated={onCreated}
      onClose={onClose}
    />
  );
}
function compose() {
  render(<Harness />);
  fireEvent.click(screen.getByRole("button", { name: /Capture an idea/ }));
}
const submit = () => fireEvent.click(screen.getByRole("button", { name: /Capture idea|Retry same capture/ }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(fetchTailoredHistory).mockResolvedValue({ entries: [] });
  vi.mocked(setTailoredHistoryHidden).mockResolvedValue({ ok: true });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function savedCapture(): TailoredHistoryEntry {
  const session = makeSession({ id: "saved-capture", title: "Saved idea", profile: "main", scratch: true, tool: "pi" });
  return {
    id: session.id,
    title: session.title,
    profile: "main",
    account: "personal",
    input_preview: "A useful database idea",
    status: "idle",
    created_at: "2026-10-24T12:00:00Z",
    hidden: false,
    session,
  };
}

describe("Tailored Sessions", () => {
  it("opens a saved conversation and returns to history without creating or navigating", async () => {
    vi.mocked(fetchTailoredHistory).mockResolvedValue({ entries: [savedCapture()] });
    render(<Harness />);
    await screen.findByRole("button", { name: "Open Saved idea" });
    fireEvent.change(screen.getByLabelText("Find a tailored preset"), { target: { value: "database" } });
    fireEvent.keyDown(screen.getByLabelText("Find a tailored preset"), { key: "Enter" });
    const terminal = await screen.findByLabelText("Capture terminal");
    expect(createSession).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
    fireEvent.keyDown(terminal, { key: "F2" });
    await screen.findByRole("button", { name: "Open Saved idea" });
    expect(document.activeElement).toBe(screen.getByLabelText("Find a tailored preset"));
    expect(setTailoredHistoryHidden).not.toHaveBeenCalled();
  });

  it("persists hide across reopening and restores without changing Session lifecycle", async () => {
    let entry = savedCapture();
    vi.mocked(fetchTailoredHistory).mockImplementation(async () => ({ entries: [entry] }));
    vi.mocked(setTailoredHistoryHidden).mockImplementation(async (_id, hidden) => {
      entry = { ...entry, hidden };
      return { ok: true };
    });
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: "Hide Saved idea from history" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Open Saved idea" })).toBeNull());
    cleanup();
    render(<Harness />);
    await screen.findByText("No matching captures.");
    fireEvent.click(screen.getByRole("button", { name: "Show hidden" }));
    fireEvent.click(await screen.findByRole("button", { name: "Restore Saved idea in history" }));
    await screen.findByText("No hidden captures.");
    fireEvent.click(screen.getByRole("button", { name: "Show recent" }));
    await screen.findByRole("button", { name: "Open Saved idea" });
    expect(setTailoredHistoryHidden).toHaveBeenNthCalledWith(1, "saved-capture", true);
    expect(setTailoredHistoryHidden).toHaveBeenNthCalledWith(2, "saved-capture", false);
    expect(createSession).not.toHaveBeenCalled();
    expect(entry.session.trashed_at).toBeFalsy();
    expect(entry.session.archived_at).toBeFalsy();
  });

  it("keeps an entry visible when hiding fails and disables mutations for read-only clients", async () => {
    vi.mocked(fetchTailoredHistory).mockResolvedValue({ entries: [savedCapture()] });
    vi.mocked(setTailoredHistoryHidden).mockResolvedValue({ ok: false, message: "Visibility save failed" });
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: "Hide Saved idea from history" }));
    await screen.findByText("Visibility save failed");
    expect(screen.getByRole("button", { name: "Open Saved idea" })).toBeDefined();
    cleanup();
    vi.mocked(setTailoredHistoryHidden).mockClear();
    render(<Harness readOnly />);
    const hide = await screen.findByRole("button", { name: "Hide Saved idea from history" });
    expect((hide as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Open Saved idea" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(hide);
    expect(setTailoredHistoryHidden).not.toHaveBeenCalled();
    expect(createSession).not.toHaveBeenCalled();
  });

  it("does not resurrect a hidden entry when an older poll finishes after the write", async () => {
    vi.useFakeTimers();
    let release!: (value: { entries: TailoredHistoryEntry[] }) => void;
    vi.mocked(fetchTailoredHistory)
      .mockResolvedValueOnce({ entries: [savedCapture()] })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            release = resolve;
          }),
      );
    await act(async () => {
      render(<Harness />);
    });
    expect(screen.getByRole("button", { name: "Open Saved idea" })).toBeDefined();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(fetchTailoredHistory).toHaveBeenCalledTimes(2);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Hide Saved idea from history" }));
    });
    expect(screen.queryByRole("button", { name: "Open Saved idea" })).toBeNull();
    await act(async () => release({ entries: [savedCapture()] }));
    expect(screen.queryByRole("button", { name: "Open Saved idea" })).toBeNull();
  });
  it("keeps new capture available if history cannot be read", async () => {
    vi.mocked(fetchTailoredHistory).mockResolvedValue(null);
    render(<Harness />);
    await screen.findByText("Capture history is unavailable. Existing Sessions are unchanged.");
    fireEvent.click(screen.getByRole("button", { name: /Capture an idea/ }));
    expect(screen.getByLabelText("Idea")).toBeDefined();
    expect(createSession).not.toHaveBeenCalled();
  });

  it("starts a new draft with Immediately off after a completed capture", async () => {
    vi.mocked(createSession).mockResolvedValue({ ok: true, session: makeSession({ id: "finished" }) });
    compose();
    fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "First idea" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "Immediately" }));
    submit();
    await screen.findByLabelText("Capture terminal");
    fireEvent.click(screen.getByRole("button", { name: "Back to history · F2" }));
    fireEvent.click(screen.getByRole("button", { name: /Capture an idea/ }));
    expect((screen.getByLabelText("Idea") as HTMLTextAreaElement).value).toBe("");
    expect((screen.getByRole("checkbox", { name: "Immediately" }) as HTMLInputElement).checked).toBe(false);
    expect(createSession).toHaveBeenCalledOnce();
  });

  it("filters presets, refuses empty ideas, and portals outside the dashboard", () => {
    const { container } = render(<Harness />);
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Find a tailored preset" }), {
      target: { value: "shopping" },
    });
    expect(screen.queryByRole("button", { name: /Capture an idea/ })).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Find a tailored preset" }), { target: { value: "idea" } });
    fireEvent.click(screen.getByRole("button", { name: /Capture an idea/ }));
    fireEvent.submit(screen.getByRole("button", { name: "Capture idea" }).closest("form")!);
    expect(createSession).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByLabelText("Idea"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it.each([
    ["personal", false],
    ["work", true],
  ] as const)(
    "launches %s with Immediately=%s without accepting client launch commands",
    async (account, immediately) => {
      const session = makeSession({ id: "capture-1", scratch: true });
      vi.mocked(createSession).mockResolvedValue({ ok: true, session });
      compose();
      fireEvent.change(screen.getByLabelText("Capture account"), { target: { value: account } });
      if (immediately) fireEvent.click(screen.getByRole("checkbox", { name: "Immediately" }));
      fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "quoted 'idea'; $x\nnext" } });
      submit();
      await screen.findByLabelText("Capture terminal");
      expect(createSession).toHaveBeenCalledOnce();
      expect(createSession).toHaveBeenCalledWith(
        expect.objectContaining({
          path: "",
          tool: "pi",
          scratch: true,
          profile: "main",
          view: "terminal",
          tailored: { preset: "capture-idea", account, idea: "quoted 'idea'; $x\nnext", immediately },
        }),
      );
      expect(vi.mocked(createSession).mock.calls[0]![0].command_override).toBeUndefined();
      expect(onCreated).toHaveBeenCalledWith(session);
      expect(onClose).not.toHaveBeenCalled();
      expect(resolvePendingCreate).toHaveBeenCalledOnce();
      fireEvent.keyDown(screen.getByLabelText("Capture terminal"), { key: "Escape" });
      expect(onClose).not.toHaveBeenCalled();
    },
  );

  it("retries an unknown outcome under its original key and daemon fence", async () => {
    vi.mocked(createSession)
      .mockResolvedValueOnce({ ok: false, network: true, error: "No answer" })
      .mockResolvedValueOnce({ ok: true, session: makeSession({ id: "capture-retry" }) });
    compose();
    fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "Keep this idea" } });
    submit();
    await screen.findByRole("button", { name: "Retry same capture" });
    const first = vi.mocked(createSession).mock.calls[0]![0];
    expect((screen.getByLabelText("Idea") as HTMLTextAreaElement).disabled).toBe(true);
    submit();
    await screen.findByLabelText("Capture terminal");
    expect(vi.mocked(createSession).mock.calls[1]![0]).toEqual({ ...first, retry_origin: "boot-1" });
  });

  it("prevents double submission and keeps an in-flight capture recoverable after dismissal", async () => {
    let resolve!: (result: Awaited<ReturnType<typeof createSession>>) => void;
    vi.mocked(createSession).mockReturnValue(
      new Promise((done) => {
        resolve = done;
      }),
    );
    compose();
    fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "One capture" } });
    const form = screen.getByRole("button", { name: "Capture idea" }).closest("form")!;
    fireEvent.submit(form);
    fireEvent.submit(form);
    expect(createSession).toHaveBeenCalledOnce();
    const pending = vi.mocked(registerPendingCreate).mock.calls[0]![0];
    cleanup();
    expect(releasePendingCreate).toHaveBeenCalledWith(pending.body.idempotency_key);
    const session = makeSession({ id: "late-capture" });
    await act(async () => resolve({ ok: true, session }));
    expect(onCreated).toHaveBeenCalledWith(session);
  });

  it("allows correction after a definite refusal but fences an unresolvable outcome", async () => {
    vi.mocked(createSession)
      .mockResolvedValueOnce({ ok: false, error: "Capture profile missing" })
      .mockResolvedValueOnce({ ok: false, outcomeUnknown: true, error: "Server restarted" });
    compose();
    fireEvent.change(screen.getByLabelText("Idea"), { target: { value: "A draft" } });
    submit();
    await screen.findByText("Capture profile missing");
    expect((screen.getByLabelText("Idea") as HTMLTextAreaElement).disabled).toBe(false);
    submit();
    await screen.findByText("Server restarted");
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Retry same capture" }) as HTMLButtonElement).disabled).toBe(true),
    );
    expect(vi.mocked(createSession).mock.calls[0]![0].idempotency_key).not.toBe(
      vi.mocked(createSession).mock.calls[1]![0].idempotency_key,
    );
  });
});
