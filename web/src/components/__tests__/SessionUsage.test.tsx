// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

import type { UsageSummary } from "../../lib/usage";
import { makeSession } from "./fixtures";

vi.mock("../../lib/api", () => ({ fetchSessionUsage: vi.fn(), fetchSessionLedgerRun: vi.fn() }));

import { fetchSessionLedgerRun, fetchSessionUsage } from "../../lib/api";
import { HeadroomOverlayEnabledContext, LedgerOverlayEnabledContext, type LedgerRunView } from "../../lib/ledgerRun";
import { UsageOverlayEnabledContext } from "../../lib/usage";
import { SessionUsage } from "../SessionUsage";
import { setServerDown } from "../../lib/connectionState";

const mockFetch = vi.mocked(fetchSessionUsage);

function summary(overrides: Partial<UsageSummary> = {}): UsageSummary {
  return {
    resets: 7,
    clears: 5,
    compactions: 2,
    compactionsAuto: 1,
    compactionsManual: 1,
    resumes: 0,
    prompts: 20,
    turns: 18,
    turnErrors: 0,
    contextStartedAt: "2026-09-29T11:00:00Z",
    contextPrompts: 4,
    contextTurns: 3,
    trackedSince: "2026-09-28T00:00:00Z",
    lastEventAt: "2026-09-29T11:59:00Z",
    tracked: true,
    ...overrides,
  };
}

const session = makeSession({ id: "s1", created_at: "2026-09-28T00:00:00Z" });

beforeEach(() => {
  vi.mocked(fetchSessionLedgerRun).mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  setServerDown(false);
  vi.clearAllMocks();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("SessionUsage", () => {
  it("shows the reset count once a summary arrives", async () => {
    mockFetch.mockResolvedValue(summary());

    render(
      <UsageOverlayEnabledContext.Provider value={true}>
        <SessionUsage session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    expect(await screen.findByText("7")).toBeTruthy();
    expect(screen.getByTestId("session-usage")).toBeTruthy();
  });

  it("renders nothing while the setting is off", () => {
    render(
      <UsageOverlayEnabledContext.Provider value={false}>
        <SessionUsage session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    expect(screen.queryByTestId("session-usage")).toBeNull();
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("renders nothing for a lifecycle-only session", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));

    render(
      <UsageOverlayEnabledContext.Provider value={true}>
        <SessionUsage session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    await vi.waitFor(() => expect(mockFetch).toHaveBeenCalled());
    expect(screen.queryByTestId("session-usage")).toBeNull();
  });
});

const ledgerView: LedgerRunView = {
  run_id: "run_00000000000000000000000000000006",
  runs: 2,
  generation: {
    state: "current",
    launched: "d861a7c0e5f1",
    current: "d861a7c0e5f1",
    current_since: "2026-10-01T08:00:00Z",
    drift: [],
    runtime_drift: [],
  },
  headroom_total: {
    requests: 105,
    input_tokens_before: 1_100_000,
    input_tokens_after: 773_500,
    saved_tokens: 326_500,
    model: "claude-opus-5-5",
    estimated_cents: 100,
  },
  headroom_now: {
    requests: 5,
    input_tokens_before: 100_000,
    input_tokens_after: 73_500,
    saved_tokens: 26_500,
    model: "claude-opus-5-5",
    estimated_cents: 10,
  },
  error: null,
};

function renderUsage({ usage = true, ledger = true, headroom = true } = {}) {
  return render(
    <UsageOverlayEnabledContext.Provider value={usage}>
      <LedgerOverlayEnabledContext.Provider value={ledger}>
        <HeadroomOverlayEnabledContext.Provider value={headroom}>
          <SessionUsage session={session} />
        </HeadroomOverlayEnabledContext.Provider>
      </LedgerOverlayEnabledContext.Provider>
    </UsageOverlayEnabledContext.Provider>,
  );
}

describe("SessionUsage Ledger lines", () => {
  it("does not show the previous session's usage or Ledger while the selected session loads", async () => {
    mockFetch.mockResolvedValueOnce(summary());
    vi.mocked(fetchSessionLedgerRun).mockResolvedValueOnce(ledgerView);
    const { rerender } = render(<SessionUsage session={session} />);
    await screen.findByTestId("ledger-drift-line");
    expect(screen.getByText("7")).toBeTruthy();
    let finishUsage!: (value: UsageSummary) => void;
    let finishLedger!: (value: LedgerRunView | null) => void;
    mockFetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishUsage = resolve;
        }),
    );
    vi.mocked(fetchSessionLedgerRun).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishLedger = resolve;
        }),
    );
    rerender(<SessionUsage session={{ ...session, id: "s2" }} />);
    expect(screen.queryByTestId("session-usage")).toBeNull();
    await act(async () => {
      finishUsage(summary({ resets: 3 }));
      finishLedger(null);
    });
    expect(screen.getByText("3")).toBeTruthy();
    expect(screen.queryByText("7")).toBeNull();
    expect(screen.queryByTestId("ledger-drift-line")).toBeNull();
  });

  it("ignores cancelled responses after a session switch", async () => {
    let finishUsage!: (value: UsageSummary) => void;
    let finishLedger!: (value: LedgerRunView) => void;
    mockFetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishUsage = resolve;
        }),
    );
    vi.mocked(fetchSessionLedgerRun).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishLedger = resolve;
        }),
    );
    const { rerender } = render(<SessionUsage session={session} />);
    mockFetch.mockResolvedValueOnce(summary({ resets: 3 }));
    rerender(<SessionUsage session={{ ...session, id: "s2" }} />);
    await screen.findByText("3");
    await act(async () => {
      finishUsage(summary());
      finishLedger(ledgerView);
    });
    expect(screen.getByText("3")).toBeTruthy();
    expect(screen.queryByText("7")).toBeNull();
    expect(screen.queryByTestId("ledger-drift-line")).toBeNull();
  });

  it("keeps the existing poll cadences, skips hidden/disconnected requests and handles failures", async () => {
    vi.useFakeTimers();
    mockFetch.mockResolvedValue(summary());
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderUsage();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(fetchSessionLedgerRun).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(mockFetch).toHaveBeenCalledTimes(7);
    expect(fetchSessionLedgerRun).toHaveBeenCalledTimes(2);
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(mockFetch).toHaveBeenCalledTimes(7);
    expect(fetchSessionLedgerRun).toHaveBeenCalledTimes(2);
    visibility.mockReturnValue("visible");
    mockFetch.mockRejectedValueOnce(new Error("offline"));
    vi.mocked(fetchSessionLedgerRun).mockRejectedValueOnce(new Error("offline"));
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    expect(screen.queryByTestId("session-usage")).toBeNull();
    act(() => setServerDown(true));
    const calls = mockFetch.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(mockFetch).toHaveBeenCalledTimes(calls);
    await act(async () => setServerDown(false));
    expect(screen.getByText("7")).toBeTruthy();
  });

  it("does not overlap slow requests with poll or visibility refreshes", async () => {
    vi.useFakeTimers();
    mockFetch.mockResolvedValue(summary());
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    let finishUsage!: (value: UsageSummary) => void;
    let finishLedger!: (value: LedgerRunView) => void;
    mockFetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishUsage = resolve;
        }),
    );
    vi.mocked(fetchSessionLedgerRun).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishLedger = resolve;
        }),
    );
    renderUsage();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));
    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(fetchSessionLedgerRun).toHaveBeenCalledTimes(1);
    await act(async () => {
      finishUsage(summary());
      finishLedger(ledgerView);
    });
    expect(screen.getByText("7")).toBeTruthy();
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    expect(mockFetch).toHaveBeenCalledTimes(2);
    expect(fetchSessionLedgerRun).toHaveBeenCalledTimes(2);
  });

  it("shows both Ledger lines for a session without usage tracking", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderUsage();
    expect((await screen.findByTestId("ledger-drift-line")).textContent).toBe("ledger d861 current");
    expect(screen.getByTestId("ledger-headroom-line").textContent).toBe("hr 326.5k saved · 29% · ~$1.00  (now 26.5k)");
  });

  it("hides each line with its own setting", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderUsage({ ledger: false });
    await screen.findByTestId("ledger-headroom-line");
    expect(screen.queryByTestId("ledger-drift-line")).toBeNull();
    cleanup();
    renderUsage({ headroom: false });
    await screen.findByTestId("ledger-drift-line");
    expect(screen.queryByTestId("ledger-headroom-line")).toBeNull();
  });

  it("does not ask for the Ledger view while both lines are off", async () => {
    mockFetch.mockResolvedValue(summary());
    renderUsage({ ledger: false, headroom: false });
    await screen.findByText("7");
    expect(vi.mocked(fetchSessionLedgerRun)).not.toHaveBeenCalled();
  });

  it("reloads the Ledger view when the tab becomes visible", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderUsage();
    await screen.findByTestId("ledger-drift-line");
    const before = vi.mocked(fetchSessionLedgerRun).mock.calls.length;
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(vi.mocked(fetchSessionLedgerRun).mock.calls.length).toBe(before + 1);
  });
});
