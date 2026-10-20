import { validEvent } from "./aoe-afk-observer-fixture.mjs";
import { validProbe } from "./aoe-afk-observer-probes.mjs";

const FAILURE_EVENTS = ["rejected", "failed", "failed_returned", "close_failed", "attach_failed", "failed_threw"];
const PROBE_FAILURES = ["rejected", "reply_rejected", "threw"];
const MAX_EVENTS = 1024, MAX_BYTES = 256 * 1024;
const enumValue = (allowed, value, fallback = "unknown") => allowed.includes(value) ? value : fallback;
const count = (rows, event) => rows.filter(row => row.event === event).length;
const same = (a, b, keys) => keys.every(key => a[key] === b[key]);
const saw = (rows, predicate) => rows.some(predicate);

function dataRecord(value) {
  try {
    return value !== null && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype
      && Reflect.ownKeys(value).every(key => {
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        return typeof key === "string" && descriptor.enumerable && Object.hasOwn(descriptor, "value");
      });
  } catch { return false; }
}

function endpointStatus(value) {
  if (value === true || value === "qualified") return "qualified";
  if (value === false || value === "unqualified") return "unqualified";
  return value === "not_reached" ? value : "unknown";
}

function exitStatus(completion) {
  const waitpid = dataRecord(completion) && completion.waitpid;
  if (!waitpid) return "missing_waitpid";
  if (!dataRecord(waitpid)) return "unknown";
  if (waitpid.code === 0 && waitpid.signal === null) return "waitpid_clean";
  if ((Number.isInteger(waitpid.code) && waitpid.code > 0) || typeof waitpid.signal === "string") return "waitpid_nonzero";
  return "unknown";
}

function validatedRows(input, validate) {
  if (input === undefined || input === null) return { rows: [], status: "absent" };
  if (!Array.isArray(input)) return { rows: [], status: "invalid" };
  const rows = [];
  let valid = input.length <= MAX_EVENTS;
  for (let i = 0; i < Math.min(input.length, MAX_EVENTS); i++) {
    const descriptor = Object.getOwnPropertyDescriptor(input, i);
    const row = descriptor && Object.hasOwn(descriptor, "value") ? descriptor.value : null;
    try {
      if (!dataRecord(row) || !validate(row)) { valid = false; continue; }
      rows.push(row);
      if (row.seq !== i + 1) valid = false;
    } catch { valid = false; }
  }
  return { rows, status: valid ? "complete" : "invalid" };
}

function writerValid(health, rows) {
  return dataRecord(health) && Object.keys(health).sort().join(",") === "bytes,events,invalid,truncated"
    && health.invalid === false && health.truncated === false && health.events === rows.length
    && Number.isInteger(health.bytes) && health.bytes >= 0 && health.bytes <= MAX_BYTES
    && health.bytes === rows.reduce((bytes, row) => bytes + Buffer.byteLength(JSON.stringify(row) + "\n"), 0);
}

function parsedTrace(input, health) {
  const parsed = validatedRows(input, validEvent);
  if (parsed.status !== "complete") return { ...parsed, closes: new Map() };
  const { rows } = parsed, pending = new Map(), last = new Map(), closes = new Map();
  let valid = true, incomplete = !writerValid(health, rows), attach, attached = false, teardown = false, failed;
  for (const row of rows) {
    if (row.event === "teardown") {
      if (teardown || row.teardown !== true) valid = false;
      teardown = true;
    }
    if (row.teardown !== teardown) valid = false;
    if (row.event === "truncated") incomplete = true;
    if (["attach", "attached", "attach_failed", "failed", "failed_returned", "failed_threw", "close", "closed", "close_failed"].includes(row.event)
      && (row.request !== 0 || row.op !== "other" || row.kind !== "none")) valid = false;
    switch (row.event) {
      case "attach":
        if (attach || teardown) valid = false;
        attach = row;
        break;
      case "attached":
        if (!attach || attached || teardown || row.bridge !== attach.bridge) valid = false;
        attached = true;
        break;
      case "attach_failed": incomplete = true; break;
      case "failed":
        if (!attached || failed || row.bridge !== attach.bridge) valid = false;
        failed = row;
        break;
      case "failed_returned": case "failed_threw":
        if (!failed || row.bridge !== failed.bridge) valid = false;
        failed = undefined;
        break;
      case "request": {
        if (!attached || row.bridge <= 0 || closes.has(row.bridge) || row.request !== (last.get(row.bridge) ?? 0) + 1) valid = false;
        last.set(row.bridge, row.request);
        const key = `${row.bridge}:${row.request}`;
        if (pending.has(key)) valid = false;
        pending.set(key, row);
        break;
      }
      case "resolved": case "rejected": {
        const key = `${row.bridge}:${row.request}`, request = pending.get(key);
        if (!request || !same(request, row, ["op", "kind"]) || closes.get(row.bridge)?.end || (request && request.seq >= row.seq)) valid = false;
        pending.delete(key);
        break;
      }
      case "close":
        if (!attached || row.bridge <= 0 || closes.has(row.bridge)) valid = false;
        closes.set(row.bridge, { start: row });
        break;
      case "closed": case "close_failed": {
        const close = closes.get(row.bridge);
        if (!close || close.end || close.start.seq >= row.seq) valid = false;
        if (close) close.end = row;
        break;
      }
    }
  }
  if (!attach || !attached || !teardown || failed || pending.size || closes.size === 0) incomplete = true;
  for (const bridge of last.keys()) if (!closes.get(bridge)?.end) incomplete = true;
  for (const close of closes.values()) if (!close.end) incomplete = true;
  return { rows, closes, status: !valid ? "invalid" : incomplete ? "incomplete" : "complete" };
}

function parsedProbes(input, health, traceRows) {
  const parsed = validatedRows(input, validProbe);
  if (parsed.status !== "complete") return parsed;
  const { rows } = parsed, calls = new Map(), dispatches = new Set();
  let valid = true, incomplete = !writerValid(health, rows), teardown = false, lastCall = 0;
  for (const row of rows) {
    if (teardown && !row.teardown) valid = false;
    teardown ||= row.teardown;
    const opening = ["start", "join", "entry", "dispatch"].includes(row.phase);
    if (row.parent >= row.call) valid = false;
    if (opening) {
      if (row.call <= lastCall || calls.has(row.call)) valid = false;
      lastCall = row.call;
      if (row.phase === "dispatch") {
        const identity = `${row.bridge}:${row.request}`;
        if (row.bridge <= 0 || row.request <= 0 || row.kind !== "stop" || dispatches.has(identity)) valid = false;
        dispatches.add(identity);
        if (row.parent) {
          const parent = calls.get(row.parent);
          if (!parent || parent.end || parent.start.phase !== "entry" || parent.start.caller !== row.caller) valid = false;
        } else if (!["unknown", "other"].includes(row.caller)) valid = false;
      } else if (row.bridge !== 0 || row.request !== 0 || row.kind !== "none") valid = false;
      if (row.phase === "join") {
        const parent = calls.get(row.parent);
        if (!parent || parent.start.type !== "refresh" || parent.start.phase !== "start" || parent.end) incomplete = true;
      } else if (row.phase !== "dispatch" && row.parent !== 0) valid = false;
      calls.set(row.call, { start: row });
    } else {
      const call = calls.get(row.call);
      if (!call || call.end || !same(call.start, row, ["parent", "caller", "bridge", "request", "kind", "type"])) { valid = false; continue; }
      const endings = call.start.type === "refresh" ? ["resolved", "rejected", "threw"]
        : call.start.phase === "entry" ? ["returned", "rejected", "threw"] : ["reply_resolved", "reply_rejected", "threw"];
      if (!endings.includes(row.phase)) valid = false;
      call.end = row;
    }
  }
  for (const call of calls.values()) {
    if (!call.end) incomplete = true;
    if (call.start.phase !== "dispatch" || !call.end || call.end.phase === "threw") continue;
    const request = traceRows.find(row => row.event === "request" && row.bridge === call.start.bridge && row.request === call.start.request);
    const outcome = traceRows.find(row => row.event === (call.end.phase === "reply_resolved" ? "resolved" : "rejected")
      && row.bridge === call.start.bridge && row.request === call.start.request);
    if (!request || !outcome || request.op !== "runtime" || request.kind !== "stop"
      || request.teardown !== call.start.teardown || outcome.teardown !== call.end.teardown) incomplete = true;
  }
  return { rows, status: !valid ? "invalid" : incomplete ? "incomplete" : "complete" };
}

function refreshPhase(rows, complete) {
  if (!complete || saw(rows, row => row.type === "refresh" && PROBE_FAILURES.includes(row.phase))) return "unknown";
  if (saw(rows, row => row.type === "refresh" && row.phase === "join")) return "join";
  return saw(rows, row => row.type === "refresh" && row.phase === "resolved") ? "resolved" : "unknown";
}

function cleanupStopPhase(rows, complete) {
  if (!complete) return "unknown";
  for (const phase of ["reply_rejected", "reply_resolved", "dispatch", "entry"]) {
    if (saw(rows, row => row.type === "cleanup_stop" && row.phase === phase)) return phase;
  }
  return "unknown";
}

// Completion comes from an independent audit channel and the supervisor's actual waitpid.
// This pure report checks their consistency; it neither obtains receipts nor replaces that audit.
export function classifyDiagnostic({ endpoint, completion, trace, probes, scope = "bridge" } = {}) {
  const endpoint_status = endpointStatus(endpoint), exit_status = exitStatus(completion);
  const audit = dataRecord(completion) && dataRecord(completion.audit) ? completion.audit : {};
  const parsed = parsedTrace(trace, audit.trace);
  const parsedProbe = parsedProbes(probes, audit.probes, parsed.rows);
  const supportedScope = ["bridge", "observed_calls"].includes(scope);
  const scopedProbes = scope === "observed_calls";
  const probeComplete = parsedProbe.status === "complete";
  const probesReady = probeComplete || (!scopedProbes && parsedProbe.status === "absent");
  const coveredCalls = !scopedProbes || (saw(parsedProbe.rows, row => row.type === "refresh") && saw(parsedProbe.rows, row => row.type === "cleanup_stop"));
  const complete = audit.completed === true && supportedScope && parsed.status === "complete" && probesReady && coveredCalls;
  const traceFailures = parsed.rows.filter(row => FAILURE_EVENTS.includes(row.event));
  const probeFailures = parsedProbe.rows.filter(row => PROBE_FAILURES.includes(row.phase));
  const failures = [...traceFailures, ...probeFailures];
  const preFailures = failures.filter(row => !row.teardown), teardownFailures = failures.filter(row => row.teardown);
  const pre_teardown_events = preFailures.length ? "observed" : complete ? "none_observed" : "unknown";
  const teardown_events = teardownFailures.length ? "observed_without_order" : complete ? "none_observed" : "unknown";

  // Only the trace has a common sequence and bridge identity for rejection/close ordering.
  const orders = new Set();
  if (parsed.status === "complete") {
    for (const row of traceFailures.filter(row => row.event === "rejected")) {
      const close = parsed.closes.get(row.bridge);
      if (close) orders.add(close.start.seq < row.seq ? "close_before_rejection" : "rejection_before_close");
    }
  }
  const close_order = orders.size === 1 ? [...orders][0] : orders.size > 1 ? "unknown"
    : complete && failures.length === 0 ? "not_applicable" : "unknown";
  let orderedTeardown = teardown_events;
  if (teardownFailures.length && parsed.status === "complete" && probeFailures.every(row => !row.teardown)) {
    const positions = new Set();
    for (const row of traceFailures.filter(row => row.teardown)) {
      const close = row.bridge ? parsed.closes.get(row.bridge) : parsed.closes.size === 1 ? [...parsed.closes.values()][0] : undefined;
      if (!close) { positions.add("unknown"); continue; }
      positions.add(row.seq < close.start.seq ? "observed_before_close" : "observed_after_close");
    }
    if (positions.size === 1 && !positions.has("unknown")) orderedTeardown = [...positions][0];
  }
  const ready = complete && exit_status === "waitpid_clean" && endpoint_status === "qualified";
  return {
    scope: enumValue(["bridge", "observed_calls"], scope),
    endpoint_status, trace_status: parsed.status, probe_status: parsedProbe.status, exit_status,
    pre_teardown_events, teardown_events: orderedTeardown, close_order,
    refresh_phase: refreshPhase(parsedProbe.rows, probeComplete),
    cleanup_stop_phase: cleanupStopPhase(parsedProbe.rows, probeComplete),
    cause_claim: "none",
    counts: {
      requests: count(parsed.rows, "request"), resolved: count(parsed.rows, "resolved"), rejected: count(parsed.rows, "rejected"),
      failed: count(parsed.rows, "failed"), failed_returned: count(parsed.rows, "failed_returned"),
      teardown_failures: teardownFailures.length, pre_teardown_failures: preFailures.length, probes: parsedProbe.rows.length,
      probe_failures: probeFailures.length,
    },
    classification: ready && preFailures.length ? "diagnostic_failure_observed"
      : ready && failures.length === 0 ? "diagnostic_clean"
      : audit.completed === true && endpoint_status === "qualified" && parsed.status === "absent" && parsedProbe.status === "absent" && exit_status === "waitpid_clean" ? "clean_endpoint_only"
      : "diagnostic_inconclusive",
  };
}

export function diagnosticFields(value = {}) {
  const defaults = classifyDiagnostic();
  return {
    scope: enumValue(["bridge", "observed_calls"], value.scope ?? defaults.scope),
    endpoint_status: enumValue(["qualified", "unqualified", "not_reached", "unknown"], value.endpoint_status ?? defaults.endpoint_status),
    trace_status: enumValue(["complete", "incomplete", "invalid", "absent"], value.trace_status ?? defaults.trace_status),
    probe_status: enumValue(["complete", "incomplete", "invalid", "absent"], value.probe_status ?? defaults.probe_status),
    exit_status: enumValue(["waitpid_clean", "waitpid_nonzero", "missing_waitpid", "unknown"], value.exit_status ?? defaults.exit_status),
    pre_teardown_events: enumValue(["none_observed", "observed", "unknown"], value.pre_teardown_events ?? defaults.pre_teardown_events),
    teardown_events: enumValue(["none_observed", "observed_before_close", "observed_after_close", "observed_without_order", "unknown"], value.teardown_events ?? defaults.teardown_events),
    close_order: enumValue(["not_applicable", "close_before_rejection", "rejection_before_close", "unknown"], value.close_order ?? defaults.close_order),
    refresh_phase: enumValue(["start", "join", "resolved", "unknown"], value.refresh_phase ?? defaults.refresh_phase),
    cleanup_stop_phase: enumValue(["entry", "dispatch", "reply_resolved", "reply_rejected", "unknown"], value.cleanup_stop_phase ?? defaults.cleanup_stop_phase),
    cause_claim: enumValue(["none", "bounded_fixture_cause", "native_fixture_cause", "original_c_unknown"], value.cause_claim ?? defaults.cause_claim),
    classification: enumValue(["clean_endpoint_only", "diagnostic_clean", "diagnostic_inconclusive", "diagnostic_failure_observed"], value.classification ?? defaults.classification),
  };
}
