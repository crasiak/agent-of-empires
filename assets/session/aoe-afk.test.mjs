import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdtempSync, writeFileSync, chmodSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { registerAfk, spawnBridge } from "./aoe-afk.mjs";

function fixture(flag = true) {
  const binding = { instance_id: "instance", profile: "default", native_id: "native", launch_id: randomUUID() };
  const bootstrap = { version: 1, app_dir: "/isolated/app", aoe_bin: "/isolated/aoe", binding };
  const handlers = new Map();
  const f = { binding, bootstrap, handlers, wall: 1000, mono: 0, native: "native", policy: null, probe: null, acks: [], generations: [], closed: 0, scheduled: [], statuses: [], calls: [], failure: false };
  const api = new Proxy({ registerFlag(name, spec) { assert.equal(name, "aoe-afk-binding"); assert.equal(spec.type, "string"); }, getFlag: () => flag ? JSON.stringify(bootstrap) : undefined,
    on(name, handler) { assert.ok(["session_start", "session_shutdown", "context"].includes(name), `forbidden handler ${name}`); handlers.set(name, handler); } },
    { get(target, property) { if (!(property in target)) throw new Error(`Forbidden Pi call: ${String(property)}`); return target[property]; } });
  f.ctx = { hasUI: true, sessionManager: { getSessionId: () => f.native }, ui: { setStatus: (_key, value) => f.statuses.push(value) } };
  registerAfk(api, { wall: () => f.wall, mono: () => f.mono, schedule(fn) { f.scheduled.push(fn); return () => f.scheduled.splice(f.scheduled.indexOf(fn), 1); },
    bridge(_bootstrap, generation) { f.generations.push(generation); return { async request(packet) { f.calls.push(packet.op); if (f.failure) throw new Error("unavailable"); if (packet.op === "poll") { const snapshot = { policy: f.policy && structuredClone(f.policy), probe: f.probe && structuredClone(f.probe) }; if (f.holdNextPoll) { f.holdNextPoll = false; return new Promise(resolve => { f.releasePoll = () => resolve(snapshot); }); } return snapshot; } f.acks.push(structuredClone(packet.ack)); return { acknowledged: true }; }, async close() { f.closed++; } }; } });
  f.start = () => handlers.get("session_start")({}, f.ctx);
  f.stop = () => handlers.get("session_shutdown")({}, f.ctx);
  f.context = (messages = []) => handlers.get("context")({ messages }, f.ctx);
  f.enable = (revision = 1) => {
    f.policy = { version: 1, binding, revision, window: randomUUID(), generation: f.generations.at(-1), enabled: true, issued_at_ms: f.wall, duration_ms: 60000, expires_at_ms: f.wall + 60000, mode: "control-only", allowance: 0 };
    f.probe = { version: 1, binding, challenge: randomUUID(), revision };
  };
  return f;
}

test("control-only reminder is ephemeral, preserves pending questions and cannot invoke agent operations", async () => {
  const f = fixture();
  await f.start();
  const question = { role: "assistant", content: [{ type: "toolCall", name: "ask_user", id: "waiting" }] };
  const original = structuredClone(question);
  assert.deepEqual((await f.context([question])).messages, [original]);
  f.enable();
  const result = await f.context([question]);
  assert.deepEqual(result.messages[0], original);
  assert.match(result.messages[1].content, /grants no new decision or action authority/);
  assert.match(result.messages[1].content, /Autonomous allowance: zero/);
  assert.equal(f.acks.at(-1).state, "control-only");
  assert.equal(f.acks.at(-1).challenge, f.probe.challenge);
  assert.equal(f.acks.at(-1).expires_at_ms, f.policy.expires_at_ms);
  const compacted = [{ role: "user", content: "compacted context" }];
  assert.equal((await f.context(compacted)).messages.length, 2);
  f.policy = { ...f.policy, revision: 2, enabled: false, expires_at_ms: null, duration_ms: 0 };
  f.probe = { ...f.probe, revision: 2, challenge: randomUUID() };
  const off = await f.context(result.messages);
  assert.deepEqual(off.messages, [original]);
  assert.equal(f.acks.at(-1).state, "off");
  await f.stop();
  assert.equal(f.closed, 1);assert.equal(f.scheduled.length, 0);
  assert.deepEqual((await f.context([question])).messages, [original]);
});

test("request boundary polls again after an idle snapshot predating Off", async () => {
  const f = fixture(); await f.start(); f.enable(); await f.context();
  f.holdNextPoll = true;
  f.scheduled[0]();
  assert.equal(typeof f.releasePoll, "function", "idle poll has captured enabled state");
  const polls = f.calls.filter(op => op === "poll").length;
  f.policy = { ...f.policy, revision: 2, enabled: false, expires_at_ms: null, duration_ms: 0 };
  f.probe = { ...f.probe, revision: 2, challenge: randomUUID() };
  const result = f.context();
  f.releasePoll();
  assert.deepEqual((await result).messages, []);
  assert.equal(f.calls.filter(op => op === "poll").length, polls + 1);
  assert.equal(f.acks.at(-1).state, "off");
  await f.stop();
});

test("monotonic expiry cannot be extended by wall rollback or replay", async () => {
  const f = fixture();await f.start();f.enable();
  await f.context();
  f.wall = -100000; f.mono = 60000;
  assert.deepEqual((await f.context()).messages, []);
  assert.equal(f.acks.at(-1).state, "expired");
  f.wall = 1000; f.mono = 60001;
  assert.deepEqual((await f.context()).messages, []);
  f.policy.revision = 0;f.probe.revision = 0;
  assert.deepEqual((await f.context()).messages, []);
  assert.equal(f.acks.at(-1).state, "invalidated");
  await f.stop();
});

test("reload, native replacement and inherited environment never reuse authority", async () => {
  const f = fixture();await f.start();f.enable();await f.context();
  const previous = f.generations.at(-1);
  await f.stop();await f.start();
  assert.notEqual(f.generations.at(-1), previous);
  assert.deepEqual((await f.context()).messages, []);
  assert.equal(f.acks.at(-1).state, "invalidated");
  f.enable(2);assert.equal((await f.context()).messages.length, 1);
  f.native = "different-native";
  assert.deepEqual((await f.context()).messages, []);
  await f.stop();await f.start();assert.equal(f.generations.length, 2);
  const child = fixture(false);
  await child.start();assert.deepEqual(child.calls, []);assert.deepEqual(child.generations, []);
  assert.deepEqual((await child.context()).messages, []);
});

test("missing helper and malformed policy fail closed at a request boundary", async () => {
  for (const change of [p => p.allowance = 1, p => p.version = 2, p => p.mode = "automatic", p => p.binding = { ...p.binding, profile: "other" }, p => p.expires_at_ms++, p => p.duration_ms = 0]) {
    const f = fixture();await f.start();f.enable();change(f.policy);
    assert.deepEqual((await f.context()).messages, []);
    assert.equal(f.acks.at(-1).state, "invalidated");await f.stop();
  }
  const f = fixture();await f.start();f.enable();await f.context();f.failure = true;
  assert.deepEqual((await f.context()).messages, []);
  assert.match(f.statuses.at(-1), /unavailable/);await f.stop();
});

test("idle polling observes a fresh probe without a model turn; old cleanup cannot overwrite a new runtime", async () => {
  const f = fixture();await f.start();f.enable();
  f.scheduled[0]();await f.context();
  assert.equal(f.acks.at(-1).challenge, f.probe.challenge);
  const count = f.acks.length;
  await f.context();assert.equal(f.acks.length, count);
  f.probe.challenge = randomUUID();await f.context();assert.equal(f.acks.length, count + 1);
  await f.stop();await f.stop();assert.equal(f.closed, 1);
  await f.start();assert.equal(f.scheduled.length, 1);await f.stop();
});

test("stdio bridge rejects broken, oversized and unresponsive helpers and reaps on close", async () => {
  const dir = mkdtempSync(join(tmpdir(), "aoe-afk-test-"));
  try {
    for (const body of ["process.stdout.write('x'.repeat(17000))", "process.stdout.write('invalid\\n')", "process.stdin.resume()", "process.exit(1)"]) {
      const exe = join(dir, "helper");
      writeFileSync(exe, `#!${process.execPath}\nprocess.stdin.once('data',()=>{${body}});\n`);chmodSync(exe, 0o700);
      const bridge = spawnBridge({ aoe_bin: exe }, randomUUID());
      await assert.rejects(bridge.request({ op: "poll" }));
      await bridge.close();
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
