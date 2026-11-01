import test from "node:test";
import assert from "node:assert/strict";
import { createAnswerClient, parseOnArgs, runInput, answeredBy, splitArgs } from "./aoe-afk-answer.mjs";

test("decide maps bridge replies and never throws", async () => {
  const ok = createAnswerClient({ request: async p => ({ answer_id: "a", menu: { kind: "choice" }, decision: { decision: "answer", choice: "1" } }) });
  assert.equal((await ok.decide({ run: {}, menu: {} })).decision.choice, "1");
  const broken = createAnswerClient({ request: async () => { throw Error("bridge gone"); } });
  assert.deepEqual((await broken.decide({ run: {}, menu: { q: 1 } })).decision, { decision: "stop", reason: "engine_unavailable", checks: [] });
  const slow = createAnswerClient({ request: () => new Promise(() => {}) }, { timeoutMs: 20 });
  assert.equal((await slow.decide({ run: {}, menu: {} })).decision.reason, "engine_unavailable");
  const malformed = createAnswerClient({ request: async () => ({ decision: { decision: "maybe" } }) });
  assert.equal((await malformed.decide({ run: {}, menu: {} })).decision.reason, "engine_unavailable");
});

test("decide sends exactly one of menu or text", async () => {
  const sent = [];
  const c = createAnswerClient({ request: async p => { sent.push(p); return { decision: { decision: "stop", reason: "x" } }; } });
  await c.decide({ run: { period: "p" }, text: { source: "agent_text", text: "t" } });
  assert.deepEqual(Object.keys(sent[0]).sort(), ["op", "run", "text"]);
  assert.equal(sent[0].op, "decide");
});

test("decide cuts menu.raw and text.text to 4096 bytes on a code point boundary", async () => {
  const sent = [];
  const c = createAnswerClient({ request: async p => { sent.push(p); return { decision: { decision: "stop", reason: "x" } }; } });
  const long = "a" + "é".repeat(4096);
  await c.decide({ run: {}, menu: { raw: long } });
  await c.decide({ run: {}, text: { source: "agent_text", text: long } });
  for (const value of [sent[0].menu.raw, sent[1].text.text]) {
    assert.ok(Buffer.byteLength(value) <= 4096);
    assert.ok(!value.includes("�"));
    assert.ok(long.startsWith(value));
  }
});

test("delivered, recognise and rulesCheck use the bridge ops", async () => {
  const sent = [];
  const c = createAnswerClient({ request: async p => { sent.push(p); return p.op === "recognise" ? { menu: { kind: "confirm" } } : { valid: true, count: 0, sources: [], format_warnings: [] }; } });
  await c.delivered("p", "a1", "ok");
  await c.delivered("p", undefined, "ok");
  assert.deepEqual((await c.recognise("agent_text", "Proceed? (y/N)")), { kind: "confirm" });
  assert.equal((await c.rulesCheck({ period: "p" })).valid, true);
  assert.deepEqual(sent.map(p => p.op), ["answer_delivered", "recognise", "rules_check"]);
  assert.deepEqual(sent[0], { op: "answer_delivered", period: "p", answer_id: "a1", delivered: "ok" });
  const gone = createAnswerClient({ request: async () => { throw Error("bridge gone"); } });
  await gone.delivered("p", "a1", "ok");
  assert.equal(await gone.recognise("agent_text", "x"), null);
  await assert.rejects(gone.rulesCheck({}), /bridge gone/);
});

test("parseOnArgs handles policy, repeated rules and positional limits", () => {
  assert.deepEqual(parseOnArgs(["--policy", "yolo", "120", "8"]), { minutes: "120", requests: "8", policy: "yolo", rules: [] });
  assert.deepEqual(parseOnArgs(["--rule", "id='a', answer='stop'", "--rule", "id='b', answer='deny'"]).rules.length, 2);
  assert.ok(parseOnArgs(["--policy", "review"]).error);
  assert.ok(parseOnArgs(["1", "2", "3"]).error);
  assert.ok(parseOnArgs(["--rule"]).error);
  assert.deepEqual(parseOnArgs([]), { policy: "rules", rules: [] });
});

test("runInput and answeredBy", () => {
  const s = { id: "p", task: "t", policy: "rules", rules: [], requests_used: 1, request_limit: 4 };
  assert.deepEqual(runInput(s, "/r"), { period: "p", item: null, goal: "t", policy: "rules", rules: [], cwd: "/r", requests_used: 1, request_limit: 4 });
  assert.equal(answeredBy("yolo"), "afk/yolo");
});

test("splitArgs keeps quoted rule bodies whole", () => {
  assert.deepEqual(splitArgs(`on --rule "id='a', answer='stop'" 60 4`), ["on", "--rule", "id='a', answer='stop'", "60", "4"]);
  assert.deepEqual(splitArgs("  status  "), ["status"]);
  assert.deepEqual(splitArgs(""), []);
  assert.deepEqual(splitArgs(`on --rule ''`), ["on", "--rule", ""]);
});
