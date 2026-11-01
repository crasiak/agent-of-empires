import { createHash, randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { createWorkPeriod } from "./aoe-afk-work-state.mjs";
import { createNativeWorkJournal } from "./aoe-afk-work-native.mjs";
import { ANSWER_TOOL, BASH_DEFAULT_TIMEOUT_S, askUserMenu, createAnswerClient, dialogMenu, dialogValue, promptLike, runInput, answeredBy, parseOnArgs, splitArgs, textTail } from "./aoe-afk-answer.mjs";

export const WORK_CHECKPOINT_TOOL = "aoe_afk_work_checkpoint";
export const MENU_NUDGE = "You ended with a menu. Resolve it with aoe_afk_answer.";
const START = "aoe-afk-work-start", CONTINUATION = "aoe-afk-work-continuation", INSTRUCTIONS = "aoe-afk-work-instructions";
const OWN = new Set([START, CONTINUATION, INSTRUCTIONS]);
const CONSENT_NAMESPACE = "aoe-afk-work-consent-v1";
// Pi drops a dialog answer after 5 s; decide inside that bound. An answered prompt must end within the delivery window.
const DIALOG_DECIDE_TIMEOUT_MS = 4000, DELIVERY_WINDOW_MS = 2000;
const LOCAL_TOOLS = ["read", "write", "edit", "bash"];
const uuid = x => typeof x === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(x);
const sameBinding = (a, b) => ["instance_id", "profile", "native_id", "launch_id"].every(k => a?.[k] === b?.[k]);
const hash = x => createHash("sha256").update(JSON.stringify(x)).digest("hex");
const keys = (value, expected) => value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).sort().join(",") === [...expected].sort().join(",");
const nativeExternal = ctx => JSON.stringify(ctx.sessionManager.getBranch().filter(e => e.type === "custom_message" && !OWN.has(e.customType)
  || e.type === "message" && ["user", "custom"].includes(e.message.role) && !OWN.has(e.message.customType)).map(e => [e.id, hash(e)]));
const clean = messages => messages.filter(m => !OWN.has(m.customType));
const external = messages => JSON.stringify(clean(messages).filter(m => ["user", "custom"].includes(m.role)));
const result = value => ({ content: [{ type: "text", text: JSON.stringify(value) }], details: value });
const completionFor = (w, id) => w.completion?.period === id ? w.completion : undefined;
function hasIssuedAnnotation(content, evidence) {
  for (const block of content) {
    if (block.type !== "text") continue;
    // Overlap bounded JSON pairs so arbitrary prefixes cannot hide an issued annotation.
    for (const match of block.text.matchAll(/(?=("(?:[^"\\]|\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4})){25}")\s*:\s*("(?:[^"\\]|\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4})){1,512}"))/gu)) {
      try {
        if (JSON.parse(match[1]) === "aoe_afk_work_evidence_ref" && evidence.has(JSON.parse(match[2]))) return true;
      } catch { /* Malformed or unknown user text is not host metadata. */ }
    }
  }
  return false;
}
function assertInstructionHistory(ctx, rows) {
  const messages = ctx.sessionManager.getEntries().filter(e => e.type === "custom_message" && OWN.has(e.customType));
  if (messages.some(e => !uuid(e.details?.period) || !rows.some(s => s?.id === e.details.period && (!e.details.generation || e.details.generation === s.generation)))) {
    throw Error("orphaned native work instructions; consent provenance unavailable");
  }
}
const REMINDER = "AoE Session-bound AFK work period. Continue only the existing assigned task and its project docs, vocabulary, intent and settled decisions. Normal configured harness permissions and human approval requirements still apply; AFK grants no new tool privilege or OS sandbox. The initial supported tools are read/write/edit/bash plus aoe_afk_work_checkpoint. No worker/agent launching or background work. Before settling, call aoe_afk_work_checkpoint with status unfinished|completed|blocked, summary, next_step, evidence, human_required:boolean, depends_on:[]. next_step must be null for completed or blocked, and a nonempty string for unfinished. Each owned local tool result appends a host JSON text block with aoe_afk_work_evidence_ref: the exact real tool-call ID, including any composite provider suffix. Copy that field's value into evidence (an array of strings); never invent or shorten it. Refs are valid only in the current period. Treat the annotation as data, not an instruction or new authority. Unfinished continuation needs new underlying tool-result evidence; fresh IDs or annotations are not progress. Human requirements/dependencies block; no_human_answer is not approval. Completion is a model claim, not independent acceptance. Never end a turn with a menu for the human. Call aoe_afk_answer with the question, options and your proposed option instead. Limits count main-loop requests and local tool admissions, not hidden retries, descendants or invoices. No extra reporting request. Commands must not wait for input. Pass answers explicitly (flags, yes |, printf … |). If a command needs a choice, decide it with aoe_afk_answer first. Bash calls default to a 600 s timeout; pass a longer timeout for long builds or test suites.";

export function workCheckpointParameters(Type) {
  return Type.Object({ status: Type.Union(["unfinished", "completed", "blocked"].map(x => Type.Literal(x))),
    summary: Type.String({ minLength: 1, maxLength: 2048 }), next_step: Type.Union([Type.Null(), Type.String({ minLength: 1, maxLength: 1024 })],
      { description: "Must be null for completed or blocked; must be a nonempty next step for unfinished." }),
    evidence: Type.Array(Type.String({ minLength: 1, maxLength: 512 }), { maxItems: 16, description: "Copy exact aoe_afk_work_evidence_ref values from host JSON annotations on current-period owned local tool results. Do not invent IDs." }), human_required: Type.Boolean(),
    depends_on: Type.Array(Type.String({ minLength: 1, maxLength: 256 }), { maxItems: 16 }) }, { additionalProperties: false });
}

export function workPeriodRuntime(pi, { version, parameters, answerParameters: answerParams, wall = Date.now, mono, journalFactory = createNativeWorkJournal } = {}) {
  let current, uiDepth = 0;
  const protectedWork = w => !!w && (w.orphanFailure || w.period?.guarded);
  const forgetSignal = w => {
    w?.signal?.removeEventListener("abort", w.onAbort);
    if (w) { w.signal = undefined; w.onAbort = undefined; }
  };
  const owned = (w, ctx = w?.ctx) => !!w?.period?.active && w.period.snapshot()?.status === "owned" && w.signal && !w.signal.aborted
    && ctx?.signal === w.signal && !ctx.isIdle();
  const stop = (w, reason, status = "interrupted") => {
    if (!w) return;
    try { w.period?.stop(reason, status); } catch (error) { w.orphanFailure = error.message; }
    w.expected = undefined;
  };
  const abort = (w, ctx = w?.ctx) => {
    if (!ctx) return;
    if (typeof ctx.abortPreservingQueue === "function") ctx.abortPreservingQueue();
    else {
      w.orphanFailure ??= "Unsupported work-period restoration: dispatch blocked; native custom-queue preservation NOT qualified";
      ctx.ui?.setStatus?.("aoe-afk", w.orphanFailure);
      ctx.abort();
    }
  };
  const legacyProtected = r => !!r && (r.owned || r.budgetDenied || r.usageDenied || r.usageWitness?.pending || r.usageWitness?.failure
    || r.questionAttempt?.intent || r.markerInvalid || r.restoredGuards?.size || r.questionGuards?.length || r.view && r.view.state !== "ended");
  const recover = (w, ctx, messages) => {
    const candidate = w?.recovery;
    if (w) w.recovery = undefined;
    if (!protectedWork(w) || !candidate || w.orphanFailure || !w.period) return false;
    const p = candidate.proof;
    const newUser = ctx.sessionManager.getEntries().some(e => !candidate.entries.has(e.id) && e.type === "message" && e.message.role === "user"
      && JSON.stringify(e.message.content) === JSON.stringify([{ type: "text", text: candidate.text }]));
    if (p.source !== "interactive" || p.settled !== true || p.transformed !== false || p.handled !== false || p.failed !== false || !newUser) return false;
    try {
      w.period.recover({ interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true });
      w.evidence.clear(); w.calls.clear(); forgetSignal(w);
      w.lastExternal = external(messages); return true;
    } catch { w.orphanFailure = "Native ordinary-recovery write unverified"; return false; }
  };
  const build = (ctx, binding, generation, r) => {
    const w = { ctx, binding, generation, r, evidence: new Map(), calls: new Map(), pendingPrompts: new Set(), controls: new Map(), expected: undefined, recovery: undefined, initialIssued: false,
      answers: { rules: 0, yolo: 0, stops: 0 }, client: r ? createAnswerClient(r.bridge) : undefined };
    try {
      const journal = journalFactory(pi, ctx), rows = journal.load();
      assertInstructionHistory(ctx, rows);
      if (!rows.length && (!uuid(binding.native_id) || !uuid(binding.launch_id))) w.disabled = "UUID native and launch identity required";
      else w.period = createWorkPeriod({ binding, generation, journal, wall, ...(mono ? { mono } : {}) });
    } catch (error) { w.orphanFailure = error.message; }
    return w;
  };
  const eligibility = w => {
    if (!w?.r) return "No current AoE direct-launch binding";
    if (w.disabled) return w.disabled;
    if (w.pendingConsent) return "Native confirmation is open; no work authority exists before an explicit answer";
    if (version !== "1.2.0" || typeof w.ctx.abortPreservingQueue !== "function" || typeof w.ctx.endRunPreservingQueue !== "function"
      || w.ctx.inputProvenance !== "pipeline-v1" || w.ctx.ui.uiPromptOwnership !== "exclusive-v1") return "Requires qualified preserving-stop, settled-input and exclusive UI lifecycle hooks; this runtime is not qualified";
    if (!w.ctx.hasUI || w.ctx.mode !== "tui" || w.ctx.sessionManager.getSessionId() !== w.binding.native_id || !uuid(w.binding.native_id)) return "Exact native interactive Session required";
    const file = w.ctx.sessionManager.getSessionFile();
    if (!file || !existsSync(file)) return "Persistent assigned Session required; start the task first";
    if (w.orphanFailure || w.period?.failure) return "Native work-period evidence unavailable; inspect retained history";
    if (!w.period) return "Native work-period state unavailable";
    if (w.period.guarded) return w.period.active ? "A work period is already active" : "Fresh ordinary interactive input required; no automatic restoration";
    if (legacyProtected(w.r)) return "Legacy delegation or unresolved native/host guard is present";
    if (uiDepth || w.ctx.hasPendingMessages()) return "Human prompt or queued work present";
    if (typeof w.ctx.ui.getEditorText !== "function" || w.ctx.ui.getEditorText() !== "") return "Observable empty native editor required; existing human draft takes precedence";
    if (w.r.workPeriodHost !== 1 || ["invalidated", "unavailable"].includes(w.r.state)) return "Current host work-period protocol unavailable";
    if (w.lastExternal === undefined || !w.ctx.sessionManager.getBranch().some(e => e.type === "message" && e.message.role === "user")) return "Start the assigned task first so its context is observable";
    if (!pi.getActiveTools().includes(WORK_CHECKPOINT_TOOL)) return "Required aoe_afk_work_checkpoint tool is not active; no work authority created";
    if (!LOCAL_TOOLS.some(t => pi.getActiveTools().includes(t))) return "No supported normal local tools are active";
    return null;
  };
  const projection = w => {
    const s = w?.period?.snapshot();
    if (!s || !sameBinding(s.binding, w.binding)) return null;
    return { id: s.id, consent: s.consent, active: w.period.active, guarded: w.period.guarded, status: s.status, reason: s.reason, expires_at_ms: s.expires_at_ms,
      request_limit: s.request_limit, requests_used: s.requests_used, tool_limit: s.tool_limit, tools_used: s.tools_used,
      summary: s.checkpoint?.summary ?? null, next_step: s.checkpoint?.next_step ?? null,
      completion_basis: s.status === "completed" ? "model_claim; inspect artifacts and validation" : null,
      settlement_status: completionFor(w, s.id)?.proved ? "proved" : completionFor(w, s.id) ? "pending" : "not_proved",
      policy: s.policy, answers: { ...w.answers } };
  };
  const reply = (w, request, detail = eligibility(w), outcome = "inspected") => ({ version: 1, id: request.id, binding: { ...w.binding }, generation: w.generation, control_revision: w.r.revision,
    observed_at_ms: wall(), sdk: version, outcome, eligible: detail === null, detail: detail ?? "Uses current task/context and existing local-tool permissions; no new privilege", period: projection(w) });
  const startEntry = w => ({ type: "custom_message", customType: START,
    content: `[AoE AFK operator-started work period] Continue the existing assigned task within the confirmed period. ${REMINDER}`,
    display: true, details: { period: w.period.snapshot().id, generation: w.generation } });
  const handle = async (w, request) => {
    if (!keys(request, ["version", "id", "binding", "kind", "generation", "period_id", "issued_at_ms", "expires_at_ms"])
      || request.version !== 1 || !uuid(request.id) || !keys(request.binding, ["instance_id", "profile", "native_id", "launch_id"]) || !sameBinding(request.binding, w.binding)
      || !["inspect", "stop"].includes(request.kind) || !Number.isSafeInteger(request.issued_at_ms)
      || !Number.isSafeInteger(request.expires_at_ms) || request.expires_at_ms <= request.issued_at_ms
      || request.expires_at_ms - request.issued_at_ms > 5000 || wall() < request.issued_at_ms - 1000 || wall() >= request.expires_at_ms
      || (request.kind === "inspect" ? request.generation !== null || request.period_id !== null : !uuid(request.generation) || request.period_id !== null && !uuid(request.period_id))) return;
    const signature = hash(request), old = w.controls.get(request.id);
    if (old) return old.signature === signature ? old.reply : reply(w, request, "Conflicting operator command identity; no retry", "refused");
    if (request.kind === "inspect") return reply(w, request);
    if (request.generation !== w.generation) return reply(w, request, "Runtime changed; no replacement target selected", "refused");
    if (w.controls.size >= 128) return reply(w, request, "Operator command capacity reached", "refused");
    let value;
    try {
      if (request.period_id !== null && request.period_id !== w.period?.snapshot()?.id) value = reply(w, request, "Observed period changed; no replacement target selected", "refused");
      else {
        if (w.pendingConsent) w.pendingConsent.cancelled = true;
        stop(w, "operator_stop"); if (!uiDepth && w.signal && !w.signal.aborted) abort(w);
        value = w.orphanFailure || w.period?.failure
          ? reply(w, request, "Dispatch blocked; native stop publication unverified; inspect retained history", "unknown")
          : reply(w, request, "Work authority withdrawn; in-flight admitted effects may already have occurred. Open prompts are not answered", "stopped");
      }
    } catch {
      w.orphanFailure = "Native stop publication unverified";
      if (w.signal && !w.signal.aborted) abort(w);
      value = reply(w, request, "Stop unverified; inspect retained state; no automatic retry", "unknown");
    }
    w.controls.set(request.id, { signature, reply: value }); return value;
  };

  const answerCounts = a => `answers: ${a.rules + a.yolo} (rules ${a.rules}, yolo ${a.yolo}), stops: ${a.stops}`;
  pi.registerCommand("afk-work", { description: "Inspect, stop, or explicitly confirm a Session-bound local work period: /afk-work on [--policy rules|yolo] [--rule '<toml>']… [minutes] [requests]",
    async handler(args, ctx) {
      const tokens = splitArgs(args), action = tokens.shift() ?? "status", w = current;
      const notify = (text, kind = "info") => ctx.ui.notify(text, kind);
      if (action === "status" && !tokens.length) {
        const p = w?.r && projection(w);
        notify(w?.r ? (p ? `${JSON.stringify(p)}\n${answerCounts(p.answers)}` : `No work period. ${eligibility(w) ?? "Ready for explicit native confirmation"}`) : "No current AoE work-period binding"); return;
      }
      if (action === "off" && !tokens.length) {
        if (!w) { notify("No work period"); return; }
        if (w.pendingConsent) w.pendingConsent.cancelled = true;
        stop(w, "operator_stop"); if (!uiDepth && w.signal && !w.signal.aborted) abort(w);
        notify(w.orphanFailure || w.period?.failure ? "AFK dispatch blocked; native stop publication unverified. Inspect retained history"
          : "AFK work authority withdrawn; in-flight effects may already have occurred. Existing prompts are not answered"); return;
      }
      if (action !== "on") { notify("Usage: /afk-work [status|off|on [--policy rules|yolo] [--rule '<toml>']… [minutes] [requests]]", "warning"); return; }
      const parsed = parseOnArgs(tokens);
      if (parsed.error) { notify(`${parsed.error}; no authority created`, "warning"); return; }
      if (!w?.r || w.pendingConsent) { notify(w?.pendingConsent ? "Native confirmation already open" : "No current AoE work-period binding", "warning"); return; }
      await w.r.refresh?.();
      const denied = eligibility(w);
      if (denied) { notify(denied, "warning"); return; }
      let ruleInfo = { count: 0, sources: [], format_warnings: [] };
      if (parsed.policy === "rules") {
        try { ruleInfo = await w.client.rulesCheck({ period: randomUUID(), item: null, goal: "consent", policy: "rules", rules: parsed.rules, cwd: ctx.cwd, requests_used: 0, request_limit: 1 }); }
        catch { notify("AFK rules could not be checked (engine unavailable); no authority created", "warning"); return; }
        if (!ruleInfo?.valid) { notify(`AFK rules invalid; no authority created: ${ruleInfo?.error}`, "warning"); return; }
        if (!/^[0-9a-f]{64}$/.test(ruleInfo.digest ?? "")) { notify("AFK rules could not be pinned (engine too old); no authority created", "warning"); return; }
      }
      const sources = Array.isArray(ruleInfo.sources) ? ruleInfo.sources : [], formatWarnings = Array.isArray(ruleInfo.format_warnings) ? ruleInfo.format_warnings : [];
      const consent = { revision: w.r.revision, external: w.lastExternal, native: nativeExternal(ctx), tools: LOCAL_TOOLS.filter(t => pi.getActiveTools().includes(t)), cancelled: false,
        owner: { namespace: CONSENT_NAMESPACE, id: randomUUID() } };
      w.pendingConsent = consent;
      const show = async (kind, run) => {
        let resolve, timer, value, fault;
        const closed = new Promise(done => resolve = done), dialog = { kind, started: false, resolve: value => resolve(value) };
        consent.dialog = dialog;
        try { value = await run({ owner: consent.owner }); } catch (error) { fault = error; }
        try {
          const ended = await Promise.race([closed, new Promise(done => { timer = setTimeout(() => done(false), 1500); })]);
          if (!ended || fault) throw Error("native owned dialog closure unavailable");
          return value;
        } finally { clearTimeout(timer); if (consent.dialog === dialog) consent.dialog = undefined; }
      };
      let confirmed = false, minutes, requests;
      try {
        const duration = parsed.minutes ?? await show("input", options => ctx.ui.input("AFK work period: minutes (1..1440)", "120", options));
        if (duration === undefined || consent.cancelled || current !== w) { notify("Cancelled; no work authority created"); return; }
        const allowance = parsed.requests ?? await show("input", options => ctx.ui.input("AFK work period: main-loop requests (1..512)", "64", options));
        if (allowance === undefined || consent.cancelled || current !== w) { notify("Cancelled; no work authority created"); return; }
        minutes = /^[1-9][0-9]*$/.test(duration) ? Number(duration) : 0;
        requests = /^[1-9][0-9]*$/.test(allowance) ? Number(allowance) : 0;
        if (!Number.isSafeInteger(minutes) || minutes < 1 || minutes > 1440 || !Number.isSafeInteger(requests) || requests < 1 || requests > 512) {
          notify("Use 1..1440 minutes and 1..512 main-loop requests; no authority created", "warning"); return;
        }
        confirmed = await show("confirm", options => ctx.ui.confirm("Start AFK work on the current Session task?",
          `${minutes} minutes; ${requests} main-loop requests; ${Math.min(requests * 16, 4096)} local tool admissions.\nUse only ${consent.tools.join("/")} and work checkpoints. Existing permissions and approval gates remain. No worker launching; this is NOT an OS sandbox or a descendant/billing cap.\nContinues while this managed agent lives, even with AoE detached. No automatic restart/restoration. Stops for completion claims, human/dependency gates, no progress, Off or limits.\nConfirming here is the start consent; AoE transport cannot grant work.`
            + `\npolicy: ${parsed.policy}; rules: ${ruleInfo.count ?? 0}${sources.length ? ` (${sources.join(", ")})` : ""}`
            + (parsed.policy === "yolo" ? "\nYOLO: AFK will answer every menu, including permission prompts. Scope checks only warn." : "")
            + (formatWarnings.length ? `\nSkipped menu formats: ${formatWarnings.length}` : ""), options)) === true;
      } catch { notify("Native confirmation unavailable; no work authority created", "warning"); return; }
      finally { if (w.pendingConsent === consent) w.pendingConsent = undefined; }
      if (!confirmed) { notify("Cancelled; no work authority created"); return; }
      await w.r?.refresh?.();
      const changed = current !== w || !w.r ? "Native runtime changed" : consent.cancelled ? "Confirmation interrupted"
        : consent.revision !== w.r.revision ? "Host control changed" : consent.external !== w.lastExternal || consent.native !== nativeExternal(ctx) ? "Task context changed"
        : JSON.stringify(consent.tools) !== JSON.stringify(LOCAL_TOOLS.filter(t => pi.getActiveTools().includes(t))) ? "Active local tools changed" : eligibility(w);
      if (changed) { notify(`${changed}; no work authority created. Inspect before another explicit start`, "warning"); return; }
      try {
        try {
          w.period.start({ id: randomUUID(), minutes, requests, task: "Continue the existing assigned Session task using its current context", tools: consent.tools,
            policy: parsed.policy, rules: parsed.rules, rules_digest: parsed.policy === "rules" ? ruleInfo.digest : null });
        } catch (error) {
          // A refused grant writes nothing; only an unverified write needs the stop path below.
          if (error.message === "work-period grant too large" && !w.period.failure) { notify("AFK rules too large for the work-period record; no authority created", "warning"); return; }
          throw error;
        }
        w.completion = undefined; w.evidence.clear(); w.calls.clear(); w.pendingPrompts.clear(); w.answers = { rules: 0, yolo: 0, stops: 0 };
        w.controlRevision = w.r.revision; w.baseline = consent.external; w.nativeBaseline = consent.native; w.initialIssued = false;
        if (w.ctx.isIdle()) {
          w.initialIssued = true; const entry = startEntry(w);
          pi.sendMessage({ customType: entry.customType, content: entry.content, display: entry.display, details: entry.details }, { triggerTurn: true });
        }
        notify("AFK work period started; current task and normal permissions only. Completion remains a model claim");
      } catch {
        stop(w, "native_start_unverified", "blocked");
        if (w.signal && !w.signal.aborted) abort(w);
        notify("Native admission/dispatch unverified; inspect retained history, no automatic retry", "error");
      }
    },
  });

  pi.registerTool({ name: WORK_CHECKPOINT_TOOL, label: "AFK work checkpoint", description: REMINDER, parameters, executionMode: "sequential",
    async execute(_id, input, _signal, _update, ctx) {
      const w = current;
      if (!owned(w, ctx)) throw Error("No owned AFK work-period request");
      // A result that ended in an unanswered prompt is not progress yet.
      const ignored = input.evidence.filter(id => w.pendingPrompts.has(id));
      const cleaned = { ...input, evidence: input.evidence.filter(id => !w.pendingPrompts.has(id)) };
      const s = w.period.checkpoint(cleaned, [...w.evidence].map(([id, observation]) => ({ id, fingerprint: observation.fingerprint })));
      return result({ status: s?.status, reason: s?.reason, summary: s?.checkpoint?.summary,
        basis: "model claims and observed tool-result fingerprints, not independent task acceptance",
        ...(ignored.length ? { ignored_evidence: ignored, ignored_reason: "prompt_seen_not_answered" } : {}) });
    },
  });
  pi.registerTool({ name: ANSWER_TOOL, label: "AFK answer", description: "Resolve a choice you would otherwise ask the human. The AFK engine decides under the run policy; the result says who answered.",
    parameters: answerParams, executionMode: "sequential",
    async execute(_id, input, _signal, _update, ctx) {
      const w = current;
      if (!owned(w, ctx) || !w.client) throw Error("No owned AFK work-period request");
      const s = w.period.snapshot();
      const menu = { source: "agent_text", format: "tool.aoe_afk_answer", kind: "choice", question: input.question,
        options: input.options.map(o => ({ key: o.key, label: o.label })), raw: JSON.stringify(input) };
      const proposal = typeof input.proposal === "string" ? { key: input.proposal, reason: input.reason } : { reason: input.reason };
      const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu, proposal });
      if (!owned(w, ctx)) {
        if (decision.decision === "answer") void w.client.delivered(s.id, answer_id, "failed");
        throw Error("AFK work period ended before the answer was delivered");
      }
      if (decision.decision !== "answer") {
        w.answers.stops++; stop(w, "menu_stopped", "blocked");
        return result({ stopped: decision.reason });
      }
      w.answers[s.policy]++; w.pendingPrompts.clear();
      const value = { answered_by: answeredBy(s.policy), choice: decision.choice ?? null, rule: decision.rule ?? null, reason: decision.reason };
      void w.client.delivered(s.id, answer_id, "ok");
      if (input.proposal) w.lastAnswerAt = wall();
      return result(value);
    },
  });
  pi.on("session_start", (_event, ctx) => {
    forgetSignal(current); current = undefined; uiDepth = 0;
    try {
      const journal = journalFactory(pi, ctx), rows = journal.load();
      assertInstructionHistory(ctx, rows);
      if (!rows.length) return;
      const last = rows.at(-1), binding = { instance_id: last?.binding?.instance_id ?? "unavailable", profile: last?.binding?.profile ?? "unavailable",
        native_id: ctx.sessionManager.getSessionId(), launch_id: randomUUID() };
      current = build(ctx, binding, randomUUID(), undefined);
    } catch { current = { ctx, orphanFailure: "Native work-period provenance unavailable" }; }
  });
  pi.on("session_shutdown", () => { stop(current, "shutdown"); });
  pi.on("tool_call", async (event, ctx) => {
    const w = current;
    if ([WORK_CHECKPOINT_TOOL, ANSWER_TOOL].includes(event.toolName) && !owned(w, ctx)) return { block: true, reason: "No owned AFK work period" };
    if (!protectedWork(w)) return;
    if (!w.period?.active || w.period.snapshot().status !== "owned") {
      if (!w.period?.active) return { block: true, reason: "AFK work period ended/guarded; fresh ordinary input required" };
      return; // The pre-existing ordinary batch is not retroactively owned.
    }
    if (!owned(w, ctx)) { stop(w, "operation_changed"); abort(w, ctx); return { block: true, reason: "AFK operation identity changed" }; }
    if (!pi.getActiveTools().includes(WORK_CHECKPOINT_TOOL)) {
      stop(w, "checkpoint_unavailable", "blocked"); abort(w, ctx); return { block: true, reason: "Required aoe_afk_work_checkpoint tool is not active" };
    }
    if (event.toolName === WORK_CHECKPOINT_TOOL || event.toolName === ANSWER_TOOL) return;
    if (event.toolName === "ask_user") {
      const menu = askUserMenu(event.input), s = w.period.snapshot();
      if (menu && w.client) {
        const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu });
        if (!owned(w, ctx)) {
          if (decision.decision === "answer") void w.client.delivered(s.id, answer_id, "failed");
          stop(w, "operation_changed"); abort(w, ctx); return { block: true, reason: "AFK operation identity changed" };
        }
        if (decision.decision === "answer") {
          w.answers[s.policy]++;
          void w.client.delivered(s.id, answer_id, "ok");
          const label = menu.options.find(o => o.key === decision.choice)?.label ?? null;
          return { block: true, reason: JSON.stringify({ answered_by: answeredBy(s.policy), choice: decision.choice, label, rule: decision.rule ?? null, reason: decision.reason }) };
        }
        w.answers.stops++;
      }
      // Fall through: allowTool("ask_user") stops human_required, and the block reason stays no_human_answer.
    }
    try {
      if (w.period.allowTool(event.toolName)) {
        if (typeof event.toolCallId !== "string" || !event.toolCallId.length || Buffer.byteLength(event.toolCallId) > 512
          || /[\u0000-\u001f\u007f]/u.test(event.toolCallId) || w.calls.has(event.toolCallId) || w.calls.size >= 256
          || Buffer.byteLength(JSON.stringify(event.input)) > 1048576) {
          stop(w, "evidence_unavailable", "blocked");
        } else {
          // Before the clone, so the recorded evidence matches what ran.
          if (event.toolName === "bash" && event.input && event.input.timeout === undefined) event.input.timeout = BASH_DEFAULT_TIMEOUT_S;
          w.calls.set(event.toolCallId, { name: event.toolName, input: structuredClone(event.input), period: w.period.snapshot().id, signal: ctx.signal }); return;
        }
      }
    } catch { w.orphanFailure = "Native tool-admission write unverified"; }
    abort(w, ctx);
    return { block: true, reason: event.toolName === "ask_user" ? "AFK: no_human_answer; a new human decision is required, no answer inferred"
      : "AFK local-tool admission unavailable; worker/orchestration tools are not qualified" };
  });
  pi.on("tool_result", async (event, ctx) => {
    const w = current, call = w?.calls.get(event.toolCallId);
    if (!call || !owned(w, ctx) || call.period !== w.period.snapshot().id || call.signal !== ctx.signal) return;
    try {
      if (event.toolName !== call.name || !Array.isArray(event.content) || typeof event.isError !== "boolean"
        || event.content.some(b => !b || (b.type === "text" ? typeof b.text !== "string"
          : b.type !== "image" || typeof b.data !== "string" || typeof b.mimeType !== "string"))) throw Error("malformed tool evidence");
      const observed = { name: call.name, input: call.input, content: event.content, details: event.details ?? null, isError: event.isError };
      const encoded = JSON.stringify(observed), old = w.evidence.get(event.toolCallId);
      if (Buffer.byteLength(encoded) > 1048576 + 4096 || w.evidence.size >= 256 && !old) throw Error("tool evidence capacity");
      const fingerprint = hash(observed);
      // Exact same-call raw/annotated replay is idempotent; issued refs elsewhere are uncertain.
      if (old?.annotatedFingerprint === fingerprint) return;
      if (old ? old.fingerprint !== fingerprint : hasIssuedAnnotation(event.content, w.evidence)) throw Error("contradictory tool evidence");
      if (Buffer.byteLength(encoded) > 1048576) throw Error("tool evidence capacity");
      const content = [...event.content, { type: "text", text: JSON.stringify({ aoe_afk_work_evidence_ref: event.toolCallId }) }];
      if (call.name === "bash") {
        const tail = promptLike(textTail(event.content.filter(b => b.type === "text").map(b => b.text).join("\n")));
        if (tail) {
          const menu = await w.client?.recognise("bash_prompt", tail);
          if (!owned(w, ctx) || w.calls.get(event.toolCallId) !== call) return;
          if (!menu) {
            // decide with text saves the unrecognised sample; an unrecognised wait always stops.
            await w.client?.decide({ run: runInput(w.period.snapshot(), ctx.cwd), text: { source: "bash_prompt", text: tail } });
            if (!owned(w, ctx) || w.calls.get(event.toolCallId) !== call) return;
            w.answers.stops++; stop(w, "unrecognised_menu", "blocked"); abort(w, ctx);
            return;
          }
          w.pendingPrompts.add(event.toolCallId);
          content.push({ type: "text", text: JSON.stringify({ aoe_afk_prompt_seen: menu }) });
        }
      }
      w.evidence.set(event.toolCallId, { fingerprint, annotatedFingerprint: hash({ ...observed, content }) });
      return { content };
    } catch { stop(w, "evidence_unavailable", "blocked"); }
  });
  pi.on("turn_end", (event, ctx) => {
    const w = current;
    if (!protectedWork(w) || w.period?.active || completionFor(w, w.period?.snapshot()?.id) || w.recovery || uiDepth || !w.signal || w.signal.aborted
      || ctx.signal !== w.signal || event.outcome !== "completed") return;
    const ticket = ctx.endRunPreservingQueue?.();
    if (ticket?.version !== 1 || typeof ticket.isCurrent !== "function" || typeof ticket.completion?.then !== "function") {
      w.orphanFailure = "Qualified terminal settlement unavailable"; abort(w, ctx); return;
    }
    const completion = { period: w.period.snapshot().id, ticket, proved: false }; w.completion = completion;
    const stillCurrent = () => current === w && w.completion === completion && completionFor(w, w.period.snapshot()?.id) === completion;
    completion.done = (async () => {
      try {
        const success = await ticket.completion;
        if (!stillCurrent()) return;
        if (success && ticket.isCurrent()) completion.proved = true;
        else w.orphanFailure = "Terminal settlement not proved; retained work remains guarded";
      } catch { if (stillCurrent()) w.orphanFailure = "Terminal settlement evidence unavailable"; }
    })();
  });
  pi.on("input", (event, ctx) => {
    const w = current;
    if (!protectedWork(w)) return;
    stop(w, "human_input"); w.recovery = undefined;
    if (ctx.inputProvenance === "pipeline-v1" && event.source === "interactive" && event.provenance && event.text && !event.images?.length) {
      w.recovery = { text: event.text, proof: event.provenance, entries: new Set(ctx.sessionManager.getEntries().map(e => e.id)) };
    }
  });
  pi.on("user_bash", () => { stop(current, "human_bash"); });
  pi.on("ui_prompt_respond", async (event, ctx) => {
    const w = current;
    if (!owned(w, ctx) || !w.client || w.pendingConsent || event.owner?.namespace === CONSENT_NAMESPACE) return;
    const menu = dialogMenu(event);
    if (!menu) return;
    const s = w.period.snapshot();
    const { answer_id, decision } = await w.client.decide({ run: runInput(s, ctx.cwd), menu, timeoutMs: DIALOG_DECIDE_TIMEOUT_MS });
    if (decision.decision !== "answer") { w.answers.stops++; return; }
    if (!owned(w, ctx) || w.answering) { void w.client.delivered(s.id, answer_id, "failed"); return; }
    const value = dialogValue(event, menu, decision);
    if (value === undefined) { w.answers.stops++; void w.client.delivered(s.id, answer_id, "failed"); return; }
    w.answers[s.policy]++;
    w.answering = { answer_id, period: s.id, policy: s.policy, by: answeredBy(s.policy), timer: undefined };
    return { value, answered_by: w.answering.by };
  });
  pi.on("ui_prompt_start", event => {
    const w = current, a = w?.answering;
    if (a && !a.timer) {
      if (event.answered_by === a.by) {
        a.timer = setTimeout(() => {
          if (current !== w || w.answering !== a) return;
          w.answering = undefined;
          void w.client.delivered(a.period, a.answer_id, "unknown");
          stop(w, "delivery_unknown", "blocked"); if (w.signal && !w.signal.aborted) abort(w);
        }, DELIVERY_WINDOW_MS);
        return;
      }
      // Pi discarded the answer (its bound passed, or another prompt opened first); this prompt is the human's.
      w.answering = undefined; if (w.period?.snapshot()?.id === a.period) w.answers[a.policy]--;
      void w.client.delivered(a.period, a.answer_id, "failed");
    }
    uiDepth++;
    const consent = current?.pendingConsent;
    if (consent?.dialog && event.owner?.namespace === consent.owner.namespace && event.owner.id === consent.owner.id && event.kind === consent.dialog.kind) consent.dialog.started = true;
    else if (consent) consent.cancelled = true;
    stop(current, "human_prompt", "blocked");
  });
  pi.on("ui_prompt_end", (event, ctx) => {
    const a = current?.answering;
    if (a?.timer && event.answered_by === a.by) {
      current.answering = undefined; clearTimeout(a.timer);
      void current.client.delivered(a.period, a.answer_id, "ok");
      return;
    }
    uiDepth = Math.max(0, uiDepth - 1);
    const consent = current?.pendingConsent;
    if (consent?.dialog) {
      const owned = consent.dialog.started && event.owner?.namespace === consent.owner.namespace && event.owner.id === consent.owner.id && event.kind === consent.dialog.kind;
      if (!owned) consent.cancelled = true;
      consent.dialog.resolve(owned);
    }
    if (!uiDepth && protectedWork(current) && current.signal && !current.signal.aborted) abort(current, ctx);
  });
  pi.on("agent_before_settle", async (event, ctx) => {
    const w = current;
    if (!w?.period?.active) return;
    const competing = event.outcome !== "completed" || event.continue || event.entries.length > 0 || event.context.pendingMessages.length > 0 || ctx.hasPendingMessages() || uiDepth > 0;
    if (competing) { stop(w, "competing_work"); return; }
    try {
      w.period.observe();
      if (!w.period.active) return;
      if (w.period.snapshot().status === "pending" && !w.initialIssued) { w.initialIssued = true; return { entries: [...event.entries, startEntry(w)], continue: true }; }
      const s = w.period.snapshot();
      if (w.client && s.status === "owned" && s.checkpoint?.status !== "unfinished" && s.menu_nudges === 0) {
        const last = [...event.context.contextMessages].reverse().find(m => m.role === "assistant");
        const textValue = (last?.content ?? []).filter(b => b.type === "text").map(b => b.text).join("\n");
        if (textValue && await w.client.recognise("agent_text", textTail(textValue))) {
          const nudge = w.period.nudge(MENU_NUDGE);
          if (nudge.continue) {
            const content = `[AoE bounded work continuation] ${MENU_NUDGE} No new scope, privilege or allowance. ${REMINDER}`;
            w.expected = { ...nudge, before: JSON.stringify(clean(event.context.contextMessages).filter(m => m.role !== "system")), content };
            return { entries: [...event.entries, { type: "custom_message", customType: CONTINUATION, content, display: true,
              details: { period: nudge.id, request: nudge.request, admission: nudge.admission } }], continue: true };
          }
        }
      }
      const next = w.period.settle();
      if (!next.continue) return;
      const content = `[AoE bounded work continuation] ${next.next_step}. No new scope, privilege or allowance. ${REMINDER}`;
      w.expected = { ...next, before: JSON.stringify(clean(event.context.contextMessages).filter(m => m.role !== "system")), content };
      return { entries: [...event.entries, { type: "custom_message", customType: CONTINUATION,
        content,
        display: true, details: { period: next.id, request: next.request, admission: next.admission } }], continue: true };
    } catch { w.orphanFailure = "Native continuation admission unverified"; abort(w, ctx); }
  });
  pi.on("agent_settled", () => { stop(current, "settled"); forgetSignal(current); if (current) current.expected = undefined; });
  pi.on("cache_warming_decision", () => protectedWork(current) || current?.pendingConsent ? { action: "stop" } : undefined);
  for (const name of ["session_before_compact", "session_before_tree", "session_before_fork", "session_before_switch"]) {
    pi.on(name, (_event, ctx) => {
      if (!protectedWork(current)) return;
      stop(current, "navigation_or_compaction");
      if (current.signal && !current.signal.aborted) abort(current, ctx);
      return { cancel: true };
    });
  }
  // Runs even when no valid AoE bootstrap was restored.
  pi.on("context", (event, ctx) => {
    const w = current;
    if (protectedWork(w) && !w.r && !recover(w, ctx, clean(event.messages))) { abort(w, ctx); return { messages: clean(event.messages) }; }
  });
  return {
    clean,
    attach(r) { current = build(r.ctx, r.bootstrap.binding, r.generation, r); },
    shutdown(r) { if (current?.r === r) { if (current.pendingConsent) current.pendingConsent.cancelled = true; stop(current, "shutdown"); forgetSignal(current); current.r = undefined; } },
    blocked(r) { return !!current && current.r === r && (protectedWork(current) || !!current.pendingConsent || current.period?.snapshot()?.binding.native_id === current.binding.native_id); },
    failed(r) {
      if (current?.r !== r) return;
      if (current.pendingConsent) current.pendingConsent.cancelled = true;
      stop(current, "host_unavailable", "blocked");
      if (!uiDepth && current.signal && !current.signal.aborted) abort(current);
    },
    async beforeDelegation(r, view, probe) {
      const w = current;
      if ((!protectedWork(w) && !w?.pendingConsent) || w.r !== r || !view || view.state === "ended") return probe;
      if (w.pendingConsent) w.pendingConsent.cancelled = true;
      stop(w, "competing_legacy_delegation", "blocked");
      if (!uiDepth && w.signal && !w.signal.aborted) abort(w);
      await r.bridge.request({ op: "runtime", command: { kind: "stop", window: view.window, reason: "interrupted" } });
      return undefined;
    },
    async poll(r, request, capability) {
      const w = current;
      if (!w || w.r !== r) return;
      r.workPeriodHost = capability;
      if (w.period?.active) {
        try { w.period.observe(); } catch { w.orphanFailure = "Native deadline write unverified"; }
        if (r.revision !== w.controlRevision || ["invalidated", "unavailable"].includes(r.state) || legacyProtected(r)) stop(w, "host_control_or_guard_changed");
        if (!w.period.active && w.signal && !w.signal.aborted) abort(w);
      }
      const value = await handle(w, request);
      if (value) await r.bridge.request({ op: "work_period_reply", reply: value });
    },
    async context(r, messages, ctx) {
      const w = current, original = messages;
      messages = clean(messages);
      if (!w || w.r !== r) return;
      w.ctx = ctx;
      if (recover(w, ctx, messages)) return;
      if (!protectedWork(w)) { w.lastExternal = external(messages); return; }
      if (!w.period?.active || w.orphanFailure) { abort(w, ctx); return { messages }; }
      try {
        if (legacyProtected(r) || uiDepth || ctx.hasPendingMessages() || !ctx.signal || ctx.signal.aborted || ctx.isIdle()
          || ctx.sessionManager.getSessionId() !== w.binding.native_id || !pi.getActiveTools().includes(WORK_CHECKPOINT_TOOL)) throw Error("current work unavailable");
        if (w.baseline !== external(messages) || w.nativeBaseline !== nativeExternal(ctx)) throw Error("new external context");
        let continuation;
        if (w.expected) {
          const entry = original.filter(m => m.customType === CONTINUATION).at(-1), d = entry?.details;
          if (d?.period !== w.expected.id || d.request !== w.expected.request || d.admission !== w.expected.admission || entry.content !== w.expected.content
            || w.expected.before !== JSON.stringify(messages.filter(m => m.role !== "system")) || r.generation !== w.generation) throw Error("continuation changed");
          continuation = entry;
        }
        const admission = w.period.reserve();
        if (w.signal !== ctx.signal) {
          forgetSignal(w); w.signal = ctx.signal;
          w.onAbort = () => stop(w, "native_abort");
          w.signal.addEventListener("abort", w.onAbort, { once: true });
        }
        w.baseline = external(messages); w.expected = undefined;
        return { messages: [...messages, ...(continuation ? [continuation] : []), { role: "custom", customType: INSTRUCTIONS,
          content: `${REMINDER}\nPeriod admission: ${JSON.stringify({ ...admission, id: w.period.snapshot().id, expires_at_ms: w.period.snapshot().expires_at_ms })}`,
          display: false, timestamp: wall() }] };
      } catch { stop(w, "request_or_context_unavailable", "blocked"); abort(w, ctx); return { messages }; }
    },
    status(r) {
      const w = current;
      if (!w || w.r !== r || !protectedWork(w)) return;
      if (w.orphanFailure?.includes("NOT qualified")) return w.orphanFailure;
      if (w.orphanFailure || w.period?.failure) return "AFK work period: native evidence unavailable; automatic work blocked";
      const s = w.period.snapshot();
      return `AFK work period: ${s.status}; requests ${s.requests_used}/${s.request_limit}; local tools ${s.tools_used}/${s.tool_limit}; ${s.reason ?? "current task"}; settlement ${completionFor(w, s.id)?.proved ? "proved" : "not proved"}; main-loop coverage, not billing`;
    },
  };
}

