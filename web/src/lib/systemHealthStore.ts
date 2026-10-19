import { fetchSystemHealth, type SystemHealth } from "./api";
import { isServerDown, onServerDownChange } from "./connectionState";
import { appendAgentSamples, SESSION_METRICS_POLL_MS, type AgentHistories } from "./sessionMetrics";

interface HealthSnapshot {
  health: SystemHealth | null;
  lastKnownHealth: SystemHealth | null;
  histories: AgentHistories;
}

const emptySnapshot: HealthSnapshot = { health: null, lastKnownHealth: null, histories: new Map() };
let snapshot = emptySnapshot;
const subscribers = new Set<{ notify: () => void; gap: number }>();
let timer: ReturnType<typeof setTimeout> | undefined;
let expiry: ReturnType<typeof setTimeout> | undefined;
let inFlight = false;
let epoch = 0;
let lastHistoryAt: number | null = null;
let unsubscribeConnection: (() => void) | undefined;

export const getSystemHealthSnapshot = () => snapshot;

const active = () => subscribers.size > 0 && !document.hidden && !isServerDown();
const pollGap = () => Math.min(...[...subscribers].map(({ gap }) => gap));
const notify = () => subscribers.forEach((subscriber) => subscriber.notify());

function publish(health: SystemHealth | null, forceGap = false) {
  const now = Date.now();
  const record = forceGap || lastHistoryAt === null || now - lastHistoryAt >= SESSION_METRICS_POLL_MS;
  snapshot = {
    health,
    lastKnownHealth: health ?? snapshot.lastKnownHealth,
    histories: record ? appendAgentSamples(snapshot.histories, health, now) : snapshot.histories,
  };
  if (record) lastHistoryAt = now;
  notify();
}

function clearTimers() {
  clearTimeout(timer);
  clearTimeout(expiry);
}

/** One request across all consumers; the fastest visible consumer sets the gap. */
async function poll() {
  if (!active() || inFlight) return;
  const mine = epoch;
  inFlight = true;
  const health = await fetchSystemHealth().catch(() => null);
  inFlight = false;
  if (!active()) return;
  if (mine !== epoch) {
    void poll();
    return;
  }
  publish(health, health === null && snapshot.health !== null);
  clearTimeout(expiry);
  if (health) {
    // A hanging request must not leave the last reading looking live forever.
    expiry = setTimeout(() => publish(null, true), Math.max(10_000, pollGap() * 2));
  }
  timer = setTimeout(() => void poll(), pollGap());
}

function availabilityChanged() {
  epoch += 1;
  clearTimers();
  publish(null, true);
  if (active()) void poll();
}

export function subscribeSystemHealth(notifySubscriber: () => void, gap: number): () => void {
  const previousGap = pollGap();
  const subscriber = { notify: notifySubscriber, gap };
  subscribers.add(subscriber);
  if (subscribers.size === 1) {
    document.addEventListener("visibilitychange", availabilityChanged);
    unsubscribeConnection = onServerDownChange(availabilityChanged);
  }
  if (gap < previousGap) {
    clearTimeout(timer);
    void poll();
  }
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      epoch += 1;
      clearTimers();
      document.removeEventListener("visibilitychange", availabilityChanged);
      unsubscribeConnection?.();
      snapshot = emptySnapshot;
      lastHistoryAt = null;
    } else if (!inFlight) {
      clearTimeout(timer);
      timer = setTimeout(() => void poll(), pollGap());
    }
  };
}
