import { writeSync } from "node:fs";

export const AUDIT_VERSION = 2;
export const AUDIT_LIMIT = 4096;
const FLAGS = ["teardown", "shutdown", "fault", "cancelled", "opened", "attributed", "nativeDeferred", "waitArmed", "runCompleted"];
const keys = (value, expected) => {
  try {
    return value !== null && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype
      && Reflect.ownKeys(value).every(key => {
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        return typeof key === "string" && descriptor.enumerable && Object.hasOwn(descriptor, "value");
      }) && Object.keys(value).sort().join(",") === [...expected].sort().join(",");
  } catch { return false; }
};
const integer = (value, max) => Number.isInteger(value) && value >= 0 && value <= max;
const writer = value => value === null || (keys(value, ["invalid", "truncated", "events", "bytes"])
  && typeof value.invalid === "boolean" && typeof value.truncated === "boolean"
  && integer(value.events, 1024) && integer(value.bytes, 262144));

export function validAudit(value, nonce) {
  return keys(value, ["version", "nonce", ...FLAGS, "calls", "trace", "probes", "fixture"])
    && value.version === AUDIT_VERSION && typeof nonce === "string" && /^[a-f0-9]{32}$/.test(nonce) && value.nonce === nonce
    && FLAGS.every(key => typeof value[key] === "boolean") && integer(value.calls, 65535)
    && writer(value.trace) && writer(value.probes)
    && (value.fixture === null || (keys(value.fixture, ["kind", "workCompleted", "shutdownCompleted", "observationInstalled"])
      && value.fixture.kind === "controlled"
      && ["workCompleted", "shutdownCompleted", "observationInstalled"].every(key => typeof value.fixture[key] === "boolean")));
}

export function finalizeAudit(state, nonce = process.env.AOE_FIXTURE_AUDIT_NONCE) {
  try { state.trace?.close?.(); } catch { state.trace?.invalidate(); state.fault = true; }
  try { state.probes?.close?.(); } catch { state.probes?.invalidate(); state.fault = true; }
  return { version: AUDIT_VERSION, nonce, ...Object.fromEntries(FLAGS.map(key => [key, state[key]])), calls: state.calls,
    trace: state.trace?.snapshot() ?? null, probes: state.probes?.snapshot() ?? null, fixture: state.fixture ?? null };
}

export function writeAudit(state) {
  const report = finalizeAudit(state);
  const rawFd = process.env.AOE_FIXTURE_AUDIT_FD;
  if (typeof rawFd !== "string" || !/^[1-9][0-9]*$/.test(rawFd)) return false;
  const fd = Number(rawFd);
  if (!Number.isInteger(fd) || fd < 3 || fd > 1024 || !validAudit(report, process.env.AOE_FIXTURE_AUDIT_NONCE)) return false;
  const line = JSON.stringify(report) + "\n";
  if (Buffer.byteLength(line) > AUDIT_LIMIT) return false;
  try { return writeSync(fd, line) === Buffer.byteLength(line); } catch { return false; }
}
