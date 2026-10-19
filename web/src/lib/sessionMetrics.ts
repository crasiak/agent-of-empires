import type { SystemHealth } from "./api";

export const SESSION_METRICS_POLL_MS = 2_000;
export const SESSION_METRICS_SAMPLES = 60;

export interface AgentSample {
  at: number;
  cpu: number | null;
  memory: number | null;
}

export type AgentHistories = ReadonlyMap<string, readonly AgentSample[]>;

export function knownMetric(value: number | null | undefined): number | null {
  return value != null && Number.isFinite(value) && value >= 0 ? value : null;
}

/** Called once per sampling tick, never by a render. Missing agents leave gaps. */
export function appendAgentSamples(histories: AgentHistories, health: SystemHealth | null, at: number): AgentHistories {
  const agents = new Map(health?.agents.map((agent) => [agent.id, agent]));
  const next = new Map<string, readonly AgentSample[]>();
  for (const id of new Set([...histories.keys(), ...agents.keys()])) {
    const agent = agents.get(id);
    const samples = [
      ...(histories.get(id) ?? []).slice(-(SESSION_METRICS_SAMPLES - 1)),
      { at, cpu: knownMetric(agent?.cpu_fraction), memory: knownMetric(agent?.memory_bytes) },
    ];
    // Drop departed sessions after their last known reading leaves the window.
    if (agent || samples.some((sample) => sample.cpu !== null || sample.memory !== null)) next.set(id, samples);
  }
  return next;
}

/** Fixed two-minute time axis; a missing or delayed sample breaks the line. */
export function sparklinePath(samples: readonly AgentSample[], metric: "cpu" | "memory", ceiling: number): string {
  const end = samples.at(-1)?.at ?? 0;
  const windowMs = (SESSION_METRICS_SAMPLES - 1) * SESSION_METRICS_POLL_MS;
  let previousAt: number | null = null;
  const path: string[] = [];
  for (const sample of samples) {
    const value = sample[metric];
    if (value === null || end - sample.at > windowMs) {
      previousAt = null;
      continue;
    }
    const x = 1 + 98 * (1 - (end - sample.at) / windowMs);
    const y = 29 - 28 * Math.min(value / Math.max(ceiling, 1), 1);
    const connected = previousAt !== null && sample.at - previousAt <= SESSION_METRICS_POLL_MS * 2;
    // A zero-length segment with a round cap keeps isolated readings visible.
    path.push(`${connected ? "L" : "M"}${x.toFixed(2)},${y.toFixed(2)}`);
    if (!connected) path.push(`L${x.toFixed(2)},${y.toFixed(2)}`);
    previousAt = sample.at;
  }
  return path.join(" ");
}
