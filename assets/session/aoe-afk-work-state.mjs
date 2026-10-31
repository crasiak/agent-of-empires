import { randomUUID } from "node:crypto";
import { performance } from "node:perf_hooks";

const TOOLS = new Set(["read", "write", "edit", "bash"]);
const TERMINAL = new Set(["completed", "blocked", "expired", "exhausted", "interrupted"]);
const FIELDS = ["version", "consent", "binding", "generation", "id", "revision", "task", "issued_at_ms", "expires_at_ms", "request_limit", "tool_limit", "tools", "requests_used", "tools_used", "checkpoint_count", "continuations_used", "status", "reason", "action", "checkpoint"];
const BINDING = ["instance_id", "profile", "native_id", "launch_id"];
const CHECKPOINT = ["status", "summary", "next_step", "evidence", "human_required", "depends_on"];
const MAX_RECORDS = 8192;
const clone = value => structuredClone(value);
const keys = (value, expected) => value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).sort().join(",") === [...expected].sort().join(",");
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const text = (value, max) => typeof value === "string" && Buffer.byteLength(value) > 0 && Buffer.byteLength(value) <= max && !/[\u0000-\u0008\u000b-\u001f\u007f]/u.test(value);
const integer = (value, min, max) => Number.isSafeInteger(value) && value >= min && value <= max;
const check = (condition, message) => { if (!condition) throw Error(message); };
const scope = (a, b) => ["instance_id", "profile", "native_id"].every(key => a?.[key] === b?.[key]);
const identity = s => JSON.stringify([s.binding, s.generation, s.id, s.task, s.issued_at_ms, s.expires_at_ms, s.request_limit, s.tool_limit, s.tools]);

function validCheckpoint(c) {
  return keys(c, CHECKPOINT) && ["unfinished", "completed", "blocked"].includes(c.status) && text(c.summary, 2048)
    && (c.status === "unfinished" ? text(c.next_step, 1024) : c.next_step === null)
    && Array.isArray(c.evidence) && c.evidence.length <= 16 && c.evidence.every(x => text(x, 512))
    && typeof c.human_required === "boolean" && Array.isArray(c.depends_on) && c.depends_on.length <= 16 && c.depends_on.every(x => text(x, 256));
}

function validate(s, binding, requireScope = true) {
  check(keys(s, FIELDS) && Buffer.byteLength(JSON.stringify(s)) <= 8192 && s.version === 1 && s.consent === "native-interactive-confirm-v1"
    && keys(s.binding, BINDING) && (!requireScope || scope(s.binding, binding)) && text(s.binding.instance_id, 256) && text(s.binding.profile, 256)
    && uuid(s.binding.native_id) && uuid(s.binding.launch_id) && uuid(s.generation) && uuid(s.id)
    && integer(s.revision, 1, MAX_RECORDS) && text(s.task, 2048) && integer(s.issued_at_ms, 0, Number.MAX_SAFE_INTEGER)
    && integer(s.expires_at_ms, s.issued_at_ms + 60000, s.issued_at_ms + 86400000)
    && integer(s.request_limit, 1, 512) && s.tool_limit === Math.min(s.request_limit * 16, 4096)
    && Array.isArray(s.tools) && s.tools.length > 0 && s.tools.length <= TOOLS.size && new Set(s.tools).size === s.tools.length && s.tools.every(t => TOOLS.has(t))
    && integer(s.requests_used, 0, s.request_limit) && integer(s.tools_used, 0, s.tool_limit)
    && integer(s.checkpoint_count, 0, s.request_limit * 2) && integer(s.continuations_used, 0, s.request_limit)
    && ["pending", "owned", "recovered", ...TERMINAL].includes(s.status)
    && (["pending", "owned"].includes(s.status) ? s.reason === null : text(s.reason, 128))
    && ["start", "request", "tool", "checkpoint", "continue", "stop", "recover"].includes(s.action)
    && (s.checkpoint === null || validCheckpoint(s.checkpoint) && s.checkpoint.evidence.every(x => /^[0-9a-f]{64}$/.test(x))), "invalid native work-period snapshot");
}

function transition(old, next) {
  check(next.revision === old.revision + 1 && identity(next) === identity(old), "conflicting work-period identity/revision");
  const changes = [next.requests_used - old.requests_used, next.tools_used - old.tools_used, next.checkpoint_count - old.checkpoint_count, next.continuations_used - old.continuations_used];
  const expected = { request: [1, 0, 0, 0], tool: [0, 1, 0, 0], checkpoint: [0, 0, 1, 0], continue: [0, 0, 0, 1], stop: [0, 0, 0, 0], recover: [0, 0, 0, 0] }[next.action];
  check(expected && changes.every((n, i) => n === expected[i]), "invalid work-period consumption");
  if (next.action === "recover") check(TERMINAL.has(old.status) || ["pending", "owned"].includes(old.status), "unmatched ordinary recovery");
  else check(["pending", "owned"].includes(old.status), "work-period authority cannot be restored");
  if (next.action === "recover") check(next.status === "recovered", "invalid recovery state");
  if (["request", "tool", "continue"].includes(next.action)) check(next.status === "owned" && next.requests_used > 0, "work requires an admitted request");
  if (next.action === "stop") check(TERMINAL.has(next.status), "invalid terminal state");
  if (next.action === "request") check(JSON.stringify(next.checkpoint) === JSON.stringify(old.checkpoint), "request changed checkpoint provenance");
  if (next.action === "tool") check(next.checkpoint === null, "local work requires a new checkpoint");
  if (next.action === "continue") check(next.checkpoint?.status === "unfinished", "continuation requires unfinished work");
}

export function createWorkPeriod({ binding, generation, journal, wall = Date.now, mono = () => performance.now() }) {
  let current, active = false, failure, records = 0, deadline = 0, lastSettlement = { request: 0, checkpoint: 0 };
  const known = new Set(), progress = new Set();
  const remember = error => { active = false; failure ??= error?.message ?? "native work-period evidence unavailable"; };
  try {
    check(keys(binding, BINDING) && uuid(binding.native_id) && uuid(binding.launch_id) && uuid(generation), "invalid work-period runtime binding");
    const entries = journal.load(); check(Array.isArray(entries) && entries.length <= MAX_RECORDS, "work-period history capacity");
    for (const s of entries) {
      validate(s, binding, false);
      if (!current || current.id !== s.id) {
        check((!current || current.status === "recovered") && !known.has(s.id) && known.size < 64 && s.action === "start" && s.status === "pending" && s.revision === 1
          && s.requests_used === 0 && s.tools_used === 0 && s.checkpoint_count === 0 && s.continuations_used === 0 && s.checkpoint === null, "invalid work-period start history");
        known.add(s.id);
      } else transition(current, s);
      current = clone(s); records++;
    }
    check(!current || scope(current.binding, binding) || current.status === "recovered", "unresolved work period belongs to another native scope");
  } catch (error) { remember(error); }
  const store = next => {
    try {
      check(!failure, failure); check(records < MAX_RECORDS, "native work-period history capacity");
      validate(next, binding);
      if (current?.id === next.id) transition(current, next);
      journal.append(clone(next)); current = clone(next); records++;
      return clone(current);
    } catch (error) { remember(error); throw error; }
  };
  const update = (action, changes) => store({ ...current, revision: current.revision + 1, action, ...changes });
  const stop = (reason, status = "interrupted") => {
    if (!active || failure) return;
    const result = update("stop", { status, reason }); active = false; return result;
  };
  const requireActive = () => {
    check(!failure, failure); check(active, "work period is not active");
    if (wall() >= current.expires_at_ms || mono() >= deadline) { stop("deadline", "expired"); throw Error("work period expired"); }
  };
  return {
    get active() { return active && !failure; },
    get guarded() { return !!failure || !!current && current.status !== "recovered"; },
    get failure() { return failure; },
    snapshot() { return current && clone(current); },
    start(grant) {
      check(!failure, failure); check(!active && (!current || current.status === "recovered"), "explicit ordinary recovery required before another period");
      check(keys(grant, ["id", "minutes", "requests", "task", "tools"]) && uuid(grant.id) && integer(grant.minutes, 1, 1440) && integer(grant.requests, 1, 512)
        && text(grant.task, 2048) && Array.isArray(grant.tools) && grant.tools.length > 0 && new Set(grant.tools).size === grant.tools.length && grant.tools.every(x => TOOLS.has(x)), "invalid work-period grant");
      check(!known.has(grant.id), "work-period grant cannot be replayed");
      const toolLimit = Math.min(grant.requests * 16, 4096);
      check(known.size < 64 && records + grant.requests * 4 + toolLimit + 4 <= MAX_RECORDS, "work-period history capacity");
      const issued = wall();
      const result = store({ version: 1, consent: "native-interactive-confirm-v1", binding: clone(binding), generation, id: grant.id, revision: 1, task: grant.task,
        issued_at_ms: issued, expires_at_ms: issued + grant.minutes * 60000, request_limit: grant.requests, tool_limit: toolLimit, tools: [...grant.tools].sort(),
        requests_used: 0, tools_used: 0, checkpoint_count: 0, continuations_used: 0, status: "pending", reason: null, action: "start", checkpoint: null });
      known.add(grant.id); deadline = mono() + grant.minutes * 60000; progress.clear(); lastSettlement = { request: 0, checkpoint: 0 }; active = true;
      return result;
    },
    reserve() {
      requireActive();
      if (current.requests_used >= current.request_limit) { stop("requests_exhausted", "exhausted"); throw Error("work-period requests exhausted"); }
      const s = update("request", { status: "owned", reason: null, requests_used: current.requests_used + 1 });
      return { request_number: s.requests_used, remaining: s.request_limit - s.requests_used };
    },
    allowTool(name) {
      if (!active) return false;
      try { requireActive(); } catch { return false; }
      if (current.status !== "owned") return false;
      if (!current.tools.includes(name)) { stop(name === "ask_user" ? "human_required" : "unsupported_tool", "blocked"); return false; }
      if (current.tools_used >= current.tool_limit) { stop("tools_exhausted", "exhausted"); return false; }
      update("tool", { tools_used: current.tools_used + 1, checkpoint: null }); return true;
    },
    checkpoint(input, observations) {
      requireActive(); check(current.status === "owned", "checkpoint requires an admitted request");
      if (!validCheckpoint(input) || !Array.isArray(observations) || observations.length > 256
        || observations.some(x => !keys(x, ["id", "fingerprint"]) || !text(x.id, 512) || !/^[0-9a-f]{64}$/.test(x.fingerprint))
        || new Set(input.evidence).size !== input.evidence.length || input.evidence.some(id => !observations.some(x => x.id === id))) {
        stop("invalid_checkpoint", "blocked"); throw Error("invalid work-period checkpoint/evidence");
      }
      if (current.checkpoint_count >= current.request_limit * 2) { stop("checkpoints_exhausted", "exhausted"); return; }
      const fingerprints = [...new Set(input.evidence.map(id => observations.find(x => x.id === id).fingerprint))].sort();
      let status = "owned", reason = null;
      if (input.human_required || input.depends_on.length) { status = "blocked"; reason = input.human_required ? "human_required" : "dependency"; }
      else if (input.status === "blocked") { status = "blocked"; reason = "model_blocked"; }
      else if (input.status === "completed") { status = "completed"; reason = "model_completed"; }
      else if (!fingerprints.some(fingerprint => !progress.has(fingerprint))) { status = "blocked"; reason = "no_progress"; }
      const result = update("checkpoint", { status, reason, checkpoint_count: current.checkpoint_count + 1, checkpoint: { ...clone(input), evidence: fingerprints } });
      if (status === "owned") for (const fingerprint of fingerprints) progress.add(fingerprint);
      else active = false;
      return result;
    },
    settle({ competing = false } = {}) {
      if (!active) return { continue: false };
      try { requireActive(); } catch { return { continue: false }; }
      if (competing) { stop("competing_work"); return { continue: false }; }
      if (current.status !== "owned" || current.checkpoint?.status !== "unfinished") { stop("missing_checkpoint", "blocked"); return { continue: false }; }
      if (current.requests_used >= current.request_limit) { stop("requests_exhausted", "exhausted"); return { continue: false }; }
      if (lastSettlement.request === current.requests_used) return { continue: false };
      if (lastSettlement.checkpoint === current.checkpoint_count) { stop("no_new_checkpoint", "blocked"); return { continue: false }; }
      update("continue", { continuations_used: current.continuations_used + 1 });
      lastSettlement = { request: current.requests_used, checkpoint: current.checkpoint_count };
      return { continue: true, id: current.id, request: current.requests_used, admission: randomUUID(), next_step: current.checkpoint.next_step };
    },
    observe() {
      if (active && (wall() >= current.expires_at_ms || mono() >= deadline)) stop("deadline", "expired");
      return current && clone(current);
    },
    stop,
    recover(proof) {
      check(!failure, failure);
      check(keys(proof, ["interactive", "settled", "transformed", "handled", "failed", "new_user_entry"]) && proof.interactive === true && proof.settled === true
        && proof.transformed === false && proof.handled === false && proof.failed === false && proof.new_user_entry === true, "fresh settled ordinary input required");
      if (!current || current.status === "recovered") return;
      const result = update("recover", { status: "recovered", reason: "ordinary_input" }); active = false; return result;
    },
  };
}
