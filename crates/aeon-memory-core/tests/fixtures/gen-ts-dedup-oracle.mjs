#!/usr/bin/env node
import {execFileSync} from "node:child_process";import {writeFileSync} from "node:fs";import {join} from "node:path";
const root=process.env.AEON_MEMORY_TS_BASELINE;if(!root)throw new Error("AEON_MEMORY_TS_BASELINE required");const sha=execFileSync("git",["-C",root,"rev-parse","HEAD"],{encoding:"utf8"}).trim();if(sha!=="4339e63650920871eb0e8888083a1779d114e3ae")throw new Error(`wrong baseline ${sha}`);
if(!process.env.AEON_MEMORY_DEDUP_ORACLE_CHILD){execFileSync("npx",["tsx",new URL(import.meta.url).pathname],{cwd:root,stdio:"inherit",env:{...process.env,AEON_MEMORY_DEDUP_ORACLE_CHILD:"1"}});process.exit(0);}
const {batchDedup}=await import(join(root,"src/core/record/l1-dedup.ts"));
const memories=[{record_id:"new-store",content:"store memory",type:"persona",priority:50,scene_name:"prefs",source_message_ids:[],metadata:{},timestamps:[]},{record_id:"new-skip",content:"skip memory",type:"persona",priority:60,scene_name:"prefs",source_message_ids:[],metadata:{},timestamps:[]},{record_id:"new-update",content:"update memory",type:"persona",priority:70,scene_name:"prefs",source_message_ids:[],metadata:{},timestamps:[]}];
const candidate={record_id:"old-1",content:"old memory",type:"persona",priority:40,scene_name:"prefs",score:.9,timestamp_str:"2026-01-01T00:00:00Z",session_key:"s",session_id:"i",metadata_json:"{}"};
const trace={fts:[],vectors:[],embeds:[],llm:[]};const store={countL1:async()=>1,isFtsAvailable:()=>true,searchL1Fts:async(q,l)=>{trace.fts.push([q,l]);return [candidate]},searchL1Vector:async(v,l,t)=>{trace.vectors.push([v,l,t]);return [candidate]}};
const embedding={embedBatch:async(texts,opts)=>{trace.embeds.push([texts,opts]);return texts.map((_,i)=>[i+1,0])}};
const answer=JSON.stringify([{record_id:"new-store",action:"store",target_ids:[]},{record_id:"new-skip",action:"skip",target_ids:["old-1"]},{record_id:"new-update",action:"update",target_ids:["old-1"],merged_content:"merged"}]);
const llm={run:async p=>{trace.llm.push(p);return answer}};const decisions=await batchDedup({memories,config:{},vectorStore:store,embeddingService:embedding,embeddingTimeoutMs:321,llmRunner:llm,conflictRecallTopK:2});
const failed=await batchDedup({memories:[memories[0]],config:{},vectorStore:{...store,countL1:async()=>0},llmRunner:{run:async()=>{throw new Error("boom")}}});
const noRecall=await batchDedup({memories:[memories[0]],config:{}});
writeFileSync(join(new URL(".",import.meta.url).pathname,"dedup_runtime_oracle.json"),JSON.stringify({decisions,failed,noRecall,trace},null,2)+"\n");
