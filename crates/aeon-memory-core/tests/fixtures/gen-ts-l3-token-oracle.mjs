#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root=process.env.AEON_MEMORY_TS_BASELINE;
if(!root) throw new Error("AEON_MEMORY_TS_BASELINE is required");
const output=resolve(process.env.AEON_MEMORY_ORACLE_OUTPUT ?? new URL("l3_token_oracle.json",import.meta.url).pathname);
const temp=mkdtempSync(join(tmpdir(),"aeon-memory-l3-token-"));
const runner=join(temp,"runner.ts"), result=join(temp,"result.json");
writeFileSync(runner, `
import { writeFileSync } from "node:fs";
import { buildTiktokenContextSnapshot, tiktokenCount } from ${JSON.stringify(join(root,"src/offload/context-token-tracker.ts"))};
let seed=0x13c0ffee; const rnd=()=>((seed=(Math.imul(seed,1664525)+1013904223)>>>0)/2**32);
const alphabet=["a","Z"," ","\\n","中","文","🙂","_","{","}","é"];
const strings=["", "hello", "你好，世界", "emoji🙂 mixed 文本", "<|endoftext|>", "x<|endofprompt|>🙂y"];
for(let i=0;i<64;i++){let s="";for(let j=0,n=Math.floor(rnd()*80);j<n;j++)s+=alphabet[Math.floor(rnd()*alphabet.length)];strings.push(s)}
const cases=[
 {stage:"empty",messages:[],system:null,user:null},
 {stage:"dedupe",messages:[{role:"user",content:" same "}],system:"sys",user:"same"},
 {stage:"blocks",messages:[{type:"message",message:{role:"user",content:[{type:"text",text:"甲"},{type:"image",url:"x"},{type:"text",text:"乙"}]}},{role:"assistant",content:"ok",details:"hidden",_offloaded:true}],system:"系统🙂",user:"甲\\n乙"},
 {stage:"separate",messages:[{role:"assistant",content:"answer",_mmdInjection:{huge:"ignored"}}],system:"",user:"new prompt"},
];
const snapshots=cases.map(c=>{const x=buildTiktokenContextSnapshot(c.stage,c.messages,c.system,c.user); delete x.timestamp; return {input:c,output:x}});
writeFileSync(${JSON.stringify(result)},JSON.stringify({strings:strings.map(text=>({text,count:tiktokenCount(text)})),snapshots},null,2)+"\\n");
`);
try { execFileSync(process.execPath,[process.env.AEON_MEMORY_TSX_CLI ?? join(root,"node_modules/tsx/dist/cli.mjs"),runner],{cwd:root,stdio:"inherit"}); writeFileSync(output,readFileSync(result)); }
finally { rmSync(temp,{recursive:true,force:true}); }
