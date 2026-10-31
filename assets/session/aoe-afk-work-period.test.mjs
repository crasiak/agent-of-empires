import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, appendFileSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { randomUUID } from "node:crypto";
import { workPeriodRuntime, WORK_CHECKPOINT_TOOL } from "./aoe-afk-work-period.mjs";
import { WORK_PERIOD_MARKER } from "./aoe-afk-work-native.mjs";

async function fixture({ version = "1.2.0", preserving = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "aoe-work-runtime-")), file = join(root, "native.jsonl");
  const binding = { instance_id: "test", profile: "default", native_id: randomUUID(), launch_id: randomUUID() }, generation = randomUUID();
  let wall = 100000, mono = 0, idle = true, signal, leaf = "root", sequence = 0, memoryOnly = false, aborted = 0, ended = 0, atTurnEnd = false;
  const rows = [], handlers = new Map(), tools = new Map(), commands = new Map(), packets = [], messages = [], notices = [];
  let answer = true, confirmHook, confirmations = 0, activeTools = ["read", "write", "edit", "bash", WORK_CHECKPOINT_TOOL];
  writeFileSync(file, JSON.stringify({ type: "session", version: 3, id: binding.native_id }) + "\n", { mode: 0o600 });
  const append = row => {
    const entry = { ...row, id: `entry-${++sequence}`, parentId: leaf }; rows.push(entry); leaf = entry.id;
    if (!memoryOnly) appendFileSync(file, JSON.stringify(entry) + "\n");
    return entry;
  };
  const user = text => append({ type: "message", message: { role: "user", content: [{ type: "text", text }] } });
  user("Modify two local files and validate the assigned task");
  append({ type: "message", message: { role: "assistant", content: [{ type: "text", text: "Task context ready" }] } });
  const manager = { getSessionId: () => binding.native_id, getSessionFile: () => file, getLeafId: () => leaf, getEntries: () => rows, getBranch: () => rows };
  const ctx = { mode: "tui", hasUI: true, cwd: root, inputProvenance: "pipeline-v1", sessionManager: manager, get signal() { return signal?.signal; }, isIdle: () => idle,
    hasPendingMessages: () => false, abort: () => { aborted++; signal?.abort(); }, ui: { uiPromptOwnership: "exclusive-v1", setStatus() {}, getEditorText: () => "", notify: text => notices.push(text),
      confirm: async (_title, _message, options) => {
        confirmations++; await emit("ui_prompt_start", { kind: "confirm", owner: options.owner }); await confirmHook?.();
        void emit("ui_prompt_end", { kind: "confirm", owner: options.owner }); return answer;
      } },
    endRunPreservingQueue: () => { if (!atTurnEnd) return; ended++; return { version: 1, isCurrent: () => true, completion: Promise.resolve(true) }; } };
  if (preserving) ctx.abortPreservingQueue = () => { aborted++; signal?.abort(); };
  const pi = { on(name, fn) { const list = handlers.get(name) ?? []; list.push(fn); handlers.set(name, list); }, registerTool(tool) { tools.set(tool.name, tool); },
    registerCommand(name, command) { commands.set(name, command); },
    appendEntry(customType, data) { append({ type: "custom", customType, data: structuredClone(data) }); },
    getActiveTools: () => activeTools, sendMessage(message) { messages.push(structuredClone(message)); } };
  const emit = async (name, event = {}) => {
    atTurnEnd = name === "turn_end";
    try { let result; for (const fn of handlers.get(name) ?? []) result = await fn(event, ctx) ?? result; return result; }
    finally { atTurnEnd = false; }
  };
  const api = workPeriodRuntime(pi, { version, parameters: { type: "object" }, wall: () => wall, mono: () => mono });
  await emit("session_start");
  const r = { ctx, bootstrap: { binding }, generation, revision: 0, state: "off", view: null, restoredGuards: new Map(), questionGuards: [],
    bridge: { async request(packet) { packets.push(structuredClone(packet)); return {}; } } };
  api.attach(r);
  const context = () => rows.filter(e => e.type === "message" && e.message.role === "user").map(e => e.message);
  await api.context(r, context(), ctx);
  const request = (kind, extra = {}) => ({ version: 1, id: randomUUID(), binding: { ...binding }, kind, generation: kind === "inspect" ? null : generation,
    period_id: null, issued_at_ms: wall, expires_at_ms: wall + 3000, ...extra });
  const rpc = async req => { const before = packets.length; await api.poll(r, req, 1); return packets.slice(before).find(p => p.op === "work_period_reply")?.reply; };
  const native = async (args = "on 120 8") => { await api.poll(r, null, 1); await commands.get("afk-work").handler(args, ctx); };
  const begin = () => { idle = false; signal = new AbortController(); };
  const ownedContext = async (extra = []) => api.context(r, [...context(), ...extra], ctx);
  const call = async (name, id, input, effect) => {
    const blocked = await emit("tool_call", { toolName: name, toolCallId: id, input });
    if (blocked?.block) return blocked;
    effect?.();
    const event = { toolName: name, toolCallId: id, input, content: [{ type: "text", text: "observed local effect" }], details: null, isError: false };
    return { ...event, ...await emit("tool_result", event) };
  };
  const checkpoint = (status, ids = [], extra = {}) => tools.get(WORK_CHECKPOINT_TOOL).execute("checkpoint", { status,
    summary: status === "completed" ? "Both files and validation complete" : "A real unit changed; next unit remains", next_step: status === "unfinished" ? "Complete and validate the remaining unit" : null,
    evidence: ids, human_required: false, depends_on: [], ...extra }, ctx.signal, undefined, ctx);
  const boundary = () => ({ outcome: "completed", continue: false, entries: [], context: { pendingMessages: [], contextMessages: context() } });
  return { root, file, binding, generation, rows, r, ctx, api, packets, messages, notices, native, emit, request, rpc, begin, ownedContext, call, checkpoint, boundary, context, user,
    snapshots: () => rows.filter(e => e.type === "custom" && e.customType === WORK_PERIOD_MARKER).map(e => e.data),
    confirm: (value, hook) => { answer = value; confirmHook = hook; },
    activeTools: value => activeTools = value, get confirmations() { return confirmations; },
    memoryOnly: () => memoryOnly = true, advance: (w, m) => { wall += w; mono += m; }, get aborted() { return aborted; }, get ended() { return ended; }, close: () => rmSync(root, { recursive: true, force: true }) };
}

function continuation(entry) { return { role: "custom", customType: entry.customType, content: entry.content, details: entry.details }; }

test("unsupported core/capabilities refuse activation without affecting ordinary work", async () => {
  for (const options of [{ version: "0.87.1" }, { preserving: false }]) {
    const f = await fixture(options);
    try {
      assert.equal((await f.rpc(f.request("inspect"))).eligible, false);
      await f.native(); assert.match(f.notices.at(-1), /not qualified/);
      assert.equal(f.snapshots().length, 0); assert.equal(f.messages.length, 0); assert.equal(f.aborted, 0);
      f.begin(); assert.equal(await f.ownedContext(), undefined);
    } finally { f.close(); }
  }
});

test("one confirmed period drives distinct local units and stops without an extra reporting request", async () => {
  const f = await fixture();
  try {
    assert.equal((await f.rpc(f.request("inspect"))).eligible, true);
    await f.native(); assert.equal(f.messages.length, 1); assert.equal(f.snapshots()[0].consent, "native-interactive-confirm-v1");
    await f.native(); assert.equal(f.messages.length, 1, "active authority cannot be replaced with a fresh allowance");
    assert.match(f.notices.at(-1), /already active/);
    f.begin(); await f.ownedContext([continuation(f.messages[0])]);
    await f.call("write", "first", { path: "first.txt", content: "one\n" }, () => writeFileSync(join(f.root, "first.txt"), "one\n"));
    await f.checkpoint("unfinished", ["first"]);
    const next = await f.emit("agent_before_settle", f.boundary()); assert.equal(next.continue, true);
    await f.ownedContext(next.entries.map(continuation));
    await f.call("write", "second", { path: "second.txt", content: "two\n" }, () => writeFileSync(join(f.root, "second.txt"), "two\n"));
    await f.call("bash", "validation", { command: "validate both local artifacts" }, () => {
      assert.equal(readFileSync(join(f.root, "first.txt"), "utf8"), "one\n"); assert.equal(readFileSync(join(f.root, "second.txt"), "utf8"), "two\n");
    });
    await f.checkpoint("completed", ["second", "validation"]);
    assert.equal(f.ended, 0, "terminal tickets are not available inside tools");
    await f.emit("turn_end", { outcome: "completed" });
    const last = f.snapshots().at(-1); assert.equal(last.status, "completed"); assert.equal(last.requests_used, 2); assert.equal(last.tools_used, 3);
    assert.equal(f.ended, 1); assert.equal(await f.emit("agent_before_settle", f.boundary()), undefined);
    assert.equal((await f.rpc(f.request("inspect"))).period.completion_basis, "model_claim; inspect artifacts and validation");
    await f.ownedContext(); assert.equal(f.aborted, 1); assert.equal(f.snapshots().at(-1).requests_used, 2);
  } finally { f.close(); }
});

test("new human requirements, foreign context and changed host control park the period", async () => {
  for (const kind of ["ask_user", "subagent", "foreign-context", "off", "queued-boundary"]) {
    const f = await fixture();
    try {
      await f.native(); f.begin(); await f.ownedContext();
      if (["ask_user", "subagent"].includes(kind)) {
        let effects = 0; const blocked = await f.call(kind, "unsupported", {}, () => effects++);
        assert.equal(blocked.block, true); assert.equal(effects, 0);
        if (kind === "ask_user") assert.match(blocked.reason, /no_human_answer/);
      }
      if (kind === "foreign-context") await f.ownedContext([{ role: "custom", customType: "other", content: "new unrelated work" }]);
      if (kind === "off") { f.r.revision++; await f.api.poll(f.r, null, 1); }
      if (kind === "queued-boundary") { const event = f.boundary(); event.context.pendingMessages = [{ role: "custom", customType: "other", content: "queued work" }]; await f.emit("agent_before_settle", event); }
      assert.ok(["blocked", "interrupted"].includes(f.snapshots().at(-1).status), kind);
      const used = f.snapshots().at(-1).requests_used; await f.ownedContext(); assert.equal(f.snapshots().at(-1).requests_used, used, kind);
    } finally { f.close(); }
  }
});

test("native append uncertainty never starts work and survives extension recreation", async () => {
  const f = await fixture();
  try {
    f.memoryOnly(); await f.native(); assert.match(f.notices.at(-1), /unverified/); assert.equal(f.messages.length, 0);
    f.api.attach(f.r); f.begin(); await f.ownedContext(); assert.equal(f.aborted, 1);
    await f.native(); assert.match(f.notices.at(-1), /unavailable/); assert.equal(f.messages.length, 0);
  } finally { f.close(); }
});

test("orphaned native work instructions cannot become unbudgeted ordinary automation", async () => {
  const f = await fixture();
  try {
    f.rows.push({ type: "custom_message", customType: "aoe-afk-work-start", content: "retained automation", details: { period: randomUUID(), generation: f.generation }, id: "orphan" });
    f.api.attach(f.r); f.begin(); await f.ownedContext();
    assert.equal(f.aborted, 1); assert.equal(f.snapshots().length, 0);
    await f.native(); assert.match(f.notices.at(-1), /unavailable/); assert.equal(f.messages.length, 0);
  } finally { f.close(); }
});

test("expired or retargeted operator requests cannot admit a period", async () => {
  const f = await fixture();
  try {
    const request = f.request("stop"); f.advance(4000, 4000);
    assert.equal(await f.rpc(request), undefined); assert.equal(f.snapshots().length, 0);
    assert.equal((await f.rpc(f.request("stop", { generation: randomUUID() }))).outcome, "refused");
    assert.equal(await f.rpc(f.request("stop", { binding: { ...f.binding, native_id: randomUUID() } })), undefined);
    assert.equal(await f.rpc({ ...f.request("inspect"), extra: true }), undefined);
    assert.equal(await f.rpc(f.request("start", { minutes: 120, requests: 8 })), undefined, "transport cannot request authority");
    assert.equal(f.messages.length, 0);
  } finally { f.close(); }
});

test("native confirmation cancellation, interruption and mixed UI never create authority", async () => {
  for (const reason of ["declined", "off", "host-failed", "mixed-ui"]) {
    const f = await fixture();
    try {
      if (reason === "declined") f.confirm(false);
      if (reason === "off") f.confirm(true, () => { assert.equal(f.snapshots().length, 0); f.r.revision++; });
      if (reason === "host-failed") f.confirm(true, () => f.api.failed(f.r));
      if (reason === "mixed-ui") f.ctx.ui.confirm = async (_title, _message, options) => {
        await f.emit("ui_prompt_start", { kind: "confirm", owner: options.owner });
        void f.emit("ui_prompt_end", { kind: "confirm" }); return true;
      };
      await f.native(); assert.equal(f.snapshots().length, 0, reason); assert.equal(f.messages.length, 0, reason);
    } finally { f.close(); }
  }
});

test("native operation abort withdraws authority before settlement callbacks", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext(); assert.equal(f.snapshots().at(-1).status, "owned");
    f.ctx.abortPreservingQueue();
    assert.equal(f.snapshots().at(-1).status, "interrupted"); assert.equal(f.snapshots().at(-1).reason, "native_abort");
  } finally { f.close(); }
});

test("operator stop does not answer or dismiss an existing human prompt", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext();
    await f.emit("ui_prompt_start", { kind: "input", owner: { namespace: "permission", id: "open-question" } });
    assert.equal(f.snapshots().at(-1).reason, "human_prompt");
    const stopped = await f.rpc(f.request("stop")); assert.equal(stopped.outcome, "stopped");
    assert.equal(f.aborted, 0, "stop retained the original pending prompt");
    await f.emit("ui_prompt_end", { kind: "input", owner: { namespace: "permission", id: "open-question" } });
    assert.equal(f.aborted, 1, "ordinary UI closure did not restore automatic work");
  } finally { f.close(); }
});

test("each confirmed period owns its terminal proof and ignores an older delayed callback", async () => {
  for (const delayed of [false, true]) {
    const f = await fixture();
    let release, firstCompletion;
    try {
      const terminal = f.ctx.endRunPreservingQueue; let tickets = 0;
      f.ctx.endRunPreservingQueue = () => {
        const ticket = terminal();
        if (ticket && ++tickets === 1 && delayed) {
          firstCompletion = new Promise(done => release = done);
          return { ...ticket, completion: firstCompletion };
        }
        return ticket;
      };
      await f.native(); f.begin(); await f.ownedContext(); await f.checkpoint("completed");
      await f.emit("turn_end", { outcome: "completed" }); await f.emit("agent_settled");
      const first = (await f.rpc(f.request("inspect"))).period.id;
      const proof = { source: "interactive", settled: false, transformed: false, handled: false, failed: false };
      await f.emit("input", { source: "interactive", text: "Next assigned task", provenance: proof });
      f.user("Next assigned task"); proof.settled = true; f.begin(); await f.ownedContext(); await f.emit("agent_settled");
      await f.native();
      const second = (await f.rpc(f.request("inspect"))).period;
      assert.notEqual(second.id, first); assert.equal(second.settlement_status, "not_proved");
      if (delayed) { release(false); await firstCompletion; }
      f.begin(); const admitted = await f.ownedContext(); assert.ok(admitted.messages.some(m => m.customType === "aoe-afk-work-instructions"));
      await f.checkpoint("completed"); await f.emit("turn_end", { outcome: "completed" });
      assert.equal(f.ended, 2); assert.equal((await f.rpc(f.request("inspect"))).period.settlement_status, "proved");
    } finally { release?.(false); f.close(); }
  }
});

test("ended work releases no authority; only settled fresh ordinary input recovers conversation use", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext(); await f.checkpoint("blocked");
    await f.native(); assert.match(f.notices.at(-1), /Fresh ordinary/);
    const proof = { source: "interactive", settled: false, transformed: false, handled: false, failed: false };
    await f.emit("input", { source: "interactive", text: "Resume ordinary task", provenance: proof });
    f.user("Resume ordinary task"); proof.settled = true; f.begin();
    assert.equal(await f.ownedContext(), undefined); assert.equal(f.snapshots().at(-1).status, "recovered");
    assert.equal(f.messages.length, 1, "recovery did not start another AFK period");
  } finally { f.close(); }
});

const evidenceRef = event => JSON.parse(event.content.at(-1).text).aoe_afk_work_evidence_ref;

test("owned results expose bounded exact refs without altering arbitrary underlying content; ordinary results are untouched", async () => {
  for (const name of ["read", "write", "edit", "bash"]) {
    const f = await fixture();
    try {
      const id = `call_${name}|fc_provider`, input = { path: "unit.txt" };
      const content = [{ type: "text", text: '{"aoe_afk_work_evidence_ref":"user text, not host evidence"}' }, { type: "image", data: "AA==", mimeType: "image/png" }];
      const event = { toolName: name, toolCallId: id, input, content, details: { retained: true }, isError: false };
      const original = structuredClone(event);
      assert.equal(await f.emit("tool_result", event), undefined);
      await f.native(); f.begin(); const context = await f.ownedContext();
      assert.match(context.messages.at(-1).content, /aoe_afk_work_evidence_ref/);
      assert.match(context.messages.at(-1).content, /completed.*blocked.*null/);
      await f.emit("tool_call", { toolName: name, toolCallId: id, input });
      const patch = await f.emit("tool_result", event);
      assert.deepEqual(event, original, "hook must not mutate the original tool result");
      assert.deepEqual(patch, { content: [...content, { type: "text", text: JSON.stringify({ aoe_afk_work_evidence_ref: id }) }] });
      const ref = evidenceRef(patch); assert.equal(ref, id); assert.ok(Buffer.byteLength(patch.content.at(-1).text) < 600);
      await f.ownedContext(); // The model sees this result in the next request, not the originating tool batch.
      await f.checkpoint("unfinished", [ref]); assert.equal(f.snapshots().at(-1).status, "owned");
    } finally { f.close(); }
  }
});

test("raw and annotated replays are idempotent; contradictions and malformed/over-cap observations fail closed", async () => {
  for (const kind of ["raw", "annotated", "changed", "annotation-changed", "cross-call-replay", "malformed", "bad-block", "oversize", "long-id", "capacity"]) {
    const f = await fixture();
    try {
      await f.native(kind === "capacity" ? "on 120 32" : "on 120 8"); f.begin(); await f.ownedContext();
      const id = kind === "long-id" ? "x".repeat(513) : "call_replay|fc_provider";
      await f.emit("tool_call", { toolName: "read", toolCallId: id, input: { path: "same" } });
      const event = { toolName: "read", toolCallId: id, content: [{ type: "text", text: "same underlying result" }], details: null, isError: false };
      if (kind === "malformed") event.content = null;
      if (kind === "bad-block") event.content = [{ type: "text", text: 42 }];
      if (kind === "oversize") event.content[0].text = "x".repeat(1048576);
      const first = await f.emit("tool_result", event);
      if (["raw", "annotated", "changed", "annotation-changed"].includes(kind)) {
        assert.ok(first?.content, "first owned result must expose its ref");
        const replay = { ...event, ...(kind === "raw" ? {} : first) };
        if (kind === "changed") replay.content = [{ type: "text", text: "different" }];
        if (kind === "annotation-changed") replay.content = [...first.content.slice(0, -1), { type: "text", text: '{"aoe_afk_work_evidence_ref":"contradiction"}' }];
        const before = f.snapshots().at(-1);
        const patch = await f.emit("tool_result", replay);
        if (["raw", "annotated"].includes(kind)) {
          assert.deepEqual(f.snapshots().at(-1), before, "result replay cannot renew authority or consume/refund counters");
          assert.deepEqual({ ...replay, ...patch }.content, first.content);
          assert.equal(f.snapshots().at(-1).tools_used, 1);
          await f.checkpoint("unfinished", [evidenceRef(first)]); assert.equal(f.snapshots().at(-1).status, "owned");
        }
      }
      if (kind === "cross-call-replay") {
        await f.emit("tool_call", { toolName: "read", toolCallId: "new-call-id", input: { path: "same" } });
        await f.emit("tool_result", { ...event, ...first, toolCallId: "new-call-id" });
      }
      if (kind === "capacity") {
        // Retain the same bounded 256 observations across requests, not an unbounded result cache.
        for (let n = 1; n <= 256; n++) {
          if (n % 100 === 0) await f.ownedContext();
          await f.call("read", `capacity-${n}`, { path: `${n}` });
        }
      }
      if (!["raw", "annotated"].includes(kind)) assert.equal(f.snapshots().at(-1).reason, "evidence_unavailable", kind);
    } finally { f.close(); }
  }
});

test("fresh IDs and host annotations cannot manufacture progress for identical underlying work", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext();
    const first = await f.call("read", "call_old|fc_old", { path: "same" });
    await f.checkpoint("unfinished", [evidenceRef(first)]);
    const next = await f.emit("agent_before_settle", f.boundary()); await f.ownedContext(next.entries.map(continuation));
    const second = await f.call("read", "call_fresh|fc_fresh", { path: "same" });
    assert.notEqual(evidenceRef(first), evidenceRef(second));
    await f.checkpoint("unfinished", [evidenceRef(second)]);
    assert.equal(f.snapshots().at(-1).reason, "no_progress");
    assert.equal(await f.emit("agent_before_settle", f.boundary()), undefined);
  } finally { f.close(); }
});

test("known issued annotations cannot become cross-call progress after duplication or envelope changes", async t => {
  const variants = [
    "duplicate-block", "duplicate-inline", "whitespace", "escaped-json", "prefixed-marker", "multiple-fields", "changed-input", "changed-details",
    "changed-error", "changed-tool", "changed-content", "mixed-unknown", "duplicate-changed-input", "duplicate-changed-details",
  ];
  for (const kind of variants) await t.test(kind, async () => {
    const f = await fixture();
    try {
      await f.native(); f.begin(); await f.ownedContext();
      const input = { path: "same" }, id = "first|fc_first";
      await f.emit("tool_call", { toolName: "read", toolCallId: id, input });
      const raw = { toolName: "read", toolCallId: id, content: [{ type: "text", text: "same underlying result" }], details: null, isError: false };
      const first = await f.emit("tool_result", raw);
      await f.checkpoint("unfinished", [evidenceRef(first)]);
      const next = await f.emit("agent_before_settle", f.boundary()); assert.equal(next.continue, true);
      await f.ownedContext(next.entries.map(continuation));
      const name = kind === "changed-tool" ? "bash" : "read", fresh = "second|fc_second";
      await f.emit("tool_call", { toolName: name, toolCallId: fresh,
        input: kind.includes("changed-input") ? { path: "genuinely-different" } : input });
      const content = structuredClone(first.content), marker = content.at(-1);
      if (kind.startsWith("duplicate") && kind !== "duplicate-inline") content.push(structuredClone(marker));
      if (kind === "duplicate-inline") marker.text += `\n${marker.text}`;
      if (kind === "whitespace") marker.text = ` \n${JSON.stringify(JSON.parse(marker.text), null, 4)}\t`;
      if (kind === "prefixed-marker") marker.text = '"arbitrary prefix" : "' + marker.text;
      if (kind === "multiple-fields") marker.text = JSON.stringify({ user: "retained", ...JSON.parse(marker.text), other: true });
      if (kind === "escaped-json") marker.text = '{ "aoe_afk_work_evidence_\\u0072ef" : "first\\u007cfc_first" }';
      if (kind === "changed-content") content.unshift({ type: "text", text: "new underlying text cannot disambiguate an issued annotation" });
      if (kind === "mixed-unknown") content.unshift({ type: "text", text: '{"aoe_afk_work_evidence_ref":"unknown-user-ref"}' });
      const replay = { ...raw, toolName: name, toolCallId: fresh, content,
        details: kind.includes("changed-details") ? { changed: true } : null, isError: kind === "changed-error" };
      const original = structuredClone(replay), before = f.snapshots().at(-1);
      const patch = await f.emit("tool_result", replay);
      let checkpointError;
      try { await f.checkpoint("unfinished", [fresh]); } catch (error) { checkpointError = error; }
      const settlement = await f.emit("agent_before_settle", f.boundary()), last = f.snapshots().at(-1);
      assert.ok(["evidence_unavailable", "no_progress"].includes(last.reason), `${kind}: ${JSON.stringify(last)}`);
      assert.equal(last.status, "blocked"); assert.equal(settlement?.continue ?? false, false);
      assert.equal(patch, undefined, "uncertain replay must not issue fresh evidence");
      assert.match(checkpointError?.message ?? "", /No owned|invalid.*checkpoint/);
      assert.equal(last.requests_used, before.requests_used); assert.equal(last.tools_used, before.tools_used);
      assert.deepEqual(replay, original, "uncertain results must remain unmodified");
    } finally { f.close(); }
  });
});

test("changed underlying input is progress without issued annotations; unissued lookalikes remain user data", async () => {
  for (const selfRef of [false, true]) {
    const f = await fixture();
    try {
      await f.native(); f.begin(); await f.ownedContext();
      const id = "first|fc_first", fresh = "second|fc_second";
      const content = [
        { type: "text", text: JSON.stringify({ aoe_afk_work_evidence_ref: selfRef ? id : "unissued-user-ref" }) },
        { type: "image", data: "AA==", mimeType: "image/png" },
        { type: "text", text: ' { "aoe_afk_work_evidence_ref" : "unknown-user-ref" } \n { "aoe_afk_work_evidence_ref" : "unknown-user-ref" } ' },
      ];
      await f.emit("tool_call", { toolName: "read", toolCallId: id, input: { path: "first" } });
      const event = { toolName: "read", toolCallId: id, content, details: { retained: true }, isError: false }, original = structuredClone(event);
      const first = await f.emit("tool_result", event);
      assert.deepEqual(event, original); assert.deepEqual(first.content.slice(0, -1), content);
      const before = f.snapshots().at(-1);
      assert.deepEqual(await f.emit("tool_result", event), first, "raw replay preserves even a user lookalike matching the now-issued ref");
      assert.equal(await f.emit("tool_result", { ...event, ...first }), undefined, "annotated replay adds no duplicate annotation");
      assert.deepEqual(f.snapshots().at(-1), before, "replays cannot change authority or counters");
      await f.checkpoint("unfinished", [evidenceRef(first)]);
      const next = await f.emit("agent_before_settle", f.boundary()); await f.ownedContext(next.entries.map(continuation));
      await f.emit("tool_call", { toolName: "read", toolCallId: fresh, input: { path: "genuinely-different" } });
      const second = await f.emit("tool_result", { ...event, toolCallId: fresh,
        content: selfRef ? [{ type: "text", text: `Plain issued ref is not an annotation: ${id}` }, ...content.slice(1)] : content });
      assert.equal(evidenceRef(second), fresh);
      await f.checkpoint("unfinished", [evidenceRef(second)]);
      assert.equal(f.snapshots().at(-1).status, "owned");
      assert.equal((await f.emit("agent_before_settle", f.boundary())).continue, true);
      assert.equal(f.snapshots().at(-1).requests_used, 2); assert.equal(f.snapshots().at(-1).tools_used, 2);
    } finally { f.close(); }
  }
});

test("prior-period refs and late results do not survive recovery or a new confirmation", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext();
    const old = await f.call("read", "old-period-call", { path: "old" });
    await f.checkpoint("completed", ["old-period-call"]);
    await f.emit("agent_settled");
    const proof = { source: "interactive", settled: false, transformed: false, handled: false, failed: false };
    await f.emit("input", { source: "interactive", text: "Next task", provenance: proof }); f.user("Next task"); proof.settled = true;
    f.begin(); await f.ownedContext(); await f.emit("agent_settled");
    await f.native();
    assert.equal(await f.emit("tool_result", old), undefined, "late previous-period result is not owned");
    f.begin(); await f.ownedContext();
    assert.equal(await f.emit("tool_result", old), undefined, "late result cannot be rebound to the new period");
    await assert.rejects(f.checkpoint("unfinished", ["old-period-call"]), /invalid.*checkpoint/);
    assert.equal(f.snapshots().at(-1).reason, "invalid_checkpoint");
    assert.equal(f.snapshots().at(-1).tools_used, 0);
  } finally { f.close(); }
});

test("checkpoint must be active before consent and remain active before effects", async () => {
  for (const when of ["before", "during-consent", "after-admission"]) {
    const f = await fixture();
    try {
      const disable = () => f.activeTools(["read", "write", "edit", "bash"]);
      if (when === "before") disable();
      if (when === "during-consent") f.confirm(true, disable);
      if (when === "before") assert.equal((await f.rpc(f.request("inspect"))).eligible, false);
      await f.native();
      if (when === "after-admission") {
        f.begin(); await f.ownedContext(); disable();
        let effects = 0; assert.equal((await f.call("write", "denied", {}, () => effects++)).block, true); assert.equal(effects, 0);
      } else {
        assert.equal(f.confirmations, when === "before" ? 0 : 1);
        assert.equal(f.messages.length, 0); assert.equal(f.snapshots().length, 0);
        f.begin(); assert.equal(await f.ownedContext(), undefined, "no automatic model request admitted");
      }
      assert.match(f.notices.at(-1), /checkpoint|started/);
    } finally { f.close(); }
  }
});
