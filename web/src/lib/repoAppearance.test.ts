// @vitest-environment jsdom

import { expect, it } from "vitest";

import { applyRepoAppearanceUpdate, type RepoAppearance } from "./repoAppearance";

it("applyRepoAppearanceUpdate trims, validates, clears, and prunes without mutating its input", () => {
  const cases: [string, Record<string, RepoAppearance>, Partial<Record<"alias" | "color", string | null>>, unknown][] =
    [
      ["trimmed alias", {}, { alias: "  Alpha  " }, { "/repo/a": { alias: "Alpha" } }],
      ["null alias prunes", { "/repo/a": { alias: "Alpha" } }, { alias: null }, {}],
      [
        "blank alias clears",
        { "/repo/a": { alias: "Alpha", color: "amber" } },
        { alias: "   " },
        { "/repo/a": { color: "amber" } },
      ],
      [
        "color keeps alias",
        { "/repo/a": { alias: "Alpha" } },
        { color: "teal" },
        { "/repo/a": { alias: "Alpha", color: "teal" } },
      ],
      ["null color prunes", { "/repo/a": { color: "rose" } }, { color: null }, {}],
      [
        "unknown color ignored",
        { "/repo/a": { alias: "Alpha" } },
        { color: "bogus" },
        { "/repo/a": { alias: "Alpha" } },
      ],
    ];
  for (const [name, current, update, expected] of cases) {
    const snapshot = JSON.stringify(current);
    expect(applyRepoAppearanceUpdate(current, "/repo/a", update as never), name).toEqual(expected);
    expect(JSON.stringify(current), name).toBe(snapshot);
  }
});
