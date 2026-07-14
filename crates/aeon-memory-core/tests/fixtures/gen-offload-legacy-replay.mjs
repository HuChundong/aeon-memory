#!/usr/bin/env node
// Rebuilds the fixture by executing the current TypeScript oracle directly.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const rustRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim();
const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE must point to a checked-out TypeScript baseline");
const expectedBaseline = "4339e63650920871eb0e8888083a1779d114e3ae";
const actualBaseline = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
if (actualBaseline !== expectedBaseline) throw new Error(`TypeScript baseline mismatch: expected ${expectedBaseline}, got ${actualBaseline}`);
const temp = mkdtempSync(join(tmpdir(), "aeon-memory-offload-replay-"));
try {
  for (const name of ["l1-prompt", "l15-prompt", "l2-prompt"]) {
    const source = readFileSync(join(root, `src/offload/local-llm/prompts/${name}.ts`));
    writeFileSync(join(temp, `${name}.ts`), source);
  }
  execFileSync("npx", ["tsc", "--target", "ES2022", "--module", "NodeNext", "--moduleResolution", "NodeNext", "--outDir", join(temp, "out"), join(temp, "l1-prompt.ts"), join(temp, "l15-prompt.ts"), join(temp, "l2-prompt.ts")], { stdio: "inherit", cwd: root });
  const load = (name) => import(pathToFileURL(join(temp, "out", `${name}.js`)));
  const [l1, l15, l2] = await Promise.all([load("l1-prompt"), load("l15-prompt"), load("l2-prompt")]);
  const pair = { toolName: "shell", toolCallId: "call-1", params: { cmd: "ls" }, result: "file.txt", timestamp: "2026-07-13T00:00:00Z" };
  const current = { filename: "001-task.mmd", content: 'flowchart TD\n  N1["x"]', path: "/tmp/001-task.mmd" };
  const metas = [{ filename: "002-old.mmd", path: "/tmp/002-old.mmd", taskGoal: "old", doneCount: 1, doingCount: 1, todoCount: 0, updatedTime: "2026-07-12T00:00:00Z", nodeSummaries: [{ nodeId: "002-N1", status: "done", summary: "ok" }] }];
  const entry = { toolCallId: "call-1", toolCall: "shell", summary: "listed files", timestamp: "2026-07-13T00:00:00Z" };
  const replay = {
    l1System: l1.L1_SYSTEM_PROMPT,
    l1User: l1.buildL1UserPrompt("ctx", [pair]),
    l15System: l15.L15_SYSTEM_PROMPT,
    l15User: l15.buildL15UserPrompt("ctx", current, metas),
    l2System: l2.L2_SYSTEM_PROMPT,
    l2User: l2.buildL2UserPrompt({ existingMmd: 'flowchart TD\n  N1["x"]', entries: [entry], recentHistory: "ctx", currentTurn: "turn", taskLabel: "task", mmdPrefix: "001", charCount: 2100 }),
  };
  writeFileSync(join(rustRoot, "crates/aeon-memory-core/tests/fixtures/offload_prompt_legacy_replay.json"), `${JSON.stringify(replay, null, 2)}\n`);
} finally {
  rmSync(temp, { recursive: true, force: true });
}
