import { createHash, randomUUID } from "node:crypto";

const CHANNEL = "ask:defer:v1:";
const MARKER = "aoe-afk-question-v1";
const kinds = ["input", "select", "custom-overlay"];
const digest = value => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const matches = (q, e) => e?.toolCallId === q?.toolCallId && e?.interactionId === q?.interactionId;
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const reference = value => typeof value === "string" && value.length > 0 && value.length <= 256 && !/[\x00-\x1f\x7f]/.test(value);
const validIdentity = q => q && uuid(q.operation) && uuid(q.interaction_id) && reference(q.tool_call_id) && reference(q.context_ref)
  && kinds.includes(q.kind) && typeof q.arguments_hash === "string" && /^[0-9a-f]{64}$/.test(q.arguments_hash);
const owner = (q, e) => e?.owner?.namespace === "pi-ask-user" && e.owner.id === q?.interactionId;
const ordinaryOutcome = a => !a.invalid && a.result === "ordinary" && ["answered", "cancelled"].includes(a.closed?.state)
  && (!a.reply || (a.reply.version === 1 && a.reply.method === "defer" && a.reply.status === "already_settled" && matches(a.observed, a.reply)));
const valid = e => e?.version === 1 && typeof e.toolCallId === "string" && e.toolCallId.length > 0 && e.toolCallId.length <= 256
  && typeof e.interactionId === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(e.interactionId)
  && kinds.includes(e.kind) && e.supported === true && e.uiPromptAttribution === true && owner(e, e) && e.state === "pending";

export function questionRuntime(pi, { current, stop, live, observable, supportedQuestion }) {
  const marker = (r, identity, phase, window = r.view?.window) => {
    if (!r.ctx.sessionManager.getSessionFile()) throw Error("persistent native session required");
    pi.appendEntry(MARKER, { version: 1, window, binding: r.bootstrap.binding, generation: r.generation, identity, phase });
  };
  const restore = r => {
    const guards = new Map(), resolved = new Set();
    for (const entry of r.ctx.sessionManager.getEntries()) {
      if (entry.type !== "custom" || entry.customType !== MARKER) continue;
      const m = entry.data;
      if (m?.version !== 1 || !validIdentity(m.identity) || !uuid(m.window) || !uuid(m.generation)
        || m.binding?.native_id !== r.bootstrap.binding.native_id || m.binding?.instance_id !== r.bootstrap.binding.instance_id
        || m.binding?.profile !== r.bootstrap.binding.profile || !uuid(m.binding?.launch_id)
        || !["intent", "observed", "ordinary", "recovered"].includes(m.phase)) { r.markerInvalid = true; continue; }
      const key = m.identity.operation;
      if (m.phase === "intent") {
        if (guards.has(key) || resolved.has(key)) r.markerInvalid = true;
        guards.set(key, m);
      } else {
        const old = guards.get(key);
        if (!old || old.window !== m.window || !same(old.identity, m.identity)) r.markerInvalid = true;
        else { guards.delete(key); resolved.add(key); }
      }
    }
    r.restoredGuards = guards; r.resolvedQuestions = resolved;
  };
  const rpc = (method, data, deadline) => new Promise((resolve, reject) => {
    const requestId = data?.requestId ?? randomUUID();
    const off = pi.events.on(CHANNEL + "reply", reply => {
      if (reply?.requestId !== requestId || reply.method !== method || reply.version !== 1) return;
      clearTimeout(timer); off(); resolve(reply);
    });
    const timer = setTimeout(() => { off(); reject(Error("question API deadline")); }, Math.max(0, Math.min(1000, deadline - Date.now())));
    pi.events.emit(CHANNEL + "request", { version: 1, method, requestId, ...data });
  });
  const eligible = r => r.questionObserved && !r.questionAttempt && !r.markerInvalid && !r.restoredGuards.size
    && !r.questionGuards?.length && !r.owned && !r.parked && !r.failure && live(r) && observable(r)
    && !r.ctx.hasPendingMessages() && r.questionObserved.signal === r.signal
    && r.ctx.mode === "tui" && typeof r.ctx.abortPreservingQueue === "function" && r.ctx.inputProvenance === "pipeline-v1"
    && r.ctx.ui.uiPromptOwnership === "exclusive-v1" && r.ctx.ui.targetedDialogCancellation?.includes(r.questionObserved.kind)
    && r.ctx.sessionManager.getSessionFile();
  const discover = async r => {
    if (!eligible(r)) throw Error("question unavailable: exact observed sole pending question and native capabilities required");
    const observed = r.questionObserved;
    const reply = await rpc("capabilities", {}, r.view.expires_at_ms);
    if (!eligible(r) || r.questionObserved !== observed || reply.status !== "capabilities" || reply.truncated !== false
      || !Array.isArray(reply.pending) || reply.pending.length !== 1 || !valid({ version: 1, ...reply.pending[0] })
      || !matches(observed, reply.pending[0]) || reply.pending[0].kind !== observed.kind) throw Error("question capability mismatch");
    return observed;
  };
  pi.on("message_end", (event, ctx) => {
    const r = current();
    if (!r || event.message.role !== "assistant") return;
    r.questionBatch = undefined; r.questionCall = undefined; r.questionObserved = undefined;
    if (r.owned) return;
    const tools = event.message.content.filter(c => c.type === "toolCall");
    if (tools.length !== 1 || tools[0].name !== "ask_user" || !supportedQuestion(tools[0].arguments)) return;
    const tool = tools[0];
    const occurrences = ctx.sessionManager.getEntries().filter(e => e.type === "message" && e.message.role === "assistant")
      .flatMap(e => e.message.content).filter(c => c.type === "toolCall" && c.id === tool.id).length;
    if (r.seenQuestionTools.has(tool.id) || r.seenQuestionTools.size >= 256 || occurrences > 1) return;
    r.seenQuestionTools.add(tool.id);
    r.questionBatch = { toolCallId: tool.id, arguments_hash: digest(tool.arguments) };
  });
  pi.on("tool_call", (event, ctx) => {
    const r = current(), batch = r?.questionBatch;
    if (r) { r.questionBatch = undefined; r.questionCall = undefined; }
    if (!r || r.owned || !batch || event.toolName !== "ask_user" || event.toolCallId !== batch.toolCallId || digest(event.input) !== batch.arguments_hash) return;
    const context_ref = ctx.sessionManager.getLeafId();
    if (context_ref && observable(r)) r.questionCall = { ...batch, context_ref, signal: ctx.signal };
  });
  let offs = [];
  const subscribe = () => { if (offs.length) return; offs = [pi.events.on(CHANNEL + "opened", event => {
    const r = current(), call = r?.questionCall;
    if (r && call?.toolCallId === event?.toolCallId) r.questionCall = undefined;
    if (!r || !call || !valid(event) || event.toolCallId !== call.toolCallId || r.questionObserved
      || call.signal.aborted || r.seenQuestionInteractions.has(event.interactionId)) return;
    r.seenQuestionInteractions.add(event.interactionId);
    r.questionObserved = { ...event, ...call };
  }), pi.events.on(CHANNEL + "closed", event => {
    const r = current(), a = r?.questionAttempt;
    if (a && event?.version === 1 && matches(a.observed, event)) {
      if ((a.closed && !same(a.closed, event)) || event.kind !== a.observed.kind || !owner(a.observed, event)
        || event.supported !== true || event.uiPromptAttribution !== true) a.invalid = true;
      a.closed = event; a.check?.();
    }
    if (r && matches(r.questionObserved, event)) r.questionObserved = undefined;
    if (r?.questionCall?.toolCallId === event?.toolCallId) r.questionCall = undefined;
  }), pi.events.on(CHANNEL + "reply", event => {
    const a = current()?.questionAttempt;
    if (!a || event?.requestId !== a.identity.operation) return;
    if (a.reply && !same(a.reply, event)) { a.invalid = true; void stop(current(), "contradictory question reply"); }
    a.reply = event; a.check?.();
  })]; };
  subscribe();
  pi.on("tool_result", event => {
    const a = current()?.questionAttempt;
    if (!a || event.toolName !== "ask_user" || event.toolCallId !== a.observed.toolCallId) return;
    const d = event.details;
    if (!event.isError && matches(a.observed, d) && digest(event.input) === a.identity.arguments_hash) {
      a.result = d.status === "no_human_answer" && d.reason === "deferred_by_afk" && d.response === null && d.cancelled === true ? "deferred"
        : !d.status && !d.reason && ((d.cancelled === false && d.response !== null) || (d.cancelled === true && d.response === null)) ? "ordinary" : "unknown";
    } else a.result = "unknown";
    a.check?.();
  });
  const attempt = async r => {
    let a;
    try {
      const observed = await discover(r);
      const identity = { operation: randomUUID(), tool_call_id: observed.toolCallId, interaction_id: observed.interactionId,
        kind: observed.kind, arguments_hash: observed.arguments_hash, context_ref: observed.context_ref };
      a = { observed, identity, window: r.view.window }; r.questionAttempt = a;
      const cmd = async command => {
        let timer;
        try {
          return await Promise.race([
            r.bridge.request({ op: "runtime", command: { window: a.window, identity, ...command } }),
            new Promise((_, reject) => { timer = setTimeout(() => reject(Error("question host deadline")), Math.max(0, Math.min(1500, r.view.expires_at_ms - Date.now()))); }),
          ]);
        } finally { clearTimeout(timer); }
      };
      const receipt = await cmd({ kind: "question_admit" });
      if (!same(receipt.identity, identity)) throw Error("question admission identity mismatch");
      const ready = () => r.view.window === a.window && !r.parked && !r.failure && live(r) && observable(r) && !r.ctx.hasPendingMessages()
        && r.questionObserved === observed && r.view.confirmed && r.view.state === "pending" && r.view.generation === r.generation;
      if (!ready()) throw Error("question admission interrupted");
      r.questionRecovery = undefined;
      marker(r, identity, "intent", a.window);
      a.intent = true;
      await cmd({ kind: "question_intent" });
      await r.refresh();
      if (!ready()) throw Error("question emission interrupted");
      a.done = new Promise(resolve => {
        a.check = () => {
          const ordinary = ordinaryOutcome(a);
          const accepted = a.result === "deferred" && a.closed?.state === "deferred" && a.end
            && a.reply?.version === 1 && a.reply?.method === "defer" && a.reply?.status === "accepted"
            && a.reply?.reason === "deferred_by_afk" && matches(observed, a.reply);
          if (ordinary || accepted || a.invalid) { clearTimeout(a.timer); resolve(ordinary ? "ordinary" : accepted && !a.invalid ? "accepted" : "unknown"); }
        };
        a.timer = setTimeout(() => resolve("unknown"), Math.max(0, Math.min(1200, r.view.expires_at_ms - Date.now())));
      });
      r.externalMessages = r.ordinaryExternal;
      pi.events.emit(CHANNEL + "request", { version: 1, requestId: identity.operation, method: "defer", toolCallId: observed.toolCallId, interactionId: observed.interactionId, reason: "deferred_by_afk" });
      a.outcome = await a.done;
      await cmd({ kind: "question_outcome", outcome: a.outcome });
      a.persisted = true;
      if (a.outcome === "ordinary") { marker(r, identity, "ordinary", a.window); a.intent = false; await stop(r, "ordinary question outcome won"); }
      else if (a.outcome !== "accepted") await stop(r, "question cleanup unknown");
    } catch { await stop(r, "question unavailable or outcome unknown", false, "failed"); }
  };
  const block = async (r, ctx) => {
    await stop(r, "unresolved controlled question return; explicit ordinary recovery required");
    if (typeof ctx.abortPreservingQueue === "function") ctx.abortPreservingQueue();
    else {
      r.questionWarning = "Unsupported restored question guard: dispatch blocked, NOT safe against native custom-queue loss; release blocked";
      ctx.abort();
      ctx.ui.setStatus("aoe-afk", r.questionWarning);
    }
    return { blocked: true };
  };
  return {
    block,
    attach(r) {
      subscribe(); r.seenQuestionTools = new Set(); r.seenQuestionInteractions = new Set();
      r.restoredGuards = new Map(); r.resolvedQuestions = new Set();
      try { restore(r); } catch { r.markerInvalid = true; }
    },
    shutdown(r) { for (const off of offs) off(); offs = []; clearTimeout(r.questionAttempt?.timer); r.questionAttempt && (r.questionAttempt.invalid = true); r.questionAttempt?.check?.(); },
    restoreGuards(r, guards) {
      if (guards != null && (!Array.isArray(guards) || guards.some(g => !validIdentity(g.identity) || !uuid(g.window)
        || typeof g.emission_intent !== "boolean" || g.binding?.native_id !== r.bootstrap.binding.native_id))) { r.markerInvalid = true; return; }
      r.questionGuards = guards ?? [];
    },
    newWindow(r) {
      if (!r.questionAttempt?.intent && !r.questionGuards?.length && !r.restoredGuards.size) { r.questionAttempt = undefined; r.questionTask = undefined; }
    },
    async poll(r) {
      if (r.view?.state !== "pending" || !r.view.grant.question_deferrals || r.parked) return;
      if (!r.questionAttempt && !r.questionTask) {
        try { await discover(r); } catch { r.questionUnavailable = true; await stop(r, "question deferral unavailable: observed identity/native capabilities required"); return; }
        if (r.view.confirmed) r.questionTask = attempt(r);
      }
    },
    end(r, event) {
      const a = r?.questionAttempt;
      if (a?.intent && owner(a.observed, event) && !a.invalid) { a.end = true; a.check?.(); return true; }
      return false;
    },
    input(r, event, ctx) {
      r.questionRecovery = undefined;
      if ((r.questionAttempt?.intent || r.restoredGuards.size || r.questionGuards?.length) && ctx.inputProvenance === "pipeline-v1" && event.provenance && event.source === "interactive" && ctx.isIdle() && !event.images?.length && event.text) {
        r.questionRecovery = { text: event.text, provenance: event.provenance, entries: new Set(ctx.sessionManager.getEntries().map(e => e.id)) };
      }
    },
    async guard(r, messages, ctx) {
      if (r.questionTask) await r.questionTask;
      const a = r.questionAttempt;
      const guards = [...r.restoredGuards.values(), ...(r.questionGuards ?? [])];
      const recovery = r.questionRecovery;
      r.questionRecovery = undefined;
      const proof = recovery?.provenance;
      if (proof?.source === "interactive" && proof.settled === true && proof.transformed === false && proof.handled === false && proof.failed === false
        && !r.markerInvalid && ctx.sessionManager.getEntries().some(e => !recovery.entries.has(e.id) && e.type === "message"
        && e.message.role === "user" && same(e.message.content, [{ type: "text", text: recovery.text }]))) {
        const unique = new Map(guards.map(g => [g.identity.operation, g]));
        if (a?.intent) unique.set(a.identity.operation, { identity: a.identity, window: a.window, emission_intent: true });
        for (const g of unique.values()) {
          try { await r.bridge.request({ op: "question_recover", window: g.window, identity: g.identity }); } catch { /* Native recovery never restores host authority. */ }
          if (!r.resolvedQuestions.has(g.identity.operation) && (g.emission_intent !== false || r.restoredGuards.has(g.identity.operation) || a?.intent)) marker(r, g.identity, "recovered", g.window);
          r.resolvedQuestions.add(g.identity.operation);
        }
        r.restoredGuards.clear(); r.questionGuards = []; if (a) a.intent = false;
        await stop(r, "explicit ordinary recovery"); return { ordinary: true };
      }
      if (!a?.intent && !guards.some(g => g.emission_intent !== false && !r.resolvedQuestions.has(g.identity.operation)) && !r.markerInvalid) return {};
      if (a?.intent && ordinaryOutcome(a) && !ctx.signal?.aborted) {
        try { await r.bridge.request({ op: "runtime", command: { kind: "question_outcome", window: a.window, identity: a.identity, outcome: "ordinary" } });
          marker(r, a.identity, "ordinary", a.window); a.intent = false; r.resolvedQuestions.add(a.identity.operation);
        } catch { /* Proven ordinary results do not acquire AFK authority. */ }
        await stop(r, "ordinary question outcome won"); return { ordinary: true };
      }
      if (a?.intent && a.persisted && a.outcome === "accepted" && !a.invalid && !r.parked && !r.failure && live(r) && observable(r)
        && r.view.confirmed && r.view.generation === r.generation && !ctx.hasPendingMessages()) return { identity: a.identity };
      return block(r, ctx);
    },
    reserved(r, identity) {
      if (!identity) return;
      marker(r, identity, "observed", r.questionAttempt.window); r.questionAttempt.intent = false;
      r.resolvedQuestions.add(identity.operation); r.questionGuards = []; r.restoredGuards.delete(identity.operation);
    },
  };
}
