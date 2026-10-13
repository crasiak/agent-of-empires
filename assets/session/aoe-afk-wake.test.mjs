import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID } from "node:crypto";
import { automaticWake, receiverClock } from "./aoe-afk-wake.mjs";
import { createUsageWitness } from "./aoe-afk-usage-guard.mjs";

const core=process.env.AOE_PI_SOURCE_ROOT;
const spec={kind:"hrtime",node:"v24.13.0",uv:"1.51.0"};
const until=async predicate=>{const end=Date.now()+10000;while(!predicate()){assert.ok(Date.now()<end,"observable deadline");await new Promise(r=>setImmediate(r));}};

test("receiver clock refuses unknown equivalence and bounds text-clock precision",()=>{
  const hr=receiverClock(spec,{node:spec.node,uv:spec.uv,hrtime:()=>123n});assert.deepEqual(hr(),{tick:123n,uncertainty:0n});
  assert.equal(receiverClock(spec,{node:"other",uv:spec.uv})(),undefined);
  assert.equal(receiverClock(spec,{node:spec.node,uv:"other"})(),undefined);
  for(const [text,valid] of [["123.45 22.10\n",true],["123.4 22.10\n",false],["bad",false],["9".repeat(130),false]]) {
    const read=receiverClock({kind:"uptime_file",path:"/fixture/uptime",resolution_ns:10000000},{read:()=>text});
    assert.equal(!!read(),valid);
    if(valid)assert.deepEqual(read(),{tick:123450000000n,uncertainty:10000000n});
  }
});

test("delivery uses pre-Poll clock age, conservative precision and one invocation",()=>{
  for(const mode of ["valid","suspend","uncertain-edge","backward","generation","binding","native-replaced","activity","ready-changed","bad-version","invalid-clock"]) {
    let now=100000000000n,sends=0,eligible=mode!=="ready-changed";
    const binding={profile:"alpha",instance_id:"x",native_id:randomUUID(),launch_id:randomUUID()};
    const r={bootstrap:{binding,wake_clock:spec},generation:randomUUID(),ctx:{sessionManager:{getSessionId:()=>binding.native_id},ui:{onTerminalInput:()=>()=>{}}}};
    const w=automaticWake({sendMessage(){sends++;}},()=>eligible,{node:spec.node,uv:spec.uv,hrtime:()=>now});w.attach(r);
    const observation=w.observation(r);assert.equal(observation.request.ready,eligible);eligible=true;
    const payload={version:1,state:"attempted",id:randomUUID(),binding,generation:r.generation,incarnation:randomUUID(),remaining_ms:1000,text:"configured continuation"};
    now+=1000000n;
    if(mode==="suspend")now+=30000000000n;
    if(mode==="uncertain-edge") {observation.sample.uncertainty=1000000000n;}
    if(mode==="backward")now=0n;
    if(mode==="generation")payload.generation=randomUUID();
    if(mode==="binding")payload.binding={...binding,launch_id:randomUUID()};
    if(mode==="native-replaced")r.ctx.sessionManager.getSessionId=()=>randomUUID();
    if(mode==="bad-version"){payload.version=2;payload.state="empty";}
    if(mode==="activity")w.activity(r);
    if(mode==="invalid-clock")r.automaticWake.clock=()=>undefined;
    w.receive(r,payload,observation);w.receive(r,payload,observation);
    assert.equal(sends,mode==="valid"?1:0,mode);
    assert.equal(w.observation(r).request.ready,false);
  }
});

async function sdkCase(mode) {
  const sdk=await import(pathToFileURL(join(core,"packages/coding-agent/src/index.ts")));
  const {loadExtensions,createExtensionRuntime}=await import(pathToFileURL(join(core,"packages/coding-agent/src/core/extensions/loader.ts")));
  const {AssistantMessageEventStream}=await import(pathToFileURL(join(core,"packages/ai/src/utils/event-stream.ts")));
  const cwd=await mkdtemp(join(tmpdir(),"afk-wake-api-"));let session,ui;
  const activityMode=mode.startsWith("activity-");
  try {
    const manager=sdk.SessionManager.create(cwd,join(cwd,"sessions"));
    const base={role:"assistant",api:"wake-fixture",provider:"wake-fixture",model:"fake",timestamp:1,content:[{type:"text",text:"previous ordinary history"}],stopReason:"stop",usage:{input:1,output:1,cacheRead:0,cacheWrite:0,totalTokens:2,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}}};manager.appendMessage(base);
    const binding={profile:"default",instance_id:"test",native_id:manager.getSessionId(),launch_id:randomUUID()};
    const bootstrap={version:6,app_dir:cwd,aoe_bin:"/unused",binding,wake_clock:spec};
    if(mode==="usage") {
      const old={bootstrap,generation:randomUUID(),recoveredUsageWindows:new Set(),ctx:{sessionManager:manager}};
      createUsageWitness({appendEntry:(t,d)=>manager.appendCustomEntry(t,d)},old).require({binding,generation:old.generation,window:randomUUID(),usage_policy_hash:"a".repeat(64),state:"owned"});
    }
    if(mode==="question") manager.appendCustomEntry("aoe-afk-question-v1",{version:1,window:randomUUID(),binding,generation:randomUUID(),phase:"intent",identity:{operation:randomUUID(),tool_call_id:"question",interaction_id:randomUUID(),kind:"input",arguments_hash:"a".repeat(64),context_ref:manager.getLeafId()}});
    const payloadId=randomUUID(),incarnation=randomUUID();let r,calls=0,inputs=0,settled=0,submitted=0,commands=0,releaseReply,tick=1000000000n,generation,first=true;
    const ops=[],errors=[];
    globalThis.__wakeAPI={bootstrap,invoked(){submitted++;},command(){commands++;},capture(value){r=value;r.automaticWake.clock=()=>({tick,uncertainty:0n});},input(){inputs++;},settled(){settled++;},bridge(_boot,gen){generation=gen;return {async close(){},async request(packet){
      ops.push(packet);
      if(packet.op!=="poll")return {};
      if(first && mode==="transient") {first=false;throw Error("transient bridge validation failure");}
      const hold=first && activityMode;
      let wake={version:1,state:"attempted",id:payloadId,binding,generation,incarnation,remaining_ms:1000,text:"configured automatic continuation"};
      if(mode==="generation")wake.generation=randomUUID();
      if(mode==="binding")wake.binding={...binding,launch_id:randomUUID()};
      if(mode==="suspend"&&first)tick+=30000000000n;
      if(mode==="unavailable")wake={version:1,state:"unavailable"};
      first=false;
      const historical=mode==="foreign-history"?{version:4,window:randomUUID(),binding:{...binding,native_id:randomUUID()},generation:randomUUID(),state:"ended",confirmed:false,issued_at_ms:Date.now()-1000,expires_at_ms:Date.now()+60000,grant:{settlement_nudges:0,question_deferrals:0,requests:1},usage_policy_hash:"b".repeat(64),usage:{blocked:"exhausted"}}:null;
      const value={policy:null,probe:null,delegation:historical,runtime_probe:null,question_guards:[],automatic_wake:wake};
      if(hold)return new Promise(resolve=>releaseReply=()=>resolve(value));
      return value;
    }};}};
    const fixture=join(cwd,"extension.mjs");await writeFile(fixture,`import {registerAfk} from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk.mjs")).href)};import {delegationRuntime} from ${JSON.stringify(pathToFileURL(resolve("assets/session/aoe-afk-runtime.mjs")).href)};export default function(pi){const f=globalThis.__wakeAPI;pi.registerCommand('handled-wake',{handler:async()=>f.command()});const d=delegationRuntime({...pi,sendMessage(...args){f.invoked();pi.sendMessage(...args);}});registerAfk(pi,{protocol:6,delegation:{...d,attach(r){d.attach(r);f.capture(r);}},bridge:f.bridge,schedule:()=>()=>{}});pi.on('input',f.input);pi.on('agent_settled',f.settled);}`);
    const runtime=createExtensionRuntime();runtime.flagValues.set("aoe-afk-binding",JSON.stringify(bootstrap));const loaded=await loadExtensions([fixture],cwd,sdk.createEventBus(),runtime);assert.deepEqual(loaded.errors,[]);
    const resourceLoader={getExtensions:()=>loaded,getSkills:()=>({skills:[],diagnostics:[]}),getPrompts:()=>({prompts:[],diagnostics:[]}),getThemes:()=>({themes:[],diagnostics:[]}),getAgentsFiles:()=>({agentsFiles:[]}),getSystemPrompt:()=>"Offline wake",getSystemPromptSource:()=>undefined,getAppendSystemPrompt:()=>[],getAppendSystemPromptSources:()=>[],extendResources(){},async reload(){}};
    const model={id:"fake",name:"fake",provider:"wake-fixture",api:"wake-fixture",baseUrl:"http://invalid.invalid",reasoning:false,input:["text"],cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:100000,maxTokens:1000};
    const modelRuntime=await sdk.ModelRuntime.create({authPath:join(cwd,"absent-auth"),modelsPath:null,modelsStorePath:join(cwd,"absent-models"),refreshOnCreate:false,allowModelNetwork:false});
    modelRuntime.registerProvider(model.provider,{api:model.api,apiKey:"offline-not-a-credential",models:[model],streamSimple(_model,context,options){assert.equal(options.signal.aborted,false);calls++;assert.ok(calls<=1);const message={...base,timestamp:Date.now()};const stream=new AssistantMessageEventStream();stream.push({type:"done",reason:"stop",message});stream.end();return stream;}});
    ({session}=await sdk.createAgentSession({cwd,agentDir:cwd,model,modelRuntime,resourceLoader,sessionManager:manager,tools:[],settingsManager:sdk.SettingsManager.inMemory({compaction:{enabled:false},retry:{enabled:false},cacheWarming:"off"})}));
    let raw={setStatus(){},getEditorText:()=>mode==="draft"?"original draft":"",onTerminalInput:()=>()=>{}}, terminal,editor,draft="",commandWork,modal;
    if(activityMode){
      const {Container,Text,isViewportTUI}=await import(pathToFileURL(join(core,"packages/tui/src/index.ts")));
      const {VirtualTerminal}=await import(pathToFileURL(join(core,"packages/tui/test/virtual-terminal.ts")));
      const {createInteractiveTui,InteractiveMode}=await import(pathToFileURL(join(core,"packages/coding-agent/src/modes/interactive/interactive-mode.ts")));
      const {KeybindingsManager}=await import(pathToFileURL(join(core,"packages/coding-agent/src/core/keybindings.ts")));
      const {initTheme}=await import(pathToFileURL(join(core,"packages/coding-agent/src/modes/interactive/theme/theme.ts")));initTheme("dark");
      terminal=new VirtualTerminal();ui=createInteractiveTui({tuiMode:"regular",showHardwareCursor:false,logDirectory:cwd,terminal});ui.requestRender=()=>{};
      editor=Object.assign(new Text("",0,0),{getText:()=>draft,handleInput(data){if(data==="\r"){commandWork=session.prompt(draft);draft="";}else if(data==="\x7f")draft=draft.slice(0,-1);else draft+=data;}});
      const editorContainer=new Container();editorContainer.addChild(editor);ui.addChild(editorContainer);if(isViewportTUI(ui))ui.setLayoutRoot(editorContainer);ui.setFocus(editor);ui.start();
      const modeObject=Object.assign(Object.create(InteractiveMode.prototype),{ui,renderer:ui,editor,editorContainer,runtimeHost:{session},keybindings:new KeybindingsManager(),extensionDialogClosers:new Map(),extensionTerminalInputSubscriptions:new Set(),updateStatusLine(){}});
      raw=modeObject.createExtensionUIContext();raw.setStatus=()=>{};
    }
    if(mode==="observer-unavailable")raw.onTerminalInput=undefined;
    const bindingRun=session.bindExtensions({mode:"tui",uiContext:raw,onError:e=>errors.push(e)});
    if(activityMode){
      await until(()=>typeof releaseReply==="function");
      if(mode==="activity-draft"){terminal.sendInput("x");assert.equal(draft,"x","read-only observer preserves input");terminal.sendInput("\x7f");assert.equal(draft,"");}
      if(mode==="activity-command"){terminal.sendInput("/handled-wake");terminal.sendInput("\r");await commandWork;assert.equal(commands,1);assert.equal(inputs,0,"extension command precedes input event");}
      if(mode==="activity-ui"){modal=r.ctx.ui.input("Unrelated UI activity");await until(()=>ui.getFocusedComponent()!==editor);}
      releaseReply();
    }
    await bindingRun;
    if(mode==="transient")await r.refresh();
    assert.equal(submitted,["valid","foreign-history"].includes(mode)?1:0,"intervening failures or native activity must suppress invocation");
    if(modal){ui.getFocusedComponent().handleInput("\x1b");await modal;}
    const positive=["valid","foreign-history"].includes(mode);
    if(positive) {await until(()=>settled>0);await session.waitForIdle();assert.equal(calls,1);}
    else {assert.equal(calls,0);assert.ok(r.automaticWake.closed);}
    const initial=manager.getEntries().filter(e=>e.customType==="aoe-automatic-wake-v1");assert.equal(initial.length,positive?1:0);assert.equal(inputs,0);
    assert.equal(manager.getEntries().filter(e=>e.type==="message"&&e.message.role==="user").length,0,"wake did not create an ordinary user boundary");
    await r.refresh();assert.equal(calls,positive?1:0,"replayed host payload was not reinvoked");
    if(mode==="usage") {assert.equal(r.usageWitness.pending,1);assert.equal(ops.filter(p=>p.op==="usage_recover").length,0);}
    if(mode==="question") {assert.equal(r.restoredGuards.size,1);assert.equal(ops.filter(p=>p.op==="question_recover").length,0);}
    if(["unavailable","transient","observer-unavailable"].includes(mode)) {await session.prompt("fresh ordinary manual work");await session.waitForIdle();assert.equal(calls,1,"wake-only refusal did not veto unrelated manual work");}
    assert.deepEqual(errors,[]);
    return {mode,calls,inputs,automationEntries:initial.length};
  } finally {await session?.extensionRunner.emit({type:"session_shutdown",reason:"exit"});session?.dispose();ui?.stop();delete globalThis.__wakeAPI;await rm(cwd,{recursive:true,force:true});}
}

test("actual SDK wake never becomes input or releases native protection",{skip:!core,timeout:60000},async()=>{
  for(const mode of ["valid","foreign-history","usage","question","draft","generation","binding","suspend","unavailable"]) console.log(JSON.stringify(await sdkCase(mode)));
});
test("failed native refresh cannot regain wake eligibility",{skip:!core,timeout:60000},async()=>console.log(JSON.stringify(await sdkCase("transient"))));
test("native terminal and UI activity permanently closes wake eligibility",{skip:!core,timeout:60000},async()=>{
  for(const mode of ["activity-draft","activity-command","activity-ui","observer-unavailable"]) console.log(JSON.stringify(await sdkCase(mode)));
});
