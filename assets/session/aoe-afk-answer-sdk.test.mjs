// Offline demo of AFK menu answering: one work period driven through the pinned Pi SDK with a scripted model.
// The bridge is a stub that implements `decide` and `recognise` with the Rust rules semantics (src/session/afk/answer),
// not a real `aoe __afk-bridge`; the Rust side has its own tests.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";

const source = process.env.AOE_PI_SOURCE_ROOT;
const hasDialogHook = async () => {
  if (!source) return false;
  try { return (await readFile(join(source, "packages/coding-agent/src/core/extensions/types.ts"), "utf8")).includes("ui_prompt_respond"); } catch { return false; }
};
const DIALOG_SKIP = "ui_prompt_respond not in this Pi runtime";
const RULES = ["id='ask', source='ask_user', answer='other'", "id='plans', kind='choice', answer='agent'"];
const PROMPT_COMMAND = "printf 'Install the dependencies?\\nOk to proceed? (y/N)\\n'; exit 1";
const dialogOutcomes = {};

const ROLE_WORDS = { approve: ["yes", "y", "apply", "proceed", "ok", "allow"], deny: ["no", "n", "skip", "cancel"] };
const roleFor = label => { const w = label.toLowerCase().split(/[^a-z0-9]+/)[0]; return ROLE_WORDS.approve.includes(w) ? "approve" : ROLE_WORDS.deny.includes(w) ? "deny" : "other"; };

// The Rust semantics the demo needs: first matching rule wins; yolo takes the proposal, then the first approve option, then the first option.
function stubDecide(run, menu, proposal) {
  const options = menu.options.map(o => ({ ...o, role: o.role ?? roleFor(o.label) }));
  const answer = (option, rule) => ({ decision: "answer", choice: option.key, text: null, rule, reason: proposal?.reason ?? "", warnings: [], checks: [] });
  const proposed = options.find(o => o.key === proposal?.key);
  if (run.policy === "yolo") return answer(proposed ?? options.find(o => o.role === "approve") ?? options[0], null);
  const rule = run.rules.map(r => Object.fromEntries([...r.matchAll(/(\w+)\s*=\s*'([^']*)'/g)].map(m => [m[1], m[2]])))
    .find(r => (!r.kind || r.kind === menu.kind) && (!r.source || r.source === menu.source));
  if (!rule) return { decision: "stop", reason: "no_rule", checks: [] };
  const option = rule.answer === "agent" ? proposed : options.find(o => o.role === rule.answer);
  return option ? answer(option, rule.id) : { decision: "stop", reason: rule.answer === "agent" ? "no_proposal" : "no_option_for_role", checks: [] };
}
const stubRecognise = (source, text) => {
  if (source === "agent_text" && /\[A\]pply\s*\/\s*\[S\]kip/.test(text))
    return { source, format: "text.bracket-shortkey", kind: "choice", question: "Apply the change?", options: [{ key: "A", label: "Apply", role: "approve" }, { key: "S", label: "Skip", role: "deny" }] };
  if (source === "bash_prompt" && /\((?:y\/N|Y\/n)\)\s*$/.test(text))
    return { source, format: "text.yes-no", kind: "confirm", question: text.trim().split("\n").at(-1), options: [{ key: "y", label: "Yes", role: "approve" }, { key: "n", label: "No", role: "deny" }] };
  return null;
};

async function runAnswerDemo(policy, dialog) {
  const sdk = await import(pathToFileURL(join(source, "packages/coding-agent/src/index.ts")));
  const { loadExtensions, createExtensionRuntime } = await import(pathToFileURL(join(source, "packages/coding-agent/src/core/extensions/loader.ts")));
  const { AssistantMessageEventStream } = await import(pathToFileURL(join(source, "packages/ai/src/utils/event-stream.ts")));
  assert.equal(sdk.VERSION, "1.2.0");
  const cwd = await mkdtemp(join(tmpdir(), "aoe-answer-sdk-")), key = `aoe-answer-sdk-${randomUUID()}`;
  const fixture = {}, errors = [], notices = [], decisions = [], deliveries = [], recognised = [];
  let session, calls = 0, armed = false, confirmations = 0, prompts = 0;
  globalThis[key] = fixture;
  const cleanup = async () => { session?.dispose(); delete globalThis[key]; await rm(cwd, { recursive: true, force: true }); };
  try {
    await writeFile(join(cwd, "notes.txt"), "notes\n");
    const manager = sdk.SessionManager.create(cwd, join(cwd, "sessions"));
    const binding = { instance_id: "offline-fixture", profile: "default", native_id: manager.getSessionId(), launch_id: randomUUID() };
    const bootstrap = { version: 6, app_dir: cwd, aoe_bin: "/unused/aoe", binding };
    Object.assign(fixture, { binding, capture(api, r) { fixture.api = api; fixture.r = r; }, command: null,
      bridge() { return { async request(p) {
        if (p.op === "poll") return { policy: null, probe: null, delegation: null, runtime_probe: null, work_period_capability: 1, work_period_request: fixture.command };
        if (p.op === "rules_check") return { valid: true, count: p.run.rules.length, sources: p.run.rules.map((_, i) => `inline rule ${i + 1}`), format_warnings: [] };
        if (p.op === "recognise") { const menu = stubRecognise(p.source, p.text); recognised.push({ source: p.source, matched: !!menu }); return { menu }; }
        if (p.op === "decide") {
          const menu = p.menu ?? stubRecognise(p.text.source, p.text.text);
          const decision = menu ? stubDecide(p.run, menu, p.proposal) : { decision: "stop", reason: "unrecognised_menu", checks: [] };
          const answer_id = randomUUID();
          decisions.push({ answer_id, source: menu?.source ?? p.text.source, format: menu?.format, decision });
          return { answer_id, menu, decision };
        }
        if (p.op === "answer_delivered") { deliveries.push({ answer_id: p.answer_id, delivered: p.delivered }); return {}; }
        return {};
      }, async close() {} }; } });
    const extension = join(cwd, "extension.mjs");
    await writeFile(extension, `import { VERSION } from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import { workPeriodRuntime,workCheckpointParameters } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-work-period.mjs")).href)};
import { answerParameters } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-answer.mjs")).href)};
import { registerAfk } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};
import { delegationRuntime } from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};
export default function(pi){const f=globalThis[${JSON.stringify(key)}];const delegation=delegationRuntime(pi);const api=workPeriodRuntime(pi,{version:VERSION,parameters:workCheckpointParameters(Type),answerParameters:answerParameters(Type)});
registerAfk(pi,{protocol:6,delegation,workPeriod:{...api,attach(r){api.attach(r);f.capture(api,r);}},bridge:f.bridge,schedule:()=>()=>{}});
// Stands in for the human-facing ask_user tool; AFK answers it in tool_call and never lets it run.
pi.registerTool({name:"ask_user",label:"Ask user",description:"Ask the human a question",parameters:Type.Object({question:Type.String(),options:Type.Array(Type.Object({title:Type.String()}))}),async execute(){throw Error("human prompt reached")}});
// A permission gate like the ones extensions add: it asks the human before a read.
if(${JSON.stringify(dialog)})pi.on("tool_call",async(e,ctx)=>{if(e.toolName==="read")await ctx.ui.confirm("Allow read of notes.txt?","Bash command: cat notes.txt");});}
`);
    const extensionRuntime = createExtensionRuntime(); extensionRuntime.flagValues.set("aoe-afk-binding", JSON.stringify(bootstrap));
    const loaded = await loadExtensions([extension], cwd, sdk.createEventBus(), extensionRuntime);
    assert.deepEqual(loaded.errors, []);
    const resources = { getExtensions: () => loaded, getSkills: () => ({ skills: [], diagnostics: [] }), getPrompts: () => ({ prompts: [], diagnostics: [] }),
      getThemes: () => ({ themes: [], diagnostics: [] }), getAgentsFiles: () => ({ agentsFiles: [] }), getSystemPrompt: () => "Offline menu-answering fixture",
      getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [], getAppendSystemPromptSources: () => [], extendResources() {}, reload: async () => {} };
    const model = { id: "offline", name: "offline", provider: "answer-fixture", api: "answer-fixture", baseUrl: "http://invalid.invalid", reasoning: false,
      input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 2000 };
    const modelRuntime = await sdk.ModelRuntime.create({ authPath: join(cwd, "absent-auth"), modelsPath: null, modelsStorePath: join(cwd, "absent-models"), refreshOnCreate: false, allowModelNetwork: false });
    const refOf = (messages, name) => {
      const m = messages.filter(x => x.role === "toolResult" && x.toolName === name).at(-1);
      for (const b of m?.content ?? []) { try { const r = JSON.parse(b.text)?.aoe_afk_work_evidence_ref; if (r) return r; } catch { /* not the ref block */ } }
      assert.fail(`missing model-visible ${name} evidence ref`);
    };
    modelRuntime.registerProvider(model.provider, { api: model.api, apiKey: "offline-not-a-credential", models: [model], streamSimple(_model, context) {
      assert.ok(++calls <= 14, "extra automatic provider request");
      const options = [{ key: "A", label: "Apply" }, { key: "S", label: "Skip" }];
      const answerCall = reason => ({ id: `answer-${calls}`, name: "aoe_afk_answer", arguments: { question: "Apply the change?", options, proposal: "A", reason } });
      const step = armed ? calls - 1 : 0;
      const tools = [() => null,
        () => null, // 1: ends the turn on a menu; AFK nudges once
        () => [answerCall("The change matches the task")], // 2
        () => [{ id: "ask-1", name: "ask_user", arguments: { question: "Which approach?", options: [{ title: "Keep the current design" }, { title: "Rewrite it" }] } }], // 3
        () => [{ id: "bash-1", name: "bash", arguments: { command: PROMPT_COMMAND } }], // 4: a command that waits for input
        () => [answerCall("Install is part of the task")], // 5: releases the held bash prompt
        () => [{ id: "read-1", name: "read", arguments: { path: "notes.txt" } }], // 6: runs the permission gate
        () => [{ id: "checkpoint-final", name: "aoe_afk_work_checkpoint", arguments: { status: "completed", summary: "Change applied and notes read",
          next_step: null, evidence: [refOf(context.messages, "read"), refOf(context.messages, "bash")], human_required: false, depends_on: [] } }], // 7
      ][step]?.() ?? [];
      const text = step === 1 ? "The change is ready.\n\n[A]pply / [S]kip" : "Ordinary task context ready";
      const message = { role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(),
        content: tools?.length ? tools.map(t => ({ type: "toolCall", ...t })) : [{ type: "text", text }], stopReason: tools?.length ? "toolUse" : "stop",
        usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
      const stream = new AssistantMessageEventStream(); stream.push({ type: "done", reason: message.stopReason, message }); stream.end(); return stream;
    } });
    ({ session } = await sdk.createAgentSession({ cwd, agentDir: cwd, model, modelRuntime, resourceLoader: resources, sessionManager: manager,
      settingsManager: sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off" }),
      tools: ["read", "write", "edit", "bash", "ask_user", "aoe_afk_work_checkpoint", "aoe_afk_answer"] }));
    session.subscribe(e => { if (e.type === "ui_prompt_start") prompts++; });
    await session.bindExtensions({ mode: "tui", uiContext: { setStatus() {}, getEditorText: () => "", notify: text => notices.push(text), confirm: async () => { confirmations++; return true; } }, onError: e => errors.push(e) });
    await session.prompt("Apply the change, install what it needs and finish the task"); await session.waitForIdle();
    assert.equal(calls, 1); armed = true;
    const rules = policy === "rules" ? RULES.map(r => `--rule "${r}"`).join(" ") : "";
    await session.prompt(`/afk-work on --policy ${policy} ${rules} 60 20`); await session.waitForIdle();
    await session.prompt("/afk-work status"); await session.waitForIdle();
    const records = sdk.SessionManager.open(manager.getSessionFile()).getEntries().filter(e => e.customType === "aoe-afk-work-period-v1");
    return { cleanup, sdk, session, fixture, records, decisions, deliveries, recognised, confirmations, prompts, notices, errors, calls, active: session.getActiveToolNames?.() ?? [] };
  } catch (e) { await cleanup(); throw e; }
}

const last = records => records.at(-1).data;
const bashPromptSeen = session => session.messages.find(m => m.role === "toolResult" && m.toolName === "bash")?.content.some(b => b.type === "text" && b.text.includes("aoe_afk_prompt_seen"));
const answered = (decisions, deliveries) => decisions.filter(d => d.decision.decision === "answer").map(d => ({ ...d, delivered: deliveries.find(x => x.answer_id === d.answer_id)?.delivered }));

test("rules policy answers the menu tool and ask_user, and stops on the unruled permission dialog", { skip: !source, timeout: 60000 }, async () => {
  const dialog = await hasDialogHook();
  const r = await runAnswerDemo("rules", dialog);
  try {
    assert.deepEqual(r.errors, []);
    assert.equal(last(r.records).policy, "rules");
    assert.deepEqual(r.recognised.filter(x => x.source === "agent_text"), [{ source: "agent_text", matched: true }], "one nudge for the [A]pply / [S]kip text");
    assert.equal(r.decisions[0].format, "tool.aoe_afk_answer");
    assert.ok(bashPromptSeen(r.session), "the waiting bash command is annotated, not decided");
    const sources = r.decisions.map(d => `${d.source}:${d.decision.decision === "answer" ? d.decision.rule : d.decision.reason}`);
    assert.deepEqual(sources.slice(0, 3), ["agent_text:plans", "ask_user:ask", "agent_text:plans"], "menu tool, ask_user, then the menu tool again after the held bash prompt");
    assert.equal(r.decisions[1].decision.choice, "1");
    assert.ok(answered(r.decisions, r.deliveries).slice(0, 3).every(d => d.delivered === "ok"));
    if (dialog) {
      assert.deepEqual(sources.slice(3), ["pi_dialog:no_rule"], "the permission dialog has no rule");
      assert.equal(last(r.records).status, "blocked"); assert.equal(last(r.records).reason, "human_prompt");
      assert.equal(r.confirmations, 2, "consent plus the dialog Pi opened for the human");
      assert.match(r.notices.at(-1), /answers: 3 \(rules 3, yolo 0\), stops: 1/);
    } else {
      assert.equal(last(r.records).status, "completed");
      assert.match(r.notices.at(-1), /answers: 3 \(rules 3, yolo 0\), stops: 0/);
    }
    dialogOutcomes.rules = { dialog, sources };
    console.log(JSON.stringify({ scenario: "rules", dialog, tools: r.active, decisions: sources, status: last(r.records).status, reason: last(r.records).reason }));
  } finally { await r.cleanup(); }
});

test("yolo policy answers every menu and the period completes", { skip: !source, timeout: 60000 }, async () => {
  const dialog = await hasDialogHook();
  const r = await runAnswerDemo("yolo", dialog);
  try {
    assert.deepEqual(r.errors, []);
    assert.equal(last(r.records).policy, "yolo");
    assert.equal(last(r.records).status, "completed", JSON.stringify({ notices: r.notices, records: r.records.map(e => e.data) }));
    const sources = r.decisions.map(d => `${d.source}:${d.decision.decision}`);
    assert.deepEqual(sources.slice(0, 3), ["agent_text:answer", "ask_user:answer", "agent_text:answer"]);
    assert.ok(r.decisions.every(d => d.decision.decision === "answer"));
    assert.ok(answered(r.decisions, r.deliveries).every(d => d.delivered === "ok"));
    assert.ok(bashPromptSeen(r.session)); assert.equal(r.records.at(-1).data.checkpoint_count, 1);
    if (dialog) {
      assert.deepEqual(sources.slice(3), ["pi_dialog:answer"]);
      assert.equal(r.decisions[3].decision.choice, "y");
      assert.equal(r.confirmations, 1, "only the consent dialog reached the human");
      assert.equal(r.prompts, 0, "the permission dialog was answered without a human prompt");
      assert.match(r.notices.at(-1), /answers: 4 \(rules 0, yolo 4\), stops: 0/);
    } else assert.match(r.notices.at(-1), /answers: 3 \(rules 0, yolo 3\), stops: 0/);
    dialogOutcomes.yolo = { dialog, sources };
    console.log(JSON.stringify({ scenario: "yolo", dialog, tools: r.active, decisions: sources, status: last(r.records).status }));
  } finally { await r.cleanup(); }
});

test("Pi confirm dialogs are answered through ui_prompt_respond", { skip: !source ? "AOE_PI_SOURCE_ROOT not set" : false, timeout: 60000 }, async t => {
  if (!await hasDialogHook()) { t.skip(DIALOG_SKIP); return; }
  assert.deepEqual(dialogOutcomes.rules?.sources.at(-1), "pi_dialog:no_rule");
  assert.deepEqual(dialogOutcomes.yolo?.sources.at(-1), "pi_dialog:answer");
});
