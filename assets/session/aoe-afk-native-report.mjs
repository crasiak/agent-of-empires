import { openSync, fstatSync, readSync, closeSync, constants } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { KIND, sha, digest, qualifyEndpoint, classifyNative } from "./aoe-afk-native-contract.mjs";
import { normalize } from "./aoe-afk-usage.mjs";

export function projectCapture(capture, native, preview) {
  const b = capture.bootstrap.binding;
  const member = capture.snapshot.batches?.[preview.operation_id]?.members?.[b.instance_id];
  const w = capture.snapshot.sessions?.[b.instance_id]?.book?.windows?.find(w => w.id === member?.window);
  const usageWindow = capture.snapshot.usage.windows?.[w?.id];
  const source = capture.snapshot.usage.sources?.[usageWindow?.source];
  const entries = capture.native_rows.slice(1);
  const normalized = entries.map(normalize);
  let head = { count: 0, hash: sha("aoe-afk-usage-v1") };
  const heads = [head];
  for (const entry of normalized) { head = { count: head.count + 1, hash: sha(`${head.hash}:${digest(entry)}`) }; heads.push(head); }
  const lease = capture.lease, hello = capture.hello, clock = capture.clock;
  const equal = (a, b) => digest(a ?? null) === digest(b ?? null);
  const fresh = lease && hello && clock && equal(lease.binding, b) && equal(hello.binding, b)
    && lease.generation === w?.generation && hello.generation === w?.generation
    && lease.incarnation === hello.incarnation && lease.source === usageWindow?.source
    && equal(lease.cursor, source?.cursor) && hello.boot === clock.boot && lease.clock.boot === clock.boot
    && clock.millis >= lease.clock.millis && clock.millis - lease.clock.millis < 5000;
  const policy = usageWindow?.policy;
  const ack = capture.runtime_ack, probe = capture.runtime_probe;
  const runtimeReady = ack?.version === 6 && probe?.version === 6 && ack.sdk === "0.87.1"
    && ack.coverage === "stock-sdk-main-loop-reservations" && ack.usage_coverage === "native-entries-terminal-v2"
    && equal(ack.binding, b) && equal(probe.binding, b) && ack.generation === capture.ready?.generation
    && typeof probe.challenge === "string" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(probe.challenge)
    && ack.challenge === probe.challenge && ack.window === w?.id && probe.window === w?.id
    && ack.grant_hash === member?.grant_hash && probe.grant_hash === member?.grant_hash
    && ack.budgeted === false && probe.budgeted === false
    && ack.usage_policy_hash === digest([policy, null]) && probe.usage_policy_hash === ack.usage_policy_hash;
  const appliedMember = capture.apply?.members?.find(m => m.session_id === b.instance_id);
  const expected = { binding: b, generation: capture.ready?.generation, window: appliedMember?.window,
    grant_hash: member?.grant_hash, usage_policy_hash: digest([policy, null]),
    operation_id: preview.operation_id, preview_digest: capture.approved_preview_digest, scratch_path: capture.scratch_path };
  const nativeResults = entries.filter(e => e.type === "message" && e.message?.role === "toolResult"
    && e.message.toolCallId === native.opened?.toolCallId);
  const nativeCalls = entries.filter(e => e.type === "message" && e.message?.role === "assistant")
    .flatMap(e => e.message.content).filter(c => c.type === "toolCall");
  const exactCall = nativeCalls.length === 1 && nativeCalls[0].name === "ask_user"
    && nativeCalls[0].id === native.opened?.toolCallId && equal(nativeCalls[0].arguments, native.result?.input);
  const registry = capture.registry;
  const row = registry?.length === 1 ? registry[0] : null;
  const registryValid = row?.id === b.instance_id && row.tool === "pi" && row.command === ""
    && row.agent_session_id === b.native_id && row.active_execution?.launch_id === b.launch_id
    && row.active_execution?.binding?.agent === "pi" && row.project_path === capture.work
    && equal(row.active_execution.binding.stores, [resolve(capture.work, "../sessions")])
    && row.active_execution.binding.cwd === capture.work && row.active_execution.binding.filesystem === "host"
    && capture.ready?.binding && equal(capture.ready.binding, b) && capture.ready.native_id === b.native_id
    && capture.apply?.operation_id === preview.operation_id && capture.apply?.preview_digest === preview.digest
    && capture.apply?.profile === b.profile && appliedMember?.host_confirmed === true && appliedMember.outcome === "confirmed"
    && equal(capture.approval, { kind: KIND, manifest: capture.attempt?.manifest, role: capture.attempt?.role, ordinal: capture.attempt?.ordinal,
      operation_id: preview.operation_id, preview_digest: preview.digest, approve_apply: true });
  const grant = w?.grant;
  const serializedGrant = grant && Object.fromEntries(["version", "task", "scope", "files", "requests", "settlement_nudges", "question_deferrals", "assurance"].map(k => [k, grant[k]]));
  if (serializedGrant) serializedGrant.files = grant.files.map(t => ({ path: t.path, capability: t.capability }));
  return { kind: KIND, expected, preview,
    authority: { capture: "bounded-read-only", snapshot: capture.snapshot, stable: capture.stable,
      registry_valid: registryValid, grant_hash: serializedGrant ? sha(JSON.stringify(serializedGrant)) : null,
      workspace_valid: w?.root === capture.work && equal(w?.root_identity, capture.work_identity),
      pending_absent: capture.pending_absent, scratch_absent: capture.scratch_absent, scratch_path: capture.scratch_path }, native,
    usage: { binding: usageWindow?.binding, generation: usageWindow?.generation, window: w?.id,
      policy, policy_hash: digest([policy, null]), source, native_head: head, native_file: capture.native_file,
      current_source: Object.values(capture.snapshot.usage.sources).filter(s => equal(s.binding, b)).length === 1
        && equal(source?.entries, normalized),
      native_result: capture.native_rows[0]?.id === b.native_id && exactCall && nativeResults.length === 1
        && equal(nativeResults[0].message.details, native.result?.details) && nativeResults[0].message.isError === false,
      ready: !!fresh && runtimeReady && usageWindow?.gap === null && usageWindow?.blocked === null && usageWindow?.closing === false
        && usageWindow.cutoff === null && usageWindow.ordinary_recovered === false
        && Number.isInteger(usageWindow.baseline?.count) && usageWindow.baseline.count >= 0
        && equal(usageWindow.baseline, heads[usageWindow.baseline.count]) && equal(usageWindow.observed_through, head),
      unresolved_absent: capture.unresolved_absent } };
}

function physical(path, limit = 256 * 1024) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const s = fstatSync(fd);
    if (!s.isFile() || s.nlink !== 1 || s.uid !== process.getuid() || (s.mode & 0o077)) throw Error("unsafe receipt file");
    const bytes = Buffer.alloc(Math.min(s.size, limit)), count = readSync(fd, bytes, 0, bytes.length, 0);
    const after = fstatSync(fd);
    return { bytes: bytes.subarray(0, count), valid: s.size <= limit && count === s.size
      && after.size === s.size && after.mtimeMs === s.mtimeMs && after.ino === s.ino };
  } finally { closeSync(fd); }
}
function json(path, limit) {
  const read = physical(path, limit);
  if (!read.valid) throw Error("changed or oversized receipt");
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(read.bytes));
}
export function parsePhysicalRows(bytes, valid = true) {
  const rows = [];
  let begin = 0;
  for (let i = 0; i < bytes.length; i++) {
    if (bytes[i] !== 10) continue;
    if (rows.length >= 1024) { valid = false; break; }
    try {
      const line = new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(begin, i));
      const row = JSON.parse(line);
      if (JSON.stringify(row) !== line) throw Error("noncanonical JSONL");
      rows.push(row);
    } catch { rows.push({ invalid: true }); valid = false; break; }
    begin = i + 1;
  }
  if (!valid || begin !== bytes.length) rows.push({ invalid: true });
  return rows;
}
function rows(path) {
  try { const read = physical(path); return parsePhysicalRows(read.bytes, read.valid); }
  catch (error) { return error.code === "ENOENT" ? undefined : [{ invalid: true }]; }
}
export function report(root, endpointOnly = false) {
  const capture = json(`${root}/capture.json`, 40 * 1024 * 1024);
  const endpoint = projectCapture(capture, json(`${root}/endpoint-native.json`), json(`${root}/preview.json`));
  if (endpointOnly) return qualifyEndpoint(endpoint);
  const proc = json(`${root}/process.json`);
  const terminal = json(`${root}/terminal-cleanup.json`);
  const terminalReady = terminal.cleanup === true && terminal.watchdog === false && terminal.controller_status === 0
    && proc.manifest === capture.attempt?.manifest && proc.role === capture.attempt?.role && proc.ordinal === capture.attempt?.ordinal;
  const result = { ...classifyNative({ endpoint, audit: proc.audit, process: proc, trace: rows(`${root}/trace.jsonl`), probes: rows(`${root}/probes.jsonl`) }), manifest: proc.manifest, terminal };
  if (!terminalReady) result.classification = "native_inconclusive";
  return result;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { process.stdout.write(JSON.stringify(report(process.argv[2], process.argv[3] === "endpoint")) + "\n"); }
  catch (error) { process.stdout.write(JSON.stringify({ kind: KIND, classification: "native_inconclusive", error: String(error) }) + "\n"); process.exitCode = 1; }
}
