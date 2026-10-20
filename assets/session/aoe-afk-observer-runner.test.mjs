import test from "node:test";
import assert from "node:assert/strict";
import { runControlledFixture } from "./aoe-afk-observer-runner.mjs";
import { finalizeAudit, validAudit } from "./aoe-afk-observer-audit.mjs";

const privateSafe = result => {
  assert.ok(!JSON.stringify(result).includes("PRIVATE"));
  assert.equal(result.provenance, "reconstructed_controlled_fixture");
  assert.equal(result.endpoint_scope, "controlled_release_not_native");
  assert.equal(result.process.cleanup_complete, true);
};

test("actual producer, private writers, audit FD and owned waitpid qualify only a controlled endpoint", { timeout: 30000 }, async t => {
  for (const scenario of ["normal", "audit-split", "holder"]) await t.test(scenario, async () => {
    const result = await runControlledFixture(scenario);
    privateSafe(result);
    assert.equal(result.diagnostic.classification, "diagnostic_clean");
    assert.equal(result.diagnostic.exit_status, "waitpid_clean");
    assert.equal(result.diagnostic.refresh_phase, "resolved");
    assert.equal(result.diagnostic.cleanup_stop_phase, "reply_resolved");
    assert.equal(result.diagnostic.counts.requests, 3);
    assert.equal(result.diagnostic.counts.probes, 6);
    assert.deepEqual(result.completion.waitpid, { code: 0, signal: null });
    assert.equal(result.completion.audit.completed, true);
    assert.equal(result.process.main_reaped, true);
    assert.equal(result.process.audit_eof, true);
    assert.equal(result.process.control_eof, true);
    assert.equal(result.retained_root, null);
    assert.ok(Object.values(result.ingestion).every(Boolean));
    assert.equal(result.process.held_fd_observed, scenario === "holder");
    if (scenario === "holder") {
      assert.ok(result.events.indexOf("holder_ready") < result.events.indexOf("main_exit"));
      assert.ok(result.events.indexOf("main_exit") < result.events.indexOf("audit_withheld"));
      assert.ok(result.events.indexOf("audit_withheld") < result.events.indexOf("holder_release"));
      assert.ok(result.events.indexOf("holder_release") < result.events.indexOf("audit_eof"));
    }
  });
});

test("valid positive observations survive physical ingestion failure without clean classification", { timeout: 30000 }, async () => {
  const failure = await runControlledFixture("pre-failure");
  privateSafe(failure);
  assert.equal(failure.diagnostic.classification, "diagnostic_failure_observed");
  assert.equal(failure.diagnostic.pre_teardown_events, "observed");
  const partial = await runControlledFixture("positive-malformed");
  privateSafe(partial);
  assert.equal(partial.diagnostic.classification, "diagnostic_inconclusive");
  assert.equal(partial.diagnostic.pre_teardown_events, "observed");
  assert.equal(partial.diagnostic.cause_claim, "none");
  assert.equal(partial.ingestion.trace_valid, false);
});

test("missing, forged, duplicate, truncated and oversized audit channels never fall back to stdout", { timeout: 60000 }, async t => {
  for (const scenario of ["audit-missing", "audit-stdout", "audit-fd1", "audit-invalid-fd", "audit-wrong-nonce", "audit-wrong-version", "audit-extra", "audit-duplicate", "audit-partial", "audit-oversized"]) {
    await t.test(scenario, async () => {
      const result = await runControlledFixture(scenario);
      privateSafe(result);
      assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
      assert.equal(result.process.main_reaped, true);
      assert.equal(result.ingestion.audit_valid, false);
      assert.equal(result.completion.audit.completed, false);
    });
  }
});

test("physical file and writer faults cannot qualify a canonical-looking parsed prefix", { timeout: 60000 }, async t => {
  for (const scenario of ["trace-missing", "trace-drop", "trace-bytes", "trace-malformed", "trace-truncated", "trace-noncanonical", "trace-mode", "trace-symlink", "trace-fifo", "trace-cap", "probe-cap", "writer-close-failure", "probe-missing", "probe-malformed", "probe-count"]) {
    await t.test(scenario, async () => {
      const result = await runControlledFixture(scenario);
      privateSafe(result);
      assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
      assert.equal(result.process.main_reaped, true);
    });
  }
});

test("exit and endpoint are separate; completion-looking data cannot replace termination", { timeout: 30000 }, async t => {
  for (const scenario of ["exit-nonzero", "exit-signal", "early-exit", "endpoint-not-reached", "observation-missing", "exit-ignored", "holder-ignored"]) {
    await t.test(scenario, async () => {
      const result = await runControlledFixture(scenario);
      privateSafe(result);
      assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
      assert.equal(result.process.main_reaped, true);
      if (scenario.startsWith("exit-")) assert.equal(result.diagnostic.exit_status, "waitpid_nonzero");
      if (scenario === "exit-ignored") {
        assert.ok(result.events.includes("ignored"), "deadline follows the observed ignored-exit precondition");
        assert.equal(result.process.deadline, true);
        assert.equal(result.process.escalated, true);
        assert.deepEqual(result.completion.waitpid, { code: null, signal: "SIGKILL" });
      }
      if (scenario === "holder-ignored") {
        assert.equal(result.process.held_fd_observed, true);
        assert.ok(result.events.includes("holder_ignored"), "holder acknowledged ignoring release before the deadline");
        assert.equal(result.process.deadline, true);
        assert.equal(result.diagnostic.exit_status, "waitpid_clean", "the main child exited but an owned holder prevented completion");
        assert.equal(result.completion.audit.completed, false);
      }
      if (scenario === "endpoint-not-reached") {
        assert.equal(result.completion.audit.completed, true);
        assert.equal(result.diagnostic.endpoint_status, "not_reached");
      }
    });
  }
  const spawnFailure = await runControlledFixture("spawn-failure");
  privateSafe(spawnFailure);
  assert.equal(spawnFailure.diagnostic.classification, "diagnostic_inconclusive");
  assert.equal(spawnFailure.diagnostic.exit_status, "missing_waitpid");
  assert.deepEqual(spawnFailure.failures, ["spawn_failed"]);
});

test("all control, output, privacy and holder validation failures veto qualification", { timeout: 30000 }, async t => {
  for (const [scenario, failure] of [["control-partial", "control_invalid"], ["unexpected-file", "unexpected_entry"], ["root-mode", "trace_invalid"], ["holder-no-fd", "holder_invalid"]]) {
    await t.test(scenario, async () => {
      const result = await runControlledFixture(scenario);
      privateSafe(result);
      assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
      assert.equal(result.completion.audit.completed, false);
      assert.ok(result.failures.includes(failure));
      if (scenario === "control-partial") assert.equal(result.ingestion.control_valid, false);
      if (scenario === "holder-no-fd") assert.equal(result.process.held_fd_observed, false);
    });
  }
});

test("external physical caps, row limits and encoding faults are independently observed", { timeout: 30000 }, async t => {
  for (const scenario of ["trace-oversize", "trace-too-many-rows", "trace-invalid-utf8"]) await t.test(scenario, async () => {
    const result = await runControlledFixture(scenario);
    privateSafe(result);
    assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
    assert.equal(result.completion.audit.completed, false);
    assert.equal(result.ingestion.trace_valid, false);
    assert.ok(result.failures.includes("trace_invalid"));
  });
});

test("a valid receipt before a main-process hang cannot replace reaping", { timeout: 30000 }, async () => {
  const result = await runControlledFixture("valid-audit-hang");
  privateSafe(result);
  assert.ok(result.events.includes("audited_hang"), "valid audit was emitted before ignoring exit");
  assert.equal(result.ingestion.audit_valid, true);
  assert.equal(result.process.deadline, true);
  assert.equal(result.process.escalated, true);
  assert.equal(result.completion.audit.completed, false);
  assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
  assert.deepEqual(result.completion.waitpid, { code: null, signal: "SIGKILL" });
});

test("allocation and holder-spawn failures clean every successfully acquired resource", { timeout: 30000 }, async () => {
  for (const [scenario, failure] of [["setup-pipe-failure", "setup_failed"], ["holder-spawn-failure", "holder_spawn_failed"]]) {
    const result = await runControlledFixture(scenario);
    privateSafe(result);
    assert.equal(result.diagnostic.classification, "diagnostic_inconclusive");
    assert.deepEqual(result.failures, [failure]);
    assert.equal(result.process.root_removed, true);
    assert.equal(result.process.open_fds_left, 0);
    assert.equal(result.process.main_reaped, scenario === "holder-spawn-failure");
  }
});

test("actual audit contract closes writers before snapshots and rejects hidden/private metadata", () => {
  const nonce = "a".repeat(32), order = [];
  const state = { teardown: true, shutdown: true, fault: false, cancelled: false, opened: false, attributed: false,
    nativeDeferred: false, waitArmed: false, runCompleted: false, calls: 0, fixture: null };
  for (const name of ["trace", "probes"]) {
    let invalid = false;
    state[name] = { close() { order.push(`${name}:close`); invalid = true; }, snapshot() { order.push(`${name}:snapshot`); return { invalid, truncated: false, events: 0, bytes: 0 }; } };
  }
  const report = finalizeAudit(state, nonce);
  assert.deepEqual(order, ["trace:close", "probes:close", "trace:snapshot", "probes:snapshot"]);
  assert.equal(report.trace.invalid, true);
  assert.equal(report.probes.invalid, true);
  assert.equal(validAudit(report, nonce), true);
  for (const mode of ["extra", "accessor", "serializer", "wrong-type", "wrong-nonce"]) {
    const row = structuredClone(report); let touched = 0;
    if (mode === "extra") row.private = "PRIVATE";
    if (mode === "accessor") Object.defineProperty(row, "fault", { get() { touched++; throw Error("PRIVATE"); }, enumerable: true });
    if (mode === "serializer") Object.defineProperty(row, "toJSON", { value() { touched++; return "PRIVATE"; } });
    if (mode === "wrong-type") row.trace.events = "0";
    if (mode === "wrong-nonce") row.nonce = "b".repeat(32);
    assert.equal(validAudit(row, nonce), false);
    assert.equal(touched, 0);
  }
});

test("runner has no caller-provided executable, endpoint, trace or completion API", async () => {
  await assert.rejects(runControlledFixture({ command: "native", endpoint: true, completion: { waitpid: { code: 0, signal: null } } }), /Unknown controlled fixture/);
  await assert.rejects(runControlledFixture("native"), /Unknown controlled fixture/);
});
