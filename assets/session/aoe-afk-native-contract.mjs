import { createHash } from "node:crypto";
import { isDeepStrictEqual as same } from "node:util";
import { nativeDeferredResult } from "./aoe-afk-observer-fixture.mjs";
import { classifyDiagnostic } from "./aoe-afk-observer-diagnostic.mjs";
import { validEvent } from "./aoe-afk-observer-fixture.mjs";
import { validProbe } from "./aoe-afk-observer-probes.mjs";

export const KIND = "aoe-afk-native-v1";
export const sha = value => createHash("sha256").update(value).digest("hex");
export const sorted = value => Array.isArray(value) ? value.map(sorted) : value && typeof value === "object"
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, sorted(value[key])])) : value;
export const digest = value => sha(JSON.stringify(sorted(value)));
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const hash = value => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const text = value => typeof value === "string" && value.length > 0 && value.length <= 256 && !/[\x00-\x1f\x7f]/.test(value);
export const validBinding = b => b && Object.keys(b).sort().join() === "instance_id,launch_id,native_id,profile"
  && [b.instance_id, b.profile].every(text) && uuid(b.native_id) && uuid(b.launch_id);
export const validIdentity = q => q && Object.keys(q).sort().join() === "arguments_hash,context_ref,interaction_id,kind,operation,tool_call_id"
  && uuid(q.operation) && uuid(q.interaction_id) && text(q.context_ref) && q.tool_call_id === "fixture-question-1"
  && ["input", "select", "custom-overlay"].includes(q.kind) && hash(q.arguments_hash);
const position = writer => {
  const s = writer?.snapshot();
  return s && s.invalid === false && s.truncated === false && Number.isInteger(s.events) && s.events >= 0
    ? { events: s.events, bytes: s.bytes } : null;
};

const PUBLICATION_OPS = ["poll", "ack", "runtime_ack", "runtime", "question_recover", "usage_recover", "other"];
const PUBLICATION_RECEIPTS = 16, PUBLICATION_BYTES = 16 * 1024;
export function createPublicationAudit() {
  let uncertain = false, truncated = false, bytes = 0;
  const receipts = [];
  return {
    observe(receipt) {
      if (receipt.publication_clean === true && Array.isArray(receipt.warnings) && receipt.warnings.length === 0) return;
      // Set custody before any bounded evidence construction or observer failure.
      uncertain = true;
      const warnings = [];
      if (!Array.isArray(receipt.warnings) || receipt.warnings.length === 0) truncated = true;
      else {
        if (receipt.warnings.length > 8) truncated = true;
        for (const warning of receipt.warnings.slice(0, 8)) {
          if (typeof warning !== "string" || !warning.length) { truncated = true; continue; }
          if (warning.length > 256) truncated = true;
          warnings.push(warning.slice(0, 256));
        }
      }
      if (!validBinding(receipt.binding) || !uuid(receipt.generation) || !Number.isInteger(receipt.request)
        || receipt.request < 1 || receipt.request > 65535 || !PUBLICATION_OPS.includes(receipt.op) || warnings.length === 0) {
        truncated = true; return;
      }
      const row = { binding: structuredClone(receipt.binding), generation: receipt.generation, request: receipt.request,
        op: receipt.op, data_commit: receipt.data_commit === true, warnings };
      const size = Buffer.byteLength(JSON.stringify(row));
      if (receipts.length >= PUBLICATION_RECEIPTS || bytes + size > PUBLICATION_BYTES) { truncated = true; return; }
      receipts.push(row); bytes += size;
    },
    snapshot() { return structuredClone({ uncertain, truncated, receipts }); },
  };
}

export function validPublication(value, expected) {
  return value && Object.keys(value).sort().join() === "receipts,truncated,uncertain"
    && typeof value.uncertain === "boolean" && typeof value.truncated === "boolean"
    && Array.isArray(value.receipts) && value.receipts.length <= PUBLICATION_RECEIPTS
    && (value.uncertain ? value.truncated || value.receipts.length > 0 : !value.truncated && value.receipts.length === 0)
    && value.receipts.every(r => r && Object.keys(r).sort().join() === "binding,data_commit,generation,op,request,warnings"
      && validBinding(r.binding) && same(r.binding, expected?.binding) && uuid(r.generation) && r.generation === expected?.generation
      && Number.isInteger(r.request) && r.request > 0 && r.request <= 65535 && PUBLICATION_OPS.includes(r.op) && r.data_commit === true
      && Array.isArray(r.warnings) && r.warnings.length > 0 && r.warnings.length <= 8
      && r.warnings.every(w => typeof w === "string" && w.length > 0 && w.length <= 256))
    && value.receipts.reduce((n, r) => n + Buffer.byteLength(JSON.stringify(r)), 0) <= PUBLICATION_BYTES;
}

// Installed inside the diagnostic wrappers, before the product receives its original promise.
export function receiptBridge(bridge, { binding, generation, trace, probes, save, fault }) {
  let ordinal = 0;
  const observe = fn => { try { fn(); } catch { try { fault(); } catch { /* Product outcome is unchanged. */ } } };
  return {
    request(...args) {
      const request = ++ordinal;
      let packet;
      observe(() => { packet = structuredClone(args[0]); });
      const promise = Reflect.apply(bridge.request, bridge, args);
      observe(() => promise.then(value => observe(() => {
        const command = packet?.command;
        const warnings = value?.publication_warnings ?? [];
        const warning = !Array.isArray(warnings) || warnings.length > 0;
        if (!warning && (packet?.op !== "runtime" || !["question_admit", "question_intent", "question_outcome", "reserve"].includes(command?.kind))) return;
        const receipt = { kind: KIND, binding, generation, request, op: packet?.op, command, value: structuredClone(value), warnings,
          data_commit: true, publication_clean: Array.isArray(warnings) && warnings.length === 0 };
        if (command?.kind === "question_intent") {
          receipt.valid = validBinding(binding) && uuid(generation) && uuid(command.window) && validIdentity(command.identity)
            && same(value?.identity, command.identity) && value?.guarded_return === true && receipt.publication_clean;
          // Each writer has its own position. No shared clock or cross-stream order is invented.
          receipt.trace = position(trace);
          receipt.probes = position(probes);
        }
        save(receipt);
      }), () => {}));
      return promise;
    },
    close(...args) { return Reflect.apply(bridge.close, bridge, args); },
  };
}

export function intentRelation(rows, boundary, validate, start, end, health) {
  if (!Array.isArray(rows) || !boundary || !health || health.invalid !== false || health.truncated !== false
    || !Number.isInteger(boundary.events) || boundary.events < 0 || boundary.events > rows.length
    || !Number.isInteger(boundary.bytes) || boundary.bytes < 0
    || rows.some((row, i) => !validate(row) || row.seq !== i + 1)
    || health.events !== rows.length || health.bytes !== rows.reduce((n, r) => n + Buffer.byteLength(JSON.stringify(r) + "\n"), 0)
    || boundary.bytes !== rows.slice(0, boundary.events).reduce((n, r) => n + Buffer.byteLength(JSON.stringify(r) + "\n"), 0)
    || !Number.isInteger(start) || !Number.isInteger(end) || start < 1 || start > end || end > rows.length) return "unknown";
  if (start > boundary.events) return "post_intent";
  if (end <= boundary.events) return "pre_intent";
  return "unknown";
}

export function qualifyEndpoint(e) {
  const missing = [];
  const check = (name, ok) => { if (!ok) missing.push(name); };
  try {
    check("native discriminator", e?.kind === KIND);
    const { expected: x, authority: a, native: n, preview: p, usage: u } = e;
    check("expected identity", validBinding(x.binding) && uuid(x.generation) && uuid(x.window) && hash(x.grant_hash) && hash(x.usage_policy_hash));
    check("read-only capture", a.capture === "bounded-read-only" && a.pending_absent === true && a.stable === true);
    check("managed registry", a.registry_valid === true && a.workspace_valid === true && a.grant_hash === x.grant_hash);
    const w = a.snapshot.sessions?.[x.binding.instance_id]?.book?.windows?.find(w => w.id === x.window);
    const batch = a.snapshot.batches?.[x.operation_id], member = batch?.members?.[x.binding.instance_id];
    check("profile", a.snapshot.version === 3 && a.snapshot.profile === x.binding.profile);
    check("window binding", same(w?.binding, x.binding) && w?.generation === x.generation && w?.confirmed === true && w?.state === "owned");
    const previewContent = { ...p }; delete previewContent.digest;
    check("reviewed preview", p?.format === "aoe-afk-batch-preview" && p.version === 3 && p.digest === digest(previewContent)
      && p.digest === x.preview_digest && p.operation_id === x.operation_id && p.profile === x.binding.profile
      && p.minutes === 1 && p.shared_requests === null && p.members?.length === 1
      && p.members[0].session_id === x.binding.instance_id && p.members[0].grant_hash === x.grant_hash);
    check("batch", batch?.id === x.operation_id && batch.preview_digest === x.preview_digest && batch.shared_requests === null
      && batch.stopped === false && Object.keys(batch.members).length === 1 && batch.tickets?.length === 0
      && member?.window === x.window && member.outcome === "confirmed" && member.grant_hash === x.grant_hash
      && same(member.binding, x.binding) && member.generation === x.generation);
    check("grant", same(w?.grant, p.members?.[0]?.grant) && w?.grant.version === 4 && w?.grant.requests === 2 && w.grant.question_deferrals === 1
      && w.grant.settlement_nudges === 0 && same(w.grant.files, [{ path: "scratch.txt", capability: "create" }])
      && w.expires_at_ms - w.issued_at_ms === 60000);
    const q = w?.question, identity = q?.identity;
    check("question authority", validIdentity(identity) && Number.isSafeInteger(q?.admitted_at_ms)
      && Number.isSafeInteger(q.emission_intent_at_ms) && Number.isSafeInteger(q.outcome_at_ms)
      && q.emission_intent_at_ms >= q.admitted_at_ms && q.outcome_at_ms >= q.emission_intent_at_ms
      && q.admitted_at_ms >= w.issued_at_ms && q.admitted_at_ms < w.expires_at_ms
      && q.outcome === "accepted" && q.recovered_at_ms === null);
    const receipts = n.receipts ?? [], intent = receipts.filter(r => r.command?.kind === "question_intent");
    const admits = receipts.filter(r => r.command?.kind === "question_admit");
    const outcomes = receipts.filter(r => r.command?.kind === "question_outcome");
    check("admission receipt", admits.length === 1 && same(admits[0].binding, x.binding) && admits[0].generation === x.generation
      && admits[0].command.window === x.window && same(admits[0].command.identity, identity)
      && same(admits[0].value?.identity, identity) && admits[0].value.admitted_at_ms === q?.admitted_at_ms);
    check("outcome receipt", outcomes.length === 1 && same(outcomes[0].binding, x.binding) && outcomes[0].generation === x.generation
      && outcomes[0].command.window === x.window && same(outcomes[0].command.identity, identity)
      && outcomes[0].command.outcome === "accepted" && outcomes[0].value?.outcome === "accepted" && same(outcomes[0].value.identity, identity));
    check("Rust intent", intent.length === 1 && intent[0].valid === true && intent[0].publication_clean === true
      && intent[0].warnings?.length === 0 && intent[0].value?.guarded_return === true
      && same(intent[0].value.identity, identity) && same(intent[0].command.identity, identity)
      && intent[0].command.window === x.window && same(intent[0].binding, x.binding) && intent[0].generation === x.generation);
    check("publication warnings", receipts.every(r => r.kind === KIND && r.publication_clean === true && r.warnings?.length === 0));
    const reserves = receipts.filter(r => r.command?.kind === "reserve");
    check("exact reservation", reserves.length === 1 && reserves[0].command.window === x.window
      && same(reserves[0].binding, x.binding) && reserves[0].generation === x.generation
      && same(reserves[0].command.question, identity) && reserves[0].command.request === q?.observed_request
      && same(w?.reservations, [q?.observed_request]) && uuid(q?.observed_request)
      && reserves[0].value?.request_number === 1 && reserves[0].value.human_required === true
      && same(reserves[0].value.grant, w?.grant) && same(reserves[0].value.unanswered_gate, q));
    const matches = event => event?.toolCallId === identity?.tool_call_id && event?.interactionId === identity?.interaction_id;
    const owner = event => event?.owner?.namespace === "pi-ask-user" && event.owner.id === identity?.interaction_id;
    check("live native open", n.ready === true && n.opened?.state === "pending" && n.opened?.version === 1
      && n.opened.supported === true && n.opened.uiPromptAttribution === true && matches(n.opened) && owner(n.opened)
      && n.opened.kind === identity?.kind && owner(n.prompt_start)
      && n.prompt_start.kind === (identity?.kind === "custom-overlay" ? "custom" : identity?.kind));
    check("accepted package reply", n.reply?.version === 1 && n.reply.method === "defer" && n.reply.status === "accepted"
      && n.reply.reason === "deferred_by_afk" && n.reply.requestId === identity?.operation && matches(n.reply));
    check("deferred closed", n.closed?.version === 1 && n.closed.state === "deferred" && n.closed.kind === identity?.kind
      && n.closed.supported === true && n.closed.uiPromptAttribution === true && owner(n.closed) && matches(n.closed));
    check("owned prompt end", owner(n.prompt_end) && n.prompt_end.kind === n.prompt_start?.kind);
    check("native deferred result", nativeDeferredResult(n.opened, n.result) && sha(JSON.stringify(n.result?.input)) === identity?.arguments_hash);
    check("native persistence", n.native_id === x.binding.native_id && n.file === u.native_file && u.native_result === true);
    check("bounded no effect", w?.records?.length === 0 && w?.permit === null && w?.nudges?.length === 0
      && a.scratch_absent === true && a.scratch_path === x.scratch_path);
    check("usage readiness", u.ready === true && u.current_source === true && u.unresolved_absent === true
      && same(u.binding, x.binding) && u.generation === x.generation && u.window === x.window
      && u.policy_hash === x.usage_policy_hash && u.policy_hash === digest([p.members?.[0]?.usage_guard, null])
      && u.policy?.usd?.limit_nano === 100 && same(u.policy, p.members?.[0]?.usage_guard)
      && u.source?.identity?.native_id === x.binding.native_id && u.source.identity.file === sha(u.native_file)
      && same(u.source.binding, x.binding) && u.source.generation === x.generation && u.source.gap === null
      && same(u.source.cursor, u.native_head) && u.source.high_water === u.source.cursor.count
      && u.source.announced && Object.keys(u.source.announced).length === 0);
  } catch { missing.push("missing or malformed evidence"); }
  return { endpoint_status: missing.length ? "inconclusive" : "qualified", missing };
}

export function validAttempt(value) {
  return value && hash(value.manifest) && ["baseline", "observed"].includes(value.role)
    && value.ordinal === (value.role === "baseline" ? 1 : 2);
}

export function classifyNative({ endpoint, audit, process: proc, trace, probes } = {}) {
  const qualified = qualifyEndpoint(endpoint);
  const native = validAttempt(audit) && validAttempt(proc) && audit.kind === KIND && audit.version === 1 && audit.manifest === proc.manifest
    && audit.role === proc.role && audit.ordinal === proc.ordinal;
  const completion = native && audit.waitArmed === true && audit.cancelled === true && audit.consumed === true
    && audit.settled === true && audit.off === true && audit.shutdown === true && audit.writersClosed === true && audit.fault === false;
  const publication_status = !validPublication(audit?.publication, endpoint?.expected) ? "invalid"
    : audit.publication.uncertain ? "uncertain" : "clean";
  const lifecycle = completion && proc?.waitpid === 0 && proc.audit_eof === true && proc.control_eof === true
    && proc.audit_valid === true && proc.cleanup === true && proc.escalated === false
    && proc.controller_lost === false && Array.isArray(proc.io_errors) && proc.io_errors.length === 0;
  const d = classifyDiagnostic({ endpoint: qualified.endpoint_status === "qualified", trace, probes, scope: "observed_calls",
    completion: { waitpid: { code: proc?.waitpid, signal: null }, audit: { completed: lifecycle && publication_status === "clean", trace: audit?.trace, probes: audit?.probes } } });
  const intent = endpoint?.native?.receipts?.find(r => r.command?.kind === "question_intent" && r.valid);
  const relations = (rows, stream, validator) => (Array.isArray(rows) ? rows : []).filter(r => r && !r.teardown && ["rejected", "threw", "reply_rejected"].includes(r.event ?? r.phase)).map(r => {
    const start = rows.find(s => s && (stream === "trace" ? s.event === "request" && s.bridge === r.bridge && s.request === r.request
      : s.call === r.call && ["start", "join", "entry", "dispatch"].includes(s.phase)));
    const causal = d.trace_status === "complete" && (stream === "trace" || d.probe_status === "complete");
    return { seq: r.seq, relation: causal ? intentRelation(rows, intent?.[stream], validator, start?.seq, r.seq, audit?.[stream]) : "unknown" };
  });
  const coverage = { trace: relations(trace, "trace", validEvent), probes: relations(probes, "probes", validProbe) };
  const covered = (rows, stream, validator) => intentRelation(rows, intent?.[stream], validator, 1, 1, audit?.[stream]) !== "unknown";
  const positionsValid = covered(trace, "trace", validEvent) && covered(probes, "probes", validProbe);
  return { kind: KIND, ...qualified, trace_status: d.trace_status, probe_status: d.probe_status, coverage,
    completion: completion ? "cancelled_sdk_completion" : "inconclusive", publication_status, publication: audit?.publication ?? null, process: proc ?? null,
    diagnostics: d, cause_claim: "none", financial_finality: "not_claimed",
    classification: !native || !lifecycle || publication_status !== "clean" || qualified.endpoint_status !== "qualified" ? "native_inconclusive"
      : audit.role === "baseline" ? "native_endpoint_only"
      : audit.role === "observed" && d.trace_status === "complete" && d.probe_status === "complete" && positionsValid
        && ["diagnostic_clean", "diagnostic_failure_observed"].includes(d.classification)
        ? "native_scoped_diagnostic" : "native_inconclusive" };
}
