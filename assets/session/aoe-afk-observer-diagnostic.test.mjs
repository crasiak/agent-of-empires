import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { registerAfk } from "./aoe-afk.mjs";
import { classifyDiagnostic, diagnosticFields } from "./aoe-afk-observer-diagnostic.mjs";
import { createTrace, observeBridge as traceBridge, observeDelegation as traceDelegation } from "./aoe-afk-observer-fixture.mjs";
import { createDiagnosticProbe, privateProbe, observeBridge, observeDelegation, observeRefresh } from "./aoe-afk-observer-probes.mjs";

const row = (event, overrides = {}) => ({ seq: 1, event, bridge: 0, request: 0, op: "other", kind: "none", duration: 0, teardown: false, ...overrides });
const trace = events => events.map((item, i) => ({ ...row(...(Array.isArray(item) ? item : [item])), seq: i + 1 }));
const request = (n = 1, extra = {}) => ["request", { bridge: 1, request: n, op: "poll", ...extra }];
const resolved = (n = 1, extra = {}) => ["resolved", { bridge: 1, request: n, op: "poll", ...extra }];
const rejected = (n = 1, extra = {}) => ["rejected", { bridge: 1, request: n, op: "poll", ...extra }];
const teardown = () => ["teardown", { teardown: true }];
const close = (bridge = 1) => ["close", { bridge, teardown: true }];
const closed = (bridge = 1) => ["closed", { bridge, teardown: true }];
const base = () => trace(["attach", "attached", request(), resolved(), teardown(), close(), closed()]);
const probeRow = (seq, type, phase, overrides = {}) => ({ seq, type, phase, call: 1, parent: 0, caller: "unknown", bridge: 0, request: 0, kind: "none", teardown: false, ...overrides });
const renumber = rows => rows.map((row, i) => ({ ...row, seq: i + 1 }));
const latch = () => { let resolve, reject; const promise = new Promise((res, rej) => { resolve = res; reject = rej; }); return { promise, resolve, reject }; };
const captureProbe = () => { const rows = []; return { rows, probe: createDiagnosticProbe(line => rows.push(JSON.parse(line))) }; };
// Unit inputs model already-verified supervisor receipts, not an actual child/process qualification.
const writer = rows => ({ invalid: false, truncated: false, events: rows.length, bytes: Buffer.byteLength(rows.map(row => JSON.stringify(row) + "\n").join("")) });
const completionFor = (rows, probes) => ({ waitpid: { code: 0, signal: null }, audit: { completed: true, ...(Array.isArray(rows) ? { trace: writer(rows) } : {}), ...(Array.isArray(probes) ? { probes: writer(probes) } : {}) } });
function classified(rows = base(), options = {}) {
  return classifyDiagnostic({ endpoint: "qualified", completion: completionFor(rows, options.probes), trace: rows, ...options });
}

// Keep the original acceptance witnesses readable alongside adversarial completeness cases.
test("qualified endpoint without trace remains endpoint-only, never absence proof", () => {
  const result = classified(undefined, { trace: undefined, completion: completionFor() });
  assert.equal(result.classification, "clean_endpoint_only");
  assert.equal(result.trace_status, "absent");
  assert.equal(result.pre_teardown_events, "unknown");
  assert.equal(result.teardown_events, "unknown");
});

test("complete writer-valid bridge scope can be clean; missing call probes remain unknown", () => {
  const result = classified();
  assert.equal(result.trace_status, "complete");
  assert.equal(result.scope, "bridge");
  assert.equal(result.probe_status, "absent");
  assert.equal(result.pre_teardown_events, "none_observed");
  assert.equal(result.teardown_events, "none_observed");
  assert.equal(result.close_order, "not_applicable");
  assert.equal(result.refresh_phase, "unknown");
  assert.equal(result.cleanup_stop_phase, "unknown");
  assert.equal(result.classification, "diagnostic_clean");
  for (const scope of ["observed_calls", "unsupported"]) assert.equal(classified(base(), { scope }).classification, "diagnostic_inconclusive");
});

test("observed-call scope needs complete refresh and cleanup evidence and both writer receipts", () => {
  const probes = [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved"),
    probeRow(3, "cleanup_stop", "entry", { call: 2 }), probeRow(4, "cleanup_stop", "returned", { call: 2 })];
  assert.equal(classified(base(), { probes, scope: "observed_calls" }).classification, "diagnostic_clean");
  for (const rows of [[], probes.slice(0, 2), probes.slice(2).map((row, i) => ({ ...row, seq: i + 1 }))]) {
    assert.equal(classified(base(), { probes: rows, scope: "observed_calls" }).classification, "diagnostic_inconclusive");
  }
});

test("valid pre-teardown failures remain observed independently of incomplete or invalid evidence", () => {
  const full = trace(["attach", "attached", request(), rejected(), "failed", "failed_returned", teardown(), close(), closed()]);
  assert.equal(classified(full).classification, "diagnostic_failure_observed");
  for (const rows of [full.slice(0, 4), [...full, null], renumber([...full, row("truncated", { teardown: true })])]) {
    const result = classified(rows);
    assert.notEqual(result.trace_status, "complete");
    assert.equal(result.pre_teardown_events, "observed");
    assert.equal(result.classification, "diagnostic_inconclusive");
    assert.equal(result.cause_claim, "none");
  }
});

test("teardown rejection ordering uses the same trace and bridge, never infers cause", () => {
  for (const before of [true, false]) {
    const rows = trace(["attach", "attached", request(), teardown(), ...(before ? [] : [close()]),
      rejected(1, { teardown: true }), ["failed", { teardown: true }], ["failed_returned", { teardown: true }], ...(before ? [close()] : []), closed()]);
    const result = classified(rows);
    assert.equal(result.trace_status, "complete");
    assert.equal(result.teardown_events, before ? "observed_before_close" : "observed_after_close");
    assert.equal(result.close_order, before ? "rejection_before_close" : "close_before_rejection");
    assert.equal(result.classification, "diagnostic_inconclusive");
    assert.equal(result.cause_claim, "none");
  }
  const onlyCallback = trace(["attach", "attached", request(), resolved(), teardown(), ["failed", { teardown: true }], ["failed_returned", { teardown: true }], close(), closed()]);
  assert.equal(classified(onlyCallback).close_order, "unknown", "a failed callback is not a rejection");
});

test("lifecycle, request identity and teardown mutations cannot produce complete clean evidence", () => {
  const rows = base();
  const cases = [
    renumber([...rows.slice(0, 4), row("truncated"), ...rows.slice(4)]),
    rows.map(row => row.event === "closed" ? { ...row, bridge: 2 } : row),
    rows.map(row => ["close", "closed"].includes(row.event) ? { ...row, bridge: 2 } : row),
    rows.map(row => row.event === "attached" ? { ...row, bridge: 1 } : row),
    rows.map(row => row.event === "request" ? { ...row, teardown: true } : row),
    rows.map(row => row.event === "resolved" ? { ...row, op: "ack" } : row),
    rows.map(row => row.event === "resolved" ? { ...row, request: 2 } : row),
    renumber([rows[2], ...rows.slice(0, 2), ...rows.slice(3)]),
    renumber([...rows, ...rows]),
    renumber([...rows.slice(0, 4), rows[3], ...rows.slice(4)]),
    renumber([...rows, row("request", { bridge: 1, request: 2, op: "poll", teardown: true })]),
    rows.slice(0, -1),
    [null, ...rows.slice(1)],
    [false, ...rows.slice(1)],
    new Array(7),
  ];
  for (const events of cases) {
    const result = classified(events);
    assert.equal(result.classification, "diagnostic_inconclusive");
    assert.notEqual(result.trace_status, "complete");
  }
});

test("writer failures, missing receipts, counts, byte caps and waitpid veto clean prefixes", () => {
  const rows = base();
  const mutations = [
    completion => { delete completion.audit; },
    completion => { delete completion.audit.completed; },
    completion => { completion.audit.completed = false; },
    completion => { delete completion.waitpid; },
    completion => { completion.waitpid = { code: 1, signal: null }; },
    completion => { completion.waitpid = { code: null, signal: "SIGTERM" }; },
    completion => { completion.waitpid = { code: 0 }; },
    completion => { completion.audit.trace.invalid = true; },
    completion => { completion.audit.trace.truncated = true; },
    completion => { completion.audit.trace.events++; },
    completion => { completion.audit.trace.bytes++; },
    completion => { completion.audit.trace.bytes = 262145; },
    completion => { delete completion.audit.trace; },
  ];
  for (const mutate of mutations) {
    const completion = completionFor(rows); mutate(completion);
    assert.equal(classified(rows, { completion }).classification, "diagnostic_inconclusive");
  }
  assert.equal(classifyDiagnostic({ endpoint: true, exit: 0, trace: rows }).exit_status, "missing_waitpid");
  assert.equal(classified(rows, { endpoint: false }).classification, "diagnostic_inconclusive");
  assert.equal(classified(rows, { completion: undefined }).classification, "diagnostic_inconclusive");
  for (const mode of ["throw", "drop", "cap"]) {
    const persisted = [], all = [...rows, row("question_closed", { seq: 8, teardown: true })];
    const traceWriter = createTrace(line => {
      if (persisted.length === rows.length) { if (mode === "throw") throw Error("PRIVATE"); return; }
      persisted.push(JSON.parse(line));
    }, mode === "cap" ? { events: 8 } : {});
    for (const row of all) { const { seq: _seq, event, ...data } = row; traceWriter.emit(event, data); }
    const completion = completionFor(persisted); completion.audit.trace = traceWriter.snapshot();
    assert.equal(classified(persisted, { completion }).classification, "diagnostic_inconclusive", mode);
  }
});

test("malformed, private, orphaned, unfinished and uncorrelated probe streams veto clean", () => {
  const cases = [
    "invalid", [null], new Array(1),
    [probeRow(1, "refresh", "start", { raw: "PRIVATE" })],
    [probeRow(1, "refresh", "entry")],
    [probeRow(1, "refresh", "start")],
    [probeRow(1, "refresh", "rejected")],
    [probeRow(1, "refresh", "join", { parent: 0 }), probeRow(2, "refresh", "resolved")],
    [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved", { caller: "before_tool" })],
    [probeRow(1, "refresh", "start", { teardown: true }), probeRow(2, "refresh", "resolved")],
    [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved"), probeRow(3, "refresh", "resolved")],
    [probeRow(1, "cleanup_stop", "dispatch", { bridge: 1, request: 2, kind: "stop" }), probeRow(2, "cleanup_stop", "reply_resolved", { bridge: 1, request: 2, kind: "stop" })],
  ];
  for (const probes of cases) {
    const result = classified(base(), { probes });
    assert.equal(result.classification, "diagnostic_inconclusive");
    assert.notEqual(result.probe_status, "complete");
    assert.ok(!JSON.stringify(result).includes("PRIVATE"));
  }
  const probes = [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved")];
  for (const mode of ["missing", "invalid", "truncated", "count", "bytes"]) {
    const completion = completionFor(base(), probes);
    if (mode === "missing") delete completion.audit.probes;
    else if (mode === "count") completion.audit.probes.events++;
    else if (mode === "bytes") completion.audit.probes.bytes++;
    else completion.audit.probes[mode] = true;
    assert.equal(classified(base(), { completion, probes }).classification, "diagnostic_inconclusive");
  }
});

test("cleanup dispatch has unique request ownership and cannot borrow an ended caller", () => {
  const rows = trace(["attach", "attached", request(), resolved(), request(2, { op: "runtime", kind: "stop" }), resolved(2, { op: "runtime", kind: "stop" }), teardown(), close(), closed()]);
  const refresh = [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved")];
  const entry = phase => probeRow(1, "cleanup_stop", phase, { call: 2, caller: "shutdown" });
  const dispatch = (phase, call = 3) => probeRow(1, "cleanup_stop", phase, { call, parent: 2, caller: "shutdown", bridge: 1, request: 2, kind: "stop" });
  const cases = [
    [...refresh, entry("entry"), dispatch("dispatch"), dispatch("reply_resolved"), dispatch("dispatch", 4), dispatch("reply_resolved", 4), entry("returned")],
    [...refresh, entry("entry"), entry("returned"), dispatch("dispatch"), dispatch("reply_resolved")],
  ];
  for (const probes of cases) {
    const result = classified(rows, { probes: renumber(probes), scope: "observed_calls" });
    assert.equal(result.probe_status, "invalid");
    assert.equal(result.classification, "diagnostic_inconclusive");
  }
});

test("correlated stop requests and replies must agree on teardown boundary flags", () => {
  const rows = trace(["attach", "attached", request(), resolved(), request(2, { op: "runtime", kind: "stop" }), resolved(2, { op: "runtime", kind: "stop" }), teardown(), close(), closed()]);
  for (const mode of ["matching", "dispatch-mismatch", "reply-mismatch"]) {
    const dispatchTeardown = mode === "dispatch-mismatch", replyTeardown = mode !== "matching";
    const probes = [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "resolved"),
      probeRow(3, "cleanup_stop", "entry", { call: 2, caller: "shutdown", teardown: dispatchTeardown }),
      probeRow(4, "cleanup_stop", "dispatch", { call: 3, parent: 2, caller: "shutdown", bridge: 1, request: 2, kind: "stop", teardown: dispatchTeardown }),
      probeRow(5, "cleanup_stop", "reply_resolved", { call: 3, parent: 2, caller: "shutdown", bridge: 1, request: 2, kind: "stop", teardown: replyTeardown }),
      probeRow(6, "cleanup_stop", "returned", { call: 2, caller: "shutdown", teardown: replyTeardown })];
    const result = classified(rows, { probes, scope: "observed_calls" });
    assert.equal(result.classification, mode === "matching" ? "diagnostic_clean" : "diagnostic_inconclusive");
    assert.equal(result.probe_status, mode === "matching" ? "complete" : "incomplete");
  }
});

test("oversized streams retain positive evidence from their bounded validated prefix", () => {
  const rows = trace(["attach", "attached", request(), rejected(), teardown(), close(), closed()]);
  const oversizedTrace = [...rows, ...new Array(1018).fill(null)];
  const traceResult = classified(oversizedTrace);
  assert.equal(traceResult.trace_status, "invalid");
  assert.equal(traceResult.pre_teardown_events, "observed");
  assert.equal(traceResult.counts.rejected, 1);
  assert.equal(traceResult.classification, "diagnostic_inconclusive");
  const probes = [probeRow(1, "refresh", "start"), probeRow(2, "refresh", "rejected"), ...new Array(1023).fill(null)];
  const probeResult = classified(base(), { probes });
  assert.equal(probeResult.probe_status, "invalid");
  assert.equal(probeResult.pre_teardown_events, "observed");
  assert.equal(probeResult.counts.probe_failures, 1);
  assert.equal(probeResult.classification, "diagnostic_inconclusive");
});

test("hidden serializers and malformed packet getters cannot escape or replace product outcomes", async () => {
  const rows = base(), completion = completionFor(rows);
  let serialized = 0;
  Object.defineProperty(rows[0], "toJSON", { value() { serialized++; throw Error("PRIVATE serializer"); } });
  const result = classifyDiagnostic({ endpoint: true, completion, trace: rows });
  assert.equal(serialized, 0);
  assert.equal(result.trace_status, "invalid");
  assert.equal(result.classification, "diagnostic_inconclusive");
  for (const rejects of [false, true]) {
    const { probe, rows } = captureProbe(), error = Error("original product error"), value = {};
    const originalPromise = rejects ? Promise.reject(error) : Promise.resolve(value);
    const packet = { get op() { throw Error("PRIVATE metadata"); } };
    let calls = 0;
    const original = { request(actual) { assert.equal(this, original); assert.equal(actual, packet); calls++; return originalPromise; }, close() {} };
    const bridge = observeBridge(original, { teardown: false }, 1, probe);
    const returned = bridge.request(packet);
    assert.equal(returned, originalPromise);
    if (rejects) await assert.rejects(returned, e => e === error); else assert.equal(await returned, value);
    assert.equal(calls, 1);
    assert.equal(probe.snapshot().invalid, true);
    assert.deepEqual(rows, []);
  }
});

test("positive refresh and local cleanup failures survive joins and later success without invented phases", () => {
  for (const [type, opening, ending] of [["refresh", "start", "rejected"], ["refresh", "start", "threw"], ["cleanup_stop", "entry", "threw"], ["cleanup_stop", "entry", "rejected"]]) {
    const probes = [probeRow(1, type, opening), probeRow(2, type, ending),
      probeRow(3, "refresh", "start", { call: 2 }), probeRow(4, "refresh", "resolved", { call: 2 })];
    const result = classified(base(), { probes });
    assert.equal(result.probe_status, "complete");
    assert.equal(result.pre_teardown_events, "observed");
    assert.equal(result.classification, "diagnostic_failure_observed");
    assert.equal(result.cause_claim, "none");
    assert.notEqual(result.refresh_phase, "poll_rejected");
    assert.notEqual(result.refresh_phase, "pre_poll_local_failure");
    assert.notEqual(result.cleanup_stop_phase, "timeout_or_closed");
    const partial = classified(base(), { probes: [...probes, null] });
    assert.equal(partial.pre_teardown_events, "observed");
    assert.equal(partial.classification, "diagnostic_inconclusive");
  }
});

test("refresh probes retain original promise and error identity with explicit join attribution", async () => {
  for (const rejects of [false, true]) {
    const { rows, probe } = captureProbe(), gate = latch(), state = { teardown: false };
    const error = Error("PRIVATE");
    const runtime = { refreshing: null, refresh() {
      if (this.refreshing) return this.refreshing;
      this.refreshing = gate.promise.finally(() => { this.refreshing = null; });
      return this.refreshing;
    } };
    assert.equal(observeRefresh(runtime, probe, state), true);
    assert.equal(observeRefresh(runtime, probe, state), false);
    const first = runtime.refresh("before_tool"), joined = runtime.refresh("turn_end");
    assert.equal(first, joined);
    if (rejects) { gate.reject(error); await assert.rejects(first, e => e === error); }
    else { gate.resolve(); await first; }
    assert.deepEqual(rows.map(row => [row.phase, row.call, row.parent]), [["start", 1, 0], ["join", 2, 1], [rejects ? "rejected" : "resolved", 1, 0], [rejects ? "rejected" : "resolved", 2, 1]]);
    const result = classified(base(), { probes: rows });
    assert.equal(result.probe_status, "complete");
    assert.equal(result.refresh_phase, rejects ? "unknown" : "join");
    assert.equal(result.classification, rejects ? "diagnostic_failure_observed" : "diagnostic_clean");
  }
});

test("cleanup caller, reply and subsequent local failure are distinct; async frames do not borrow callers", async () => {
  for (const mode of ["resolved", "reply-rejected", "local-rejected", "async-dispatch", "sync-throw"]) {
    const { rows, probe } = captureProbe(), state = { teardown: false }, error = Error("PRIVATE");
    const traceRows = trace(["attach", "attached", request(), resolved(), request(2, { op: "runtime", kind: "stop" }),
      ...(mode === "reply-rejected" ? [rejected(2, { op: "runtime", kind: "stop" })] : [resolved(2, { op: "runtime", kind: "stop" })]), teardown(), close(), closed()]);
    const original = { request(packet) { return mode === "reply-rejected" && packet.op === "runtime" ? Promise.reject(error) : Promise.resolve("ok"); }, close() {} };
    const bridge = observeBridge(original, state, 1, probe);
    await bridge.request({ op: "poll" }); // Keep request ordinals aligned with the trace.
    let originalPromise;
    const delegate = observeDelegation({ shutdown() {
      if (mode === "sync-throw") throw error;
      originalPromise = (async () => {
        if (mode === "async-dispatch") await Promise.resolve();
        await bridge.request({ op: "runtime", command: { kind: "stop" } });
        if (mode === "local-rejected") throw error;
        return "done";
      })();
      return originalPromise;
    }, failed() {} }, state, probe);
    if (mode === "sync-throw") assert.throws(() => delegate.shutdown(), e => e === error);
    else {
      const result = delegate.shutdown(); assert.equal(result, originalPromise);
      if (["reply-rejected", "local-rejected"].includes(mode)) await assert.rejects(result, e => e === error);
      else assert.equal(await result, "done");
    }
    assert.equal(state.cleanupCaller, undefined);
    const dispatch = rows.find(row => row.phase === "dispatch");
    if (dispatch) {
      assert.equal(dispatch.caller, mode === "async-dispatch" ? "unknown" : "shutdown");
      assert.equal(dispatch.parent, mode === "async-dispatch" ? 0 : 1);
    }
    const result = classified(traceRows, { probes: rows });
    assert.equal(result.probe_status, "complete");
    assert.equal(result.cleanup_stop_phase, mode === "reply-rejected" ? "reply_rejected" : mode === "sync-throw" ? "entry" : "reply_resolved");
    assert.equal(result.classification, ["resolved", "async-dispatch"].includes(mode) ? "diagnostic_clean" : "diagnostic_failure_observed");
    assert.ok(!JSON.stringify(rows).includes("PRIVATE"));
  }
});

test("probe writer caps, sticky errors, private mode and receipts prevent silent-prefix qualification", () => {
  for (const limits of [{ events: 1 }, { bytes: 1 }]) {
    const rows = [], probe = createDiagnosticProbe(line => rows.push(JSON.parse(line)), limits);
    probe.emit("refresh", "start", { call: probe.allocateCall() });
    probe.emit("refresh", "resolved", { call: 1 });
    assert.equal(probe.snapshot().invalid, true);
    assert.equal(probe.snapshot().truncated, true);
    const completion = completionFor(base(), rows); completion.audit.probes = probe.snapshot();
    assert.equal(classified(base(), { probes: rows, completion }).classification, "diagnostic_inconclusive");
  }
  for (const silent of [false, true]) {
    const rows = [], probe = createDiagnosticProbe(line => {
      if (rows.length === 1) { if (!silent) throw Error("PRIVATE"); return; }
      rows.push(JSON.parse(line));
    });
    probe.emit("refresh", "start", { call: probe.allocateCall() }); probe.emit("refresh", "resolved", { call: 1 });
    const completion = completionFor(base(), rows); completion.audit.probes = probe.snapshot();
    assert.equal(classified(base(), { probes: rows, completion }).classification, "diagnostic_inconclusive");
    assert.equal(probe.snapshot().events, 2);
    if (!silent) assert.equal(probe.snapshot().invalid, true);
  }
  const dir = mkdtempSync(join(tmpdir(), "diagnostic-probe-"));
  try {
    const path = join(dir, "probe"), probe = privateProbe(path);
    probe.emit("refresh", "start", { call: probe.allocateCall() }); probe.close();
    assert.equal(statSync(path).mode & 0o777, 0o600);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("real registration distinguishes outer failures without claiming an unobserved internal phase", async () => {
  for (const mode of ["observation", "post-reply", "failed-callback", "status"]) {
    const handlers = new Map(), state = { teardown: false }, traceRows = [], error = Error("PRIVATE");
    const trace = createTrace(line => traceRows.push(JSON.parse(line))), { rows: probes, probe } = captureProbe();
    let runtime, armed = false;
    const binding = { instance_id: "fixture", profile: "fixture", native_id: "fixture", launch_id: randomUUID() };
    const bootstrap = { version: 1, binding, aoe_bin: "/unused", app_dir: "/unused" };
    const original = { async request() { return { policy: null }; }, async close() {} };
    const delegate = { status() {}, attach(r) { runtime = r; }, observation() { if (armed && mode !== "post-reply") throw error; },
      async poll() { if (armed && mode === "post-reply") throw error; }, failed() { if (mode === "failed-callback") throw error; }, async shutdown() {} };
    const ctx = { hasUI: true, sessionManager: { getSessionId: () => "fixture" }, ui: { setStatus() { if (armed && mode === "status") throw error; } } };
    registerAfk({ registerFlag() {}, getFlag: () => JSON.stringify(bootstrap), on: (event, fn) => handlers.set(event, fn) }, {
      delegation: traceDelegation(observeDelegation(delegate, state, probe), trace, state),
      bridge: () => traceBridge(original, trace, state), schedule: () => () => {},
    });
    await handlers.get("session_start")({}, ctx);
    assert.equal(observeRefresh(runtime, probe, state), true);
    armed = true;
    const refreshing = runtime.refresh();
    if (["failed-callback", "status"].includes(mode)) await assert.rejects(refreshing, e => e === error);
    else await refreshing;
    if (mode === "status") assert.equal(runtime.refreshing, refreshing, "fixture must not fix the product's original stuck coalescing");
    armed = false; state.teardown = true; trace.emit("teardown", { teardown: true });
    await handlers.get("session_shutdown")();
    const result = classified(traceRows, { probes });
    assert.equal(result.pre_teardown_events, "observed");
    assert.notEqual(result.classification, "diagnostic_clean");
    assert.equal(result.refresh_phase, ["failed-callback", "status"].includes(mode) ? "unknown" : "resolved");
    assert.equal(result.cause_claim, "none");
  }
});

test("stacked wrappers synchronize a new teardown marker between dispatch and settlement", async () => {
  for (const rejects of [false, true]) {
    const rows = [], { rows: probes, probe } = captureProbe(), gate = latch();
    const trace = createTrace(line => rows.push(JSON.parse(line)));
    const state = { teardown: false, published: false, syncTeardown() {
      if (this.published && !this.teardown) { this.teardown = true; trace.emit("teardown", { teardown: true }); }
    } };
    const original = { request: () => gate.promise, close: () => Promise.resolve() };
    const bridge = traceBridge(observeBridge(original, state, 1, probe), trace, state);
    trace.emit("attach"); trace.emit("attached");
    const pending = bridge.request({ op: "runtime", command: { kind: "stop" } });
    assert.equal(pending, gate.promise);
    assert.equal(rows.find(row => row.event === "request").teardown, false);
    assert.equal(probes.find(row => row.phase === "dispatch").teardown, false);
    state.published = true;
    const error = Error("PRIVATE controlled reply");
    if (rejects) { gate.reject(error); await assert.rejects(pending, e => e === error); }
    else { gate.resolve(); await pending; }
    await bridge.close();
    assert.equal(rows.find(row => row.event === (rejects ? "rejected" : "resolved")).teardown, true);
    assert.equal(probes.find(row => row.phase === (rejects ? "reply_rejected" : "reply_resolved")).teardown, true);
    assert.equal(rows.filter(row => row.event === "teardown").length, 1);
    const result = classified(rows, { probes });
    assert.equal(result.trace_status, "complete");
    assert.equal(result.probe_status, "complete");
  }
});

test("diagnostic field projection retains only bounded report fields", () => {
  const fields = diagnosticFields({ classification: "diagnostic_clean", cause_claim: "PRIVATE", private_payload: "SECRET", cleanup_stop_phase: "reply_rejected", refresh_phase: "poll_rejected" });
  assert.equal(fields.cause_claim, "unknown");
  assert.equal(fields.cleanup_stop_phase, "reply_rejected");
  assert.equal(fields.refresh_phase, "unknown");
  assert.equal(fields.classification, "diagnostic_clean");
  assert.equal(Object.hasOwn(fields, "private_payload"), false);
});
