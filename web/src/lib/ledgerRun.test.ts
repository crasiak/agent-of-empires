import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  compactCount,
  driftLine,
  headroomLine,
  parseHeadroomOverlayEnabled,
  parseLedgerOverlayEnabled,
  type LedgerRunView,
} from "./ledgerRun";

interface Case {
  name: string;
  now: string;
  view: LedgerRunView;
  lines: string[];
}

// Shared with the TUI's ledger_overlay tests.
const cases: Case[] = JSON.parse(
  readFileSync(new URL("../../../tests/fixtures/ledger-run/cases.json", import.meta.url), "utf8"),
);

describe("ledger overlay text", () => {
  it.each(cases.map((c) => [c.name, c] as const))("matches the TUI for %s", (_name, c) => {
    const now = Date.parse(c.now);
    const lines = [driftLine(c.view, now)?.text, headroomLine(c.view)].filter(
      (line): line is string => typeof line === "string",
    );
    expect(lines).toEqual(c.lines);
  });

  it("dims only the error line", () => {
    for (const c of cases) {
      expect(driftLine(c.view, Date.parse(c.now))?.dim ?? false).toBe(c.view.error !== null);
    }
  });

  it("rounds counts down with integer math", () => {
    expect([0, 999, 1_000, 26_500, 999_999, 1_250_000, 1_132_000].map(compactCount)).toEqual([
      "0",
      "999",
      "1.0k",
      "26.5k",
      "999.9k",
      "1.2M",
      "1.1M",
    ]);
  });

  it("defaults both lines on", () => {
    expect(parseLedgerOverlayEnabled(null)).toBe(true);
    expect(parseHeadroomOverlayEnabled(null)).toBe(true);
    expect(parseLedgerOverlayEnabled({ session: { show_ledger_overlay: false } })).toBe(false);
    expect(parseHeadroomOverlayEnabled({ session: { show_headroom_overlay: false } })).toBe(false);
  });
});
