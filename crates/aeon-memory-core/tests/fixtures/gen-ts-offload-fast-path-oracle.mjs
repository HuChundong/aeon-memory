#!/usr/bin/env node
/** Execute the pinned TypeScript before_prompt_build fast path. */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to the pinned TypeScript checkout");
const tsx = process.env.AEON_MEMORY_TSX_CLI ?? join(root, "node_modules/tsx/dist/cli.mjs");
const output = resolve(process.env.AEON_MEMORY_TS_OFFLOAD_FAST_PATH_OUTPUT ?? join(here, "offload_fast_path_oracle.json"));
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-ts-offload-fast-path-"));
const runner = join(temp, "oracle.ts");
const rawOutput = join(temp, "oracle.json");

writeFileSync(runner, `
import { writeFileSync } from "node:fs";
import { OffloadStateManager } from ${JSON.stringify(join(root, "src/offload/state-manager.ts"))};
import { createBeforePromptBuildHandler } from ${JSON.stringify(join(root, "src/offload/hooks/before-prompt-build.ts"))};
import { createStorageContext, ensureDirs, rewriteOffloadEntries } from ${JSON.stringify(join(root, "src/offload/storage.ts"))};

async function main() {
  const dataRoot = ${JSON.stringify(join(temp, "data"))};
  const ctx = createStorageContext(dataRoot, "agent", "session");
  await ensureDirs(ctx);
  await rewriteOffloadEntries(ctx, [
    {timestamp:"2026-07-13T00:00:00Z",node_id:"N1",tool_call:"read({})",summary:"summary",result_ref:"refs/a.md",tool_call_id:"tool_u_1",score:9,offloaded:true},
    {timestamp:"2026-07-13T00:00:00Z",node_id:"N2",tool_call:"read({})",summary:"summary",result_ref:"refs/a.md",tool_call_id:"gone_2",score:9,offloaded:"deleted"},
  ] as any);
  const manager = new OffloadStateManager();
  await manager.switchSession("agent:agent:session", dataRoot, "session");
  const messages:any[] = [
    {role:"assistant",content:[{type:"tool_use",id:"toolu1",input:{path:"raw"}}]},
    {role:"tool",toolCallId:"toolu1",content:"raw confirmed result"},
    {role:"assistant",content:[
      {type:"text",text:"keep this text"},
      {type:"tool_use",id:"gone2",input:{path:"delete"}},
      {type:"tool_use",id:"toolu1",input:{path:"compress"}},
    ]},
    {role:"assistant",content:[{type:"tool_use",id:"gone2",input:{}}]},
    {role:"tool",toolCallId:"gone2",content:"raw deleted result"},
    {role:"user",content:"latest request"},
  ];
  const handler = createBeforePromptBuildHandler(
    manager,
    {error(){},warn(){},info(){},debug(){}},
    () => 200000,
    {},
  );
  await handler({messages}, {sessionKey:"agent:agent:session"});
  writeFileSync(${JSON.stringify(rawOutput)}, JSON.stringify({
    baseline:${JSON.stringify("4339e63650920871eb0e8888083a1779d114e3ae")},
    confirmed:[...manager.confirmedOffloadIds].sort(),
    deleted:[...manager.deletedOffloadIds].sort(),
    messages,
  }, null, 2) + "\\n");
}
main().catch(error => { console.error(error); process.exitCode = 1; });
`);

try {
  execFileSync(process.execPath, [tsx, runner], { cwd: root, stdio: "inherit" });
  writeFileSync(output, readFileSync(rawOutput));
} finally {
  rmSync(temp, { recursive: true, force: true });
}
console.log(`Generated TS offload fast-path oracle at ${output}`);
