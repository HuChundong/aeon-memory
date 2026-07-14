#!/usr/bin/env node
/** Build the fixture through the real TypeScript VectorStore runtime. */
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to the fixed TypeScript checkout");
const tsx = process.env.AEON_MEMORY_TSX_CLI ?? join(root, "node_modules/tsx/dist/cli.mjs");
const db = resolve(process.env.AEON_MEMORY_TS_DB_OUTPUT ?? join(here, "vectors-ts.db"));
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-ts-store-"));
const runner = join(temp, "generate.ts");
for (const suffix of ["", "-wal", "-shm"]) rmSync(db + suffix, { force: true });
writeFileSync(runner, `
import { VectorStore } from ${JSON.stringify(join(root, "src/core/store/sqlite.ts"))};
const dbPath=${JSON.stringify(db)};
const store=new VectorStore(dbPath,4,{error:console.error,warn:console.warn,info:console.log,debug:console.log});
const initialized=store.init({provider:"test",model:"test-model",dimensions:4});
if(store.isDegraded()) throw new Error("TypeScript VectorStore degraded; sqlite-vec runtime is required");
const l0a=store.upsertL0({id:"l0_compat_001",sessionKey:"session-compat",sessionId:"sid-1",role:"user",messageText:"Hello from TS generator",recordedAt:"2026-07-13T12:00:00.000Z",timestamp:1781836800000},new Float32Array([1,0,0,0]));
const l0b=store.upsertL0({id:"l0_compat_002",sessionKey:"session-compat",sessionId:"sid-1",role:"assistant",messageText:"Hi! This is a compatibility test.",recordedAt:"2026-07-13T12:00:01.000Z",timestamp:1781836801000},new Float32Array([0,1,0,0]));
const l1=store.upsertL1({id:"l1_compat_001",content:"test user memory for cross-compat verification",type:"persona",priority:50,scene_name:"test-scene",source_message_ids:["msg-1"],metadata:{},timestamps:["2026-07-13"],createdAt:"2026-07-13T12:00:00.000Z",updatedAt:"2026-07-13T12:00:00.000Z",sessionKey:"session-compat",sessionId:"sid-1"},new Float32Array([.5,.5,.5,.5]));
if(!l0a||!l0b||!l1) throw new Error("real TypeScript store write failed");
store.close();
console.log(JSON.stringify(initialized));
`);
try {
  execFileSync(process.execPath,[tsx,runner],{cwd:root,stdio:"inherit"});
} finally {
  rmSync(temp,{recursive:true,force:true});
}
console.log(`Generated ${db} through VectorStore.init/upsertL0/upsertL1`);
