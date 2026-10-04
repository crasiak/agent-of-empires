import { sessionFlagGate } from "./sessionFlagGate";
import { formatDurationShort } from "./usage";

/** Mirrors `crate::ledger_run::LedgerRunView` (snake_case on the wire). */
export interface LedgerChange {
  kind: string;
  name: string;
  change: string;
}

export interface LedgerRuntimeChange {
  kind: string;
  change: string;
}

export interface LedgerGeneration {
  state: string;
  launched: string;
  current: string;
  current_since: string;
  drift: LedgerChange[];
  runtime_drift: LedgerRuntimeChange[];
}

export interface HeadroomTotals {
  requests: number;
  input_tokens_before: number;
  input_tokens_after: number;
  saved_tokens: number;
  model: string;
  estimated_cents: number | null;
}

export interface LedgerRunView {
  run_id: string;
  runs: number;
  generation: LedgerGeneration | null;
  headroom_total: HeadroomTotals | null;
  headroom_now: HeadroomTotals | null;
  error: string | null;
}

/** On by default, matching `session.show_ledger_overlay`. */
const ledgerGate = sessionFlagGate("show_ledger_overlay", true);
export const LedgerOverlayEnabledContext = ledgerGate.Context;
export const parseLedgerOverlayEnabled = ledgerGate.parse;
export const useLedgerOverlayEnabled = ledgerGate.use;

/** On by default, matching `session.show_headroom_overlay`. */
const headroomGate = sessionFlagGate("show_headroom_overlay", true);
export const HeadroomOverlayEnabledContext = headroomGate.Context;
export const parseHeadroomOverlayEnabled = headroomGate.parse;
export const useHeadroomOverlayEnabled = headroomGate.use;

export { fetchSessionLedgerRun } from "./api";

const MAX_SKILLS = 6;

function marker(change: string): string {
  switch (change) {
    case "older":
      return "↓";
    case "newer":
      return "↑";
    case "extra":
      return "+";
    case "missing":
      return "-";
    default:
      return "~";
  }
}

const short = (id: string) => [...id].slice(0, 4).join("");

/** Mirrors the TUI's `drift_line`. */
export function driftLine(view: LedgerRunView, now: number): { text: string; dim: boolean } | null {
  if (view.error !== null) return { text: `ledger ? ${view.error}`, dim: true };
  const g = view.generation;
  if (!g) return null;
  if (g.state === "current") return { text: `ledger ${short(g.launched)} current`, dim: false };
  const since = Date.parse(g.current_since);
  const age = Number.isNaN(since) ? "?" : formatDurationShort(now - since);
  const segments = [`ledger ${g.state} ${short(g.launched)}↔${short(g.current)} ${age}`];
  const skills = g.drift.filter((c) => c.kind === "skill");
  const named = skills.slice(0, MAX_SKILLS).map((c) => `${marker(c.change)}${c.name}`);
  if (skills.length > MAX_SKILLS) named.push(`+${skills.length - MAX_SKILLS} more`);
  if (named.length > 0) segments.push(named.join(" "));
  const kinds: string[] = [];
  for (const c of g.drift) if (c.kind !== "skill" && !kinds.includes(c.kind)) kinds.push(c.kind);
  const counted = kinds.map((kind) => {
    const changes = g.drift.filter((c) => c.kind === kind).map((c) => c.change);
    const first = changes[0] ?? "";
    const mark = changes.every((c) => c === first) ? marker(first) : "~";
    return `${mark}${changes.length} ${kind}`;
  });
  counted.push(...g.runtime_drift.map((c) => `${marker(c.change)}${c.kind}`));
  if (counted.length > 0) segments.push(counted.join(" "));
  return { text: segments.join("  "), dim: false };
}

/** Mirrors the TUI's `compact_count`: rounded down, integer math. */
export function compactCount(n: number): string {
  if (n < 1_000) return String(n);
  if (n < 1_000_000) {
    const tenths = Math.floor(n / 100);
    return `${Math.floor(tenths / 10)}.${tenths % 10}k`;
  }
  const tenths = Math.floor(n / 100_000);
  return `${Math.floor(tenths / 10)}.${tenths % 10}M`;
}

/** Columns a drift row may take before its segments wrap; matches the TUI. */
export const DRIFT_WRAP_COLUMNS = 48;

/** Mirrors the TUI's `wrap_segments`: packs the drift line's segments (joined
 *  by two spaces) into rows of at most `max` columns, never splitting one. */
export function wrapSegments(text: string, max: number): string[] {
  const rows: string[] = [];
  for (const segment of text.split("  ")) {
    const last = rows.at(-1);
    if (last !== undefined && [...last].length + 2 + [...segment].length <= max) {
      rows[rows.length - 1] = `${last}  ${segment}`;
    } else {
      rows.push(segment);
    }
  }
  return rows;
}

/** Mirrors the TUI's `headroom_line`. */
export function headroomLine(view: LedgerRunView): string | null {
  if (view.error !== null) return null;
  const total = view.headroom_total;
  if (!total) return null;
  const percent =
    total.input_tokens_before === 0 ? 0 : Math.floor((total.saved_tokens * 100) / total.input_tokens_before);
  const parts = [`hr ${compactCount(total.saved_tokens)} saved`, `${percent}%`];
  if (total.estimated_cents !== null) {
    const cents = total.estimated_cents;
    parts.push(`~$${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`);
  }
  let line = parts.join(" · ");
  if (view.runs > 1 && view.headroom_now) line += `  (now ${compactCount(view.headroom_now.saved_tokens)})`;
  return line;
}
