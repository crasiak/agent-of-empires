import { describe, expect, it } from "vitest";
import type { SystemHealth } from "./api";
import { appendAgentSamples, knownMetric, sparklinePath, type AgentHistories } from "./sessionMetrics";

const health: SystemHealth = {
  status: "ok",
  cpu_fraction: 0.99,
  memory_used_bytes: 8_000,
  memory_total_bytes: 16_000,
  load_average: null,
  swap_used_bytes: 0,
  swap_total_bytes: 0,
  agent_count: 1,
  proc_count: 3,
  agents: [{ id: "s1", title: "one", cpu_fraction: 0.25, memory_bytes: 100, procs: 3, sandboxed: false }],
};

describe("session metrics history", () => {
  it("bounds each session to 60 actual samples, leaves gaps and evicts departed histories", () => {
    let histories: AgentHistories = new Map();
    for (let tick = 0; tick < 70; tick++) histories = appendAgentSamples(histories, health, tick * 2_000);
    const samples = histories.get("s1")!;
    expect(samples).toHaveLength(60);
    expect(samples[0]).toEqual({ at: 20_000, cpu: 0.25, memory: 100 });
    expect(samples.at(-1)?.at).toBe(138_000);
    histories = appendAgentSamples(histories, null, 140_000);
    expect(histories.get("s1")?.at(-1)).toEqual({ at: 140_000, cpu: null, memory: null });
    expect(samples).toHaveLength(60);
    expect(samples.at(-1)?.at).toBe(138_000);
    for (let tick = 71; tick < 130; tick++)
      histories = appendAgentSamples(histories, { ...health, agents: [] }, tick * 2_000);
    expect(histories.has("s1")).toBe(false);
  });

  it("keeps initial unknowns distinct from genuine zero and rejects invalid readings", () => {
    const cases = [null, undefined, NaN, Infinity, -1];
    for (const value of cases) expect(knownMetric(value)).toBeNull();
    const histories = appendAgentSamples(
      new Map(),
      {
        ...health,
        agents: [
          { ...health.agents[0], cpu_fraction: null, memory_bytes: 0 },
          { ...health.agents[0], id: "s2", cpu_fraction: 0, memory_bytes: null },
        ],
      },
      0,
    );
    expect(histories.get("s1")).toEqual([{ at: 0, cpu: null, memory: 0 }]);
    expect(histories.get("s2")).toEqual([{ at: 0, cpu: 0, memory: null }]);
  });

  it("uses fixed host CPU and auto memory scales, breaking missing and delayed segments", () => {
    const samples = [
      { at: 0, cpu: 0, memory: 0 },
      { at: 2_000, cpu: 0.5, memory: 50 },
      { at: 4_000, cpu: null, memory: null },
      { at: 6_000, cpu: 1, memory: 100 },
      { at: 12_000, cpu: 0.25, memory: 25 },
    ];
    const path = sparklinePath(samples, "cpu", 1);
    expect(path.match(/M/g)).toHaveLength(3);
    expect(path).toContain(",29.00");
    expect(path).toContain(",15.00");
    expect(path).toContain(",1.00");
    expect(path).toContain("M99.00,22.00 L99.00,22.00");
    expect(sparklinePath(samples, "memory", 100)).toBe(path);
    expect(sparklinePath([], "cpu", 1)).toBe("");
    expect(sparklinePath([{ at: 0, cpu: null, memory: null }], "cpu", 1)).toBe("");
    expect(sparklinePath([samples[0], { ...samples[1], at: 200_000 }], "cpu", 1)).toBe("M99.00,15.00 L99.00,15.00");
  });
});
