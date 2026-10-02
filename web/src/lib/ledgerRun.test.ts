import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  compactCount,
  DRIFT_WRAP_COLUMNS,
  driftLine,
  headroomLine,
  parseHeadroomOverlayEnabled,
  parseLedgerOverlayEnabled,
  wrapSegments,
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

describe("drift wrapping", () => {
  // Same cases as the TUI's long_drift_lines_wrap_at_segment_boundaries.
  const text = "ledger behind d861↔f00d 3h0m  +caveman -dataviz ↓tdd  ↓2 settings ↓renderer";

  it("packs segments into rows of at most 48 columns", () => {
    expect(wrapSegments(text, DRIFT_WRAP_COLUMNS)).toEqual([
      "ledger behind d861↔f00d 3h0m",
      "+caveman -dataviz ↓tdd  ↓2 settings ↓renderer",
    ]);
    expect(wrapSegments("ledger d861 current", DRIFT_WRAP_COLUMNS)).toEqual(["ledger d861 current"]);
    expect(wrapSegments("a  b  c", 4)).toEqual(["a  b", "c"]);
  });

  it("never loses or reorders a segment", () => {
    expect(wrapSegments(text, DRIFT_WRAP_COLUMNS).join("  ")).toBe(text);
  });
});

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
    expect(
      parseHeadroomOverlayEnabled({
        session: { show_headroom_overlay: false },
      }),
    ).toBe(false);
  });
});
