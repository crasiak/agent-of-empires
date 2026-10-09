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
import { useServerDown } from "../lib/connectionState";

const POLL_MS = 5_000;
const LEDGER_POLL_MS = 30_000;

function currentUsageLines(summary: UsageSummary, createdAt: string): string[] {
  return usageLines(summary, createdAt, Date.now());
}

function currentDriftLine(view: LedgerRunView) {
  return driftLine(view, Date.now());
}

export function SessionUsage({ session }: { session: SessionResponse }) {
  const disconnected = useServerDown();
  const usageEnabled = useUsageOverlayEnabled();
  const ledgerEnabled = useLedgerOverlayEnabled();
  const headroomEnabled = useHeadroomOverlayEnabled();
  const ledgerWanted = ledgerEnabled || headroomEnabled;
  const [usageResult, setUsageResult] = useState<{ id: string; value: UsageSummary | null } | null>(null);
  const [ledgerResult, setLedgerResult] = useState<{ id: string; value: LedgerRunView | null } | null>(null);
  const summary = usageResult?.id === session.id ? usageResult.value : null;
  const ledger = ledgerResult?.id === session.id ? ledgerResult.value : null;

  useEffect(() => {
    if (!usageEnabled || disconnected) return;
    let cancelled = false;
    let inFlight = false;
    const load = () => {
      if (inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      void fetchSessionUsage(session.id)
        .catch(() => null)
        .then((next) => {
          if (!cancelled) setUsageResult({ id: session.id, value: next });
        })
        .finally(() => {
          inFlight = false;
        });
    };
    load();
    const timer = window.setInterval(load, POLL_MS);
    document.addEventListener("visibilitychange", load);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", load);
    };
  }, [usageEnabled, disconnected, session.id]);

  useEffect(() => {
    if (!ledgerWanted || disconnected) return;
    let cancelled = false;
    let inFlight = false;
    const load = () => {
      if (inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      void fetchSessionLedgerRun(session.id)
        .catch(() => null)
        .then((next) => {
          if (!cancelled) setLedgerResult({ id: session.id, value: next });
        })
        .finally(() => {
          inFlight = false;
        });
    };
    load();
    const timer = window.setInterval(load, LEDGER_POLL_MS);
    document.addEventListener("visibilitychange", load);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", load);
    };
  }, [ledgerWanted, disconnected, session.id]);

  const showUsage = usageEnabled && !!summary?.tracked;
  const drift = ledgerEnabled && ledger ? currentDriftLine(ledger) : null;
  const headroom = headroomEnabled && ledger ? headroomLine(ledger) : null;
  if (!showUsage && !drift && !headroom) return null;
  return (
    <div
      className="min-w-0 flex-[2_1_14rem] space-y-1 font-mono text-[11px] text-text-secondary"
      data-testid="session-usage"
      title={showUsage ? "Context resets: clears + compactions + resumes" : "Ledger run"}
    >
      {showUsage && summary && (
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <span className="whitespace-nowrap">
            <strong className="text-lg font-semibold text-status-error tabular-nums">{summary.resets}</strong> resets
          </span>
          {currentUsageLines(summary, session.created_at).map((line) => (
            <span key={line}>{line}</span>
          ))}
        </div>
      )}
      {drift && (
        <div data-testid="ledger-drift-line" className="whitespace-pre-wrap break-words">
          {drift.text}
        </div>
      )}
      {headroom && (
        <div data-testid="ledger-headroom-line" className="whitespace-pre-wrap break-words">
          {headroom}
        </div>
      )}
    </div>
  );
}
