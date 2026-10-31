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
  for (const mutate of [x => x.version = 2, x => x.extra = true, x => x.requests_used = -1, x => x.binding.native_id = randomUUID(), x => x.revision = 2]) {
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
