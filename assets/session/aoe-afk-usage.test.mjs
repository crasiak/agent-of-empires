import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collect, fingerprint, normalize } from "./aoe-afk-usage.mjs";
const sha = value => createHash("sha256").update(value).digest("hex");
const initial = () => ({ count: 0, hash: sha("aoe-afk-usage-v1") });
const usage = () => ({ input: 10, output: 4, cacheRead: 3, cacheWrite: 2, reasoning: 2, cacheWrite1h: 1, totalTokens: 19, cost: { total: 0.01 } });
const assistant = (id = "a") => ({ id, parentId: null, type: "message", message: { role: "assistant", provider: "provider", model: "model", stopReason: "stop", content: [{ type: "text", text: "PRIVATE-CONTENT" }], usage: usage() } });

test("projection keeps only resource metadata and distinguishes missing, zero, nested and unpriced usage", () => {
  for (const kind of ["assistant", "abort", "missing", "zero", "zero-price", "summary", "unknown-usage", "cache-warm", "nested", "edit", "user"]) {
    const e = assistant();
    if (kind === "abort") e.message.stopReason = "aborted";
    if (kind === "missing") delete e.message.usage;
    if (kind === "zero") e.message.usage = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: { total: 0 } };
    if (kind === "zero-price") e.message.usage.cost.total = 0;
    if (kind === "summary") Object.assign(e, { type: "compaction", usage: usage(), summary: "PRIVATE-SUMMARY" });
    if (["unknown-usage", "cache-warm"].includes(kind)) Object.assign(e, { type: "usage", kind: kind === "cache-warm" ? "cache_warm" : "unqualified_rollup", provider: "provider", model: "model", usage: usage() });
    if (kind === "nested") e.message.role = "toolResult";
    if (kind === "edit") Object.assign(e, { type: "context_edit", targetId: "old", replacement: "PRIVATE-REPLACEMENT" });
    if (kind === "user") e.message = { role: "user", content: "PRIVATE-HUMAN-INPUT" };
    const row = normalize(e);
    assert.ok(!JSON.stringify(row).includes("PRIVATE"));
    assert.deepEqual(row, normalize(JSON.parse(JSON.stringify(e))), "reopen normalization stable");
    if (["nested", "unknown-usage"].includes(kind)) { assert.equal(row.counters, null); assert.equal(row.gap, "nested_overlap"); }
    if (kind === "missing") { assert.equal(row.counters, null); assert.equal(row.gap, "missing_usage"); }
    if (kind === "abort") assert.equal(row.gap, "error_or_abort");
    if (kind === "zero") { assert.equal(row.gap, null); assert.equal(row.counters.input, 0); }
    if (kind === "zero-price") assert.equal(row.sdk_cost_nano, null, "SDK zero price is not a free-cost claim");
    if (kind === "summary") { assert.equal(row.model, null); assert.equal(row.counters.input, 10); }
    if (kind === "user") { assert.equal(row.kind, "user"); assert.equal(row.gap, null); }
  }
  for (const patch of [{ input: -1 }, { output: NaN }, { reasoning: 5 }, { cacheWrite1h: 3 }, { input: Number.MAX_SAFE_INTEGER }]) {
    const e = assistant(); Object.assign(e.message.usage, patch); const row = normalize(e);
    assert.equal(row.gap, "invalid_usage"); assert.equal(row.counters, null);
  }
});

test("paged raw history preserves prefixes and refuses changed IDs, forks and limits", () => {
  const dir = mkdtempSync(join(tmpdir(), "afk-usage-projection-"));
  try {
    const file = join(dir, "native.jsonl"); writeFileSync(file, "isolated fixture\n", { mode: 0o600 });
    let entries = Array.from({ length: 130 }, (_, i) => assistant(`e${i}`));
    const manager = { getSessionFile: () => file, getSessionId: () => "native", getHeader: () => ({ id: "native" }), getEntries: () => entries, getBranch: () => { throw Error("must not use active branch"); } };
    let challenge = { id: "00000000-0000-4000-8000-000000000001", after: initial(), identity: null };
    const first = collect(manager, challenge, "poll", null); assert.equal(first.gap, null); assert.equal(first.report.entries.length, 128); assert.equal(first.report.head.count, 130);
    let prefix = initial(); for (const row of first.report.entries) prefix = { count: prefix.count + 1, hash: sha(`${prefix.hash}:${fingerprint(row)}`) };
    challenge = { ...challenge, after: prefix, identity: first.report.identity };
    const last = collect(manager, challenge, "before_tool", "window"); assert.equal(last.report.entries.length, 2); assert.deepEqual(last.report.head, first.report.head);
    const heart = collect(manager, { ...challenge, after: last.report.head }, "poll", null); assert.deepEqual(heart.report.entries, []); assert.deepEqual(heart.report.head, last.report.head);
    entries[0].message.usage.input++; assert.equal(collect(manager, challenge, "poll", null).gap, "source_changed"); entries[0].message.usage.input--;
    entries[129].id = entries[0].id; assert.equal(collect(manager, challenge, "poll", null).gap, "source_changed");
    entries = Array.from({ length: 8193 }, (_, i) => assistant(`e${i}`)); assert.equal(collect(manager, challenge, "poll", null).gap, "history_limit");
    assert.equal(collect({ ...manager, getSessionFile: () => undefined }, challenge, "poll", null).gap, "not_persisted");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
