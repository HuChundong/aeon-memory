#!/usr/bin/env node
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to baseline checkout");
const out=new URL("./runtime_trace.json",import.meta.url);
const temp=mkdtempSync(join(tmpdir(),"aeon-memory-runtime-trace-"));
const runner=join(temp,"trace.mts");
const q=JSON.stringify;
writeFileSync(runner,`
import { writeFileSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { MemoryPipelineManager } from ${q(join(root,"src/utils/pipeline-manager.ts"))};
import { parseSceneBlock, formatSceneBlock, formatMeta } from ${q(join(root,"src/core/scene/scene-format.ts"))};
import { normalizeSceneFilename } from ${q(join(root,"src/core/scene/filename-normalizer.ts"))};
import { generateSceneNavigation, stripSceneNavigation } from ${q(join(root,"src/core/scene/scene-navigation.ts"))};
import { buildProfileStableId, listLocalProfiles } from ${q(join(root,"src/core/profile/profile-sync.ts"))};
import { parseSessionKey, sanitizeText, sanitizeJsonLine, parseJsonlSafe, isoToFilename } from ${q(join(root,"src/offload/storage.ts"))};
import { findActiveMmdInsertionPoint, findHistoryMmdInsertionPoint } from ${q(join(root,"src/offload/mmd-injector.ts"))};
import { normalizeToolCallIdForLookup, compactToolCall, extractAllToolUseIds } from ${q(join(root,"src/offload/l3-helpers.ts"))};
let fakeNow=1767225845000;
const NativeDate=Date;
// @ts-ignore deterministic oracle clock
globalThis.Date=class extends NativeDate { constructor(...args:any[]){ super(...(args.length?args:[fakeNow]) as [any]); } static now(){ return fakeNow++; } } as any;
const events:any[]=[];
const manager=new MemoryPipelineManager({everyNConversations:2,enableWarmup:false,l1:{idleTimeoutSeconds:60},l2:{delayAfterL1Seconds:60,minIntervalSeconds:0,maxIntervalSeconds:3600,sessionActiveWindowHours:24}});
manager.setL1Runner(async p=>{events.push({kind:"l1",session:p.sessionKey,messages:p.messages});return {success:true,lastSceneName:"scene-a",latestRecordUpdatedAt:"2026-01-02T03:04:05.000Z"}});
manager.setL2Runner(async (s,c)=>{events.push({kind:"l2",session:s,cursor:c??null});return {latestProcessedUpdatedAt:"2026-01-02T04:00:00.000Z",skipped:false}});
manager.setL3Runner(async()=>{events.push({kind:"l3"})});
manager.setPersister(async states=>events.push({kind:"persist",states:JSON.parse(JSON.stringify(states))}));
manager.start();
await manager.notifyConversation("s",[{role:"user",content:"one",timestamp:"2026-01-01T00:00:00Z"}]);
await manager.notifyConversation("s",[{role:"assistant",content:"two",timestamp:"2026-01-01T00:00:01Z"}]);
await new Promise(r=>setTimeout(r,20));
await manager.destroy();
const raw='---\\nsummary: Coding\\nkeywords: [rust, ai]\\nheat: 3\\nupdated: 2026-01-01\\n---\\nBody\\n';
const entries=[{filename:"coding.md",summary:"Coding",keywords:["rust","ai"],heat:3,updated:"2026-01-01"}];
const fsroot=join(${q(temp)},"profiles");mkdirSync(join(fsroot,"scene_blocks"),{recursive:true});
writeFileSync(join(fsroot,"scene_blocks","coding.md"),raw);writeFileSync(join(fsroot,"persona.md"),"Persona body");
const profiles=await listLocalProfiles(fsroot);
const messages=[{role:"user",content:"u"},{role:"assistant",content:[{type:"tool_use",id:"call_1",name:"shell",input:{cmd:"ls"}}]},{role:"tool",tool_call_id:"call_1",content:"ok"}];
const output={pipeline:{events,finalState:manager.getSessionState("s")??null,destroyed:manager.isDestroyed},scene:{parsed:parseSceneBlock(raw,"coding.md"),formatted:formatSceneBlock({created:"2025-01-01",summary:"S",keywords:["a","b"],heat:2,updated:"2026-01-01"},"Body"),meta:formatMeta({created:"2025-01-01",summary:"S",keywords:["a","b"],heat:2,updated:"2026-01-01"}),names:["Hello World","中文 场景","a/b:c"].map(normalizeSceneFilename),navigation:generateSceneNavigation(entries,"/data"),stripped:stripSceneNavigation("Persona\\n\\n<scene-navigation>old</scene-navigation>")},profile:{ids:[["scope","l2","coding.md"],["scope","l3","persona.md"]].map(x=>buildProfileStableId(x[0],x[1] as any,x[2])),profiles:profiles.map(p=>({id:p.id,type:p.type,filename:p.filename,content:p.content,version:p.version}))},offload:{sessionKeys:["agent:main:abc","bad","agent:x:swebench-w12-z"].map(parseSessionKey),sanitized:sanitizeText("a\\u0000b\\n c"),json:sanitizeJsonLine('{"x":"a\\n b"}'),parsed:parseJsonlSafe('{"tool_call_id":"x","summary":"s","timestamp":"t","tool_call":"c"}\\nBAD'),filename:isoToFilename("2026-01-02T03:04:05.678Z"),activePoint:findActiveMmdInsertionPoint(messages),historyPoint:findHistoryMmdInsertionPoint(messages),normalized:normalizeToolCallIdForLookup("toolu_abc-123"),compact:compactToolCall("shell: {\\\"cmd\\\":\\\"ls -la /very/long/path\\\"}"),toolIds:extractAllToolUseIds(messages[1])}};
writeFileSync(${q(out.pathname)},JSON.stringify(output,null,2)+"\\n");
`);
try { execFileSync("npx",["tsx",runner],{cwd:root,stdio:"inherit"}); } finally { rmSync(temp,{recursive:true,force:true}); }
