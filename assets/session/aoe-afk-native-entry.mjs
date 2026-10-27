import { readFileSync, writeSync, closeSync, createReadStream } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { AssistantMessageEventStream } from "@earendil-works/pi-ai";
import { registerAfk, spawnBridge } from "./aoe-afk.mjs";
import { delegationRuntime } from "./aoe-afk-runtime.mjs";
import { privateTrace, observeBridge, observeDelegation, questionAttribution, publishMarker, cancellationCommand } from "./aoe-afk-observer-fixture.mjs";
import * as probes from "./aoe-afk-observer-probes.mjs";
import { KIND, receiptBridge, createPublicationAudit } from "./aoe-afk-native-contract.mjs";
import { fixedProvider } from "./aoe-afk-native-provider.mjs";

export default function nativeDiagnostic(pi) {
  const root = process.env.AOE_NATIVE_ATTEMPT;
  const config = JSON.parse(readFileSync(join(root, "attempt.json"), "utf8"));
  const state = { calls: 0, fault: false, teardown: false, cancelled: false, waitArmed: false, runCompleted: false,
    shutdown: false, off: false, receipts: [] };
  const publication = createPublicationAudit();
  let runtime, ctx, bridgeCount = 0, closed = false;
  const observed = config.role === "observed";
  const trace = observed ? privateTrace(join(root, "trace.jsonl")) : null;
  const probe = observed ? probes.privateProbe(join(root, "probes.jsonl")) : null;
  const fault = () => { state.fault = true; trace?.invalidate(); probe?.invalidate(); };
  const save = (name, value) => {
    try { publishMarker(join(root, name), value); return true; } catch { fault(); return false; }
  };
  const native = () => ({ kind: KIND, ready: state.ready === true, native_id: ctx?.sessionManager.getSessionId(),
    file: ctx?.sessionManager.getSessionFile(), opened: state.opened, prompt_start: state.prompt_start,
    closed: state.closed, reply: state.reply, prompt_end: state.prompt_end, result: state.result, receipts: state.receipts });
  const attribution = questionAttribution(() => {
    if (!ctx?.sessionManager.getSessionFile() || state.calls !== 1 || state.fault) { fault(); return; }
    state.ready = true;
    save("ready.json", { ...native(), binding: runtime.bootstrap.binding, generation: runtime.generation });
  }, fault);
  pi.on("ui_prompt_start", event => {
    if (event.owner?.namespace !== "pi-ask-user") return;
    state.prompt_start = structuredClone(event); attribution.start(event);
  });
  pi.on("ui_prompt_end", event => {
    if (event.owner?.namespace !== "pi-ask-user") return;
    if (state.prompt_end) fault();
    state.prompt_end = structuredClone(event);
  });
  for (const channel of ["opened", "closed", "reply"]) pi.events.on(`ask:defer:v1:${channel}`, event => {
    if (channel === "reply" && event.method !== "defer") return;
    if (state[channel]) fault();
    state[channel] = structuredClone(event);
    if (channel === "opened") attribution.opened(event);
    trace?.emit(channel === "closed" ? "question_closed" : channel, { teardown: state.teardown });
  });
  pi.on("tool_result", event => {
    if (event.toolName !== "ask_user") { fault(); return; }
    if (state.result) fault();
    state.result = structuredClone(event);
  });
  const delegate = delegationRuntime(pi);
  const captured = { ...delegate, attach(r) { runtime = r; return delegate.attach(r); } };
  const registration = { ...pi, on(event, handler) {
    return pi.on(event, event === "session_start" ? async (...args) => {
      await handler(...args);
      ctx = args[1];
      if (!runtime || ctx.mode !== "tui" || !ctx.hasUI || ctx.sessionManager.getSessionId() !== runtime.bootstrap.binding.native_id) throw Error("native managed binding unavailable");
      if (observed && !probes.observeRefresh(runtime, probe, state)) fault();
    } : handler);
  } };
  const d = observed ? observeDelegation(probes.observeDelegation(captured, state, probe), trace, state) : captured;
  registerAfk(registration, { protocol: 6, delegation: d, bridge(bootstrap, generation) {
    if (++bridgeCount !== 1) throw Error("native fixture cannot replace its bridge");
    const bridge = receiptBridge(spawnBridge(bootstrap, generation), {
      binding: structuredClone(bootstrap.binding), generation, trace, probes: probe, fault,
      save(receipt) {
        publication.observe(receipt);
        if (state.receipts.length >= 16) throw Error("receipt cap");
        state.receipts.push(receipt);
      },
    });
    return observed ? observeBridge(probes.observeBridge(bridge, state, 1, probe), trace, state, 1) : bridge;
  } });
  pi.registerProvider(config.model.provider, { api: config.model.api, apiKey: "offline-not-a-credential", models: [config.model],
    streamSimple: fixedProvider(AssistantMessageEventStream, config, state, { save, fault, native }),
  });
  pi.registerCommand("aoe-native-complete", { description: "Arm this diagnostic's cancelled SDK completion", handler: cancellationCommand(state, save) });
  const controls = createInterface({ input: createReadStream(null, { fd: Number(process.env.AOE_NATIVE_CONTROL_FD), autoClose: true }) });
  controls.on("line", line => {
    try {
      if (line === "cancel" && state.waitArmed && !state.teardown && state.calls === 2) {
        state.teardown = true; trace?.emit("teardown", { teardown: true }); ctx.abortPreservingQueue();
      } else if (line === "off" && state.runCompleted && !state.off) { state.off = true; save("off-seen.json", { off: true }); }
      else if (line === "shutdown" && state.off) ctx.shutdown();
      else throw Error("invalid native control transition");
    } catch { fault(); }
  });
  controls.on("close", () => { if (!state.shutdown) fault(); });
  pi.on("session_shutdown", () => { state.shutdown = true; controls.close(); });
  process.on("uncaughtExceptionMonitor", fault);
  process.on("exit", () => {
    if (closed) return;
    closed = true;
    trace?.close(); probe?.close();
    const audit = { kind: KIND, version: 1, manifest: config.manifest, role: config.role, ordinal: config.ordinal,
      waitArmed: state.waitArmed, cancelled: state.cancelled, consumed: state.episode?.consumed === true,
      settled: state.runCompleted, off: state.off, shutdown: state.shutdown, writersClosed: true, fault: state.fault,
      trace: trace?.snapshot() ?? null, probes: probe?.snapshot() ?? null, publication: publication.snapshot() };
    try {
      const fd = Number(process.env.AOE_NATIVE_AUDIT_FD), bytes = JSON.stringify(audit) + "\n";
      if (writeSync(fd, bytes) !== Buffer.byteLength(bytes)) process.exitCode = 1;
      closeSync(fd);
    } catch { process.exitCode = 1; }
  });
}
