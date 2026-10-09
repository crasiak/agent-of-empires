import { useCallback, useSyncExternalStore } from "react";
import { getSystemHealthSnapshot, subscribeSystemHealth } from "../lib/systemHealthStore";

export function useSystemHealthSnapshot(enabled: boolean, gap: number) {
  const subscribe = useCallback(
    (notify: () => void) => (enabled ? subscribeSystemHealth(notify, gap) : () => {}),
    [enabled, gap],
  );
  return useSyncExternalStore(subscribe, getSystemHealthSnapshot);
}

/** The sidebar keeps its last good reading; session metrics use the fresh snapshot. */
export function useSystemHealth(enabled: boolean, detailOpen = false) {
  const snapshot = useSystemHealthSnapshot(enabled, detailOpen ? 500 : 15_000);
  return enabled ? snapshot.lastKnownHealth : null;
}
