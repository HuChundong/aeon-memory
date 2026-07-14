#!/usr/bin/env node
import {execFileSync} from "node:child_process";
import {mkdtempSync, mkdirSync, rmSync, writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";

const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root) throw new Error("AEON_MEMORY_TS_BASELINE required");
const sha=execFileSync("git",["-C",root,"rev-parse","HEAD"],{encoding:"utf8"}).trim();
if(sha!=="4339e63650920871eb0e8888083a1779d114e3ae") throw new Error(`wrong baseline ${sha}`);
if(!process.env.AEON_MEMORY_DEGRADED_L1_CHILD){
  execFileSync("npx",["tsx",new URL(import.meta.url).pathname],{cwd:root,stdio:"inherit",env:{...process.env,AEON_MEMORY_DEGRADED_L1_CHILD:"1",TZ:"UTC"}});
  process.exit(0);
}
const {readConversationMessagesGroupedBySessionId}=await import(join(root,"src/core/conversation/l0-recorder.ts"));
const {formatExtractionPrompt}=await import(join(root,"src/core/prompts/l1-extraction.ts"));
const {initTimeModule}=await import(join(root,"src/utils/time.ts"));
initTimeModule({timezone:"UTC"});
const dir=mkdtempSync(join(tmpdir(),"aeon-memory-degraded-l1-"));
mkdirSync(join(dir,"conversations"),{recursive:true});
const lines=Array.from({length:60},(_,index)=>JSON.stringify({
  sessionKey:"degraded-60",sessionId:index<30?"round-a":"round-b",
  recordedAt:new Date(Date.UTC(2026,0,1,0,0,index)).toISOString(),
  id:`m${index.toString().padStart(2,"0")}`,role:index%2===0?"user":"assistant",
  content:`ordered-message-${index.toString().padStart(2,"0")}`,timestamp:1000+index
}));
writeFileSync(join(dir,"conversations","2026-01-01.jsonl"),lines.join("\n")+"\n");
const groups=await readConversationMessagesGroupedBySessionId("degraded-60",dir,undefined,undefined,50);
const output={groups:groups.map(group=>{
  const qualified=group.messages;
  const newMessages=qualified.slice(-10);
  const backgroundMessages=qualified.slice(Math.max(0,qualified.length-newMessages.length-5),qualified.length-newMessages.length);
  return {sessionId:group.sessionId,messages:qualified.map(message=>({id:message.id,content:message.content,timestamp:message.timestamp})),prompt:formatExtractionPrompt({newMessages,backgroundMessages,previousSceneName:"无"})};
})};
writeFileSync(new URL("degraded_l1_oracle.json",import.meta.url),JSON.stringify(output,null,2)+"\n");
rmSync(dir,{recursive:true,force:true});
