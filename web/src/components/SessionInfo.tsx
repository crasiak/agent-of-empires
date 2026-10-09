import type { SessionResponse } from "../lib/types";
import { useServerDown } from "../lib/connectionState";
import { useSystemHealthSnapshot } from "../hooks/useSystemHealth";
import { formatBytes, formatPercent } from "../lib/systemHealth";
import { knownMetric, SESSION_METRICS_POLL_MS, sparklinePath, type AgentSample } from "../lib/sessionMetrics";
import { SessionUsage } from "./SessionUsage";

function MetricChart({
  label,
  value,
  detail,
  samples,
  metric,
  ceiling,
  scale,
}: {
  label: string;
  value: string;
  detail: string;
  samples: readonly AgentSample[];
  metric: "cpu" | "memory";
  ceiling: number;
  scale: string;
}) {
  return (
    <div className="min-w-0" title={detail}>
      <div className="flex flex-wrap justify-between gap-x-2 font-mono text-[11px]">
        <span className="text-text-secondary">{label}</span>
        <span className="text-text-primary tabular-nums" data-testid={`session-${metric}-value`}>
          {value}
        </span>
      </div>
      <svg
        viewBox="0 0 100 30"
        preserveAspectRatio="none"
        className="h-7 w-full text-brand-500"
        role="img"
        aria-label={`${label} history, ${value}; ${scale}`}
      >
        <title>{detail}</title>
        <path d="M1,29 L99,29" className="stroke-surface-700" fill="none" vectorEffect="non-scaling-stroke" />
        <path
          data-testid={`session-${metric}-history`}
          d={sparklinePath(samples, metric, ceiling)}
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          fill="none"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
      <div className="text-[10px] text-text-secondary">{scale}</div>
    </div>
  );
}

/** Selected-session data stays in layout, never on top of terminal cells. */
export function SessionInfo({ session }: { session: SessionResponse }) {
  const disconnected = useServerDown();
  const stopped = session.status === "Stopped" || session.dormant || !!session.archived_at || !!session.trashed_at;
  const snoozed = !!session.snoozed_until;
  const running = !stopped && !snoozed && !["Unknown", "Deleting", "Creating"].includes(session.status);
  const enabled = running && !disconnected;
  const snapshot = useSystemHealthSnapshot(enabled, SESSION_METRICS_POLL_MS);
  const agent = enabled ? snapshot.health?.agents.find((row) => row.id === session.id) : undefined;
  const cpu = knownMetric(agent?.cpu_fraction);
  const memory = knownMetric(agent?.memory_bytes);
  const samples = enabled && snapshot.health ? (snapshot.histories.get(session.id) ?? []) : [];
  const memoryReadings = samples.map((sample) => sample.memory).filter((value) => value !== null);
  const memoryCeiling = memoryReadings.length ? Math.max(...memoryReadings) : null;
  const sandboxed = agent?.sandboxed ?? session.is_sandboxed;
  const scope = sandboxed ? "Session container" : "Agent process tree";
  const state = disconnected
    ? "Disconnected"
    : stopped
      ? "Stopped"
      : snoozed
        ? "Snoozed"
        : !agent
          ? "Unavailable"
          : null;

  return (
    <section
      aria-label={`Session info: ${session.title}`}
      data-testid="session-info"
      className="shrink-0 max-h-[min(14rem,35%)] overflow-y-auto border-b border-surface-700/60 bg-surface-900 px-3 py-2"
    >
      <div className="mb-1 flex min-w-0 items-baseline gap-2 text-[11px] text-text-secondary">
        <span className="truncate font-mono text-text-primary">{session.title}</span>
        <span className="ml-auto shrink-0">
          {scope}
          {state ? ` · ${state}` : ""}
        </span>
      </div>
      <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
        <SessionUsage key={session.id} session={session} />
        <div className="grid min-w-0 flex-[1_1_16rem] grid-cols-2 gap-3">
          <MetricChart
            label="CPU (% host)"
            value={formatPercent(cpu)}
            detail={`${scope} CPU as a percentage of total host CPU capacity, not single-core utilization. Up to 60 samples at about 2 seconds; gaps are unknown.`}
            samples={samples}
            metric="cpu"
            ceiling={1}
            scale="0–100% · ~2 min"
          />
          <MetricChart
            label={sandboxed ? "Container memory" : "RSS"}
            value={memory === null ? "?" : formatBytes(memory)}
            detail={
              sandboxed
                ? "Selected session container memory, not host resident memory. History autoscales."
                : "Resident memory summed over the selected agent process tree, not host memory. History autoscales."
            }
            samples={samples}
            metric="memory"
            ceiling={memoryCeiling ?? 1}
            scale={`Auto 0–${memoryCeiling === null ? "?" : formatBytes(memoryCeiling)} · ~2 min`}
          />
        </div>
      </div>
    </section>
  );
}
