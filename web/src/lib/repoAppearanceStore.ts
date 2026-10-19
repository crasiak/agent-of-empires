import { migrateRepoAppearances } from "./migrateRepoAppearances";
import { useEffect, useSyncExternalStore } from "react";
import { fetchRepoAppearances, patchRepoAppearance } from "./api";
import { applyRepoAppearanceUpdate, type RepoAppearance, type RepoAppearanceUpdate } from "./repoAppearance";
import { reportError } from "./toastBus";

type Map = Record<string, RepoAppearance>;
let confirmed: Map = {};
let snapshot: Map = confirmed;
const listeners = new Set<() => void>();
const pending: Array<{ repoId: string; update: RepoAppearanceUpdate }> = [];
let revision = 0;
let refreshing: Promise<void> | null = null;
let writing = false;

function publish() {
  snapshot = pending.reduce((map, op) => applyRepoAppearanceUpdate(map, op.repoId, op.update), confirmed);
  listeners.forEach((listener) => listener());
}
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

export function useRepoAppearances(): Map {
  useEffect(() => {
    void refreshRepoAppearances();
  }, []);
  return useSyncExternalStore(subscribe, () => snapshot);
}

// Driven by the existing sessions refresh, not a timer for every consumer.
export function refreshRepoAppearances(): Promise<void> {
  if (refreshing) return refreshing;
  const started = revision;
  refreshing = migrateRepoAppearances()
    .then((imported) => imported ?? fetchRepoAppearances())
    .then((map) => {
      if (map && started === revision && !writing) {
        confirmed = map;
        publish();
      }
    })
    .finally(() => {
      refreshing = null;
    });
  return refreshing;
}

export async function updateRepoAppearance(repoId: string, update: RepoAppearanceUpdate): Promise<void> {
  pending.push({ repoId, update });
  revision++;
  publish();
  if (writing) return;
  writing = true;
  try {
    while (pending.length) {
      const op = pending[0]!;
      const map = await patchRepoAppearance(op.repoId, op.update);
      if (map) confirmed = map;
      else reportError("Could not save project appearance. Your change was reverted.");
      pending.shift();
      revision++;
      publish();
    }
  } finally {
    writing = false;
  }
}
