#!/usr/bin/env node
// Execute the immutable TypeScript repository as the compatibility oracle.
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tsRoot = process.env.AEON_MEMORY_TS_BASELINE;
if (!tsRoot) throw new Error("AEON_MEMORY_TS_BASELINE must point to the checked-out TypeScript baseline");
const tsxCli = process.env.AEON_MEMORY_TSX_CLI ?? join(tsRoot, "node_modules/tsx/dist/cli.mjs");
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-config-search-oracle-"));
const runner = join(temp, "oracle.ts");
const q = JSON.stringify;

writeFileSync(runner, `
import { writeFileSync } from "node:fs";
import { parseConfig } from ${q(join(tsRoot, "src/config.ts"))};
import { rrfMerge } from ${q(join(tsRoot, "src/core/store/search-utils.ts"))};
import { bm25RankToScore, buildFtsQuery, tokenizeForFts } from ${q(join(tsRoot, "src/core/store/sqlite.ts"))};
const output = ${q(process.env.AEON_MEMORY_ORACLE_OUTPUT ?? join(here, "config_search_oracle.json"))};
const configInputs = [
  {},
  {storeBackend:"unknown", recall:{strategy:"bogus"}},
  {capture:{l0l1RetentionDays:2,allowAggressiveCleanup:false,cleanTime:"25:99"}},
  {capture:{l0l1RetentionDays:2,allowAggressiveCleanup:true,cleanTime:"7:05"}},
  {embedding:{enabled:true,provider:"none",dimensions:1536,model:"ignored"}},
  {embedding:{enabled:true,provider:"local"}},
  {embedding:{enabled:true,provider:"openai",apiKey:"",baseUrl:"",model:"",dimensions:0}},
  {embedding:{enabled:true,provider:"qclaw",proxyUrl:"http://proxy",baseUrl:"http://embed",apiKey:"k",model:"m",dimensions:3}},
  {offload:{backendUrl:"http://backend",mode:"invalid",offloadRetentionDays:2}},
];
const selectConfig = (c:any) => ({
  timezone:c.timezone, capture:c.capture, recall:c.recall,
  embedding:{enabled:c.embedding.enabled,provider:c.embedding.provider,baseUrl:c.embedding.baseUrl,model:c.embedding.model,dimensions:c.embedding.dimensions,configError:c.embedding.configError??null},
  storeBackend:c.storeBackend, memoryCleanup:c.memoryCleanup,
  offload:{enabled:c.offload.enabled,mode:c.offload.mode,backendTimeoutMs:c.offload.backendTimeoutMs,offloadRetentionDays:c.offload.offloadRetentionDays},
});
let state=0x5eed1234;
const rnd=()=>{ state=(Math.imul(state,1664525)+1013904223)>>>0; return state; };
const ids=["a","b","c","d","e","甲","乙"];
const rrfCases=[];
for(let c=0;c<80;c++){
  const lists=[];
  const listCount=1+rnd()%4;
  for(let l=0;l<listCount;l++){
    const len=rnd()%8; const list=[];
    for(let i=0;i<len;i++) list.push({id:ids[rnd()%ids.length],payload:c*100+l*10+i});
    lists.push(list);
  }
  const k=[0,1,10,60,100][rnd()%5];
  rrfCases.push({lists,k,expected:rrfMerge(lists,(x:any)=>x.id,k)});
}
const ranks=[];
for(let i=-200;i<=200;i++) ranks.push(i/7);
ranks.push(-0,0,Number.MAX_VALUE,Number.MIN_VALUE,Infinity,-Infinity,NaN);
const ftsInputs=["", "   ", "的 了 和", "用户喜欢编程和 TypeScript", "人工智能的分支", "北京烤鸭", "quoted \\\"term\\\"", "snake_case API-2", "emoji😀中文", "１２３ abc"];
writeFileSync(output, JSON.stringify({
  configCases:configInputs.map(input=>({input,expected:selectConfig(parseConfig(input))})),
  rrfCases,
  bm25Cases:ranks.map(rank=>({rank:Number.isFinite(rank)?rank:String(rank),expected:bm25RankToScore(rank)})),
  ftsCases:ftsInputs.map(input=>({input,query:buildFtsQuery(input),indexed:tokenizeForFts(input)})),
}, null, 2));
`);

try {
  execFileSync(process.execPath, [tsxCli, runner], {
    cwd: tsRoot,
    env: {...process.env, TZ: "Asia/Shanghai"},
    stdio: "inherit",
  });
} finally {
  rmSync(temp, {recursive:true, force:true});
}
console.log(`generated TypeScript oracle from ${tsRoot}`);
