import { useEffect, useState } from "react";
import type { SessionResponse } from "../lib/types";
import {
  driftLine,
  fetchSessionLedgerRun,
  headroomLine,
  useHeadroomOverlayEnabled,
  useLedgerOverlayEnabled,
  type LedgerRunView,
} from "../lib/ledgerRun";
import { fetchSessionUsage, usageLines, useUsageOverlayEnabled, type UsageSummary } from "../lib/usage";

const POLL_MS = 5_000;
const LEDGER_POLL_MS = 30_000;

/** Wraps the impure `Date.now()` read outside the component body, matching
 *  `ConnectedDevices`' `relativeTime` helper. */
function currentUsageLines(summary: UsageSummary, createdAt: string): string[] {
  return usageLines(summary, createdAt, Date.now());
}

function currentDriftLine(view: LedgerRunView) {
  return driftLine(view, Date.now());
}

/** The session's overlay stack over the terminal's top-right corner: the
 *  context-reset count, big and red, then the Ledger drift and Headroom lines. */
export function UsageOverlay({ session }: { session: SessionResponse }) {
  const usageEnabled = useUsageOverlayEnabled();
  const ledgerEnabled = useLedgerOverlayEnabled();
  const headroomEnabled = useHeadroomOverlayEnabled();
  const ledgerWanted = ledgerEnabled || headroomEnabled;
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [ledger, setLedger] = useState<LedgerRunView | null>(null);

  useEffect(() => {
    if (!usageEnabled) return;
    let cancelled = false;
    const load = () => {
      if (document.visibilityState !== "visible") return;
      void fetchSessionUsage(session.id).then((next) => {
        if (!cancelled) setSummary(next);
      });
    };
    load();
    const timer = window.setInterval(load, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [usageEnabled, session.id]);

  useEffect(() => {
    if (!ledgerWanted) return;
    let cancelled = false;
    const load = () => {
      if (document.visibilityState !== "visible") return;
      void fetchSessionLedgerRun(session.id).then((next) => {
        if (!cancelled) setLedger(next);
      });
    };
    load();
    const timer = window.setInterval(load, LEDGER_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [ledgerWanted, session.id]);

  const showUsage = usageEnabled && !!summary?.tracked;
  const drift = ledgerEnabled && ledger ? currentDriftLine(ledger) : null;
  const headroom = headroomEnabled && ledger ? headroomLine(ledger) : null;
  if (!showUsage && !drift && !headroom) return null;
  return (
    <div
      className="pointer-events-none absolute top-2 right-3 z-10 hidden rounded-md bg-surface-900/25 px-2 py-1 text-right sm:block"
      data-testid="usage-overlay"
      title={showUsage ? "Context resets: clears + compactions + resumes" : "Ledger run"}
    >
      {showUsage && summary && (
        <>
          <div className="text-3xl font-black leading-none text-status-error tabular-nums opacity-90">
            {summary.resets}
          </div>
          {currentUsageLines(summary, session.created_at).map((line) => (
            <div key={line} className="mt-1 font-mono text-[10px] text-text-secondary opacity-90">
              {line}
            </div>
          ))}
        </>
      )}
      {drift && (
        <div
          data-testid="ledger-drift-line"
          className={`mt-1 whitespace-pre font-mono text-[10px] opacity-90 ${drift.dim ? "text-text-dim" : "text-text-secondary"}`}
        >
          {drift.text}
        </div>
      )}
      {headroom && (
        <div
          data-testid="ledger-headroom-line"
          className="mt-1 whitespace-pre font-mono text-[10px] text-text-secondary opacity-90"
        >
          {headroom}
        </div>
      )}
    </div>
  );
}
