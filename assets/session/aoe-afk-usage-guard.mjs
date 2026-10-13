import { closeSync, constants, fstatSync, lstatSync, openSync, readSync } from "node:fs";

export const USAGE_MARKER = "aoe-afk-usage-v1";
export const USAGE_COVERAGE = "native-entries-terminal-v2";
const MAX_MARKER = 4096, MAX_IDENTITIES = 4096, MAX_RECORDS = 8192, MAX_APPEND = 8192;
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const reference = value => typeof value === "string" && Buffer.byteLength(value) > 0 && Buffer.byteLength(value) <= 256 && !/[\u0000-\u001f\u007f]/u.test(value);
const hash = value => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const keys = (value, expected) => value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).sort().join(",") === [...expected].sort().join(",");
const identity = m => ({ version: m.version, binding: { instance_id: m.binding.instance_id, profile: m.binding.profile, native_id: m.binding.native_id, launch_id: m.binding.launch_id }, window: m.window, generation: m.generation, policy_hash: m.policy_hash });
const same = (a, b) => JSON.stringify(identity(a)) === JSON.stringify(identity(b));
const bindingSame = (a, b) => ["instance_id", "profile", "native_id", "launch_id"].every(k => a?.[k] === b?.[k]);
const check = (condition, message) => { if (!condition) throw Error(message); };
const cursor = c => c && Number.isSafeInteger(c.count) && c.count >= 0 && hash(c.hash);
const FAILURE_SCOPES = Symbol.for("aoe.afk.usage-append-failures.v1");
const FAILURE_UNAVAILABLE = Symbol.for("aoe.afk.usage-append-failures.v1.unavailable");
const validScope = tuple => Array.isArray(tuple) && tuple.length === 3 && reference(tuple[0]) && reference(tuple[1]) && uuid(tuple[2]);
function custody() {
  try {
    if (FAILURE_UNAVAILABLE in globalThis) return;
    if (!(FAILURE_SCOPES in globalThis)) Object.defineProperty(globalThis, FAILURE_SCOPES, { value: { scopes: new Set(), overflow: false } });
    const state = globalThis[FAILURE_SCOPES];
    check(keys(state, ["scopes", "overflow"]) && state.scopes instanceof Set && typeof state.overflow === "boolean"
      && state.scopes.size <= MAX_IDENTITIES && [...state.scopes].every(s => typeof s === "string" && Buffer.byteLength(s) <= 2048 && validScope(JSON.parse(s))), "invalid usage failure custody");
    return state;
  } catch {
    if (!(FAILURE_UNAVAILABLE in globalThis)) Object.defineProperty(globalThis, FAILURE_UNAVAILABLE, { value: true });
  }
}

export function createUsageWitness(pi, r) {
  const known = new Map(), pending = new Map(), completed = new Map();
  let replaying = true;
  const tuple = [r.bootstrap.binding.profile, r.bootstrap.binding.instance_id, r.bootstrap.binding.native_id];
  const scope = validScope(tuple) ? JSON.stringify(tuple) : undefined;
  let records = 0, failure = scope ? undefined : "invalid native usage runtime scope";
  const failed = () => {
    const state = custody();
    return failure ?? (!state || state.overflow ? "native usage failure custody unavailable; restart required" : state.scopes.has(scope) ? "native usage append unverified; restart required" : undefined);
  };
  const remember = error => {
    failure ??= error?.message ?? "native usage provenance unavailable";
    const state = custody();
    if (state && scope && !state.overflow) {
      if (state.scopes.size >= MAX_IDENTITIES && !state.scopes.has(scope)) Object.defineProperty(state, "overflow", { value: true, writable: false });
      else state.scopes.add(scope);
    }
  };
  const fail = error => { remember(error); throw error; };
  const validate = m => {
    check(keys(m, ["version", "binding", "window", "generation", "policy_hash", "phase", "reason"])
      && Buffer.byteLength(JSON.stringify(m)) <= MAX_MARKER && m.version === 1
      && keys(m.binding, ["instance_id", "profile", "native_id", "launch_id"])
      && reference(m.binding.instance_id) && reference(m.binding.profile) && uuid(m.binding.native_id) && uuid(m.binding.launch_id)
      && ["instance_id", "profile", "native_id"].every(k => m.binding[k] === r.bootstrap.binding[k])
      && uuid(m.window) && uuid(m.generation) && hash(m.policy_hash)
      && ((m.phase === "required" && m.reason === null) || (m.phase === "released" && ["ordinary", "superseded", "completed"].includes(m.reason))), "invalid native usage provenance");
  };
  const apply = m => {
    validate(m);
    check(++records <= MAX_RECORDS, "native usage record limit");
    const old = known.get(m.window);
    if (m.phase === "required") {
      check(!old && known.size < MAX_IDENTITIES, "duplicate or excessive native usage identity");
      pending.set(m.window, m);
    } else {
      check(old && same(old, m) && (old.phase === "required" || (old.reason === "completed" && ["ordinary", "superseded"].includes(m.reason))), "unmatched native usage release");
      if (failed()) return;
      completed.delete(m.window);
      if (replaying && m.reason === "completed") pending.set(m.window, m);
      else pending.delete(m.window);
      if (m.reason === "ordinary") r.recoveredUsageWindows.add(m.window);
    }
    known.set(m.window, m);
  };
  try {
    for (const entry of r.ctx.sessionManager.getEntries()) if (entry.type === "custom" && entry.customType === USAGE_MARKER) apply(entry.data);
  } catch (error) { remember(error); }
  replaying = false;

  const append = m => {
    let fd;
    try {
      check(!failed() && records < MAX_RECORDS, failed() ?? "native usage record limit");
      validate(m);
      const manager = r.ctx.sessionManager, file = manager.getSessionFile(), native = manager.getSessionId(), parent = manager.getLeafId();
      check(file && native === r.bootstrap.binding.native_id, "persistent exact native session required");
      fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW);
      const before = fstatSync(fd), path = lstatSync(file);
      const qualified = stat => stat.isFile() && stat.nlink === 1 && (typeof process.getuid !== "function" || stat.uid === process.getuid());
      check(qualified(before) && qualified(path) && !path.isSymbolicLink() && before.dev === path.dev && before.ino === path.ino, "native usage file identity unavailable");
      pi.appendEntry(USAGE_MARKER, m);
      const after = fstatSync(fd), current = lstatSync(file), size = after.size - before.size;
      check(manager.getSessionId() === native && manager.getSessionFile() === file
        && qualified(after) && qualified(current) && !current.isSymbolicLink()
        && after.dev === before.dev && after.ino === before.ino && current.dev === before.dev && current.ino === before.ino
        && current.size === after.size && Number.isSafeInteger(size) && size > 0 && size <= MAX_APPEND, "native usage append not persisted exactly");
      const bytes = Buffer.alloc(size);
      check(readSync(fd, bytes, 0, size, before.size) === size, "native usage readback incomplete");
      const text = bytes.toString("utf8");
      check(Buffer.from(text).equals(bytes) && text.endsWith("\n") && text.indexOf("\n") === text.length - 1, "unexpected native usage append extent");
      const entry = JSON.parse(text), leaf = manager.getLeafId();
      check(entry.type === "custom" && entry.customType === USAGE_MARKER && reference(entry.id) && entry.id === leaf && leaf !== parent
        && entry.parentId === parent && JSON.stringify(entry.data) === JSON.stringify(m), "native usage append identity mismatch");
      apply(m);
    } catch (error) { fail(error); }
    finally { if (fd !== undefined) closeSync(fd); }
  };
  const fromView = view => ({ version: 1, binding: { ...view.binding }, window: view.window, generation: view.generation, policy_hash: view.usage_policy_hash, phase: "required", reason: null });
  const completeView = view => {
    const u = view?.usage, amount = u?.measurement;
    return view?.state === "ended" && view.terminal_reason === "completed"
      && u?.gap === null && u.blocked === null && u.shared_blocked === null && u.unavailable === null && u.negative_evidence_pending !== true
      && u.source_complete === true && Array.isArray(amount?.gaps) && amount.gaps.length === 0 && u.guard && view.shared_usage_guard === null
      && (u.guard.tokens == null || amount.tokens_complete === true) && (u.guard.usd == null || amount.valuation_complete === true);
  };
  const revokeCompleted = () => {
    for (const window of completed.keys()) pending.set(window, known.get(window));
    completed.clear();
  };
  const inspectCompleted = view => {
    try {
      for (const [window, proof] of completed) {
        if (!proof.ticket.isCurrent() || r.ctx.sessionManager.getSessionId() !== r.bootstrap.binding.native_id
          || r.ctx.sessionManager.getLeafId() !== proof.leaf || (view?.window === window && !completeView(view))) {
          pending.set(window, known.get(window)); completed.delete(window);
        }
      }
    } catch (error) { revokeCompleted(); remember(error); }
  };
  const requireIntent = view => {
    inspectCompleted(view);
    check(!failed(), failed());
    if (view?.usage_policy_hash == null) return false;
    try {
      const m = fromView(view); validate(m);
      const old = known.get(m.window);
      if (old) {
        check(same(old, m), "native usage identity changed");
        check(old.phase !== "released" || view.state === "ended" || r.recoveredUsageWindows.has(m.window), "released usage window reactivated");
        return old.phase === "required";
      }
      check(known.size < MAX_IDENTITIES, "native usage identity limit");
      append(m); return true;
    } catch (error) { fail(error); }
  };
  const explicitNoGuard = view => view?.usage_policy_hash === null && view.usage?.guard === null && view.shared_usage_guard === null;
  const current = (view, confirmed) => view && bindingSame(view.binding, r.bootstrap.binding) && view.generation === r.generation
    && uuid(view.window) && ["pending", "owned"].includes(view.state) && (!confirmed || view.confirmed === true)
    && Number.isSafeInteger(view.expires_at_ms) && view.expires_at_ms > Date.now();
  const protectedCurrent = view => current(view, false) && pending.has(view.window) && same(pending.get(view.window), fromView(view));
  const released = view => {
    inspectCompleted(view);
    return !failed() && !!view && !pending.has(view.window) && known.get(view.window)?.phase === "released" && same(known.get(view.window), fromView(view));
  };
  const release = (m, reason) => append({ ...identity(m), phase: "released", reason });
  return {
    require: requireIntent,
    get pending() { inspectCompleted(r.view); return pending.size; },
    get failure() { return failed(); },
    get restoring() { inspectCompleted(r.view); return !!failed() || [...pending.values()].some(m => m.generation !== r.generation || m.reason === "completed"); },
    released,
    revokeCompleted,
    acknowledge(view) { return !failed() && protectedCurrent(view); },
    blocked(view) {
      inspectCompleted(view);
      if (failed()) return true;
      if (!pending.size) return false;
      return !(current(view, true) && (protectedCurrent(view) || explicitNoGuard(view)));
    },
    handoff(view) {
      check(!failed() && current(view, true), failed() ?? "replacement authority not current");
      if (view.usage_policy_hash != null) check(requireIntent(view), "replacement usage protection missing");
      else if (pending.size) check(explicitNoGuard(view), "replacement is not explicit NoGuard");
      for (const m of [...pending.values()]) if (m.window !== view.window) release(m, "superseded");
    },
    cleanEnd(view, proof) {
      const m = view && pending.get(view.window), u = view?.usage;
      if (failed() || !m || m.phase !== "required" || !same(m, fromView(view)) || !completeView(view)
        || !proof || !r.usageCompletion?.proved || proof.ticket !== r.usageCompletion.ticket || proof.ticket.version !== 1 || !proof.ticket.isCurrent()
        || !bindingSame(view.binding, r.bootstrap.binding) || view.generation !== r.generation
        || !cursor(proof.head) || r.ctx.sessionManager.getEntries().length !== proof.head.count || [u.cutoff, u.observed_through, u.source_cursor].some(c => !cursor(c) || c.count !== proof.head.count || c.hash !== proof.head.hash)) return false;
      release(m, "completed");
      completed.set(m.window, { ticket: proof.ticket, leaf: r.ctx.sessionManager.getLeafId() });
      return true;
    },
    async recover() {
      check(!failed(), failed());
      revokeCompleted();
      const windows = [...pending.values()].map(m => m.window);
      for (const m of [...pending.values()]) release(m, "ordinary");
      for (const window of windows) {
        try { await r.bridge.request({ op: "usage_recover", window }); } catch { /* Native recovery is not host cleanup or new authority. */ }
      }
    },
  };
}
