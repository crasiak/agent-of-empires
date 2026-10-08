import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";

const core = process.env.AOE_PI_SOURCE_ROOT;
const packageRoot = process.env.AOE_ASK_USER_SOURCE_ROOT;
const load = path => import(pathToFileURL(join(core, path)));
const latch = () => { let resolve; const promise = new Promise(r => resolve = r); return { promise, resolve }; };
const until = async predicate => { const deadline = Date.now() + 5000; while (!predicate()) { assert.ok(Date.now() < deadline, "observable predicate deadline"); await new Promise(r => setImmediate(r)); } };

export async function runQuestionCase(scenario, host) {
  const sdk = await load("packages/coding-agent/src/index.ts");
  const { loadExtensions, createExtensionRuntime } = await load("packages/coding-agent/src/core/extensions/loader.ts");
  const { AssistantMessageEventStream } = await load("packages/ai/src/utils/event-stream.ts");
  const { Container, Text, isViewportTUI } = await load("packages/tui/src/index.ts");
  const { VirtualTerminal } = await load("packages/tui/test/virtual-terminal.ts");
  const { createInteractiveTui, InteractiveMode } = await load("packages/coding-agent/src/modes/interactive/interactive-mode.ts");
  const { KeybindingsManager } = await load("packages/coding-agent/src/core/keybindings.ts");
  const { initTheme } = await load("packages/coding-agent/src/modes/interactive/theme/theme.ts");
  initTheme("dark");
  const cwd = await mkdtemp(join(tmpdir(), "aoe-afk-question-"));
  let session, r, armed = false, calls = 0, emitted = 0, closed, generation, mixed;
  const openedEvents = [];
  const reused = scenario === "reused-id-siblings";
  const recoveryFailure = scenario.startsWith("restore-recovery-failed");
  let recoverFailed = recoveryFailure;
  const ops = [], errors = [], contexts = [], replies = [], endings = [], opened = latch(), intent = latch(), release = latch();
  let sessionManager = scenario === "memory" ? sdk.SessionManager.inMemory(cwd) : sdk.SessionManager.create(cwd, join(cwd, "sessions"));
  const binding = { instance_id: "test", profile: "default", native_id: sessionManager.getSessionId(), launch_id: randomUUID() };
  const grant = { version: 4, task: "independent scratch", scope: "read scratch.txt independently of the unanswered question", files: [{ path: "scratch.txt", capability: "read" }], requests: 1, question_deferrals: scenario === "zero" ? 0 : 1, settlement_nudges: 0, assurance: "stock-sdk-main-loop-reservations" };
  const view = { version: 4, window: randomUUID(), state: "pending", confirmed: true, grant, generation: "", reservations_used: 0, issued_at_ms: Date.now(), expires_at_ms: Date.now() + 60000 };
  const bootstrap = { version: 4, app_dir: cwd, aoe_bin: "/unused", binding };
  let guards = [], hostFailed = false, delayedReply;
  const request = async packet => {
    ops.push(packet);
    if (hostFailed) throw Error("host unavailable");
    if (packet.op === "question_recover" && recoverFailed) throw Error("transient recovery failure");
    let value;
    if (host) value = await host({ packet, generation, binding, grant });
    else if (packet.op === "poll") value = { delegation: armed ? { ...view } : null, question_guards: guards };
    else if (packet.op === "question_recover") { guards = guards.filter(g => g.window !== packet.window || g.identity.operation !== packet.identity.operation); }
    else if (packet.op === "runtime") {
      const c = packet.command;
      if (["stop", "settle"].includes(c.kind)) view.state = "ended";
      if (c.kind === "question_admit") {
        assert.equal(view.state, "pending"); guards = [{ window: view.window, identity: c.identity, binding, generation, emission_intent: false }]; value = { identity: c.identity };
      }
      if (c.kind === "question_intent") { guards = [{ window: view.window, identity: c.identity, binding, generation, emission_intent: true }]; value = { guarded_return: true }; }
      if (c.kind === "reserve") { assert.equal(view.state, "pending"); assert.equal(view.reservations_used, 0); view.reservations_used++; view.state = "owned"; guards = []; value = { grant, unanswered_gate: c.question }; }
      if (c.kind === "read") value = { content: null };
    }
    if (packet.op === "poll" && !armed) value.delegation = null;
    const c = packet.command;
    if (c?.kind === "question_admit" && scenario === "admission-failed") throw Error("admission acknowledgement lost");
    if (c?.kind === "question_intent") { intent.resolve(); if (["answer-first", "cancel-first", "off-intent", "stop-intent"].includes(scenario)) await release.promise; }
    if (c?.kind === "question_outcome" && scenario === "ledger-failed") { hostFailed = true; throw Error("outcome ledger unavailable"); }
    return value ?? {};
  };
  const bus = sdk.createEventBus();
  if (["lost-reply", "late-reply", "conflicting-reply", "missing-closed", "truncated", "stale-capability", "extra-pending"].includes(scenario) || scenario.startsWith("restore-")) {
    const emit = bus.emit.bind(bus);
    bus.emit = (channel, data) => {
      if (channel === "ask:defer:v1:closed" && scenario === "missing-closed") return;
      if (channel === "ask:defer:v1:reply" && data.method === "capabilities") {
        if (scenario === "truncated") data = { ...data, truncated: true };
        if (scenario === "stale-capability") data = { ...data, pending: data.pending.map(p => ({ ...p, interactionId: randomUUID() })) };
        if (scenario === "extra-pending") data = { ...data, pending: [...data.pending, ...data.pending] };
      }
      if (channel === "ask:defer:v1:reply" && data.method === "defer") {
        if (["lost-reply", "late-reply"].includes(scenario) || scenario.startsWith("restore-")) { delayedReply = () => emit(channel, data); return; }
        if (scenario === "conflicting-reply") { emit(channel, data); emit(channel, { ...data, status: "cleanup_failed" }); return; }
      }
      emit(channel, data);
    };
  }
  bus.on("ask:defer:v1:opened", e => { openedEvents.push(e); opened.resolve(e); });
  bus.on("ask:defer:v1:closed", e => closed = e);
  bus.on("ask:defer:v1:reply", e => replies.push(e));
  bus.on("ask:defer:v1:request", e => { if (e.method === "defer") {
    emitted++;
    if (scenario === "off-after-intent") view.state = "ended";
    if (scenario === "expiry-after-intent") view.expires_at_ms = Date.now();
    if (scenario === "human-input") void session.prompt("genuine queued human", { streamingBehavior: "followUp" });
  } });
  globalThis.__aoeQuestion = {
    bootstrap, capture(value) { r = value; },
    bridge(_bootstrap, gen) { generation = gen; view.generation = gen; return { request, async close() {} }; },
    end(event) { endings.push(event); },
    earlyInput(event) {
      if (event.text === "earlier transformed recovery") return { action: "transform", text: "changed before AoE" };
      if (event.text === "reverted recovery") return { action: "transform", text: "temporary recovery" };
      if (event.text === "failed recovery") throw Error("injected recovery input failure");
    },
    earlyRevert(event) { if (event.text === "temporary recovery") return { action: "transform", text: "reverted recovery" }; },
    input(event) {
      if (event.text === "handled recovery") return { action: "handled" };
      if (event.text === "transformed recovery") return { action: "transform", text: "not the submitted input" };
    },
  };
  const fixture = join(cwd, "extension.mjs");
  await writeFile(fixture, `import { registerAfk } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import { delegationRuntime } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
export default function(pi) {const f=globalThis.__aoeQuestion; pi.on("input",f.earlyInput); pi.on("input",f.earlyRevert); const d=delegationRuntime(pi); registerAfk(pi,{protocol:4,delegation:{...d,attach(r){d.attach(r);f.capture(r);}},bridge:f.bridge,schedule:()=>()=>{}}); pi.on("ui_prompt_end",f.end); pi.on("input",f.input);}`);
  let runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
  let loaded = await loadExtensions([fixture, join(packageRoot, "index.ts")], cwd, bus, runtime);
  assert.deepEqual(loaded.errors, []);
  const resourceLoader = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline questions", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, async reload() {} };
  const model = { id: "fake", name: "fake", provider: "afk-question", api: "afk-question", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000 };
  const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
  const ordinary = ["select", "zero", "control", "memory", "siblings", "missing-capability", "answer-first", "cancel-first", "admission-failed", "off-intent", "truncated", "stale-capability", "extra-pending", "marker-failed", "startup-snapshot", "reused-id-siblings", "missing-provenance", "rpc", "inline"].includes(scenario);
  const positive = ["input", "options", "fullscreen"].includes(scenario);
  modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, context, options) {
    assert.equal(options.signal.aborted, false); calls++; contexts.push(context.messages);
    assert.ok(calls <= (positive ? 2 : recoveryFailure ? 5 : 3), "unbudgeted provider leak");
    const args = ["options", "select", "inline"].includes(scenario) ? { ...(scenario === "inline" ? { displayMode: "inline" } : {}), question: "Human prerequisite unrelated to scratch?", options: [{ title: "Alpha" }, { title: "Beta" }] } : { question: "Human prerequisite unrelated to scratch?" };
    const tools = reused && calls === 2 ? [{ name: "ask_user", arguments: { question: "Changed reused question?" } }, { name: "ask_user", arguments: { question: "Unrelated sibling?" } }] : calls === 1 ? [{ name: "ask_user", arguments: args }, ...(scenario === "siblings" ? [{ name: "ask_user", arguments: args }] : [])]
      : positive && calls === 2 ? [{ name: "aoe_afk_read", arguments: { path: "scratch.txt" } }] : [];
    const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(), content: tools.length ? tools.map((t, i) => ({ type: "toolCall", id: reused && calls === 2 && i === 0 ? "q-1-0" : `q-${calls}-${i}`, ...t })) : [{ type: "text", text: "ordinary outcome" }], stopReason: tools.length ? "toolUse" : "stop", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    const stream = new AssistantMessageEventStream(); stream.push({ type: "done", reason: message.stopReason, message }); stream.end(); return stream;
  } });
  const ui = createInteractiveTui({ tuiMode: scenario === "fullscreen" ? "fullscreen" : "regular", showHardwareCursor: false, logDirectory: cwd, terminal: new VirtualTerminal() });
  ui.requestRender = () => {};
  const editor = Object.assign(new Text("", 0, 0), { getText: () => "", setText() {} });
  const editorContainer = new Container(); editorContainer.addChild(editor); ui.addChild(editorContainer); if (isViewportTUI(ui)) ui.setLayoutRoot(editorContainer); ui.setFocus(editor);
  const mode = Object.assign(Object.create(InteractiveMode.prototype), { ui, renderer: ui, editor, editorContainer, keybindings: new KeybindingsManager(), extensionDialogClosers: new Map(), extensionTerminalInputSubscriptions: new Set(), updateStatusLine() {} });
  const raw = mode.createExtensionUIContext(); raw.setStatus = () => {};
  if (scenario === "missing-capability") raw.targetedDialogCancellation = undefined;
  if (scenario === "select") raw.custom = async () => undefined;
  try {
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader, settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager, tools: ["ask_user", "aoe_afk_read"] }));
    if (scenario === "missing-provenance") {
      const create = session.extensionRunner.createContext.bind(session.extensionRunner);
      session.extensionRunner.createContext = () => { const ctx = create(); delete ctx.inputProvenance; return ctx; };
    }
    await session.bindExtensions({ uiContext: raw, mode: scenario === "rpc" ? "rpc" : "tui", onError: e => errors.push(e) });
    const running = session.prompt("ordinary task opens a question");
    const openedEvent = await opened.promise;
    await until(() => ui.getFocusedComponent() !== editor);
    assert.equal(calls, 1); assert.equal(emitted, 0);
    if (reused) {
      ui.getFocusedComponent().handleInput("ordinary first"); ui.getFocusedComponent().handleInput("\r");
      await until(() => openedEvents.length === 2 && ui.getFocusedComponent() !== editor);
      assert.equal(r.questionCall, undefined, "rejected batch retires prior call provenance");
      assert.equal(r.questionObserved, undefined, "reused ID plus siblings never adopts a fresh opened event");
    }
    if (scenario === "mixed") mixed = r.ctx.ui.input("Unrelated human UI");
    if (scenario === "startup-snapshot") await session.extensionRunner.emit({ type: "session_start" });
    if (scenario === "marker-failed") {
      const append = sessionManager.appendCustomEntry.bind(sessionManager);
      sessionManager.appendCustomEntry = (type, data) => { if (type === "aoe-afk-question-v1") throw Error("native persistence failure"); return append(type, data); };
    }
    armed = scenario !== "control";
    await r.refresh();
    if (["answer-first", "cancel-first", "off-intent", "stop-intent"].includes(scenario)) {
      await intent.promise;
      if (scenario === "off-intent") { view.state = "ended"; await r.refresh(); }
      if (scenario === "stop-intent") void session.abort();
      else {
        const component = ui.getFocusedComponent();
        if (scenario !== "cancel-first") component.handleInput("human");
        component.handleInput(scenario === "cancel-first" ? "\x1b" : "\r");
      }
      release.resolve();
    }
    if (ordinary && !["answer-first", "cancel-first", "off-intent"].includes(scenario)) {
      if (r.questionTask) await r.questionTask;
      assert.equal(emitted, 0, "ineligible grant cannot mutate UI");
      if (scenario === "admission-failed") assert.equal(r.parked, true);
      ui.getFocusedComponent().handleInput("human"); ui.getFocusedComponent().handleInput("\r");
      if (scenario === "siblings" || reused) { await until(() => ui.getFocusedComponent() !== editor); ui.getFocusedComponent().handleInput("second"); ui.getFocusedComponent().handleInput("\r"); }
    }
    if (scenario === "mixed") {
      await until(() => closed?.state === "deferred");
      assert.notEqual(ui.getFocusedComponent(), editor, "unrelated replacement survives owned dismissal");
      ui.getFocusedComponent().handleInput("human"); ui.getFocusedComponent().handleInput("\r"); await mixed;
    }
    await running; await session.waitForIdle();
    assert.deepEqual(errors, [], JSON.stringify(errors));
    const results = session.messages.filter(m => m.role === "toolResult" && m.toolName === "ask_user");
    assert.equal(results[0].isError, false);
    if (positive || ["mixed", "lost-reply", "late-reply", "conflicting-reply", "ledger-failed", "missing-closed", "off-after-intent", "expiry-after-intent", "human-input"].includes(scenario) || scenario.startsWith("restore-")) {
      assert.equal(emitted, 1); assert.equal(results[0].details.reason, "deferred_by_afk"); assert.equal(results[0].details.response, null);
      if (scenario !== "missing-closed") assert.equal(closed.state, "deferred"); assert.equal(calls, positive ? 2 : 1, `${scenario}: ${r.reason}`);
      assert.equal(ops.filter(p => p.command?.kind === "reserve").length, positive ? 2 : 0);
      if (positive) { assert.ok(JSON.stringify(contexts[1]).includes("unanswered_gate")); assert.ok(session.messages.some(m => m.role === "toolResult" && m.toolName === "aoe_afk_read" && !m.isError)); }
    } else if (scenario === "stop-intent") assert.equal(calls, 1);
    else { assert.equal(calls, reused ? 3 : 2, `${scenario}: ordinary winner preserved, ${r.reason}`); assert.equal(results[0].details.reason, undefined); }
    assert.equal(endings[0]?.owner?.id, scenario === "mixed" ? undefined : openedEvent.interactionId);
    if (scenario === "late-reply") {
      delayedReply(); await r.refresh();
      await session.sendCustomMessage({ customType: "late-work", content: "retained late work", display: true }, { triggerTurn: true });
      await session.waitForIdle(); assert.equal(calls, 1, "late acceptance cannot release guarded return");
    }
    if (scenario === "human-input") assert.ok([...session.messages, ...session.agent.peekQueuedMessages()].some(m => m.role === "user" && JSON.stringify(m.content).includes("genuine queued human")), "original queued human retained");
    if (scenario === "admission-failed") {
      assert.equal(r.questionGuards[0].emission_intent, false);
      await session.prompt("ordinary recovery of admission-only receipt"); await session.waitForIdle();
      assert.equal(calls, 3); assert.equal(emitted, 0); assert.equal(guards.length, 0);
      assert.ok(!sessionManager.getEntries().some(e => e.customType === "aoe-afk-question-v1"), "admission-only recovery needs no native intent marker");
    }
    if (scenario.startsWith("restore-")) {
      const nativeFile = sessionManager.getSessionFile();
      assert.ok(nativeFile);
      const operation = r.questionAttempt.identity.operation;
      const oldGeneration = r.generation;
      await session.extensionRunner.emit({ type: "session_shutdown", reason: "reload" }); session.dispose();
      sessionManager = sdk.SessionManager.open(nativeFile);
      if (scenario === "restore-conflict") {
        const old = sessionManager.getEntries().find(e => e.customType === "aoe-afk-question-v1").data;
        sessionManager.appendCustomEntry("aoe-afk-question-v1", { ...old, window: randomUUID() });
      }
      runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
      loaded = await loadExtensions([fixture, join(packageRoot, "index.ts")], cwd, bus, runtime);
      assert.deepEqual(loaded.errors, []);
      if (scenario === "restore-corrupt" && host) await host({ packet: { fixture: "corrupt" }, generation, binding, grant });
      else if (["restore-unavailable", "restore-corrupt"].includes(scenario)) hostFailed = true;
      ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader,
        settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager, tools: ["ask_user", "aoe_afk_read"] }));
      if (["restore-unsupported", "restore-no-provenance"].includes(scenario)) {
        const create = session.extensionRunner.createContext.bind(session.extensionRunner);
        session.extensionRunner.createContext = () => { const ctx = create(); if (scenario === "restore-unsupported") delete ctx.abortPreservingQueue; else delete ctx.inputProvenance; return ctx; };
      }
      mode.runtimeHost = { session }; mode.compactionQueuedMessages = []; mode.updatePendingMessagesDisplay = () => {};
      await session.bindExtensions({ uiContext: raw, mode: "tui", onError: e => errors.push(e), abortHandler: () => mode.restoreQueuedMessagesToEditor({ abort: true }) });
      assert.notEqual(r.generation, oldGeneration);
      assert.ok(r.restoredGuards.has(operation), "guard restored from actual native file, not process memory");
      const original = { role: "custom", customType: "original-queue", content: "original queued object", display: true, timestamp: Date.now() };
      session.agent.followUp(original);
      await session.sendCustomMessage({ customType: "automatic-return", content: "automatic continuation", display: true }, { triggerTurn: true });
      await session.waitForIdle();
      assert.equal(calls, 1); assert.equal(emitted, 1);
      if (scenario === "restore-unsupported") {
        assert.match(r.questionWarning, /NOT safe against native custom-queue loss/);
        assert.ok(!session.agent.peekQueuedMessages().includes(original), "limitation evidence: native ordinary abort discards custom queue");
        assert.ok(r.restoredGuards.has(operation), "containment never resolves guard");
      } else {
        assert.ok(session.agent.peekQueuedMessages().includes(original), "preserving abort retains exact queue object");
        await session.prompt("extension is not recovery", { source: "extension" }); await session.waitForIdle();
        assert.equal(calls, 1); assert.ok(r.restoredGuards.has(operation));
        await session.prompt("handled recovery"); await session.waitForIdle(); assert.equal(calls, 1);
        await session.prompt("handled recovery", { source: "extension" }); await session.waitForIdle(); assert.equal(calls, 1);
        await session.prompt("transformed recovery"); await session.waitForIdle(); assert.equal(calls, 1);
        await session.prompt("earlier transformed recovery"); await session.waitForIdle(); assert.equal(calls, 1);
        await session.prompt("reverted recovery"); await session.waitForIdle(); assert.equal(calls, 1);
        await session.prompt("failed recovery"); await session.waitForIdle(); assert.equal(calls, 1);
        const failure = errors.pop(); assert.equal(failure?.event, "input"); assert.match(failure?.error ?? "", /injected recovery input failure/);
        if (!["restore-conflict", "restore-no-provenance"].includes(scenario)) {
          await session.prompt("fresh ordinary recovery"); await session.waitForIdle();
          assert.ok(calls > 1, `fresh ordinary recovery should dispatch: ${r.reason}`);
          assert.equal(r.owned, false); assert.equal(emitted, 1);
          assert.equal(ops.filter(p => p.command?.kind === "reserve").length, 0);
          assert.ok(session.messages.some(m => m.role === "user" && JSON.stringify(m.content).includes("fresh ordinary recovery")));
          assert.ok(session.messages.some(m => m.customType === "original-queue" && m.content === original.content));
          if (recoveryFailure) {
            assert.equal(ops.filter(p => p.op === "question_recover").length, 1);
            assert.equal(guards.length, 1, "failed host recovery remains authoritative");
            recoverFailed = false;
            if (scenario === "restore-recovery-failed-reopen") {
              await session.extensionRunner.emit({ type: "session_shutdown", reason: "reload" }); session.dispose();
              sessionManager = sdk.SessionManager.open(nativeFile);
              runtime = createExtensionRuntime(); runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
              loaded = await loadExtensions([fixture, join(packageRoot, "index.ts")], cwd, bus, runtime);
              assert.deepEqual(loaded.errors, []);
              ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader,
                settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager, tools: ["ask_user", "aoe_afk_read"] }));
              mode.runtimeHost = { session };
              await session.bindExtensions({ uiContext: raw, mode: "tui", onError: e => errors.push(e), abortHandler: () => mode.restoreQueuedMessagesToEditor({ abort: true }) });
            }
            await r.refresh();
            assert.equal(r.questionGuards.length, 1, "host receipt remains discoverable after local recovery/reload");
            assert.ok(!r.markerInvalid);
            const before = calls;
            await session.sendCustomMessage({ customType: "ordinary-after-recovery", content: "already recovered ordinary work", display: true }, { triggerTurn: true });
            await session.waitForIdle();
            assert.equal(calls, before + 1, "host ambiguity does not reinstate native quarantine");
            assert.equal(ops.filter(p => p.op === "question_recover").length, 1, "no background recovery retry");
            await session.prompt("fresh exact host recovery retry"); await session.waitForIdle();
            assert.equal(calls, before + 2);
            const recoveries = ops.filter(p => p.op === "question_recover");
            assert.equal(recoveries.length, 2);
            assert.deepEqual(recoveries[0], recoveries[1], "fresh recovery targets the same exact receipt");
            assert.equal(guards.length, 0);
            const markers = sessionManager.getEntries().filter(e => e.customType === "aoe-afk-question-v1");
            assert.deepEqual(markers.map(e => e.data.phase), ["intent", "recovered"], "no duplicate resolution marker");
            assert.equal(emitted, 1);
            assert.equal(ops.filter(p => p.command?.kind === "reserve").length, 0);
          }
        }
      }
      assert.deepEqual(errors, []);
    }
    const result = { scenario, calls, emitted, reservation_attempts: ops.filter(p => p.command?.kind === "reserve").length, replies: replies.filter(e => e.method === "defer").map(e => e.status), native: true };
    return result;
  } finally { await session?.extensionRunner.emit({ type: "session_shutdown", reason: "exit" }); session?.dispose(); delete globalThis.__aoeQuestion; await rm(cwd, { recursive: true, force: true }); }
}

if (process.argv.includes("--host")) {
  setTimeout(() => process.exit(1), 30000).unref();
  const { createInterface } = await import("node:readline");
  const lines = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
  const result = await runQuestionCase(process.argv[process.argv.indexOf("--host") + 1], async value => {
    process.stdout.write(JSON.stringify(value) + "\n"); const line = await lines.next(); assert.equal(line.done, false);
    const result = JSON.parse(line.value); if (!result.ok) throw Error(result.error); return result.value;
  });
  process.stdout.write(JSON.stringify({ result }) + "\n"); process.exit(0);
} else test("actual source SDK/package/native UI controlled question return", { skip: !core || !packageRoot, timeout: 60000 }, async () => {
  for (const scenario of ["input", "options", "select", "fullscreen", "answer-first", "cancel-first", "zero", "control", "missing-capability", "memory", "siblings", "mixed", "admission-failed", "off-intent", "stop-intent", "lost-reply", "ledger-failed", "off-after-intent", "expiry-after-intent", "late-reply", "conflicting-reply", "missing-closed", "truncated", "stale-capability", "extra-pending", "marker-failed", "startup-snapshot", "human-input", "restore-readable", "restore-unavailable", "restore-corrupt", "restore-conflict", "restore-unsupported", "restore-no-provenance", "restore-recovery-failed", "restore-recovery-failed-reopen", "reused-id-siblings", "missing-provenance", "rpc", "inline"]) console.log(JSON.stringify(await runQuestionCase(scenario)));
});
