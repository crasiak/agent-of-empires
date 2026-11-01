export const ANSWER_TOOL = "aoe_afk_answer";
export const DECIDE_TIMEOUT_MS = 5000;
export const BASH_DEFAULT_TIMEOUT_S = 600;
const FRAME_TEXT_MAX = 4096;
const STOP = reason => ({ decision: "stop", reason, checks: [] });

export function answerParameters(Type) {
  return Type.Object({
    question: Type.String({ minLength: 1, maxLength: 2048 }),
    options: Type.Array(Type.Object({ key: Type.String({ minLength: 1, maxLength: 64 }), label: Type.String({ minLength: 1, maxLength: 256 }) }, { additionalProperties: false }), { minItems: 1, maxItems: 32 }),
    proposal: Type.Union([Type.Null(), Type.String({ minLength: 1, maxLength: 64 })], { description: "Key of the option you would choose, or null." }),
    reason: Type.String({ maxLength: 1024 }),
  }, { additionalProperties: false });
}

const withTimeout = (promise, ms) => {
  let timer;
  return Promise.race([promise, new Promise((_, no) => { timer = setTimeout(() => no(Error("timeout")), ms); })]).finally(() => clearTimeout(timer));
};

// Keep bridge frames bounded; a cut inside a multi-byte character drops the partial character.
const bounded = value => {
  if (typeof value !== "string" || Buffer.byteLength(value) <= FRAME_TEXT_MAX) return value;
  return Buffer.from(value).subarray(0, FRAME_TEXT_MAX).toString().replace(/�$/u, "");
};

// Menus sit at the end of agent text: keep its last bytes, starting on a character boundary.
export const textTail = value => {
  const bytes = Buffer.from(value);
  if (bytes.length <= FRAME_TEXT_MAX) return value;
  let start = bytes.length - FRAME_TEXT_MAX;
  while ((bytes[start] & 0xc0) === 0x80) start++;
  return bytes.subarray(start).toString();
};

// Pi's bash result ends with a status line on a non-zero exit, timeout, abort or signal, after an optional truncation line
// (pi coding-agent core/tools/bash.ts). Exit 0 has no status line: the command finished, so it is not waiting.
const PI_WAIT_STATUS = /(?:^|\n\n)(?:Command exited with code -?[1-9]\d*|Command timed out after \d+ seconds|Command aborted|Command terminated without an exit code)$/;
const PI_TRUNCATION = /(?:^|\n\n)\[Showing (?:lines \d+-\d+ of \d+(?: \([^)\n]* limit\))?|last [^\n]* of line \d+ \(line is [^)\n]*\))\. Full output: [^\n]*\]$/;
// A yes/no marker may sit anywhere in the line (apt: "Continue? [Y/n] Abort."); the other cues end it.
const PROMPT_END = /\((?:y\/N|Y\/n|y\/n)\)|\[(?:y\/N|Y\/n)\]|\(yes\/no[^)]*\)|\?\s*$|Password:\s*$|›\s*$/;
// The last 12 non-empty output lines when Pi's trailer says the command did not finish and its last line asks for input.
export function promptLike(text) {
  const body = String(text ?? "").replace(/\s+$/u, "");
  if (!PI_WAIT_STATUS.test(body)) return null;
  const lines = body.replace(PI_WAIT_STATUS, "").replace(PI_TRUNCATION, "").split("\n").map(l => l.trimEnd()).filter(l => l.trim());
  if (!lines.length || !PROMPT_END.test(lines.at(-1))) return null;
  return lines.slice(-12).join("\n");
}

export function askUserMenu(input) {
  if (!input || typeof input.question !== "string" || input.questions || input.allowMultiple || !Array.isArray(input.options) || !input.options.length) return null;
  return { source: "ask_user", format: "tool.ask_user", kind: "choice", question: input.question, subject: input.context ?? null,
    options: input.options.slice(0, 32).map((o, i) => ({ key: String(i + 1), label: String(o.title ?? "").slice(0, 256) })),
    free_text: input.allowFreeform !== false, raw: JSON.stringify(input).slice(0, 4096) };
}

// Pi's permission gates put the command in the title: `⚠️ Dangerous command:\n\n  <cmd>\n\nAllow?` (examples/extensions/permission-gate.ts).
const DANGER = /dangerous|⚠/i;
const PERMISSION = /^(allow|approve|permit|run|execute)\b|permission|dangerous|⚠/i;
// Pi select/confirm/input dialogs offered through ui_prompt_respond; editor and custom dialogs stay with the human.
// The subject scope checks read is the message, else the title; a dangerous-looking dialog carries a harness warning.
export function dialogMenu(e) {
  const kind = e?.kind, title = String(e?.title ?? "").slice(0, 2048) || "(untitled dialog)", raw = JSON.stringify(e).slice(0, 4096);
  const subject = typeof e?.message === "string" ? e.message.slice(0, 2048) : e?.title ? title : null;
  const warnings = DANGER.test(title) || DANGER.test(subject ?? "") ? ["dialog marked dangerous"] : [];
  if (kind === "select" && Array.isArray(e.options) && e.options.length)
    return { source: "pi_dialog", format: "pi.select", kind: PERMISSION.test(title) ? "permission" : "choice", question: title, subject,
      options: e.options.slice(0, 32).map((label, i) => ({ key: String(i + 1), label: String(label).slice(0, 256) })), warnings, raw };
  if (kind === "confirm")
    return { source: "pi_dialog", format: "pi.confirm", kind: PERMISSION.test(title) ? "permission" : "confirm", question: title, subject,
      options: [{ key: "y", label: "Yes" }, { key: "n", label: "No" }], warnings, raw };
  if (kind === "input") return { source: "pi_dialog", format: "pi.input", kind: "input", question: title, free_text: true, options: [], raw };
  return null;
}

// The value Pi accepts for the dialog, or undefined. A select answer is the original option: labels are cut to 256 characters.
export function dialogValue(e, menu, d) {
  if (e.kind === "select") { const i = menu.options.findIndex(o => o.key === d.choice); return i < 0 ? undefined : e.options[i]; }
  if (e.kind === "confirm") return d.choice === "y";
  return typeof d.text === "string" && Buffer.byteLength(d.text) <= FRAME_TEXT_MAX ? d.text : undefined;
}

export function createAnswerClient(bridge, { timeoutMs = DECIDE_TIMEOUT_MS } = {}) {
  return {
    async decide({ run, menu, text, proposal, timeoutMs: ms = timeoutMs }) {
      try {
        const packet = { op: "decide", run, ...(menu ? { menu: { ...menu, ...("raw" in menu ? { raw: bounded(menu.raw) } : {}) } } : {}),
          ...(text ? { text: { ...text, text: bounded(text.text) } } : {}), ...(proposal ? { proposal } : {}) };
        const reply = await withTimeout(bridge.request(packet), ms);
        if (!reply?.decision || !["answer", "stop"].includes(reply.decision.decision)) throw Error("malformed decision");
        return reply;
      } catch { return { menu: menu ?? null, decision: STOP("engine_unavailable") }; }
    },
    async delivered(period, answer_id, status) {
      if (!answer_id) return;
      try { await withTimeout(bridge.request({ op: "answer_delivered", period, answer_id, delivered: status }), timeoutMs); } catch { /* the log keeps "pending"; the review shows it */ }
    },
    async recognise(source, text) {
      try { return (await withTimeout(bridge.request({ op: "recognise", source, text: bounded(text) }), timeoutMs))?.menu ?? null; } catch { return null; }
    },
    async rulesCheck(run) { return withTimeout(bridge.request({ op: "rules_check", run }), timeoutMs); },
  };
}

export const answeredBy = policy => `afk/${policy}`;

export const runInput = (s, cwd) => ({ period: s.id, item: null, goal: s.task, policy: s.policy, rules: s.rules, cwd,
  requests_used: s.requests_used, request_limit: s.request_limit });

export function splitArgs(s) {
  const out = []; let cur = "", quote = null, any = false;
  for (const ch of s.trim()) {
    if (quote) { if (ch === quote) quote = null; else cur += ch; continue; }
    if (ch === "'" || ch === '"') { quote = ch; any = true; continue; }
    if (/\s/.test(ch)) { if (cur || any) { out.push(cur); cur = ""; any = false; } continue; }
    cur += ch;
  }
  if (cur || any) out.push(cur);
  return out;
}

export function parseOnArgs(tokens) {
  const out = { policy: "rules", rules: [] }, positional = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i] === "--policy") { out.policy = tokens[++i]; continue; }
    if (tokens[i] === "--rule") { out.rules.push(tokens[++i] ?? ""); continue; }
    positional.push(tokens[i]);
  }
  if (!["rules", "yolo"].includes(out.policy)) return { ...out, error: "Use --policy rules or --policy yolo" };
  if (out.rules.length > 8 || out.rules.some(r => !r || Buffer.byteLength(r) > 1024)) return { ...out, error: "At most 8 --rule values of up to 1024 bytes" };
  if (positional.length > 2) return { ...out, error: "Usage: /afk-work on [--policy rules|yolo] [--rule '<toml>']… [minutes] [requests]" };
  return { ...out, ...(positional[0] ? { minutes: positional[0] } : {}), ...(positional[1] ? { requests: positional[1] } : {}) };
}
