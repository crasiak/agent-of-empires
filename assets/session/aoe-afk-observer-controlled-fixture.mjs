import assert from "node:assert/strict";
import { readFileSync, writeFileSync, appendFileSync, unlinkSync, symlinkSync, chmodSync, writeSync } from "node:fs";
import { Socket } from "node:net";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { health, registerObserved } from "./aoe-afk-observer-fixture.mjs";
import { finalizeAudit, writeAudit } from "./aoe-afk-observer-audit.mjs";
import { privateProbe, observeRefresh, observeBridge, observeDelegation } from "./aoe-afk-observer-probes.mjs";
import scenarios from "./aoe-afk-observer-scenarios.json" with { type: "json" };

const scenario = process.argv[2];
assert.ok(scenarios.includes(scenario));
const root = process.env.AOE_FIXTURE_ROOT, auditFd = Number(process.env.AOE_FIXTURE_AUDIT_FD);
const controlFd = Number(process.env.AOE_FIXTURE_CONTROL_FD), commandsFd = Number(process.env.AOE_FIXTURE_COMMANDS_FD);
assert.ok([auditFd, controlFd, commandsFd].every(fd => Number.isInteger(fd) && fd >= 3));
assert.equal(new Set([auditFd, controlFd, commandsFd]).size, 3);
const state = health();
state.fixture = { kind: "controlled", workCompleted: false, shutdownCompleted: false, observationInstalled: false };
const handlers = new Map(), binding = { instance_id: "fixture", profile: "fixture", native_id: "fixture", launch_id: randomUUID() };
const bootstrap = { version: 6, binding, aoe_bin: "/unused", app_dir: root };
let runtime, polls = 0;
const pi = { registerFlag() {}, getFlag: () => JSON.stringify(bootstrap), events: { on() {} },
  on(event, handler) { const group = handlers.get(event) ?? []; group.push(handler); handlers.set(event, group); } };
const context = { hasUI: false, sessionManager: { getSessionId: () => binding.native_id } };
const emit = async event => { for (const handler of handlers.get(event) ?? []) await handler({}, context); };
registerObserved(pi, () => ({
  attach(r) { assert.equal(r.refresh, undefined, "attachment precedes refresh installation"); runtime = r; },
  async poll() {}, failed() {}, status() {},
  shutdown(r) { return r.bridge.request({ op: "runtime", command: { kind: "stop" } }); },
}), { probes: { open: privateProbe, refresh: scenario === "observation-missing" ? () => false : observeRefresh,
  bridge: observeBridge, delegation: observeDelegation }, schedule: () => () => {},
  bridge: () => ({ async request(packet) {
    if (packet.op === "poll" && ++polls === 2 && ["pre-failure", "positive-malformed"].includes(scenario)) throw Error("PRIVATE controlled poll failure");
    return { policy: null };
  }, async close() {} }),
});
await emit("session_start");
if (scenario !== "observation-missing") {
  assert.equal(runtime.refresh.__aoeObserved, true, "install after completed session_start");
  assert.equal(state.probes.snapshot().events, 0, "initial lexical refresh is not claimed as an observed call");
}
writeSync(controlFd, "ready\n");
const input = new Socket({ fd: commandsFd, readable: true, writable: false });
const lines = createInterface({ input });
for await (const command of lines) {
  if (command === "release") {
    if (scenario === "early-exit") process.exit(0);
    await runtime.refresh("before_tool");
    state.fixture.workCompleted = true;
    writeFileSync(join(root, "teardown"), "", { mode: 0o600, flag: "wx" });
    await emit("session_shutdown");
    state.fixture.shutdownCompleted = true;
    writeSync(controlFd, scenario === "endpoint-not-reached" ? "not_reached\n" : "completed\n");
  } else if (command === "exit") {
    if (["exit-ignored", "valid-audit-hang"].includes(scenario)) {
      process.on("SIGTERM", () => {});
      if (scenario === "valid-audit-hang") { assert.equal(writeAudit(state), true); delete process.env.AOE_FIXTURE_AUDIT_FD; }
      writeSync(controlFd, scenario === "valid-audit-hang" ? "audited_hang\n" : "ignored\n");
      setInterval(() => {}, 1000); continue;
    }
    lines.close(); input.destroy();
    if (scenario === "trace-cap") for (let n = 0; n < 2000; n++) state.trace.emit("opened", { teardown: true });
    if (scenario === "probe-cap") for (let n = 0; n < 2000; n++) {
      const call = state.probes.allocateCall(); state.probes.emit("cleanup_stop", "entry", { call, teardown: true });
      state.probes.emit("cleanup_stop", "returned", { call, teardown: true });
    }
    if (scenario === "writer-close-failure") {
      const close = state.trace.close; state.trace.close = () => { close(); throw Error("PRIVATE close failure"); };
    }
    const report = finalizeAudit(state);
    const trace = join(root, "trace.jsonl"), probes = join(root, "probes.jsonl");
    if (scenario === "trace-missing") unlinkSync(trace);
    if (scenario === "probe-missing") unlinkSync(probes);
    if (scenario === "trace-drop") writeFileSync(trace, readFileSync(trace, "utf8").split("\n").slice(0, -2).join("\n") + "\n");
    if (scenario === "trace-bytes") report.trace.bytes++;
    if (scenario === "probe-count") report.probes.events++;
    if (["trace-malformed", "positive-malformed"].includes(scenario)) appendFileSync(trace, "PRIVATE malformed\n");
    if (scenario === "probe-malformed") appendFileSync(probes, "PRIVATE malformed\n");
    if (scenario === "trace-truncated") {
      const last = JSON.parse(readFileSync(trace, "utf8").trim().split("\n").at(-1));
      appendFileSync(trace, JSON.stringify({ ...last, seq: last.seq + 1, event: "truncated", bridge: 0 }) + "\n");
    }
    if (scenario === "trace-noncanonical") writeFileSync(trace, readFileSync(trace, "utf8").replaceAll('\"seq\":', '\"seq\": '));
    if (scenario === "trace-mode") chmodSync(trace, 0o644);
    if (scenario === "root-mode") chmodSync(root, 0o755);
    if (scenario === "control-partial") writeSync(controlFd, "PRIVATE trailing");
    if (scenario === "unexpected-file") writeFileSync(join(root, "unexpected-private-output"), "PRIVATE", { mode: 0o600 });
    if (scenario === "trace-oversize") appendFileSync(trace, "x".repeat(262145));
    if (scenario === "trace-invalid-utf8") appendFileSync(trace, Buffer.from([255, 10]));
    if (scenario === "trace-too-many-rows") {
      const template = JSON.parse(readFileSync(trace, "utf8").trim().split("\n").at(-1));
      for (let n = 0; n < 1025; n++) appendFileSync(trace, JSON.stringify({ ...template, seq: template.seq + n + 1, event: "opened", bridge: 0 }) + "\n");
    }
    if (scenario === "trace-symlink") { unlinkSync(trace); const other = join(root, "other"); writeFileSync(other, "PRIVATE", { mode: 0o600 }); symlinkSync(other, trace); }
    if (scenario === "exit-signal") process.kill(process.pid, "SIGTERM");
    if (scenario === "exit-nonzero") process.exit(2);
    const manual = scenario.startsWith("audit-") || ["trace-bytes", "probe-count"].includes(scenario);
    if (manual) {
      delete process.env.AOE_FIXTURE_AUDIT_FD;
      if (scenario === "audit-fd1") process.env.AOE_FIXTURE_AUDIT_FD = "1";
      if (scenario === "audit-invalid-fd") process.env.AOE_FIXTURE_AUDIT_FD = `${auditFd}.0`;
      if (scenario === "audit-wrong-nonce") report.nonce = "0".repeat(32);
      if (scenario === "audit-wrong-version") report.version = 1;
      if (scenario === "audit-extra") report.raw = "PRIVATE";
      const line = JSON.stringify(report) + "\n";
      if (scenario === "audit-stdout") process.stdout.write(line);
      else if (["audit-missing", "audit-fd1", "audit-invalid-fd"].includes(scenario)) { /* No audit emission. */ }
      else if (scenario === "audit-duplicate") writeSync(auditFd, line + line);
      else if (scenario === "audit-partial") writeSync(auditFd, line.slice(0, -1));
      else if (scenario === "audit-oversized") writeSync(auditFd, "x".repeat(5000));
      else if (scenario === "audit-split") {
        const half = Math.floor(line.length / 2); writeSync(auditFd, line.slice(0, half));
        await new Promise(resolve => setImmediate(resolve)); writeSync(auditFd, line.slice(half));
      } else writeSync(auditFd, line);
    }
    process.exit(0);
  } else throw Error("unexpected controlled command");
}
