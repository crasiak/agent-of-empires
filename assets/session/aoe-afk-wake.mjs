import { closeSync, constants, openSync, readSync } from "node:fs";

const TYPE = "aoe-automatic-wake-v1";
const DELIVERY = "custom-message-v1";
const MAX_MS = 30000;
const uuid = value => typeof value === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const same = (a, b) => a && b && ["profile", "instance_id", "native_id", "launch_id"].every(k => a[k] === b[k]);
function readUptime(path) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const bytes = Buffer.alloc(129), n = readSync(fd, bytes, 0, bytes.length, 0);
    if (!n || n > 128) throw Error("clock read unavailable");
    return bytes.subarray(0, n).toString("utf8");
  } finally { closeSync(fd); }
}
export function receiverClock(spec, dependencies = {}) {
  const node = dependencies.node ?? process.version, uv = dependencies.uv ?? process.versions.uv;
  const hrtime = dependencies.hrtime ?? (() => process.hrtime.bigint()), read = dependencies.read ?? readUptime;
  return () => {
    try {
      if (spec?.kind === "hrtime" && spec.node === node && spec.uv === uv) {
        const tick = hrtime();
        return typeof tick === "bigint" && tick >= 0n ? { tick, uncertainty: 0n } : undefined;
      }
      if (spec?.kind === "uptime_file" && typeof spec.path === "string" && spec.path.startsWith("/") && spec.path.length <= 256
        && !/[\u0000-\u001f\u007f]/u.test(spec.path) && Number.isSafeInteger(spec.resolution_ns) && spec.resolution_ns > 0 && spec.resolution_ns <= 1000000000) {
        const text = read(spec.path);
        if (typeof text !== "string" || Buffer.byteLength(text) > 128) return;
        const match = /^(\d{1,12})\.(\d{1,9})(?:\s|$)/u.exec(text);
        if (!match || 10n ** BigInt(9 - match[2].length) > BigInt(spec.resolution_ns)) return;
        return { tick: BigInt(match[1]) * 1000000000n + BigInt(match[2].padEnd(9, "0")), uncertainty: BigInt(spec.resolution_ns) };
      }
    } catch { /* Unknown clock equivalence disables only automated wake. */ }
  };
}

export function automaticWake(pi, eligible, dependencies = {}) {
  return {
    attach(r) {
      const state = r.automaticWake = { closed: false, clock: receiverClock(r.bootstrap.wake_clock, dependencies) };
      try {
        state.stop = r.ctx.ui.onTerminalInput(() => { state.closed = true; });
        if (typeof state.stop !== "function") state.closed = true;
      } catch { state.closed = true; }
    },
    close(r) {
      const state = r?.automaticWake;
      if (!state) return;
      state.closed = true;
      try { state.stop?.(); } catch { /* Wake stays disabled if observation cleanup fails. */ }
      state.stop = undefined;
    },
    activity(r) { if (r?.automaticWake) r.automaticWake.closed = true; },
    observation(r) {
      const state = r.automaticWake, sample = state.clock();
      const ready = !!sample && !state.closed && eligible(r, false);
      if (!ready) state.closed = true;
      return { sample, request: { version: 1, delivery: DELIVERY, ready } };
    },
    receive(r, reply, observation) {
      if (!reply) return;
      const state = r.automaticWake, closed = state.closed;
      if (reply.version === 1 && (reply.state === "empty" || reply.state === "unmatched")) return;
      state.closed = true;
      if (reply.version === 1 && reply.state === "attempted" && reply.text === undefined) {
        state.notice ??= "Automatic restart wake already attempted; no replay"; return;
      }
      if (reply.version !== 1 || reply.state !== "attempted" || typeof reply.text !== "string") {
        state.notice ??= "Automatic restart wake unavailable or suppressed; no retry";
        return;
      }
      const now = state.clock(), start = observation?.sample;
      const remaining = reply.remaining_ms;
      const valid = !closed && eligible(r, true) && same(reply.binding, r.bootstrap.binding)
        && r.ctx.sessionManager.getSessionId() === r.bootstrap.binding.native_id
        && reply.generation === r.generation && uuid(reply.id) && uuid(reply.incarnation)
        && reply.text.length > 0 && Buffer.byteLength(reply.text) <= 4096 && !/[\u0000-\u0008\u000b-\u001f\u007f]/u.test(reply.text)
        && Number.isSafeInteger(remaining) && remaining > 0 && remaining <= MAX_MS && now && start
        && now.tick >= start.tick && now.tick - start.tick + now.uncertainty + start.uncertainty < BigInt(remaining) * 1000000n;
      if (!valid) { state.notice = "Automatic restart wake suppressed after stale or ineligible delivery"; return; }
      state.notice = "Automatic restart wake submitted as automation; execution unconfirmed";
      pi.sendMessage({ customType: TYPE, content: `[AoE automatic restart wake, not human input]\n${reply.text}`, display: true,
        details: { version: 1, origin: "aoe_automation", request_id: reply.id, binding: reply.binding, generation: reply.generation, incarnation: reply.incarnation } }, { triggerTurn: true });
    },
    status(r) { return r.automaticWake?.notice; },
  };
}
