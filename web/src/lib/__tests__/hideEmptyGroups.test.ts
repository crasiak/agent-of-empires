import { describe, expect, it } from "vitest";
import { parseHideEmptyGroups } from "../hideEmptyGroups";

describe("empty group visibility settings", () => {
  it.each([
    [undefined, true],
    [null, true],
    [{}, true],
    [{ session: { hide_empty_groups: "false" } }, true],
    [{ session: { hide_empty_groups: true } }, true],
    [{ session: { hide_empty_groups: false } }, false],
  ])("resolves %j to %s", (settings, expected) => {
    expect(parseHideEmptyGroups(settings)).toBe(expected);
  });
});
