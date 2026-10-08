import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID, createHash } from "node:crypto";

const root = process.env.AOE_PI_ROOT;
const coverage = "stock-sdk-main-loop-reservations";
const sha = x => createHash("sha256").update(x).digest("hex");
function claims() {
  return { id: randomUUID(), question: "Scratch choice?", alternatives: [{ name: "hello", pros: "simple", cons: "small", scores: [5] }, { name: "other", pros: "different", cons: "unneeded", scores: [2] }], criteria: [{ name: "clarity", weight: 4 }], evidence: ["scratch grant"], assumptions: [], uncertainties: [], recommendation: "hello", rationale: "Small reversible scratch task", rollback: "Remove manually", reversible: true, risk: "low", human_required: false, depends_on: [], action: { path: "scratch.txt", expected_hash: null, content: "hello\n" } };
}

async function runCase(scenario, externalHost) {
  const sdk = await import(pathToFileURL(join(root, "dist/index.js")));
  assert.equal(sdk.VERSION, "0.87.1");
  const { loadExtensions, createExtensionRuntime } = await import(pathToFileURL(join(root, "dist/core/extensions/loader.js")));
  const { AssistantMessageEventStream } = await import(pathToFileURL(join(root, "node_modules/@earendil-works/pi-ai/dist/utils/event-stream.js")));
  const cwd = await mkdtemp(join(tmpdir(), "aoe-afk-sdk-"));
  let session;
  try {
    const sessionManager = sdk.SessionManager.inMemory(cwd);
    const binding = { instance_id: "test", profile: "default", native_id: sessionManager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 2, app_dir: cwd, aoe_bin: "/unused/aoe", binding };
    const d = claims();
    if (scenario === "question") { d.human_required = true; d.action = null; }
    const grant = { version: 2, task: "scratch", scope: "scratch text", files: [{ path: "scratch.txt", capability: "create" }], requests: scenario === "limit-one" ? 1 : 2, assurance: coverage };
    let armed = false, stopped = false, stale = false, later = false, generation, requestNumber = 0, recordRequest, attempted = false, toolExecutions = 0, calls = 0, reserves = 0, settled = 0, uiEnded = 0, compactCancelled = 0;
    const ops = [], messages = [], errors = [], warmingChecks = [];
    const challenge = randomUUID();
    const warming = action => session.extensionRunner.emitCacheWarmingDecision({ type: "cache_warming_decision", action, warmCost: 0, missCost: 1, continuationProbability: 1 });
    const assertNativeWarming = async () => { for (const action of ["warm", "stop"]) assert.equal(await warming(action), action, `${scenario}: ordinary warming passes through`); };
    const view = { version: 2, revision: 1, window: randomUUID(), binding, generation: "", state: "pending", confirmed: true, issued_at_ms: Date.now(), expires_at_ms: Date.now() + 60000, grant, grant_hash: sha(JSON.stringify(grant)), reservations_used: 0 };
    const host = async packet => {
      ops.push(packet);
      if (externalHost) return externalHost({ packet, generation, binding, decision: d });
      if (packet.op === "poll" && recordRequest && scenario === "failed-refresh") throw Error("poll failed after ownership");
      if (packet.op === "poll" && recordRequest && scenario === "missing-ledger") return { policy: null, probe: null, delegation: null, runtime_probe: null };
      if (packet.op === "poll") {
        const result = { policy: null, probe: null, delegation: armed ? { ...view, state: stale ? "pending" : stopped ? "ended" : view.state, generation, reservations_used: requestNumber } : null, runtime_probe: armed ? { version: 2, binding, challenge, window: view.window, grant_hash: view.grant_hash } : null };
        if (scenario === "pending-poll-stop" && armed && !stopped) void session.abort();
        return result;
      }
      if (packet.op === "runtime_ack" && scenario === "pending-ack-stop") void session.abort();
      if (packet.op !== "runtime") return { acknowledged: true };
      const cmd = packet.command;
      if (cmd.kind === "stop") { stopped = true; return { ended: true }; }
      assert.ok(!stopped, "host stopped");
      if (cmd.kind === "reserve") {
        reserves++;
        if (scenario === "failed-store") throw Error("injected failed store");
        assert.ok(requestNumber < grant.requests, "request cap");
        view.state = "owned"; requestNumber++; return { request_number: requestNumber, grant };
      }
      if (cmd.kind === "record") { recordRequest = requestNumber; if (scenario === "lost-ack") throw Error("record committed but acknowledgement lost"); if (scenario === "expiry") view.expires_at_ms = Date.now(); return { decision: d.id, disposition: d.human_required ? "human_required" : "admitted", record_request: recordRequest, ...(scenario === "compaction" ? { filler: "evidence ".repeat(10000) } : {}) }; }
      if (cmd.kind === "apply") { assert.equal(await warming("warm"), "stop", "owned work suppresses warming"); assert.ok(requestNumber > recordRequest, "later request required"); assert.ok(!attempted); attempted = true; stopped = true; return { outcome: "observed_applied" }; }
      throw Error("unexpected host command");
    };
    globalThis.__aoeAfkFixture = {
      bootstrap,
      capture(r) { this.current = r; },
      bridge(_boot, gen) { generation = gen; return { async request(packet) { const result = await host(packet); if (packet.op === "poll" && !armed) result.delegation = null; return result; }, async close() {} }; },
      observe(event) { if (event.type === "agent_settled") settled++; if (event.type === "ui_prompt_end") uiEnded++; if (event.type === "session_compact_failed" && event.aborted) compactCancelled++; },
      async noop(_id, _args, _signal, _update, ctx) {
        toolExecutions++;
        if (!later) {
          await globalThis.__aoeAfkFixture.refresh();
          if (scenario === "pending-stop") {
            assert.equal(reserves, 0, "pending activation precedes any AFK reservation");
            assert.ok(ops.some(p => p.op === "runtime_ack"), "active pending grant acknowledged");
            assert.equal(await warming("warm"), "stop", "pending work suppresses warming");
          }
          if (["pending-off", "pending-expiry"].includes(scenario)) {
            assert.equal(await warming("warm"), "stop");
            if (scenario === "pending-off") stopped = true;
            else view.expires_at_ms = Date.now();
            await globalThis.__aoeAfkFixture.refresh();
            await assertNativeWarming();
          }
          if (scenario === "pending-stop") {
            void session.abort();
            assert.equal(globalThis.__aoeAfkFixture.current.parked, true, "ordinary signal parks synchronously before reservation");
          }
        }
        if (scenario === "ui-answer") assert.equal(await ctx.ui.input("Already-open question"), "real answer");
        return { content: [{ type: "text", text: "ordinary tool" }], details: {} };
      },
    };
    const fixture = join(cwd, "extension.mjs");
    await writeFile(fixture, `import { registerAfk } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import { delegationRuntime } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
import { Type } from "@sinclair/typebox";
export default function(pi) { const f=globalThis.__aoeAfkFixture; const delegate=delegationRuntime(pi); registerAfk(pi,{protocol:2,delegation:{...delegate,attach(r){delegate.attach(r);f.capture(r);f.refresh=()=>r.refresh();}},bridge:f.bridge,schedule:()=>()=>{}}); pi.on("agent_settled",f.observe); pi.on("ui_prompt_end",f.observe); pi.on("session_compact_failed",f.observe); pi.registerTool({name:"noop",label:"noop",description:"ordinary tool",parameters:Type.Object({}),execute:f.noop}); pi.registerTool({name:"ask_user",label:"Ask User",description:"0.12 compatible question shape",parameters:Type.Object({question:Type.String()}),execute:async()=>{throw Error("question must never execute");}}); }
`);
    const runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    const loaded = await loadExtensions([fixture], cwd, sdk.createEventBus(), runtime);
    assert.deepEqual(loaded.errors, [], "actual Pi loader resolves core and TypeBox peers");
    const resourceLoader = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline AFK fixture", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources: () => {}, reload: async () => {} };
    const model = { id: "fake", name: "fake", provider: "afk-fixture", api: "afk-fixture", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000 };
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, context, options) {
      assert.equal(options.signal.aborted, false); assert.ok(++calls <= 4, "unexpected provider dispatch");
      messages.push(context.messages);
      let tools;
      if (later || scenario === "pending-idle") tools = later === "tool" ? [{ name: "noop", arguments: {} }] : [];
      else if (calls === 1) { armed = true; tools = [{ name: "noop", arguments: {} }]; }
      else if (["ui-answer", "pending-completion", "pending-off", "pending-expiry"].includes(scenario)) tools = [];
      else if (scenario === "question") tools = calls === 2 ? [{ name: "ask_user", arguments: { question: "Human choice required?" } }] : [{ name: "aoe_afk_record", arguments: { decision: d } }];
      else if (calls === 2) tools = scenario === "unknown-tool" ? [{ name: "noop", arguments: {} }] : [{ name: "aoe_afk_record", arguments: { decision: d } }, ...(scenario === "sibling" ? [{ name: "aoe_afk_apply", arguments: { decision: d.id } }] : [])];
      else tools = [{ name: "aoe_afk_apply", arguments: { decision: d.id } }];
      const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(), content: tools.length ? tools.map((t, i) => ({ type: "toolCall", id: `tool-${calls}-${i}`, ...t })) : [{ type: "text", text: "human-authorized follow-up" }], stopReason: tools.length ? "toolUse" : "stop", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      const stream = new AssistantMessageEventStream();
      if (scenario === "pending-completion" && !later) {
        message.content = [{ type: "text", text: "ordinary task complete" }]; message.stopReason = "stop";
        void (async () => {
          try {
            await globalThis.__aoeAfkFixture.refresh();
            assert.equal(reserves, 0);
            assert.ok(ops.some(p => p.op === "runtime_ack"));
            assert.equal(await warming("warm"), "stop");
            stream.push({ type: "done", reason: "stop", message }); stream.end();
          } catch (error) { stream.end(); errors.push(error); }
        })();
      } else { stream.push({ type: "done", reason: message.stopReason, message }); stream.end(); }
      if (later === "tool") later = "done";
      return stream;
    } });
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader, settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: scenario === "compaction", reserveTokens: 90000, keepRecentTokens: 1 }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager, tools: ["noop", "aoe_afk_record", "aoe_afk_apply", "aoe_afk_read", "ask_user"] }));
    await session.bindExtensions({ onError: e => errors.push(e), ...(scenario === "ui-answer" ? { uiContext: { input: async () => "real answer", setStatus() {} }, mode: "tui" } : {}) });
    if (["explicit-stop", "human-input", "off"].includes(scenario)) {
      session.subscribe(event => {
        if (event.type === "tool_execution_end" && event.toolName === "aoe_afk_record") {
          if (scenario === "explicit-stop") void session.abort();
          if (scenario === "human-input") void session.prompt("human message preserved", { streamingBehavior: "steer" });
          if (scenario === "off") stopped = true;
          warmingChecks.push(warming("warm").then(action => assert.equal(action, "stop", "ownership retained through terminal settlement")));
        }
      });
    }
    if (scenario === "pending-idle") {
      armed = true; await globalThis.__aoeAfkFixture.refresh();
      assert.equal(calls, 0); assert.equal(requestNumber, 0); assert.equal(stopped, true, "idle attempt durably ended");
      assert.equal(ops.filter(p => p.op === "runtime_ack").length, 0, "idle executable arming refused");
      assert.match(globalThis.__aoeAfkFixture.current.reason, /activation refused/);
      await session.abort();
    } else {
      await session.prompt("ordinary work already in flight"); await session.waitForIdle();
    }
    await Promise.all(warmingChecks);
    assert.notEqual(session.messages.find(m => m.role === "toolResult" && m.toolName === "noop")?.isError, true, "ordinary tool fixture assertions must not be swallowed by the SDK");
    await assertNativeWarming();
    if (["pending-idle", "pending-completion", "pending-stop", "pending-poll-stop", "pending-ack-stop"].includes(scenario)) {
      assert.equal(stopped, true);
      assert.equal(calls, scenario === "pending-idle" ? 0 : 1);
      assert.equal(reserves, 0);
      assert.equal(ops.filter(p => p.op === "runtime" && p.command.kind === "reserve").length, 0);
      stale = true;
      await globalThis.__aoeAfkFixture.refresh();
      await assertNativeWarming();
      const before = calls;
      later = "tool";
      await session.sendCustomMessage({ customType: "fixture-follow-up", content: "non-human ordinary follow-up", display: false }, { triggerTurn: true });
      await session.waitForIdle();
      assert.equal(calls, before + 2, "ordinary non-human request and native tool follow-up preserved");
      assert.ok(session.messages.some(m => m.role === "toolResult" && m.toolName === "noop" && !m.isError), "ordinary tool still executes after terminal delegation");
      assert.equal(reserves, 0, "stale window cannot be adopted by later non-human work");
      assert.equal(ops.filter(p => p.op === "runtime" && p.command.kind === "reserve").length, 0);
      assert.equal(attempted, false);
      assert.deepEqual(errors, []);
      return { scenario, calls, requestNumber, attempted, stopped, peerImports: "actual Pi loader" };
    }
    assert.deepEqual(errors, []);
    assert.equal(settled, 1);
    const expected = ["positive", "question"].includes(scenario) ? 3 : scenario === "failed-store" ? 1 : 2;
    assert.equal(calls, expected, `${scenario}: registered provider invocation count`);
    assert.equal(toolExecutions, 1, "ordinary batch not retroactively owned; delegated native tool blocked");
    if (scenario === "question") {
      const result = session.messages.find(m => m.role === "toolResult" && m.toolName === "ask_user");
      assert.match(JSON.stringify(result), /no_human_answer/); assert.doesNotMatch(JSON.stringify(result), /User answered|User cancelled/);
    }
    if (!externalHost) {
      assert.equal(attempted, scenario === "positive");
      if (scenario === "compaction") assert.equal(compactCancelled, 1, "owned automatic compaction cancelled before its provider request");
      if (scenario === "ui-answer") { assert.equal(uiEnded, 1); assert.equal(reserves, 0); }
      if (scenario === "human-input") assert.ok(session.messages.some(m => m.role === "user" && JSON.stringify(m.content).includes("human message preserved")), "human input kept intact");
      await session.extensionRunner.emit({ type: "session_start" });
      await assertNativeWarming();
      assert.deepEqual(errors, []);
    }
    return { scenario, calls, settled, requestNumber, attempted, ops: ops.filter(o => o.op === "runtime").map(o => o.command.kind), peerImports: "actual Pi loader" };
  } finally { session?.dispose(); delete globalThis.__aoeAfkFixture; await rm(cwd, { recursive: true, force: true }); }
}

if (process.argv.includes("--host")) {
  setTimeout(() => { console.error("host fixture deadline exceeded"); process.exit(1); }, 30000).unref();
  const { createInterface } = await import("node:readline");
  const lines = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  const result = await runCase("positive", async value => {
    process.stdout.write(JSON.stringify(value) + "\n");
    const line = await lines.next(); assert.equal(line.done, false);
    const result = JSON.parse(line.value); if (!result.ok) throw Error(result.error); return result.value;
  });
  process.stdout.write(JSON.stringify({ result }) + "\n"); process.exit(0);
} else {
  test("actual stock Pi SDK request/ownership gate, offline provider", { skip: !root, timeout: 60000 }, async () => {
    for (const scenario of ["positive", "sibling", "limit-one", "failed-store", "unknown-tool", "explicit-stop", "human-input", "off", "ui-answer", "question", "lost-ack", "expiry", "pending-idle", "pending-stop", "pending-completion", "pending-poll-stop", "pending-ack-stop", "pending-off", "pending-expiry", "compaction", "missing-ledger", "failed-refresh"]) console.log(JSON.stringify(await runCase(scenario)));
  });
}
