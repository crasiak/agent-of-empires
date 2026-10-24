// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { TailoredSessionsOverlay, type CaptureAccount } from "../TailoredSessionsOverlay";
import { createSession } from "../../lib/api";
import { registerPendingCreate, releasePendingCreate, resolvePendingCreate } from "../../lib/pendingCreates";
import { makeSession } from "./fixtures";

vi.mock("../../lib/api", () => ({ createSession: vi.fn() }));
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
function Harness() {
  const [account, setAccount] = useState<CaptureAccount>("personal");
  return (
    <TailoredSessionsOverlay
      account={account}
      onAccountChange={setAccount}
      profile="main"
      bootId="boot-1"
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
});
afterEach(cleanup);

describe("Tailored Sessions", () => {
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
