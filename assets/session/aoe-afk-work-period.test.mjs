import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, appendFileSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { randomUUID } from "node:crypto";
import { workPeriodRuntime, WORK_CHECKPOINT_TOOL } from "./aoe-afk-work-period.mjs";
import { WORK_PERIOD_MARKER } from "./aoe-afk-work-native.mjs";
import { ANSWER_TOOL } from "./aoe-afk-answer.mjs";

async function fixture({ version = "1.2.0", preserving = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "aoe-work-runtime-")), file = join(root, "native.jsonl");
  const binding = { instance_id: "test", profile: "default", native_id: randomUUID(), launch_id: randomUUID() }, generation = randomUUID();
  let wall = 100000, mono = 0, idle = true, signal, leaf = "root", sequence = 0, memoryOnly = false, aborted = 0, ended = 0, atTurnEnd = false;
  const rows = [], handlers = new Map(), tools = new Map(), commands = new Map(), packets = [], messages = [], notices = [];
  let answer = true, confirmHook, confirmMessage, confirmations = 0, decideReply = { decision: { decision: "stop", reason: "no_rule", checks: [] } }, recogniseReply = null,
    rulesCheck = { valid: true, count: 1, sources: ["inline rule 1"], format_warnings: [] }, activeTools = ["read", "write", "edit", "bash", WORK_CHECKPOINT_TOOL];
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
      confirm: async (_title, message, options) => {
        confirmations++; confirmMessage?.(message); await emit("ui_prompt_start", { kind: "confirm", owner: options.owner }); await confirmHook?.();
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
    bridge: { async request(packet) {
      packets.push(structuredClone(packet));
      return packet.op === "decide" ? (typeof decideReply === "function" ? decideReply() : structuredClone(decideReply)) : packet.op === "rules_check" ? structuredClone(rulesCheck)
        : packet.op === "recognise" ? { menu: structuredClone(recogniseReply) } : {};
    } } };
  api.attach(r);
  const context = () => rows.filter(e => e.type === "message" && e.message.role === "user").map(e => e.message);
  await api.context(r, context(), ctx);
  const request = (kind, extra = {}) => ({ version: 1, id: randomUUID(), binding: { ...binding }, kind, generation: kind === "inspect" ? null : generation,
    period_id: null, issued_at_ms: wall, expires_at_ms: wall + 3000, ...extra });
  const rpc = async req => { const before = packets.length; await api.poll(r, req, 1); return packets.slice(before).find(p => p.op === "work_period_reply")?.reply; };
  const native = async (args = "on 120 8") => { await api.poll(r, null, 1); await commands.get("afk-work").handler(args, ctx); };
  const begin = () => { idle = false; signal = new AbortController(); };
  const ownedContext = async (extra = []) => api.context(r, [...context(), ...extra], ctx);
  const callWithOutput = async (name, id, input, text, effect) => {
    const blocked = await emit("tool_call", { toolName: name, toolCallId: id, input });
    if (blocked?.block) return blocked;
    effect?.();
    const event = { toolName: name, toolCallId: id, input, content: [{ type: "text", text }], details: null, isError: false };
    return { ...event, ...await emit("tool_result", event) };
  };
  const call = (name, id, input, effect) => callWithOutput(name, id, input, "observed local effect", effect);
  const checkpoint = (status, ids = [], extra = {}) => tools.get(WORK_CHECKPOINT_TOOL).execute("checkpoint", { status,
    summary: status === "completed" ? "Both files and validation complete" : "A real unit changed; next unit remains", next_step: status === "unfinished" ? "Complete and validate the remaining unit" : null,
    evidence: ids, human_required: false, depends_on: [], ...extra }, ctx.signal, undefined, ctx);
  const answerTool = input => tools.get(ANSWER_TOOL).execute("id", input, ctx.signal, undefined, ctx);
  const boundary = () => ({ outcome: "completed", continue: false, entries: [], context: { pendingMessages: [], contextMessages: context() } });
  return { root, file, binding, generation, rows, r, ctx, api, packets, messages, notices, native, emit, request, rpc, begin, ownedContext, call, callWithOutput, checkpoint, answer: answerTool, boundary, context, user,
    snapshots: () => rows.filter(e => e.type === "custom" && e.customType === WORK_PERIOD_MARKER).map(e => e.data),
    confirm: (value, hook, onMessage) => { answer = value; confirmHook = hook; confirmMessage = onMessage; },
    decideReply: value => decideReply = value, recogniseReply: value => recogniseReply = value, rulesCheckError: error => rulesCheck = { valid: false, error, format_warnings: [] },
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

// Pi bash result shapes (pi coding-agent core/tools/bash.ts formatOutput/appendStatus).
const exited = (text, code = 1) => `${text}\n\nCommand exited with code ${code}`;
const timedOut = (text, secs = 600) => `${text}\n\nCommand timed out after ${secs} seconds`;

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
      // tool_call may set bash defaults in place; the result event carries its own copy.
      await f.emit("tool_call", { toolName: name, toolCallId: id, input: structuredClone(input) });
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

test("aoe_afk_answer returns the engine decision labelled afk/<policy> and logs delivery", async () => {
  const f = await fixture();
  try {
    f.decideReply({ answer_id: "a1", menu: {}, decision: { decision: "answer", choice: "2", rule: "r", reason: "ok", warnings: [], checks: [] } });
    await f.native("on --policy rules 120 8"); f.begin(); await f.ownedContext();
    const out = await f.answer({ question: "Which?", options: [{ key: "1", label: "A" }, { key: "2", label: "B" }], proposal: "2", reason: "B fits" });
    assert.deepEqual(out.details, { answered_by: "afk/rules", choice: "2", rule: "r", reason: "ok" });
    const decide = f.packets.find(p => p.op === "decide");
    assert.deepEqual(decide.proposal, { key: "2", reason: "B fits" });
    assert.equal(decide.run.policy, "rules"); assert.equal(decide.run.cwd, f.root); assert.equal(decide.menu.format, "tool.aoe_afk_answer");
    assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "a1" && p.delivered === "ok"));
    assert.equal(f.snapshots().at(-1).status, "owned");
  } finally { f.close(); }
});

test("a stopped aoe_afk_answer ends the period as menu_stopped", async () => {
  const f = await fixture();
  try {
    f.decideReply({ answer_id: "a2", menu: {}, decision: { decision: "stop", reason: "no_rule", checks: [] } });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const out = await f.answer({ question: "Q?", options: [{ key: "1", label: "Yes" }], proposal: null, reason: "" });
    assert.deepEqual(out.details, { stopped: "no_rule" });
    assert.equal(f.snapshots().at(-1).reason, "menu_stopped");
    assert.equal(f.packets.find(p => p.op === "decide").proposal.key, undefined);
    assert.ok(!f.packets.some(p => p.op === "answer_delivered"));
  } finally { f.close(); }
});

test("consent shows the yolo warning and status counts answers", async () => {
  const f = await fixture();
  try {
    let message;
    f.confirm(true, undefined, m => message = m);
    await f.native("on --policy yolo 120 8");
    assert.match(message, /YOLO: AFK will answer every menu, including permission prompts\. Scope checks only warn\./);
    assert.match(message, /policy: yolo/);
    assert.ok(!f.packets.some(p => p.op === "rules_check"), "yolo needs no rule check");
    assert.equal(f.snapshots().at(-1).policy, "yolo");
    f.decideReply({ answer_id: "a3", menu: {}, decision: { decision: "answer", choice: "1", rule: null, reason: "yolo", warnings: [], checks: [] } });
    f.begin(); await f.ownedContext();
    assert.equal((await f.answer({ question: "Q?", options: [{ key: "1", label: "Yes" }], proposal: "1", reason: "go" })).details.answered_by, "afk/yolo");
    await f.native("status");
    assert.match(f.notices.at(-1), /answers: 1 \(rules 0, yolo 1\), stops: 0/);
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 0, yolo: 1, stops: 0 });
  } finally { f.close(); }
});

test("consent shows the rule count and sources under rules", async () => {
  const f = await fixture();
  try {
    let message;
    f.confirm(true, undefined, m => message = m);
    await f.native(`on --rule "id='a', answer='stop'" 120 8`);
    assert.match(message, /policy: rules; rules: 1 \(inline rule 1\)/);
    assert.doesNotMatch(message, /YOLO/);
    assert.deepEqual(f.packets.find(p => p.op === "rules_check").run.rules, ["id='a', answer='stop'"]);
    assert.deepEqual(f.snapshots().at(-1).rules, ["id='a', answer='stop'"]);
  } finally { f.close(); }
});

test("invalid rules refuse the start under rules", async () => {
  const f = await fixture();
  try {
    f.rulesCheckError("afk-rules.toml: invalid rules file");
    await f.native("on 120 8");
    assert.ok(f.notices.some(n => n.includes("afk-rules.toml")));
    assert.equal(f.snapshots().length, 0);
  } finally { f.close(); }
});

test("bad /afk-work on arguments and oversized rule grants refuse the start", async () => {
  for (const args of ["on --policy review 120 8", "on 1 2 3", `on ${Array.from({ length: 8 }, () => `--rule "${"x".repeat(1024)}"`).join(" ")} 120 8`]) {
    const f = await fixture();
    try {
      await f.native(args);
      assert.equal(f.snapshots().length, 0, args); assert.equal(f.messages.length, 0, args);
      assert.match(f.notices.at(-1), args.includes("--rule") ? /rules too large.*no authority created/ : /no authority created/, args);
    } finally { f.close(); }
  }
});

test("aoe_afk_answer is admitted without counting a tool and refused outside an owned period", async () => {
  const f = await fixture();
  try {
    assert.equal((await f.emit("tool_call", { toolName: ANSWER_TOOL, toolCallId: "early", input: {} }))?.block, true);
    await assert.rejects(f.answer({ question: "Q?", options: [{ key: "1", label: "Yes" }], proposal: null, reason: "" }), /No owned/);
    await f.native(); f.begin(); await f.ownedContext();
    assert.equal(await f.emit("tool_call", { toolName: ANSWER_TOOL, toolCallId: "owned", input: {} }), undefined);
    assert.equal(f.snapshots().at(-1).tools_used, 0);
  } finally { f.close(); }
});

test("an answer decided after the period ended is logged as failed delivery", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext();
    f.decideReply(() => { f.ctx.abortPreservingQueue(); return { answer_id: "a4", menu: {}, decision: { decision: "answer", choice: "1", rule: "r", reason: "ok", warnings: [], checks: [] } }; });
    await assert.rejects(f.answer({ question: "Q?", options: [{ key: "1", label: "Yes" }], proposal: "1", reason: "go" }), /ended before the answer/);
    assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "a4" && p.delivered === "failed"));
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 0, yolo: 0, stops: 0 });
  } finally { f.close(); }
});

const menuText = text => ({ role: "assistant", content: [{ type: "text", text }] });
const menuBoundary = (f, text) => ({ ...f.boundary(), context: { pendingMessages: [], contextMessages: [...f.context(), menuText(text)] } });

test("a turn that ends with a recognised text menu gets one corrective continuation", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "choice" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const b = menuBoundary(f, "[A]pply / [S]kip");
    const first = await f.emit("agent_before_settle", b);
    assert.equal(first.continue, true);
    assert.match(first.entries.at(-1).content, /Resolve it with aoe_afk_answer/);
    assert.deepEqual(f.packets.find(p => p.op === "recognise"), { op: "recognise", source: "agent_text", text: "[A]pply / [S]kip" });
    assert.equal(f.snapshots().at(-1).menu_nudges, 1);
    await f.ownedContext([menuText("[A]pply / [S]kip"), continuation(first.entries.at(-1))]);
    assert.equal(f.snapshots().at(-1).status, "owned");
    const second = await f.emit("agent_before_settle", b);
    assert.equal(second, undefined);
    assert.equal(f.snapshots().at(-1).reason, "missing_checkpoint");
  } finally { f.close(); }
});

test("no recognised menu at settle stops as missing_checkpoint", async () => {
  const f = await fixture();
  try {
    f.recogniseReply(null);
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    assert.equal(await f.emit("agent_before_settle", menuBoundary(f, "Done for now.")), undefined);
    assert.equal(f.snapshots().at(-1).reason, "missing_checkpoint");
    assert.equal(f.snapshots().at(-1).menu_nudges, 0);
  } finally { f.close(); }
});

test("an unfinished checkpoint continues without asking the recogniser", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "choice" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    await f.call("read", "unit", { path: "unit" }); await f.checkpoint("unfinished", ["unit"]);
    const next = await f.emit("agent_before_settle", menuBoundary(f, "[A]pply / [S]kip"));
    assert.equal(next.continue, true); assert.doesNotMatch(next.entries.at(-1).content, /Resolve it with/);
    assert.ok(!f.packets.some(p => p.op === "recognise"));
  } finally { f.close(); }
});

test("the settle recogniser gets the byte-bounded tail of long agent text", async () => {
  const f = await fixture();
  try {
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const text = "é".repeat(3000) + "\n[A]pply / [S]kip";
    await f.emit("agent_before_settle", menuBoundary(f, text));
    const sent = f.packets.find(p => p.op === "recognise").text;
    assert.ok(Buffer.byteLength(sent) <= 4096); assert.ok(Buffer.byteLength(sent) >= 4095);
    assert.ok(text.endsWith(sent)); assert.ok(sent.endsWith("[A]pply / [S]kip"));
  } finally { f.close(); }
});

test("ask_user is answered by the engine with an afk-labelled blocked result", async () => {
  const f = await fixture();
  try {
    f.decideReply({ answer_id: "q1", menu: {}, decision: { decision: "answer", choice: "2", rule: "r", reason: "ok", warnings: [], checks: [] } });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    let effects = 0;
    const out = await f.call("ask_user", "c1", { question: "Which DB?", options: [{ title: "SQLite" }, { title: "Postgres" }] }, () => effects++);
    assert.equal(out.block, true); assert.equal(effects, 0);
    assert.deepEqual(JSON.parse(out.reason), { answered_by: "afk/rules", choice: "2", label: "Postgres", rule: "r", reason: "ok" });
    assert.equal(f.snapshots().at(-1).status, "owned"); assert.equal(f.snapshots().at(-1).tools_used, 0);
    assert.equal(f.packets.find(p => p.op === "decide").menu.format, "tool.ask_user");
    assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "q1" && p.delivered === "ok"));
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 1, yolo: 0, stops: 0 });
  } finally { f.close(); }
});

test("ask_user stop keeps today's no_human_answer behaviour", async () => {
  const f = await fixture();
  try {
    f.decideReply({ answer_id: "q2", menu: {}, decision: { decision: "stop", reason: "no_rule", checks: [] } });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const out = await f.call("ask_user", "c2", { question: "Q?", options: [{ title: "A" }] });
    assert.match(out.reason, /no_human_answer/);
    assert.equal(f.snapshots().at(-1).reason, "human_required");
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 0, yolo: 0, stops: 1 });
  } finally { f.close(); }
});

test("an unmappable ask_user batch goes straight to no_human_answer", async () => {
  const f = await fixture();
  try {
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const out = await f.call("ask_user", "c3", { questions: [{ question: "a" }, { question: "b" }] });
    assert.match(out.reason, /no_human_answer/);
    assert.ok(!f.packets.some(p => p.op === "decide"));
    assert.equal(f.snapshots().at(-1).reason, "human_required");
  } finally { f.close(); }
});

test("an ask_user answer decided after the period ended is logged as failed delivery", async () => {
  const f = await fixture();
  try {
    await f.native(); f.begin(); await f.ownedContext();
    f.decideReply(() => { f.ctx.abortPreservingQueue(); return { answer_id: "q4", menu: {}, decision: { decision: "answer", choice: "1", rule: "r", reason: "ok", warnings: [], checks: [] } }; });
    const out = await f.call("ask_user", "c4", { question: "Q?", options: [{ title: "Yes" }] });
    assert.equal(out.block, true); assert.match(out.reason, /operation identity changed/);
    assert.ok(f.packets.some(p => p.op === "answer_delivered" && p.answer_id === "q4" && p.delivered === "failed"));
    assert.ok(!f.packets.some(p => p.op === "answer_delivered" && p.delivered === "ok"));
    assert.equal(f.snapshots().at(-1).reason, "native_abort");
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 0, yolo: 0, stops: 0 });
  } finally { f.close(); }
});

test("bash without timeout gets the 600 s default; explicit timeout is kept", async () => {
  const f = await fixture();
  try {
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const a = { command: "ls" }, b = { command: "cargo build", timeout: 1800 };
    await f.call("bash", "b1", a); await f.call("bash", "b2", b);
    assert.equal(a.timeout, 600);
    assert.equal(b.timeout, 1800);
  } finally { f.close(); }
});

test("a bash result ending in a prompt is annotated and not progress until answered", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm", question: "Ok to proceed? (y/N)" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const r = await f.callWithOutput("bash", "b3", { command: "npx create-vite" }, exited("Need to install the following packages:\ncreate-vite@5.2.0\nOk to proceed? (y/N)"));
    assert.ok(r.content.some(b => b.type === "text" && b.text.includes("aoe_afk_prompt_seen")));
    assert.ok(f.packets.some(p => p.op === "recognise" && p.source === "bash_prompt"));
    const out = await f.checkpoint("unfinished", ["b3"]);
    assert.equal(out.details.status, "blocked");
    assert.equal(out.details.reason, "no_progress");
    assert.deepEqual(out.details.ignored_evidence, ["b3"]);
    assert.equal(out.details.ignored_reason, "prompt_seen_not_answered");
  } finally { f.close(); }
});

test("a prompt-annotated bash result replays idempotently", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const event = { toolName: "bash", toolCallId: "b6", input: { command: "x" }, content: [{ type: "text", text: exited("Continue? (y/N)") }], details: null, isError: true };
    await f.emit("tool_call", { toolName: "bash", toolCallId: "b6", input: event.input });
    const first = await f.emit("tool_result", event);
    assert.equal(await f.emit("tool_result", { ...event, content: first.content }), undefined);
    assert.equal(f.snapshots().at(-1).status, "owned");
  } finally { f.close(); }
});

test("a prompt-like bash tail that no format recognises stops as unrecognised_menu and saves the sample", async () => {
  const f = await fixture();
  try {
    f.recogniseReply(null);
    f.decideReply({ answer_id: "u1", menu: null, decision: { decision: "stop", reason: "unrecognised_menu", checks: [] }, sample: "p-unrecognised-1.txt" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    await f.callWithOutput("bash", "b5", { command: "x" }, timedOut("Enter the magic word?"));
    assert.ok(f.packets.some(p => p.op === "decide" && p.text?.source === "bash_prompt"));
    assert.equal(f.snapshots().at(-1).reason, "unrecognised_menu");
    assert.equal(f.snapshots().at(-1).status, "blocked");
    assert.deepEqual((await f.rpc(f.request("inspect"))).period.answers, { rules: 0, yolo: 0, stops: 1 });
  } finally { f.close(); }
});

test("exit-0 bash output is never a prompt, even when its last line looks like one", async () => {
  const f = await fixture();
  try {
    await f.native("on 120 16"); f.begin(); await f.ownedContext();
    const outputs = ['assets/session/x.test.mjs:12:    "Ok to proceed? (y/N)"', "src/bridge.rs:40:        let reply = client.request(packet).await?",
      "## Why not anchor the regex?", "ℹ tests 274\nℹ pass 271\nℹ fail 1", exited("error: could not compile")];
    for (const [i, out] of outputs.entries()) {
      const r = await f.callWithOutput("bash", `b7-${i}`, { command: "x" }, out);
      assert.ok(!r.content.some(b => b.text?.includes("aoe_afk_prompt_seen")), out);
    }
    assert.ok(!f.packets.some(p => p.op === "recognise"));
    assert.equal(f.snapshots().at(-1).status, "owned");
  } finally { f.close(); }
});

test("a waiting ssh host-key prompt that hits the bash timeout is recognised", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    const r = await f.callWithOutput("bash", "b9", { command: "ssh host" },
      timedOut("The authenticity of host 'host (10.0.0.1)' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?"));
    assert.ok(r.content.some(b => b.text?.includes("aoe_afk_prompt_seen")));
    assert.ok(f.packets.find(p => p.op === "recognise").text.endsWith("(yes/no/[fingerprint])?"));
  } finally { f.close(); }
});

test("a truncated bash output whose last real line is a prompt is recognised", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    await f.callWithOutput("bash", "b10", { command: "x" }, timedOut("lots of output\nOk to proceed? (y/N)\n\n[Showing lines 1999-4000 of 4000 (50.0KB limit). Full output: /tmp/pi-bash-1.log]"));
    assert.ok(f.packets.find(p => p.op === "recognise").text.endsWith("Ok to proceed? (y/N)"));
  } finally { f.close(); }
});

test("the bash prompt recogniser gets the tail of long output", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm" });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    await f.callWithOutput("bash", "b8", { command: "x" }, exited("é".repeat(5000) + "\nContinue? (y/N)"));
    const sent = f.packets.find(p => p.op === "recognise");
    assert.ok(sent.text.endsWith("Continue? (y/N)")); assert.ok(!sent.text.includes("Command exited"));
  } finally { f.close(); }
});

test("an answered aoe_afk_answer releases the prompt evidence", async () => {
  const f = await fixture();
  try {
    f.recogniseReply({ kind: "confirm" });
    f.decideReply({ answer_id: "x", menu: {}, decision: { decision: "answer", choice: "y", reason: "", warnings: [], checks: [] } });
    await f.native("on 120 8"); f.begin(); await f.ownedContext();
    await f.callWithOutput("bash", "b4", { command: "x" }, timedOut("Continue? (y/N)"));
    await f.answer({ question: "Continue?", options: [{ key: "y", label: "Yes" }, { key: "n", label: "No" }], proposal: "y", reason: "" });
    const out = await f.checkpoint("unfinished", ["b4"]);
    assert.equal(out.details.status, "owned");
    assert.equal(out.details.ignored_evidence, undefined);
  } finally { f.close(); }
});
