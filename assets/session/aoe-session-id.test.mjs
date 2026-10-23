import test from "node:test";
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import aoeSessionId, { attentionSummary, registerHookStatus } from "./aoe-session-id.js";

function bus() {
  const handlers = new Map();
  return {
    handlers,
    on(name, fn) { handlers.set(name, [...(handlers.get(name) ?? []), fn]); },
    fire(name, event) { for (const fn of handlers.get(name) ?? []) fn(event, {}); },
  };
}

function fixture(instanceId = "inst_1-A") {
  const root = mkdtempSync(join(tmpdir(), "aoe-session-id-hooks-"));
  const base = join(root, "aoe-hooks");
  const dir = join(base, instanceId);
  const pi = bus();
  registerHookStatus(pi, { base, instanceId });
  const read = name => (existsSync(join(dir, name)) ? readFileSync(join(dir, name), "utf8") : undefined);
  return { root, base, dir, pi, read, done: () => rmSync(root, { recursive: true, force: true }) };
}

const ask = (toolCallId, args) => ({ type: "tool_execution_start", toolCallId, toolName: "ask_user", args });
const askEnd = toolCallId => ({ type: "tool_execution_end", toolCallId, toolName: "ask_user", result: {}, isError: false });

test("each lifecycle event writes AoE's status vocabulary and the attention file", () => {
  const f = fixture();
  try {
    f.pi.fire("agent_start", { type: "agent_start" });
    assert.equal(f.read("status"), "running");
    for (const path of [f.base, f.dir]) assert.equal(statSync(path).mode & 0o7777, 0o700, path);
    assert.equal(statSync(join(f.dir, "status")).mode & 0o777, 0o600);

    const inode = () => statSync(join(f.dir, "status")).ino;
    const before = inode();
    f.pi.fire("tool_execution_start", { type: "tool_execution_start", toolCallId: "b1", toolName: "bash", args: { command: "ls" } });
    assert.equal(f.read("status"), "running", "an ordinary tool leaves the turn running");
    assert.notEqual(inode(), before, "an ordinary tool start refreshes the running write");
    assert.equal(f.read("attention"), undefined);

    f.pi.fire("tool_execution_start", ask("q1", { question: "Ship it?" }));
    assert.equal(f.read("status"), "waiting");
    assert.equal(f.read("attention"), "Ship it?");
    assert.equal(statSync(join(f.dir, "attention")).mode & 0o777, 0o600);

    f.pi.fire("tool_execution_start", { type: "tool_execution_start", toolCallId: "b2", toolName: "read", args: {} });
    assert.equal(f.read("status"), "waiting", "another tool starting does not answer the question");
    f.pi.fire("tool_execution_end", { type: "tool_execution_end", toolCallId: "b1", toolName: "bash", isError: false });
    assert.equal(f.read("status"), "waiting", "another tool ending does not answer the question");
    f.pi.fire("tool_execution_end", askEnd("q1"));
    assert.equal(f.read("status"), "running");
    assert.equal(f.read("attention"), undefined);

    for (const settle of ["agent_settled"]) {
      f.pi.fire("agent_start", { type: "agent_start" });
      f.pi.fire("tool_execution_start", ask(`q-${settle}`, { question: "Which?" }));
      assert.equal(f.read("status"), "waiting");
      f.pi.fire(settle, { type: settle });
      assert.equal(f.read("status"), "idle", settle);
      assert.equal(f.read("attention"), undefined, settle);
    }

    f.pi.fire("agent_start", { type: "agent_start" });
    f.pi.fire("agent_end", { type: "agent_end", messages: [], willRetry: true });
    assert.equal(f.read("status"), "running", "a run Pi is about to retry is not idle");
    f.pi.fire("agent_end", { type: "agent_end", messages: [] });
    assert.equal(f.read("status"), "running", "only agent_settled is idle: follow-ups may run after agent_end");

    assert.deepEqual(readdirSync(f.dir).sort(), ["status"], "atomic writes leave no temp files");
  } finally {
    f.done();
  }
});

test("attention summary strips controls, caps at 200 characters, and falls back", () => {
  assert.equal(attentionSummary({ question: "  Pick\none\tof\u0000these\u001b[31m\u007f\u0085\u2028now  " }), "Pick one of these [31m   now");
  assert.equal(attentionSummary({ questions: [{ question: "First?" }, { question: "Second?" }] }), "First?");
  for (const args of [undefined, null, {}, { question: "" }, { question: " \n\t " }, { question: 42 }, { questions: [] }, { questions: [null] }]) {
    assert.equal(attentionSummary(args), "Waiting for your answer", JSON.stringify(args));
  }
  const long = "\u{1F600}".repeat(150) + "x".repeat(150);
  const capped = attentionSummary({ question: long });
  assert.equal([...capped].length, 200);
  assert.equal(capped, "\u{1F600}".repeat(150) + "x".repeat(50), "the cap counts code points, never splitting a pair");
  assert.ok(!/[\p{Cc}]/u.test(attentionSummary({ question: "a\r\n\u0007b" })));

  const f = fixture();
  try {
    f.pi.fire("tool_execution_start", ask("q", { question: `${"y".repeat(300)}\n` }));
    assert.equal(f.read("attention"), "y".repeat(200));
  } finally {
    f.done();
  }
});

test("an unsafe base or instance directory gets no write", () => {
  const cases = {
    "symlinked instance dir": f => {
      const decoy = join(f.root, "decoy");
      mkdirSync(f.base, { mode: 0o700 });
      mkdirSync(decoy, { mode: 0o700 });
      symlinkSync(decoy, f.dir);
      return decoy;
    },
    "symlinked base dir": f => {
      const decoy = join(f.root, "decoy-base");
      mkdirSync(decoy, { mode: 0o700 });
      symlinkSync(decoy, f.base);
      return decoy;
    },
    "group-readable instance dir": f => {
      mkdirSync(f.dir, { recursive: true, mode: 0o700 });
      chmodSync(f.base, 0o700);
      chmodSync(f.dir, 0o750);
      return f.dir;
    },
    "world-readable base dir": f => {
      mkdirSync(f.dir, { recursive: true, mode: 0o700 });
      chmodSync(f.base, 0o755);
      return f.dir;
    },
    "sticky instance dir": f => {
      mkdirSync(f.dir, { recursive: true, mode: 0o700 });
      chmodSync(f.base, 0o700);
      chmodSync(f.dir, 0o1700);
      return f.dir;
    },
    "instance path is a file": f => {
      mkdirSync(f.base, { mode: 0o700 });
      writeFileSync(f.dir, "not a dir");
      return f.base;
    },
  };
  for (const [label, arrange] of Object.entries(cases)) {
    const f = fixture();
    try {
      const watched = arrange(f);
      const before = readdirSync(watched).sort();
      f.pi.fire("agent_start", { type: "agent_start" });
      f.pi.fire("tool_execution_start", ask("q", { question: "Allowed?" }));
      assert.deepEqual(readdirSync(watched).sort(), before, label);
      for (const name of ["status", "attention"]) assert.ok(!existsSync(join(watched, name)), `${label}: ${name}`);
    } finally {
      f.done();
    }
  }
});

test("an invalid instance id registers no status writer", () => {
  for (const instanceId of [undefined, "", "../escape", "a/b", "a b", "a.b", "id\n"]) {
    const root = mkdtempSync(join(tmpdir(), "aoe-session-id-badid-"));
    try {
      const pi = bus();
      registerHookStatus(pi, { base: join(root, "aoe-hooks"), instanceId });
      pi.fire("agent_start", { type: "agent_start" });
      pi.fire("tool_execution_start", ask("q", { question: "x" }));
      assert.deepEqual(readdirSync(root), [], JSON.stringify(instanceId));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test("handlers never throw into the agent", () => {
  const f = fixture();
  try {
    const hostile = { get toolName() { throw new Error("payload"); }, get toolCallId() { throw new Error("payload"); } };
    const args = { get question() { throw new Error("args"); } };
    // A base that cannot be created at all: its parent is a regular file.
    writeFileSync(join(f.root, "file"), "");
    const blocked = bus();
    registerHookStatus(blocked, { base: join(f.root, "file", "aoe-hooks"), instanceId: "inst" });
    for (const pi of [f.pi, blocked]) {
      for (const [name, event] of [
        ["agent_start", undefined],
        ["agent_start", null],
        ["tool_execution_start", hostile],
        ["tool_execution_start", ask("q", args)],
        ["tool_execution_start", null],
        ["tool_execution_end", hostile],
        ["tool_execution_end", undefined],
        ["agent_end", hostile],
        ["agent_end", null],
        ["agent_settled", undefined],
      ]) {
        assert.doesNotThrow(() => pi.fire(name, event), `${name} ${String(event)}`);
      }
    }
    assert.equal(f.read("attention"), undefined, "the last event settled the turn");
    assert.equal(f.read("status"), "idle");
  } finally {
    f.done();
  }
});

test("the extension registers the status writer only for a valid AOE_INSTANCE_ID", () => {
  const saved = { id: process.env.AOE_INSTANCE_ID, source: process.env.AOE_SESSION_SOURCE };
  try {
    delete process.env.AOE_SESSION_SOURCE;
    for (const [id, expected] of [["valid_id", true], ["bad/id", false], [undefined, false]]) {
      if (id === undefined) delete process.env.AOE_INSTANCE_ID; else process.env.AOE_INSTANCE_ID = id;
      const pi = bus();
      aoeSessionId(pi);
      // Registration only: firing would write under the real per-user base.
      for (const name of ["agent_start", "tool_execution_start", "tool_execution_end"]) {
        assert.equal(pi.handlers.has(name), expected, `${id}: ${name}`);
      }
      assert.equal(pi.handlers.has("agent_end"), false, "agent_end never writes idle");
      assert.ok(pi.handlers.has("agent_settled"), "usage reporting keeps its handler");
      assert.ok(pi.handlers.has("session_start"));
    }
  } finally {
    if (saved.id === undefined) delete process.env.AOE_INSTANCE_ID; else process.env.AOE_INSTANCE_ID = saved.id;
    if (saved.source === undefined) delete process.env.AOE_SESSION_SOURCE; else process.env.AOE_SESSION_SOURCE = saved.source;
  }
});
