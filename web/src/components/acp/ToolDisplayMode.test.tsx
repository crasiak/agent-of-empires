// @vitest-environment jsdom
//
// Transcript tool-card density (#1767). Pins the pieces that give the
// user back a scannable view after plan approval without touching the
// automatic grouping in AcpRuntime:
//   - useToolDensityPref persists to localStorage and defaults detailed,
//   - ToolDensityToggle reflects state via aria-pressed,
//   - compact mode collapses an otherwise-open card (a <=5 item todo),
//   - a user's per-card toggle is scoped to the active density, so
//     flipping the global toggle re-collapses an expanded card,
//   - errored cards stay auto-open even in compact mode so a failure is
//     never hidden (#1467),
//   - fold all folds every card, failures included, and unfold all opens
//     every card; each press clears per-card toggles, which work again after
//     it (CRA-1813).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { ReactNode } from "react";

vi.mock("../../lib/snippetHighlighter", () => ({
  highlightSnippet: vi.fn().mockResolvedValue(null),
}));

vi.mock("../../hooks/useShikiTheme", () => ({
  useShikiTheme: () => ({ theme: "dark-plus", appearance: "dark" }),
}));

import {
  ToolDensityToggle,
  ToolDisplayModeProvider,
  ToolFoldToggle,
  useToolDensityPref,
  useToolDisplayMode,
  useToolFold,
  type ToolDensity,
  type ToolFold,
} from "./ToolDisplayMode";
import { ToolCard } from "./ToolCards";
import { ToolGroupCard } from "./GroupToolCards";
import { TodoGroupCard } from "./TodoCards";
import { ToolExpansionProvider } from "./ToolExpansion";
import { AgentProfileProvider } from "../../lib/agentProfileContext";
import { fixtures, makeCompletion, makeError, makeToolCall } from "./__fixtures__/toolCalls";

const STORAGE_KEY = "aoe.acp.toolDensity.v1";

function Wrap({
  density,
  fold = null,
  toolKey,
  children,
}: {
  density: ToolDensity;
  fold?: ToolFold;
  toolKey?: string;
  children: ReactNode;
}) {
  return (
    <AgentProfileProvider toolKey={toolKey ?? null}>
      <ToolDisplayModeProvider density={density} fold={fold}>
        <ToolExpansionProvider>{children}</ToolExpansionProvider>
      </ToolDisplayModeProvider>
    </AgentProfileProvider>
  );
}

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
});

describe("useToolDisplayMode", () => {
  it("defaults to detailed outside a provider and reads the provided density inside one", () => {
    expect(renderHook(() => useToolDisplayMode()).result.current).toBe("detailed");
    const { result } = renderHook(() => useToolDisplayMode(), {
      wrapper: ({ children }) => <ToolDisplayModeProvider density="compact">{children}</ToolDisplayModeProvider>,
    });
    expect(result.current).toBe("compact");
  });
});

describe("useToolDensityPref", () => {
  it("defaults detailed, toggles with persistence, and initialises from storage", () => {
    const { result, unmount } = renderHook(() => useToolDensityPref());
    expect(result.current[0]).toBe("detailed");
    act(() => result.current[1]());
    expect(result.current[0]).toBe("compact");
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe("compact");
    unmount();
    const reloaded = renderHook(() => useToolDensityPref());
    expect(reloaded.result.current[0]).toBe("compact");
    act(() => reloaded.result.current[1]());
    expect(reloaded.result.current[0]).toBe("detailed");
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe("detailed");
  });
});

describe("ToolDensityToggle", () => {
  it("reflects density via aria-pressed and fires onToggle", () => {
    const onToggle = vi.fn();
    const { rerender } = render(<ToolDensityToggle density="detailed" onToggle={onToggle} />);
    const btn = screen.getByRole("button", { name: /compact tools/i });
    expect(btn.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
    rerender(<ToolDensityToggle density="compact" onToggle={onToggle} />);
    expect(screen.getByRole("button", { name: /compact tools/i }).getAttribute("aria-pressed")).toBe("true");
  });
});

describe("tool-card density", () => {
  it("compact collapses an otherwise-open todo card", () => {
    const { container, rerender } = render(
      <Wrap density="detailed" toolKey="claude">
        <ToolCard tool={fixtures.todoWrite} result={undefined} />
      </Wrap>,
    );
    // <=5 todos default open in detailed mode, so the list body shows.
    expect(container.textContent).toContain("Step one");

    rerender(
      <Wrap density="compact" toolKey="claude">
        <ToolCard tool={fixtures.todoWrite} result={undefined} />
      </Wrap>,
    );
    expect(container.textContent).not.toContain("Step one");
  });

  it("re-collapses a user-expanded card when density flips", () => {
    const { container, rerender } = render(
      <Wrap density="detailed" toolKey="claude">
        <ToolCard tool={fixtures.todoWrite} result={undefined} />
      </Wrap>,
    );
    // Collapse it by hand in detailed mode.
    fireEvent.click(screen.getByRole("button", { name: /items/i }));
    expect(container.textContent).not.toContain("Step one");

    // Flipping to compact must not leak the detailed-mode override; the
    // compact baseline (collapsed) applies.
    rerender(
      <Wrap density="compact" toolKey="claude">
        <ToolCard tool={fixtures.todoWrite} result={undefined} />
      </Wrap>,
    );
    expect(container.textContent).not.toContain("Step one");

    // And expanding it in compact works.
    fireEvent.click(screen.getByRole("button", { name: /items/i }));
    expect(container.textContent).toContain("Step one");
  });

  it("keeps an errored card open even in compact mode", () => {
    const { container } = render(
      <Wrap density="compact">
        <ToolCard tool={fixtures.bash} result={makeError({ toolCallId: "bash-1", text: "boom" })} />
      </Wrap>,
    );
    expect(container.textContent).toContain("boom");
  });
});

describe("useToolFold", () => {
  it("starts unset, folds first, then alternates with a new epoch per press, and resets", () => {
    const { result } = renderHook(() => useToolFold());
    expect(result.current[0]).toBeNull();
    act(() => result.current[1]());
    expect(result.current[0]).toEqual({ open: false, epoch: 1 });
    act(() => result.current[1]());
    expect(result.current[0]).toEqual({ open: true, epoch: 2 });
    act(() => result.current[1]());
    expect(result.current[0]).toEqual({ open: false, epoch: 3 });
    act(() => result.current[2]());
    expect(result.current[0]).toBeNull();
    // A later press must not revive toggles made under an earlier one.
    act(() => result.current[1]());
    expect(result.current[0]).toEqual({ open: false, epoch: 4 });
  });
});

describe("ToolFoldToggle", () => {
  it("offers fold all until everything is folded, then unfold all", () => {
    const onToggle = vi.fn();
    const { rerender } = render(<ToolFoldToggle fold={null} onToggle={onToggle} />);
    fireEvent.click(screen.getByRole("button", { name: "Fold all" }));
    expect(onToggle).toHaveBeenCalledTimes(1);
    rerender(<ToolFoldToggle fold={{ open: false, epoch: 1 }} onToggle={onToggle} />);
    expect(screen.getByRole("button", { name: "Unfold all" })).toBeTruthy();
    rerender(<ToolFoldToggle fold={{ open: true, epoch: 2 }} onToggle={onToggle} />);
    expect(screen.getByRole("button", { name: "Fold all" })).toBeTruthy();
  });
});

describe("fold all", () => {
  const ok = makeCompletion({ toolCallId: "bash-1", text: "listing-output" });
  const failed = makeError({ toolCallId: "bash-1", text: "boom" });
  const card = (result: typeof ok) => <ToolCard tool={fixtures.bash} result={result} />;

  it.each<ToolDensity>(["detailed", "compact"])("folds a failed card and unfolds a successful one (%s)", (density) => {
    const { container, rerender } = render(<Wrap density={density}>{card(failed)}</Wrap>);
    expect(container.textContent).toContain("boom");
    rerender(
      <Wrap density={density} fold={{ open: false, epoch: 1 }}>
        {card(failed)}
      </Wrap>,
    );
    expect(container.textContent).not.toContain("boom");
    rerender(
      <Wrap density={density} fold={{ open: true, epoch: 2 }}>
        {card(ok)}
      </Wrap>,
    );
    expect(container.textContent).toContain("listing-output");
  });

  it("keeps per-card toggles working, and the next press overrides them", () => {
    const folded = { open: false, epoch: 1 };
    const { container, rerender } = render(
      <Wrap density="detailed" fold={folded}>
        {card(failed)}
      </Wrap>,
    );
    fireEvent.click(screen.getByRole("button", { name: /ls -la/ }));
    expect(container.textContent).toContain("boom");

    rerender(
      <Wrap density="detailed" fold={{ open: true, epoch: 2 }}>
        {card(failed)}
      </Wrap>,
    );
    fireEvent.click(screen.getByRole("button", { name: /ls -la/ }));
    expect(container.textContent).not.toContain("boom");

    rerender(
      <Wrap density="detailed" fold={{ open: false, epoch: 3 }}>
        {card(failed)}
      </Wrap>,
    );
    fireEvent.click(screen.getByRole("button", { name: /ls -la/ }));
    expect(container.textContent).toContain("boom");
  });

  it("folds and unfolds an actions group and its cards", () => {
    const items = ["a", "b", "c"].map((id) => ({
      tool: { ...fixtures.bash, id, args_preview: JSON.stringify({ command: `cmd-${id}` }) },
      result: makeCompletion({ toolCallId: id, text: `out-${id}` }),
      kind: "execute",
    }));
    const { container, rerender } = render(
      <Wrap density="detailed" fold={{ open: true, epoch: 1 }}>
        <ToolGroupCard items={items} />
      </Wrap>,
    );
    expect(container.textContent).toContain("out-a");
    expect(container.textContent).toContain("out-c");
    rerender(
      <Wrap density="detailed" fold={{ open: false, epoch: 2 }}>
        <ToolGroupCard items={items} />
      </Wrap>,
    );
    expect(container.textContent).toContain("3 actions");
    expect(container.textContent).not.toContain("cmd-a");
  });

  it("folds and unfolds a run of todo updates, keeping the latest list as its summary", () => {
    const updates = ["x", "y"].map((id) => ({
      tool: makeToolCall({
        id,
        name: "TodoWrite",
        args_preview: JSON.stringify({
          todos: Array.from({ length: 6 }, (_, n) => ({ content: `item-${id}-${n}`, status: "pending" })),
        }),
      }),
      result: makeCompletion({ toolCallId: id }),
    }));
    const { container, rerender } = render(
      <Wrap density="detailed" toolKey="claude" fold={{ open: true, epoch: 1 }}>
        <TodoGroupCard items={updates} />
      </Wrap>,
    );
    expect(container.textContent).toContain("item-x-0");
    rerender(
      <Wrap density="detailed" toolKey="claude" fold={{ open: false, epoch: 2 }}>
        <TodoGroupCard items={updates} />
      </Wrap>,
    );
    expect(container.textContent).toContain("updated 2 times");
    expect(container.textContent).toContain("item-y-0");
    expect(container.textContent).not.toContain("item-x-0");
  });
});
