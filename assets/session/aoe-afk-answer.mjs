export const ANSWER_TOOL = "aoe_afk_answer";
export const DECIDE_TIMEOUT_MS = 5000;
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

export function askUserMenu(input) {
  if (!input || typeof input.question !== "string" || input.questions || input.allowMultiple || !Array.isArray(input.options) || !input.options.length) return null;
  return { source: "ask_user", format: "tool.ask_user", kind: "choice", question: input.question, subject: input.context ?? null,
    options: input.options.slice(0, 32).map((o, i) => ({ key: String(i + 1), label: String(o.title ?? "").slice(0, 256) })),
    free_text: input.allowFreeform !== false, raw: JSON.stringify(input).slice(0, 4096) };
}

export function createAnswerClient(bridge, { timeoutMs = DECIDE_TIMEOUT_MS } = {}) {
  return {
    async decide({ run, menu, text, proposal }) {
      try {
        const packet = { op: "decide", run, ...(menu ? { menu: { ...menu, ...("raw" in menu ? { raw: bounded(menu.raw) } : {}) } } : {}),
          ...(text ? { text: { ...text, text: bounded(text.text) } } : {}), ...(proposal ? { proposal } : {}) };
        const reply = await withTimeout(bridge.request(packet), timeoutMs);
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
