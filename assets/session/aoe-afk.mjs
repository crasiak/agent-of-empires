import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { performance } from "node:perf_hooks";

const LIMIT = 16 * 1024;
const REMINDER = "AoE AFK control-only: the human is away. This notice grants no new decision or action authority. Preserve all existing approval and waiting requirements. Do not answer, dismiss, or bypass human questions because of this notice. Autonomous allowance: zero.";
const same = (a, b) => a && b && ["instance_id", "profile", "native_id", "launch_id"].every(k => typeof a[k] === "string" && a[k] === b[k]);
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);

export function spawnBridge(bootstrap, generation) {
  const child = spawn(bootstrap.aoe_bin, ["__afk-bridge", JSON.stringify(bootstrap), generation], { stdio: ["pipe", "pipe", "ignore"] });
  let pending, buffer = "", closed = false;
  const fail = () => {
    if (closed) return;
    closed = true;
    pending?.reject(new Error("AFK bridge unavailable"));
    pending = undefined;
    child.stdin.destroy();
    child.kill();
  };
  child.on("error", fail);
  child.on("exit", fail);
  child.stdin.on("error", fail);
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", data => {
    buffer += data;
    if (Buffer.byteLength(buffer) > LIMIT) return fail();
    const end = buffer.indexOf("\n");
    if (end < 0) return;
    if (!pending || end !== buffer.length - 1) return fail();
    const waiter = pending;
    pending = undefined;
    const line = buffer.slice(0, end);
    buffer = "";
    try {
      const result = JSON.parse(line);
      if (result.ok !== true) waiter.reject(new Error("AFK bridge refused request"));
      else waiter.resolve(result.value);
    } catch { waiter.reject(new Error("Invalid AFK bridge reply")); fail(); }
  });
  return {
    request(packet) {
      if (closed || pending) return Promise.reject(new Error("AFK bridge unavailable or busy"));
      const line = JSON.stringify(packet) + "\n";
      if (Buffer.byteLength(line) > LIMIT) return Promise.reject(new Error("AFK packet too large"));
      return new Promise((resolve, reject) => {
        const timer = setTimeout(fail, 1500);
        pending = { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } };
        child.stdin.write(line);
      });
    },
    async close() {
      if (child.exitCode !== null || child.signalCode !== null) { fail(); return; }
      const exited = new Promise(resolve => { child.once("exit", resolve); child.once("error", resolve); });
      fail();
      const timer = setTimeout(() => child.kill("SIGKILL"), 500);
      await exited;
      clearTimeout(timer);
    },
  };
}

export function registerAfk(pi, options = {}) {
  const wall = options.wall ?? Date.now;
  const mono = options.mono ?? (() => performance.now());
  const bridgeFactory = options.bridge ?? spawnBridge;
  const schedule = options.schedule ?? (fn => { const timer = setInterval(fn, 500); timer.unref(); return () => clearInterval(timer); });
  pi.registerFlag("aoe-afk-binding", { type: "string", description: "AoE direct-launch AFK control binding (internal)" });
  let runtime;
  const shutdown = async () => {
    const old = runtime;
    runtime = undefined;
    if (!old) return;
    old.stop?.();
    old.state = "unavailable";
    if (old.ctx.hasUI) old.ctx.ui.setStatus("aoe-afk", undefined);
    await old.bridge.close();
  };
  pi.on("session_shutdown", shutdown);
  pi.on("session_start", async (_event, ctx) => {
    await shutdown();
    const raw = pi.getFlag("aoe-afk-binding");
    if (typeof raw !== "string" || Buffer.byteLength(raw) > LIMIT) return;
    let bootstrap;
    try { bootstrap = JSON.parse(raw); } catch { return; }
    if (bootstrap.version !== 1 || !bootstrap.binding || typeof bootstrap.aoe_bin !== "string"
      || !bootstrap.aoe_bin.startsWith("/") || typeof bootstrap.app_dir !== "string" || !bootstrap.app_dir.startsWith("/")
      || !same(bootstrap.binding, bootstrap.binding) || !uuid(bootstrap.binding.launch_id)
      || ctx.sessionManager.getSessionId() !== bootstrap.binding.native_id) return;
    const generation = randomUUID();
    const r = { ctx, bootstrap, generation, bridge: bridgeFactory(bootstrap, generation), state: "off", revision: 0, deadline: 0, lastAck: "", policy: null };
    runtime = r;
    const refresh = () => {
      if (r.refreshing) return r.refreshing;
      r.refreshing = (async () => {
        try {
          if (runtime !== r || ctx.sessionManager.getSessionId() !== bootstrap.binding.native_id) {
            r.state = "invalidated"; return;
          }
          const { policy: p, probe } = await r.bridge.request({ op: "poll" });
          if (runtime !== r) return;
          r.policy = p;
          if (!p) r.state = "off";
          else if (p.version !== 1 || p.mode !== "control-only" || p.allowance !== 0 || !Number.isSafeInteger(p.revision) || p.revision < r.revision
            || p.binding.instance_id !== bootstrap.binding.instance_id || p.binding.profile !== bootstrap.binding.profile) r.state = "invalidated";
          else if (!p.enabled) { r.revision = p.revision; r.state = "off"; r.deadline = 0; }
          else if (!same(p.binding, bootstrap.binding) || p.generation !== generation || !uuid(p.window)
            || !Number.isSafeInteger(p.expires_at_ms) || !Number.isSafeInteger(p.duration_ms)
            || p.duration_ms < 60000 || p.duration_ms > 86400000
            || !Number.isSafeInteger(p.issued_at_ms) || p.expires_at_ms !== p.issued_at_ms + p.duration_ms) r.state = "invalidated";
          else {
            if (p.revision > r.revision) {
              r.deadline = mono() + Math.max(0, Math.min(p.duration_ms, p.expires_at_ms - wall()));
              r.revision = p.revision;
            }
            if (wall() >= p.expires_at_ms || mono() >= r.deadline) r.expiredRevision = p.revision;
            r.state = r.expiredRevision === p.revision ? "expired" : "control-only";
          }
          if (probe && probe.version === 1 && same(probe.binding, bootstrap.binding) && probe.revision === (p?.revision ?? 0)) {
            const ack = { version: 1, binding: bootstrap.binding, challenge: probe.challenge, generation, revision: p?.revision ?? 0,
              window: p?.window ?? null, expires_at_ms: p?.expires_at_ms ?? null, mode: "control-only", allowance: 0, state: r.state };
            const signature = JSON.stringify(ack);
            if (signature !== r.lastAck) {
              await r.bridge.request({ op: "ack", ack });
              if (runtime === r) r.lastAck = signature;
            }
          }
        } catch { r.state = "unavailable"; }
        finally {
          if (runtime === r && ctx.hasUI) ctx.ui.setStatus("aoe-afk", r.state === "off" ? undefined : `AFK: ${r.state} (no autonomy)`);
          r.refreshing = undefined;
        }
      })();
      return r.refreshing;
    };
    r.refresh = refresh;
    r.stop = schedule(() => void refresh());
    await refresh();
  });
  pi.on("context", async (event, ctx) => {
    const r = runtime;
    const messages = event.messages.filter(m => m.customType !== "aoe-afk-control");
    if (!r) return { messages };
    // An idle poll may have read its snapshot before this request boundary.
    if (r.refreshing) await r.refreshing;
    if (runtime !== r) return { messages };
    await r.refresh();
    if (runtime !== r || r.state !== "control-only" || mono() >= r.deadline || ctx.sessionManager.getSessionId() !== r.bootstrap.binding.native_id) return { messages };
    return { messages: [...messages, { role: "custom", customType: "aoe-afk-control", content: REMINDER, display: false, timestamp: wall() }] };
  });
}

export default registerAfk;
