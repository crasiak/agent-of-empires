import test from "node:test";
import assert from "node:assert/strict";
import { KIND, sha, digest, qualifyEndpoint, classifyNative, receiptBridge, intentRelation, createPublicationAudit } from "./aoe-afk-native-contract.mjs";
import { createTrace, validEvent } from "./aoe-afk-observer-fixture.mjs";
import { createDiagnosticProbe, validProbe } from "./aoe-afk-observer-probes.mjs";
import { parsePhysicalRows, projectCapture } from "./aoe-afk-native-report.mjs";
import { fixedProvider } from "./aoe-afk-native-provider.mjs";

const id = n => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
function endpoint() {
  const binding = { instance_id: "session", profile: "default", native_id: id(1), launch_id: id(2) };
  const generation = id(3), window = id(4), operation = id(5), request = id(6);
  const input = { question: "Disposable question" };
  const identity = { operation: id(7), interaction_id: id(8), tool_call_id: "fixture-question-1", context_ref: "abc123", kind: "input", arguments_hash: sha(JSON.stringify(input)) };
  const grant = { version: 4, requests: 2, question_deferrals: 1, settlement_nudges: 0, files: [{ path: "scratch.txt", capability: "create" }] };
  const policy = { usd: { limit_nano: 100 } };
  const preview = { format: "aoe-afk-batch-preview", version: 3, operation_id: operation, profile: "default", minutes: 1, shared_requests: null,
    members: [{ session_id: "session", grant_hash: "a".repeat(64), grant, usage_guard: policy }] };
  preview.digest = digest(preview);
  const question = { identity, admitted_at_ms: 1, emission_intent_at_ms: 2, outcome_at_ms: 3, outcome: "accepted", observed_request: request, recovered_at_ms: null };
  const member = { window, binding, generation, grant_hash: "a".repeat(64), outcome: "confirmed" };
  const receipt = (kind, extra, value) => ({ kind: KIND, binding, generation, command: { kind, window, ...extra }, value, publication_clean: true, warnings: [] });
  const intent = { ...receipt("question_intent", { identity }, { identity, guarded_return: true }), valid: true };
  const opened = { version: 1, toolCallId: identity.tool_call_id, interactionId: identity.interaction_id, owner: { namespace: "pi-ask-user", id: identity.interaction_id }, kind: "input", supported: true, uiPromptAttribution: true, state: "pending" };
  const file = "/private/native.jsonl";
  return JSON.parse(JSON.stringify({ kind: KIND, expected: { binding, generation, window, operation_id: operation, preview_digest: preview.digest,
    grant_hash: member.grant_hash, usage_policy_hash: digest([policy, null]), scratch_path: "/private/work/scratch.txt" }, preview,
    authority: { capture: "bounded-read-only", registry_valid: true, workspace_valid: true, grant_hash: "a".repeat(64), stable: true, pending_absent: true, scratch_absent: true, scratch_path: "/private/work/scratch.txt",
      snapshot: { version: 3, profile: "default", sessions: { session: { book: { windows: [{ id: window, binding, generation, confirmed: true, state: "owned", grant, issued_at_ms: 0, expires_at_ms: 60000,
        question, reservations: [request], records: [], permit: null, nudges: [] }] } } }, batches: { [operation]: { id: operation, preview_digest: preview.digest, shared_requests: null,
        stopped: false, members: { session: member }, tickets: [] } } } },
    native: { ready: true, native_id: binding.native_id, file, opened, prompt_start: { owner: opened.owner, kind: "input" }, prompt_end: { owner: opened.owner, kind: "input" },
      closed: { ...opened, state: "deferred" }, reply: { version: 1, requestId: identity.operation, method: "defer", status: "accepted", reason: "deferred_by_afk", toolCallId: identity.tool_call_id, interactionId: identity.interaction_id },
      result: { toolName: "ask_user", toolCallId: identity.tool_call_id, input, isError: false, details: { toolCallId: identity.tool_call_id, interactionId: identity.interaction_id,
        status: "no_human_answer", reason: "deferred_by_afk", response: null, cancelled: true } }, receipts: [intent, receipt("reserve", { request, question: identity }, { request_number: 1, human_required: true, grant, unanswered_gate: question }),
        receipt("question_admit", { identity }, { identity, admitted_at_ms: 1 }), receipt("question_outcome", { identity, outcome: "accepted" }, { identity, outcome: "accepted" })] },
    usage: { ready: true, current_source: true, unresolved_absent: true, native_result: true, native_file: file, binding, generation, window, policy,
      policy_hash: digest([policy, null]), source: { identity: { native_id: binding.native_id, file: sha(file) }, binding, generation, gap: null, announced: {}, cursor: { count: 1, hash: "b".repeat(64) }, high_water: 1 },
      native_head: { count: 1, hash: "b".repeat(64) } } }));
}

test("native endpoint conjunction refuses independently mutated evidence", () => {
  assert.equal(qualifyEndpoint(endpoint()).endpoint_status, "qualified");
  const mutations = [
    e => e.kind = "controlled",
    e => e.authority.pending_absent = false,
    e => e.authority.stable = false,
    e => e.authority.registry_valid = false,
    e => e.authority.workspace_valid = false,
    e => e.authority.grant_hash = "c".repeat(64),
    e => e.native.receipts[2].value.admitted_at_ms = 0,
    e => e.native.receipts[3].command.outcome = "ordinary",
    e => e.native.receipts[1].value.unanswered_gate.identity.operation = id(20),
    e => e.native.receipts[1].value.request_number = 2,
    e => e.authority.scratch_absent = false,
    e => e.authority.snapshot.profile = "other",
    e => e.preview.digest = "c".repeat(64),
    e => e.preview.members[0].grant.requests = 3,
    e => e.native.ready = false,
    e => e.native.reply.status = "already_settled",
    e => e.native.reply.requestId = id(20),
    e => e.native.closed.state = "cancelled",
    e => e.native.closed.owner.id = id(20),
    e => e.native.prompt_end.owner.namespace = "other",
    e => e.native.prompt_start.kind = "select",
    e => e.native.result.details.response = "answer",
    e => e.native.result.details.reason = "ordinary",
    e => e.native.result.details.cancelled = false,
    e => e.native.result.isError = true,
    e => e.native.result.input.question = "different",
    e => e.native.receipts[0].warnings.push("data committed, cleanup uncertain"),
    e => e.native.receipts[0].value.guarded_return = false,
    e => e.native.receipts[0].command.window = id(20),
    e => e.native.receipts.push(structuredClone(e.native.receipts[1])),
    e => e.native.receipts[1].command.request = id(20),
    e => e.native.receipts[1].command.question.operation = id(20),
    e => e.usage.ready = false,
    e => e.usage.current_source = false,
    e => e.usage.unresolved_absent = false,
    e => e.usage.source.gap = "error_or_abort",
    e => e.usage.source.identity.file = "c".repeat(64),
    e => e.usage.native_head.count = 2,
    e => e.usage.policy_hash = "c".repeat(64),
    e => e.usage.native_result = false,
  ];
  for (const key of ["profile", "instance_id", "native_id", "launch_id"]) mutations.push(e => e.expected.binding[key] = "wrong");
  for (const key of ["generation", "window", "grant_hash", "usage_policy_hash", "operation_id", "preview_digest"]) mutations.push(e => e.expected[key] = "wrong");
  const window = e => e.authority.snapshot.sessions.session.book.windows[0];
  for (const key of ["operation", "interaction_id", "tool_call_id", "context_ref", "kind", "arguments_hash"]) mutations.push(e => window(e).question.identity[key] = "wrong");
  mutations.push(e => window(e).question.emission_intent_at_ms = null, e => window(e).question.outcome = "unknown",
    e => window(e).question.observed_request = id(20), e => window(e).reservations = [], e => window(e).reservations.push(id(20)),
    e => window(e).records.push({}), e => window(e).permit = {}, e => window(e).nudges.push({}), e => window(e).confirmed = false);
  for (const mutate of mutations) { const e = endpoint(); mutate(e); assert.notEqual(qualifyEndpoint(e).endpoint_status, "qualified", mutate.toString()); }
});

test("receipt reaction is synchronous before caller refresh and preserves exact promise and uncertainty", async () => {
  for (const scenario of ["clean", "warning", "identity", "guard", "observer-throws", "reject"]) {
    const e = endpoint(), r = e.native.receipts[0], order = [], receipts = [];
    const trace = createTrace(() => {}), probes = createDiagnosticProbe(() => {});
    let resolve, reject;
    const original = new Promise((yes, no) => { resolve = yes; reject = no; });
    let faults = 0;
    const bridge = receiptBridge({ request: () => original, close: () => original }, { binding: r.binding, generation: r.generation, trace, probes,
      save(value) { order.push("receipt"); receipts.push(value); if (scenario === "observer-throws") throw Error("observer failure"); }, fault() { faults++; } });
    const returned = bridge.request({ op: "runtime", command: r.command });
    assert.equal(returned, original); assert.equal(bridge.close(), original);
    returned.then(() => order.push("refresh"), () => order.push("rejected"));
    const value = structuredClone(r.value);
    if (scenario === "warning") value.publication_warnings = ["commit positive, cleanup uncertain"];
    if (scenario === "identity") value.identity.context_ref = "other";
    if (scenario === "guard") value.guarded_return = false;
    if (scenario === "reject") reject(Error("host refused")); else resolve(value);
    await returned.catch(() => {});
    if (scenario === "reject") { assert.deepEqual(receipts, []); continue; }
    assert.deepEqual(order, ["receipt", "refresh"]);
    assert.equal(receipts[0].valid, ["clean", "observer-throws"].includes(scenario));
    assert.deepEqual(receipts[0].trace, { events: 0, bytes: 0 });
    assert.equal(receipts[0].data_commit, true);
    if (scenario === "warning") assert.deepEqual(receipts[0].warnings, value.publication_warnings);
    assert.equal(faults, scenario === "observer-throws" ? 1 : 0);
  }
});

test("independent trace and probe positions preserve unknown for straddling, pending and malformed streams", () => {
  for (const type of ["trace", "probes"]) {
    const rows = [], writer = type === "trace" ? createTrace(line => rows.push(JSON.parse(line))) : createDiagnosticProbe(line => rows.push(JSON.parse(line)));
    const emit = start => type === "trace" ? writer.emit(start ? "request" : "rejected", { bridge: 1, request: 1, op: "poll" })
      : writer.emit("refresh", start ? "start" : "rejected", { call: 1 });
    emit(true);
    const middle = { events: rows.length, bytes: writer.snapshot().bytes };
    emit(false);
    const validate = type === "trace" ? validEvent : validProbe;
    const relation = (boundary, start = 1, end = 2, events = rows, health = writer.snapshot()) => intentRelation(events, boundary, validate, start, end, health);
    assert.equal(relation({ events: 0, bytes: 0 }), "post_intent");
    assert.equal(relation({ events: 2, bytes: writer.snapshot().bytes }), "pre_intent");
    for (const boundary of [null, {}, middle, { events: -1, bytes: 0 }, { events: 3, bytes: 0 }, { events: 0, bytes: 1 }]) assert.equal(relation(boundary), "unknown");
    assert.equal(relation({ events: 0, bytes: 0 }, 1, null), "unknown");
    assert.equal(relation({ events: 0, bytes: 0 }, 1, 3), "unknown");
    assert.equal(relation({ events: 0, bytes: 0 }, 1, 2, [...rows].reverse()), "unknown");
    assert.equal(relation({ events: 0, bytes: 0 }, 1, 2, rows, { ...writer.snapshot(), invalid: true }), "unknown");
  }
});

test("native audit rejects controlled crossover and keeps lifecycle facts separate", () => {
  const e = endpoint();
  const proc = { manifest: "d".repeat(64), role: "baseline", ordinal: 1, waitpid: 0, audit_eof: true, control_eof: true, audit_valid: true, cleanup: true, escalated: false, controller_lost: false, io_errors: [] };
  const audit = { kind: KIND, version: 1, manifest: proc.manifest, role: "baseline", ordinal: 1, waitArmed: true, cancelled: true, consumed: true, settled: true, off: true, shutdown: true, writersClosed: true, fault: false, publication: { uncertain: false, truncated: false, receipts: [] } };
  assert.equal(classifyNative({ endpoint: e, process: proc, audit }).classification, "native_endpoint_only");
  for (const publication of [undefined, null, {}, { uncertain: false, truncated: true, receipts: [] }]) {
    const result = classifyNative({ endpoint: e, process: proc, audit: { ...audit, publication } });
    assert.equal(result.classification, "native_inconclusive");
    assert.equal(result.publication_status, "invalid");
  }
  for (const changed of [{ ...proc, controller_lost: true }, { ...proc, io_errors: ["fixed I/O fault"] }]) {
    assert.equal(classifyNative({ endpoint: e, process: changed, audit }).classification, "native_inconclusive");
  }
  for (const key of ["waitArmed", "cancelled", "consumed", "settled", "off", "shutdown", "writersClosed"]) {
    assert.equal(classifyNative({ endpoint: e, process: proc, audit: { ...audit, [key]: false } }).classification, "native_inconclusive", key);
  }
  for (const key of ["audit_eof", "control_eof", "audit_valid", "cleanup"]) assert.equal(classifyNative({ endpoint: e, process: { ...proc, [key]: false }, audit }).classification, "native_inconclusive", key);
  assert.equal(classifyNative({ endpoint: e, process: proc, audit: { ...audit, kind: "controlled" } }).classification, "native_inconclusive");
  const result = classifyNative({ endpoint: e, process: { ...proc, role: "observed" }, audit: { ...audit, role: "observed" } });
  assert.equal(result.classification, "native_inconclusive");
  assert.equal(result.endpoint_status, "qualified");
  assert.equal(result.cause_claim, "none");
});

test("physical ingestion retains positive prefix through corrupt, overlong and incomplete suffix", () => {
  const rows = [], trace = createTrace(line => rows.push(JSON.parse(line)));
  trace.emit("request", { bridge: 1, request: 1, op: "poll" });
  trace.emit("rejected", { bridge: 1, request: 1, op: "poll" });
  const prefix = rows.map(r => JSON.stringify(r) + "\n").join("");
  for (const [suffix, complete] of [["{broken}\n", true], ["{", true], ["", false], ["\xff\n", true]]) {
    const parsed = parsePhysicalRows(Buffer.from(prefix + suffix, "binary"), complete);
    assert.deepEqual(parsed.slice(0, 2), rows);
    assert.equal(parsed.some(r => r.invalid), true);
    const result = classifyNative({ trace: parsed });
    assert.equal(result.diagnostics.pre_teardown_events, "observed");
    assert.equal(result.classification, "native_inconclusive");
  }
});

class FakeStream {
  events = [];
  constructor() { this.done = new Promise(resolve => { this.finish = resolve; }); }
  push(event) { this.events.push(structuredClone(event)); if (["done", "error"].includes(event.type)) this.finish(event.message ?? event.error); }
  end(message) { if (message) this.finish(message); }
  result() { return this.done; }
}

test("fixed provider emits the exact ordinary question, holds only an abortable no-effect stream and rejects extras", async () => {
  const config = { model: { id: "fixed", provider: "local", api: "local" }, question: { question: "Do not answer" }, ordinary_usage: { input: 1, output: 1 } };
  const state = { calls: 0 }, markers = new Map(), order = [];
  let release;
  const pendingMarker = new Promise(resolve => { release = resolve; });
  const provider = fixedProvider(FakeStream, config, state, { native: () => ({ real: "callback fixture" }), fault: () => { state.fault = true; },
    save(name, value) { markers.set(name, value); if (name === "pending.json") release(); } });
  const firstSignal = new AbortController();
  const options = signal => ({ signal, onPayload: value => { order.push(`payload${value.invocation}`); return value; }, onResponse: () => order.push("response") });
  const first = provider(config.model, {}, options(firstSignal.signal));
  const result = await first.result();
  assert.deepEqual(first.events.map(e => e.type), ["start", "toolcall_start", "toolcall_delta", "toolcall_end", "done"]);
  assert.deepEqual(result.content, [{ type: "toolCall", id: "fixture-question-1", name: "ask_user", arguments: config.question }]);
  assert.deepEqual(result.usage, config.ordinary_usage);
  const secondSignal = new AbortController();
  const second = provider(config.model, {}, options(secondSignal.signal));
  await pendingMarker;
  assert.deepEqual(second.events.map(e => e.type), ["start"]);
  assert.deepEqual(markers.get("pending.json"), { calls: 2, active: true });
  assert.deepEqual(order, ["payload1", "response", "payload2", "response"]);
  assert.equal(state.episode.consumed, false);
  secondSignal.abort();
  const aborted = await second.result();
  assert.equal(aborted.stopReason, "aborted");
  assert.equal(state.episode.consumed, true);
  assert.deepEqual(aborted.content, []);
  assert.equal(state.cancelled, true);
  assert.throws(() => provider(config.model, {}, options(new AbortController().signal)), /unexpected/);
  assert.equal(state.fault, true);
});

test("independent capture projection checks persisted payload, registry, lease, source entries and Rust grant serialization", () => {
  const e = endpoint(), b = e.expected.binding, w = e.authority.snapshot.sessions.session.book.windows[0];
  w.grant = { version: 4, task: "diagnostic", scope: "scratch only", files: [{ path: "scratch.txt", capability: "create" }], requests: 2, settlement_nudges: 0, question_deferrals: 1, assurance: "stock-sdk-main-loop-reservations" };
  w.root = "/private/work"; w.root_identity = [1, 2];
  const grantHash = sha(JSON.stringify(w.grant));
  e.preview.members[0].grant = w.grant; e.preview.members[0].grant_hash = grantHash;
  const content = { ...e.preview }; delete content.digest;
  e.preview.digest = digest(content);
  const batch = e.authority.snapshot.batches[e.expected.operation_id];
  batch.preview_digest = e.preview.digest; batch.members.session.grant_hash = grantHash;
  e.native.receipts[1].value.grant = w.grant;
  const nativeRows = [{ type: "session", id: b.native_id },
    { id: "a", type: "message", parentId: null, message: { role: "assistant", api: "fixed", provider: "fixed", model: "fixed", stopReason: "toolUse", content: [{ type: "toolCall", id: "fixture-question-1", name: "ask_user", arguments: e.native.result.input }], usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2 } } },
    { id: "b", type: "message", parentId: "a", message: { role: "toolResult", toolCallId: "fixture-question-1", details: e.native.result.details, isError: false } }];
  const source = { ...e.usage.source, entries: [] };
  const capture = { bootstrap: { binding: b }, snapshot: e.authority.snapshot, native_rows: nativeRows, native_file: e.native.file,
    ready: { binding: b, generation: e.expected.generation, native_id: b.native_id },
    apply: { operation_id: batch.id, preview_digest: e.preview.digest, profile: "default", members: [{ session_id: "session", window: w.id, host_confirmed: true, outcome: "confirmed" }] },
    attempt: { manifest: "d".repeat(64), role: "baseline", ordinal: 1 },
    approved_preview_digest: e.preview.digest, registry: [{ id: "session", tool: "pi", command: "", agent_session_id: b.native_id, active_execution: { launch_id: b.launch_id, binding: { agent: "pi", stores: ["/private/sessions"], cwd: w.root, filesystem: "host" } }, project_path: w.root }],
    work: w.root, work_identity: w.root_identity, stable: true, pending_absent: true, scratch_absent: true, scratch_path: e.expected.scratch_path, unresolved_absent: true,
    clock: { boot: "boot", millis: 1000 }, hello: { binding: b, generation: e.expected.generation, incarnation: "incarnation", boot: "boot" },
    lease: { binding: b, generation: e.expected.generation, incarnation: "incarnation", source: "source", cursor: {}, clock: { boot: "boot", millis: 999 } } };
  capture.approval = { kind: KIND, ...capture.attempt, operation_id: batch.id, preview_digest: e.preview.digest, approve_apply: true };
  capture.runtime_probe = { version: 6, binding: b, challenge: id(88), window: w.id, grant_hash: grantHash, budgeted: false, usage_policy_hash: digest([e.usage.policy, null]) };
  capture.runtime_ack = { ...capture.runtime_probe, generation: e.expected.generation, sdk: "0.87.1", coverage: "stock-sdk-main-loop-reservations", usage_coverage: "native-entries-terminal-v2" };
  capture.snapshot.usage = { windows: { [w.id]: { binding: b, generation: e.expected.generation, source: "source", policy: e.usage.policy, gap: null, blocked: null, closing: false, cutoff: null, ordinary_recovered: false,
    baseline: { count: 0, hash: sha("aoe-afk-usage-v1") }, observed_through: null } }, sources: { source } };
  // Derive source normalization using the same source-entry contract, not a readiness Boolean.
  const projected = projectCapture(capture, e.native, e.preview);
  source.cursor = projected.usage.native_head; source.high_water = source.cursor.count;
  capture.lease.cursor = source.cursor;
  capture.snapshot.usage.windows[w.id].observed_through = source.cursor;
  // Import normalization through the public producer, also used by the Rust-side chain protocol.
  return import("./aoe-afk-usage.mjs").then(({ normalize }) => {
    source.entries = nativeRows.slice(1).map(normalize);
    assert.equal(qualifyEndpoint(projectCapture(capture, e.native, e.preview)).endpoint_status, "qualified");
    const mutations = [
      c => c.registry[0].command = "wrapper",
      c => c.registry[0].active_execution.launch_id = id(99),
      c => c.work_identity[1] = 999,
      c => c.native_rows[1].message.content[0].arguments.question = "changed persisted arguments",
      c => c.native_rows[2].message.details.response = "not null",
      c => c.snapshot.usage.sources.source.entries[0].kind = "metadata",
      c => c.hello.incarnation = "other",
      c => c.snapshot.usage.sources.source.announced = { 100: "a".repeat(64) },
      c => c.snapshot.usage.windows[w.id].observed_through = null,
      c => c.snapshot.usage.windows[w.id].ordinary_recovered = true,
      c => c.snapshot.usage.windows[w.id].baseline.hash = "a".repeat(64),
      c => c.lease.clock.boot = "other",
      c => c.clock.millis = 10000,
      c => c.ready.generation = id(99),
      c => c.runtime_ack.sdk = "other",
      c => c.runtime_ack.usage_coverage = "unknown",
      c => c.runtime_probe.challenge = id(99),
      c => c.runtime_probe.usage_policy_hash = "a".repeat(64),
      c => c.approval.preview_digest = "a".repeat(64),
      c => c.apply.members[0].window = id(99),
    ];
    for (const change of mutations) {
      const copy = JSON.parse(JSON.stringify(capture)); change(copy);
      assert.notEqual(qualifyEndpoint(projectCapture(copy, e.native, e.preview)).endpoint_status, "qualified", change.toString());
    }
  });
});

test("native promotion validates exact attempt identity, semantic coverage and causal pairs", () => {
  const e = endpoint(), traceRows = [], probeRows = [];
  const trace = createTrace(line => traceRows.push(JSON.parse(line)));
  const probe = createDiagnosticProbe(line => probeRows.push(JSON.parse(line)));
  trace.emit("attach"); trace.emit("attached");
  trace.emit("request", { bridge: 1, request: 1, op: "poll" });
  trace.emit("rejected", { bridge: 1, request: 1, op: "poll" });
  probe.emit("refresh", "start", { call: 1 }); probe.emit("refresh", "resolved", { call: 1 });
  trace.emit("teardown", { teardown: true });
  probe.emit("cleanup_stop", "entry", { call: 2, teardown: true }); probe.emit("cleanup_stop", "returned", { call: 2, teardown: true });
  trace.emit("close", { bridge: 1, teardown: true }); trace.emit("closed", { bridge: 1, teardown: true });
  e.native.receipts[0].trace = { events: 0, bytes: 0 }; e.native.receipts[0].probes = { events: 0, bytes: 0 };
  const proc = { manifest: "d".repeat(64), role: "observed", ordinal: 2, waitpid: 0, audit_eof: true, control_eof: true, audit_valid: true, cleanup: true, escalated: false, controller_lost: false, io_errors: [] };
  const audit = { kind: KIND, version: 1, manifest: proc.manifest, role: "observed", ordinal: 2, waitArmed: true, cancelled: true, consumed: true, settled: true, off: true, shutdown: true, writersClosed: true, fault: false, trace: trace.snapshot(), probes: probe.snapshot(), publication: { uncertain: false, truncated: false, receipts: [] } };
  const run = (p = proc, a = audit, t = traceRows, q = probeRows) => classifyNative({ endpoint: e, process: p, audit: a, trace: t, probes: q });
  assert.equal(run().classification, "native_scoped_diagnostic");
  assert.equal(run().coverage.trace[0].relation, "post_intent");
  for (const [field, values] of [["manifest", [undefined, null, "", "wrong"]], ["role", [undefined, null, "controlled", "other"]], ["ordinal", [undefined, null, 0, 1, 3]]]) {
    for (const value of values) assert.equal(run({ ...proc, [field]: value }, { ...audit, [field]: value }).classification, "native_inconclusive", `${field}:${value}`);
  }
  const noRefresh = probeRows.slice(2).map((r, i) => ({ ...r, seq: i + 1, call: 1 }));
  const health = rows => ({ invalid: false, truncated: false, events: rows.length, bytes: Buffer.byteLength(rows.map(r => JSON.stringify(r) + "\n").join("")) });
  const uncovered = run(proc, { ...audit, probes: health(noRefresh) }, traceRows, noRefresh);
  assert.equal(uncovered.probe_status, "complete");
  assert.equal(uncovered.diagnostics.classification, "diagnostic_inconclusive");
  assert.equal(uncovered.classification, "native_inconclusive");
  const invalid = traceRows.map(r => r.event === "rejected" ? { ...r, op: "ack" } : r);
  const mismatch = run(proc, { ...audit, trace: health(invalid) }, invalid);
  assert.equal(mismatch.trace_status, "invalid");
  assert.equal(mismatch.coverage.trace[0].relation, "unknown");
  assert.equal(mismatch.diagnostics.pre_teardown_events, "observed");
});

test("publication warnings on poll and other bridge operations remain positive but veto qualification", async () => {
  for (const op of ["poll", "ack", "runtime_ack", "other"]) {
    const e = endpoint(), receipts = [], original = Promise.resolve({ publication_warnings: ["snapshot committed; veto removal uncertain"], policy: null });
    const bridge = receiptBridge({ request: () => original }, { binding: e.expected.binding, generation: e.expected.generation,
      save: r => receipts.push(r), fault: () => assert.fail("observer fault not expected") });
    assert.equal(bridge.request({ op }), original);
    await original;
    assert.equal(receipts.length, 1);
    assert.equal(receipts[0].op, op);
    assert.equal(receipts[0].data_commit, true);
    assert.equal(receipts[0].publication_clean, false);
    e.native.receipts.push(receipts[0]);
    assert.ok(qualifyEndpoint(e).missing.includes("publication warnings"));
  }
});

test("frozen baseline and observed endpoints cannot hide later publication uncertainty", async () => {
  for (const role of ["baseline", "observed"]) for (const op of ["poll", "runtime_ack"]) {
    const e = endpoint();
    e.native.receipts[0].trace = { events: 0, bytes: 0 }; e.native.receipts[0].probes = { events: 0, bytes: 0 };
    const live = structuredClone(e.native.receipts), publication = createPublicationAudit(), markers = new Map();
    const state = { calls: 1 };
    let pending;
    const frozen = new Promise(resolve => { pending = resolve; });
    const config = { model: { id: "fixed", provider: "local", api: "local" }, question: {}, ordinary_usage: {} };
    const signal = new AbortController();
    const provider = fixedProvider(FakeStream, config, state, { native: () => ({ ...e.native, receipts: live }), fault: () => assert.fail("provider fault"),
      save(name, value) { markers.set(name, JSON.stringify(value)); if (name === "pending.json") pending(); } });
    const stream = provider(config.model, {}, { signal: signal.signal });
    await frozen;
    const bytes = markers.get("endpoint-native.json");
    e.native = JSON.parse(bytes);
    const traceRows = [], probeRows = [];
    const trace = createTrace(line => traceRows.push(JSON.parse(line)));
    const probe = createDiagnosticProbe(line => probeRows.push(JSON.parse(line)));
    trace.emit("attach"); trace.emit("attached");
    trace.emit("request", { bridge: 1, request: 1, op: "poll" }); trace.emit("resolved", { bridge: 1, request: 1, op: "poll" });
    probe.emit("refresh", "start", { call: 1 }); probe.emit("refresh", "resolved", { call: 1 });
    trace.emit("teardown", { teardown: true });
    probe.emit("cleanup_stop", "entry", { call: 2, teardown: true }); probe.emit("cleanup_stop", "returned", { call: 2, teardown: true });
    trace.emit("close", { bridge: 1, teardown: true }); trace.emit("closed", { bridge: 1, teardown: true });
    const proc = { manifest: "d".repeat(64), role, ordinal: role === "baseline" ? 1 : 2, waitpid: 0, audit_eof: true, control_eof: true, audit_valid: true, cleanup: true, escalated: false, controller_lost: false, io_errors: [] };
    const audit = { kind: KIND, version: 1, manifest: proc.manifest, role, ordinal: proc.ordinal, waitArmed: true, cancelled: true, consumed: true, settled: true, off: true, shutdown: true, writersClosed: true, fault: false,
      trace: role === "observed" ? trace.snapshot() : null, probes: role === "observed" ? probe.snapshot() : null,
      publication: publication.snapshot() };
    const report = () => classifyNative({ endpoint: e, audit, process: proc, trace: role === "observed" ? traceRows : undefined, probes: role === "observed" ? probeRows : undefined });
    assert.equal(report().classification, role === "baseline" ? "native_endpoint_only" : "native_scoped_diagnostic");
    const original = Promise.resolve({ publication_warnings: ["snapshot committed; veto removal uncertain"] });
    const bridge = receiptBridge({ request: () => original }, { binding: e.expected.binding, generation: e.expected.generation,
      save(receipt) { publication.observe(receipt); live.push(receipt); }, fault: () => assert.fail("observer fault") });
    assert.equal(bridge.request({ op }), original);
    await original;
    audit.publication = publication.snapshot();
    const result = report();
    assert.equal(result.classification, "native_inconclusive", `${role}/${op}`);
    assert.equal(result.endpoint_status, "qualified");
    assert.equal(markers.get("endpoint-native.json"), bytes);
    assert.equal(e.native.receipts.length, 4);
    assert.equal(live.length, 5);
    assert.equal(result.publication_status, "uncertain");
    assert.equal(result.publication.receipts[0].data_commit, true);
    assert.deepEqual(result.publication.receipts[0].warnings, ["snapshot committed; veto removal uncertain"]);
    assert.equal(audit.fault, false);
    signal.abort(); await stream.result();
  }
});

test("final publication custody is sticky and bounded even when the main receipt observer fails", async () => {
  const e = endpoint(), publication = createPublicationAudit();
  let faults = 0;
  const original = Promise.resolve({ publication_warnings: ["snapshot committed; veto removal uncertain"] });
  const bridge = receiptBridge({ request: () => original }, { binding: e.expected.binding, generation: e.expected.generation,
    save(receipt) { publication.observe(receipt); throw Error("fixed main receipt cap"); }, fault() { faults++; } });
  for (let i = 0; i < 40; i++) { assert.equal(bridge.request({ op: "poll" }), original); await original; }
  const snapshot = publication.snapshot();
  assert.equal(faults, 40);
  assert.equal(snapshot.uncertain, true);
  assert.equal(snapshot.truncated, true);
  assert.equal(snapshot.receipts.length, 16);
  assert.ok(snapshot.receipts.every(r => r.data_commit === true));
  assert.ok(Buffer.byteLength(JSON.stringify(snapshot.receipts)) < 16 * 1024);
  publication.observe({ publication_clean: true, warnings: [] });
  assert.deepEqual(publication.snapshot(), snapshot);
  const long = createPublicationAudit();
  long.observe({ binding: e.expected.binding, generation: e.expected.generation, request: 1, op: "runtime_ack", data_commit: true,
    publication_clean: false, warnings: ["committed:" + "x".repeat(500)] });
  assert.equal(long.snapshot().truncated, true);
  assert.equal(long.snapshot().receipts[0].warnings[0].length, 256);
});
