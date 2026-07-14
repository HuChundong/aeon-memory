#!/usr/bin/env node
import {mkdtempSync,mkdirSync,writeFileSync,rmSync} from "node:fs";import{tmpdir}from"node:os";import{join}from"node:path";import{pathToFileURL}from"node:url";
const root=process.env.AEON_MEMORY_TS_BASELINE;if(!root)throw Error("AEON_MEMORY_TS_BASELINE required");
const {PersonaTrigger}=await import(pathToFileURL(join(root,"src/core/persona/persona-trigger.ts")));const {CheckpointManager}=await import(pathToFileURL(join(root,"src/utils/checkpoint.ts")));
const cases=[
 {name:"explicit",cp:{request_persona_update:true,persona_update_reason:"manual"},scene:false,persona:null},
 {name:"explicit-default",cp:{request_persona_update:true,persona_update_reason:""},scene:false,persona:null},
 {name:"cold",cp:{scenes_processed:2,last_persona_at:0},scene:true,persona:null},
 {name:"recovery-missing",cp:{scenes_processed:2,last_persona_at:4},scene:true,persona:null},
 {name:"recovery-nav-only",cp:{scenes_processed:2,last_persona_at:4},scene:true,persona:"---\n## 🗺️ Scene Navigation (Scene Index)\nnav"},
 {name:"first-scene",cp:{scenes_processed:1,last_persona_at:0,memories_since_last_persona:1},scene:false,persona:null},
 {name:"threshold",cp:{scenes_processed:3,last_persona_at:3,memories_since_last_persona:5},scene:false,persona:"body"},
 {name:"none",cp:{scenes_processed:3,last_persona_at:3,memories_since_last_persona:4},scene:false,persona:"body"},
];
const out=[];for(const c of cases){const d=mkdtempSync(join(tmpdir(),"aeon-memory-trigger-"));try{const m=new CheckpointManager(d);const cp=await m.read();Object.assign(cp,c.cp);await m.write(cp);if(c.scene){mkdirSync(join(d,"scene_blocks"),{recursive:true});writeFileSync(join(d,"scene_blocks","a.md"),"body");}if(c.persona!==null)writeFileSync(join(d,"persona.md"),c.persona);out.push({case:c,result:await new PersonaTrigger({dataDir:d,interval:5}).shouldGenerate()});}finally{rmSync(d,{recursive:true,force:true});}}
writeFileSync(process.env.AEON_MEMORY_ORACLE_OUTPUT||new URL("./persona_trigger_oracle.json",import.meta.url).pathname,JSON.stringify(out,null,2)+"\n");
