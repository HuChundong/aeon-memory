#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE required");
const sha = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
if (sha !== "4339e63650920871eb0e8888083a1779d114e3ae") throw new Error(`wrong baseline ${sha}`);
const output = resolve(process.env.AEON_MEMORY_ORACLE_OUTPUT ?? new URL("disabled_capture_oracle.json", import.meta.url).pathname);
const scratch = mkdtempSync(join(tmpdir(), "aeon-memory-disabled-capture-oracle-"));
const run = join(scratch, "run.mts");
const result = join(scratch, "result.json");

writeFileSync(run, `
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { TdaiGateway } from ${JSON.stringify(join(root, "src/gateway/server.ts"))};
import { parseConfig } from ${JSON.stringify(join(root, "src/config.ts"))};
const dataDir = mkdtempSync(join(tmpdir(), "aeon-memory-ts-disabled-capture-"));
const gateway = new TdaiGateway({server:{port:0,host:"127.0.0.1",corsOrigins:[]},data:{baseDir:dataDir},llm:{baseUrl:"http://127.0.0.1:1/v1",apiKey:"test",model:"test",maxTokens:1,timeoutMs:100},memory:parseConfig({extraction:{enabled:false},embedding:{provider:"none"}})});
try {
  await gateway.start();
  const port = (gateway as any).server.address().port;
  const base = Date.now()+10000;
  const firstMessages=[{role:"user",content:"user message one",timestamp:base+1},{role:"assistant",content:"assistant answer one",timestamp:base+2}];
  const response = await fetch("http://127.0.0.1:"+port+"/capture",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({user_content:"user message one",assistant_content:"assistant answer one",session_key:"disabled-extraction",messages:firstMessages})});
  const body = await response.json();
  const secondResponse = await fetch("http://127.0.0.1:"+port+"/capture",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({user_content:"user message two",assistant_content:"assistant answer two",session_key:"disabled-extraction",messages:[...firstMessages,{role:"user",content:"user message two",timestamp:base+3},{role:"assistant",content:"assistant answer two",timestamp:base+4}]})});
  const secondBody = await secondResponse.json();
  const checkpointPath = join(dataDir,".metadata/recall_checkpoint.json");
  const cp = JSON.parse(readFileSync(checkpointPath,"utf8"));
  const runner = cp.runner_states["disabled-extraction"];
  writeFileSync(${JSON.stringify(result)},JSON.stringify({status:response.status,body,secondStatus:secondResponse.status,secondBody,checkpointExists:existsSync(checkpointPath),totalProcessed:cp.total_processed,l0ConversationsCount:cp.l0_conversations_count,runnerState:{lastCapturedTimestampPositive:runner.last_captured_timestamp>0,lastL1Cursor:runner.last_l1_cursor},pipelineStateCount:Object.keys(cp.pipeline_states).length},null,2)+"\\n");
} finally { await gateway.stop(); rmSync(dataDir,{recursive:true,force:true}); }
`);

try {
  execFileSync(process.execPath, [join(root, "node_modules/tsx/dist/cli.mjs"), run], { cwd: root, stdio: "inherit" });
  writeFileSync(output, readFileSync(result));
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
