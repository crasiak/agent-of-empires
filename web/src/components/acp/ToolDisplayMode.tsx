/* eslint-disable react-refresh/only-export-components */
// Client-side tool-card density: "compact" collapses every card by default.
// Fold all is a transient view-level override on top of it.

import { createContext, useCallback, useContext, useMemo, useRef, useState } from "react";
import { ChevronsDownUp, ChevronsUpDown, ListCollapse } from "lucide-react";
import { safeGetItem, safeSetItem } from "../../lib/safeStorage";

export type ToolDensity = "detailed" | "compact";

/** The last fold-all press: every card at `open` until the reader toggles one.
 *  Each press has a new `epoch`, so it overrides the toggles made before it. */
export type ToolFold = { open: boolean; epoch: number } | null;

const STORAGE_KEY = "aoe.acp.toolDensity.v1";

function readStoredDensity(): ToolDensity {
  return safeGetItem(STORAGE_KEY) === "compact" ? "compact" : "detailed";
}

export function useToolDensityPref(): [ToolDensity, () => void] {
  const [density, setDensity] = useState<ToolDensity>(readStoredDensity);
  const toggle = useCallback(() => {
    setDensity((prev) => {
      const next: ToolDensity = prev === "compact" ? "detailed" : "compact";
      safeSetItem(STORAGE_KEY, next);
      return next;
    });
  }, []);
  return [density, toggle];
}

/** The first press folds, since the default already shows a mix. `reset`
 *  hands control back to density; epochs keep counting so old toggles stay stale. */
export function useToolFold(): [ToolFold, () => void, () => void] {
  const [fold, setFold] = useState<ToolFold>(null);
  const epoch = useRef(0);
  const toggle = useCallback(() => {
    epoch.current += 1;
    const next = epoch.current;
    setFold((prev) => ({ open: prev ? !prev.open : false, epoch: next }));
  }, []);
  const reset = useCallback(() => setFold(null), []);
  return [fold, toggle, reset];
}

export interface ToolDisplay {
  density: ToolDensity;
  fold: ToolFold;
}

const ToolDisplayModeContext = createContext<ToolDisplay>({ density: "detailed", fold: null });

export function ToolDisplayModeProvider({
  density,
  fold = null,
  children,
}: {
  density: ToolDensity;
  fold?: ToolFold;
  children: React.ReactNode;
}) {
  const value = useMemo(() => ({ density, fold }), [density, fold]);
  return <ToolDisplayModeContext.Provider value={value}>{children}</ToolDisplayModeContext.Provider>;
}

export function useToolDisplayMode(): ToolDensity {
  return useContext(ToolDisplayModeContext).density;
}

export function useToolDisplay(): ToolDisplay {
  return useContext(ToolDisplayModeContext);
}

/** The key a per-card toggle is valid under: the density, or the fold-all press. */
export function expansionMode({ density, fold }: ToolDisplay): string {
  return fold ? `fold-${fold.epoch}` : density;
}

/** A card's state before any toggle. Fold all wins; otherwise failures open
 *  and compact density closes the rest. */
export function expansionBaseline({ density, fold }: ToolDisplay, failed: boolean, defaultOpen = false): boolean {
  if (fold) return fold.open;
  if (failed) return true;
  return density === "compact" ? false : defaultOpen;
}

export function ToolDensityToggle({ density, onToggle }: { density: ToolDensity; onToggle: () => void }) {
  const compact = density === "compact";
  return (
    <button
      type="button"
      aria-pressed={compact}
      onClick={onToggle}
      title={
        compact
          ? "Tool cards collapsed for scanning; click to restore detail"
          : "Collapse every tool card to its header for scanning"
      }
      className={[
        "ml-auto inline-flex items-center gap-1.5 rounded-md border px-2 py-1",
        "text-[11px] uppercase tracking-wider transition-colors",
        compact
          ? "border-brand-700/50 bg-brand-700/10 text-brand-400"
          : "border-surface-700 bg-surface-800 text-text-dim hover:bg-surface-700",
      ].join(" ")}
    >
      <ListCollapse className="h-3.5 w-3.5" />
      Compact tools
    </button>
  );
}

export function ToolFoldToggle({ fold, onToggle }: { fold: ToolFold; onToggle: () => void }) {
  const unfold = fold?.open === false;
  const Icon = unfold ? ChevronsUpDown : ChevronsDownUp;
  return (
    <button
      type="button"
      onClick={onToggle}
      title={unfold ? "Expand every tool card" : "Fold every tool card to one line, failed ones included"}
      className={[
        "ml-2 inline-flex items-center gap-1.5 rounded-md border px-2 py-1",
        "text-[11px] uppercase tracking-wider transition-colors",
        "border-surface-700 bg-surface-800 text-text-dim hover:bg-surface-700",
      ].join(" ")}
    >
      <Icon className="h-3.5 w-3.5" />
      {unfold ? "Unfold all" : "Fold all"}
    </button>
  );
}
