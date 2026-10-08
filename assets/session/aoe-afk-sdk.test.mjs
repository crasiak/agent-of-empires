import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID, createHash } from "node:crypto";

const root = process.env.AOE_PI_ROOT;
const sourceRoot = process.env.AOE_PI_SOURCE_ROOT;
const coverage = "stock-sdk-main-loop-reservations";
const sha = x => createHash("sha256").update(x).digest("hex");
function claims() {
  return { id: randomUUID(), question: "Scratch choice?", alternatives: [{ name: "hello", pros: "simple", cons: "small", scores: [5] }, { name: "other", pros: "different", cons: "unneeded", scores: [2] }], criteria: [{ name: "clarity", weight: 4 }], evidence: ["scratch grant"], assumptions: [], uncertainties: [], recommendation: "hello", rationale: "Small reversible scratch task", rollback: "Remove manually", reversible: true, risk: "low", human_required: false, depends_on: [], action: { path: "scratch.txt", expected_hash: null, content: "hello\n" } };
}

async function runCase(scenario, externalHost) {
  const sdk = await import(pathToFileURL(sourceRoot ? join(sourceRoot, "packages/coding-agent/src/index.ts") : join(root, "dist/index.js")));
  assert.equal(sdk.VERSION, "0.87.1");
  const { loadExtensions, createExtensionRuntime } = await import(pathToFileURL(sourceRoot ? join(sourceRoot, "packages/coding-agent/src/core/extensions/loader.ts") : join(root, "dist/core/extensions/loader.js")));
  const { AssistantMessageEventStream } = await import(pathToFileURL(sourceRoot ? join(sourceRoot, "packages/ai/src/utils/event-stream.ts") : join(root, "node_modules/@earendil-works/pi-ai/dist/utils/event-stream.js")));
  const cwd = await mkdtemp(join(tmpdir(), "aoe-afk-sdk-"));
  let session, supportsNudges = false;
  const nudging = scenario.startsWith("nudge-");
  const twice = scenario === "nudge-two";
  const liveCustom = scenario === "nudge-late-extension-live";
  const lateCustom = scenario === "nudge-late-extension" || liveCustom;
  let liveForeignSeen = false;
  let hostEvidence = [], checkpointSequence = 0, admissions = 0, intents = 0, checkpoint;
  let enter, release;
  const entered = new Promise(resolve => enter = resolve), gate = new Promise(resolve => release = resolve);
  try {
    const sessionManager = sdk.SessionManager.inMemory(cwd);
    const binding = { instance_id: "test", profile: "default", native_id: sessionManager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 3, app_dir: cwd, aoe_bin: "/unused/aoe", binding };
    const d = claims();
    if (scenario === "question") { d.human_required = true; d.action = null; }
    const grant = { version: 3, task: "scratch", scope: "scratch text", files: [{ path: "scratch.txt", capability: "create" }], requests: nudging ? (twice || liveCustom ? 8 : 4) : scenario === "limit-one" ? 1 : 2, settlement_nudges: nudging ? (twice ? 2 : 1) : 0, assurance: coverage };
    let armed = false, stopped = false, stale = false, later = false, generation, requestNumber = 0, recordRequest, attempted = false, toolExecutions = 0, calls = 0, reserves = 0, settled = 0, uiEnded = 0, compactCancelled = 0;
    const ops = [], messages = [], errors = [], warmingChecks = [];
    const challenge = randomUUID();
    const warming = action => session.extensionRunner.emitCacheWarmingDecision({ type: "cache_warming_decision", action, warmCost: 0, missCost: 1, continuationProbability: 1 });
    const assertNativeWarming = async () => { for (const action of ["warm", "stop"]) assert.equal(await warming(action), action, `${scenario}: ordinary warming passes through`); };
    const view = { version: 3, revision: 1, window: randomUUID(), binding, generation: "", state: "pending", confirmed: true, issued_at_ms: Date.now(), expires_at_ms: Date.now() + 60000, grant, grant_hash: sha(JSON.stringify(grant)), reservations_used: 0 };
    const hostOperation = async packet => {
      ops.push(packet);
      if (externalHost) return externalHost({ packet, generation, binding, decision: d });
      if (packet.op === "poll" && recordRequest && scenario === "failed-refresh") throw Error("poll failed after ownership");
      if (packet.op === "poll" && recordRequest && scenario === "missing-ledger") return { policy: null, probe: null, delegation: null, runtime_probe: null };
      if (packet.op === "poll") {
        const result = { policy: null, probe: null, delegation: armed ? { ...view, state: stale ? "pending" : stopped ? "ended" : view.state, generation, reservations_used: requestNumber } : null, runtime_probe: armed ? { version: 3, binding, challenge, window: view.window, grant_hash: view.grant_hash } : null };
        if (scenario === "pending-poll-stop" && armed && !stopped) void session.abort();
        return result;
      }
      if (packet.op === "runtime_ack" && scenario === "pending-ack-stop") void session.abort();
      if (packet.op !== "runtime") return { acknowledged: true };
      const cmd = packet.command;
      if (["stop", "settle"].includes(cmd.kind)) { stopped = true; return { ended: true }; }
      assert.ok(!stopped, "host stopped");
      assert.ok(view.expires_at_ms > Date.now(), "host expiry");
      if (cmd.kind === "reserve") {
        reserves++;
        if (scenario === "failed-store") throw Error("injected failed store");
        assert.ok(requestNumber < grant.requests, "request cap");
        view.state = "owned"; requestNumber++; return { request_number: requestNumber, grant };
      }
      if (cmd.kind === "read") return { evidence: sha("observed scratch missing") };
      if (cmd.kind === "checkpoint") { checkpoint = { sequence: ++checkpointSequence, claims: cmd.claims }; view.checkpoints = [checkpoint]; return checkpoint; }
      if (cmd.kind === "nudge") { admissions++; return { identity: { window: view.window, generation, revision: 1, checkpoint: cmd.checkpoint, admission: randomUUID() } }; }
      if (cmd.kind === "contribution_intent") { intents++; return { identity: cmd.identity }; }
      if (cmd.kind === "record") { recordRequest = requestNumber; if (scenario === "lost-ack") throw Error("record committed but acknowledgement lost"); if (scenario === "expiry") view.expires_at_ms = Date.now(); return { evidence: [sha("admitted scratch action")], decision: d.id, disposition: d.human_required ? "human_required" : "admitted", record_request: recordRequest, ...(scenario === "compaction" ? { filler: "evidence ".repeat(10000) } : {}) }; }
      if (cmd.kind === "apply") { assert.equal(await warming("warm"), "stop", "owned work suppresses warming"); assert.ok(requestNumber > recordRequest, "later request required"); assert.ok(!attempted); attempted = true; stopped = true; return { outcome: "observed_applied" }; }
      throw Error("unexpected host command");
    };
    const host = async packet => {
      const cmd = packet.command;
      if (nudging && cmd?.kind === "nudge" && scenario.endsWith("during")) { enter(); await gate; }
      const result = await hostOperation(packet);
      if (nudging && result?.evidence) hostEvidence = Array.isArray(result.evidence) ? result.evidence : [result.evidence];
      if (nudging && cmd?.kind === "nudge") {
        if (scenario === "nudge-lost-ack") throw Error("admission persisted, acknowledgement lost");
      }
      if (nudging && cmd?.kind === "contribution_intent" && ["nudge-stop-after", "nudge-human", "nudge-extension", "nudge-off-after", "nudge-expiry-after"].includes(scenario)) { enter(); await gate; }
      return result;
    };
    const invalidate = async kind => {
      if (externalHost) await externalHost({ packet: { fixture: kind }, generation, binding, decision: d });
      if (kind === "off") stopped = true;
      else view.expires_at_ms = Date.now();
    };
    globalThis.__aoeAfkFixture = {
      bootstrap,
      capture(r) { this.current = r; },
      bridge(_boot, gen) { generation = gen; return { async request(packet) { const result = await host(packet); if (packet.op === "poll" && !armed) result.delegation = null; return result; }, async close() {} }; },
      async before(event) {
        if (!nudging || calls !== 4) return;
        if (scenario === "nudge-stop-before") void session.abort();
        if (scenario === "nudge-off-before") await invalidate("off");
        if (scenario === "nudge-expiry-before") await invalidate("expiry");
        if (scenario === "nudge-prior") return { entries: [...event.entries, { type: "custom_message", customType: "other", content: "prior contribution", display: true }], continue: true };
      },
      async context(event) {
        if (liveCustom && !later && calls === 5 && event.messages.some(m => m.customType === "other")) {
          const current = globalThis.__aoeAfkFixture.current;
          assert.ok(current.owned && current.view.state === "owned", "foreign context arrives before terminal latch");
          assert.ok(ops.filter(p => p.op === "runtime" && p.command.kind === "reserve").length < grant.requests);
          liveForeignSeen = true;
        }
        if (!nudging || calls !== 4 || !globalThis.__aoeAfkFixture.current.expectedContinuation) return;
        if (scenario === "nudge-stop-context") void session.abort();
        if (scenario === "nudge-off-context") await invalidate("off");
        if (scenario === "nudge-expiry-context") await invalidate("expiry");
      },
      async after(event) {
        if (nudging && calls === 4 && scenario === "nudge-late-human") await session.prompt("human message preserved", { streamingBehavior: "followUp" });
        if (nudging && calls === 4 && lateCustom) await session.sendCustomMessage({ customType: "other", content: "late queued extension", display: true }, { deliverAs: "followUp", triggerTurn: true });
        if (nudging && calls === 4 && scenario === "nudge-late") return { entries: [...event.entries, { type: "custom_message", customType: "other", content: "late contribution", display: true }], continue: true };
      },
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
export default function(pi) { const f=globalThis.__aoeAfkFixture; pi.on("agent_before_settle",f.before); pi.on("context",f.context); const delegate=delegationRuntime(pi); registerAfk(pi,{protocol:3,delegation:{...delegate,attach(r){delegate.attach(r);f.capture(r);f.refresh=()=>r.refresh();}},bridge:f.bridge,schedule:()=>()=>{}}); pi.on("agent_before_settle",f.after); pi.on("agent_settled",f.observe); pi.on("ui_prompt_end",f.observe); pi.on("session_compact_failed",f.observe); pi.registerTool({name:"noop",label:"noop",description:"ordinary tool",parameters:Type.Object({}),execute:f.noop}); pi.registerTool({name:"ask_user",label:"Ask User",description:"0.12 compatible question shape",parameters:Type.Object({question:Type.String()}),execute:async()=>{throw Error("question must never execute");}}); }
`);
    const runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    const loaded = await loadExtensions([fixture], cwd, sdk.createEventBus(), runtime);
    assert.deepEqual(loaded.errors, [], "actual Pi loader resolves core and TypeBox peers");
    const resourceLoader = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline AFK fixture", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources: () => {}, reload: async () => {} };
    const model = { id: "fake", name: "fake", provider: "afk-fixture", api: "afk-fixture", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000 };
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, context, options) {
      assert.equal(options.signal.aborted, false); assert.ok(++calls <= 10, "unexpected provider dispatch");
      messages.push(context.messages);
      let tools;
      if (later || scenario === "pending-idle") tools = later === "tool" ? [{ name: "noop", arguments: {} }] : [];
      else if (calls === 1) { armed = true; tools = [{ name: "noop", arguments: {} }]; }
      else if (nudging && !supportsNudges) tools = [];
      else if (nudging) {
        if (calls === 2) tools = [{ name: twice ? "aoe_afk_read" : "aoe_afk_record", arguments: twice ? { path: "scratch.txt" } : { decision: d } }];
        else if (calls === 3 || (twice && calls === 6)) tools = [{ name: "aoe_afk_checkpoint", arguments: { checkpoint: { status: "unfinished", next_step: twice && calls === 3 ? { kind: "record", path: "scratch.txt" } : { kind: "apply", decision: d.id }, rationale: "Granted next step remains", evidence: hostEvidence, depends_on: [] } } }];
        else if (calls === 4 || (twice && calls === 7) || (liveCustom && calls === 5) || ["nudge-prior", "nudge-human", "nudge-extension"].includes(scenario)) tools = [];
        else tools = [{ name: twice && calls === 5 ? "aoe_afk_record" : "aoe_afk_apply", arguments: { decision: twice && calls === 5 ? d : d.id } }];
      }
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
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader, settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: scenario === "compaction", reserveTokens: 90000, keepRecentTokens: 1 }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager, tools: ["noop", "aoe_afk_record", "aoe_afk_apply", "aoe_afk_read", "aoe_afk_checkpoint", "ask_user"] }));
    await session.bindExtensions({ onError: e => errors.push(e), ...(scenario === "ui-answer" ? { uiContext: { input: async () => "real answer", setStatus() {} }, mode: "tui" } : {}) });
    supportsNudges = typeof globalThis.__aoeAfkFixture.current.ctx.abortPreservingQueue === "function";
    if (nudging && !supportsNudges) {
      assert.ok(!sourceRoot, "candidate source must implement the actual queue-preserving operation");
      await session.prompt("ordinary active task before unsupported grant"); await session.waitForIdle();
      assert.equal(calls, 2, "unsupported grant leaves ordinary work unchanged");
      assert.equal(reserves, 0);
      assert.equal(ops.filter(p => p.op === "runtime" && ["reserve", "nudge", "record"].includes(p.command.kind)).length, 0);
      assert.equal(ops.filter(p => p.op === "runtime_ack").length, 0);
      assert.match(globalThis.__aoeAfkFixture.current.reason, /queue-preserving abort capability required/);
      assert.equal(session.messages.filter(m => m.customType === "aoe-afk-continuation").length, 0);
      await assertNativeWarming(); assert.deepEqual(errors, []);
      return { scenario, capability_refused: true, calls, requestNumber, attempted };
    }
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
      const running = session.prompt("ordinary work already in flight");
      let stopping;
      if (["nudge-stop-during", "nudge-stop-after", "nudge-human", "nudge-extension", "nudge-off-during", "nudge-expiry-during", "nudge-off-after", "nudge-expiry-after"].includes(scenario)) {
        await entered;
        assert.equal(calls, 4); assert.equal(settled, 0);
        if (scenario.includes("stop")) stopping = session.abort();
        if (scenario === "nudge-human") await session.prompt("human message preserved", { streamingBehavior: "followUp" });
        if (scenario === "nudge-extension") await session.sendCustomMessage({ customType: "other", content: "extension queued", display: true }, { deliverAs: "followUp", triggerTurn: true });
        if (scenario.includes("off")) await invalidate("off");
        if (scenario.includes("expiry")) await invalidate("expiry");
        release();
      }
      await running; await stopping; await session.waitForIdle();
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
    if (nudging) {
      const positive = ["nudge-one", "nudge-two", "nudge-late-extension"].includes(scenario);
      assert.equal(calls, positive ? twice ? 8 : 5 : liveCustom ? 5 : 4, `${scenario}: registered provider calls; ${globalThis.__aoeAfkFixture.current.reason}; ${JSON.stringify(ops.filter(o => o.op === "runtime"))}`);
      assert.equal(session.messages.filter(m => m.role === "custom" && m.customType === "aoe-afk-continuation").length,
        positive ? twice ? 2 : 1 : liveCustom || ["nudge-stop-before", "nudge-stop-during", "nudge-stop-after", "nudge-late", "nudge-late-human", "nudge-late-extension", "nudge-off-after", "nudge-expiry-after", "nudge-off-context", "nudge-stop-context", "nudge-expiry-context"].includes(scenario) ? 1 : 0, `${scenario}: committed contributions`);
      if (["nudge-human", "nudge-late-human"].includes(scenario)) assert.ok([...session.messages, ...session.agent.peekQueuedMessages()].some(m => m.role === "user" && JSON.stringify(m).includes("human message preserved")), "original human message retained in transcript or SDK queue");
      if (lateCustom || ["nudge-extension", "nudge-prior", "nudge-late"].includes(scenario)) assert.ok([...session.messages, ...session.agent.peekQueuedMessages()].some(m => m.role === "custom" && m.customType === "other"), "competing work remains in transcript or SDK queue if conservatively parked");
      assert.ok(session.messages.filter(m => m.role === "toolResult" && m.toolName.startsWith("aoe_afk")).every(m => !m.isError), "checkpoint/action tools succeeded");
      if (!externalHost) assert.equal(attempted, positive);
      assert.deepEqual(messages.flatMap((context, index) => JSON.stringify(context).includes("[AoE settlement nudge]") ? [index + 1] : []), positive || liveCustom ? twice ? [5, 8] : [5] : [], "only the observed continuation request receives its labelled nudge");
      if (lateCustom) assert.ok(messages.every(context => !JSON.stringify(context).includes("late queued extension")), "late custom work never enters an AFK provider context");
      if (liveCustom) {
        assert.ok(liveForeignSeen, "guard exercised on a still-live owned context");
        assert.equal(ops.filter(p => p.op === "runtime" && p.command.kind === "reserve").length, 4, "foreign work spends no reservation");
        assert.equal(ops.filter(p => p.op === "runtime" && p.command.kind === "apply").length, 0);
      }
      const before = calls; later = "done";
      await session.sendCustomMessage({ customType: "ordinary-later", content: "ordinary later request", display: true }, { triggerTurn: true });
      await session.waitForIdle();
      assert.equal(calls, before + (["nudge-late-human", "nudge-late-extension"].includes(scenario) ? 2 : 1));
      if (scenario === "nudge-late-human") assert.ok(session.messages.some(m => m.role === "user" && JSON.stringify(m).includes("human message preserved")), "later ordinary work recovers original queued human");
      if (lateCustom) assert.ok(session.messages.some(m => m.customType === "other" && m.content === "late queued extension"), "later ordinary work recovers queued custom");
      assert.ok(!JSON.stringify(messages.at(-1)).includes("[AoE settlement nudge]"), "stale instructions filtered, transcript retained");
      return { scenario, calls: before, admissions, intents, settled: settled - 1, ops: ops.filter(o => o.op === "runtime").map(o => o.command.kind) };
    }
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
  const result = await runCase(process.argv[process.argv.indexOf("--host") + 1] || "positive", async value => {
    process.stdout.write(JSON.stringify(value) + "\n");
    const line = await lines.next(); assert.equal(line.done, false);
    const result = JSON.parse(line.value); if (!result.ok) throw Error(result.error); return result.value;
  });
  process.stdout.write(JSON.stringify({ result }) + "\n"); process.exit(0);
} else {
  test(`actual ${sourceRoot ? "candidate source" : "stock"} Pi SDK request/ownership gate, offline provider`, { skip: !root && !sourceRoot, timeout: 60000 }, async () => {
    for (const scenario of ["positive", "sibling", "limit-one", "failed-store", "unknown-tool", "explicit-stop", "human-input", "off", "ui-answer", "question", "lost-ack", "expiry", "pending-idle", "pending-stop", "pending-completion", "pending-poll-stop", "pending-ack-stop", "pending-off", "pending-expiry", "compaction", "missing-ledger", "failed-refresh", "nudge-one", "nudge-two", "nudge-stop-before", "nudge-stop-during", "nudge-stop-after", "nudge-off-before", "nudge-expiry-before", "nudge-human", "nudge-extension", "nudge-prior", "nudge-late", "nudge-lost-ack", "nudge-off-during", "nudge-expiry-during", "nudge-off-after", "nudge-expiry-after", "nudge-stop-context", "nudge-off-context", "nudge-expiry-context", "nudge-late-human", "nudge-late-extension", "nudge-late-extension-live"]) { const result = await runCase(scenario); console.log(JSON.stringify(result)); if (result.capability_refused) { console.log("Candidate nudge matrix blocked: actual queue-preserving context operation absent"); break; } }
  });
}
