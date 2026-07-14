#!/usr/bin/env node
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE is required");
const { extractJson, extractMermaidFromFence } = await import(pathToFileURL(join(root, "src/offload/local-llm/parsers/json-utils.ts")));
const { parseL1Response } = await import(pathToFileURL(join(root, "src/offload/local-llm/parsers/l1-parser.ts")));
const { parseL15Response } = await import(pathToFileURL(join(root, "src/offload/local-llm/parsers/l15-parser.ts")));
const { parseL2Response } = await import(pathToFileURL(join(root, "src/offload/local-llm/parsers/l2-parser.ts")));

const cases = [
  "", "not json", "prefix {\"a\":1,} suffix", "```json\n[1,2,]\n```",
  "```mermaid\nflowchart TD\n A-->B\n```",
  JSON.stringify([{tool_call_id:"x",timestamp:"t",tool_call:"shell",summary:"done",score:9},{tool_call_id:"",summary:"drop"},{tool_call_id:"y"}]),
  JSON.stringify({taskCompleted:true,isContinuation:false,isLongTask:true,continuationMmdFile:"a.mmd",newTaskLabel:"task"}),
  JSON.stringify({taskCompleted:null,isContinuation:null,isLongTask:null}),
  JSON.stringify({file_action:"replace",replace_blocks:[{start_line:"2",end_line:4,content:"```mermaid\ngraph TD\n A-->B\n```"}],node_mapping:{a:"b",bad:3}}),
  JSON.stringify({file_action:"create",mmd_content:"flowchart TD\n A-->B"}),
];
const normalize = value => value === undefined ? null : value;
const output = cases.map(raw => ({
  raw,
  json: normalize(extractJson(raw)),
  mermaid: normalize(extractMermaidFromFence(raw)),
  l1: parseL1Response(raw),
  l15: normalize(parseL15Response(raw)),
  l2: normalize(parseL2Response(raw)),
}));
const target = process.env.AEON_MEMORY_ORACLE_OUTPUT || new URL("./offload_parser_oracle.json", import.meta.url).pathname;
writeFileSync(target, JSON.stringify(output, null, 2) + "\n");
