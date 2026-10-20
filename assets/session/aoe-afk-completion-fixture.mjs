import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";

const source = process.env.AOE_PI_SOURCE_ROOT;
const scenario = process.argv[2];
const sdk = await import(pathToFileURL(join(source, "packages/coding-agent/src/index.ts")));
const { loadExtensions, createExtensionRuntime } = await import(pathToFileURL(join(source, "packages/coding-agent/src/core/extensions/loader.ts")));
const { AssistantMessageEventStream } = await import(pathToFileURL(join(source, "packages/ai/src/utils/event-stream.ts")));
const lines = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
let queue = Promise.resolve();
const rpc = packet => {
  const next = queue.then(async () => {
    process.stdout.write(JSON.stringify({ packet }) + "\n");
    const line = await lines.next(); assert.equal(line.done, false);
    const reply = JSON.parse(line.value);
    if (!reply.ok) { const error = Error(reply.error); error.code = reply.code; throw error; }
    return reply.value;
  });
  queue = next.catch(() => {}); return next;
};
const cwd = await mkdtemp(join(tmpdir(), "aoe-completion-"));
const timer = setTimeout(() => { console.error("completion fixture deadline"); process.exit(1); }, 45000);
let session, native, r, unavailable = false, missingView = false, calls = 0, partialPages = 0, displaced, firstSettled = false;
const paging = scenario.startsWith("pagination-");
const incomplete = ["late-before-final", "pagination-lost", "pagination-obsolete"].includes(scenario);
const closingQueue = { role: "custom", customType: "held-during-completion", content: "original closing queue", display: false, timestamp: Date.now() };
const errors = [], decision = {
  id: randomUUID(), question: "Which greeting?", alternatives: [{ name: "hello", pros: "simple", cons: "short", scores: [5] }, { name: "other", pros: "different", cons: "unneeded", scores: [2] }],
  criteria: [{ name: "clarity", weight: 4 }], evidence: ["explicit disposable grant"], assumptions: [], uncertainties: [], recommendation: "hello", rationale: "small granted action", rollback: "remove manually", reversible: true, risk: "low", human_required: false, depends_on: [], action: { path: "scratch.txt", expected_hash: null, content: "hello\n" },
};
const markers = () => sdk.SessionManager.open(native.getSessionFile()).getEntries().filter(e => e.customType === "aoe-afk-usage-v1").map(e => e.data);
const assertClosing = () => {
  assert.equal(markers().filter(m => m.reason === "completed").length, 0, "release before final SDK completion");
  assert.equal(r.usageWitness.pending, 1);
  assert.equal(r.usageDenied, undefined, "completion-closing became a financial denial");
  assert.equal(calls, 3);
  assert.ok(session.agent.peekQueuedMessages().includes(closingQueue), "completion drained the original queued object");
};
const model = { id: "completion", name: "completion", provider: "afk-completion", api: "afk-completion", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000 };
const late = () => ({ role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(), content: [], stopReason: "aborted", errorMessage: "unqualified late attempt", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } });
try {
  native = sdk.SessionManager.create(cwd, join(cwd, "sessions"));
  const binding = { instance_id: "test", profile: "default", native_id: native.getSessionId(), launch_id: randomUUID() };
  const bootstrap = { version: 6, app_dir: cwd, aoe_bin: "/unused/aoe", binding };
  let introduced = false;
  globalThis.__completionFixture = {
    capture(value) { r = value; },
    bridge(_bootstrap, generation) {
      if (!introduced) { process.stdout.write(JSON.stringify({ binding, generation }) + "\n"); introduced = true; }
      return { close: async () => {}, async request(packet) {
        if (unavailable) throw Error("isolated host unavailable");
        const reply = await rpc(packet);
        if (missingView && packet.op === "poll") reply.delegation = null;
        if (paging && packet.usage?.boundary === "settled" && packet.usage.report?.entries.length === 128) {
          partialPages++;
          assert.equal(markers().some(m => m.reason === "completed"), false, "partial page released protection");
          if (scenario === "pagination-lost") unavailable = true;
          if (scenario === "pagination-obsolete") displaced = session.sendCustomMessage({ customType: "competing-episode", content: "not recovery", display: false }, { triggerTurn: true });
        }
        if (packet.command?.kind === "apply") session.agent.followUp(closingQueue);
        if (packet.command?.kind === "apply" && scenario === "poll-before-tool") {
          await r.refresh(); assertClosing();
        }
        return reply;
      } };
    },
    async noop() {
      await r.refresh(); await r.refresh();
      await rpc({ fixture: "enroll" });
      await r.refresh();
      return { content: [{ type: "text", text: "ordinary completed" }], details: {} };
    },
    async afterTurn() {
      if (unavailable || calls !== 3) return;
      if (scenario === "turn-before-poll") { await r.refresh(); assertClosing(); }
    },
    beforeSettled() {
      if (firstSettled || unavailable || calls !== 3) return;
      firstSettled = true;
      assertClosing();
      if (scenario === "late-before-final") native.appendMessage(late());
      if (paging && partialPages === 0) for (let i = 0; i < 140; i++) native.appendCustomEntry("final-metadata", { i });
    },
  };
  const fixture = join(cwd, "extension.mjs");
  await writeFile(fixture, `import {registerAfk} from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import {delegationRuntime} from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
import {Type} from "@sinclair/typebox";
export default function(pi){const f=globalThis.__completionFixture;pi.on("agent_settled",f.beforeSettled);const d=delegationRuntime(pi);registerAfk(pi,{protocol:6,delegation:{...d,attach(r){d.attach(r);f.capture(r);}},bridge:f.bridge,schedule:()=>()=>{}});pi.on("turn_end",f.afterTurn);pi.registerTool({name:"noop",label:"noop",description:"ordinary fixture",parameters:Type.Object({}),execute:f.noop});}`);
  let runtime, loaded;
  const load = async () => {
    runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    loaded = await loadExtensions([fixture], cwd, sdk.createEventBus(), runtime); assert.deepEqual(loaded.errors, []);
  };
  await load();
  const resources = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "offline completion fixture", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, reload: async () => {} };
  const models = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
  models.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, _context, options) {
    assert.equal(options.signal.aborted, false); assert.ok(++calls <= 3, "extra provider dispatch after effect completion");
    const tool = calls === 1 ? { name: "noop", arguments: {} } : calls === 2 ? { name: "aoe_afk_record", arguments: { decision } } : { name: "aoe_afk_apply", arguments: { decision: decision.id } };
    const message = { ...late(), errorMessage: undefined, stopReason: "toolUse", content: [{ type: "toolCall", id: "call-" + calls, ...tool }], usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    const stream = new AssistantMessageEventStream(); stream.push({ type: "done", reason: "toolUse", message }); stream.end(); return stream;
  } });
  const create = async () => {
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime: models, resourceLoader: resources, settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager: native, tools: ["noop", "aoe_afk_record", "aoe_afk_apply"] }));
    await session.bindExtensions({ onError: e => errors.push(e) });
  };
  await create();
  await session.prompt("ordinary work"); await session.waitForIdle();
  assert.equal(calls, 3); assert.deepEqual(errors, []);
  assert.ok(r.usageCompletion?.done, "missing identity-bound completion task");
  await r.usageCompletion.done;
  await displaced;
  if (paging) assert.equal(partialPages, 1, "pagination path must be exercised");
  assert.ok(session.agent.peekQueuedMessages().includes(closingQueue));
  const view = await rpc({ fixture: "inspect" });
  assert.equal(view.reservations_used, 2); assert.equal(view.terminal_reason, "completed");
  assert.ok(session.messages.filter(m => m.role === "toolResult").every(m => !m.isError));
  if (incomplete) {
    assert.equal(markers().some(m => m.reason === "completed"), false);
    assert.equal(r.usageWitness.pending, 1);
  } else {
    assert.equal(markers().filter(m => m.reason === "completed").length, 1);
    assert.equal(view.usage.measurement.tokens_complete, true);
    assert.equal(view.usage.gap, null);
    assert.equal(native.getEntries().filter(e => e.type === "message" && e.message.role === "assistant").length, 3, "synthetic trailing assistant");
    if (scenario === "late-after-final") { native.appendMessage(late()); await r.refresh(); assert.equal(r.usageWitness.pending, 1); }
    if (scenario === "host-negative") { await rpc({fixture:"negative"}); await r.refresh(); }
    if (scenario === "host-missing") { missingView = true; await r.refresh(); }
    if (scenario === "host-lost") { unavailable = true; await r.refresh(); }
    if (scenario.startsWith("host-")) {
      assert.equal(r.usageWitness.pending, 1, "changed/unavailable authority must revoke the live completion exemption");
      assert.equal(await session.extensionRunner.emitCacheWarmingDecision({type:"cache_warming_decision",action:"warm",warmCost:0,missCost:1,continuationProbability:1}), "stop");
    }
  }
  const file = native.getSessionFile(); session.dispose(); session = undefined;
  unavailable = true; native = sdk.SessionManager.open(file); await load(); await create();
  const original = { role: "custom", customType: "retained-original", content: "do not lose me", display: false, timestamp: Date.now() };
  session.agent.followUp(original);
  await session.sendCustomMessage({ customType: "automatic-after-reopen", content: "not human recovery", display: false }, { triggerTurn: true });
  await session.waitForIdle();
  assert.equal(calls, 3, "unavailable-host reopen released protection");
  assert.ok(session.agent.peekQueuedMessages().includes(original));
  assert.equal(r.usageWitness.pending, 1);
  assert.deepEqual(errors, []);
  process.stdout.write(JSON.stringify({ result: { scenario, passed: true, calls } }) + "\n");
} finally {
  clearTimeout(timer); session?.dispose(); delete globalThis.__completionFixture;
  await rm(cwd, { recursive: true, force: true });
}
process.exit(0);
