// Publish the native ID and transcript path without requiring a materialized file,
// report context resets, prompts, and turns to AoE's usage log, and write AoE's
// hook status file so a turn's state does not depend on reading the screen.
import { spawn } from "node:child_process";
import { closeSync, constants, fchmodSync, fstatSync, lstatSync, mkdirSync, openSync, renameSync, unlinkSync, writeFileSync, writeSync } from "node:fs";
import { dirname, join } from "node:path";

// AoE's validate_instance_id: 1 to 64 bytes of [0-9a-zA-Z_-].
const HOOK_INSTANCE_ID = /^[0-9a-zA-Z_-]{1,64}$/;
// Tools that block on the user; their start is AoE's `waiting`.
const DEFAULT_WAITING_TOOLS = new Set(["ask_user"]);
// AoE's own check on `[agents.pi] waiting_tools`.
const WAITING_TOOL_NAME = /^[A-Za-z0-9_.:-]{1,64}$/;
const WAITING_TOOLS_MAX = 32;
// The only base AoE may name: the sandbox's Pi config mount.
export const SANDBOX_HOOK_BASE = "/root/.pi/aoe-session";
const ATTENTION_FALLBACK = "Waiting for your answer";
const PROMPT_FALLBACK = { confirm: "Waiting for confirmation", select: "Waiting for a choice" };
const PROMPT_FALLBACK_DEFAULT = "Waiting for your input";
const ATTENTION_MAX_CHARS = 200;
// AoE bounds a running write at 900 s; long turns refresh it this often.
const RUNNING_REFRESH_MS = 60_000;

/** `text` as one line of at most 200 code points, or "" when nothing is left. */
function summaryLine(text) {
  if (typeof text !== "string") return "";
  // Format characters (bidi overrides, zero-width) could disguise the text.
  const line = text.replace(/\p{Cf}/gu, "").replace(/[\p{Cc}\u2028\u2029]/gu, " ").trim();
  return Array.from(line).slice(0, ATTENTION_MAX_CHARS).join("").trimEnd();
}

/** The question an `ask_user` call shows, as one line of at most 200 code points. */
export function attentionSummary(args) {
  try {
    const question = typeof args?.question === "string" ? args.question : args?.questions?.[0]?.question;
    return summaryLine(question) || ATTENTION_FALLBACK;
  } catch {
    return ATTENTION_FALLBACK;
  }
}

/** A blocking UI prompt's title, or a fixed text for its kind. */
function promptSummary(kind, title) {
  return summaryLine(title) || PROMPT_FALLBACK[kind] || PROMPT_FALLBACK_DEFAULT;
}

/**
 * `ask_user` plus the valid names in AoE's comma-separated `AOE_PI_WAITING_TOOLS`,
 * at most 32 in all: the same rule as AoE's `pi_waiting_tools`.
 */
export function waitingTools(raw) {
  const tools = new Set(DEFAULT_WAITING_TOOLS);
  if (typeof raw !== "string") return tools;
  for (const name of raw.split(",")) {
    if (tools.size >= WAITING_TOOLS_MAX) break;
    if (WAITING_TOOL_NAME.test(name)) tools.add(name);
  }
  return tools;
}

/** The status base: the sandbox mount when AoE names it exactly, else the per-user host base. */
export function hookBase(env, euid) {
  return env?.AOE_HOOK_BASE === SANDBOX_HOOK_BASE
    ? { base: SANDBOX_HOOK_BASE, sandbox: true }
    : { base: `/tmp/aoe-hooks-${euid}`, sandbox: false };
}

// The same gates as AoE's shell hooks and `dir_guard`: a real directory, owned by
// the euid, mode exactly 0700. Anything else gets no write.
function safeDir(path, euid) {
  const st = lstatSync(path);
  return st.isDirectory() && st.uid === euid && (st.mode & 0o7777) === 0o700;
}

// The sandbox mount, like AoE's sandbox shell hooks, has no uid check: the
// container uid is unpredictable and the host validated the mount source.
function safeSandboxBase(path) {
  const st = lstatSync(path);
  return st.isDirectory() && (st.mode & 0o022) === 0;
}

// An instance dir an older publisher created 0755 is tightened, but only when it is ours.
function safeSandboxDir(path, euid, repair) {
  const st = lstatSync(path);
  if (!st.isDirectory()) return false;
  const mode = st.mode & 0o7777;
  if (mode === 0o700) return true;
  if (!repair || mode !== 0o755 || st.uid !== euid) return false;
  const fd = openSync(path, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try {
    const opened = fstatSync(fd);
    if (opened.ino !== st.ino || opened.dev !== st.dev || opened.uid !== euid) return false;
    fchmodSync(fd, 0o700);
    return (fstatSync(fd).mode & 0o7777) === 0o700;
  } finally {
    closeSync(fd);
  }
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
 * turn boundaries, tool waits and blocking UI prompts. Every failure leaves
 * the files as they were.
 */
export function registerHookStatus(pi, {
  base,
  instanceId,
  now = Date.now,
  sandbox = false,
  euid = process.geteuid?.(),
  waitingTools: waitTools = DEFAULT_WAITING_TOOLS,
}) {
  if (typeof base !== "string" || !base || typeof instanceId !== "string" || !HOOK_INSTANCE_ID.test(instanceId)) return;
  if (typeof euid !== "number") return;
  const dir = join(base, instanceId);
  let counter = 0;
  const baseOk = () => (sandbox ? safeSandboxBase(base) : safeDir(base, euid));
  const dirOk = repair => (sandbox ? safeSandboxDir(dir, euid, repair) : safeDir(dir, euid));
  const ready = () => {
    ensureDir(base);
    if (!baseOk()) return false;
    ensureDir(dir);
    return dirOk(true);
  };
  const write = (name, value) => {
    let tmp;
    let fd;
    try {
      if (!ready()) return false;
      const candidate = join(dir, `.${name}.tmp.${process.pid}.${counter++}`);
      fd = openSync(candidate, "wx", 0o600);
      // Only a file this call created is ours to clean up.
      tmp = candidate;
      writeSync(fd, value);
      closeSync(fd);
      fd = undefined;
      renameSync(tmp, join(dir, name));
      return true;
    } catch {
      if (fd !== undefined) {
        try {
          closeSync(fd);
        } catch {}
      }
      if (tmp) {
        try {
          unlinkSync(tmp);
        } catch {}
      }
      return false;
    }
  };
  const remove = name => {
    try {
      if (baseOk() && dirOk(false)) unlinkSync(join(dir, name));
    } catch {}
  };
  // Open questions by tool call, in the order asked.
  const waiting = new Map();
  // Open blocking UI prompts, the latest last.
  const prompts = [];
  let inTurn = false;
  let state;
  let shown;
  let runningAt = -Infinity;
  const open = () => waiting.size + prompts.length > 0;
  const guard = fn => event => {
    try {
      fn(event);
    } catch {
      // never block the agent
    }
  };
  const running = () => {
    state = "running";
    if (write("status", "running")) runningAt = now();
  };
  const clearAttention = () => {
    shown = undefined;
    remove("attention");
  };
  const settle = () => {
    waiting.clear();
    prompts.length = 0;
    inTurn = false;
    state = "idle";
    write("status", "idle");
    clearAttention();
  };
  // An open question outranks a prompt; the latest of either is shown.
  const showWait = () => {
    const prompt = prompts.at(-1);
    const summary = waiting.size > 0 ? [...waiting.values()].at(-1) : promptSummary(prompt?.kind, prompt?.title);
    // Rewriting the same text would move the `since` AoE reads off its mtime.
    if (summary !== shown && write("attention", summary)) shown = summary;
    if (state !== "waiting") {
      state = "waiting";
      // After the attention: a reader that sees `waiting` finds its summary.
      write("status", "waiting");
    }
  };
  // The last wait closed: back to the turn, or idle when none runs.
  const release = () => {
    clearAttention();
    if (inTurn) {
      running();
    } else {
      state = "idle";
      write("status", "idle");
    }
  };
  // While a turn runs, any activity keeps the write inside AoE's freshness bound.
  const activity = guard(() => {
    if (state === "running" && !open() && now() - runningAt >= RUNNING_REFRESH_MS) running();
  });

  // A new or restarted session is idle: this also clears a status a crashed
  // process left, which AoE would otherwise keep showing.
  pi.on("session_start", guard(settle));
  // A cleanly quit Pi leaves no status, so AoE does not read its dead pane as an error.
  pi.on("session_shutdown", guard(() => {
    waiting.clear();
    prompts.length = 0;
    inTurn = false;
    state = undefined;
    remove("status");
    clearAttention();
  }));
  pi.on("agent_start", guard(() => {
    waiting.clear();
    prompts.length = 0;
    inTurn = true;
    clearAttention();
    running();
  }));
  pi.on("tool_execution_start", guard(event => {
    if (!waitTools.has(event?.toolName)) {
      // Refresh the write's age, which AoE bounds, unless a wait is open.
      if (!open()) running();
      return;
    }
    waiting.set(event.toolCallId, attentionSummary(event.args));
    showWait();
  }));
  pi.on("tool_execution_end", guard(event => {
    if (!waiting.delete(event?.toolCallId)) return;
    if (open()) showWait();
    else release();
  }));
  // Pi reports only the outermost of nested prompts, so starts and ends pair up.
  pi.on("ui_prompt_start", guard(event => {
    prompts.push({ kind: event?.kind, title: event?.title });
    showWait();
  }));
  pi.on("ui_prompt_end", guard(() => {
    if (prompts.length === 0) return;
    prompts.pop();
    if (open()) showWait();
    else release();
  }));
  for (const name of ["turn_start", "message_update", "message_end", "tool_execution_update"]) pi.on(name, activity);
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
      mkdirSync(dirname(target), { recursive: true, mode: 0o700 });
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
    registerHookStatus(pi, {
      ...hookBase(process.env, process.geteuid()),
      instanceId: process.env.AOE_INSTANCE_ID,
      waitingTools: waitingTools(process.env.AOE_PI_WAITING_TOOLS),
    });
  } catch {
    // never block the agent
  }
}
