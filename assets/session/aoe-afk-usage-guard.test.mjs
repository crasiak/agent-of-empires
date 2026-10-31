import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createUsageWitness, USAGE_MARKER } from "./aoe-afk-usage-guard.mjs";

const core = process.env.AOE_PI_SOURCE_ROOT;
const sdk = core ? await import(pathToFileURL(join(core, "packages/coding-agent/src/core/session-manager.ts"))) : undefined;
const assistant = { role: "assistant", api: "fixture", provider: "fixture", model: "fixture", timestamp: 1, content: [{ type: "text", text: "ordinary history" }], stopReason: "stop", usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "afk-usage-witness-"));
  const manager = sdk.SessionManager.create(dir, join(dir, "sessions"));
  manager.appendMessage(assistant);
  const binding = { profile: "default", instance_id: "fixture", native_id: manager.getSessionId(), launch_id: randomUUID() };
  const ops = [];
  const r = { bootstrap: { binding }, generation: randomUUID(), recoveredUsageWindows: new Set(), ctx: { sessionManager: manager }, bridge: { async request(packet) { ops.push(packet); throw Error("host unavailable"); } } };
  const view = { binding, generation: r.generation, window: randomUUID(), usage_policy_hash: "a".repeat(64), confirmed: true, state: "pending", expires_at_ms: Date.now() + 60000,
    shared_usage_guard: null, usage: { guard: { tokens: 100, usd: null }, blocked: null, shared_blocked: null, gap: null, unavailable: null, ordinary_recovered: false } };
  const pi = { appendEntry: (type, data) => manager.appendCustomEntry(type, data) };
  return { dir, manager, r, view, pi, ops, witness: createUsageWitness(pi, r), close: () => rmSync(dir, { recursive: true, force: true }) };
}
function restore(f, manager = sdk.SessionManager.open(f.manager.getSessionFile())) {
  const r = { ...f.r, generation: randomUUID(), bootstrap: { binding: { ...f.r.bootstrap.binding, launch_id: randomUUID() } }, recoveredUsageWindows: new Set(), ctx: { sessionManager: manager } };
  const pi = { appendEntry: (type, data) => manager.appendCustomEntry(type, data) };
  return { r, pi, manager, witness: createUsageWitness(pi, r) };
}
const markers = manager => manager.getEntries().filter(e => e.type === "custom" && e.customType === USAGE_MARKER);
if (process.argv.includes("--custody-fault")) {
  const mode = process.argv[process.argv.indexOf("--custody-fault") + 1], f = fixture();
  try {
    const key=Symbol.for("aoe.afk.usage-append-failures.v1");
    if(mode === "preexisting") Object.defineProperty(globalThis,key,{value:null});
    else assert.equal(f.witness.blocked(undefined),false);
    const state=globalThis[key];
    if(mode === "preexisting") assert.equal(f.witness.blocked(undefined),true);
    else if (mode === "malformed") state.scopes.add("not a scope tuple");
    else {
      for(let n=0;n<4096;n++)state.scopes.add(JSON.stringify(["other",String(n),randomUUID()]));
      f.manager._persist=()=>{};assert.throws(()=>f.witness.require(f.view));assert.equal(state.overflow,true);
    }
    assert.equal(f.witness.blocked(undefined),true);
    if(state) state.scopes.clear(); else assert.equal(globalThis[key],null,"malformed existing registry was not replaced");
    const reloaded=await import(`./aoe-afk-usage-guard.mjs?fault=${randomUUID()}`);
    const clean=fixture();try {assert.equal(reloaded.createUsageWitness(clean.pi,clean.r).blocked(undefined),true);}finally{clean.close();}
    console.log(JSON.stringify({mode,stickyRefusal:true}));
  } finally {f.close();}
  process.exit(0);
}
if (process.argv.includes("--read-witness")) {
  const index = process.argv.indexOf("--read-witness"), manager = sdk.SessionManager.open(process.argv[index + 1]);
  const r = {bootstrap:{binding:JSON.parse(process.argv[index + 2])},generation:randomUUID(),recoveredUsageWindows:new Set(),ctx:{sessionManager:manager}};
  const witness = createUsageWitness({},r);
  console.log(JSON.stringify({blocked:witness.blocked(undefined),pending:witness.pending,failure:witness.failure ?? null}));process.exit(0);
}

test("usage witness uses persisted identity and recovers without a host view", { skip: !core }, async () => {
  const f = fixture();
  try {
    assert.equal(f.witness.require(f.view), true);
    assert.equal(f.witness.acknowledge(f.view), true);
    assert.equal(f.witness.blocked(f.view), false);
    const intent = markers(f.manager)[0].data;
    assert.equal(intent.phase, "required"); assert.equal(f.ops.length, 0);
    const resumed = restore(f);
    assert.equal(resumed.witness.blocked(f.view), true, "old positive generation is not reusable");
    assert.equal(resumed.witness.blocked(undefined), true);
    await resumed.witness.recover();
    assert.equal(resumed.witness.blocked(undefined), false);
    assert.deepEqual(f.ops, [{ op: "usage_recover", window: f.view.window }]);
    const release = markers(sdk.SessionManager.open(f.manager.getSessionFile())).at(-1).data;
    assert.deepEqual(release, { ...intent, phase: "released", reason: "ordinary" });
    const again = restore(f);
    assert.equal(again.witness.blocked(undefined), false); assert.ok(again.r.recoveredUsageWindows.has(f.view.window));
  } finally { f.close(); }
});

test("unpersisted SDK memory never qualifies a usage intent", { skip: !core }, () => {
  for (const mode of ["deferred", "throw", "wrong-parent", "wrong-id"]) {
    const f = fixture();
    try {
      const before = readFileSync(f.manager.getSessionFile());
      if (mode === "deferred") f.manager._persist = () => {};
      if (mode === "throw") f.manager._persist = () => { throw Error("injected append failure"); };
      if (mode === "wrong-parent") f.pi.appendEntry = (type, data) => { f.manager.leafId = null; f.manager.appendCustomEntry(type, data); };
      if (mode === "wrong-id") f.pi.appendEntry = (type, data) => {
        f.manager.appendCustomEntry(type, data);
        f.manager.getLeafId = () => "not-the-persisted-entry";
      };
      assert.throws(() => f.witness.require(f.view));
      assert.equal(f.witness.acknowledge(f.view), false);
      assert.equal(f.witness.blocked(f.view), true);
      assert.equal(markers(f.manager).length, 1, "SDK memory can contain the failed append");
      if (["deferred", "throw"].includes(mode)) assert.deepEqual(readFileSync(f.manager.getSessionFile()), before);
      assert.equal(f.ops.length, 0, "no host admission or refund");
    } finally { f.close(); }
  }
});

test("failed usage release cannot be trusted by same-manager extension reload", { skip: !core }, async () => {
  const f = fixture();
  try {
    f.witness.require(f.view);
    const size = statSync(f.manager.getSessionFile()).size;
    f.manager._persist = () => {};
    await assert.rejects(f.witness.recover(), /not persisted/);
    assert.equal(statSync(f.manager.getSessionFile()).size, size);
    assert.equal(markers(f.manager).at(-1).data.phase, "released", "failed release exists only in memory");
    assert.equal(f.witness.blocked(undefined), true);
    const reloaded = restore(f, f.manager);
    const freshModule = await import(`./aoe-afk-usage-guard.mjs?reload=${randomUUID()}`);
    assert.notEqual(freshModule.createUsageWitness,createUsageWitness,"module cache is bypassed");
    reloaded.witness = freshModule.createUsageWitness(reloaded.pi,reloaded.r);
    assert.equal(reloaded.witness.blocked(undefined), true, "reload must not trust an unpersisted release left in SDK memory");
    assert.equal(reloaded.witness.released(f.view),false,"poisoned memory is not released protection");
    assert.equal(reloaded.r.recoveredUsageWindows.size,0,"poisoned memory cannot seed recovered-window suppression");
    const reopened = restore(f);
    assert.equal(reopened.witness.blocked(undefined), true);
    assert.equal(reopened.witness.pending, 1);
    const child = spawnSync(process.execPath,["--import",join(core,"packages/coding-agent/src/experimental/source-resolver.ts"),fileURLToPath(import.meta.url),"--read-witness",f.manager.getSessionFile(),JSON.stringify(f.r.bootstrap.binding)],{encoding:"utf8",timeout:10000});
    assert.equal(child.status,0,child.stderr);
    assert.deepEqual(JSON.parse(child.stdout),{blocked:true,pending:1,failure:null},"fresh process reads original required intent, not poisoned memory");
    assert.equal(f.ops.length, 0);
  } finally { f.close(); }
});

test("failure custody dominates already-created release and admission shortcuts", {skip:!core}, async()=>{
  const f=fixture();let unrelated;
  try {
    f.witness.require(f.view);await f.witness.recover();assert.equal(f.witness.released(f.view),true);
    const other=restore(f,f.manager),b={...f.view,binding:other.r.bootstrap.binding,generation:other.r.generation,window:randomUUID()};
    f.manager._persist=()=>{};assert.throws(()=>other.witness.require(b));
    assert.equal(f.witness.released(f.view),false);assert.equal(f.witness.acknowledge(f.view),false);assert.equal(f.witness.blocked(undefined),true);await assert.rejects(f.witness.recover());
    unrelated=fixture();assert.equal(unrelated.witness.blocked(undefined),false,"unrelated NoGuard scope remains ordinary");
  }finally{unrelated?.close();f.close();}
});

test("failure custody overflow and malformed state are sticky across module reload", {skip:!core}, ()=>{
  for(const mode of ["preexisting","malformed","overflow"]){
    const child=spawnSync(process.execPath,["--import",join(core,"packages/coding-agent/src/experimental/source-resolver.ts"),fileURLToPath(import.meta.url),"--custody-fault",mode],{encoding:"utf8",timeout:20000});
    assert.equal(child.status,0,child.stderr);assert.deepEqual(JSON.parse(child.stdout),{mode,stickyRefusal:true});
  }
});

test("guarded handoff keeps A until B protection and confirmation", { skip: !core }, () => {
  for (const mode of ["success", "B-append-failed", "A-release-failed", "NoGuard", "missing-policy"]) {
    const f = fixture();
    try {
      f.witness.require(f.view);
      const b = { ...f.view, window: randomUUID(), confirmed: false };
      if (["NoGuard", "missing-policy"].includes(mode)) {
        b.usage_policy_hash = null; b.usage = { guard: null }; if (mode === "missing-policy") delete b.shared_usage_guard;
      }
      const persist = f.manager._persist.bind(f.manager);
      if (mode === "B-append-failed") f.manager._persist = () => { throw Error("B failure"); };
      if (mode === "B-append-failed") {
        assert.throws(() => f.witness.require(b));
      } else {
        f.witness.require(b);
        assert.throws(() => f.witness.handoff(b), /not current/);
        assert.equal(markers(f.manager).filter(e => e.data.phase === "released").length, 0);
        b.confirmed = true;
        if (mode === "A-release-failed") f.manager._persist = () => { throw Error("A release failure"); };
        if (["A-release-failed", "missing-policy"].includes(mode)) assert.throws(() => f.witness.handoff(b));
        else f.witness.handoff(b);
      }
      f.manager._persist = persist;
      const reopened = restore(f);
      assert.equal(reopened.witness.pending, mode === "A-release-failed" ? 2 : mode === "NoGuard" ? 0 : 1);
      if (mode !== "NoGuard") assert.equal(reopened.witness.blocked(undefined), true);
      assert.equal(f.ops.length, 0);
    } finally { f.close(); }
  }
});

test("clean-end release requires current terminal proof and restores conservatively", { skip: !core }, async () => {
  for (const mode of ["complete", "complete-ticket", "missing", "denied", "shared-unknown", "shared-local-complete", "expired", "late", "pending-negative", "no-terminal", "incomplete-source", "stale-head"]) {
    const f = fixture();
    try {
      if(mode.startsWith("shared-")) f.view.shared_usage_guard={tokens:null,usd:{limit_nano:1}};
      if(mode === "shared-local-complete") f.view.usage.guard={tokens:null,usd:{limit_nano:100}};
      f.witness.require(f.view);
      const c = { count: f.manager.getEntries().length, hash: "b".repeat(64) };
      const ticket = {version:1,isCurrent:()=>true};
      f.r.usageCompletion = {ticket,proved:true};
      const ended = { ...f.view, state: "ended", terminal_reason: "completed", usage: { ...f.view.usage, cutoff: c, observed_through: c, source_cursor:c,source_complete:true, measurement: { tokens_complete: true, valuation_complete: false, gaps: [] } } };
      if (mode === "missing") delete ended.usage.measurement;
      if (mode === "denied") ended.usage.shared_blocked = "stale";
      if(mode === "shared-local-complete") ended.usage.measurement.valuation_complete=true;
      if (mode === "expired") ended.terminal_reason = "expired";
      if (mode === "late") ended.usage.gap = "late_work";
      if (mode === "pending-negative") ended.usage.negative_evidence_pending = true;
      if (mode === "no-terminal") f.r.usageCompletion.proved = false;
      if (mode === "incomplete-source") ended.usage.source_complete = false;
      if (mode === "stale-head") ended.usage.source_cursor = {...c,count:c.count+1};
      f.witness.cleanEnd(ended,{ticket,head:c});
      assert.equal(f.witness.pending, mode.startsWith("complete") ? 0 : 1, mode);
      if (mode.startsWith("complete")) {
        assert.equal(restore(f).witness.pending, 1, "historical completed release is not new SDK proof");
        if(mode === "complete-ticket") ticket.isCurrent=()=>false;
        else f.manager.appendMessage({...assistant,stopReason:"aborted"});
        assert.equal(f.witness.released(ended), false, "late evidence cannot retain a completion exemption");
        assert.equal(f.witness.pending, 1);
        await f.witness.recover();
        assert.equal(restore(f).witness.blocked(undefined), false, "qualified ordinary recovery survives reopen");
      }
    } finally { f.close(); }
  }
});

test("replayed completed protection requires a qualified handoff", {skip:!core}, () => {
  for (const mode of ["guarded", "NoGuard", "missing-policy", "failed-release"]) {
    const f=fixture();
    try {
      f.witness.require(f.view);
      const c={count:f.manager.getEntries().length,hash:"b".repeat(64)},ticket={version:1,isCurrent:()=>true};
      f.r.usageCompletion={ticket,proved:true};
      const ended={...f.view,state:"ended",terminal_reason:"completed",usage:{...f.view.usage,cutoff:c,observed_through:c,source_cursor:c,source_complete:true,measurement:{tokens_complete:true,gaps:[]}}};
      assert.equal(f.witness.cleanEnd(ended,{ticket,head:c}),true);
      const resumed=restore(f),b={...f.view,binding:resumed.r.bootstrap.binding,generation:resumed.r.generation,window:randomUUID(),confirmed:false};
      assert.equal(resumed.witness.pending,1);
      if (["NoGuard","missing-policy"].includes(mode)) {b.usage_policy_hash=null;b.usage={guard:null};if(mode==="missing-policy")delete b.shared_usage_guard;}
      resumed.witness.require(b);
      assert.throws(()=>resumed.witness.handoff(b),/not current/);
      b.confirmed=true;
      if(mode==="failed-release")resumed.manager._persist=()=>{throw Error("supersession failure");};
      if(["failed-release","missing-policy"].includes(mode))assert.throws(()=>resumed.witness.handoff(b));
      else resumed.witness.handoff(b);
      const reopened=restore(f);
      assert.equal(reopened.witness.pending,mode==="NoGuard"?0:mode==="failed-release"?2:1,mode);
      assert.equal(reopened.witness.blocked(undefined),mode!=="NoGuard",mode);
    } finally {f.close();}
  }
});

test("malformed history vetoes while unrelated NoGuard history does not", { skip: !core }, () => {
  for (const mode of ["foreign", "oversized", "unmatched", "changed", "records", "identities", "NoGuard"]) {
    const f = fixture();
    try {
      f.witness.require(f.view); const intent = markers(f.manager)[0];
      let entries = [intent];
      if (mode === "NoGuard") entries = Array.from({ length: 8200 }, (_, i) => ({ type: "custom", id: String(i), customType: "unrelated", data: { body: "not copied into witness state" } }));
      if (mode === "foreign") entries = [{ ...intent, data: { ...intent.data, binding: { ...intent.data.binding, native_id: randomUUID() } } }];
      if (mode === "oversized") entries = [{ ...intent, data: { ...intent.data, junk: "x".repeat(4096) } }];
      if (mode === "unmatched") entries = [{ ...intent, data: { ...intent.data, phase: "released", reason: "ordinary" } }];
      if (mode === "changed") entries.push({ ...intent, data: { ...intent.data, policy_hash: "b".repeat(64), phase: "released", reason: "ordinary" } });
      if (["records", "identities"].includes(mode)) {
        entries = [];
        for (let i = 0; i < 4096; i++) {
          const m = { ...intent.data, window: randomUUID() };
          entries.push({ ...intent, data: m });
          if (mode === "records") entries.push({ ...intent, data: { ...m, phase: "released", reason: "ordinary" } });
        }
        entries.push({ ...intent, data: { ...intent.data, window: randomUUID() } });
      }
      f.manager.getEntries = () => entries;
      const w = createUsageWitness(f.pi, { ...f.r, recoveredUsageWindows: new Set() });
      assert.equal(w.blocked(undefined), mode !== "NoGuard", mode);
      assert.equal(!!w.failure, mode !== "NoGuard", mode);
    } finally { f.close(); }
  }
});
