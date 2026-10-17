import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { registerAfk, spawnBridge } from "./aoe-afk.mjs";
import { createTrace, observeBridge, observeDelegation, validEvent } from "./aoe-afk-observer-fixture.mjs";

// The side channel proves receipt by this controlled peer, never by Rust.
async function peerFixture(t, autoFirst = false) {
  const root = mkdtempSync("/tmp/aoe-close-");
  const path = join(root, "control.sock"), executable = join(root, "peer.mjs");
  const server = createServer();
  let raw, socket, reader;
  const issued = [];
  t.after(async () => {
    t.mock.timers.reset();
    try { await raw?.close(); await Promise.all(issued); }
    finally {
      reader?.close();
      socket?.destroy();
      await new Promise(resolve => server.close(resolve));
      rmSync(root, { recursive: true, force: true });
    }
  });
  writeFileSync(executable, `#!${process.execPath}
import { connect } from 'node:net';
import { createInterface } from 'node:readline';
const { control, autoFirst } = JSON.parse(process.argv[3]);
const socket = connect(control);
const reply = () => process.stdout.write(JSON.stringify({ok:true,value:{policy:null}})+'\\n');
let count = 0;
socket.on('connect', () => {
  createInterface({input:process.stdin}).on('line', () => {
    socket.write(String(++count)+'\\n');
    if (autoFirst && count === 1) reply();
  });
});
createInterface({input:socket}).on('line', command => {
  if (command === 'reply') reply();
  else process.exit(command === 'exit' ? 0 : 2);
});
`, { mode: 0o700 });
  const listening = once(server, "listening", { signal: t.signal });
  server.listen(path);
  await listening;
  const connected = once(server, "connection", { signal: t.signal });
  // A missed peer intervention must fail the test, not pass via a bridge deadline.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  raw = spawnBridge({ aoe_bin: executable, control: path, autoFirst }, "fixture");
  [socket] = await connected;
  reader = createInterface({ input: socket, signal: t.signal });
  const messages = reader[Symbol.asyncIterator]();
  return {
    raw,
    track(promise) { const tracked = outcome(promise); issued.push(tracked); return tracked; },
    async received(ordinal) {
      const message = await messages.next();
      assert.equal(message.done, false, "peer closed before receipt barrier");
      assert.equal(message.value, String(ordinal));
    },
    command(value) { socket.write(`${value}\n`); },
    async drained() {
      const remaining = [];
      for await (const message of messages) remaining.push(message);
      return remaining;
    },
  };
}

function observation() {
  const rows = [], state = { teardown: false, fault: false };
  const trace = createTrace(line => rows.push(JSON.parse(line)));
  return {
    rows, state, trace,
    teardown() { state.teardown = true; trace.emit("teardown", { teardown: true }); },
    assertComplete(expectedEvents) {
      assert.equal(trace.snapshot().invalid, false);
      assert.ok(rows.every(validEvent));
      assert.deepEqual(rows.map(row => row.event), expectedEvents);
      const boundary = index(rows, "teardown");
      rows.forEach((row, i) => {
        assert.equal(row.seq, i + 1);
        assert.equal(row.teardown, i >= boundary);
      });
      const requests = rows.filter(row => row.event === "request");
      const outcomes = rows.filter(row => ["resolved", "rejected"].includes(row.event));
      assert.equal(outcomes.length, requests.length);
      for (const request of requests) {
        const matches = outcomes.filter(row => row.bridge === request.bridge && row.request === request.request);
        assert.equal(matches.length, 1);
        assert.equal(matches[0].op, request.op);
        assert.equal(matches[0].kind, request.kind);
        assert.ok(matches[0].seq > request.seq);
      }
      assert.equal(rows.filter(row => row.event === "close").length, 1);
      assert.equal(rows.filter(row => row.event === "closed").length, 1);
    },
  };
}

function outcome(promise) {
  const tracked = promise.then(
    value => { tracked.settled = true; return { status: "resolved", value }; },
    error => { tracked.settled = true; return { status: "rejected", error }; },
  );
  tracked.settled = false;
  return tracked;
}
const index = (rows, event) => {
  const found = rows.findIndex(row => row.event === event);
  assert.notEqual(found, -1, `missing ${event}`);
  return found;
};

test("real bridge preserves pending and queued rejections across controlled close and pre-teardown peer exit", async t => {
  for (const mode of ["release", "close-pending", "close-queued", "peer-exit", "teardown-peer-exit"]) {
    await t.test(mode, { timeout: 10000 }, async t => {
      const peer = await peerFixture(t), seen = observation();
      const bridge = observeBridge(peer.raw, seen.trace, seen.state);
      const first = peer.track(bridge.request({ op: "poll" }));
      await peer.received(1);
      assert.equal(first.settled, false, "intervention requires a still-pending received request");
      const second = mode === "close-queued" ? peer.track(bridge.request({ op: "runtime", command: { kind: "stop" } })) : null;
      if (second) assert.equal(second.settled, false);
      if (mode === "release") {
        peer.command("reply");
        assert.deepEqual(await first, { status: "resolved", value: { policy: null } });
      } else if (mode.endsWith("peer-exit")) {
        if (mode === "teardown-peer-exit") seen.teardown();
        peer.command("exit");
        assert.equal((await first).status, "rejected");
        assert.equal(seen.rows.some(row => row.event === "close"), false);
      }
      if (!seen.state.teardown) seen.teardown();
      await bridge.close();
      const results = await Promise.all([first, ...(second ? [second] : [])]);
      assert.ok(results.every(result => result.status === (mode === "release" ? "resolved" : "rejected")));
      assert.deepEqual(await peer.drained(), [], "queued request was not received by the controlled peer");
      const events = {
        release: ["request", "resolved", "teardown", "close", "closed"],
        "close-pending": ["request", "teardown", "close", "rejected", "closed"],
        "close-queued": ["request", "request", "teardown", "close", "rejected", "rejected", "closed"],
        "peer-exit": ["request", "rejected", "teardown", "close", "closed"],
        "teardown-peer-exit": ["request", "teardown", "rejected", "close", "closed"],
      };
      seen.assertComplete(events[mode]);
      if (mode === "release") {
        assert.equal(seen.state.fault, false);
        assert.equal(seen.rows.some(row => row.event === "rejected"), false);
      } else {
        const rejected = seen.rows.filter(row => row.event === "rejected");
        assert.equal(rejected.length, mode === "close-queued" ? 2 : 1);
        assert.ok(rejected.every(row => row.teardown === (mode !== "peer-exit")));
        if (mode === "peer-exit") assert.ok(index(seen.rows, "rejected") < index(seen.rows, "teardown"));
        else if (mode === "teardown-peer-exit") {
          assert.ok(index(seen.rows, "teardown") < index(seen.rows, "rejected"));
          assert.ok(index(seen.rows, "rejected") < index(seen.rows, "close"));
        } else assert.ok(index(seen.rows, "close") < index(seen.rows, "rejected"));
        // The sealed observer's sticky fault covers teardown, not every earlier rejection.
        assert.equal(seen.state.fault, mode !== "peer-exit");
      }
    });
  }
});

test("real registerAfk invokes failed for both shutdown cancellation and pre-teardown refresh failure", async t => {
  for (const mode of ["release", "shutdown", "peer-exit"]) {
    await t.test(mode, { timeout: 10000 }, async t => {
      const peer = await peerFixture(t, true), seen = observation(), handlers = new Map();
      let current, failures = 0, stopped = false;
      const bridge = observeBridge(peer.raw, seen.trace, seen.state);
      // Only registration, refresh and bridge are real here; no delegation authority is exercised.
      const delegate = observeDelegation({
        attach(runtime) { current = runtime; },
        failed(runtime) { assert.equal(runtime, current); failures++; },
        async poll() {},
        async shutdown() {},
      }, seen.trace, seen.state);
      const binding = { instance_id: "fixture", profile: "fixture", native_id: "fixture", launch_id: randomUUID() };
      const bootstrap = { version: 1, binding, aoe_bin: "/unused", app_dir: "/unused" };
      const pi = { registerFlag() {}, getFlag: () => JSON.stringify(bootstrap), on: (event, fn) => handlers.set(event, fn) };
      const ctx = { hasUI: false, sessionManager: { getSessionId: () => binding.native_id } };
      registerAfk(pi, { delegation: delegate, bridge: () => bridge, schedule: () => () => { stopped = true; } });
      await handlers.get("session_start")({}, ctx);
      await peer.received(1);
      assert.equal(failures, 0, "successful startup is required before the controlled failure");
      assert.equal(current.state, "off");
      const refreshing = peer.track(current.refresh());
      await peer.received(2);
      assert.equal(refreshing.settled, false);
      assert.equal(failures, 0);
      if (mode === "release") peer.command("reply");
      if (mode === "peer-exit") peer.command("exit");
      if (mode !== "shutdown") {
        assert.equal((await refreshing).status, "resolved", "refresh catch retains its fulfillment contract");
        assert.equal(failures, mode === "peer-exit" ? 1 : 0);
      }
      seen.teardown();
      await handlers.get("session_shutdown")();
      assert.equal((await refreshing).status, "resolved", "close must not turn swallowed refresh failure into a new throw");
      assert.equal(stopped, true);
      assert.equal(failures, mode === "release" ? 0 : 1);
      assert.deepEqual(await peer.drained(), []);
      const events = {
        release: ["resolved", "teardown", "close", "closed"],
        shutdown: ["teardown", "close", "rejected", "failed", "failed_returned", "closed"],
        "peer-exit": ["rejected", "failed", "failed_returned", "teardown", "close", "closed"],
      };
      seen.assertComplete(["attach", "attached", "request", "resolved", "request", ...events[mode]]);
      if (mode === "release") {
        assert.equal(seen.state.fault, false);
        assert.equal(seen.rows.some(row => row.event === "failed"), false);
      } else {
        assert.ok(index(seen.rows, "rejected") < index(seen.rows, "failed"));
        assert.ok(index(seen.rows, "failed") < index(seen.rows, "failed_returned"));
        assert.equal(seen.rows.find(row => row.event === "failed").teardown, mode === "shutdown");
        if (mode === "shutdown") assert.ok(index(seen.rows, "close") < index(seen.rows, "rejected"));
        else assert.ok(index(seen.rows, "failed_returned") < index(seen.rows, "teardown"));
        assert.equal(current.state, "unavailable");
        assert.equal(current.refreshing, undefined);
        assert.equal(seen.state.fault, mode === "shutdown");
      }
    });
  }
});
