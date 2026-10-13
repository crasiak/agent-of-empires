import { createHash } from "node:crypto";
import { lstatSync, realpathSync } from "node:fs";

const SAFE = Number.MAX_SAFE_INTEGER;
const MAX_ENTRIES = 8192;
const PAGE = 128;
const sha = value => createHash("sha256").update(value).digest("hex");
const sorted = value => Array.isArray(value) ? value.map(sorted) : value && typeof value === "object" ? Object.fromEntries(Object.keys(value).sort().map(key => [key, sorted(value[key])])) : value;
export const fingerprint = value => sha(JSON.stringify(sorted(value)));
const initial = () => ({ count: 0, hash: sha("aoe-afk-usage-v1") });
const append = (cursor, row) => ({ count: cursor.count + 1, hash: sha(`${cursor.hash}:${fingerprint(row)}`) });
const integer = value => Number.isSafeInteger(value) && value >= 0;
const identity = value => typeof value === "string" && value.length > 0 && Buffer.byteLength(value) <= 256 && !/[\u0000-\u001f\u007f]/u.test(value);
const id = value => typeof value === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(value);

export function normalize(entry) {
  if (!entry || !id(entry.id) || !(entry.parentId == null || id(entry.parentId))) throw Error("source_changed");
  const row = { id: entry.id, parent_id: entry.parentId ?? null, kind: "metadata", model: null, counters: null, gap: null, sdk_cost_nano: null };
  let usage, provider, model, uncertain = false;
  if (entry.type === "message" && entry.message?.role === "user") { row.kind = "user"; return row; }
  else if (entry.type === "message" && entry.message?.role === "assistant") {
    row.kind = "assistant"; usage = entry.message.usage; provider = entry.message.provider; model = entry.message.responseModel ?? entry.message.model;
    uncertain = !["stop", "length", "toolUse"].includes(entry.message.stopReason);
  } else if (entry.type === "usage" && entry.kind === "cache_warm") {
    row.kind = "usage"; usage = entry.usage; provider = entry.provider; model = entry.model;
  } else if (entry.type === "compaction" || entry.type === "branch_summary") {
    row.kind = entry.type; usage = entry.usage;
  } else if ((entry.type === "message" && entry.message?.role === "toolResult" && entry.message.usage !== undefined) || entry.type === "usage" || entry.usage !== undefined) {
    row.kind = "nested"; row.gap = "nested_overlap"; return row;
  } else return row;
  if (identity(provider) && identity(model)) row.model = { provider, model };
  if (!usage || typeof usage !== "object") { row.gap = "missing_usage"; return row; }
  const fields = [usage.input, usage.output, usage.cacheRead, usage.cacheWrite];
  const long = usage.cacheWrite1h ?? 0, reasoning = usage.reasoning ?? 0;
  const total = fields.reduce((a, b) => a + b, 0);
  if (![...fields, long, reasoning, total].every(integer) || long > usage.cacheWrite || reasoning > usage.output) { row.gap = "invalid_usage"; return row; }
  row.counters = { input: usage.input, output: usage.output, cache_read: usage.cacheRead, cache_write: usage.cacheWrite, cache_write_1h: long, reasoning };
  if (uncertain) row.gap = "error_or_abort";
  const cost = usage.cost?.total;
  if (typeof cost === "number" && Number.isFinite(cost) && cost >= 0 && (cost > 0 || total === 0)) {
    const nano = Math.ceil(cost * 1e9);
    if (integer(nano) && nano <= SAFE) row.sdk_cost_nano = nano;
  }
  return row;
}

export function collect(manager, challenge, boundary, window) {
  const observation = { window: window ?? null, boundary, report: null, gap: null };
  if (!challenge) return observation;
  try {
    const file = manager.getSessionFile();
    if (!file) { observation.gap = "not_persisted"; return observation; }
    const stat = lstatSync(file);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.nlink !== 1 || (typeof process.getuid === "function" && stat.uid !== process.getuid())) throw Error("source_changed");
    const header = manager.getHeader();
    if (!header || header.id !== manager.getSessionId()) throw Error("source_changed");
    const source = { native_id: header.id, file: sha(realpathSync(file)), parent: header.parentSession ? sha(header.parentSession) : null };
    if (challenge.identity && fingerprint(challenge.identity) !== fingerprint(source)) throw Error("source_changed");
    const entries = manager.getEntries();
    if (entries.length > MAX_ENTRIES) { observation.gap = "history_limit"; return observation; }
    if (!integer(challenge.after?.count) || challenge.after.count > entries.length) throw Error("source_changed");
    let cursor = initial(), prefix = challenge.after.count === 0 ? cursor : undefined;
    const rows = [], ids = new Set(), models = new Set();
    for (let i = 0; i < entries.length; i++) {
      const row = normalize(entries[i]);
      if (ids.has(row.id)) throw Error("source_changed"); ids.add(row.id);
      if (row.model) models.add(JSON.stringify(row.model));
      if (models.size > 16) { observation.gap = "history_limit"; return observation; }
      cursor = append(cursor, row);
      if (cursor.count === challenge.after.count) prefix = cursor;
      if (i >= challenge.after.count && rows.length < PAGE) rows.push(row);
    }
    if (!prefix || prefix.hash !== challenge.after.hash) throw Error("source_changed");
    const report = { version: 1, challenge: challenge.id, identity: source, after: prefix, entries: rows, head: cursor };
    if (Buffer.byteLength(JSON.stringify(report)) > 128 * 1024) { observation.gap = "history_limit"; return observation; }
    observation.report = report;
  } catch (error) { observation.gap = error?.message === "source_changed" ? "source_changed" : "unavailable"; }
  return observation;
}

export function createCollector() {
  let challenge, fresh = false;
  return {
    observe(r, boundary = "poll") { return collect(r.ctx.sessionManager, challenge, boundary, r.view?.window); },
    accept(reply) { challenge = reply?.challenge; fresh = reply?.fresh === true; },
    supports(r) {
      if (!fresh || typeof r.ctx.abortPreservingQueue !== "function" || typeof r.ctx.endRunPreservingQueue !== "function") return false;
      return [r.view?.usage?.guard, r.view?.shared_usage_guard].every(policy => !policy?.usd || policy.usd.quotes.some(q => q.model.provider === r.ctx.model?.provider && q.model.model === r.ctx.model?.id));
    },
  };
}
