import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";

const source = process.env.AOE_PI_SOURCE_ROOT;
const FOUR_SLOT_COMMAND = "printf 'two\\n' > two.txt && node validate.mjs";
const FOUR_SLOT_VALIDATOR = `import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = dirname(fileURLToPath(import.meta.url));
assert.equal(await readFile(join(root, 'one.txt'), 'utf8'), 'one\\n');
assert.equal(await readFile(join(root, 'two.txt'), 'utf8'), 'two\\n');
assert.deepEqual((await readdir(root)).sort(), ['AGENTS.md', 'one.txt', 'two.txt', 'validate.mjs']);
console.log('PASS: both canary files have exact contents; no extra files');
`;
async function runNativeSdk(early, missingCheckpoint = false, fourSlot = false) {
  const finalCall = fourSlot ? 5 : early ? 7 : 6;
  const sdk = await import(pathToFileURL(join(source, "packages/coding-agent/src/index.ts")));
  const { loadExtensions, createExtensionRuntime } = await import(pathToFileURL(join(source, "packages/coding-agent/src/core/extensions/loader.ts")));
  const { AssistantMessageEventStream } = await import(pathToFileURL(join(source, "packages/ai/src/utils/event-stream.ts")));
  assert.equal(sdk.VERSION, "1.2.0");
  const cwd = await mkdtemp(join(tmpdir(), "aoe-work-sdk-")), key = `aoe-work-sdk-${randomUUID()}`;
  const fixture = {}, errors = [], outcomes = [], contexts = [], notices = [], consumedRefs = [];
  let session, calls = 0, armed = false, confirmations = 0;
  globalThis[key] = fixture;
  try {
    const workDir = fourSlot ? join(cwd, "task") : cwd;
    const instructions = "Write one.txt and two.txt with exact contents, then run the existing read-only validator. Do not change AGENTS.md or validate.mjs.\n";
    if (fourSlot) {
      await mkdir(workDir);
      await writeFile(join(workDir, "AGENTS.md"), instructions);
      await writeFile(join(workDir, "validate.mjs"), FOUR_SLOT_VALIDATOR);
    }
    let manager = sdk.SessionManager.create(workDir, join(cwd, "sessions"));
    const binding = { instance_id: "offline-fixture", profile: "default", native_id: manager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 6, app_dir: cwd, aoe_bin: "/unused/aoe", binding };
    Object.assign(fixture, { binding, capture(api, r) { fixture.api = api; fixture.r = r; }, replies: [], command: null,
      bridge() { return { async request(p) {
        if (p.op === "poll") return { policy: null, probe: null, delegation: null, runtime_probe: null, work_period_capability: 1, work_period_request: fixture.command };
        fixture.replies.push(p); return {};
      }, async close() {} }; } });
    const extension = join(cwd, "extension.mjs");
    await writeFile(extension, `import { VERSION } from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import { workPeriodRuntime,workCheckpointParameters } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-work-period.mjs")).href)};
import { registerAfk } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import { delegationRuntime } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
export default function(pi){const f=globalThis[${JSON.stringify(key)}];const delegation=delegationRuntime(pi);const api=workPeriodRuntime(pi,{version:VERSION,parameters:workCheckpointParameters(Type)});
registerAfk(pi,{protocol:6,delegation,workPeriod:{...api,attach(r){api.attach(r);f.capture(api,r);}},bridge:f.bridge,schedule:()=>()=>{}});}
`);
    const extensionRuntime = createExtensionRuntime(); extensionRuntime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    let loaded = await loadExtensions([extension], cwd, sdk.createEventBus(), extensionRuntime);
    assert.deepEqual(loaded.errors, []);
    const resources = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }),
      getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline multi-step work fixture",
      getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, reload: async () => {} };
    const model = { id: "offline", name: "offline", provider: "work-fixture", api: "work-fixture", baseUrl: "http://invalid.invalid", reasoning: false,
      input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 2000 };
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, context, options) {
      assert.equal(options.signal.aborted, false); assert.ok(++calls <= (fourSlot ? 5 : 8), "extra automatic provider request"); contexts.push(context.messages);
      const visibleRefs = names => names.map(name => {
        const message = context.messages.filter(m => m.role === "toolResult" && m.toolName === name).at(-1);
        const data = JSON.parse(message?.content.at(-1)?.text ?? "null");
        assert.equal(typeof data?.aoe_afk_work_evidence_ref, "string", `missing model-visible ${name} evidence ref`);
        consumedRefs.push(data.aoe_afk_work_evidence_ref);
        return data.aoe_afk_work_evidence_ref;
      });
      const checkpoint = (status, evidence) => ({ status, summary: status === "completed" ? "Two files and local validation finished" : "First artifact written; second and validation remain",
        next_step: status === "unfinished" ? "Write the second artifact and validate both" : null, evidence, human_required: false, depends_on: [] });
      const tools = !armed ? [] : calls === 2 ? [
        { id: "call_first|fc_first", name: "write", arguments: { path: fourSlot ? "one.txt" : "first.txt", content: "one\n" } },
      ] : calls === 3 ? [
        { id: "checkpoint-first", name: "aoe_afk_work_checkpoint", arguments: checkpoint("unfinished", visibleRefs(["write"])) },
      ] : calls === finalCall - 2 ? [
        { id: "call_second|fc_second", name: "write", arguments: { path: "second.txt", content: "two\n" } },
      ] : calls === finalCall - 1 ? [
        { id: "call_validate|fc_validate", name: "bash", arguments: { command: fourSlot ? FOUR_SLOT_COMMAND : "test \"$(cat first.txt)\" = one && test \"$(cat second.txt)\" = two && printf 'both validated\\n'" } },
      ] : calls === finalCall ? [
        { id: "checkpoint-final", name: "aoe_afk_work_checkpoint", arguments: checkpoint("completed", visibleRefs(fourSlot ? ["bash"] : ["write", "bash"])) },
      ] : [];
      const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(),
        content: tools.length ? tools.map(t => ({ type: "toolCall", ...t })) : [{ type: "text", text: "Ordinary task context ready" }], stopReason: tools.length ? "toolUse" : "stop",
        usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      const stream = new AssistantMessageEventStream(); stream.push({ type: "done", reason: message.stopReason, message }); stream.end(); return stream;
    } });
    const bind = async () => {
      ({ session } = await sdk.createAgentSession({ cwd: workDir, agentDir: cwd, model, modelRuntime, resourceLoader: resources, sessionManager: manager,
        settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }),
        tools: ["read", "write", "edit", "bash", ...missingCheckpoint ? [] : ["aoe_afk_work_checkpoint"]] }));
      session.subscribe(e => { if (["turn_start", "turn_end", "agent_settled"].includes(e.type)) outcomes.push({ type: e.type, outcome: e.outcome }); });
      await session.bindExtensions({ mode: "tui", uiContext: { setStatus() {}, getEditorText: () => "", notify: text => notices.push(text), confirm: async () => { confirmations++; return true; } }, onError: e => errors.push(e) });
    };
    await bind();
    await session.prompt(fourSlot ? "Write one.txt containing one followed by a newline. Checkpoint unfinished using its visible ref. Then run exactly: " + FOUR_SLOT_COMMAND + ". Checkpoint completed using the visible Bash ref; do not alter the validator."
      : "Write first.txt containing one, then second.txt containing two, validate both, and finish the existing task"); await session.waitForIdle();
    assert.equal(calls, 1); armed = true;
    await session.prompt(fourSlot ? "/afk-work on 5 4" : "/afk-work on 60 8"); await session.waitForIdle();
    if (missingCheckpoint) {
      assert.equal(confirmations, 0, "disabled checkpoint refuses before native consent");
      assert.equal(calls, 1, "no owned provider request dispatched");
      assert.equal(manager.getEntries().filter(e => e.customType === "aoe-afk-work-period-v1").length, 0);
      assert.match(notices.at(-1), /checkpoint/); assert.deepEqual(errors, []);
      console.log(JSON.stringify({ scenario: "missing-checkpoint", confirmations, ordinaryRequests: calls, ownedRequests: 0 })); return;
    }
    assert.equal(confirmations, 1, "actual registered interactive command required native confirmation");
    assert.equal(calls, finalCall, JSON.stringify({ notices, outcomes, status: fixture.api.status(fixture.r), records: manager.getEntries().filter(e => e.customType === "aoe-afk-work-period-v1").map(e => ({ action: e.data.action, status: e.data.status, reason: e.data.reason, requests: e.data.requests_used })), errors }));
    assert.equal(await readFile(join(workDir, fourSlot ? "one.txt" : "first.txt"), "utf8"), "one\n");
    assert.equal(await readFile(join(workDir, fourSlot ? "two.txt" : "second.txt"), "utf8"), "two\n");
    if (fourSlot) {
      assert.equal(await readFile(join(workDir, "validate.mjs"), "utf8"), FOUR_SLOT_VALIDATOR);
      assert.equal(await readFile(join(workDir, "AGENTS.md"), "utf8"), instructions);
      assert.deepEqual((await readdir(workDir)).sort(), ["AGENTS.md", "one.txt", "two.txt", "validate.mjs"]);
      const validation = session.messages.find(m => m.role === "toolResult" && m.toolName === "bash");
      assert.ok(validation?.content.some(b => b.type === "text" && b.text.includes("PASS: both canary files have exact contents; no extra files")), JSON.stringify(validation));
    }
    assert.equal(calls, finalCall, "one ordinary request plus only the admitted work requests; no reporting request");
    const records = sdk.SessionManager.open(manager.getSessionFile()).getEntries().filter(e => e.customType === "aoe-afk-work-period-v1");
    assert.equal(records[0].data.consent, "native-interactive-confirm-v1");
    assert.equal(records.at(-1).data.status, "completed"); assert.equal(records.at(-1).data.requests_used, finalCall - 1); assert.equal(records.at(-1).data.tools_used, fourSlot ? 2 : 3);
    assert.equal(records.at(-1).data.request_limit, fourSlot ? 4 : 8);
    assert.equal(records.at(-1).data.checkpoint_count, 2);
    assert.match(fixture.api.status(fixture.r), /completed/); assert.doesNotMatch(fixture.api.status(fixture.r), /unavailable|unverified/);
    const inspected = { version: 1, id: randomUUID(), binding, kind: "inspect", generation: null, period_id: null, issued_at_ms: Date.now(), expires_at_ms: Date.now() + 3000 };
    fixture.command = inspected; await fixture.r.refresh();
    assert.equal(fixture.replies.at(-1).reply.period.settlement_status, "proved", "actual completed-turn terminal ticket resolved successfully");
    assert.ok(session.messages.filter(m => m.role === "toolResult").every(m => !m.isError),
      JSON.stringify({ expected: "normal tools and checkpoints succeeded", failed: session.messages.filter(m => m.role === "toolResult" && m.isError) }));
    assert.equal(session.agent.signal?.aborted ?? false, false, "normal terminal ticket, not abort-as-success");
    assert.deepEqual(errors, []); assert.ok(contexts.slice(1).every(c => JSON.stringify(c).includes("AoE Session-bound AFK work period")));
    assert.equal(contexts.some(c => JSON.stringify(c).includes("[AoE bounded work continuation]")), early);
    assert.equal(consumedRefs.length, fourSlot ? 2 : 3); assert.ok(consumedRefs.every(ref => ref.includes("|fc_")));
    console.log(JSON.stringify({ scenario: fourSlot ? "visible-evidence-four-slot" : early ? "visible-evidence-early-stop" : "visible-evidence", modelVisibleRefs: consumedRefs,
      requests: records.at(-1).data.requests_used, requestLimit: records.at(-1).data.request_limit, tools: records.at(-1).data.tools_used,
      status: records.at(-1).data.status, terminalProof: fixture.replies.at(-1).reply.period.settlement_status,
      ...(fourSlot ? { command: FOUR_SLOT_COMMAND, validatorUnchanged: true, independentFileReadback: true, noReportingRequest: true } : {}) }));
    if (!early && !fourSlot) {
      const before = calls, file = manager.getSessionFile();
      await session.extensionRunner.emit({ type: "session_shutdown", reason: "reload" }); session.dispose();
      manager = sdk.SessionManager.open(file);
      const reopenedRuntime = createExtensionRuntime(); reopenedRuntime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
      loaded = await loadExtensions([extension], cwd, sdk.createEventBus(), reopenedRuntime); assert.deepEqual(loaded.errors, []);
      fixture.command = null; await bind();
      const queued = { role: "custom", customType: "original-queued-work", content: "preserve this exact object", display: true, timestamp: Date.now() };
      session.agent.followUp(queued);
      await session.sendCustomMessage({ customType: "automatic-after-reopen", content: "not new human authority", display: true }, { triggerTurn: true }); await session.waitForIdle();
      assert.equal(calls, before, "compatible restoration never reactivates the period or dispatches ordinary fallback");
      assert.ok(session.agent.peekQueuedMessages().includes(queued), "preserving veto retains the original native queued object");
      await session.prompt("Fresh ordinary task after returning"); await session.waitForIdle();
      assert.ok(calls > before, "genuine ordinary input can recover conversation use");
      const recovered = sdk.SessionManager.open(file).getEntries().filter(e => e.customType === "aoe-afk-work-period-v1").at(-1).data;
      assert.equal(recovered.status, "recovered"); assert.equal(recovered.requests_used, finalCall - 1);
      assert.equal(confirmations, 1, "ordinary recovery did not create another work period"); assert.deepEqual(errors, []);
    }
  } finally { session?.dispose(); delete globalThis[key]; await rm(cwd, { recursive: true, force: true }); }
}
test("actual pinned SDK fits exact writes, unchanged read-only validation and proved completion in four owned requests", { skip: !source, timeout: 30000 }, () => runNativeSdk(false, false, true));
test("actual pinned SDK refuses the old four-tool selector before UI consent or owned requests", { skip: !source, timeout: 30000 }, () => runNativeSdk(false, true));
for (const early of [false, true]) {
  test(`actual pinned SDK native consent drives multiple units${early ? " after an early plain-text stop" : ""} without an extra reporting request`, { skip: !source, timeout: 30000 }, () => runNativeSdk(early));
}
