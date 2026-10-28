// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { TailoredCaptureForm } from "../TailoredCaptureForm";
import { TailoredSessionsOverlay } from "../TailoredSessionsOverlay";
import { createSession, fetchTailoredHistory } from "../../lib/api";
import {
  PENDING_CREATE_EXPIRED_MESSAGE,
  PENDING_CREATE_MAX_AGE_MS,
  peekPendingCreate,
  resolvePendingCreate,
  startPendingCreates,
} from "../../lib/pendingCreates";
import { makeSession } from "./fixtures";

vi.mock("../../lib/api", () => ({
  createSession: vi.fn(),
  fetchTailoredHistory: vi.fn(),
  getWorkshopDefaults: vi.fn().mockResolvedValue({ repository: null }),
}));
vi.mock("../TerminalView", () => ({ TerminalView: () => <div>Terminal</div> }));

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.mocked(fetchTailoredHistory).mockResolvedValue({ entries: [] });
});
afterEach(() => {
  for (const pending of JSON.parse(localStorage.getItem("aoe-pending-creates") ?? "[]"))
    resolvePendingCreate(pending.body.idempotency_key);
  cleanup();
  vi.restoreAllMocks();
});

it("keeps account, preset and profile owners independent during real background settlement", async () => {
  type Result = Awaited<ReturnType<typeof createSession>>;
  const finishes = new Map<string, (result: Result) => void>();
  vi.mocked(createSession)
    .mockReset()
    .mockImplementation(async (body) => {
      if (!body.retry_origin) return { ok: false, network: true, error: "offline" };
      return new Promise((resolve) => {
        finishes.set(body.idempotency_key!, resolve);
      });
    });
  const onCreated = vi.fn(),
    onFailed = vi.fn(),
    onUnknown = vi.fn(),
    localCreated = vi.fn();
  startPendingCreates({ onCreated, onFailed, onUnknown });
  const owners = [
    { profile: "first", account: "personal" as const, documentChat: false },
    { profile: "first", account: "work" as const, documentChat: false },
    { profile: "first", account: "personal" as const, documentChat: true },
    { profile: "second", account: "personal" as const, documentChat: false },
  ];
  const forms = (active: boolean) =>
    owners.map((owner, i) => (
      <section key={i} aria-label={`owner-${i}`}>
        <TailoredCaptureForm
          {...owner}
          active={active}
          bootId="boot"
          readOnly={false}
          onAccountChange={vi.fn()}
          onCreated={localCreated}
        />
      </section>
    ));
  const view = render(<>{forms(true)}</>);
  for (const [i, owner] of owners.entries()) {
    const region = within(screen.getByRole("region", { name: `owner-${i}` }));
    fireEvent.change(region.getByRole("textbox", { name: owner.documentChat ? "Public document URL" : "Idea" }), {
      target: { value: owner.documentChat ? "https://example.org/source.md" : `private-${i}` },
    });
    fireEvent.click(region.getByRole("button", { name: owner.documentChat ? "Start" : "Capture idea", exact: true }));
    await region.findByText("offline");
  }
  const originals = vi.mocked(createSession).mock.calls.map(([body]) => body);
  view.rerender(<>{forms(false)}</>);
  await waitFor(() => expect(finishes.size).toBe(4));
  expect(new Set(originals.map((body) => body.idempotency_key)).size).toBe(4);
  const results: Result[] = [
    { ok: true, session: makeSession({ id: "first-done" }) },
    { ok: false, outcomeUnknown: true, error: "work unknown" },
    { ok: false, error: "document refused" },
    { ok: true, session: makeSession({ id: "second-done" }) },
  ];
  for (let i = 0; i < 4; i++) {
    await act(async () => finishes.get(originals[i]!.idempotency_key!)!(results[i]!));
    for (let j = i + 1; j < 4; j++) {
      const region = within(screen.getByRole("region", { name: `owner-${j}` }));
      expect(region.getByRole("alert").textContent).toBe("offline");
      expect(
        (
          region.getByRole("textbox", {
            name: owners[j]!.documentChat ? "Public document URL" : "Idea",
          }) as HTMLTextAreaElement
        ).disabled,
      ).toBe(true);
    }
  }
  view.rerender(<>{forms(true)}</>);
  expect(onCreated).toHaveBeenCalledTimes(2);
  expect(onFailed).toHaveBeenCalledOnce();
  expect(onUnknown).toHaveBeenCalledOnce();
  expect(localCreated).not.toHaveBeenCalled();
  expect(createSession).toHaveBeenCalledTimes(8);
  expect(peekPendingCreate()).toBeNull();
});

it("keeps an expired retained owner blocked without retrying beyond the replay window", async () => {
  vi.mocked(createSession).mockReset().mockResolvedValue({ ok: false, network: true, error: "offline" });
  const onFailed = vi.fn();
  startPendingCreates({ onCreated: vi.fn(), onFailed, onUnknown: vi.fn() });
  const props = {
    account: "personal" as const,
    documentChat: false,
    active: true,
    bootId: "boot",
    readOnly: false,
    onCreated: vi.fn(),
    onAccountChange: vi.fn(),
  };
  const view = render(<TailoredCaptureForm {...props} />);
  fireEvent.change(screen.getByRole("textbox", { name: "Idea" }), { target: { value: "original" } });
  fireEvent.click(screen.getByRole("button", { name: "Capture idea" }));
  await screen.findByText("offline");
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now + PENDING_CREATE_MAX_AGE_MS);
  view.rerender(<TailoredCaptureForm {...props} active={false} />);
  await waitFor(() => expect(onFailed).toHaveBeenCalledOnce());
  view.rerender(<TailoredCaptureForm {...props} />);
  expect(screen.getByRole("alert").textContent).toBe(PENDING_CREATE_EXPIRED_MESSAGE);
  expect((screen.getByRole("button", { name: "Capture idea" }) as HTMLButtonElement).disabled).toBe(true);
  expect(createSession).toHaveBeenCalledOnce();
  expect(peekPendingCreate()).toBeNull();
  expect(JSON.parse(localStorage.getItem("aoe-pending-creates") ?? "[]")).toEqual([]);
});

it.each(
  (["capture", "document"] as const).flatMap((kind) =>
    (["success", "refusal", "unknown"] as const).flatMap((outcome) =>
      [false, true].map((reopenBeforeSettlement) => ({ kind, outcome, reopenBeforeSettlement })),
    ),
  ),
)(
  "settles retained $kind owner on background $outcome once (early reopen: $reopenBeforeSettlement)",
  async ({ kind, outcome, reopenBeforeSettlement }) => {
    const globalCreated = vi.fn(),
      globalFailed = vi.fn(),
      globalUnknown = vi.fn(),
      localCreated = vi.fn();
    startPendingCreates({ onCreated: globalCreated, onFailed: globalFailed, onUnknown: globalUnknown });
    let finish!: (value: Awaited<ReturnType<typeof createSession>>) => void;
    vi.mocked(createSession)
      .mockReset()
      .mockResolvedValueOnce({ ok: false, network: true, error: "network unknown" })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      );
    const props = {
      account: "personal" as const,
      onAccountChange: vi.fn(),
      profile: "owner",
      bootId: "original-boot",
      readOnly: false,
      onCreated: localCreated,
      onClose: vi.fn(),
    };
    const view = render(<TailoredSessionsOverlay {...props} />);
    fireEvent.click(screen.getByRole("button", { name: kind === "capture" ? /Capture an idea/ : /Document chat/ }));
    const label = kind === "capture" ? "Idea" : "Public document URL";
    const editor = screen.getByRole("textbox", { name: label }) as HTMLTextAreaElement;
    const text = kind === "capture" ? "private original 🌱\n\n" : "https://example.org/original.md";
    fireEvent.change(editor, { target: { value: text } });
    if (kind === "capture") fireEvent.click(screen.getByRole("checkbox", { name: "Immediately" }));
    else
      fireEvent.change(screen.getByRole("textbox", { name: "Opening question (optional)" }), {
        target: { value: "original question" },
      });
    fireEvent.click(screen.getByRole("button", { name: kind === "capture" ? "Capture idea" : "Start", exact: true }));
    await screen.findByText("network unknown");
    const original = vi.mocked(createSession).mock.calls[0]![0];
    view.rerender(<TailoredSessionsOverlay {...props} open={false} bootId="new-boot" />);
    await waitFor(() => expect(createSession).toHaveBeenCalledTimes(2));
    expect(vi.mocked(createSession).mock.calls[1]![0]).toEqual({ ...original, retry_origin: "original-boot" });
    if (reopenBeforeSettlement) {
      view.rerender(<TailoredSessionsOverlay {...props} open bootId="new-boot" />);
      expect(screen.getByRole("textbox", { name: label })).toBe(editor);
      expect(editor.value).toBe(text);
      expect(editor.disabled).toBe(true);
      expect(createSession).toHaveBeenCalledTimes(2);
    }
    const session = makeSession({ id: `settled-${kind}-${outcome}` });
    await act(async () =>
      finish(
        outcome === "success"
          ? { ok: true, session }
          : {
              ok: false,
              outcomeUnknown: outcome === "unknown",
              error: outcome === "unknown" ? "restart unknown" : "explicit refusal",
            },
      ),
    );
    await waitFor(() =>
      expect(
        outcome === "success" ? globalCreated : outcome === "unknown" ? globalUnknown : globalFailed,
      ).toHaveBeenCalledOnce(),
    );
    expect(localCreated).not.toHaveBeenCalled();
    expect(peekPendingCreate()).toBeNull();
    expect(JSON.parse(localStorage.getItem("aoe-pending-creates") ?? "[]")).toEqual([]);
    view.rerender(<TailoredSessionsOverlay {...props} open bootId="new-boot" />);
    expect(screen.getByRole("textbox", { name: label })).toBe(editor);
    if (outcome === "success") {
      expect(editor.value).toBe("");
      expect(editor.disabled).toBe(false);
      if (kind === "capture")
        expect((screen.getByRole("checkbox", { name: "Immediately" }) as HTMLInputElement).checked).toBe(false);
      else
        expect(
          (screen.getByRole("textbox", { name: "Opening question (optional)" }) as HTMLTextAreaElement).value,
        ).toBe("");
    } else {
      expect(editor.value).toBe(text);
      expect(screen.getByRole("alert").textContent).toBe(
        outcome === "unknown" ? "restart unknown" : "explicit refusal",
      );
      expect(editor.disabled).toBe(outcome === "unknown");
      expect(
        (
          screen.getByRole("button", {
            name: outcome === "unknown" ? "Retry same capture" : kind === "capture" ? "Capture idea" : "Start",
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(outcome === "unknown");
    }
    expect(createSession).toHaveBeenCalledTimes(2);
    expect(peekPendingCreate()).toBeNull();
    expect(JSON.parse(localStorage.getItem("aoe-pending-creates") ?? "[]")).toEqual([]);
    cleanup();
  },
);
