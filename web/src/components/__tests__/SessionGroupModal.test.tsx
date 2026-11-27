// @vitest-environment jsdom

import { useLayoutEffect } from "react";
import { act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { SessionGroupModal } from "../SessionGroupModal";
import { expectRestoresFocus } from "./dialogTestUtils";

function setup(currentGroup = "work", onSave: (group: string) => Promise<boolean> = vi.fn().mockResolvedValue(true)) {
  const onClose = vi.fn();
  const utils = render(
    <SessionGroupModal sessionTitle="alpha" currentGroup={currentGroup} onSave={onSave} onClose={onClose} />,
  );
  return {
    ...utils,
    onSave,
    onClose,
    input: screen.getByTestId("session-group-modal-input") as HTMLInputElement,
    saveBtn: screen.getByTestId("session-group-modal-save") as HTMLButtonElement,
    cancelBtn: screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement,
  };
}

const errorText = () => screen.queryByTestId("session-group-modal-error")?.textContent;
const pending = () => {
  let resolve: (ok: boolean) => void = () => {};
  const onSave = vi.fn(() => new Promise<boolean>((r) => (resolve = r)));
  return { onSave, resolve: (ok: boolean) => resolve(ok) };
};

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify([]))),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("SessionGroupModal", () => {
  it("is a modal named by its title with the current group focused", () => {
    const { input, container } = setup("work/projects");
    expect(screen.getByRole("dialog", { name: "Edit group" }).getAttribute("aria-modal")).toBe("true");
    expect(container.textContent).toContain("alpha");
    expect(input.value).toBe("work/projects");
    expect(document.activeElement).toBe(input);
    // The prompt, not the input or hint, describes the dialog.
    screen.getByRole("dialog", { description: "Move alpha to a group." });
  });

  it.each([
    ["Save click", "", "  work/api  ", "work/api", "click"],
    ["Enter", "old", "new", "new", "enter"],
    ["blank value ungroups", "work", "   ", "", "click"],
    ["unchanged value closes without saving", "work", "  work  ", null, "click"],
  ] as const)("%s: saves the trimmed value then closes", async (_name, current, typed, saved, via) => {
    const { input, saveBtn, onSave, onClose } = setup(current);
    fireEvent.change(input, { target: { value: typed } });
    if (via === "enter") fireEvent.keyDown(input, { key: "Enter" });
    else fireEvent.click(saveBtn);
    if (saved == null) expect(onSave).not.toHaveBeenCalled();
    else expect(onSave).toHaveBeenCalledWith(saved);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it.each([
    ["", "work", "Failed to update group."],
    ["work", "", "Failed to clear group."],
  ])("from %j to %j a failed save shows %j and stays open", async (current, typed, message) => {
    const { input, saveBtn, onClose } = setup(current, vi.fn().mockResolvedValue(false));
    fireEvent.change(input, { target: { value: typed } });
    fireEvent.click(saveBtn);
    await waitFor(() => expect(errorText()).toBe(message));
    expect(onClose).not.toHaveBeenCalled();
    expect(saveBtn.disabled).toBe(false);
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: "again" } });
    expect(errorText()).toBeUndefined();
  });

  it("disables controls and ignores repeat saves while one is in flight", async () => {
    const { onSave, resolve } = pending();
    const { input, saveBtn, cancelBtn, onClose } = setup("", onSave);
    fireEvent.change(input, { target: { value: "work" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await Promise.resolve();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(saveBtn.disabled).toBe(true);
    expect(cancelBtn.disabled).toBe(true);
    expect(saveBtn.textContent).toContain("Saving...");
    resolve(true);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("closes without saving via Cancel, Escape, and the backdrop but not the panel", () => {
    const { cancelBtn, input, onClose, onSave } = setup();
    fireEvent.click(cancelBtn);
    fireEvent.keyDown(input, { key: "Escape" });
    const backdrop = screen.getByTestId("session-group-modal");
    fireEvent.click(backdrop.firstElementChild!);
    expect(onClose).toHaveBeenCalledTimes(2);
    fireEvent.click(backdrop);
    expect(onClose).toHaveBeenCalledTimes(3);
    expect(onSave).not.toHaveBeenCalled();
  });

  it.each([
    ["keyCode 229", { key: "Enter", keyCode: 229 }],
    ["isComposing", { key: "Enter", isComposing: true }],
  ] as const)("does not submit while an IME composes (%s)", (_label, init) => {
    const { input, onSave, onClose } = setup("old");
    fireEvent.change(input, { target: { value: "new" } });
    fireEvent.keyDown(input, init);
    expect(onSave).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSave).toHaveBeenCalledWith("new");
  });

  it("leaves an already-handled Enter alone", () => {
    const { input, onSave } = setup("old");
    fireEvent.change(input, { target: { value: "new" } });
    const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    event.preventDefault();
    fireEvent(input, event);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("withholds another profile's groups in the committed render and ignores its late result", async () => {
    let finishB!: (value: Response) => void;
    const pendingB = new Promise<Response>((resolve) => {
      finishB = resolve;
    });
    const requested: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        requested.push(url);
        return url.endsWith("profile=b")
          ? pendingB
          : Promise.resolve(new Response(JSON.stringify([{ path: "from-a", session_count: 1 }])));
      }),
    );
    const painted: string[][] = [];
    function PaintProbe() {
      useLayoutEffect(() => {
        painted.push(Array.from(document.querySelectorAll("option")).map((option) => option.value));
      });
      return null;
    }
    const props = { sessionTitle: "alpha", currentGroup: "work", onSave: vi.fn(), onClose: vi.fn() };
    const view = (profile: string) => (
      <>
        <SessionGroupModal {...props} profile={profile} />
        <PaintProbe />
      </>
    );
    const { rerender } = render(view("a"));
    await screen.findByRole("option", { name: '"from-a"' });
    rerender(view("b"));
    expect(painted.at(-1)).not.toContain("group:from-a");
    expect(screen.queryByRole("option", { name: '"from-a"' })).toBeNull();
    await act(async () => {
      finishB(new Response(JSON.stringify([{ path: "from-b", session_count: 1 }])));
    });
    await screen.findByRole("option", { name: '"from-b"' });
    expect(requested).toEqual(["/api/groups?profile=a", "/api/groups?profile=b"]);
    expect(screen.queryByRole("option", { name: '"from-a"' })).toBeNull();
  });

  it("restores focus on unmount", () => {
    expectRestoresFocus(() => setup().unmount);
  });
});
