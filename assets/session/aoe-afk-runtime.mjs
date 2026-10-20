import { VERSION } from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import { performance } from "node:perf_hooks";
import { randomUUID } from "node:crypto";
import { questionRuntime } from "./aoe-afk-questions.mjs";
import { registerAfk } from "./aoe-afk.mjs";
import { createCollector } from "./aoe-afk-usage.mjs";
import { createUsageWitness, USAGE_COVERAGE } from "./aoe-afk-usage-guard.mjs";
import { automaticWake } from "./aoe-afk-wake.mjs";

const COVERAGE = "stock-sdk-main-loop-reservations";
const NAMES = ["aoe_afk_read", "aoe_afk_record", "aoe_afk_apply", "aoe_afk_checkpoint"];
const textResult = value => ({ content: [{ type: "text", text: JSON.stringify(value) }], details: value });
const CONTINUATION = "aoe-afk-continuation";
const clean = messages => messages.filter(m => ![CONTINUATION, "aoe-afk-delegation"].includes(m.customType));
const externalMessages = messages => JSON.stringify(messages.filter(m => ["user", "custom"].includes(m.role)));
const competing = event => event.continue || event.entries.length > 0 || event.context.pendingMessages.length > 0;
const REMINDER = "AoE explicit delegation. Use only aoe_afk_read, aoe_afk_record, aoe_afk_apply, aoe_afk_checkpoint. Checkpoint schema: {status:unfinished|completed|blocked,next_step:null|{kind:read|record,path}|{kind:apply,decision},rationale,evidence:[host evidence hashes],depends_on:[]}. Record unfinished granted work before settlement. Host binds evidence; prose and identical reads are not progress. Explicit settlement nudges grant no requests or effects. One reversible low-risk file action within the exact operator grant. Record alternatives with pros/cons, criteria weights and scores (1..5), evidence, assumptions, uncertainties, rationale, rollback, recommendation, risk, human_required, depends_on and exact action before applying on a LATER request. Same-response sibling apply is forbidden. Known ask-first/security gates veto regardless of scores; unknown/high risk or dependent work must be deferred. Claims are model evaluations, not human approval or machine-proven safety. No native tools, shell, network or descendants. No extra reporting request. Bound is stock-SDK main-loop reservations, not physical retries or dollars. Native transcript/provider retention is separate. Independent background coverage is unknown.";

export function delegationRuntime(pi) {
  let current;
  const qualified = VERSION === "0.87.1";
  const live = r => r.view && performance.now() < r.delegateDeadline && Date.now() < r.view.expires_at_ms;
  const observable = r => r.signal && !r.signal.aborted && r.ctx.signal === r.signal && !r.ctx.isIdle() && !r.settling;
  const active = r => r && (r.usageWitness?.failure || r.usageWitness?.pending || r.usageDenied || r.owned || (live(r) && (r.budgetDenied || (!r.parked && r.view.state === "pending"))));
  const usageBlocked = r => r.usageWitness.blocked(r.view) || (r.usageWitness.pending > 0
    && (r.parked || r.failure || !live(r) || !observable(r) || r.stoppedWindow === r.view?.window));
  const usageDenial = error => ["usage_exhausted", "usage_stale", "usage_unavailable"].includes(error?.code);
  const sharedDenial = error => error?.code === "shared_budget_exhausted";
  const supportedQuestion = args => args && typeof args.question === "string" && args.question.length > 0
    && Buffer.byteLength(JSON.stringify(args)) <= 8192
    && Object.keys(args).every(k => ["question", "context", "options", "allowMultiple", "allowFreeform", "allowComment", "displayMode", "overlayToggleKey", "commentToggleKey", "timeout"].includes(k))
    && (!args.options || (Array.isArray(args.options) && args.options.every(o => o && typeof o.title === "string" && Object.keys(o).every(k => ["title", "description"].includes(k)))));
  const abortEpisode = (r, ctx = r?.ctx) => {
    if (r?.queueAbort) r.queueAbort();
    else if (!r?.view?.grant.settlement_nudges && !r?.view?.grant.question_deferrals) ctx.abort();
  };
  const policyAbort = (r, ctx = r.ctx) => {
    r.budgetDenied = true;
    r.reason = "shared admission denied; existing host rights retained; fresh ordinary input required";
    r.budgetAbortSignal = ctx.signal;
    abortEpisode(r, ctx);
  };
  const usageAbort = (r, ctx = r.ctx, reason = r.usageDenied ?? "usage_unavailable", force = false) => {
    if (r.usageWitness?.released(r.view)) return;
    try { r.usageWitness?.require(r.view); } catch { reason = "usage_unavailable"; }
    r.usageDenied = reason; r.parked = true; r.reason = `observed usage guard: ${reason}`;
    if ((!force && !r.owned) || r.questionObserved || r.questionCall) return;
    if (typeof ctx.abortPreservingQueue === "function") ctx.abortPreservingQueue();
    else { r.questionWarning = "Unsupported usage-guard restoration: dispatch blocked, NOT safe against native custom-queue loss; release blocked"; ctx.abort(); }
  };
  const refreshUsage = async (r, boundary) => { if (r.refreshing) await r.refreshing; await r.refresh?.(boundary); };
  const closing = (r, view = r.view) => view?.usage_policy_hash != null && view.state === "ended" && view.terminal_reason === "completed"
    && view.window === r.window && view.generation === r.generation && ["instance_id", "profile", "native_id", "launch_id"].every(k => view.binding?.[k] === r.bootstrap.binding[k])
    && !r.failure && !r.usageDenied && !r.usageWitness.failure && !r.usageWitness.restoring
    && ((r.owned && r.signal && !r.signal.aborted) || (r.usageCompletion?.window === view.window && r.usageCompletion.ticket.isCurrent()));
  const completeEpisode = (r, ctx) => {
    const ticket = ctx.endRunPreservingQueue?.();
    if (ticket?.version !== 1 || typeof ticket.isCurrent !== "function") { usageAbort(r, ctx, "usage_unavailable", true); return; }
    const completion = { window: r.view.window, generation: r.generation, ticket, proved: false, done: undefined };
    r.usageCompletion = completion;
    const currentProof = () => current === r && r.usageCompletion === completion && ticket.isCurrent()
      && r.view?.window === completion.window && r.generation === completion.generation && !r.failure && !r.usageDenied;
    completion.done = (async () => {
      try {
        if (!(await ticket.completion) || !currentProof()) return;
        completion.proved = true;
        for (let page = 0; page < 65; page++) {
          if (!currentProof()) return;
          await refreshUsage(r, "settled");
          if (!currentProof()) return;
          const observation = r.usageCollector.observe(r, "settled"), report = observation.report;
          if (observation.gap || !report) return;
          if (report.after.count === report.head.count && report.after.hash === report.head.hash) {
            r.usageWitness.cleanEnd(r.view, { ticket, head: report.head });
            return;
          }
        }
      } catch { usageAbort(r, r.ctx, "usage_unavailable"); }
    })();
  };
  const stop = async (r, reason, abort = false, disposition = "interrupted") => {
    if (!r) return;
    r.parked = true;
    r.reason ??= reason;
    if (abort) abortEpisode(r);
    const window = r.view?.window;
    if (!window || r.view.state === "ended" || r.stoppedWindow === window) return;
    if (r.stopping) await r.stopping;
    if (r.stoppedWindow === window) return;
    r.stopping = (async () => {
      try {
        await r.bridge.request({ op: "runtime", command: { kind: "stop", window, reason: disposition } });
        r.stoppedWindow = window;
      } catch { r.failure = true; }
    })();
    await r.stopping;
    r.stopping = undefined;
  };
  const questions = questionRuntime(pi, { current: () => current, stop, live, observable, supportedQuestion, usageDenied: (r, reason) => usageAbort(r, r.ctx, reason) });
  const wake = automaticWake(pi, (r, received) => {
    try {
      return qualified && r.ctx.mode === "tui" && r.ctx.isIdle() && !r.ctx.hasPendingMessages()
        && typeof r.ctx.ui.getEditorText === "function" && r.ctx.ui.getEditorText() === ""
        && !active(r) && !r.failure && !r.interaction && questions.wakeClear(r) && (!received || r.state === "off");
    } catch { return false; }
  });
  const forgetSignal = r => {
    r.signal?.removeEventListener("abort", r.onAbort);
    r.signal = undefined;
    r.onAbort = undefined;
  };
  const observe = (event, ctx) => {
    const r = current;
    if (!r) return;
    wake.activity(r);
    if (event.type === "context") r.ordinaryExternal = externalMessages(clean(event.messages));
    if (r.signal === ctx.signal) return;
    forgetSignal(r);
    r.signal = ctx.signal;
    if (!r.signal) return;
    r.onAbort = () => { if (r.budgetDenied && r.budgetAbortSignal === r.signal) return; void stop(r, "explicit stop"); };
    r.signal.addEventListener("abort", r.onAbort, { once: true });
    if (r.signal.aborted) r.onAbort();
  };
  // Capture the ordinary operation before any poll can acknowledge a grant.
  pi.on("turn_start", observe);
  pi.on("context", observe);
  const command = async (r, data, ctx) => {
    if (!r || !r.view || !r.owned || r.parked || !observable(r) || ctx.signal?.aborted || !r.request || performance.now() >= r.delegateDeadline || Date.now() >= r.view.expires_at_ms) {
      abortEpisode(r, ctx); throw new Error("AFK tool has no current owned request");
    }
    try { return await r.bridge.request({ op: "runtime", command: { ...data, window: r.view.window, request: r.request } }); }
    catch (error) { if (usageDenial(error)) usageAbort(r, ctx, error.code); else await stop(r, "host denied/failed", true, "failed"); throw error; }
  };
  for (const [name, parameters, run] of [
    [NAMES[0], Type.Object({ path: Type.String({ maxLength: 256 }) }, { additionalProperties: false }), (args) => ({ kind: "read", path: args.path })],
    [NAMES[1], Type.Object({ decision: Type.Unknown() }, { additionalProperties: false }), (args) => ({ kind: "record", decision: args.decision })],
    [NAMES[2], Type.Object({ decision: Type.String({ maxLength: 36 }) }, { additionalProperties: false }), (args) => ({ kind: "apply", decision: args.decision })],
    [NAMES[3], Type.Object({ checkpoint: Type.Unknown() }, { additionalProperties: false }), args => ({ kind: "checkpoint", claims: args.checkpoint })],
  ]) {
    pi.registerTool({ name, label: name, description: `${REMINDER} Record schema: {id:UUID,question,alternatives:[{name,pros,cons,scores:[1..5]}],criteria:[{name,weight:1..5}],evidence:[],assumptions:[],uncertainties:[],recommendation,rationale,rollback,reversible:boolean,risk:"low"|"high"|"unknown",human_required:boolean,depends_on:[],action:null|{path,expected_hash:null|sha256,content}}. Record at most 8KiB UTF8 JSON.`, parameters, executionMode: "sequential",
      async execute(_id, args, _signal, _update, ctx) { return textResult(await command(current, run(args), ctx)); },
    });
  }
  pi.on("tool_call", async (event, ctx) => {
    const r = current;
    if (r) await refreshUsage(r, "before_tool");
    if (r && ((r.usageDenied && r.owned) || (r.usageWitness.restoring && r.usageWitness.blocked(r.view)))) { usageAbort(r, ctx, r.usageDenied, true); return { block: true, reason: "AFK observed usage guard denied automatic effect" }; }
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
  pi.on("input", async (event, ctx) => {
    wake.activity(current);
    if (current) questions.input(current, event, ctx);
    // No abort or transformation: Pi retains the input, including queued steering.
    await intervention("human input");
    return { action: "continue" };
  });
  pi.on("ui_prompt_start", () => wake.activity(current));
  pi.on("ui_prompt_end", event => {
    wake.activity(current);
    return questions.end(current, event) ? undefined : intervention("UI interaction; origin unknown, no human answer inferred");
  });
  pi.on("turn_end", async (event, ctx) => {
    if (current) await refreshUsage(current, "turn_end");
    if (current && closing(current) && event.outcome === "completed" && !ctx.signal?.aborted) {
      if (!current.usageCompletion) completeEpisode(current, ctx);
      return;
    }
    if (current?.usageDenied) { usageAbort(current, ctx); return; }
    if (current?.budgetDenied && current.budgetAbortSignal?.aborted) return;
    if (event.outcome === "aborted" || ctx.signal?.aborted) await intervention("explicit stop");
  });
  pi.on("agent_before_settle", async (event, ctx) => {
    const r = current;
    if (!r?.owned || r.budgetDenied || r.usageDenied) return;
    r.settling = true;
    r.contribution = undefined;
    if (event.outcome !== "completed") { await stop(r, "non-completed boundary", true, event.outcome === "error" ? "failed" : "interrupted"); return; }
    if (r.parked || r.failure || !live(r)) { await stop(r, "ended boundary", true); return; }
    if (competing(event) || ctx.hasPendingMessages()) { await stop(r, "existing work at settlement"); return; }
    try {
      await r.refresh("turn_end");
      if (r.parked || r.failure || !live(r) || r.view.state !== "owned" || r.view.generation !== r.generation) { await stop(r, "stale boundary", true); return; }
      const checkpoint = r.view.checkpoints?.at(-1);
      if (!checkpoint || checkpoint.claims.status !== "unfinished" || !r.view.grant.settlement_nudges) return;
      const receipt = await r.bridge.request({ op: "runtime", command: { kind: "nudge", window: r.view.window, request: r.request, checkpoint: checkpoint.sequence } });
      if (r.parked || ctx.hasPendingMessages()) { await stop(r, "interrupted admission"); return; }
      if (!live(r)) { await stop(r, "expired admission", true); return; }
      await r.bridge.request({ op: "runtime", command: { kind: "contribution_intent", window: r.view.window, identity: receipt.identity } });
      if (r.parked) return;
      if (!live(r)) { await stop(r, "expired contribution", true); return; }
      r.contribution = receipt.identity;
    } catch (error) {
      if (usageDenial(error)) { usageAbort(r, ctx, error.code); return; }
      if (sharedDenial(error)) { r.budgetDenied = true; r.reason = "shared admission denied; no nudge contributed"; return; }
      await stop(r, "settlement admission denied/failed", true, "failed");
    }
  });
  // Pi refreshes the queue/draft preview between these ordered handlers.
  pi.on("agent_before_settle", (event, ctx) => {
    const r = current;
    if (!r?.contribution || r.parked) return;
    if (!live(r) || event.outcome !== "completed") { void stop(r, "ended contribution", true); return; }
    if (competing(event) || ctx.hasPendingMessages()) { void stop(r, "competing settlement work"); return; }
    const entry = { type: "custom_message", customType: CONTINUATION, content: `[AoE settlement nudge] Continue only the recorded unfinished operator-granted step. No new request or file authority. Identity: ${JSON.stringify(r.contribution)}`, display: true, details: r.contribution };
    r.expectedContinuation = { identity: r.contribution, content: entry.content, before: JSON.stringify(clean(event.context.contextMessages).filter(m => m.role !== "system")) };
    return { entries: [...event.entries, entry], continue: true };
  });
  pi.on("agent_settled", async () => {
    const r = current;
    if (!r) return;
    r.settling = true;
    forgetSignal(r);
    if (r.usageCompletion) {
      r.owned = false; r.request = undefined; r.expectedContinuation = undefined; r.contribution = undefined; r.settling = false;
      return;
    }
    await refreshUsage(r, "settled");
    if (r.budgetDenied || r.usageDenied) {
      r.owned = false; r.request = undefined; r.settling = false; r.budgetAbortSignal = undefined;
      return;
    }
    await r.refresh?.();
    if (r.view || r.owned) {
      r.parked = true;
      try { await r.bridge.request({ op: "runtime", command: { kind: "settle", window: r.view.window } }); }
      catch { await stop(r, "settlement failed", false, "failed"); }
    }
    r.expectedContinuation = undefined; r.contribution = undefined;
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
    attach(r) {
      current = r; r.owned = false; r.parked = false; r.usageCollector = createCollector(); r.recoveredUsageWindows = new Set();
      r.usageWitness = createUsageWitness(pi, r);
      if (r.usageWitness.failure || r.usageWitness.pending) r.usageDenied = "usage_unavailable";
      questions.attach(r); wake.attach(r);
    },
    wakeObservation(r) { return wake.observation(r); },
    wakeReply(r, reply, observation) { wake.receive(r, reply, observation); },
    observation(r, boundary) { return r.usageCollector.observe(r, boundary); },
    usageReply(r, reply) { r.usageCollector.accept(reply); },
    async shutdown(r) { wake.close(r); questions.shutdown(r); forgetSignal(r); await stop(r, "shutdown", r.owned); if (current === r) current = undefined; },
    failed(r) {
      wake.activity(r);
      r?.usageWitness?.revokeCompleted();
      if (r?.usageWitness?.failure || r?.usageWitness?.pending || (r?.view?.usage_policy_hash && !r.usageWitness.released(r.view) && !r.view.usage?.ordinary_recovered && !r.recoveredUsageWindows.has(r.view.window))) usageAbort(r, r.ctx, "usage_unavailable");
      else if (active(r)) { void stop(r, "host unavailable", r.owned, "failed"); }
    },
    status(r) { return (r.usageWitness?.failure ? "AFK native usage persistence/provenance unavailable; automatic work blocked" : undefined) ?? r.questionWarning ?? (r.usageDenied ? `AFK observed usage guard: ${r.usageDenied}; fresh ordinary recovery required; no refund` : undefined) ?? r.questionDenied ?? (r.budgetDenied ? `AFK shared admission denied; ordinary recovery required; no refund` : undefined) ?? (r.view ? `AFK delegation: ${r.parked ? `ended (${r.reason})` : r.view.state}; SDK reservations ${r.view.reservations_used}/${r.view.grant.requests}; nudges ${r.view.nudges_used}/${r.view.grant.settlement_nudges}; question deferrals ${r.view.question_deferrals_used ?? 0}/${r.view.grant.question_deferrals ?? 0}; one file effect; ${r.view.terminal_reason ?? "nonterminal"}; background unknown` : wake.status(r)); },
    async poll(r, view, probe, guards) {
      questions.restoreGuards(r, guards);
      if (view?.binding && ["instance_id", "profile", "native_id"].some(k => view.binding[k] !== r.bootstrap.binding[k])) view = null;
      if (view && r.window !== view.window) {
        // Never replace a still-owned episode with a new operator window.
        if (r.owned) { await stop(r, "replacement during owned episode", true); return; }
        questions.newWindow(r);
        r.window = view.window; r.externalMessages = undefined; r.queueAbort = undefined; r.usageCompletion = undefined;
        r.budgetDenied = false; r.budgetAbortSignal = undefined; r.questionDenied = undefined; r.usageDenied = undefined;
        r.parked = false; r.request = undefined; r.reason = undefined; r.questionUnavailable = false;
        if (!qualified) { r.view = view; await stop(r, "unsupported SDK; executable activation refused"); return; }
        r.delegateDeadline = performance.now() + Math.max(0, Math.min(view.expires_at_ms - view.issued_at_ms, view.expires_at_ms - Date.now()));
      }
      if (!view && r.view) {
        r.usageWitness.revokeCompleted();
        await stop(r, "delegation disappeared", r.owned);
      }
      r.view = view;
      try {
        if (view?.usage_policy_hash && !view.usage?.ordinary_recovered && !r.recoveredUsageWindows.has(view.window)) r.usageWitness.require(view);
      } catch { usageAbort(r, r.ctx, "usage_unavailable"); return; }
      const usageBlocked = view?.usage?.shared_blocked ?? view?.usage?.blocked;
      if (usageBlocked && !r.usageWitness.released(view) && !view.usage.ordinary_recovered && !r.recoveredUsageWindows.has(view.window)) usageAbort(r, r.ctx, `usage_${usageBlocked}`);
      if (r.usageWitness.blocked(view) && !r.usageWitness.acknowledge(view) && !closing(r, view)) usageAbort(r, r.ctx);
      if (view && (view.state === "ended" || !live(r))) await stop(r, "ended or expired");
      if (r.budgetDenied) return;
      if (view?.state === "pending" && (view.grant.settlement_nudges > 0 || view.grant.question_deferrals > 0 || view.batch?.shared_requests != null || view.usage_policy_hash != null)) {
        if (typeof r.ctx.abortPreservingQueue !== "function") {
          await stop(r, "settlement nudges unavailable: queue-preserving abort capability required");
          return;
        }
        r.queueAbort = () => r.ctx.abortPreservingQueue();
      }
      if (view?.state === "pending" && (r.parked || r.failure || r.interaction || !observable(r))) {
        await stop(r, "executable activation refused: start ordinary work with an observable stop signal, then delegate");
        return;
      }
      await questions.poll(r);
      if (r.parked && view?.state === "pending") return;
      if (probe && qualified && !r.usageWitness.failure && (!probe.window || !r.questionUnavailable) && (!probe.window || (!view?.grant.settlement_nudges && !view?.grant.question_deferrals && !probe.budgeted) || typeof r.ctx.abortPreservingQueue === "function") && probe.version === 6 && JSON.stringify(probe.binding) === JSON.stringify(r.bootstrap.binding)
        && (!probe.window || (probe.window === view?.window && probe.grant_hash === view?.grant_hash && probe.budgeted === (view?.batch?.shared_requests != null)))
        && (!probe.window || probe.usage_policy_hash === view?.usage_policy_hash)
        && (probe.usage_policy_hash == null || (r.usageWitness.acknowledge(view) && r.usageCollector.supports(r)))) {
        const ack = { ...probe, generation: r.generation, sdk: VERSION, coverage: COVERAGE, usage_coverage: USAGE_COVERAGE };
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
      const original = messages;
      messages = clean(messages);
      r.settling = false;
      if (r.interaction) await r.interaction;
      if (r.usageWitness.failure) { usageAbort(r, ctx, "usage_unavailable", true); return { messages }; }
      let question;
      try { question = await questions.guard(r, messages, ctx); }
      catch { question = await questions.block(r, ctx); }
      if (question.blocked || question.ordinary) return { messages };
      if (r.usageDenied || usageBlocked(r)) { usageAbort(r, ctx, r.usageDenied, true); return { messages }; }
      if (r.budgetDenied) { policyAbort(r, ctx); return { messages }; }
      const w = r.view;
      if (!qualified || (!r.owned && (r.parked || !w || w.state !== "pending" || !w.confirmed))) return;
      // Context is emitted after the prior tool batch settles, never in a tool callback.
      r.owned = true;
      if (!w || r.parked || r.failure || !observable(r) || performance.now() >= r.delegateDeadline || Date.now() >= w.expires_at_ms || ctx.signal?.aborted || ctx.hasPendingMessages() || w.generation !== r.generation || w.state === "ended") {
        await stop(r, "ended, interrupted or stale", true); return { messages };
      }
      try {
        if (r.externalMessages !== undefined && externalMessages(messages) !== r.externalMessages) {
          await stop(r, "new human or extension context", true); return { messages };
        }
        const expected = r.expectedContinuation;
        const continuation = expected ? original.filter(m => m.customType === CONTINUATION).at(-1) : undefined;
        if (expected) {
          if (!continuation || JSON.stringify(continuation.details) !== JSON.stringify(expected.identity) || continuation.content !== expected.content || JSON.stringify(messages) !== expected.before) {
            await stop(r, "late competing or changed continuation context", true); return { messages };
          }
        }
        const request = randomUUID();
        const receipt = await r.bridge.request({ op: "runtime", command: { kind: "reserve", window: w.window, request, continuation: r.expectedContinuation?.identity ?? null, question: question.identity ?? null } });
        if (!observable(r) || ctx.signal?.aborted || r.parked || performance.now() >= r.delegateDeadline || Date.now() >= w.expires_at_ms) { await stop(r, "interrupted reservation", true); return { messages }; }
        r.usageWitness.handoff(w);
        questions.reserved(r, question.identity);
        r.request = request;
        r.externalMessages = externalMessages(messages);
        r.expectedContinuation = undefined; r.contribution = undefined;
        return { messages: [...messages, ...(continuation ? [continuation] : []), { role: "custom", customType: "aoe-afk-delegation", content: `${REMINDER}\nHost grant: ${JSON.stringify(receipt)}`, display: false, timestamp: Date.now() }] };
      } catch (error) {
        if (usageDenial(error) || r.usageWitness.failure) { usageAbort(r, ctx, error.code ?? "usage_unavailable", true); return { messages }; }
        if (sharedDenial(error)) { policyAbort(r, ctx); return { messages }; }
        await stop(r, "reservation denied/failed", true, "failed"); return { messages };
      }
    },
  };
}

export default function (pi) { registerAfk(pi, { protocol: 6, delegation: delegationRuntime(pi) }); }
