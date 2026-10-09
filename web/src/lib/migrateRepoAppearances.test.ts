// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { importRepoAppearances } from "./api";
import { migrateRepoAppearances, parseLegacyRepoAppearances } from "./migrateRepoAppearances";
vi.mock("./api", () => ({ importRepoAppearances: vi.fn() }));
const KEY = "aoe-repo-appearance-v1";
beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
});
it("imports once, retries failures, and never uses browser data over canonical clears", async () => {
  const old = JSON.stringify({
    "/a": { alias: "Old", color: "sky" },
    "/browser": { alias: " Browser ", color: "rose" },
  });
  localStorage.setItem(KEY, old);
  vi.mocked(importRepoAppearances)
    .mockResolvedValueOnce(null)
    .mockResolvedValue({ "/a": {}, "/browser": { alias: "Browser", color: "rose" } });
  expect(await migrateRepoAppearances()).toBeNull();
  expect(localStorage.getItem(KEY)).toBe(old);
  expect(await migrateRepoAppearances()).toEqual({ "/a": {}, "/browser": { alias: "Browser", color: "rose" } });
  expect(importRepoAppearances).toHaveBeenCalledWith({
    "/a": { alias: "Old", color: "sky" },
    "/browser": { alias: "Browser", color: "rose" },
  });
  expect(localStorage.getItem(KEY)).toBeNull();
  expect(await migrateRepoAppearances()).toBeNull();
  expect(importRepoAppearances).toHaveBeenCalledTimes(2);
});
it("normalizes only valid legacy fields and paths", () => {
  expect(
    parseLegacyRepoAppearances(
      JSON.stringify({
        "/a": { alias: " A ", color: "bad" },
        "/b": { color: "teal" },
        "/c": {},
        relative: { color: "sky" },
        __scratch__: { color: "amber" },
        "/invalid": true,
      }),
    ),
  ).toEqual({ "/a": { alias: "A" }, "/b": { color: "teal" }, __scratch__: { color: "amber" } });
  for (const raw of ["bad", "[]", "null"]) expect(parseLegacyRepoAppearances(raw)).toEqual({});
});
