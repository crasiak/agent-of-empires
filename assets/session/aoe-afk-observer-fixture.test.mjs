import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync, statSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { createTrace, privateTrace, observeBridge, observeDelegation, validEvent, pendingStream, questionAttribution, publishMarker, nativeDeferredResult, cancellationCommand } from "./aoe-afk-observer-fixture.mjs";
import { registerAfk, spawnBridge } from "./aoe-afk.mjs";

const source = process.env.AOE_PI_SOURCE_ROOT;
assert.ok(source, "real pinned Pi source is required, never skip SDK coverage");
const load = p => import(pathToFileURL(join(source, p)));
const latch = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
const capture = () => { const lines = []; return { lines, trace: createTrace(line => lines.push(JSON.parse(line))) }; };

test("copied observer entry initializes through the actual source loader outside node_modules", () => {
  const dir = mkdtempSync(join(tmpdir(), "observer-entry-"));
  try {
    for (const file of ["aoe-afk-observer-entry-fixture.mjs", "aoe-afk-observer-fixture.mjs", "aoe-afk.mjs", "aoe-afk-runtime.mjs", "aoe-afk-questions.mjs", "aoe-afk-usage.mjs", "aoe-afk-usage-guard.mjs", "aoe-afk-wake.mjs"]) {
      writeFileSync(join(dir, file), readFileSync(resolve("assets/session", file)));
    }
    const script = `import assert from 'node:assert/strict';
import {loadExtensions,createExtensionRuntime} from ${JSON.stringify(pathToFileURL(join(source, "packages/coding-agent/src/core/extensions/loader.ts")).href)};
import {createEventBus} from ${JSON.stringify(pathToFileURL(join(source, "packages/coding-agent/src/index.ts")).href)};
const loaded=await loadExtensions([${JSON.stringify(join(dir, "aoe-afk-observer-entry-fixture.mjs"))}],${JSON.stringify(dir)},createEventBus(),createExtensionRuntime());
assert.deepEqual(loaded.errors,[]);assert.equal(loaded.extensions.length,1);assert.ok(loaded.extensions[0].flags.has('aoe-afk-binding'));assert.ok(loaded.extensions[0].tools.has('aoe_afk_record'));`;
    const result = spawnSync(process.execPath, ["--require", join(source, "node_modules/tsx/dist/preflight.cjs"), "--import", join(source, "node_modules/tsx/dist/loader.mjs"), "--input-type=module", "-e", script], {
      cwd: dir, env: { ...process.env, AOE_FIXTURE_ROOT: dir, AOE_FIXTURE_TRACE: join(dir, "trace.jsonl") }, encoding: "utf8", timeout: 20000, maxBuffer: 16384,
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("forward arguments, this, promise/result/error identities and synchronous throws without raw metadata", async () => {
  for (const kind of ["success", "refusal", "error", "throw"]) {
    const { trace, lines } = capture(), state = { teardown: false }, packet = { op: "runtime", command: { kind: "stop", prompt: "PRIVATE" } };
    const value = { private: "PRIVATE" }, error = Object.assign(Error("PRIVATE"), { code: "usage_stale" });
    const pending = kind === "success" ? Promise.resolve(value) : kind === "throw" ? null : Promise.reject(error);
    let calls = 0;
    const original = { request(...args) { assert.equal(this, original); assert.equal(args[0], packet); assert.equal(args[1], value); calls++; if (kind === "throw") throw error; return pending; }, close: () => Promise.resolve() };
    const observed = observeBridge(original, trace, state);
    if (kind === "throw") assert.throws(() => observed.request(packet, value), e => e === error);
    else {
      const returned = observed.request(packet, value); assert.equal(returned, pending);
      if (kind === "success") assert.equal(await returned, value); else await assert.rejects(returned, e => e === error);
    }
    assert.equal(calls, 1); assert.equal(trace.snapshot().invalid, false);
    assert.deepEqual(lines.map(e => e.event), ["request", kind === "success" ? "resolved" : "rejected"]);
    assert.ok(!JSON.stringify(lines).includes("PRIVATE")); assert.ok(lines.every(validEvent));
  }
});

test("trace enforces privacy, sticky writer failure, both reserved caps and private file mode", () => {
  for (const data of [{ message: "PRIVATE" }, { op: "PRIVATE" }, { request: 65536 }, { duration: NaN }, { teardown: 1 }, { get op() { throw Error("PRIVATE"); } }]) {
    const { lines, trace } = capture(); trace.emit("request", data); trace.emit("close");
    assert.deepEqual(lines, []); assert.equal(trace.snapshot().invalid, true);
  }
  const broken = createTrace(() => { throw Error("PRIVATE"); }); assert.doesNotThrow(() => broken.emit("request"));
  assert.equal(broken.snapshot().invalid, true); broken.emit("close"); assert.equal(broken.snapshot().events, 1);
  for (const limits of [{}, { events: 5 }, { bytes: 500 }]) {
    const output = []; const t = createTrace(line => output.push(line), limits);
    for (let i = 0; i < 2000; i++) t.emit("request");
    assert.equal(t.snapshot().invalid, true); assert.equal(t.snapshot().truncated, true);
    assert.equal(JSON.parse(output.at(-1)).event, "truncated");
    assert.ok(output.length <= (limits.events ?? 1024)); assert.ok(Buffer.byteLength(output.join("")) <= (limits.bytes ?? 262144));
    assert.equal(output.length, t.snapshot().events);
  }
  const dir = mkdtempSync(join(tmpdir(), "observer-trace-"));
  try { const t = privateTrace(join(dir, "trace")); t.emit("request"); t.close(); assert.equal(statSync(join(dir, "trace")).mode & 0o777, 0o600); }
  finally { rmSync(dir, { recursive: true, force: true }); }
});

test("real transport refusal stays usable; fatal JSON and deadline remain indistinguishable rejected outcomes", { timeout: 8000 }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "observer-bridge-"));
  const host = join(dir, "host.mjs");
  writeFileSync(host, `#!${process.execPath}\nimport {createInterface} from 'node:readline';\nconst mode=JSON.parse(process.argv[3]).mode; let n=0;\ncreateInterface({input:process.stdin}).on('line',()=>{n++; if(mode==='deadline')return; process.stdout.write(mode==='json'?'broken\\n':JSON.stringify(n===1?{ok:false,code:'usage_stale'}:{ok:true,value:{count:n}})+'\\n');});`, { mode: 0o700 });
  try {
    for (const mode of ["refusal", "json", "deadline"]) {
      const { trace, lines } = capture(), state = { teardown: false };
      const bridge = observeBridge(spawnBridge({ aoe_bin: host, mode }, "test"), trace, state);
      try {
        await assert.rejects(bridge.request({ op: "poll" }), error => mode !== "refusal" || error.code === "usage_stale");
        if (mode === "refusal") assert.deepEqual(await bridge.request({ op: "poll" }), { count: 2 });
        else await assert.rejects(bridge.request({ op: "runtime", command: { kind: "stop" } }));
        assert.equal(lines[1].event, "rejected");
        assert.ok(!JSON.stringify(lines).match(/deadline|closure|written|caller|cycle|usage_stale/));
      } finally { await bridge.close(); }
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

async function realDelegation() {
  const sdk = await load("packages/coding-agent/src/index.ts"); assert.equal(sdk.VERSION, "0.87.1");
  const { loadExtensions, createExtensionRuntime } = await load("packages/coding-agent/src/core/extensions/loader.ts");
  const dir = mkdtempSync(join(tmpdir(), "observer-sdk-"));
  const entry = join(dir, "entry.mjs");
  writeFileSync(entry, `import {delegationRuntime} from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)}; export default pi=>{globalThis.__observerTestDelegate=delegationRuntime(pi);};`);
  const loaded = await loadExtensions([entry], dir, sdk.createEventBus(), createExtensionRuntime());
  assert.deepEqual(loaded.errors, []);
  const delegate = globalThis.__observerTestDelegate; delete globalThis.__observerTestDelegate;
  const manager = sdk.SessionManager.create(dir, join(dir, "sessions"));
  return { delegate, manager, dir, close: () => rmSync(dir, { recursive: true, force: true }) };
}

test("real delegation and registerAfk preserve joins, local failure, throwing failed callback and final status rejection", async () => {
  for (const mode of ["joined", "observation", "teardown-observation", "callback", "status"]) {
    const f = await realDelegation();
    const handlers = new Map(), { trace, lines } = capture(), gate = latch(), entered = latch();
    const state = { teardown: false }; let current, scheduled, holding = false, localFailure = false;
    const error = Error("PRIVATE callback"), statusError = Error("PRIVATE final status");
    const originalAttach = f.delegate.attach, originalFailed = f.delegate.failed;
    const delegate = { ...f.delegate, attach(r) { originalAttach(r); current = r; }, failed(r) { originalFailed(r); if (mode === "callback") throw error; } };
    const binding = { instance_id: "fixture", profile: "fixture", native_id: f.manager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 6, binding, aoe_bin: "/unused", app_dir: f.dir };
    const pi = { registerFlag() {}, getFlag: () => JSON.stringify(bootstrap), on: (name, fn) => handlers.set(name, fn) };
    const ctx = { hasUI: true, sessionManager: f.manager, isIdle: () => true, hasPendingMessages: () => false,
      ui: { onTerminalInput: () => () => {}, setStatus() { if (localFailure && mode === "status") throw statusError; } } };
    registerAfk(pi, { protocol: 6, delegation: observeDelegation(delegate, trace, state), schedule: fn => { scheduled = fn; return () => {}; },
      bridge: () => observeBridge({ async request() { if (holding) { entered.resolve(); await gate.promise; } return { policy: null }; }, async close() {} }, trace, state) });
    try {
      await handlers.get("session_start")({}, ctx);
      assert.ok(current.usageWitness); assert.ok(current.usageCollector);
      if (mode === "joined") {
        holding = true; scheduled(); await entered.promise;
        const first = current.refreshing, joined = current.refresh("before_tool");
        assert.equal(joined, first); assert.equal(lines.filter(e => e.event === "request").length, 2);
        gate.resolve(); await joined;
        assert.equal(lines.filter(e => e.event === "request").length, 2, "join did not invent another poll");
        assert.ok(!JSON.stringify(lines).match(/cycle|join|before_tool/));
      } else {
        current.usageCollector.observe = () => { throw Error("PRIVATE local observation"); };
        localFailure = true;
        state.teardown = mode === "teardown-observation";
        const refreshing = current.refresh();
        if (mode === "callback") await assert.rejects(refreshing, e => e === error);
        else if (mode === "status") { await assert.rejects(refreshing, e => e === statusError); assert.equal(current.refreshing, refreshing, "original finalizer failure leaves coalescing stuck"); }
        else await refreshing;
        if (mode.includes("observation")) assert.equal(!!state.fault, mode === "teardown-observation");
        assert.equal(lines.filter(e => e.event === "request").length, 1, "local error preceded poll request");
        assert.equal(lines.filter(e => e.event === "failed").length, 1);
        assert.equal(lines.some(e => e.event === "failed_threw"), mode === "callback");
      }
    } finally { localFailure = false; await handlers.get("session_shutdown")(); f.close(); }
  }
});

test("delegation forwarding, writer failure and teardown faults never suppress callback behavior", async () => {
  const state = { teardown: true }, value = {}, failure = Error("PRIVATE");
  const trace = createTrace(() => { throw failure; }); let calls = 0;
  const original = { attach(...args) { assert.equal(this, original); assert.equal(args[0], value); calls++; return value; }, failed() { calls++; throw failure; } };
  const d = observeDelegation(original, trace, state);
  assert.equal(d.attach(value), value); assert.throws(() => d.failed(value), e => e === failure); assert.equal(calls, 2);
  assert.equal(trace.snapshot().invalid, true); assert.equal(state.fault, true);
  const late = { teardown: false }, { trace: t } = capture(), gate = latch();
  const bridge = observeBridge({ request: () => gate.promise.then(() => { throw failure; }), close: () => Promise.reject(failure) }, t, late);
  const pending = bridge.request({ op: "poll" }); late.teardown = true; gate.resolve();
  await assert.rejects(pending, e => e === failure); assert.equal(late.fault, true);
  await assert.rejects(bridge.close(), e => e === failure);
});

test("pending cutoff emits nothing until cancellation and resolves both iterator and result; already aborted also completes", async () => {
  const { AssistantMessageEventStream } = await load("packages/ai/src/utils/event-stream.ts");
  for (const preAborted of [false, true]) {
    const controller = new AbortController(); let cancelled = 0;
    if (preAborted) controller.abort();
    const stream = pendingStream(AssistantMessageEventStream, { api: "fixture", provider: "fixture", id: "fixture" }, controller.signal, () => cancelled++);
    const iterator = stream[Symbol.asyncIterator](); let delivered = false;
    const next = iterator.next().then(value => { delivered = true; return value; });
    if (!preAborted) { await Promise.resolve(); assert.equal(delivered, false); controller.abort(); }
    const first = await next, result = await stream.result();
    assert.equal(first.value.type, "error"); assert.equal(result.stopReason, "aborted"); assert.deepEqual(result.content, []);
    assert.equal((await iterator.next()).done, true); assert.equal(cancelled, 1);
  }
});

test("native readiness needs matching opened and UI ownership in either order; mismatches never qualify", () => {
  for (const order of ["opened-first", "start-first"]) {
    for (const mismatch of ["none", "owner", "kind", "tool", "attribution"]) {
      let ready = 0, faults = 0;
      const observe = questionAttribution(() => ready++, () => faults++);
      const opened = { version: 1, supported: true, uiPromptAttribution: mismatch !== "attribution", state: "pending", toolCallId: mismatch === "tool" ? "wrong" : "fixture-question-1", interactionId: "local-test", owner: { namespace: "pi-ask-user", id: "local-test" }, kind: "input" };
      const start = { owner: { namespace: "pi-ask-user", id: mismatch === "owner" ? "other" : "local-test" }, kind: mismatch === "kind" ? "select" : "input" };
      if (order === "opened-first") { observe.opened(opened); assert.equal(ready, 0); observe.start(start); }
      else { observe.start(start); assert.equal(ready, 0); observe.opened(opened); }
      assert.equal(ready, mismatch === "none" ? 1 : 0);
      assert.equal(faults, ["tool", "attribution"].includes(mismatch) ? 1 : 0);
    }
  }
});

test("observer-only hooks cannot throw into product or create rejected observer reactions", async () => {
  for (const rejects of [false, true]) {
    const { trace } = capture(), error = Error("product"), value = {};
    const state = { teardown: false, syncTeardown() { throw Error("observer only"); } };
    const original = rejects ? Promise.reject(error) : Promise.resolve(value);
    const bridge = observeBridge({ request: () => original, close: () => Promise.resolve(value) }, trace, state);
    assert.equal(bridge.request({ op: "poll" }), original);
    if (rejects) await assert.rejects(original, e => e === error); else assert.equal(await original, value);
    assert.equal(await bridge.close(), value);
    const d = observeDelegation({ attach: () => value, failed() { throw error; } }, trace, state);
    assert.equal(d.attach(), value); assert.throws(() => d.failed(), e => e === error);
    assert.equal(trace.snapshot().invalid, true); assert.equal(state.fault, true);
  }
});

test("write-once marker is invisible until complete publication, with a controlled prepublication barrier", () => {
  const dir = mkdtempSync(join(tmpdir(), "observer-marker-")), file = join(dir, "ready.json");
  try {
    const expected = { ready: true };
    publishMarker(file, expected, () => {
      assert.equal(existsSync(file), false, "consumer must not parse a partially written final path");
      assert.deepEqual(JSON.parse(readFileSync(`${file}.pending`)), expected);
    });
    assert.deepEqual(JSON.parse(readFileSync(file)), expected);
    assert.throws(() => publishMarker(file, { ready: false }));
    assert.deepEqual(JSON.parse(readFileSync(file)), expected, "never overwrite an existing marker");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("only the matching exact native deferral result qualifies, never ordinary completion or error", () => {
  const question = { toolCallId: "fixture-question-1", interactionId: "private-id" };
  for (const mode of ["accepted", "answer", "cancel", "error", "tool", "interaction", "reason"]) {
    const event = { toolName: "ask_user", toolCallId: question.toolCallId, isError: false, details: { ...question, status: "no_human_answer", reason: "deferred_by_afk", response: null, cancelled: true } };
    if (mode === "answer") event.details.response = "PRIVATE";
    if (mode === "cancel") delete event.details.status;
    if (mode === "error") event.isError = true;
    if (mode === "tool") event.toolCallId = "other";
    if (mode === "interaction") event.details.interactionId = "other";
    if (mode === "reason") event.details.reason = "other";
    assert.equal(nativeDeferredResult(question, event), mode === "accepted");
  }
});

test("armed source-SDK idle wait outlives abort, delayed consumption and early-idle settled handlers without entering input", { timeout: 20000 }, async () => {
  const sdk = await load("packages/coding-agent/src/index.ts");
  const { loadExtensions, createExtensionRuntime } = await load("packages/coding-agent/src/core/extensions/loader.ts");
  const { AssistantMessageEventStream } = await load("packages/ai/src/utils/event-stream.ts");
  const dir = mkdtempSync(join(tmpdir(), "observer-idle-"));
  const entered = latch(), armed = latch(), aborted = latch(), consume = latch(), settling = latch(), release = latch();
  const state = { calls: 2, waitArmed: false, runCompleted: false, fault: false, teardown: false, cancelled: false };
  const markers = [], errors = []; let inputs = 0, providerCalls = 0, earlyIdle, session, running, command, aborting, captured;
  const save = (name, value) => { markers.push(name); if (name === "wait-armed.json") armed.resolve(); return true; };
  globalThis.__observerIdle = { command: (args, ctx) => { captured = ctx; return cancellationCommand(state, save)(args, ctx); }, input() { inputs++; },
    consumed(event) { if (state.episode && event.message.role === "assistant" && event.message.stopReason === "aborted") state.episode.consumed = true; }, async settled(_event, ctx) { earlyIdle = ctx.isIdle(); settling.resolve(); await release.promise; } };
  const entry = join(dir, "entry.mjs");
  writeFileSync(entry, `export default pi=>{const f=globalThis.__observerIdle;pi.registerCommand('fixture-await-cancel',{handler:f.command});pi.on('input',f.input);pi.on('message_end',f.consumed);pi.on('agent_settled',f.settled);};`);
  try {
    const loaded = await loadExtensions([entry], dir, sdk.createEventBus(), createExtensionRuntime()); assert.deepEqual(loaded.errors, []);
    const resources = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "offline cancellation", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, async reload() {} };
    const model = { id: "fixture", name: "fixture", provider: "fixture", api: "fixture", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000 };
    const models = await sdk.ModelRuntime.create({ authPath: join(dir, "absent-auth"), modelsPath: null, modelsStorePath: join(dir, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    class DelayedStream extends AssistantMessageEventStream { async *[Symbol.asyncIterator]() { await consume.promise; yield* super[Symbol.asyncIterator](); } }
    models.registerProvider("fixture", { api: "fixture", apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, _context, options) {
      providerCalls++; state.episode = { signal: options.signal, consumed: false };
      const stream = pendingStream(DelayedStream, model, options.signal, () => { state.cancelled = true; aborted.resolve(); });
      entered.resolve(); return stream;
    } });
    const manager = sdk.SessionManager.create(dir, join(dir, "sessions"));
    ({ session } = await sdk.createAgentSession({ cwd: dir, agentDir: dir, model, modelRuntime: models, resourceLoader: resources, sessionManager: manager,
      settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), tools: [] }));
    await session.bindExtensions({ onError: error => errors.push(error), commandContextActions: { waitForIdle: () => session.waitForIdle() } });
    running = session.prompt("ordinary fixture episode"); await entered.promise;
    command = session.prompt("/fixture-await-cancel", { streamingBehavior: "steer" }); await armed.promise;
    assert.equal(inputs, 1); assert.equal(session.agent.peekQueuedMessages().length, 0);
    state.teardown = true; aborting = session.abort(); await aborted.promise;
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(state.runCompleted, false); assert.equal(state.episode.consumed, false);
    consume.resolve(); await settling.promise;
    assert.equal(earlyIdle, true, "isIdle alone would incorrectly pass while this awaited handler is held");
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(state.runCompleted, false); assert.deepEqual(markers, ["wait-armed.json"]);
    release.resolve(); await Promise.all([running, command, aborting]);
    assert.equal(state.runCompleted, true, JSON.stringify({cancelled:state.cancelled,consumed:state.episode.consumed,aborted:state.episode.signal.aborted,teardown:state.teardown,idle:captured.isIdle(),pending:captured.hasPendingMessages(),fault:state.fault})); assert.equal(state.fault, false); assert.equal(providerCalls, 1);
    assert.deepEqual(markers, ["wait-armed.json", "run-completed.json"]); assert.deepEqual(errors, []);
    assert.equal(inputs, 1); assert.equal(session.agent.peekQueuedMessages().length, 0);
    assert.ok(!JSON.stringify(manager.getEntries()).includes("/fixture-await-cancel"));
    assert.equal(manager.getEntries().filter(e => e.type === "message" && e.message.role === "assistant").at(-1).message.stopReason, "aborted");
  } finally {
    consume.resolve(); release.resolve(); await session?.abort(); await Promise.allSettled([running, command, aborting]);
    session?.dispose(); delete globalThis.__observerIdle; rmSync(dir, { recursive: true, force: true });
  }
});

test("command failures are sticky even when SDK would swallow them; missing completion never qualifies", async () => {
  for (const mode of ["already-idle", "wait-failed", "publish-failed", "replaced"]) {
    const controller = new AbortController(), done = latch(), armed = latch(); let waiting = 0;
    const state = { calls: 2, episode: { signal: controller.signal, consumed: false }, fault: false };
    const markers = [];
    const ctx = { sessionManager: { getSessionId: () => "local" }, isIdle: () => mode === "already-idle" || controller.signal.aborted, hasPendingMessages: () => false,
      waitForIdle() { waiting++; if (mode === "wait-failed") throw Error("PRIVATE"); return done.promise; } };
    const command = cancellationCommand(state, name => { markers.push(name); armed.resolve(); return mode !== "publish-failed"; });
    const running = command("", ctx);
    if (mode === "replaced") {
      await armed.promise; controller.abort(); state.cancelled = true; state.teardown = true;
      state.episode = { signal: controller.signal, consumed: true }; done.resolve();
    }
    await running;
    assert.equal(state.fault, true); assert.notEqual(state.runCompleted, true);
    assert.equal(markers.includes("run-completed.json"), false);
    assert.equal(waiting, mode === "already-idle" ? 0 : 1);
  }
});
