import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, existsSync, realpathSync, mkdirSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import net, { createConnection } from "node:net";
import { EventEmitter } from "node:events";
import { syncBuiltinESMExports } from "node:module";
import test from "node:test";
import install from "../../assets/session/aoe-session-id.js";

const source = "22222222-2222-4222-8222-222222222222";
const sid = "11111111-1111-4111-8111-111111111111";

function setup(t) {
  const dir = mkdtempSync(join(tmpdir(), "aoe-pi-arc-cursor-"));
  const prior = Object.fromEntries(
    ["AOE_SESSION_ID_FILE", "AOE_SESSION_SOURCE", "AOE_USAGE_BIN", "AOE_INSTANCE_ID", "AOE_HOOK_BASE"].map(
      (key) => [key, process.env[key]],
    ),
  );
  process.env.AOE_SESSION_ID_FILE = join(dir, "session_id");
  process.env.AOE_SESSION_SOURCE = source;
  delete process.env.AOE_USAGE_BIN;
  process.env.AOE_INSTANCE_ID = "aoe-test";
  // The absent sandbox mount: the hook status writer registers but writes nothing.
  process.env.AOE_HOOK_BASE = "/root/.pi/aoe-session";
  t.after(async () => {
    await handlers.get("session_shutdown")({}, ctx);
    for (const [key, value] of Object.entries(prior)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(dir, { recursive: true, force: true });
  });
  const handlers = new Map();
  // Pi runs every handler registered for an event, in order; the first one's result is the arc's.
  install({
    on: (name, handler) => {
      const prior = handlers.get(name);
      handlers.set(name, prior ? async (...args) => {
        const result = await prior(...args);
        await handler(...args);
        return result;
      } : handler);
    },
  });
  const state = { sid, leaf: null };
  const ctx = {
    isIdle: () => true,
    sessionManager: {
      getSessionId: () => state.sid,
      getSessionFile: () => join(dir, `${state.sid}.jsonl`),
      getLeafId: () => state.leaf,
    },
  };
  const cursor = join(dir, `work_arc_cursor.${source}`);
  const socketFile = join(dir, `work_arc_socket.${source}`);
  return { handlers, state, ctx, cursor, socketFile };
}

function request(socket, payload) {
  return new Promise((resolve, reject) => {
    const connection = createConnection(socket);
    let received = "";
    connection.setTimeout(3000, () => connection.destroy(new Error("request timed out")));
    connection.on("error", reject);
    connection.on("data", (chunk) => {
      received += chunk.toString("utf8");
      if (received.includes("\n")) {
        connection.destroy();
        resolve(JSON.parse(received.split("\n", 1)[0]));
      }
    });
    connection.on("connect", () => connection.write(JSON.stringify(payload) + "\n"));
  });
}

test("Pi cursor follows the active leaf, not the file's latest entry", async (t) => {
  const { handlers, state, ctx, cursor } = setup(t);
  await handlers.get("session_start")({ reason: "startup" }, ctx);
  const initial = JSON.parse(readFileSync(cursor, "utf8"));
  assert.deepEqual(
    { session_id: initial.session_id, source: initial.source, leaf_id: initial.leaf_id },
    { session_id: sid, source, leaf_id: null },
  );
  assert.equal(initial.version, 1);
  assert.equal(initial.process_id, process.pid, "cursor identifies its actual socket publisher");
  assert.ok(!Number.isNaN(Date.parse(initial.updated_at)));

  state.leaf = "oldbranch";
  await handlers.get("session_tree")({ fromId: "newbranch", toId: "oldbranch" }, ctx);
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).leaf_id, "oldbranch");

  state.leaf = "newbranch";
  await handlers.get("agent_settled")({}, ctx);
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).leaf_id, "newbranch");
  assert.ok(!readFileSync(cursor, "utf8").includes("prompt text"));
});

test("socket publisher resolves a symlinked temporary directory before binding and publication", async (t) => {
  const previous = process.env.TMPDIR;
  const root = mkdtempSync(join(realpathSync(tmpdir()), "ap-"));
  const physical = join(root, "real");
  const alias = join(root, "alias");
  mkdirSync(physical);
  symlinkSync(physical, alias);
  process.env.TMPDIR = alias;
  const { handlers, state, ctx, socketFile } = setup(t);
  t.after(() => {
    if (previous === undefined) delete process.env.TMPDIR;
    else process.env.TMPDIR = previous;
    rmSync(root, { recursive: true, force: true });
  });
  state.leaf = "leaf";
  ctx.model = { provider: "fake", id: "selected" };
  ctx.modelRegistry = { complete: () => assert.fail("status cannot infer") };
  await handlers.get("session_start")({}, ctx);
  const published = readFileSync(socketFile, "utf8").trim();
  assert.equal(published, realpathSync(published), "consumer admits only physical no-follow paths");
  assert.ok(published.startsWith(physical + "/"));
  assert.equal((await request(published, { op: "status", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "physical" })).ok, true);
});

test("bounded work-arc requests use the owning Pi process model route", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  const calls = [];
  ctx.model = { provider: "router", id: "auto" };
  ctx.modelRegistry = {
    complete: async (...args) => {
      calls.push(args);
      return {
        provider: "anthropic", model: "test-physical", stopReason: "stop",
        content: [{ type: "text", text: '{"goal":"G","now":"N","remaining":"R","next":"X"}' }],
        usage: { input: 12, output: 9, cost: { total: 0.01 } },
      };
    },
  };
  state.leaf = "done";
  await handlers.get("session_start")({}, ctx);
  const socket = readFileSync(socketFile, "utf8").trim();
  const base = { op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "done", nonce: "request-1", input: "[user] Synthetic goal" };
  const foreign = await request(socket, { ...base, session_id: "foreign" });
  assert.equal(foreign.ok, false);
  assert.equal(calls.length, 0);
  const stale = await request(socket, { ...base, leaf_id: "other" });
  assert.equal(stale.ok, false);
  assert.equal(calls.length, 0);
  const result = await request(socket, base);
  assert.equal(result.ok, true);
  assert.equal(result.selected_route, "router/auto");
  assert.equal(result.provider, "anthropic");
  assert.equal(result.model, "test-physical");
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], ctx.model);
  assert.equal(calls[0][2].maxTokens, 512);
  assert.equal(calls[0][1].messages[0].content[0].text, base.input);
  assert.equal((await request(socket, base)).ok, false, "duplicate request nonce");
  assert.equal((await request(socket, { ...base, nonce: "request-2" })).ok, false, "five-minute floor");
  assert.equal(calls.length, 1);
  await handlers.get("session_shutdown")({}, ctx);
  assert.equal(existsSync(socket), false);
});

test("Pi cursor is invalidated on shutdown and session switch", async (t) => {
  const { handlers, state, ctx, cursor } = setup(t);
  state.leaf = "oldbranch";
  await handlers.get("session_start")({}, ctx);
  assert.ok(existsSync(cursor));
  await handlers.get("session_shutdown")({}, ctx);
  assert.equal(existsSync(cursor), false);

  await handlers.get("session_start")({}, ctx);
  state.sid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  state.leaf = null;
  await handlers.get("session_start")({}, ctx);
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).session_id, state.sid);
  await handlers.get("session_shutdown")({}, { sessionManager: { getSessionId: () => sid } });
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).session_id, state.sid);
});

test("idle heartbeat refreshes provenance without a model request", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "Date"] });
  const { handlers, state, ctx, cursor } = setup(t);
  state.leaf = "idle-leaf";
  ctx.modelRegistry = { complete: () => assert.fail("heartbeat must not spend") };
  await handlers.get("session_start")({}, ctx);
  const first = JSON.parse(readFileSync(cursor, "utf8"));
  t.mock.timers.tick(180_000);
  const latest = JSON.parse(readFileSync(cursor, "utf8"));
  assert.equal(latest.leaf_id, first.leaf_id);
  assert.ok(Date.parse(latest.updated_at) > Date.parse(first.updated_at));
  await handlers.get("session_shutdown")({}, ctx);
  t.mock.timers.tick(180_000);
  assert.equal(existsSync(cursor), false);
});

test("shutdown closes accepted sockets even when the model ignores abort", { timeout: 3000 }, async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  ctx.model = { provider: "test", id: "fake" };
  ctx.modelRegistry = { complete: () => { entered(); return new Promise(() => {}); } };
  state.leaf = "leaf";
  await handlers.get("session_start")({}, ctx);
  const socket = createConnection(readFileSync(socketFile, "utf8").trim());
  t.after(() => socket.destroy());
  socket.on("error", () => {});
  socket.on("connect", () => socket.write(JSON.stringify({
    op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "hung", input: "Synthetic",
  }) + "\n"));
  await started;
  await handlers.get("session_shutdown")({}, ctx);
  assert.equal(existsSync(socketFile), false);
});

test("busy cursor and heartbeat cannot generate until settled", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "Date"] });
  const { handlers, state, ctx, cursor, socketFile } = setup(t);
  ctx.isIdle = () => state.idle !== false;
  state.leaf = "leaf";
  let calls = 0;
  ctx.model = { provider: "fake", id: "model" };
  ctx.modelRegistry = { complete: async () => { calls++; throw new Error("synthetic failure"); } };
  await handlers.get("session_start")({}, ctx);
  const payload = { op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "busy", input: "Synthetic" };
  const socket = readFileSync(socketFile, "utf8").trim();
  state.idle = false;
  await handlers.get("input")({}, ctx);
  t.mock.timers.tick(180_000);
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).settled, false);
  assert.equal((await request(socket, payload)).ok, false);
  assert.equal(calls, 0);
  state.idle = true;
  await handlers.get("agent_settled")({}, ctx);
  assert.equal(JSON.parse(readFileSync(cursor, "utf8")).settled, true);
  const failed = await request(socket, payload);
  assert.equal(failed.ok, false);
  assert.equal(failed.usage, undefined, "SDK failure does not imply zero usage");
  assert.equal(calls, 1);
});

test("missing model route fails closed without a request", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  state.leaf = "leaf";
  await handlers.get("session_start")({}, ctx);
  const result = await request(readFileSync(socketFile, "utf8").trim(), {
    op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "absent", input: "Synthetic",
  });
  assert.equal(result.error, "unavailable");
  assert.equal(result.usage, undefined);
});

test("model deadline responds and cleans up even if fake model ignores abort", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  const { handlers, state, ctx, socketFile } = setup(t);
  let entered;
  const started = new Promise((resolve) => { entered = resolve; });
  state.leaf = "leaf";
  ctx.model = { provider: "fake", id: "model" };
  ctx.modelRegistry = { complete: () => { entered(); return new Promise(() => {}); } };
  await handlers.get("session_start")({}, ctx);
  const pending = request(readFileSync(socketFile, "utf8").trim(), {
    op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "deadline", input: "Synthetic",
  });
  await started;
  t.mock.timers.tick(150_000);
  const result = await pending;
  assert.equal(result.ok, false);
  assert.equal(result.usage, undefined);
});

test("hostile JSON envelopes are rejected and the next request still works", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  state.leaf = "leaf";
  let calls = 0;
  ctx.model = { provider: "fake", id: "selected" };
  ctx.modelRegistry = { complete: async () => { calls++; return { provider: "fake", model: "physical", stopReason: "stop", content: [{ type: "text", text: '{"goal":"G","now":"N","remaining":"R","next":"X"}' }] }; } };
  await handlers.get("session_start")({}, ctx);
  const socket = readFileSync(socketFile, "utf8").trim();
  for (const envelope of [null, [], "text", 1]) assert.equal((await request(socket, envelope)).ok, false);
  const good = await request(socket, { op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "good", input: "Synthetic" });
  assert.equal(good.ok, true);
  assert.equal(calls, 1);
});

test("decoded excerpt limit permits worst-case JSON wire escaping", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  state.leaf = "leaf";
  const input = '\u0001"\\'.repeat(8000);
  assert.equal(Buffer.byteLength(input), 24_000);
  ctx.model = { provider: "fake", id: "selected" };
  ctx.modelRegistry = { complete: async (_model, context) => {
    assert.equal(context.messages[0].content[0].text, input);
    return { provider: "fake", model: "physical", stopReason: "stop", content: [{ type: "text", text: '{"goal":"G","now":"N","remaining":"R","next":"X"}' }] };
  } };
  await handlers.get("session_start")({}, ctx);
  const result = await request(readFileSync(socketFile, "utf8").trim(), { op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "escaped", input });
  assert.equal(result.ok, true);
});

test("rejected oversized model output retains only bounded usage", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  state.leaf = "leaf";
  ctx.model = { provider: "fake", id: "selected" };
  ctx.modelRegistry = { complete: async () => ({
    provider: "fake", model: "physical", stopReason: "stop",
    content: [{ type: "text", text: "x".repeat(1025) }],
    usage: { input: 17, output: 9, cost: { total: 0.01 }, unbounded: "not retained" },
  }) };
  await handlers.get("session_start")({}, ctx);
  const result = await request(readFileSync(socketFile, "utf8").trim(), {
    op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "oversize", input: "Synthetic",
  });
  assert.equal(result.ok, false);
  assert.deepEqual(result.usage, { input: 17, output: 9, cost: { total: 0.01 } });
  assert.equal(result.text, undefined);
});

test("socket framing preserves a UTF-8 character split across data events", async (t) => {
  let accept;
  const original = net.createServer;
  t.mock.method(net, "createServer", (listener) => {
    accept = listener;
    return original(listener);
  });
  syncBuiltinESMExports();
  t.after(() => { t.mock.restoreAll(); syncBuiltinESMExports(); });
  const { handlers, state, ctx } = setup(t);
  state.leaf = "leaf";
  ctx.model = { provider: "fake", id: "selected" };
  let received;
  ctx.modelRegistry = { complete: async (_model, context) => {
    received = context.messages[0].content[0].text;
    return { provider: "fake", model: "physical", stopReason: "stop", content: [{ type: "text", text: '{"goal":"G","now":"N","remaining":"R","next":"X"}' }] };
  } };
  await handlers.get("session_start")({}, ctx);
  let finish;
  const reply = new Promise((resolve) => { finish = resolve; });
  const socket = new EventEmitter();
  socket.destroyed = false;
  socket.setTimeout = () => {};
  socket.destroy = () => { socket.destroyed = true; socket.emit("close"); };
  socket.end = (value) => { finish(JSON.parse(value)); socket.destroy(); };
  accept(socket);
  const frame = Buffer.from(JSON.stringify({ op: "generate", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf", nonce: "utf8", input: "Café" }) + "\n");
  const split = frame.indexOf(Buffer.from("é")) + 1;
  socket.emit("data", frame.subarray(0, split));
  socket.emit("data", frame.subarray(split));
  assert.equal((await reply).ok, true);
  assert.equal(received, "Café");
});

test("status checks route without inference, auth probes or attempt budget", async (t) => {
  const { handlers, state, ctx, socketFile } = setup(t);
  state.leaf = "leaf";
  let calls = 0;
  ctx.model = { provider: "fake", id: "virtual" };
  ctx.modelRegistry = {
    complete: async () => { calls++; throw new Error("fake generation"); },
    resolveModel: () => assert.fail("status must not resolve providers"),
    getApiKey: () => assert.fail("status must not probe auth"),
  };
  await handlers.get("session_start")({}, ctx);
  const socket = readFileSync(socketFile, "utf8").trim();
  const base = { op: "status", version: 1, instance_id: "aoe-test", session_id: sid, source, leaf_id: "leaf" };
  for (let n = 0; n < 24; n++) {
    const status = await request(socket, { ...base, nonce: `status-${n}` });
    assert.equal(status.ok, true);
    assert.equal(status.selected_route, "fake/virtual");
  }
  ctx.model = undefined;
  assert.equal((await request(socket, { ...base, nonce: "absent" })).error, "unavailable");
  ctx.model = { provider: "fake", id: "virtual" };
  assert.equal(calls, 0);
  await request(socket, { ...base, op: "generate", nonce: "first", input: "Synthetic" });
  assert.equal(calls, 1, "status does not consume attempt budget");
});
