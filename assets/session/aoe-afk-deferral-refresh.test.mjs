import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { mkdir, mkdtemp, writeFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const core = process.env.AOE_PI_SOURCE_ROOT;
const latch = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const load = path => import(pathToFileURL(join(core, path)));
const channel = "ask:defer:v1:";
const wait = (promise, signal) => new Promise((resolve, reject) => {
  const finish = (callback, value) => { signal.removeEventListener("abort", abort); callback(value); };
  const abort = () => finish(reject, signal.reason);
  if (signal.aborted) abort();
  else signal.addEventListener("abort", abort, { once: true });
  Promise.resolve(promise).then(value => finish(resolve, value), error => finish(reject, error));
});

async function runCase(scenario, testSignal, fault) {
  const sdk = await load("packages/coding-agent/src/index.ts");
  const { loadExtensions, createExtensionRuntime } = await load("packages/coding-agent/src/core/extensions/loader.ts");
  const { AssistantMessageEventStream } = await load("packages/ai/src/utils/event-stream.ts");
  assert.equal(sdk.VERSION, "0.87.1");
  const target = resolve("target/deferral-refresh");
  await mkdir(target, { recursive: true });
  const cwd = await mkdtemp(join(target, "case-"));
  const fixtureFailure = new AbortController();
  const signal = AbortSignal.any([testSignal, fixtureFailure.signal, AbortSignal.timeout(10000)]);
  const opened = latch(), intent = latch(), releaseIntent = latch(), refreshEntered = latch(), releaseRefresh = latch(), answer = latch();
  const recordReached = latch(), releaseRecord = latch();
  const observations = [], packets = [], statuses = [], errors = [], records = [], bridgeErrors = [];
  const peerErrors = [], providerErrors = [], peerTasks = [], teardownErrors = [], failures = [];
  let snapshot, audit, failedCallbacks = 0, faultInjected = false;
  const captureFixtureError = (list, error) => { list.push(error); fixtureFailure.abort(error); };
  const captureSync = (list, handler) => (...args) => {
    try { return handler(...args); }
    catch (error) { captureFixtureError(list, error); throw error; }
  };
  let session, running, r, armed = false, calls = 0, defers = 0, questionOpen = false, outcome = null;
  let guards = [], failureInjected = false, stopped = false, questionTaskSettled = false, cleaningUp = false;
  const rejectRefresh = scenario.startsWith("refresh-rejected");
  const rejectStop = scenario === "refresh-rejected-stop-rejected";
  const refreshError = Object.assign(Error("injected post-question_intent poll failure"), { origin: "fixture:post-intent-poll" });
  const stopError = Object.assign(Error("injected cleanup stop failure"), { origin: "fixture:cleanup-stop" });
  try {
    const manager = sdk.SessionManager.create(cwd, join(cwd, "sessions"));
    const binding = { instance_id: "deferral-repro", profile: "isolated", native_id: manager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 6, app_dir: cwd, aoe_bin: "/unused/offline-bridge", binding };
    const model = { id: "fixture", name: "fixture", provider: "deferral-fixture", api: "deferral-fixture", baseUrl: "http://invalid.invalid", reasoning: false, input: ["text"], cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 1 }, contextWindow: 100000, maxTokens: 1000 };
    const grant = { version: 4, task: "independent scratch", scope: "no consequential action or human answer", files: [{ path: "scratch.txt", capability: "create" }], requests: 2, question_deferrals: 1, settlement_nudges: 0, assurance: "stock-sdk-main-loop-reservations" };
    const guard = { version: 1, basis: "observed-native-entries", tokens: null, usd: { currency: "USD", basis: "operator-fixed-rates", limit_nano: 100, quotes: [{ model: { provider: model.provider, model: model.id }, input: 1, output: 1, cache_read: 1, cache_write: 1, cache_write_1h: 1 }] } };
    const view = { version: 4, binding, generation: "", window: randomUUID(), state: "pending", confirmed: false, issued_at_ms: Date.now(), expires_at_ms: Date.now() + 60000, grant, grant_hash: "b".repeat(64), reservations_used: 0, usage_policy_hash: "a".repeat(64), shared_usage_guard: null, usage: { guard, blocked: null, gap: null, ordinary_recovered: false } };
    const challenge = { id: randomUUID(), after: { count: 0, hash: createHash("sha256").update("aoe-afk-usage-v1").digest("hex") } };
    const question = { version: 1, toolCallId: "work-c-1", interactionId: randomUUID(), kind: "input", supported: true, uiPromptAttribution: true, state: "pending" };
    question.owner = { namespace: "pi-ask-user", id: question.interactionId };
    const decision = { id: randomUUID(), question: "Should a consequential change proceed without approval?", alternatives: [{ name: "defer", pros: "preserve approval", cons: "wait", scores: [5] }, { name: "proceed", pros: "progress", cons: "missing approval", scores: [1] }], criteria: [{ name: "approval safety", weight: 5 }], evidence: ["required approval absent"], assumptions: [], uncertainties: [], recommendation: "defer", rationale: "Required approval is absent", rollback: "No action authorized", reversible: true, risk: "high", human_required: true, depends_on: [], action: null };
    const nativeMarkers = type => sdk.SessionManager.open(manager.getSessionFile()).getEntries().filter(e => e.customType === type).map(e => e.data);
    const observe = event => observations.push(event);
    const host = async packet => {
      packets.push(structuredClone(packet));
      const c = packet.command;
      if (packet.op === "poll") {
        if (armed) assert.equal(packet.usage.gap, null, "guarded observations must use persisted native history");
        if (guards[0]?.emission_intent && !failureInjected) {
          failureInjected = true;
          assert.equal(packet.usage.report.entries.length, manager.getEntries().length, "post-intent poll includes the native question marker");
          observe("post-intent-poll-entered"); refreshEntered.resolve();
          await wait(releaseRefresh.promise, signal);
          if (rejectRefresh) throw refreshError;
          observe("post-intent-poll-replied");
        }
        return { policy: null, usage_report: { fresh: true, challenge }, delegation: armed ? structuredClone(view) : null, question_guards: structuredClone(guards), runtime_probe: armed ? { version: 6, binding, generation: view.generation, challenge: "runtime-probe", window: view.window, grant_hash: view.grant_hash, budgeted: false, usage_policy_hash: view.usage_policy_hash } : null };
      }
      if (packet.op === "runtime_ack") {
        assert.equal(packet.ack.usage_coverage, "native-entries-terminal-v2");
        assert.equal(nativeMarkers("aoe-afk-usage-v1")[0].phase, "required");
        view.confirmed = true;
        return {};
      }
      assert.equal(packet.op, "runtime", "unexpected fixture bridge operation");
      if (c.kind === "question_admit") {
        assert.equal(view.state, "pending");
        guards = [{ window: view.window, binding, generation: view.generation, identity: c.identity, emission_intent: false }];
        return { identity: c.identity };
      }
      if (c.kind === "question_intent") {
        assert.deepEqual(c.identity, guards[0].identity);
        assert.equal(nativeMarkers("aoe-afk-question-v1")[0].phase, "intent");
        guards[0].emission_intent = true;
        observe("host-question-intent"); intent.resolve();
        await wait(releaseIntent.promise, signal);
        return { guarded_return: true };
      }
      if (c.kind === "question_outcome") {
        assert.equal(defers, 1); assert.equal(c.outcome, "accepted");
        outcome = c.outcome; observe("host-question-outcome");
        return { identity: c.identity, outcome };
      }
      if (c.kind === "reserve") {
        assert.equal(outcome, "accepted", "provider reservation requires persisted accepted outcome");
        assert.deepEqual(c.question, guards[0].identity);
        assert.equal(view.reservations_used, 0);
        view.reservations_used++; view.state = "owned"; guards = [];
        observe("host-reserve");
        return { grant, unanswered_gate: c.question };
      }
      if (c.kind === "record") {
        assert.equal(calls, 2); assert.equal(c.decision.risk, "high"); assert.equal(c.decision.human_required, true); assert.equal(c.decision.action, null);
        records.push(c.decision);
        observe("host-record-human-required"); recordReached.resolve();
        await wait(releaseRecord.promise, signal);
        return { disposition: "human_required", decision: c.decision.id };
      }
      if (c.kind === "stop" && cleaningUp && fault?.kind === "late-bridge" && !faultInjected) { faultInjected = true; throw fault.error; }
      if (c.kind === "stop" && rejectStop && !cleaningUp) { observe("host-stop-rejected"); throw stopError; }
      if (["stop", "settle"].includes(c.kind)) { stopped = true; view.state = "ended"; observe(`host-${c.kind}`); return {}; }
      assert.fail(`unexpected fixture command: ${c.kind}`);
    };
    const bus = sdk.createEventBus();
    // The question peer is a protocol double, not native UI qualification.
    bus.on(channel + "request", captureSync(peerErrors, request => {
      if (request.method === "capabilities") {
        bus.emit(channel + "reply", { version: 1, requestId: request.requestId, method: "capabilities", status: "capabilities", truncated: false, pending: questionOpen ? [question] : [] });
        if (fault?.kind === "peer" && !faultInjected) { faultInjected = true; throw fault.error; }
      } else if (request.method === "defer") {
        defers++; observe("defer-emitted");
        assert.equal(r.parked, false); assert.equal(r.usageDenied, undefined); assert.equal(r.view.confirmed, true);
        assert.ok(observations.includes("post-intent-poll-replied"), "real readiness must follow released refresh");
        assert.equal(request.toolCallId, question.toolCallId); assert.equal(request.interactionId, question.interactionId);
        questionOpen = false;
        bus.emit(channel + "closed", { ...question, state: "deferred" });
        bus.emit(channel + "reply", { ...request, status: "accepted", reason: "deferred_by_afk" });
        peerTasks.push(session.extensionRunner.emit({ type: "ui_prompt_end", owner: question.owner })
          .then(() => answer.resolve({ toolCallId: question.toolCallId, interactionId: question.interactionId, status: "no_human_answer", reason: "deferred_by_afk", response: null, cancelled: true }))
          .catch(error => captureFixtureError(peerErrors, error)));
      } else assert.fail(`unexpected question method: ${request.method}`);
    }));
    globalThis.__aoeDeferralRefresh = {
      bootstrap,
      capture(value) { r = value; },
      failed(runtime, ...args) {
        failedCallbacks++;
        observe({ event: "failed-callback", original_error_received: args.includes(refreshError), argument_count: 1 + args.length });
      },
      bridge(_bootstrap, generation) {
        view.generation = generation;
        return {
          async request(packet) {
            try { return await host(packet); }
            catch (error) {
              bridgeErrors.push(error);
              observe({ event: "bridge-error", origin: error.origin, message: error.message, stack: error.stack });
              throw error;
            }
          },
          async close() {},
        };
      },
      shutdown() {
        if (fault?.kind === "late-sdk") { faultInjected = true; throw fault.error; }
      },
      async ask(_id, _args, toolSignal) {
        try {
          assert.equal(toolSignal.aborted, false);
          questionOpen = true; bus.emit(channel + "opened", question); opened.resolve();
          const details = await wait(answer.promise, signal);
          return { content: [{ type: "text", text: "No completed human answer was submitted" }], details };
        } catch (error) { captureFixtureError(peerErrors, error); throw error; }
      },
    };
    const extension = join(cwd, "extension.mjs");
    await writeFile(extension, `import { registerAfk } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import { delegationRuntime } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
import { Type } from "@sinclair/typebox";
export default function(pi) {
  const f = globalThis.__aoeDeferralRefresh, d = delegationRuntime(pi);
  registerAfk(pi, { protocol: 6, delegation: { ...d, attach(r) { d.attach(r); f.capture(r); }, failed(...args) { f.failed(...args); return d.failed(...args); } }, bridge: f.bridge, schedule: () => () => {} });
  pi.registerTool({ name: "ask_user", label: "Question protocol double", description: "Offline attributed question", parameters: Type.Object({ question: Type.String() }), execute: f.ask });
  pi.on("session_shutdown", f.shutdown);
}
`);
    const runtime = createExtensionRuntime();
    runtime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    const loaded = await loadExtensions([extension], cwd, bus, runtime);
    assert.deepEqual(loaded.errors, []);
    const resourceLoader = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }), getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline deferral refresh reproduction", getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, async reload() {} };
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple: captureSync(providerErrors, (_model, context, options) => {
      assert.equal(options.signal.aborted, false); assert.ok(++calls <= 2, "unexpected third provider dispatch");
      if (calls === 2 && fault?.kind === "provider") { faultInjected = true; throw fault.error; }
      if (calls === 2) { assert.equal(outcome, "accepted"); assert.ok(JSON.stringify(context.messages).includes("No completed human answer")); }
      observe(`provider-${calls}`);
      const tool = calls === 1 ? { name: "ask_user", arguments: { question: "USAGE QUESTION work-c?" } } : { name: "aoe_afk_record", arguments: { decision } };
      const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(), content: [{ type: "toolCall", id: `work-c-${calls}`, ...tool }], stopReason: "toolUse", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0.000001, output: 0.000001, cacheRead: 0, cacheWrite: 0, total: 0.000002 } } };
      const stream = new AssistantMessageEventStream(); stream.push({ type: "done", reason: "toolUse", message }); stream.end(); return stream;
    }) });
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader, settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }), sessionManager: manager, tools: ["ask_user", "aoe_afk_record"] }));
    if (fault?.kind === "dispose") {
      const dispose = session.dispose.bind(session);
      session.dispose = () => { dispose(); faultInjected = true; throw fault.error; };
    }
    await session.bindExtensions({ mode: "tui", onError: error => errors.push(error), uiContext: { uiPromptOwnership: "exclusive-v1", targetedDialogCancellation: ["input"], setStatus(_key, text) { statuses.push(text); }, getEditorText: () => "" } });
    running = session.prompt("Open the independent question and wait. No answer or approval supplied.");
    running.catch(error => captureFixtureError(providerErrors, error));
    await wait(opened.promise, signal);
    assert.equal(calls, 1); assert.ok(r.questionObserved); assert.equal(r.questionAttempt, undefined);
    armed = true;
    await wait(r.refresh(), signal); // Persist required usage witness and acknowledge the guarded window.
    assert.equal(view.confirmed, true); assert.equal(r.usageWitness.failure, undefined);
    await wait(r.refresh(), signal); // The confirmed view starts the actual question attempt.
    await wait(intent.promise, signal);
    assert.ok(!r.refreshing, "admission refresh has completed before releasing host intent");
    assert.equal(r.usageWitness.pending, 1); assert.equal(calls, 1); assert.equal(defers, 0);
    r.questionTask.then(() => { questionTaskSettled = true; }, error => captureFixtureError(peerErrors, error));
    releaseIntent.resolve();
    await wait(refreshEntered.promise, signal);
    assert.equal(questionOpen, true); assert.equal(defers, 0); assert.equal(outcome, null);
    const awaitedRefresh = r.refreshing;
    assert.ok(awaitedRefresh, "attempt is waiting on real registerAfk refresh");
    if (fault?.kind === "blocked-barrier") {
      faultInjected = true; fault.entered.resolve();
      await wait(awaitedRefresh, signal);
      assert.fail("held refresh unexpectedly completed before cancellation");
    }
    if (scenario === "held-refresh") {
      // Drain a full event-loop turn while the causal response gate remains held.
      await wait(new Promise(resolve => setImmediate(resolve)), signal);
      assert.equal(questionTaskSettled, false); assert.equal(r.usageDenied, undefined);
      assert.equal(stopped, false); assert.equal(view.state, "pending"); assert.equal(calls, 1);
      observe("held-response-still-pending-without-denial");
    }
    releaseRefresh.resolve();
    await wait(awaitedRefresh, signal).then(
      () => observe({ event: "refresh-caller", completion: "resolved", original_error_received: false }),
      error => { observe({ event: "refresh-caller", completion: "rejected", original_error_received: error === refreshError }); throw error; },
    );
    await wait(r.questionTask, signal);
    if (rejectRefresh) {
      assert.equal(questionOpen, true); assert.equal(calls, 1); assert.equal(defers, 0); assert.equal(outcome, null);
      assert.equal(r.usageDenied, "usage_unavailable"); assert.equal(r.signal.aborted, false);
      assert.equal(stopped, !rejectStop);
      assert.equal(view.state, rejectStop ? "pending" : "ended", "canonical pending requires cleanup failure in this injection");
      assert.equal(view.usage.blocked, null); assert.equal(view.usage.gap, null); assert.equal(view.usage.ordinary_recovered, false);
      assert.equal(view.reservations_used, 0); assert.equal(r.usageWitness.pending, 1);
      assert.deepEqual(nativeMarkers("aoe-afk-usage-v1").map(m => m.phase), ["required"]);
      assert.deepEqual(nativeMarkers("aoe-afk-question-v1").map(m => m.phase), ["intent"]);
      assert.deepEqual(nativeMarkers("aoe-afk-question-v1")[0].identity, guards[0].identity);
      assert.equal(guards[0].emission_intent, true);
      if (rejectStop) { assert.equal(r.failure, true); assert.equal(r.stoppedWindow, undefined); }
      assert.match(statuses.at(-1), /usage_unavailable/);
      assert.equal(records.length, 0);
      observe(rejectStop ? "refresh-and-stop-rejected-pending-signature" : "refresh-error-swallowed-question-stop-succeeded");
    } else {
      await wait(recordReached.promise, signal);
      assert.equal(calls, 2); assert.equal(defers, 1); assert.equal(outcome, "accepted");
      assert.equal(records.length, 1); assert.equal(questionOpen, false);
      assert.deepEqual(nativeMarkers("aoe-afk-question-v1").map(m => m.phase), ["intent", "observed"]);
      assert.ok(observations.indexOf("host-question-outcome") < observations.indexOf("provider-2"));
    }
    assert.equal(existsSync(join(cwd, "scratch.txt")), false);
    assert.ok(!packets.some(p => p.command?.kind === "apply"));
    snapshot = { scenario, calls, defers, outcome, host_state: view.state, blocked: view.usage.blocked, gap: view.usage.gap,
      record_submissions: records.length, reservations: view.reservations_used, question_open: questionOpen,
      native_denial: r.usageDenied ?? null, native_usage_pending: r.usageWitness.pending,
      original_error_in_status: statuses.some(status => status?.includes(refreshError.message)),
      usage_markers: nativeMarkers("aoe-afk-usage-v1").map(m => m.phase),
      question_markers: nativeMarkers("aoe-afk-question-v1").map(m => m.phase),
      observations: structuredClone(observations) };
  } catch (error) { failures.push(error); }
  finally {
    cleaningUp = true;
    const cleanupSignal = AbortSignal.timeout(5000);
    const cleanup = async action => {
      try { await wait(action(), cleanupSignal); }
      catch (error) { teardownErrors.push(error); }
    };
    try {
      let aborting;
      try { aborting = session?.abort(); }
      catch (error) { teardownErrors.push(error); }
      finally {
        releaseIntent.resolve(); releaseRefresh.resolve(); releaseRecord.resolve();
        answer.resolve({ cancelled: true, response: null });
      }
      await cleanup(() => aborting);
      await cleanup(() => running);
      await cleanup(() => r?.questionTask);
      await cleanup(() => Promise.all(peerTasks));
      await cleanup(() => r?.refreshing);
      await cleanup(() => r?.stopping);
      await cleanup(() => session?.extensionRunner.emit({ type: "session_shutdown", reason: "exit" }));
    } finally {
      try { session?.dispose(); }
      catch (error) { teardownErrors.push(error); }
      finally {
        delete globalThis.__aoeDeferralRefresh;
        try { await rm(cwd, { recursive: true, force: true }); }
        catch (error) { teardownErrors.push(error); }
      }
    }
    audit = { bridgeErrors, sdkErrors: errors, peerErrors, providerErrors, teardownErrors, failedCallbacks,
      faultInjected, cwd, fixtureRemoved: !existsSync(cwd), globalRemoved: !("__aoeDeferralRefresh" in globalThis) };
    const check = assertion => { try { assertion(); } catch (error) { failures.push(error); } };
    const expectedErrors = rejectStop ? [refreshError, stopError] : rejectRefresh ? [refreshError] : [];
    check(() => assert.equal(bridgeErrors.length, expectedErrors.length, "unexpected bridge error, including teardown"));
    expectedErrors.forEach((error, index) => check(() => assert.equal(bridgeErrors[index], error, "retain exact original injected Error object")));
    check(() => assert.equal(failedCallbacks, rejectRefresh ? 1 : 0, "unexpected refresh failure callback count"));
    check(() => assert.deepEqual(errors, [], "unexpected SDK error, including teardown"));
    check(() => assert.deepEqual(peerErrors, [], "unexpected question-peer fixture error"));
    check(() => assert.deepEqual(providerErrors, [], "unexpected provider fixture error"));
    failures.push(...teardownErrors);
    check(() => assert.equal(audit.fixtureRemoved, true));
    check(() => assert.equal(audit.globalRemoved, true));
  }
  if (failures.length) throw Object.assign(new AggregateError(failures, "deferral harness failed"), { snapshot, audit });
  return { ...snapshot, final_audit: { bridge_errors: bridgeErrors.length, sdk_errors: errors.length,
    peer_errors: peerErrors.length, provider_errors: providerErrors.length, teardown_errors: teardownErrors.length,
    failed_callbacks: failedCallbacks, fixture_removed: audit.fixtureRemoved, global_removed: audit.globalRemoved } };
}

test("guarded question intent uses the actual refresh/readiness/defer chain", { skip: !core, timeout: 30000 }, async t => {
  for (const scenario of ["successful-refresh", "refresh-rejected", "held-refresh", "refresh-rejected-stop-rejected"]) {
    await t.test(scenario, async t => t.diagnostic(JSON.stringify(await runCase(scenario, t.signal))));
  }
});

test("harness rejects swallowed faults and unwinds blocked barriers", { skip: !core, timeout: 30000 }, async t => {
  for (const kind of ["late-bridge", "late-sdk", "peer", "provider", "blocked-barrier", "dispose"]) {
    await t.test(kind, async t => {
      const error = Error(`harness self-check: ${kind}`);
      const fault = { kind, error, entered: latch() }, cancel = new AbortController();
      const consoleErrors = [];
      if (kind === "peer") t.mock.method(console, "error", (...args) => consoleErrors.push(args));
      const settled = runCase("successful-refresh", AbortSignal.any([t.signal, cancel.signal]), fault)
        .then(value => ({ value }), failure => ({ failure }));
      if (kind === "blocked-barrier") {
        await wait(fault.entered.promise, AbortSignal.any([t.signal, AbortSignal.timeout(10000)]));
        cancel.abort(error);
      }
      const { failure } = await settled;
      assert.ok(failure instanceof AggregateError, "fault must fail the harness, not just emit a diagnostic");
      const { audit, snapshot } = failure;
      assert.equal(audit.faultInjected, true, "self-check reached its actual call-site boundary");
      assert.equal(audit.fixtureRemoved, true); assert.equal(existsSync(audit.cwd), false);
      assert.equal(audit.globalRemoved, true); assert.equal("__aoeDeferralRefresh" in globalThis, false);
      if (["late-bridge", "late-sdk", "dispose"].includes(kind)) {
        assert.equal(snapshot.record_submissions, 1, "behavioral snapshot precedes the failing teardown");
        assert.equal(snapshot.outcome, "accepted");
      }
      if (kind === "late-bridge") {
        assert.ok(audit.bridgeErrors.includes(error));
        assert.ok(failure.errors.some(e => e.message.includes("unexpected bridge error, including teardown")));
      }
      if (kind === "late-sdk") {
        assert.ok(audit.sdkErrors.some(e => e.event === "session_shutdown" && e.error.includes(error.message)));
        assert.ok(failure.errors.some(e => e.message.includes("unexpected SDK error, including teardown")));
      }
      if (kind === "peer") {
        assert.ok(audit.peerErrors.includes(error));
        assert.ok(consoleErrors.some(args => args.includes(error)), "real SDK event bus swallowed and logged the peer exception");
        assert.ok(failure.errors.some(e => e.message.includes("unexpected question-peer fixture error")));
      }
      if (kind === "provider") {
        assert.ok(audit.providerErrors.includes(error));
        assert.ok(failure.errors.some(e => e.message.includes("unexpected provider fixture error")));
      }
      if (kind === "blocked-barrier") {
        assert.ok(failure.errors.includes(error), "cancellation must reject the held wait with its original reason");
        assert.equal(snapshot, undefined, "no completed behavior is claimed after a blocked observation");
      }
      if (kind === "dispose") assert.deepEqual(audit.teardownErrors, [error]);
      else assert.deepEqual(audit.teardownErrors, [], "cleanup completed without a deadline or secondary teardown failure");
      t.diagnostic(JSON.stringify({ self_check: kind, rejected: true, fixture_removed: audit.fixtureRemoved,
        global_removed: audit.globalRemoved, teardown_errors: audit.teardownErrors.length }));
    });
  }
});
