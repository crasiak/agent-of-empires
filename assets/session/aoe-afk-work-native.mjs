import { closeSync, constants, fstatSync, lstatSync, openSync, readSync } from "node:fs";
import { isAbsolute } from "node:path";

export const WORK_PERIOD_MARKER = "aoe-afk-work-period-v1";
const FAILURE = Symbol.for("aoe.afk.work-period.append-failures.v1");
const MAX_RECORDS = 8192, MAX_MARKER = 8192, MAX_APPEND = 16384, MAX_SCOPES = 512;
const check = (condition, message) => { if (!condition) throw Error(message); };
function custody() {
  if (!(FAILURE in globalThis)) Object.defineProperty(globalThis, FAILURE, { value: { scopes: new Set(), overflow: false } });
  const value = globalThis[FAILURE];
  check(value && value.scopes instanceof Set && typeof value.overflow === "boolean" && value.scopes.size <= MAX_SCOPES, "native work-period failure custody unavailable");
  return value;
}
const qualified = stat => stat.isFile() && stat.nlink === 1 && (stat.mode & 0o022) === 0 && (typeof process.getuid !== "function" || stat.uid === process.getuid());

export function createNativeWorkJournal(pi, ctx) {
  const manager = ctx.sessionManager;
  const scope = JSON.stringify([manager.getSessionId(), manager.getSessionFile() ?? null]);
  const available = () => {
    const state = custody();
    check(!state.overflow && !state.scopes.has(scope), "native work-period append unverified; automatic work unavailable");
  };
  return {
    load() {
      available();
      const rows = manager.getEntries().filter(e => e.type === "custom" && e.customType === WORK_PERIOD_MARKER);
      check(rows.length <= MAX_RECORDS && rows.every(e => Buffer.byteLength(JSON.stringify(e.data)) <= MAX_MARKER), "native work-period history capacity");
      return rows.map(e => structuredClone(e.data));
    },
    append(value) {
      let fd;
      try {
        available();
        const file = manager.getSessionFile(), native = manager.getSessionId(), parent = manager.getLeafId();
        check(typeof file === "string" && isAbsolute(file) && value?.version === 2 && value.binding?.native_id === native
          && Buffer.byteLength(JSON.stringify(value)) <= MAX_MARKER, "persistent exact native work-period scope required");
        fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW);
        const before = fstatSync(fd), path = lstatSync(file);
        check(qualified(before) && qualified(path) && !path.isSymbolicLink() && before.dev === path.dev && before.ino === path.ino, "native work-period file identity unavailable");
        const prefix = Buffer.alloc(Math.min(before.size, MAX_APPEND));
        const count = readSync(fd, prefix, 0, prefix.length, 0), end = prefix.subarray(0, count).indexOf(0x0a);
        check(end > 0, "bounded native session header unavailable");
        const headerBytes = prefix.subarray(0, end + 1), headerText = headerBytes.toString("utf8");
        check(Buffer.from(headerText).equals(headerBytes), "native session header is not UTF-8");
        const header = JSON.parse(headerText);
        check(header.type === "session" && header.version === 3 && header.id === native, "physical native session header scope mismatch");
        pi.appendEntry(WORK_PERIOD_MARKER, structuredClone(value));
        const after = fstatSync(fd), current = lstatSync(file), size = after.size - before.size;
        check(manager.getSessionId() === native && manager.getSessionFile() === file && qualified(after) && qualified(current) && !current.isSymbolicLink()
          && after.dev === before.dev && after.ino === before.ino && current.dev === before.dev && current.ino === before.ino
          && current.size === after.size && Number.isSafeInteger(size) && size > 0 && size <= MAX_APPEND, "native work-period append extent unverified");
        const currentHeader = Buffer.alloc(headerBytes.length);
        check(readSync(fd, currentHeader, 0, currentHeader.length, 0) === currentHeader.length && currentHeader.equals(headerBytes), "native header changed during append");
        const bytes = Buffer.alloc(size);
        check(readSync(fd, bytes, 0, size, before.size) === size, "native work-period readback incomplete");
        const text = bytes.toString("utf8");
        check(Buffer.from(text).equals(bytes) && text.endsWith("\n") && text.indexOf("\n") === text.length - 1, "unexpected native work-period append extent");
        const entry = JSON.parse(text), leaf = manager.getLeafId();
        check(entry.type === "custom" && entry.customType === WORK_PERIOD_MARKER && typeof entry.id === "string" && entry.id === leaf && leaf !== parent
          && entry.parentId === parent && JSON.stringify(entry.data) === JSON.stringify(value), "native work-period append identity mismatch");
      } catch (error) {
        try {
          const state = custody();
          if (state.scopes.size >= MAX_SCOPES && !state.scopes.has(scope)) state.overflow = true;
          else state.scopes.add(scope);
        } catch { /* Unavailable custody also refuses every subsequent append. */ }
        throw Error("native work-period append unverified", { cause: error });
      } finally { if (fd !== undefined) closeSync(fd); }
    },
  };
}
