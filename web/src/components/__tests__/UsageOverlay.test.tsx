// @vitest-environment jsdom
//
// Tests for the terminal-corner usage overlay: it shows the reset count once
// a summary with any recorded event arrives, and renders nothing while the
// setting is off.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

import type { UsageSummary } from "../../lib/usage";
import { makeSession } from "./fixtures";

vi.mock("../../lib/api", () => ({ fetchSessionUsage: vi.fn(), fetchSessionLedgerRun: vi.fn() }));

import { fetchSessionLedgerRun, fetchSessionUsage } from "../../lib/api";
import { HeadroomOverlayEnabledContext, LedgerOverlayEnabledContext, type LedgerRunView } from "../../lib/ledgerRun";
import { UsageOverlayEnabledContext } from "../../lib/usage";
import { UsageOverlay } from "../UsageOverlay";

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
  vi.clearAllMocks();
});

describe("UsageOverlay", () => {
  it("shows the reset count once a summary arrives", async () => {
    mockFetch.mockResolvedValue(summary());

    render(
      <UsageOverlayEnabledContext.Provider value={true}>
        <UsageOverlay session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    expect(await screen.findByText("7")).toBeTruthy();
    expect(screen.getByTestId("usage-overlay")).toBeTruthy();
  });

  it("renders nothing while the setting is off", () => {
    render(
      <UsageOverlayEnabledContext.Provider value={false}>
        <UsageOverlay session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    expect(screen.queryByTestId("usage-overlay")).toBeNull();
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("renders nothing for a lifecycle-only session", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));

    render(
      <UsageOverlayEnabledContext.Provider value={true}>
        <UsageOverlay session={session} />
      </UsageOverlayEnabledContext.Provider>,
    );

    await vi.waitFor(() => expect(mockFetch).toHaveBeenCalled());
    expect(screen.queryByTestId("usage-overlay")).toBeNull();
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

function renderOverlay({ usage = true, ledger = true, headroom = true } = {}) {
  return render(
    <UsageOverlayEnabledContext.Provider value={usage}>
      <LedgerOverlayEnabledContext.Provider value={ledger}>
        <HeadroomOverlayEnabledContext.Provider value={headroom}>
          <UsageOverlay session={session} />
        </HeadroomOverlayEnabledContext.Provider>
      </LedgerOverlayEnabledContext.Provider>
    </UsageOverlayEnabledContext.Provider>,
  );
}

describe("UsageOverlay Ledger lines", () => {
  it("shows both Ledger lines for a session without usage tracking", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderOverlay();
    expect((await screen.findByTestId("ledger-drift-line")).textContent).toBe("ledger d861 current");
    expect(screen.getByTestId("ledger-headroom-line").textContent).toBe("hr 326.5k saved · 29% · ~$1.00  (now 26.5k)");
  });

  it("hides each line with its own setting", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderOverlay({ ledger: false });
    await screen.findByTestId("ledger-headroom-line");
    expect(screen.queryByTestId("ledger-drift-line")).toBeNull();
    cleanup();
    renderOverlay({ headroom: false });
    await screen.findByTestId("ledger-drift-line");
    expect(screen.queryByTestId("ledger-headroom-line")).toBeNull();
  });

  it("does not ask for the Ledger view while both lines are off", async () => {
    mockFetch.mockResolvedValue(summary());
    renderOverlay({ ledger: false, headroom: false });
    await screen.findByText("7");
    expect(vi.mocked(fetchSessionLedgerRun)).not.toHaveBeenCalled();
  });

  it("reloads the Ledger view when the tab becomes visible", async () => {
    mockFetch.mockResolvedValue(summary({ tracked: false }));
    vi.mocked(fetchSessionLedgerRun).mockResolvedValue(ledgerView);
    renderOverlay();
    await screen.findByTestId("ledger-drift-line");
    const before = vi.mocked(fetchSessionLedgerRun).mock.calls.length;
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(vi.mocked(fetchSessionLedgerRun).mock.calls.length).toBe(before + 1);
  });
});
