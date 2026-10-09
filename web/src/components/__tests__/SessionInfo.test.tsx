// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import { makeSession } from "./fixtures";
import { SessionInfo } from "../SessionInfo";
import { SidebarSystemHealth } from "../SystemHealthStrip";
import { SystemHealthEnabledContext } from "../../lib/systemHealth";
import { setServerDown } from "../../lib/connectionState";
import { getSystemHealthSnapshot } from "../../lib/systemHealthStore";
import { fetchSystemHealth, type SystemHealth } from "../../lib/api";

vi.mock("../../lib/api", () => ({
  fetchSystemHealth: vi.fn(),
  fetchSessionUsage: vi.fn(async () => null),
  fetchSessionLedgerRun: vi.fn(async () => null),
}));

const fetchHealth = vi.mocked(fetchSystemHealth);
const session = makeSession({ id: "s1", title: "Selected", status: "Running" });
const health = (overrides: Partial<SystemHealth> = {}): SystemHealth => ({
  status: "ok",
  cpu_fraction: 0.99,
  memory_used_bytes: 24 * 1024 ** 3,
  memory_total_bytes: 32 * 1024 ** 3,
  load_average: null,
  swap_used_bytes: 0,
  swap_total_bytes: 0,
  agent_count: 2,
  proc_count: 8,
  agents: [
    { id: "s1", title: "Selected", cpu_fraction: 0.12, memory_bytes: 384 * 1024 ** 2, procs: 3, sandboxed: false },
    { id: "s2", title: "Other", cpu_fraction: 0.85, memory_bytes: 2 * 1024 ** 3, procs: 5, sandboxed: false },
  ],
  ...overrides,
});
const advance = async (ms = 0) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
const current = (metric: "cpu" | "memory") => screen.getByTestId(`session-${metric}-value`).textContent;
const path = (metric: "cpu" | "memory") => screen.getByTestId(`session-${metric}-history`).getAttribute("d");

beforeEach(() => {
  vi.useFakeTimers();
  fetchHealth.mockResolvedValue(health());
});
afterEach(() => {
  cleanup();
  setServerDown(false);
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("SessionInfo", () => {
  it("uses selected agent attribution, not host totals, and records only polling ticks across renders and switches", async () => {
    const { rerender } = render(<SessionInfo session={session} />);
    await advance();
    expect(current("cpu")).toBe("12%");
    expect(current("memory")).toBe("384M");
    expect(screen.getByText("CPU (% host)")).toBeTruthy();
    expect(screen.getByText("RSS")).toBeTruthy();
    expect(screen.getByText("Auto 0–384M · ~2 min")).toBeTruthy();
    const firstPath = path("cpu");
    rerender(<SessionInfo session={{ ...session, title: "Renamed" }} />);
    expect(path("cpu")).toBe(firstPath);
    expect(fetchHealth).toHaveBeenCalledTimes(1);
    await advance(2_000);
    expect(getSystemHealthSnapshot().histories.get("s1")).toHaveLength(2);
    expect(path("cpu")).not.toBe(firstPath);
    rerender(<SessionInfo session={{ ...session, id: "s2", title: "Other" }} />);
    expect(current("cpu")).toBe("85%");
    expect(current("memory")).toBe("2G");
    rerender(<SessionInfo session={{ ...session, id: "missing" }} />);
    expect(current("cpu")).toBe("?");
    expect(current("memory")).toBe("?");
    expect(path("cpu")).toBe("");
    rerender(<SessionInfo session={session} />);
    expect(getSystemHealthSnapshot().histories.get("s1")).toHaveLength(2);
    expect(current("cpu")).toBe("12%");
  });

  it("shares the sidebar's single request pipeline and downsamples faster polls to two-second history", async () => {
    const { rerender } = render(
      <SystemHealthEnabledContext.Provider value={true}>
        <SidebarSystemHealth />
        <SessionInfo session={session} />
      </SystemHealthEnabledContext.Provider>,
    );
    await advance();
    expect(fetchHealth).toHaveBeenCalledTimes(1);
    await advance(1_999);
    expect(fetchHealth).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(fetchHealth).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByTitle("System health"));
    await advance();
    const afterOpen = fetchHealth.mock.calls.length;
    await advance(500);
    expect(fetchHealth).toHaveBeenCalledTimes(afterOpen + 1);
    expect(getSystemHealthSnapshot().histories.get("s1")).toHaveLength(2);
    await advance(2_000);
    expect(getSystemHealthSnapshot().histories.get("s1")).toHaveLength(3);
    rerender(
      <SystemHealthEnabledContext.Provider value={true}>
        <SidebarSystemHealth />
      </SystemHealthEnabledContext.Provider>,
    );
    // Closing detail returns the only remaining consumer to the idle cadence.
    fireEvent.click(screen.getByTitle("System health"));
    await advance();
    const beforeIdle = fetchHealth.mock.calls.length;
    await advance(14_999);
    expect(fetchHealth).toHaveBeenCalledTimes(beforeIdle);
    await advance(1);
    expect(fetchHealth).toHaveBeenCalledTimes(beforeIdle + 1);
  });

  it("inserts a failure gap even between faster sidebar polls", async () => {
    render(
      <SystemHealthEnabledContext.Provider value={true}>
        <SidebarSystemHealth />
        <SessionInfo session={session} />
      </SystemHealthEnabledContext.Provider>,
    );
    await advance();
    fireEvent.click(screen.getByTitle("System health"));
    await advance();
    fetchHealth.mockResolvedValueOnce(null);
    await advance(500);
    expect(current("cpu")).toBe("?");
    expect(
      getSystemHealthSnapshot()
        .histories.get("s1")
        ?.map((sample) => sample.cpu),
    ).toEqual([0.12, null]);
    await advance(2_000);
    expect(current("cpu")).toBe("12%");
    expect(
      getSystemHealthSnapshot()
        .histories.get("s1")
        ?.map((sample) => sample.cpu),
    ).toEqual([0.12, null, 0.12]);
    expect(path("cpu")?.match(/M/g)).toHaveLength(2);
    fetchHealth.mockResolvedValue(null);
    await advance(30_000);
    const history = getSystemHealthSnapshot().histories.get("s1");
    expect(history?.some((sample) => sample.cpu === 0.12)).toBe(true);
    expect(history!.length).toBeLessThan(25);
  });

  it("distinguishes warm-up and missing metrics from real zeros, including container memory", async () => {
    const agent = health().agents[0];
    fetchHealth.mockResolvedValue(
      health({ agents: [{ ...agent, cpu_fraction: null, memory_bytes: null, sandboxed: true }] }),
    );
    render(<SessionInfo session={{ ...session, is_sandboxed: true }} />);
    await advance();
    expect(screen.getByText("Container memory")).toBeTruthy();
    expect(screen.getByText("Session container")).toBeTruthy();
    expect(screen.queryByText("RSS")).toBeNull();
    expect(current("cpu")).toBe("?");
    expect(current("memory")).toBe("?");
    expect(path("cpu")).toBe("");
    fetchHealth.mockResolvedValue(
      health({ agents: [{ ...agent, cpu_fraction: 0, memory_bytes: 0, sandboxed: true }] }),
    );
    await advance(2_000);
    expect(current("cpu")).toBe("0%");
    expect(current("memory")).toBe("0B");
    expect(screen.getByText("Auto 0–0B · ~2 min")).toBeTruthy();
    expect(path("cpu")).toContain(",29.00");
  });

  it("clears live readings on failure, disconnect and stop, and resumes only from a fresh sample", async () => {
    const { rerender } = render(<SessionInfo session={session} />);
    await advance();
    expect(current("cpu")).toBe("12%");
    fetchHealth.mockRejectedValueOnce(new Error("network"));
    await advance(2_000);
    expect(current("cpu")).toBe("?");
    expect(path("cpu")).toBe("");
    expect(getSystemHealthSnapshot().histories.get("s1")?.at(-1)?.cpu).toBeNull();
    await advance(2_000);
    expect(current("cpu")).toBe("12%");
    act(() => setServerDown(true));
    expect(current("cpu")).toBe("?");
    expect(screen.getByText("Agent process tree · Disconnected")).toBeTruthy();
    const offlineCalls = fetchHealth.mock.calls.length;
    await advance(30_000);
    expect(fetchHealth).toHaveBeenCalledTimes(offlineCalls);
    act(() => setServerDown(false));
    expect(current("cpu")).toBe("?");
    await advance();
    expect(current("cpu")).toBe("12%");
    rerender(<SessionInfo session={{ ...session, status: "Stopped" }} />);
    expect(current("cpu")).toBe("?");
    expect(path("memory")).toBe("");
    expect(screen.getByText("Agent process tree · Stopped")).toBeTruthy();
    const stoppedCalls = fetchHealth.mock.calls.length;
    await advance(30_000);
    expect(fetchHealth).toHaveBeenCalledTimes(stoppedCalls);
    rerender(<SessionInfo session={{ ...session, snoozed_until: new Date(Date.now() + 60_000).toISOString() }} />);
    expect(screen.getByText("Agent process tree · Snoozed")).toBeTruthy();
    await advance(30_000);
    expect(fetchHealth).toHaveBeenCalledTimes(stoppedCalls);
    expect(current("cpu")).toBe("?");
    rerender(<SessionInfo session={session} />);
    await advance();
    expect(current("cpu")).toBe("12%");
  });

  it("pauses hidden tabs and clears old readings until the foreground refresh lands", async () => {
    render(<SessionInfo session={session} />);
    await advance();
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(current("memory")).toBe("?");
    const beforeHidden = fetchHealth.mock.calls.length;
    await advance(20_000);
    expect(fetchHealth).toHaveBeenCalledTimes(beforeHidden);
    hidden.mockReturnValue(false);
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(current("cpu")).toBe("?");
    await advance();
    expect(current("cpu")).toBe("12%");
    expect(fetchHealth).toHaveBeenCalledTimes(beforeHidden + 1);
  });

  it("expires a stalled request without stacking requests and ignores its result after disconnect", async () => {
    render(
      <StrictMode>
        <SessionInfo session={session} />
      </StrictMode>,
    );
    await advance();
    expect(current("cpu")).toBe("12%");
    let settle!: (value: SystemHealth) => void;
    const pending = () =>
      new Promise<SystemHealth>((resolve) => {
        settle = resolve;
      });
    fetchHealth.mockImplementationOnce(pending);
    await advance(2_000);
    const pendingCalls = fetchHealth.mock.calls.length;
    await advance(10_000);
    expect(current("cpu")).toBe("?");
    expect(fetchHealth).toHaveBeenCalledTimes(pendingCalls);
    await act(async () => settle(health()));
    expect(current("cpu")).toBe("12%");
    expect(fetchHealth).toHaveBeenCalledTimes(pendingCalls);
    fetchHealth.mockImplementationOnce(pending);
    await advance(2_000);
    act(() => setServerDown(true));
    await act(async () => settle(health()));
    expect(current("cpu")).toBe("?");
    expect(fetchHealth).toHaveBeenCalledTimes(pendingCalls + 1);
  });
});
