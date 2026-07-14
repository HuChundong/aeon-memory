#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root) throw new Error("AEON_MEMORY_TS_BASELINE is required");
const sha=execFileSync("git",["-C",root,"rev-parse","HEAD"],{encoding:"utf8"}).trim();
if(sha!=="4339e63650920871eb0e8888083a1779d114e3ae") throw new Error(`wrong baseline ${sha}`);
const here=new URL(".",import.meta.url).pathname;
const temp=mkdtempSync(join(tmpdir(),"aeon-memory-l0-oracle-"));
const runner=join(temp,"run.mts");
writeFileSync(runner,`
import {recordConversation,readConversationRecords} from ${JSON.stringify(join(root,"src/core/conversation/l0-recorder.ts"))};
import {writeFileSync} from "node:fs";
const root=${JSON.stringify(temp)};
const cases=[
 {name:"cursor",params:{sessionKey:"s",sessionId:"sid",rawMessages:[{role:"user",content:"old",timestamp:100},{role:"assistant",content:"new answer",timestamp:201},{role:"user",content:"new question",timestamp:202}],afterTimestamp:200}},
 {name:"slice_replace",params:{sessionKey:"s",sessionId:"sid",rawMessages:[{role:"user",content:"history",timestamp:1},{role:"assistant",content:"history answer",timestamp:2},{role:"user",content:"<memory-context>polluted</memory-context> actual",timestamp:300},{role:"assistant",content:"before\\n"+String.fromCharCode(96).repeat(3)+"js\\nsecret()\\n"+String.fromCharCode(96).repeat(3)+"\\nafter",timestamp:301}],originalUserMessageCount:2,originalUserText:"clean user prompt"}},
 {name:"content_parts",params:{sessionKey:"parts",sessionId:"p",rawMessages:[{role:"user",content:[{type:"text",text:"hello"},{type:"image",data:"data:image/png;base64,AAAA"}],timestamp:400},{role:"assistant",content:[{type:"text",text:"world"}],timestamp:401}]}}
];
const output=[];
for(const c of cases){const baseDir=root+"/"+c.name;const returned=await recordConversation({...c.params,baseDir});const records=await readConversationRecords(c.params.sessionKey,baseDir);const clean=(x:any)=>({role:x.role,content:x.content,timestamp:x.timestamp});output.push({name:c.name,returned:returned.map(clean),persisted:records.flatMap((r:any)=>r.messages.map(clean))});}
writeFileSync(${JSON.stringify(join(here,"l0_runtime_oracle.json"))},JSON.stringify(output,null,2)+"\\n");
`);
try{execFileSync(process.execPath,[join(root,"node_modules/tsx/dist/cli.mjs"),runner],{cwd:root,stdio:"inherit"});}finally{rmSync(temp,{recursive:true,force:true});}
