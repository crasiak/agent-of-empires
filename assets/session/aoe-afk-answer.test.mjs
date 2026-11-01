import test from "node:test";
import assert from "node:assert/strict";
import { askUserMenu, createAnswerClient, parseOnArgs, runInput, answeredBy, splitArgs, textTail, promptLike, BASH_DEFAULT_TIMEOUT_S } from "./aoe-afk-answer.mjs";

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

test("ask_user maps a single question with options", () => {
  assert.deepEqual(askUserMenu({ question: "Which DB?", context: "storage", options: [{ title: "SQLite" }, { title: "Postgres" }], allowFreeform: false }), {
    source: "ask_user", format: "tool.ask_user", kind: "choice", question: "Which DB?", subject: "storage",
    options: [{ key: "1", label: "SQLite" }, { key: "2", label: "Postgres" }], free_text: false,
    raw: JSON.stringify({ question: "Which DB?", context: "storage", options: [{ title: "SQLite" }, { title: "Postgres" }], allowFreeform: false }) });
  assert.equal(askUserMenu({ question: "Q?", options: [{ title: "A" }] }).free_text, true);
  assert.equal(askUserMenu({ question: "Q?", options: [{ title: "A" }] }).subject, null);
});

test("ask_user batches are not mapped", () => {
  assert.equal(askUserMenu({ questions: [{ question: "a" }, { question: "b" }] }), null);
  assert.equal(askUserMenu({ question: "a", options: [{ title: "x" }], allowMultiple: true }), null);
  assert.equal(askUserMenu({ question: "a", options: [] }), null);
  assert.equal(askUserMenu({ question: "a" }), null);
  assert.equal(askUserMenu(undefined), null);
});

test("textTail keeps the last 4096 bytes without splitting a character", () => {
  assert.equal(textTail("short"), "short");
  const long = "x" + "€".repeat(2000);
  const tail = textTail(long);
  assert.ok(Buffer.byteLength(tail) <= 4096); assert.ok(long.endsWith(tail)); assert.ok(!tail.includes("\uFFFD"));
  assert.equal(tail.length, 1365);
});

// Pi bash result shapes (pi coding-agent core/tools/bash.ts formatOutput/appendStatus).
const piStatus = (text, status) => `${text ? `${text}\n\n` : ""}${status}`;
const exited = (text, code = 1) => piStatus(text, `Command exited with code ${code}`);
const timedOut = (text, secs = 600) => piStatus(text, `Command timed out after ${secs} seconds`);
const truncated = (text, a, b) => `${text}\n\n[Showing lines ${a}-${b} of ${b}. Full output: /tmp/pi-bash-1.log]`;

test("promptLike flags prompts only when Pi reports a non-zero exit, timeout or abort", () => {
  assert.equal(promptLike(exited("Need to install the following packages:\ncreate-vite@5.2.0\nOk to proceed? (y/N)")),
    "Need to install the following packages:\ncreate-vite@5.2.0\nOk to proceed? (y/N)");
  assert.equal(promptLike(timedOut("Password:")), "Password:");
  assert.ok(promptLike(timedOut("The authenticity of host 'h (10.0.0.1)' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?")));
  assert.ok(promptLike(piStatus("Overwrite? [Y/n] ", "Command aborted")));
  assert.ok(promptLike(exited("? Select a framework: ›", 130)));
  assert.ok(promptLike(timedOut(truncated("x\nOk to proceed? (y/N)", 1, 2))));
  assert.ok(promptLike(timedOut("Continue? [y/N]:")));
  assert.equal(promptLike(timedOut("")), null);
  assert.equal(promptLike(exited("error: could not compile `aoe`")), null);
  assert.equal(promptLike(""), null);
  assert.equal(promptLike(undefined), null);
  assert.equal(BASH_DEFAULT_TIMEOUT_S, 600);
});

test("promptLike never flags exit-0 output, even when it ends like a prompt", () => {
  for (const out of ['assets/session/x.test.mjs:12:    "Ok to proceed? (y/N)"', "        let reply = client.request(packet).await?", "## Why not anchor the regex?",
    "ℹ tests 274\nℹ pass 271\nℹ fail 1", "Ok to proceed? (y/N)", "Password:", truncated("Continue? (y/N)", 1, 1)]) assert.equal(promptLike(out), null, out);
});

test("promptLike anchors yes/no markers to the line end", () => {
  assert.equal(promptLike(exited('src/x.rs:4: "(y/N)" is matched here')), null);
  assert.equal(promptLike(exited("found [Y/n] marker in docs")), null);
});

test("promptLike keeps the last 12 non-empty lines", () => {
  const lines = Array.from({ length: 20 }, (_, i) => `line ${i}`);
  const tail = promptLike(exited([...lines, "", "Continue? (y/N)  ", ""].join("\n")));
  assert.equal(tail.split("\n").length, 12);
  assert.equal(tail.split("\n")[0], "line 9");
  assert.equal(tail.split("\n").at(-1), "Continue? (y/N)");
});
