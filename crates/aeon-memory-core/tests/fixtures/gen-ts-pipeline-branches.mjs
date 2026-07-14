#!/usr/bin/env node
import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {execFileSync} from "node:child_process";
const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to baseline checkout");
const temp=mkdtempSync(join(tmpdir(),"aeon-memory-pipeline-branches-"));
const runner=join(temp,"branches.mts"); const q=JSON.stringify;
const output=new URL("./pipeline_branches.json",import.meta.url).pathname;
writeFileSync(runner,`
import {writeFileSync} from "node:fs";
import {MemoryPipelineManager} from ${q(join(root,"src/utils/pipeline-manager.ts"))};
const sleep=(ms:number)=>new Promise(r=>setTimeout(r,ms));
// Timer delivery remains real, but wall-clock observations are pinned so the
// oracle records only pipeline semantics rather than the generation instant.
const RealDate=Date;
const FIXED_NOW=Date.parse("2026-02-02T03:04:05.000Z");
class FixedDate extends RealDate {
  constructor(...args:any[]){super(...(args.length?args:[FIXED_NOW]) as []);}
  static now(){return FIXED_NOW;}
}
globalThis.Date=FixedDate as DateConstructor;
const cfg=(idle=.01,delay=.01)=>({everyNConversations:1,enableWarmup:false,l1:{idleTimeoutSeconds:idle},l2:{delayAfterL1Seconds:delay,minIntervalSeconds:0,maxIntervalSeconds:3600,sessionActiveWindowHours:24}});
const msg=(x:string)=>[{role:"user",content:x,timestamp:"2026-01-01T00:00:00Z"}];
async function retry(){const e:any[]=[];let n=0;const m=new MemoryPipelineManager(cfg());(m as any).L1_RETRY_DELAY_MS=5;m.setL1Runner(async p=>{n++;e.push({kind:"l1",attempt:n,count:p.msg.length});if(n<3)throw new Error("planned");return {processedCount:p.msg.length}});m.setL2Runner(async()=>{e.push({kind:"l2"});return {skipped:true}});m.setL3Runner(async()=>e.push({kind:"l3"}));await m.notifyConversation("retry",msg("x"));await sleep(30);await m.destroy();return {events:e,attempts:n,state:m.getSessionState("retry"),destroyed:m.isDestroyed};}
async function retryExhausted(){let n=0;const m=new MemoryPipelineManager(cfg());(m as any).L1_RETRY_DELAY_MS=3;m.setL1Runner(async()=>{n++;throw new Error("always")});await m.notifyConversation("fail",msg("x"));await sleep(30);await m.destroy();return {attempts:n,buffered:m.getBufferedMessageCount("fail"),state:m.getSessionState("fail"),destroyed:m.isDestroyed};}
async function recovery(){const e:any[]=[];const m=new MemoryPipelineManager(cfg(.01,.005));m.setL1Runner(async()=>{e.push({kind:"l1"});return {processedCount:0}});m.setL2Runner(async(s,c)=>{e.push({kind:"l2",session:s,cursor:c??null});return {latestCursor:"2026-02-01T00:00:00Z",skipped:false}});m.setL3Runner(async()=>e.push({kind:"l3"}));m.start({recover:{conversation_count:2,last_extraction_time:"",last_extraction_updated_time:"cursor-old",last_active_time:Date.now(),l2_pending_l1_count:1,warmup_threshold:0,l2_last_extraction_time:""}});await sleep(20);await m.destroy();return {events:e,state:m.getSessionState("recover"),destroyed:m.isDestroyed};}
async function interleave(){const e:any[]=[];const m=new MemoryPipelineManager({...cfg(.012,.05),everyNConversations:99});m.setL1Runner(async p=>{e.push({kind:"l1",session:p.sessionKey,contents:p.msg.map(x=>x.content)});return {processedCount:p.msg.length}});m.setL2Runner(async s=>{e.push({kind:"l2",session:s});return {skipped:true}});m.setL3Runner(async()=>e.push({kind:"l3"}));await m.notifyConversation("a",msg("a1"));await sleep(4);await m.notifyConversation("a",msg("a2"));await m.notifyConversation("b",msg("b1"));await sleep(25);await m.flushSession("b");await m.destroy();return {events:e,a:m.getSessionState("a"),b:m.getSessionState("b"),destroyed:m.isDestroyed};}
writeFileSync(${q(output)},JSON.stringify({retry:await retry(),retryExhausted:await retryExhausted(),recovery:await recovery(),interleave:await interleave()},null,2)+"\\n");
`);
try{execFileSync("npx",["tsx",runner],{cwd:root,stdio:"inherit"});}finally{rmSync(temp,{recursive:true,force:true});}
