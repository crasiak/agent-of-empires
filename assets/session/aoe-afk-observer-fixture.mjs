// Test-only bridge and exposed-call observations, not timer origins or remote receipt.
import { openSync, writeSync, closeSync, statSync, existsSync, writeFileSync, linkSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";
import { performance } from "node:perf_hooks";
import { registerAfk, spawnBridge } from "./aoe-afk.mjs";
import { writeAudit } from "./aoe-afk-observer-audit.mjs";

export const EVENTS = ["request", "resolved", "rejected", "close", "closed", "close_failed", "attach", "attached", "attach_failed", "failed", "failed_returned", "failed_threw", "opened", "question_closed", "reply", "teardown", "truncated"];
export const OPS = ["poll", "ack", "runtime_ack", "question_recover", "runtime", "other"];
export const KINDS = ["none", "reserve", "stop", "settle", "question_admit", "question_intent", "question_outcome", "read", "record", "apply", "checkpoint", "nudge", "contribution_intent", "other"];
const MAX_EVENTS = 1024, MAX_BYTES = 256 * 1024, MAX_ORDINAL = 65535;
const number = (n, max) => Number.isInteger(n) && n >= 0 && n <= max;
export function validEvent(e) {
  if (!e || Object.getPrototypeOf(e) !== Object.prototype) return false;
  const keys = Object.keys(e).sort().join(",");
  return keys === "bridge,duration,event,kind,op,request,seq,teardown" && EVENTS.includes(e.event)
    && OPS.includes(e.op) && KINDS.includes(e.kind) && typeof e.teardown === "boolean"
    && number(e.seq, MAX_EVENTS) && e.seq > 0 && number(e.bridge, MAX_ORDINAL)
    && number(e.request, MAX_ORDINAL) && number(e.duration, 86400000);
}
export function createTrace(write, limits = {}) {
  const maxEvents = Math.min(MAX_EVENTS, limits.events ?? MAX_EVENTS);
  const maxBytes = Math.min(MAX_BYTES, limits.bytes ?? MAX_BYTES);
  let invalid = false, truncated = false, events = 0, bytes = 0;
  const row = (event, data = {}) => ({ seq: events + 1, event, bridge: 0, request: 0, op: "other", kind: "none", duration: 0, teardown: false, ...data });
  const markerSize = Buffer.byteLength(JSON.stringify({ ...row("truncated"), seq: MAX_EVENTS, teardown: false }) + "\n");
  const persist = e => {
    const line = JSON.stringify(e) + "\n";
    // Counts describe attempted writes too, so partial/silent writes cannot pass the external audit.
    events++; bytes += Buffer.byteLength(line);
    try { write(line); } catch { invalid = true; }
  };
  return {
    emit(event, data) {
      if (invalid) return;
      try {
        const e = row(event, data);
        if (!validEvent(e)) { invalid = true; return; }
        const size = Buffer.byteLength(JSON.stringify(e) + "\n");
        if (events >= maxEvents - 1 || bytes + size + markerSize > maxBytes) {
          invalid = true; truncated = true;
          const marker = row("truncated");
          if (events < maxEvents && bytes + Buffer.byteLength(JSON.stringify(marker) + "\n") <= maxBytes) persist(marker);
          return;
        }
        persist(e);
      } catch { invalid = true; }
    },
    invalidate() { invalid = true; },
    snapshot() { return { invalid, truncated, events, bytes }; },
  };
}
export function privateTrace(path) {
  let fd;
  const trace = createTrace(line => {
    if (fd === undefined) {
      if ((statSync(dirname(path)).mode & 0o777) !== 0o700) throw Error("private directory required");
      fd = openSync(path, "wx", 0o600);
    }
    if (writeSync(fd, line) !== Buffer.byteLength(line)) throw Error("short write");
  });
  return { ...trace, close() { try { if (fd !== undefined) closeSync(fd); } catch { trace.invalidate(); } fd = undefined; } };
}

// The independent launcher requires this separate exit channel AND actual waitpid completion.
// It also revalidates trace bytes/schema/counts; this channel alone is never qualification.
export function health() {
  const key = Symbol.for("aoe.test.native-observer.health");
  if (globalThis[key]) return globalThis[key];
  const state = { teardown: false, shutdown: false, fault: false, calls: 0, cancelled: false, opened: false, attributed: false, nativeDeferred: false, waitArmed: false, runCompleted: false, trace: null, probes: null, fixture: null };
  globalThis[key] = state;
  process.on("uncaughtExceptionMonitor", () => { state.fault = true; });
  process.on("exit", () => {
    try { writeAudit(state); } catch { /* Missing or invalid audit emission is inconclusive. */ }
  });
  return state;
}

function observerCall(trace, state, fn) {
  try { fn(); } catch {
    try { trace.invalidate(); state.fault = true; } catch { /* Never replace a product outcome. */ }
  }
}
export function observeBridge(bridge, trace, state, ordinal = 1) {
  let requests = 0;
  const emit = (event, data = {}) => observerCall(trace, state, () => {
    state.syncTeardown?.();
    trace.emit(event, { bridge: ordinal, teardown: state.teardown, ...data });
  });
  const watch = (result, success, failure, data, start) => {
    // Return the original promise; observer reactions never replace the caller's result/rejection.
    observerCall(trace, state, () => result.then(() => emit(success, { ...data, duration: Math.min(86400000, Math.max(0, Math.round(performance.now() - start))) }), () => {
      observerCall(trace, state, () => {
        state.syncTeardown?.();
        if (state.teardown || failure === "close_failed") state.fault = true;
      });
      emit(failure, data);
    }));
    return result;
  };
  return {
    request(...args) {
      const request = ++requests, start = performance.now();
      let data = { request, op: "other", kind: "none" };
      try {
        const packet = args[0];
        data.op = OPS.includes(packet?.op) ? packet.op : "other";
        data.kind = packet?.op === "runtime" ? (KINDS.includes(packet.command?.kind) ? packet.command.kind : "other") : "none";
      } catch { trace.invalidate(); }
      emit("request", data);
      let result;
      try { result = Reflect.apply(bridge.request, bridge, args); }
      catch (error) { if (state.teardown) state.fault = true; emit("rejected", data); throw error; }
      return watch(result, "resolved", "rejected", data, start);
    },
    close(...args) {
      emit("close");
      let result;
      try { result = Reflect.apply(bridge.close, bridge, args); }
      catch (error) { state.fault = true; emit("close_failed"); throw error; }
      return watch(result, "closed", "close_failed", {}, performance.now());
    },
  };
}
export function observeDelegation(delegate, trace, state) {
  const decorated = { ...delegate };
  for (const [method, success, failure] of [["attach", "attached", "attach_failed"], ["failed", "failed_returned", "failed_threw"]]) {
    decorated[method] = function (...args) {
      observerCall(trace, state, () => { state.syncTeardown?.(); if (method === "failed" && state.teardown) state.fault = true; trace.emit(method, { teardown: state.teardown }); });
      try {
        const result = Reflect.apply(delegate[method], delegate, args);
        observerCall(trace, state, () => trace.emit(success, { teardown: state.teardown }));
        return result;
      } catch (error) {
        observerCall(trace, state, () => { state.fault = true; trace.emit(failure, { teardown: state.teardown }); });
        throw error;
      }
    };
  }
  return decorated;
}
export function registerObserved(pi, delegationRuntime, options = {}) {
  const state = health(), trace = privateTrace(process.env.AOE_FIXTURE_TRACE);
  const probe = process.env.AOE_FIXTURE_PROBES ? options.probes?.open(process.env.AOE_FIXTURE_PROBES) : null;
  state.trace = trace; state.probes = probe;
  state.syncTeardown = () => {
    if (!state.teardown && existsSync(`${process.env.AOE_FIXTURE_ROOT}/teardown`)) {
      state.teardown = true;
      trace.emit("teardown", { teardown: true });
    }
  };
  pi.on("session_shutdown", () => observerCall(trace, state, state.syncTeardown));
  let bridges = 0, current;
  const delegate = delegationRuntime(pi);
  const captured = { ...delegate, attach(...args) { current = args[0]; return Reflect.apply(delegate.attach, delegate, args); } };
  const observed = probe ? options.probes.delegation(captured, state, probe) : captured;
  const registration = { ...pi, on(event, handler) {
    return pi.on(event, event === "session_start" ? async (...args) => {
      await handler(...args);
      if (!probe) return;
      observerCall(trace, state, () => {
        const installed = !!current && options.probes.refresh(current, probe, state);
        if (state.fixture) state.fixture.observationInstalled = installed;
        if (!installed) { probe.invalidate(); state.fault = true; }
      });
    } : handler);
  } };
  registerAfk(registration, { protocol: 6, delegation: observeDelegation(observed, trace, state), schedule: options.schedule,
    bridge: (...args) => {
      const ordinal = ++bridges, bridge = (options.bridge ?? spawnBridge)(...args);
      return observeBridge(probe ? options.probes.bridge(bridge, state, ordinal, probe) : bridge, trace, state, ordinal);
    } });
  pi.on("session_shutdown", () => { state.shutdown = true; });
  for (const [channel, event] of [["opened", "opened"], ["closed", "question_closed"], ["reply", "reply"]]) {
    pi.events.on(`ask:defer:v1:${channel}`, () => observerCall(trace, state, () => trace.emit(event, { teardown: state.teardown })));
  }
}

export function pendingStream(Stream, model, signal, cancelled) {
  const stream = new Stream();
  const abort = () => {
    signal.removeEventListener("abort", abort);
    cancelled();
    const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id,
      timestamp: Date.now(), content: [], stopReason: "aborted", errorMessage: "fixture cutoff cancelled",
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    stream.push({ type: "error", reason: "aborted", error: message });
    stream.end(message);
  };
  if (signal.aborted) abort(); else signal.addEventListener("abort", abort, { once: true });
  return stream;
}
// These two native notifications can arrive in either order. Retain no payload beyond bounded identities.
export function questionAttribution(ready, fault) {
  const starts = new Map(); let opened, announced = false;
  const check = () => {
    if (!announced && opened && starts.get(opened.id) === opened.kind) { announced = true; ready(); }
  };
  const owner = e => e?.owner?.namespace === "pi-ask-user" && typeof e.owner.id === "string" && e.owner.id.length > 0 && e.owner.id.length <= 256;
  return {
    start(e) {
      if (!owner(e)) return;
      if (starts.size >= 8 || (starts.has(e.owner.id) && starts.get(e.owner.id) !== e.kind)) { fault(); return; }
      starts.set(e.owner.id, ["input", "select", "custom"].includes(e.kind) ? e.kind : "other"); check();
    },
    opened(e) {
      if (opened || !owner(e) || e.version !== 1 || e.supported !== true || e.uiPromptAttribution !== true
        || e.state !== "pending" || e.toolCallId !== "fixture-question-1" || e.owner.id !== e.interactionId
        || !["input", "select", "custom-overlay"].includes(e.kind)) { fault(); return; }
      opened = { id: e.interactionId, kind: e.kind === "custom-overlay" ? "custom" : e.kind }; check();
    },
  };
}

export function publishMarker(path, value, beforePublish = () => {}) {
  const temporary = `${path}.pending`;
  writeFileSync(temporary, JSON.stringify(value) + "\n", { flag: "wx", mode: 0o600 });
  beforePublish();
  linkSync(temporary, path);
  unlinkSync(temporary);
}

export function nativeDeferredResult(question, event) {
  const details = event?.details;
  return !!question && event?.toolName === "ask_user" && event.toolCallId === question.toolCallId && event.isError === false
    && details?.toolCallId === question.toolCallId && details.interactionId === question.interactionId
    && details.status === "no_human_answer" && details.reason === "deferred_by_afk" && details.response === null && details.cancelled === true;
}

// Capture the SDK's idle promise while active, never after isIdle becomes true during settlement.
export function cancellationCommand(state, save) {
  return async (_args, ctx) => {
    try {
      const episode = state.episode, session = ctx.sessionManager.getSessionId();
      if (state.waitArmed || state.calls !== 2 || !episode || episode.signal.aborted || ctx.isIdle()
        || ctx.hasPendingMessages() || state.fault) throw Error("fixture cannot arm completion");
      const idle = ctx.waitForIdle();
      state.waitArmed = true;
      if (!save("wait-armed.json", { armed: true })) throw Error("fixture arm publication failed");
      await idle;
      if (state.episode !== episode || state.calls !== 2 || !episode.signal.aborted || !state.cancelled
        || !episode.consumed || !state.teardown || !ctx.isIdle() || ctx.hasPendingMessages()
        || ctx.sessionManager.getSessionId() !== session || state.fault) throw Error("fixture completion unavailable");
      state.runCompleted = true;
      if (!save("run-completed.json", { completed: true })) throw Error("fixture completion publication failed");
    } catch { state.fault = true; }
  };
}
