import test from "node:test";
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import { createWorkPeriod } from "./aoe-afk-work-state.mjs";

const fingerprint = value => createHash("sha256").update(value).digest("hex");
function fixture(records = []) {
  let wall = 100000, mono = 0, fail = false;
  const binding = { instance_id: "test", profile: "default", native_id: randomUUID(), launch_id: randomUUID() };
  const journal = { load: () => structuredClone(records), append: value => { if (fail) throw Error("native append unverified"); records.push(structuredClone(value)); } };
  const options = { binding, generation: randomUUID(), journal, wall: () => wall, mono: () => mono };
  const period = createWorkPeriod(options);
  const grant = { id: randomUUID(), minutes: 120, requests: 8, task: "Continue the existing Session task", tools: ["read", "write", "edit", "bash"] };
  return { period, grant, records, options, start: () => period.start(grant), advance: (w, m) => { wall += w; mono += m; }, fail: () => fail = true };
}
const checkpoint = (status, evidence = [], extra = {}) => ({ status, summary: "Task evidence and remaining work", next_step: status === "unfinished" ? "Run the next authorized unit" : null, evidence, human_required: false, depends_on: [], ...extra });
const evidence = (id, value = id) => ({ id, fingerprint: fingerprint(value) });

test("work-period rejects invalid grants without changing the journal", () => {
  for (const mutate of [g => g.requests = 0, g => g.requests = 513, g => g.requests = 1.5, g => g.minutes = 0, g => g.minutes = 1441, g => g.task = "", g => g.tools = ["subagent"], g => g.extra = true]) {
    const f = fixture(); mutate(f.grant);
    assert.throws(() => f.start()); assert.equal(f.records.length, 0); assert.equal(f.period.active, false);
  }
});

test("native history restores a protective guard, never work authority or a fresh allowance", () => {
  const f = fixture(); f.start(); f.period.reserve();
  const restored = createWorkPeriod(f.options);
  assert.equal(restored.active, false); assert.equal(restored.guarded, true);
  assert.throws(() => restored.reserve(), /not active/);
  assert.throws(() => restored.start({ ...f.grant, id: randomUUID() }), /ordinary recovery/);
  for (const key of ["interactive", "settled", "new_user_entry"]) {
    const proof = { interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true }; proof[key] = false;
    assert.throws(() => restored.recover(proof));
  }
  assert.throws(() => restored.recover({ interactive: true, settled: true, transformed: true, handled: false, failed: false, new_user_entry: true }));
  restored.recover({ interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true });
  assert.equal(restored.guarded, false);
  assert.throws(() => restored.start(f.grant), /replayed/);
  restored.start({ ...f.grant, id: randomUUID() }); assert.equal(restored.snapshot().requests_used, 0);
});

test("last request keeps valid tool rights but the next request cannot exceed the period", () => {
  const f = fixture(); f.grant.requests = 1; f.start();
  assert.equal(f.period.reserve().request_number, 1);
  assert.equal(f.period.allowTool("write"), true);
  assert.throws(() => f.period.reserve(), /exhausted/);
  assert.equal(f.period.snapshot().requests_used, 1); assert.equal(f.period.snapshot().status, "exhausted");
  assert.equal(f.period.allowTool("write"), false); assert.equal(f.period.guarded, true);
});

test("wall rollback and repeated reads cannot extend monotonic expiry", () => {
  for (const [wall, mono] of [[-600000, 7200001], [7200001, 1]]) {
    const f = fixture(); f.start(); f.advance(wall, mono);
    assert.throws(() => f.period.reserve(), /expired/);
    assert.equal(f.period.snapshot().status, "expired"); assert.equal(f.period.snapshot().requests_used, 0);
  }
});

test("unfinished work requires new actual evidence, not a new call ID for identical results", () => {
  const f = fixture(); f.start(); f.period.reserve();
  f.period.checkpoint(checkpoint("unfinished", ["one"]), [evidence("one", "same file and result")]);
  assert.equal(f.period.settle().continue, true);
  f.period.reserve();
  f.period.checkpoint(checkpoint("unfinished", ["two"]), [evidence("two", "same file and result")]);
  assert.equal(f.period.active, false); assert.equal(f.period.snapshot().reason, "no_progress");
  assert.equal(f.period.settle().continue, false);
});

test("human gates, dependencies, competing work and missing checkpoints never auto-continue", () => {
  for (const kind of ["human", "dependency", "competing", "missing", "unsupported-tool"]) {
    const f = fixture(); f.start(); f.period.reserve();
    if (kind === "human") f.period.checkpoint(checkpoint("completed", [], { human_required: true }), []);
    if (kind === "dependency") f.period.checkpoint(checkpoint("unfinished", [], { depends_on: ["unanswered approval"] }), []);
    if (kind === "competing") { f.period.checkpoint(checkpoint("unfinished", ["one"]), [evidence("one")]); f.period.settle({ competing: true }); }
    if (kind === "missing") f.period.settle();
    if (kind === "unsupported-tool") assert.equal(f.period.allowTool("subagent"), false);
    assert.equal(f.period.active, false, kind); assert.equal(f.period.guarded, true, kind);
    assert.equal(f.period.settle().continue, false, kind);
  }
});

test("multi-unit work consumes one allowance and stops at a declared completion", () => {
  const f = fixture(); f.start();
  f.period.reserve(); assert.equal(f.period.allowTool("read"), true);
  f.period.reserve(); assert.equal(f.period.allowTool("write"), true);
  f.period.checkpoint(checkpoint("unfinished", ["edit"]), [evidence("edit", "actual changed source")]);
  const next = f.period.settle(); assert.equal(next.continue, true); assert.equal(next.next_step, "Run the next authorized unit");
  assert.equal(f.period.snapshot().continuations_used, 1);
  f.period.reserve(); assert.equal(f.period.allowTool("bash"), true);
  f.period.checkpoint(checkpoint("completed", ["validation"]), [evidence("validation", "focused checks completed")]);
  assert.equal(f.period.snapshot().status, "completed"); assert.equal(f.period.snapshot().requests_used, 3);
  assert.equal(f.period.active, false); assert.equal(f.period.guarded, true);
  assert.throws(() => f.period.reserve(), /not active/); assert.equal(f.period.settle().continue, false);
  assert.deepEqual(f.records.map(x => x.revision), f.records.map((_, i) => i + 1));
});

test("unverified journal writes withhold admission and poison further work", () => {
  const f = fixture(); f.start(); f.fail();
  assert.throws(() => f.period.reserve(), /append unverified/);
  assert.equal(f.period.snapshot().requests_used, 0); assert.equal(f.period.active, false); assert.equal(f.period.guarded, true);
  assert.throws(() => f.period.reserve(), /unverified/);
});

test("malformed or conflicting native snapshots fail closed rather than disappearing", () => {
  const f = fixture(); f.start();
  for (const mutate of [x => x.version = 3, x => x.version = 1, x => x.policy = "review", x => x.rules = Array(9).fill("x"), x => x.menu_nudges = 1, x => x.extra = true, x => x.requests_used = -1, x => x.binding.native_id = randomUUID(), x => x.revision = 2]) {
    const rows = structuredClone(f.records); mutate(rows[0]);
    const restored = createWorkPeriod({ ...f.options, journal: { load: () => rows, append() { assert.fail("invalid history must never be repaired"); } } });
    assert.equal(restored.guarded, true); assert.equal(restored.active, false); assert.throws(() => restored.start({ ...f.grant, id: randomUUID() }));
  }
});

test("one response cannot evade the finite local tool-admission allowance", () => {
  const f = fixture(); f.grant.requests = 1; f.start(); f.period.reserve();
  for (let i = 0; i < 16; i++) assert.equal(f.period.allowTool("read"), true);
  assert.equal(f.period.allowTool("read"), false);
  assert.equal(f.period.snapshot().tools_used, 16); assert.equal(f.period.snapshot().reason, "tools_exhausted");
});

test("unverified tool admission withholds the effect without refunding its request", () => {
  const f = fixture(); f.start(); f.period.reserve(); f.fail();
  assert.throws(() => f.period.allowTool("write"), /append unverified/);
  assert.equal(f.period.snapshot().requests_used, 1); assert.equal(f.period.snapshot().tools_used, 0);
  assert.equal(f.period.active, false); assert.equal(f.period.guarded, true);
});

test("alternating old observations cannot manufacture fresh progress", () => {
  const f = fixture(); f.start();
  for (const fingerprint of ["a", "b", "a"]) {
    f.period.reserve(); f.period.allowTool("read");
    f.period.checkpoint({ status: "unfinished", summary: "Observed local data", next_step: "Finish the remaining unit", evidence: ["read"], human_required: false, depends_on: [] }, [{ id: "read", fingerprint: fingerprint.repeat(64) }]);
  }
  assert.equal(f.period.snapshot().status, "blocked"); assert.equal(f.period.snapshot().reason, "no_progress");
});

test("plain-text settlement can consume an earlier unfinished checkpoint only once", () => {
  const f = fixture(); f.start(); f.period.reserve(); f.period.allowTool("write");
  f.period.checkpoint({ status: "unfinished", summary: "First unit done", next_step: "Finish remaining unit", evidence: ["first"], human_required: false, depends_on: [] }, [{ id: "first", fingerprint: "a".repeat(64) }]);
  f.period.reserve(); assert.equal(f.period.settle().continue, true); assert.equal(f.period.settle().continue, false);
  f.period.reserve(); assert.equal(f.period.settle().continue, false); assert.equal(f.period.snapshot().reason, "no_new_checkpoint");
});

test("new local tool admission invalidates an earlier checkpoint", () => {
  const f = fixture(); f.start(); f.period.reserve(); f.period.allowTool("write");
  f.period.checkpoint({ status: "unfinished", summary: "First unit done", next_step: "Finish remaining unit", evidence: ["first"], human_required: false, depends_on: [] }, [{ id: "first", fingerprint: "a".repeat(64) }]);
  f.period.reserve(); f.period.allowTool("edit");
  assert.equal(f.period.snapshot().checkpoint, null); assert.equal(f.period.settle().continue, false); assert.equal(f.period.snapshot().reason, "missing_checkpoint");
});

test("fully recovered inherited history carries no authority into a new native conversation", () => {
  const f = fixture(); f.start(); f.period.reserve(); f.period.stop("operator_stop");
  f.period.recover({ interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true });
  const binding = { ...f.options.binding, native_id: randomUUID(), launch_id: randomUUID() };
  const next = createWorkPeriod({ ...f.options, binding, generation: randomUUID() });
  assert.equal(next.active, false); assert.equal(next.guarded, false);
  next.start({ ...f.grant, id: randomUUID() });
  assert.deepEqual(next.snapshot().binding, binding); assert.equal(next.snapshot().requests_used, 0);
});

const v1 = rows => rows.map(({ policy, rules, rules_digest, menu_nudges, ...row }) => ({ ...row, version: 1 }));
function v1History(steps = f => f.period.reserve()) {
  const f = fixture(); f.start(); steps(f);
  return { rows: v1(f.records), options: f.options, grant: f.grant };
}

test("grant carries policy and inline rules into every snapshot", () => {
  const f = fixture();
  const s = f.period.start({ ...f.grant, policy: "yolo", rules: ["id='p', kind='plan', answer='agent'"] });
  assert.equal(s.version, 2); assert.equal(s.policy, "yolo");
  assert.deepEqual(s.rules, ["id='p', kind='plan', answer='agent'"]); assert.equal(s.menu_nudges, 0);
  f.period.reserve();
  assert.equal(f.period.snapshot().policy, "yolo"); assert.deepEqual(f.period.snapshot().rules, s.rules);
  const plain = fixture(); plain.start();
  assert.equal(plain.period.snapshot().policy, "rules"); assert.deepEqual(plain.period.snapshot().rules, []);
});

test("invalid policy or oversized rules refuse the grant", () => {
  for (const extra of [{ policy: "review" }, { rules: Array(9).fill("id='x', answer='stop'") }, { rules: ["x".repeat(1025)] }, { rules: [""] }, { rules: "id='x'" }]) {
    const f = fixture();
    assert.throws(() => f.period.start({ ...f.grant, ...extra })); assert.equal(f.records.length, 0); assert.equal(f.period.failure, undefined);
  }
});

test("grant size leaves room for the largest later row, so a bound-size period stops cleanly", () => {
  const rules = n => [...Array(7).fill("r".repeat(1024)), "r".repeat(n)];
  const accepts = n => { const f = fixture(); try { f.period.start({ ...f.grant, rules: rules(n) }); return true; } catch (error) { assert.match(error.message, /grant too large/); assert.equal(f.records.length, 0); assert.equal(f.period.failure, undefined); return false; } };
  let low = 1, high = 1024;
  assert.equal(accepts(low), true); assert.equal(accepts(high), false);
  while (high - low > 1) { const mid = (low + high) >> 1; if (accepts(mid)) low = mid; else high = mid; }
  assert.equal(accepts(low + 1), false, "one byte over the bound is refused");
  for (const finish of [f => f.period.stop("operator_stop"), f => assert.throws(() => f.period.reserve(), /exhausted/)]) {
    const f = fixture(); f.grant.requests = 1; f.period.start({ ...f.grant, rules: rules(low) });
    f.period.reserve(); finish(f);
    assert.equal(f.period.failure, undefined); assert.equal(f.period.active, false);
    assert.equal(f.records.at(-1).action, "stop"); assert.equal(createWorkPeriod(f.options).failure, undefined);
  }
});

test("policy and rules cannot change mid-period in the journal", () => {
  for (const mutate of [x => x.policy = "yolo", x => x.rules = ["id='x', answer='stop'"]]) {
    const f = fixture(); f.start(); f.period.reserve();
    const rows = structuredClone(f.records); mutate(rows[1]);
    const restored = createWorkPeriod({ ...f.options, journal: { load: () => rows, append() { assert.fail("invalid history must never be repaired"); } } });
    assert.ok(restored.failure);
  }
});

test("the rules digest is fixed at the grant and cannot change mid-period in the journal", () => {
  const digest = "a".repeat(64), f = fixture();
  assert.equal(f.period.start({ ...f.grant, rules_digest: digest }).rules_digest, digest);
  f.period.reserve(); assert.equal(f.period.snapshot().rules_digest, digest);
  const rows = structuredClone(f.records); rows[1].rules_digest = "b".repeat(64);
  assert.ok(createWorkPeriod({ ...f.options, journal: { load: () => rows, append() { assert.fail("invalid history must never be repaired"); } } }).failure);
  const plain = fixture(); plain.start(); assert.equal(plain.period.snapshot().rules_digest, null);
  for (const extra of [{ rules_digest: "xyz" }, { rules_digest: 7 }, { policy: "yolo", rules_digest: digest }]) {
    const g = fixture();
    assert.throws(() => g.period.start({ ...g.grant, ...extra })); assert.equal(g.records.length, 0);
  }
});

test("menu nudge continues once without a checkpoint", () => {
  const f = fixture(); f.start(); f.period.reserve();
  const first = f.period.nudge("You ended with a menu. Resolve it with aoe_afk_answer.");
  assert.equal(first.continue, true); assert.equal(first.id, f.grant.id); assert.equal(first.request, 1);
  assert.equal(first.next_step, "You ended with a menu. Resolve it with aoe_afk_answer.");
  assert.equal(f.period.snapshot().menu_nudges, 1); assert.equal(f.period.snapshot().continuations_used, 1);
  assert.equal(f.period.snapshot().action, "nudge"); assert.equal(f.period.snapshot().status, "owned");
  f.period.reserve();
  assert.deepEqual(f.period.nudge("again"), { continue: false });
  assert.equal(f.period.active, true);
  const restored = createWorkPeriod(f.options);
  assert.equal(restored.failure, undefined); assert.equal(restored.snapshot().menu_nudges, 1);
});

test("menu nudge shares settlement bookkeeping and needs an owned request", () => {
  const pending = fixture(); pending.start();
  assert.deepEqual(pending.period.nudge("menu"), { continue: false });
  const settled = fixture(); settled.start(); settled.period.reserve();
  settled.period.checkpoint(checkpoint("unfinished", ["one"]), [evidence("one")]);
  assert.equal(settled.period.settle().continue, true);
  assert.deepEqual(settled.period.nudge("menu"), { continue: false }, "one continuation per request");
  const nudged = fixture(); nudged.start(); nudged.period.reserve();
  nudged.period.checkpoint(checkpoint("unfinished", ["one"]), [evidence("one")]);
  assert.equal(nudged.period.nudge("menu").continue, true);
  assert.equal(nudged.period.settle().continue, false, "nudge consumed this request's settlement");
  const last = fixture(); last.grant.requests = 1; last.start(); last.period.reserve();
  assert.deepEqual(last.period.nudge("menu"), { continue: false });
});

test("forged nudge rows fail closed on load", () => {
  const swapped = { status: "unfinished", summary: "forged", next_step: "forged", evidence: [], human_required: false, depends_on: [] };
  for (const mutate of [x => x.menu_nudges = 2, x => x.continuations_used = 0, x => x.status = "pending", x => x.checkpoint = swapped]) {
    const f = fixture(); f.start(); f.period.reserve(); f.period.nudge("menu");
    const rows = structuredClone(f.records); mutate(rows[2]);
    assert.ok(createWorkPeriod({ ...f.options, journal: { load: () => rows, append() {} } }).failure);
  }
  const f = fixture(); f.start(); f.period.reserve();
  const rows = structuredClone(f.records); rows[1].menu_nudges = 1;
  assert.ok(createWorkPeriod({ ...f.options, journal: { load: () => rows, append() {} } }).failure, "only a nudge may set menu_nudges");
});

test("v1 journal rows load as rules policy", () => {
  const { rows, options } = v1History();
  const period = createWorkPeriod({ ...options, journal: { load: () => rows, append: r => rows.push(r) } });
  assert.equal(period.failure, undefined);
  assert.equal(period.snapshot().version, 2); assert.equal(period.snapshot().policy, "rules");
  assert.deepEqual(period.snapshot().rules, []); assert.equal(period.snapshot().menu_nudges, 0);
});

test("v1 mid-period and terminal histories replay, recover and accept a v2 period", () => {
  const histories = [
    f => f.period.reserve(),
    f => { f.period.reserve(); f.period.allowTool("read"); f.period.checkpoint(checkpoint("unfinished", ["one"]), [evidence("one")]); f.period.settle(); f.period.reserve(); },
    f => { f.period.reserve(); f.period.checkpoint(checkpoint("completed", [], {}), []); },
    f => { f.period.reserve(); f.period.stop("operator_stop"); },
    f => { f.period.reserve(); f.period.stop("operator_stop"); f.period.recover({ interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true }); },
  ];
  for (const steps of histories) {
    const { rows, options, grant } = v1History(steps);
    const period = createWorkPeriod({ ...options, journal: { load: () => structuredClone(rows), append: r => rows.push(structuredClone(r)) } });
    assert.equal(period.failure, undefined);
    period.recover({ interactive: true, settled: true, transformed: false, handled: false, failed: false, new_user_entry: true });
    assert.throws(() => period.start(grant), /replayed/);
    period.start({ ...grant, id: randomUUID() });
    assert.ok(rows.slice(rows.findIndex(r => r.version === 2)).every(r => r.version === 2));
    const reloaded = createWorkPeriod({ ...options, journal: { load: () => structuredClone(rows), append() {} } });
    assert.equal(reloaded.failure, undefined); assert.equal(reloaded.snapshot().version, 2);
  }
});

test("v1 rows with v2 fields, or unknown versions, fail closed", () => {
  for (const mutate of [x => x.policy = "rules", x => x.menu_nudges = 0, x => x.rules_digest = null, x => x.version = 3]) {
    const { rows, options } = v1History(); mutate(rows[0]);
    assert.ok(createWorkPeriod({ ...options, journal: { load: () => rows, append() {} } }).failure);
  }
});

test("a row with no room left for its stop row stops as record_capacity instead of poisoning the runtime", () => {
  // The review probe: in-limit inline rules pass the start guard, then an ordinary checkpoint outgrows the record.
  const f = fixture(); f.period.start({ ...f.grant, rules: Array(8).fill("r".repeat(760)) }); f.period.reserve();
  const s = f.period.checkpoint(checkpoint("unfinished", ["one"], { summary: "s".repeat(1600) }), [evidence("one")]);
  assert.equal(s.status, "blocked"); assert.equal(s.reason, "record_capacity"); assert.equal(s.checkpoint, null);
  assert.equal(f.period.failure, undefined); assert.equal(f.period.active, false);
  assert.deepEqual([f.records.at(-1).action, f.records.at(-1).status, f.records.at(-1).reason], ["stop", "blocked", "record_capacity"]);
  assert.ok(f.records.every(r => Buffer.byteLength(JSON.stringify(r)) <= 8192));
  const reloaded = createWorkPeriod(f.options);
  assert.equal(reloaded.failure, undefined); assert.equal(reloaded.snapshot().reason, "record_capacity");
});

test("record capacity refuses a continuation or nudge cleanly", () => {
  // Five tool rows put the checkpoint at revision 8, so the row after it is revision 9 and its stop row revision 10.
  const big = () => { const f = fixture(); f.period.start({ ...f.grant, rules: Array(8).fill("r".repeat(760)) }); f.period.reserve(); for (let i = 0; i < 5; i++) assert.ok(f.period.allowTool("read")); return f; };
  const fill = (f, size) => f.period.checkpoint(checkpoint("unfinished", ["one"], { summary: "s".repeat(size) }), [evidence("one")]);
  // The largest checkpoint that still fits leaves its stop row at exactly 8192 bytes, one short for the next row's.
  let low = 1, high = 2048;
  while (high - low > 1) { const mid = (low + high) >> 1, f = big(); if (fill(f, mid).status === "owned") low = mid; else high = mid; }
  const settled = big(); fill(settled, low);
  assert.equal(settled.period.settle().continue, false);
  const nudged = big(); fill(nudged, low);
  assert.equal(nudged.period.nudge("You ended with a menu. Resolve it with aoe_afk_answer.").continue, false);
  for (const f of [settled, nudged]) {
    assert.equal(f.period.failure, undefined); assert.equal(f.period.snapshot().reason, "record_capacity");
    assert.equal(createWorkPeriod(f.options).failure, undefined);
  }
});
