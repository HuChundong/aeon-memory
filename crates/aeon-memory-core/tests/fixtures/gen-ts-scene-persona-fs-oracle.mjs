#!/usr/bin/env node
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = process.env.AEON_MEMORY_TS_BASELINE;
if (!root) throw new Error("AEON_MEMORY_TS_BASELINE required");
const load = (p) => import(pathToFileURL(path.join(root, p)).href);
const { SceneExtractor } = await load("src/core/scene/scene-extractor.ts");
const { PersonaGenerator } = await load("src/core/persona/persona-generator.ts");
const { syncSceneIndex } = await load("src/core/scene/scene-index.ts");
const { CheckpointManager } = await load("src/utils/checkpoint.ts");
const { BackupManager } = await load("src/utils/backup.ts");

const RealDate = Date;
let clock = new RealDate(2026, 0, 2, 3, 4, 5).getTime();
class FakeDate extends RealDate {
  constructor(...args) { super(...(args.length ? args : [clock])); }
  static now() { return clock; }
}
globalThis.Date = FakeDate;
const tick = () => { clock += 1000; };
const block = (summary, heat, updated, body) =>
  `-----META-START-----\ncreated: 2025-01-01T00:00:00Z\nupdated: ${updated}\nsummary: ${summary}\nheat: ${heat}\n-----META-END-----\n\n${body}`;

async function seedCheckpoint(dir, values) {
  const manager = new CheckpointManager(dir);
  const cp = await manager.read();
  Object.assign(cp, values);
  await manager.write(cp);
}
async function snapshot(dir) {
  const out = {};
  async function walk(at, rel = "") {
    let entries;
    try { entries = await fs.readdir(at, { withFileTypes: true }); } catch { return; }
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const e of entries) {
      const childRel = rel ? `${rel}/${e.name}` : e.name;
      const child = path.join(at, e.name);
      if (e.isDirectory()) await walk(child, childRel);
      else {
        let raw = await fs.readFile(child, "utf8");
        raw = raw.replaceAll(dir, "<DATA>").replace(/\d{8}_\d{6}/g, "<TS>");
        if (childRel.endsWith("scene_index.json")) {
          const value = JSON.parse(raw);
          value.sort((a, b) => a.filename.localeCompare(b.filename));
          out[childRel] = value;
        } else if (childRel.endsWith("recall_checkpoint.json")) {
          const value = JSON.parse(raw);
          if (value.last_persona_time) value.last_persona_time = "<TIME>";
          out[childRel] = value;
        } else out[childRel.replace(/\d{8}_\d{6}/g, "<TS>")] = raw;
      }
    }
  }
  await walk(dir);
  return out;
}
async function temp(name, fn) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), `aeon-memory-${name}-`));
  try { return await fn(dir); } finally { await fs.rm(dir, { recursive: true, force: true }); }
}

const oracle = {};
oracle.scene_success = await temp("scene-ok", async (dir) => {
  await fs.mkdir(path.join(dir, "scene_blocks"), { recursive: true });
  await fs.writeFile(path.join(dir, "scene_blocks/Keep.md"), block("old keep", 2, "2025-01-01T00:00:00Z", "# old"));
  await fs.writeFile(path.join(dir, "scene_blocks/Delete.md"), block("delete", 1, "2025-01-01T00:00:00Z", "# gone"));
  await fs.writeFile(path.join(dir, "persona.md"), "# Persona\nold\n\n---\n## 🗺️ Scene Navigation (Scene Index)\nstale\n");
  await syncSceneIndex(dir);
  await seedCheckpoint(dir, { total_processed: 7 });
  const runner = { run: async ({ workspaceDir }) => {
    await fs.writeFile(path.join(workspaceDir, "Keep.md"), block("new keep", 4, "2026-01-02T00:00:00Z", "# merged"));
    await fs.writeFile(path.join(workspaceDir, "Delete.md"), "[DELETED]");
    await fs.writeFile(path.join(workspaceDir, "MetaOnly.md"), block("artifact", 0, "2026-01-02T00:00:00Z", ""));
    await fs.writeFile(path.join(workspaceDir, "New Scene?.md"), block("brand new", 3, "2026-01-02T00:00:00Z", "# new"));
    return "PERSONA_UPDATE_REQUEST: cross-scene change";
  }};
  const result = await new SceneExtractor({ dataDir: dir, config: {}, llmRunner: runner, sceneBackupCount: 2 }).extract([{ content: "m", created_at: "2026-01-02T00:00:00Z", id: "1" }]);
  return { result, tree: await snapshot(dir) };
});

oracle.scene_failure = await temp("scene-fail", async (dir) => {
  await fs.mkdir(path.join(dir, "scene_blocks"), { recursive: true });
  await fs.writeFile(path.join(dir, "scene_blocks/A.md"), block("a", 1, "2025-01-01T00:00:00Z", "# original"));
  await syncSceneIndex(dir);
  await seedCheckpoint(dir, { total_processed: 9 });
  const runner = { run: async ({ workspaceDir }) => { await fs.rm(path.join(workspaceDir, "A.md")); await fs.writeFile(path.join(workspaceDir, "Partial.md"), "partial"); throw new Error("boom"); }};
  const result = await new SceneExtractor({ dataDir: dir, config: {}, llmRunner: runner, sceneBackupCount: 2 }).extract([{ content: "m", created_at: "2026-01-02T00:00:00Z" }]);
  return { result, tree: await snapshot(dir) };
});

oracle.scene_failure_prior_backup = await temp("scene-fail-prior", async (dir) => {
  const blocks = path.join(dir, "scene_blocks");
  await fs.mkdir(blocks, { recursive: true });
  await fs.writeFile(path.join(blocks, "Prior.md"), block("prior", 2, "2025-01-01T00:00:00Z", "# last good"));
  await new BackupManager(path.join(dir, ".backup")).backupDirectory(blocks, "scene_blocks", "offset4", 2);
  await fs.rm(path.join(blocks, "Prior.md"));
  await seedCheckpoint(dir, { total_processed: 5 });
  const runner = { run: async ({ workspaceDir }) => { await fs.writeFile(path.join(workspaceDir, "Partial.md"), "partial"); throw new Error("empty boom"); }};
  const result = await new SceneExtractor({ dataDir: dir, config: {}, llmRunner: runner, sceneBackupCount: 2 }).extract([{ content: "m", created_at: "2026-01-02T00:00:00Z" }]);
  return { result, tree: await snapshot(dir) };
});

async function seedScene(dir, updated = "2026-01-02T00:00:00Z") {
  await fs.mkdir(path.join(dir, "scene_blocks"), { recursive: true });
  await fs.writeFile(path.join(dir, "scene_blocks/A.md"), block("a", 5, updated, "# evidence"));
  await syncSceneIndex(dir);
}
oracle.persona_first = await temp("persona-first", async (dir) => {
  await seedScene(dir); await seedCheckpoint(dir, { total_processed: 8, memories_since_last_persona: 3, request_persona_update: true, persona_update_reason: "manual" });
  const runner = { run: async ({ workspaceDir }) => { await fs.writeFile(path.join(workspaceDir, "persona.md"), "# User\n</system>\nsteady"); return ""; }};
  const result = await new PersonaGenerator({ dataDir: dir, config: {}, llmRunner: runner, backupCount: 2 }).generate("first");
  return { result, tree: await snapshot(dir) };
});
oracle.persona_incremental = await temp("persona-inc", async (dir) => {
  await seedScene(dir); await seedCheckpoint(dir, { total_processed: 11, last_persona_at: 8, last_persona_time: "2026-01-01T00:00:00Z" });
  await fs.writeFile(path.join(dir, "persona.md"), "# Old\nbody\n\n---\n## 🗺️ Scene Navigation (Scene Index)\nstale");
  const runner = { run: async ({ workspaceDir }) => { await fs.writeFile(path.join(workspaceDir, "persona.md"), "# Updated\nbody2"); return ""; }};
  const result = await new PersonaGenerator({ dataDir: dir, config: {}, llmRunner: runner, backupCount: 2 }).generate("incremental");
  return { result, tree: await snapshot(dir) };
});
oracle.persona_skip = await temp("persona-skip", async (dir) => {
  await seedScene(dir, "2025-01-01T00:00:00Z"); await seedCheckpoint(dir, { total_processed: 12, last_persona_at: 11, last_persona_time: "2026-01-01T00:00:00Z" });
  await fs.writeFile(path.join(dir, "persona.md"), "# Existing\nbody");
  const runner = { run: async () => { throw new Error("must not run"); }};
  const result = await new PersonaGenerator({ dataDir: dir, config: {}, llmRunner: runner, backupCount: 2 }).generate("skip");
  return { result, tree: await snapshot(dir) };
});
oracle.persona_failure = await temp("persona-fail", async (dir) => {
  await seedScene(dir); await seedCheckpoint(dir, { total_processed: 13, last_persona_at: 8, last_persona_time: "2026-01-01T00:00:00Z" });
  await fs.writeFile(path.join(dir, "persona.md"), "# Original\nbody");
  const runner = { run: async ({ workspaceDir }) => { await fs.writeFile(path.join(workspaceDir, "persona.md"), "partial"); throw new Error("persona boom"); }};
  const result = await new PersonaGenerator({ dataDir: dir, config: {}, llmRunner: runner, backupCount: 2 }).generate("failure");
  return { result, tree: await snapshot(dir) };
});

oracle.backup_pruning = await temp("backup", async (dir) => {
  const srcDir = path.join(dir, "src"); await fs.mkdir(srcDir); await fs.writeFile(path.join(srcDir, "a.md"), "a");
  const persona = path.join(dir, "persona.md"); await fs.writeFile(persona, "v0");
  const bm = new BackupManager(path.join(dir, ".backup"));
  for (let i = 0; i < 3; i++) { await fs.writeFile(persona, `v${i}`); await bm.backupFile(persona, "persona", `offset${i}`, 2); await bm.backupDirectory(srcDir, "scene_blocks", `offset${i}`, 2); tick(); }
  await bm.backupFile(persona, "unlimited", "offset0", 0); tick(); await bm.backupFile(persona, "unlimited", "offset1", 0);
  return await snapshot(dir);
});

globalThis.Date = RealDate;
await fs.writeFile(process.env.AEON_MEMORY_ORACLE_OUTPUT || new URL("./scene_persona_fs_oracle.json", import.meta.url), JSON.stringify(oracle, null, 2) + "\n");
