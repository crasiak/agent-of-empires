import { VERSION } from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import { performance } from "node:perf_hooks";
import { randomUUID } from "node:crypto";
import { registerAfk } from "./aoe-afk.mjs";

const COVERAGE = "stock-sdk-main-loop-reservations";
const NAMES = ["aoe_afk_read", "aoe_afk_record", "aoe_afk_apply"];
const textResult = value => ({ content: [{ type: "text", text: JSON.stringify(value) }], details: value });
const REMINDER = "AoE explicit delegation. Use only aoe_afk_read, aoe_afk_record, aoe_afk_apply. One reversible low-risk file action within the exact operator grant. Record alternatives with pros/cons, criteria weights and scores (1..5), evidence, assumptions, uncertainties, rationale, rollback, recommendation, risk, human_required, depends_on and exact action before applying on a LATER request. Same-response sibling apply is forbidden. Known ask-first/security gates veto regardless of scores; unknown/high risk or dependent work must be deferred. Claims are model evaluations, not human approval or machine-proven safety. No native tools, shell, network or descendants. No extra reporting request. Bound is stock-SDK main-loop reservations, not physical retries or dollars. Native transcript/provider retention is separate. Independent background coverage is unknown.";

export function delegationRuntime(pi) {
  let current;
  const qualified = VERSION === "0.87.1";
  const live = r => r.view && performance.now() < r.delegateDeadline && Date.now() < r.view.expires_at_ms;
  const observable = r => r.signal && !r.signal.aborted && r.ctx.signal === r.signal && !r.ctx.isIdle() && !r.settling;
  const active = r => r && (r.owned || (live(r) && !r.parked && r.view.state === "pending"));
  const supportedQuestion = args => args && typeof args.question === "string" && args.question.length > 0
    && Buffer.byteLength(JSON.stringify(args)) <= 8192
    && Object.keys(args).every(k => ["question", "context", "options", "allowMultiple", "allowFreeform", "allowComment", "displayMode", "overlayToggleKey", "commentToggleKey", "timeout"].includes(k))
    && (!args.options || (Array.isArray(args.options) && args.options.every(o => o && typeof o.title === "string" && Object.keys(o).every(k => ["title", "description"].includes(k)))));
  const stop = async (r, reason, abort = false) => {
    if (!r) return;
    r.parked = true;
    r.reason ??= reason;
    if (abort) r.ctx.abort();
    const window = r.view?.window;
    if (!window || r.view.state === "ended" || r.stoppedWindow === window) return;
    if (r.stopping) await r.stopping;
    if (r.stoppedWindow === window) return;
    r.stopping = (async () => {
      try {
        await r.bridge.request({ op: "runtime", command: { kind: "stop", window } });
        r.stoppedWindow = window;
      } catch { r.failure = true; }
    })();
    await r.stopping;
    r.stopping = undefined;
  };
  const forgetSignal = r => {
    r.signal?.removeEventListener("abort", r.onAbort);
    r.signal = undefined;
    r.onAbort = undefined;
  };
  const observe = (_event, ctx) => {
    const r = current;
    if (!r || r.signal === ctx.signal) return;
    forgetSignal(r);
    r.signal = ctx.signal;
    if (!r.signal) return;
    r.onAbort = () => { void stop(r, "explicit stop"); };
    r.signal.addEventListener("abort", r.onAbort, { once: true });
    if (r.signal.aborted) r.onAbort();
  };
  // Capture the ordinary operation before any poll can acknowledge a grant.
  pi.on("turn_start", observe);
  pi.on("context", observe);
  const command = async (r, data, ctx) => {
    if (!r || !r.view || !r.owned || r.parked || !observable(r) || ctx.signal?.aborted || !r.request || performance.now() >= r.delegateDeadline || Date.now() >= r.view.expires_at_ms) {
      ctx.abort(); throw new Error("AFK tool has no current owned request");
    }
    try { return await r.bridge.request({ op: "runtime", command: { ...data, window: r.view.window, request: r.request } }); }
    catch (error) { await stop(r, "host denied/failed", true); throw error; }
  };
  for (const [name, parameters, run] of [
    [NAMES[0], Type.Object({ path: Type.String({ maxLength: 256 }) }, { additionalProperties: false }), (args) => ({ kind: "read", path: args.path })],
    [NAMES[1], Type.Object({ decision: Type.Unknown() }, { additionalProperties: false }), (args) => ({ kind: "record", decision: args.decision })],
    [NAMES[2], Type.Object({ decision: Type.String({ maxLength: 36 }) }, { additionalProperties: false }), (args) => ({ kind: "apply", decision: args.decision })],
  ]) {
    pi.registerTool({ name, label: name, description: `${REMINDER} Record schema: {id:UUID,question,alternatives:[{name,pros,cons,scores:[1..5]}],criteria:[{name,weight:1..5}],evidence:[],assumptions:[],uncertainties:[],recommendation,rationale,rollback,reversible:boolean,risk:"low"|"high"|"unknown",human_required:boolean,depends_on:[],action:null|{path,expected_hash:null|sha256,content}}. Record at most 8KiB UTF8 JSON.`, parameters, executionMode: "sequential",
      async execute(_id, args, _signal, _update, ctx) { return textResult(await command(current, run(args), ctx)); },
    });
  }
  pi.on("tool_call", async (event, ctx) => {
    const r = current;
    if (!r?.owned) {
      if (NAMES.includes(event.toolName)) return { block: true, reason: "AFK delegation is not owned" };
      return;
    }
    if (!r.parked && event.toolName === "ask_user" && supportedQuestion(event.input)) {
      return { block: true, reason: JSON.stringify({ status: "deferred_by_afk", provenance: "no_human_answer", human_required: true, approval: false, instruction: "Record this question as human-required. Only independent granted work may proceed." }) };
    }
    if (r.parked || !NAMES.includes(event.toolName)) {
      await stop(r, "unsupported tool or ended delegation", true);
      return { block: true, reason: "AFK: no human answer or permission approval; unsupported tool blocked" };
    }
  });
  const intervention = reason => {
    const r = current;
    if (!r) return Promise.resolve();
    r.parked = true;
    const pending = (async () => {
      await r.refresh?.();
      await stop(r, reason);
    })();
    r.interaction = pending;
    return pending.finally(() => { if (r.interaction === pending) r.interaction = undefined; });
  };
  pi.on("input", async () => {
    // No abort or transformation: Pi retains the input, including queued steering.
    await intervention("human input");
    return { action: "continue" };
  });
  pi.on("ui_prompt_end", () => intervention("UI interaction; origin unknown, no human answer inferred"));
  pi.on("turn_end", async (event, ctx) => {
    if (event.outcome === "aborted" || ctx.signal?.aborted) await intervention("explicit stop");
  });
  pi.on("agent_settled", async () => {
    const r = current;
    if (!r) return;
    r.settling = true;
    forgetSignal(r);
    await r.refresh?.();
    if (r.view || r.owned) await stop(r, "settled without further ownership");
    r.owned = false; r.request = undefined; r.settling = false;
  });
  pi.on("cache_warming_decision", () => active(current) ? { action: "stop" } : undefined);
  pi.on("session_before_compact", async (_event, ctx) => {
    if (!active(current)) return;
    await stop(current, "compaction requires human intervention", current.owned);
    return { cancel: true };
  });
  for (const event of ["session_before_tree", "session_before_fork", "session_before_switch"]) {
    pi.on(event, async () => {
      if (!active(current)) return;
      await stop(current, "human navigation", current.owned);
      return { cancel: true };
    });
  }
  return {
    attach(r) { current = r; r.owned = false; r.parked = false; },
    async shutdown(r) { forgetSignal(r); await stop(r, "shutdown", r.owned); if (current === r) current = undefined; },
    failed(r) { if (active(r)) { void stop(r, "host unavailable", r.owned); } },
    status(r) { return r.view ? `AFK delegation: ${r.parked ? `ended (${r.reason})` : r.view.state}; SDK reservations ${r.view.reservations_used}/${r.view.grant.requests}; background unknown` : undefined; },
    async poll(r, view, probe) {
      if (view && r.window !== view.window) {
        // Never replace a still-owned episode with a new operator window.
        if (r.owned) { await stop(r, "replacement during owned episode", true); return; }
        r.window = view.window;
        r.parked = false; r.request = undefined; r.reason = undefined;
        if (!qualified) { r.view = view; await stop(r, "unsupported SDK; executable activation refused"); return; }
        r.delegateDeadline = performance.now() + Math.max(0, Math.min(view.expires_at_ms - view.issued_at_ms, view.expires_at_ms - Date.now()));
      }
      if (!view && r.view) await stop(r, "delegation disappeared", r.owned);
      r.view = view;
      if (view && (view.state === "ended" || !live(r))) await stop(r, "ended or expired");
      if (view?.state === "pending" && (r.parked || r.failure || r.interaction || !observable(r))) {
        await stop(r, "executable activation refused: start ordinary work with an observable stop signal, then delegate");
        return;
      }
      if (probe && qualified && probe.version === 2 && JSON.stringify(probe.binding) === JSON.stringify(r.bootstrap.binding)
        && (!probe.window || (probe.window === view?.window && probe.grant_hash === view?.grant_hash))) {
        const ack = { ...probe, generation: r.generation, sdk: VERSION, coverage: COVERAGE };
        const signature = JSON.stringify(ack);
        if (r.runtimeAck !== signature) {
          await r.bridge.request({ op: "runtime_ack", ack });
          if (view?.state === "pending" && (r.parked || r.interaction || !observable(r) || !live(r))) {
            await stop(r, "activation interrupted before acknowledgement completed");
            return;
          }
          r.runtimeAck = signature;
        }
      }
    },
    async context(r, messages, ctx) {
      if (r.interaction) await r.interaction;
      const w = r.view;
      if (!qualified || (!r.owned && (r.parked || !w || w.state !== "pending" || !w.confirmed))) return;
      // Context is emitted after the prior tool batch settles, never in a tool callback.
      r.owned = true;
      if (!w || r.parked || r.failure || !observable(r) || performance.now() >= r.delegateDeadline || Date.now() >= w.expires_at_ms || ctx.signal?.aborted || ctx.hasPendingMessages() || w.generation !== r.generation || w.state === "ended") {
        await stop(r, "ended, interrupted or stale", true); return { messages };
      }
      try {
        const request = randomUUID();
        const receipt = await r.bridge.request({ op: "runtime", command: { kind: "reserve", window: w.window, request } });
        if (!observable(r) || ctx.signal?.aborted || r.parked || performance.now() >= r.delegateDeadline || Date.now() >= w.expires_at_ms) { await stop(r, "interrupted reservation", true); return { messages }; }
        r.request = request;
        return { messages: [...messages.filter(m => m.customType !== "aoe-afk-delegation"), { role: "custom", customType: "aoe-afk-delegation", content: `${REMINDER}\nHost grant: ${JSON.stringify(receipt)}`, display: false, timestamp: Date.now() }] };
      } catch { await stop(r, "reservation denied/failed", true); return { messages }; }
    },
  };
}

export default function (pi) { registerAfk(pi, { protocol: 2, delegation: delegationRuntime(pi) }); }
