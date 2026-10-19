// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { fetchRepoAppearances, patchRepoAppearance } from "./api";
import { reportError } from "./toastBus";
import { refreshRepoAppearances, updateRepoAppearance, useRepoAppearances } from "./repoAppearanceStore";
import type { RepoAppearance } from "./repoAppearance";

vi.mock("./api", () => ({ fetchRepoAppearances: vi.fn(), patchRepoAppearance: vi.fn() }));
vi.mock("./toastBus", () => ({ reportError: vi.fn() }));
type Map = Record<string, RepoAppearance>;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
beforeEach(async () => {
  vi.clearAllMocks();
  vi.mocked(fetchRepoAppearances).mockResolvedValue({});
  await refreshRepoAppearances();
});

it("hydrates after first render and shares external updates without one poll per consumer", async () => {
  const response = deferred<Map>();
  vi.mocked(fetchRepoAppearances).mockReturnValue(response.promise);
  const a = renderHook(useRepoAppearances);
  const b = renderHook(useRepoAppearances);
  expect(a.result.current).toEqual({});
  await act(async () => {
    response.resolve({ "/a": { alias: "Alpha", color: "sky" } });
    await refreshRepoAppearances();
  });
  expect(a.result.current).toEqual({ "/a": { alias: "Alpha", color: "sky" } });
  expect(b.result.current).toBe(a.result.current);
  expect(fetchRepoAppearances).toHaveBeenCalledTimes(2); // reset + one shared hydration
  vi.mocked(fetchRepoAppearances).mockResolvedValue({
    "/a": { alias: "Alpha", color: "rose" },
    "/peer": { color: "teal" },
  });
  await act(() => refreshRepoAppearances());
  expect(a.result.current["/a"]?.color).toBe("rose");
  expect(b.result.current["/peer"]?.color).toBe("teal");
  vi.mocked(fetchRepoAppearances).mockResolvedValue(null);
  await act(() => refreshRepoAppearances());
  expect(a.result.current["/a"]?.color).toBe("rose");
});

it("serializes optimistic patches, ignores stale reads, and rolls back only failed edits", async () => {
  vi.mocked(fetchRepoAppearances).mockResolvedValue({ "/a": { alias: "Alpha" } });
  await refreshRepoAppearances();
  const hook = renderHook(useRepoAppearances);
  await act(() => refreshRepoAppearances());
  const stale = deferred<Map>();
  vi.mocked(fetchRepoAppearances).mockReturnValue(stale.promise);
  const reading = refreshRepoAppearances();
  const first = deferred<Map | null>();
  vi.mocked(patchRepoAppearance).mockReturnValueOnce(first.promise).mockResolvedValueOnce(null);
  let writing!: Promise<void>;
  act(() => {
    writing = updateRepoAppearance("/a", { color: "sky" });
    void updateRepoAppearance("/a", { alias: null });
  });
  expect(hook.result.current["/a"]).toEqual({ color: "sky" });
  await act(async () => {
    stale.resolve({});
    await reading;
  });
  expect(hook.result.current["/a"]).toEqual({ color: "sky" });
  await act(async () => {
    first.resolve({ "/a": { alias: "Alpha", color: "sky" }, "/peer": { color: "rose" } });
    await writing;
  });
  expect(hook.result.current).toEqual({ "/a": { alias: "Alpha", color: "sky" }, "/peer": { color: "rose" } });
  expect(patchRepoAppearance).toHaveBeenNthCalledWith(1, "/a", { color: "sky" });
  expect(patchRepoAppearance).toHaveBeenNthCalledWith(2, "/a", { alias: null });
  expect(reportError).toHaveBeenCalledWith(expect.stringContaining("reverted"));
  expect(localStorage.getItem("aoe-repo-appearance-v1")).toBeNull();
});
