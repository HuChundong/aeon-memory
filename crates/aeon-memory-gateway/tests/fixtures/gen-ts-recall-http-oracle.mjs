#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE required");
const output = resolve(process.env.AEON_MEMORY_ORACLE_OUTPUT ?? new URL("recall_http_oracle.json", import.meta.url).pathname);
const scratch = mkdtempSync(join(tmpdir(), "aeon-memory-recall-http-oracle-"));
const run = join(scratch, "run.mts");
const result = join(scratch, "result.json");

writeFileSync(run, `
import { writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { TdaiGateway } from ${JSON.stringify(join(root, "src/gateway/server.ts"))};
import { parseConfig } from ${JSON.stringify(join(root, "src/config.ts"))};

(async () => {
  const dataDir = mkdtempSync(join(tmpdir(), "aeon-memory-ts-recall-http-"));
  const gateway = new TdaiGateway({
    server: {port: 0, host: "127.0.0.1", corsOrigins: []},
    data: {baseDir: dataDir},
    llm: {baseUrl: "http://127.0.0.1:1/v1", apiKey: "test", model: "test", maxTokens: 4096, timeoutMs: 1000},
    memory: parseConfig({extraction: {enabled: false}, embedding: {provider: "none"}}),
  });
  let recalled: any = {};
  (gateway as any).core.handleBeforeRecall = async () => recalled;
  try {
    await gateway.start();
    const port = (gateway as any).server.address().port;
    const invoke = async (value: any) => {
      recalled = value;
      const response = await fetch("http://127.0.0.1:" + port + "/recall", {
        method: "POST", headers: {"content-type":"application/json"},
        body: JSON.stringify({query:"q", session_key:"s"}),
      });
      const text = await response.text();
      return {status: response.status, text, body: JSON.parse(text)};
    };
    const empty = await invoke({});
    const split = await invoke({
      prependContext:"dynamic L1 must stay private",
      appendSystemContext:"stable official context",
      recalledL1Memories:[{content:"m",score:0,type:"episodic"}],
      recalledL3Persona:"persona",
      recallStrategy:"hybrid",
    });
    writeFileSync(${JSON.stringify(result)}, JSON.stringify({empty, split}, null, 2) + "\\n");
  } finally {
    await gateway.stop();
    rmSync(dataDir, {recursive:true, force:true});
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
`);

try {
  execFileSync(process.execPath, [process.env.AEON_MEMORY_TSX_CLI ?? join(root, "node_modules/tsx/dist/cli.mjs"), run], {cwd: root, stdio: "inherit"});
  writeFileSync(output, readFileSync(result));
} finally {
  rmSync(scratch, {recursive: true, force: true});
}
