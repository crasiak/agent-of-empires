import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, appendFileSync, readFileSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { randomUUID } from "node:crypto";
import { createNativeWorkJournal, WORK_PERIOD_MARKER } from "./aoe-afk-work-native.mjs";

function fixture(kind = "normal") {
  const root = mkdtempSync(join(tmpdir(), "aoe-work-journal-")), file = join(root, "native.jsonl"), native = randomUUID();
  writeFileSync(file, JSON.stringify({ type: "session", version: 3, id: native }) + "\n", { mode: 0o600 });
  const rows = []; let leaf = "root";
  const manager = { getSessionId: () => native, getSessionFile: () => file, getLeafId: () => leaf, getEntries: () => rows };
  const pi = { appendEntry(customType, data) {
    const row = { type: "custom", customType, data: structuredClone(data), id: String(rows.length + 1), parentId: leaf }; rows.push(row); leaf = row.id;
    if (kind === "throw") throw Error("injected native append failure");
    if (kind === "memory-only") return;
    if (kind === "replace") { renameSync(file, file + ".old"); writeFileSync(file, "", { mode: 0o600 }); }
    appendFileSync(file, JSON.stringify(row) + "\n");
    if (kind === "extra") appendFileSync(file, JSON.stringify({ type: "custom", id: "foreign" }) + "\n");
  } };
  return { root, file, native, rows, manager, pi, journal: createNativeWorkJournal(pi, { sessionManager: manager }), data: { version: 2, binding: { native_id: native }, status: "pending" }, close: () => rmSync(root, { recursive: true, force: true }) };
}

test("native work-period admission requires the exact physically appended entry", () => {
  const f = fixture();
  try {
    f.journal.append(f.data);
    const entry = JSON.parse(readFileSync(f.file, "utf8").trim().split("\n").at(-1));
    assert.equal(entry.customType, WORK_PERIOD_MARKER); assert.deepEqual(entry.data, f.data);
    assert.deepEqual(f.journal.load(), [f.data]);
  } finally { f.close(); }
});

test("memory-only, failed, replaced and interleaved native writes keep failure custody across reload", () => {
  for (const kind of ["memory-only", "throw", "replace", "extra"]) {
    const f = fixture(kind);
    try {
      assert.throws(() => f.journal.append(f.data), undefined, kind);
      assert.equal(f.rows.length, 1, "SDK memory is not accepted as disk proof");
      assert.throws(() => createNativeWorkJournal(f.pi, { sessionManager: { ...f.manager } }).load(), /unverified|unavailable/, kind);
    } finally { f.close(); }
  }
});

test("foreign or malformed physical headers cannot admit native work", () => {
  for (const header of [{ type: "session", version: 3, id: randomUUID() }, { type: "session", version: 99 }, { type: "message", id: "not-a-session" }]) {
    const f = fixture();
    try {
      writeFileSync(f.file, JSON.stringify(header) + "\n");
      assert.throws(() => f.journal.append(f.data), /unverified/);
      assert.equal(f.rows.length, 0, "the SDK append was not called in a foreign scope");
      assert.throws(() => createNativeWorkJournal(f.pi, { sessionManager: f.manager }).load(), /unverified|unavailable/);
    } finally { f.close(); }
  }
});

test("unrelated custom entries stay outside the work-period journal; malformed work records cannot be dropped", () => {
  const f = fixture();
  try {
    f.rows.push({ type: "custom", customType: "other", data: { unrelated: true } });
    assert.deepEqual(f.journal.load(), []);
    f.rows.push({ type: "custom", customType: WORK_PERIOD_MARKER, data: { version: 3 } });
    assert.deepEqual(f.journal.load(), [{ version: 3 }], "the state validator must see and refuse unknown versions");
  } finally { f.close(); }
});
