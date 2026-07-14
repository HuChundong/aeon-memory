#!/usr/bin/env node
import { execFileSync } from "node:child_process"; import { mkdtempSync,readFileSync,rmSync,writeFileSync } from "node:fs"; import {tmpdir} from "node:os"; import {join,resolve} from "node:path";
const root=process.env.AEON_MEMORY_TS_BASELINE;if(!root)throw Error("AEON_MEMORY_TS_BASELINE is required");
const output=resolve(process.env.AEON_MEMORY_ORACLE_OUTPUT??new URL("reclaim_oracle.json",import.meta.url).pathname),temp=mkdtempSync(join(tmpdir(),"aeon-memory-reclaim-")),runner=join(temp,"run.ts"),result=join(temp,"out.json");
writeFileSync(runner,`import {mkdirSync,writeFileSync,readdirSync,readFileSync,statSync,utimesSync} from "node:fs";import{join,relative}from"node:path";import{reclaimOffloadData}from ${JSON.stringify(join(root,"src/offload/reclaimer.ts"))};(async()=>{
const d=join(${JSON.stringify(temp)},"data"),a=join(d,"agent");mkdirSync(join(a,"refs"),{recursive:true});mkdirSync(join(a,"mmds"),{recursive:true});
const old=new Date("2000-01-01T00:00:00Z"),fresh=new Date("2100-01-01T00:00:00Z");const put=(p,c,age=old)=>{writeFileSync(p,c);utimesSync(p,age,age)};
put(join(d,"offload-root.jsonl"),"{}\\n");put(join(a,"offload-old.jsonl"),"{}\\n");put(join(a,"offload-live.jsonl"),JSON.stringify({result_ref:"refs/kept.md"})+"\\n",fresh);
put(join(a,"refs","kept.md"),"keep");put(join(a,"refs","orphan.md"),"gone");put(join(a,"refs","ignored.bin"),"stay");
for(let i=0;i<17;i++)put(join(a,"mmds",String(i).padStart(2,"0")+".mmd"),"m",i<3?old:fresh);writeFileSync(join(a,"state.json"),JSON.stringify({activeMmdFile:"00.mmd"}));
writeFileSync(join(a,"sessions-registry.json"),JSON.stringify({missing:{offloadFile:"offload-old.jsonl",updatedAt:"2100-01-01T00:00:00Z"},expired:{updatedAt:"2000-01-01T00:00:00Z"},fresh:{updatedAt:"2100-01-01T00:00:00Z"}}));
put(join(d,"debug.log"),"x".repeat(900000),fresh);put(join(d,"trace.jsonl"),"y".repeat(300000),fresh);put(join(d,"offload-data.jsonl"),"z".repeat(300000),fresh);
const stats=await reclaimOffloadData(d,{retentionDays:3,logMaxSizeMb:1},{warn(){},debug(){}});const disabled=await reclaimOffloadData(d,{retentionDays:2,logMaxSizeMb:1},{warn(){},debug(){}});const missing=await reclaimOffloadData(join(d,"absent"),{retentionDays:3,logMaxSizeMb:1},{warn(){},debug(){}});const walk=p=>readdirSync(p,{withFileTypes:true}).flatMap(e=>e.isDirectory()?walk(join(p,e.name)):[{path:relative(d,join(p,e.name)),size:statSync(join(p,e.name)).size}]).sort((x,y)=>x.path.localeCompare(y.path));
writeFileSync(${JSON.stringify(result)},JSON.stringify({stats,disabled,missing,files:walk(d),registry:JSON.parse(readFileSync(join(a,"sessions-registry.json"),"utf8"))},null,2)+"\\n");})()`);
try{execFileSync(process.execPath,[process.env.AEON_MEMORY_TSX_CLI??join(root,"node_modules/tsx/dist/cli.mjs"),runner],{cwd:root,stdio:"inherit"});writeFileSync(output,readFileSync(result))}finally{rmSync(temp,{recursive:true,force:true})}
