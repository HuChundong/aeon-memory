#!/usr/bin/env node
/**
 * Exercise the pinned TypeScript VectorStore embedding lifecycle and emit the
 * observations consumed by Rust's differential test. No external embedding
 * model is used: text is mapped to deterministic local vectors.
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to the pinned TypeScript checkout");
const tsx = process.env.AEON_MEMORY_TSX_CLI ?? join(root, "node_modules/tsx/dist/cli.mjs");
const output = resolve(process.env.AEON_MEMORY_TS_REINDEX_ORACLE_OUTPUT ?? join(here, "embedding_reindex_oracle.json"));
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-ts-reindex-oracle-"));
const runner = join(temp, "oracle.ts");
const db = join(temp, "vectors.db");
const rawOutput = join(temp, "oracle.json");

writeFileSync(runner, `
import { writeFileSync } from "node:fs";
import { VectorStore } from ${JSON.stringify(join(root, "src/core/store/sqlite.ts"))};

const dbPath = ${JSON.stringify(db)};
const outPath = ${JSON.stringify(rawOutput)};
const logger = { error() {}, warn() {}, info() {}, debug() {} };
const l1Records = [
  { id:"l1-alpha", content:"memory alpha", type:"fact", priority:80, scene_name:"", source_message_ids:["m1"], metadata:{}, timestamps:["2026-07-13"], createdAt:"2026-07-13T01:00:00.000Z", updatedAt:"2026-07-13T01:00:00.000Z", sessionKey:"session", sessionId:"sid" },
  { id:"l1-beta", content:"memory beta", type:"fact", priority:70, scene_name:"", source_message_ids:["m2"], metadata:{}, timestamps:["2026-07-13"], createdAt:"2026-07-13T01:00:01.000Z", updatedAt:"2026-07-13T01:00:01.000Z", sessionKey:"session", sessionId:"sid" },
];
const l0Records = [
  { id:"l0-alpha", sessionKey:"session", sessionId:"sid", role:"user", messageText:"raw alpha", recordedAt:"2026-07-13T01:00:02.000Z", timestamp:1783904402000 },
  { id:"l0-beta", sessionKey:"session", sessionId:"sid", role:"assistant", messageText:"raw beta", recordedAt:"2026-07-13T01:00:03.000Z", timestamp:1783904403000 },
];

function vector(text:string, dimensions:number): Float32Array {
  const values = new Float32Array(dimensions);
  if (text.endsWith("alpha")) { values[0] = 1; values[1] = text.startsWith("raw") ? 0.25 : 0; }
  else { values[0] = text.startsWith("raw") ? 0.25 : 0; values[1] = 1; }
  return values;
}
function initView(value:{needsReindex:boolean,reason?:string}) {
  return { needsReindex:value.needsReindex, reason:value.reason ?? null };
}
function ids(rows:Array<{record_id:string}>) { return rows.map(row => row.record_id); }
function storedVectors(store:any, table:string) {
  return store.db.prepare(
    \`SELECT record_id, vec_to_json(embedding) AS embedding FROM \${table} ORDER BY record_id\`,
  ).all().map((row:any) => ({
    id:row.record_id,
    vector:JSON.parse(row.embedding).map((value:number) => value.toFixed(6)),
  }));
}
function inspect(store:any, dimensions:number) {
  return {
    vecCounts: {
      l1: store.db.prepare("SELECT count(*) AS n FROM l1_vec").get().n,
      l0: store.db.prepare("SELECT count(*) AS n FROM l0_vec").get().n,
    },
    search: {
      l1: ids(store.searchL1Vector(vector("memory alpha", dimensions), 10)),
      l0: ids(store.searchL0Vector(vector("memory alpha", dimensions), 10)),
    },
    vectors: {
      l1: storedVectors(store, "l1_vec"),
      l0: storedVectors(store, "l0_vec"),
    },
  };
}
async function runReindex(store:any, dimensions:number, failText?:string) {
  const calls:string[] = [];
  const progress:Array<{done:number,total:number,layer:string}> = [];
  const result = await store.reindexAll(async (text:string) => {
    calls.push(text);
    if (text === failText) throw new Error("deterministic embedding failure");
    return vector(text, dimensions);
  }, (done:number,total:number,layer:string) => progress.push({done,total,layer}));
  return { result, calls, progress, ...inspect(store, dimensions) };
}

async function main() {
const oracle:any = { baseline:${JSON.stringify("4339e63650920871eb0e8888083a1779d114e3ae")} };

let store = new VectorStore(dbPath, 0, logger);
oracle.noneInit = initView(store.init());
for (const row of l1Records) store.upsertL1(row, undefined);
for (const row of l0Records) store.upsertL0(row, undefined);
oracle.noneCounts = { l1:store.countL1(), l0:store.countL0() };
store.close();

store = new VectorStore(dbPath, 4, logger);
oracle.enableInit = initView(store.init({provider:"provider-a", model:"model-a", dimensions:4}));
oracle.enableReindex = await runReindex(store, 4, "memory beta");
store.close();

store = new VectorStore(dbPath, 4, logger);
oracle.sameConfigInit = initView(store.init({provider:"provider-a", model:"model-a", dimensions:4}));
oracle.sameConfigState = inspect(store, 4);
store.close();

store = new VectorStore(dbPath, 4, logger);
oracle.providerChangeInit = initView(store.init({provider:"provider-b", model:"model-a", dimensions:4}));
oracle.providerChangeReindex = await runReindex(store, 4);
store.close();

store = new VectorStore(dbPath, 4, logger);
oracle.modelChangeInit = initView(store.init({provider:"provider-b", model:"model-b", dimensions:4}));
oracle.modelChangeReindex = await runReindex(store, 4);
store.close();

store = new VectorStore(dbPath, 6, logger);
oracle.dimensionsChangeInit = initView(store.init({provider:"provider-b", model:"model-b", dimensions:6}));
oracle.dimensionsChangeReindex = await runReindex(store, 6);
store.close();

writeFileSync(outPath, JSON.stringify(oracle, null, 2) + "\\n");
}
main().catch(error => { console.error(error); process.exitCode = 1; });
`);

try {
  execFileSync(process.execPath, [tsx, runner], { cwd: root, stdio: "inherit" });
  writeFileSync(output, readFileSync(rawOutput));
} finally {
  rmSync(temp, { recursive: true, force: true });
}
console.log(`Generated deterministic TS embedding/reindex oracle at ${output}`);
