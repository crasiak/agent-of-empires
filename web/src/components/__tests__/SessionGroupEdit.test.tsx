// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { jsonResponse, makeSession, makeWorkspace, renderRow } from "./fixtures";

const paths = ["Review/Done", "Review / Done", "  raw / path  "];
const label = (path: string) =>
  path === paths[2] ? String.raw`"\u0020\u0020raw / path\u0020\u0020"` : JSON.stringify(path);
const groups = paths.map((path) => ({ path, session_count: 1 }));
const workspace = (id = "s1", group_path = "old/stale") =>
  makeWorkspace("w", [makeSession({ id, title: id, group_path })]);
let patches: Array<{ url: string; body: unknown }>;
let load: () => Promise<Response>;
let patchStatus: number;
let patchResult: (() => Promise<Response>) | undefined;

function deferred() {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>((r) => (resolve = r));
  return { promise, resolve };
}

beforeEach(() => {
  patches = [];
  patchStatus = 200;
  patchResult = undefined;
  load = async () => jsonResponse(groups);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/groups") return load();
      if (init?.method === "PATCH") {
        patches.push({ url, body: JSON.parse(init.body as string) });
        return patchResult ? patchResult() : jsonResponse({ id: "s1" }, patchStatus);
      }
      return jsonResponse({});
    }),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function open() {
  fireEvent.contextMenu(screen.getByTestId("sidebar-session-row"));
  fireEvent.click(screen.getByTestId("sidebar-context-menu-more"));
  fireEvent.click(screen.getByTestId("sidebar-context-menu-edit-group"));
}
const chooser = () => screen.getByRole("combobox", { name: "Existing group" }) as HTMLSelectElement;
const input = () => screen.getByTestId("session-group-modal-input");
const save = () => fireEvent.click(screen.getByTestId("session-group-modal-save"));
function select(path: string) {
  const option = screen.getByRole("option", {
    name: label(path),
  }) as HTMLOptionElement;
  fireEvent.change(chooser(), { target: { value: option.value } });
}
async function closed() {
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
}

describe("session group chooser PATCH contract", () => {
  it.each(paths)("sends the exact selected raw path %j", async (path) => {
    renderRow(workspace());
    open();
    await screen.findByRole("option", { name: label(path) });
    expect(screen.getByRole("option", { name: '"Review/Done"', exact: true }).textContent).not.toBe(
      screen.getByRole("option", { name: '"Review / Done"', exact: true }).textContent,
    );
    select(path);
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s1/group", body: { group: path } }]);
  });

  it.each([
    ["  new / project  ", "new / project"],
    ["   ", ""],
  ])("manual input %j keeps trim and clear semantics", async (typed, expected) => {
    renderRow(workspace());
    open();
    await screen.findByRole("option", { name: '"Review/Done"' });
    fireEvent.change(chooser(), { target: { value: "manual" } });
    fireEvent.change(input(), { target: { value: typed } });
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s1/group", body: { group: expected } }]);
  });

  it("offers explicit ungrouping", async () => {
    renderRow(workspace());
    open();
    fireEvent.change(chooser(), { target: { value: "" } });
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s1/group", body: { group: "" } }]);
  });

  it.each(["Cancel", "Escape", "unchanged"])("%s does not PATCH a stale raw current group", async (action) => {
    renderRow(workspace("s1", "  stale / group  "));
    open();
    await screen.findByRole("option", { name: '"Review/Done"' });
    expect(chooser().selectedOptions[0]!.label).toBe(String.raw`"\u0020\u0020stale / group\u0020\u0020"`);
    if (action === "Cancel") {
      select("Review/Done");
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    } else if (action === "Escape") {
      select("Review / Done");
      fireEvent.keyDown(chooser(), { key: "Escape" });
    } else save();
    await closed();
    expect(patches).toEqual([]);
  });

  it.each(["empty", "error"])("distinguishes %s loads and allows manual saving", async (state) => {
    load = state === "empty" ? async () => jsonResponse([]) : async () => jsonResponse({}, 500);
    renderRow(workspace());
    open();
    await screen.findByText(
      state === "empty" ? "No existing groups." : "Could not load groups. Enter a group manually.",
    );
    expect(chooser().selectedOptions[0]!.textContent).toContain('"old/stale"');
    fireEvent.change(input(), { target: { value: "  recovery  " } });
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s1/group", body: { group: "recovery" } }]);
  });

  it("retains an exact existing destination after a failed PATCH and retries it unchanged", async () => {
    patchStatus = 500;
    renderRow(workspace());
    open();
    await screen.findByRole("option", { name: '"Review / Done"' });
    select("Review / Done");
    save();
    await screen.findByText("Failed to update group.");
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(chooser().value).toBe("group:Review / Done");
    patchStatus = 200;
    save();
    await closed();
    expect(patches).toEqual([
      { url: "/api/sessions/s1/group", body: { group: "Review / Done" } },
      { url: "/api/sessions/s1/group", body: { group: "Review / Done" } },
    ]);
  });

  it("unchanged Save preserves a stale current path while loading", async () => {
    const request = deferred();
    const started = vi.fn(() => request.promise);
    load = started;
    renderRow(workspace("s1", "  stale  "));
    open();
    await waitFor(() => expect(started).toHaveBeenCalledTimes(1));
    expect(screen.getByText("Loading groups...")).toBeTruthy();
    save();
    await closed();
    await act(async () => request.resolve(jsonResponse(groups)));
    expect(patches).toEqual([]);
  });

  it("keeps the current group and newer manual choice across a delayed load", async () => {
    const request = deferred();
    const started = vi.fn(() => request.promise);
    load = started;
    renderRow(workspace());
    open();
    await waitFor(() => expect(started).toHaveBeenCalledTimes(1));
    expect(screen.getByText("Loading groups...")).toBeTruthy();
    expect(chooser().selectedOptions[0]!.textContent).toContain('"old/stale"');
    fireEvent.change(input(), { target: { value: "  chosen / now  " } });
    await act(async () => request.resolve(jsonResponse(groups)));
    await screen.findByRole("option", { name: '"Review/Done"' });
    expect((input() as HTMLInputElement).value).toBe("  chosen / now  ");
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s1/group", body: { group: "chosen / now" } }]);
  });

  it.each([200, 500])("ignores the previous session's late PATCH completion (%i)", async (status) => {
    const request = deferred();
    patchResult = () => request.promise;
    const row = renderRow(workspace("s1", "first"));
    open();
    fireEvent.change(input(), { target: { value: "first-destination" } });
    save();
    await waitFor(() => expect(patches).toHaveLength(1));
    row.rerenderRow(workspace("s2", "second"));
    fireEvent.change(input(), { target: { value: "second-draft" } });
    patchResult = undefined;
    await act(async () => request.resolve(jsonResponse({ id: "s1" }, status)));
    expect(screen.getByRole("dialog", { name: "Edit group" })).toBeTruthy();
    expect((input() as HTMLInputElement).value).toBe("second-draft");
    expect(screen.queryByText("Failed to update group.")).toBeNull();
    save();
    await closed();
    expect(patches).toEqual([
      { url: "/api/sessions/s1/group", body: { group: "first-destination" } },
      { url: "/api/sessions/s2/group", body: { group: "second-draft" } },
    ]);
  });

  it("isolates a different session and ignores the previous session's late request", async () => {
    const oldRequest = deferred();
    const newRequest = deferred();
    const started = vi.fn().mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);
    load = started;
    const row = renderRow(workspace("s1", "first"));
    open();
    await waitFor(() => expect(started).toHaveBeenCalledTimes(1));
    fireEvent.change(input(), { target: { value: "wrong-session-draft" } });
    row.rerenderRow(workspace("s2", "second"));
    await waitFor(() => expect(started).toHaveBeenCalledTimes(2));
    expect((input() as HTMLInputElement).value).toBe("second");
    fireEvent.change(input(), { target: { value: "second-choice" } });
    await act(async () => newRequest.resolve(jsonResponse(groups)));
    await screen.findByRole("option", { name: '"Review/Done"' });
    select("Review / Done");
    await act(async () => oldRequest.resolve(jsonResponse([{ path: "obsolete", session_count: 1 }])));
    expect(screen.queryByRole("option", { name: '"obsolete"' })).toBeNull();
    save();
    await closed();
    expect(patches).toEqual([{ url: "/api/sessions/s2/group", body: { group: "Review / Done" } }]);
  });
});
