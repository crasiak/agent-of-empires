import { openSync, writeSync, closeSync, statSync } from "node:fs";
import { dirname } from "node:path";
import { KINDS } from "./aoe-afk-observer-fixture.mjs";

export const PROBE_TYPES = ["refresh", "cleanup_stop"];
export const PROBE_CALLERS = ["poll", "before_request", "before_tool", "turn_end", "settled", "shutdown", "failed_callback", "unknown", "other"];
const PHASES = {
  refresh: ["start", "join", "resolved", "rejected", "threw"],
  cleanup_stop: ["entry", "returned", "rejected", "dispatch", "reply_resolved", "reply_rejected", "threw"],
};
const MAX_EVENTS = 1024, MAX_BYTES = 256 * 1024, MAX_ORDINAL = 65535;
const number = (n, max) => Number.isInteger(n) && n >= 0 && n <= max;
const caller = value => value === undefined || value === null ? "unknown" : PROBE_CALLERS.includes(value) ? value : "other";

export function validProbe(e) {
  if (!e || Object.getPrototypeOf(e) !== Object.prototype) return false;
  const keys = Object.keys(e).sort().join(",");
  return keys === "bridge,call,caller,kind,parent,phase,request,seq,teardown,type" && PROBE_TYPES.includes(e.type)
    && PHASES[e.type].includes(e.phase) && PROBE_CALLERS.includes(e.caller) && KINDS.includes(e.kind)
    && typeof e.teardown === "boolean" && number(e.seq, MAX_EVENTS) && e.seq > 0
    && number(e.call, MAX_ORDINAL) && e.call > 0 && number(e.parent, MAX_ORDINAL)
    && number(e.bridge, MAX_ORDINAL) && number(e.request, MAX_ORDINAL);
}

export function createDiagnosticProbe(write, limits = {}) {
  const maxEvents = Math.min(MAX_EVENTS, limits.events ?? MAX_EVENTS);
  const maxBytes = Math.min(MAX_BYTES, limits.bytes ?? MAX_BYTES);
  let invalid = false, truncated = false, events = 0, bytes = 0, calls = 0;
  return {
    allocateCall() {
      if (invalid || calls >= MAX_ORDINAL) { invalid = true; return 0; }
      return ++calls;
    },
    emit(type, phase, data = {}) {
      if (invalid) return;
      try {
        const row = { seq: events + 1, type, phase, call: 0, parent: 0, caller: "unknown", bridge: 0, request: 0, kind: "none", teardown: false, ...data };
        if (!validProbe(row)) { invalid = true; return; }
        const line = JSON.stringify(row) + "\n", size = Buffer.byteLength(line);
        if (events >= maxEvents || bytes + size > maxBytes) { invalid = true; truncated = true; return; }
        events++; bytes += size;
        write(line);
      } catch { invalid = true; }
    },
    invalidate() { invalid = true; },
    snapshot() { return { invalid, truncated, events, bytes }; },
  };
}

export function privateProbe(path) {
  let fd;
  const probe = createDiagnosticProbe(line => {
    if (fd === undefined) {
      if ((statSync(dirname(path)).mode & 0o777) !== 0o700) throw Error("private directory required");
      fd = openSync(path, "wx", 0o600);
    }
    if (writeSync(fd, line) !== Buffer.byteLength(line)) throw Error("short write");
  });
  return { ...probe, close() { try { if (fd !== undefined) closeSync(fd); } catch { probe.invalidate(); } fd = undefined; } };
}

// These wrappers observe calls only, not lexical refreshes, writes, timer origins or Rust receipt.
export function observeRefresh(runtime, probe, state) {
  if (!probe || typeof runtime?.refresh !== "function" || runtime.refresh.__aoeObserved) return false;
  const original = runtime.refresh;
  let active;
  runtime.refresh = function (boundary = "poll", ...args) {
    const joined = !!runtime.refreshing;
    const call = probe.allocateCall();
    const data = { call, parent: joined && active?.promise === runtime.refreshing ? active.call : 0,
      caller: caller(boundary), teardown: !!state.teardown };
    probe.emit("refresh", joined ? "join" : "start", data);
    let result;
    try { result = Reflect.apply(original, this, [boundary, ...args]); }
    catch (error) { probe.emit("refresh", "threw", { ...data, teardown: !!state.teardown }); throw error; }
    if (!joined) active = { call, promise: result };
    Promise.resolve(result).then(
      () => probe.emit("refresh", "resolved", { ...data, teardown: !!state.teardown }),
      () => probe.emit("refresh", "rejected", { ...data, teardown: !!state.teardown }),
    );
    return result;
  };
  runtime.refresh.__aoeObserved = true;
  return true;
}

export function observeBridge(bridge, state, ordinal = 1, probe = null) {
  let requests = 0;
  return {
    request(...args) {
      const request = ++requests;
      let stop = false, data;
      try {
        const packet = args[0];
        const kind = packet?.op === "runtime" && KINDS.includes(packet.command?.kind) ? packet.command.kind : "none";
        stop = packet?.op === "runtime" && kind === "stop";
        data = { call: stop ? probe?.allocateCall() ?? 0 : 0, parent: state.cleanupCall ?? 0,
          bridge: ordinal, request, kind, caller: caller(state.cleanupCaller), teardown: !!state.teardown };
        if (stop) probe?.emit("cleanup_stop", "dispatch", data);
      } catch { stop = false; probe?.invalidate(); }
      let result;
      try { result = Reflect.apply(bridge.request, bridge, args); }
      catch (error) { if (stop) probe?.emit("cleanup_stop", "threw", { ...data, teardown: !!state.teardown }); throw error; }
      if (stop) Promise.resolve(result).then(
        () => probe?.emit("cleanup_stop", "reply_resolved", { ...data, teardown: !!state.teardown }),
        () => probe?.emit("cleanup_stop", "reply_rejected", { ...data, teardown: !!state.teardown }),
      );
      return result;
    },
    close(...args) { return Reflect.apply(bridge.close, bridge, args); },
  };
}

export function observeDelegation(delegate, state, probe = null) {
  const decorated = { ...delegate };
  for (const [method, label] of [["failed", "failed_callback"], ["shutdown", "shutdown"]]) {
    if (typeof delegate[method] !== "function") continue;
    decorated[method] = function (...args) {
      const call = probe?.allocateCall() ?? 0;
      const data = { call, caller: label, teardown: !!state.teardown };
      const previousCall = state.cleanupCall, previousCaller = state.cleanupCaller;
      state.cleanupCall = call; state.cleanupCaller = label;
      probe?.emit("cleanup_stop", "entry", data);
      let result;
      try { result = Reflect.apply(delegate[method], delegate, args); }
      catch (error) { probe?.emit("cleanup_stop", "threw", { ...data, teardown: !!state.teardown }); throw error; }
      finally { state.cleanupCall = previousCall; state.cleanupCaller = previousCaller; }
      // Async dispatch after this synchronous call frame has an unknown caller, not borrowed attribution.
      Promise.resolve(result).then(
        () => probe?.emit("cleanup_stop", "returned", { ...data, teardown: !!state.teardown }),
        () => probe?.emit("cleanup_stop", "rejected", { ...data, teardown: !!state.teardown }),
      );
      return result;
    };
  }
  return decorated;
}
