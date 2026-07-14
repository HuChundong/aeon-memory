#!/usr/bin/env node
/** Generate compatibility goldens by executing the real TypeScript functions. */
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const here = dirname(fileURLToPath(import.meta.url));
const rustRoot = join(here, "..", "..", "..", "..");
const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to a checked-out TypeScript baseline");
const expectedBaseline = "4339e63650920871eb0e8888083a1779d114e3ae";
const actualBaseline = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
if (actualBaseline !== expectedBaseline) throw new Error(`TypeScript baseline mismatch: expected ${expectedBaseline}, got ${actualBaseline}`);
if (!existsSync(join(root, "node_modules/@node-rs/jieba"))) {
  throw new Error(`TypeScript oracle dependencies are missing at ${root}/node_modules; refusing to generate fallback-mode goldens`);
}
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-golden-"));
const runner = join(temp, "generate.ts");
const q = JSON.stringify;
const resources = join(rustRoot, "crates/aeon-memory-core/src/prompt/resources");
mkdirSync(resources, { recursive: true });

writeFileSync(runner, `
import { writeFileSync } from "node:fs";
import { rrfMerge } from ${q(join(root, "src/core/store/search-utils.ts"))};
import { bm25RankToScore, buildFtsQuery } from ${q(join(root, "src/core/store/sqlite.ts"))};
import { formatExtractionPrompt, EXTRACT_MEMORIES_SYSTEM_PROMPT } from ${q(join(root, "src/core/prompts/l1-extraction.ts"))};
import { formatBatchConflictPrompt, CONFLICT_DETECTION_SYSTEM_PROMPT } from ${q(join(root, "src/core/prompts/l1-dedup.ts"))};
import { buildSceneExtractionPrompt } from ${q(join(root, "src/core/prompts/scene-extraction.ts"))};
import { buildPersonaPrompt } from ${q(join(root, "src/core/prompts/persona-generation.ts"))};
const out = ${q(here)};
const resources = ${q(resources)};
const lists = [[{id:"a",score:.9},{id:"b",score:.8},{id:"d",score:.7}],[{id:"b",score:.7},{id:"c",score:.6},{id:"a",score:.5}]];
writeFileSync(out+"/rrf_merge.json", JSON.stringify({lists,k:60,expected:rrfMerge(lists,x=>x.id,60)},null,2));
const ranks=[-5,10,0,Number.POSITIVE_INFINITY,Number.NEGATIVE_INFINITY,Number.NaN];
writeFileSync(out+"/bm25_rank.json", JSON.stringify({cases:ranks.map(rank=>({rank:Number.isFinite(rank)?rank:String(rank),expected:bm25RankToScore(rank)}))},null,2));
const inputs=["hello world","用户喜欢编程和 TypeScript","旅行计划 API", "quoted \\\"term\\\"", "的 了 和"];
writeFileSync(out+"/fts_query.json",JSON.stringify({cases:inputs.map(input=>({input,expected:buildFtsQuery(input)}))},null,2));
const l1={id:"golden_fixture_1",content:"test memory for format verification",type:"persona",priority:50,scene_name:"test-scene",source_message_ids:["msg_001"],metadata:{nested:true},timestamps:["2026-07-13"],created_at:"2026-07-13T00:00:00.000Z",updated_at:"2026-07-13T00:00:01.000Z",session_key:"golden-session",session_id:"sid"};
const wire={id:l1.id,content:l1.content,type:l1.type,priority:l1.priority,scene_name:l1.scene_name,source_message_ids:l1.source_message_ids,metadata:l1.metadata,timestamps:l1.timestamps,createdAt:l1.created_at,updatedAt:l1.updated_at,sessionKey:l1.session_key,sessionId:l1.session_id};
writeFileSync(out+"/l1_jsonl_record.json",JSON.stringify({input:l1,expectedValue:wire,expectedBytes:JSON.stringify(wire)+"\\n"},null,2));
writeFileSync(out+"/prompt_l1_extraction.txt",EXTRACT_MEMORIES_SYSTEM_PROMPT);
writeFileSync(out+"/prompt_l1_dedup.txt",CONFLICT_DETECTION_SYSTEM_PROMPT);
const extraction=formatExtractionPrompt({newMessages:[{id:"n1",role:"user",content:"明天去杭州",timestamp:1783900800000}],backgroundMessages:[{id:"b1",role:"assistant",content:"背景",timestamp:1783814400000}],previousSceneName:"旅行"});
writeFileSync(out+"/prompt_l1_extraction_dynamic.txt",extraction);
const dedup=formatBatchConflictPrompt([{newMemory:{record_id:"new1",content:"用户喜欢 Rust",type:"persona",priority:80,scene_name:"编程",timestamps:["2026-07-13"],metadata:{}},candidates:[{id:"old1",content:"用户喜欢编程",type:"persona",priority:70,scene_name:"编程",timestamps:["2026-07-12"],metadata:{},created_at:"",updated_at:"",source_message_ids:[],session_key:"",session_id:""}]}]);
writeFileSync(out+"/prompt_l1_dedup_dynamic.txt",dedup);
const scene=buildSceneExtractionPrompt({memoriesJson:'[{"id":"m1"}]',sceneSummaries:"summary",currentTimestamp:"2026-07-13 12:00:00",sceneCountWarning:"warning",existingSceneFiles:["coding.md"],maxScenes:15});
writeFileSync(out+"/prompt_scene_system_15.txt",scene.systemPrompt);
const sceneTemplate=buildSceneExtractionPrompt({memoriesJson:"",sceneSummaries:"",currentTimestamp:"",existingSceneFiles:[],maxScenes:987654});
writeFileSync(resources+"/scene_extraction_template.txt",sceneTemplate.systemPrompt);
writeFileSync(out+"/prompt_scene_dynamic.txt",scene.userPrompt);
const persona=buildPersonaPrompt({mode:"incremental",currentTime:"2026-07-13 12:00:00",totalProcessed:12,sceneCount:2,changedSceneCount:1,changedScenesContent:"changed",existingPersona:"old",triggerInfo:"reason",personaFilePath:"",checkpointPath:""});
writeFileSync(out+"/prompt_persona_system.txt",persona.systemPrompt);
writeFileSync(resources+"/persona_generation.txt",persona.systemPrompt);
writeFileSync(out+"/prompt_persona_dynamic.txt",persona.userPrompt);
`);

try {
  execFileSync("npx", ["--yes", "tsx", runner], { cwd: root, env: { ...process.env, TZ: "Asia/Shanghai" }, stdio: "inherit" });
} finally {
  rmSync(temp, { recursive: true, force: true });
}
console.log("Generated goldens by executing TypeScript runtime functions");
