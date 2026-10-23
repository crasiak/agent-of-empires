// Publish the native ID and transcript path without requiring a materialized file,
// report context resets, prompts, and turns to AoE's usage log, and write AoE's
// hook status file so a turn's state does not depend on reading the screen.
import { spawn } from "node:child_process";
import { lstatSync, mkdirSync, writeFileSync, renameSync, unlinkSync } from "node:fs";
import { dirname, join } from "node:path";

const HOOK_INSTANCE_ID = /^[0-9a-zA-Z_-]+$/;
// Tools that block on the user; their start is AoE's `waiting`.
const WAITING_TOOLS = new Set(["ask_user"]);
const ATTENTION_FALLBACK = "Waiting for your answer";
const ATTENTION_MAX_CHARS = 200;

/** The question an `ask_user` call shows, as one line of at most 200 code points. */
export function attentionSummary(args) {
  try {
    const question = typeof args?.question === "string" ? args.question : args?.questions?.[0]?.question;
    if (typeof question !== "string") return ATTENTION_FALLBACK;
    const line = question.replace(/[\p{Cc}\u2028\u2029]/gu, " ").trim();
    const capped = Array.from(line).slice(0, ATTENTION_MAX_CHARS).join("").trimEnd();
    return capped || ATTENTION_FALLBACK;
  } catch {
    return ATTENTION_FALLBACK;
  }
}

// The same gates as AoE's shell hooks and `dir_guard`: a real directory, owned by
// the euid, mode exactly 0700. Anything else gets no write.
function safeDir(path, euid) {
  const st = lstatSync(path);
  return st.isDirectory() && st.uid === euid && (st.mode & 0o7777) === 0o700;
}

function ensureDir(path) {
  try {
    mkdirSync(path, { mode: 0o700 });
  } catch (e) {
    if (e?.code !== "EEXIST") throw e;
  }
}

/**
 * Write `<base>/<instanceId>/status` (and `attention` while waiting) on Pi's
 * turn boundaries. Every failure leaves the files as they were.
 */
export function registerHookStatus(pi, { base, instanceId }) {
  if (typeof base !== "string" || !base || typeof instanceId !== "string" || !HOOK_INSTANCE_ID.test(instanceId)) return;
  if (typeof process.geteuid !== "function") return;
  const euid = process.geteuid();
  const dir = join(base, instanceId);
  let counter = 0;
  const ready = () => {
    ensureDir(base);
    if (!safeDir(base, euid)) return false;
    ensureDir(dir);
    return safeDir(dir, euid);
  };
  const write = (name, value) => {
    let tmp;
    try {
      if (!ready()) return;
      tmp = join(dir, `.${name}.tmp.${process.pid}.${counter++}`);
      writeFileSync(tmp, value, { flag: "wx", mode: 0o600 });
      renameSync(tmp, join(dir, name));
      tmp = undefined;
    } catch {
      if (tmp) {
        try {
          unlinkSync(tmp);
        } catch {}
      }
    }
  };
  const remove = name => {
    try {
      if (safeDir(base, euid) && safeDir(dir, euid)) unlinkSync(join(dir, name));
    } catch {}
  };
  const waiting = new Set();
  const guard = fn => event => {
    try {
      fn(event);
    } catch {
      // never block the agent
    }
  };
  const settle = () => {
    waiting.clear();
    write("status", "idle");
    remove("attention");
  };

  pi.on("agent_start", guard(() => {
    waiting.clear();
    write("status", "running");
  }));
  pi.on("tool_execution_start", guard(event => {
    if (!WAITING_TOOLS.has(event?.toolName)) {
      // Refresh the write's age, which AoE bounds, unless a question is open.
      if (waiting.size === 0) write("status", "running");
      return;
    }
    waiting.add(event.toolCallId);
    // Attention first: a reader that sees `waiting` finds its summary.
    write("attention", attentionSummary(event.args));
    write("status", "waiting");
  }));
  pi.on("tool_execution_end", guard(event => {
    if (!waiting.delete(event?.toolCallId) || waiting.size > 0) return;
    write("status", "running");
    remove("attention");
  }));
  // Only agent_settled means idle: queued follow-ups can start a new run
  // after agent_end, and a brief idle there would set AoE's unread marker.
  pi.on("agent_settled", guard(settle));
}

export default function (pi) {
  const idTarget = process.env.AOE_SESSION_ID_FILE;
  const usageBin = process.env.AOE_USAGE_BIN;
  const source = process.env.AOE_SESSION_SOURCE;
  if (source !== undefined && !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(source)) return;
  const suffix = source ? "." + source : "";
  const sessionId = (ctx) => {
    try {
      return ctx?.sessionManager?.getSessionId?.() || undefined;
    } catch {
      return undefined;
    }
  };
  // Fire and forget: the agent never waits on the usage log.
  const report = (payload) => {
    if (!usageBin || !process.env.AOE_INSTANCE_ID) return;
    try {
      const child = spawn(usageBin, ["__usage-event", "--agent", "pi"], {
        stdio: ["pipe", "ignore", "ignore"],
        detached: true,
      });
      child.on("error", () => {});
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
      child.unref();
    } catch {
      // never block the agent
    }
  };
  const writeAtomic = (target, value) => {
    let tmp;
    try {
      mkdirSync(dirname(target), { recursive: true });
      // Rename so a reader never sees a half-written value.
      tmp = join(dirname(target), `.${target.split("/").pop()}.${process.pid}.tmp`);
      writeFileSync(tmp, `${value}\n`, { mode: 0o600 });
      renameSync(tmp, target);
      tmp = undefined;
    } catch {
      if (tmp) {
        try {
          unlinkSync(tmp);
        } catch {}
      }
    }
  };

  pi.on("session_start", async (event, ctx) => {
    report({ hook_event_name: "session_start", reason: event?.reason, session_id: sessionId(ctx) });
    if (!idTarget) return;
    try {

      const id = ctx?.sessionManager?.getSessionId?.();
      if (!id) return;
      const file = ctx?.sessionManager?.getSessionFile?.();
      writeAtomic(idTarget + suffix, id);
      if (file) writeAtomic(join(dirname(idTarget), "session_path" + suffix), file);
    } catch {
      // never block the agent
    }
  });
  pi.on("session_shutdown", async (event, ctx) => {
    report({ hook_event_name: "session_shutdown", reason: event?.reason, session_id: sessionId(ctx) });
  });
  pi.on("session_compact", async (event, ctx) => {
    report({ hook_event_name: "session_compact", reason: event?.reason, session_id: sessionId(ctx) });
  });
  pi.on("input", async (event, ctx) => {
    report({ hook_event_name: "input", source: event?.source, session_id: sessionId(ctx) });
  });
  pi.on("agent_settled", async (_event, ctx) => {
    report({ hook_event_name: "agent_settled", session_id: sessionId(ctx) });
  });
  try {
    registerHookStatus(pi, { base: `/tmp/aoe-hooks-${process.geteuid()}`, instanceId: process.env.AOE_INSTANCE_ID });
  } catch {
    // never block the agent
  }
}
