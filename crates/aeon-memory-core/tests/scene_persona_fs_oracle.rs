use aeon_memory_core::{
    AeonMemoryCoreError, AeonMemoryResult,
    persona::PersonaGenerator,
    pipeline::checkpoint::{Checkpoint, write_checkpoint},
    scene::{SceneExtractor, SceneMemory, sync_scene_index},
    types::{LlmRunParams, LlmRunner},
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/scene_persona_fs_oracle.json")).unwrap()
}
fn block(summary: &str, heat: i64, updated: &str, body: &str) -> String {
    format!(
        "-----META-START-----\ncreated: 2025-01-01T00:00:00Z\nupdated: {updated}\nsummary: {summary}\nheat: {heat}\n-----META-END-----\n\n{body}"
    )
}
fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "aeon-memory-scene-persona-oracle-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn normalize_timestamp(s: &str) -> String {
    let b = s.as_bytes();
    for i in 0..b.len().saturating_sub(14) {
        if b[i..i + 8].iter().all(u8::is_ascii_digit)
            && b[i + 8] == b'_'
            && b[i + 9..i + 15].iter().all(u8::is_ascii_digit)
        {
            let prefix = &s[..i];
            let suffix = &s[i + 15..];
            return format!("{}<TS>{}", prefix, suffix);
        }
    }
    s.to_string()
}
fn snapshot(root: &Path) -> Value {
    fn walk(root: &Path, at: &Path, out: &mut Map<String, Value>) {
        let mut entries = std::fs::read_dir(at)
            .into_iter()
            .flatten()
            .flatten()
            .collect::<Vec<_>>();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let p = entry.path();
            if p.is_dir() {
                walk(root, &p, out);
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let key = normalize_timestamp(&rel);
            let mut raw = std::fs::read_to_string(&p).unwrap();
            raw = raw.replace(&root.to_string_lossy().to_string(), "<DATA>");
            raw = normalize_timestamp(&raw);
            if rel.ends_with("scene_index.json") {
                let mut v: Value = serde_json::from_str(&raw).unwrap();
                v.as_array_mut()
                    .unwrap()
                    .sort_by(|a, b| a["filename"].as_str().cmp(&b["filename"].as_str()));
                out.insert(key, v);
            } else if rel.ends_with("recall_checkpoint.json") {
                let mut v: Value = serde_json::from_str(&raw).unwrap();
                if !v["last_persona_time"].as_str().unwrap_or("").is_empty() {
                    v["last_persona_time"] = json!("<TIME>");
                }
                out.insert(key, v);
            } else {
                out.insert(key, json!(raw));
            }
        }
    }
    let mut out = Map::new();
    walk(root, root, &mut out);
    Value::Object(out)
}
fn checkpoint(dir: &Path, cp: Checkpoint) {
    write_checkpoint(dir.to_str().unwrap(), &cp).unwrap();
}
fn seed_scene(dir: &Path, updated: &str) {
    std::fs::create_dir_all(dir.join("scene_blocks")).unwrap();
    std::fs::write(
        dir.join("scene_blocks/A.md"),
        block("a", 5, updated, "# evidence"),
    )
    .unwrap();
    sync_scene_index(dir).unwrap();
}

enum SceneAction {
    Success,
    Failure,
    FailureEmpty,
}
struct SceneRunner(SceneAction);
#[async_trait::async_trait]
impl LlmRunner for SceneRunner {
    async fn run(&self, p: LlmRunParams) -> AeonMemoryResult<String> {
        let d = PathBuf::from(p.workspace_dir.unwrap());
        match self.0 {
            SceneAction::Success => {
                std::fs::write(
                    d.join("Keep.md"),
                    block("new keep", 4, "2026-01-02T00:00:00Z", "# merged"),
                )?;
                std::fs::write(d.join("Delete.md"), "[DELETED]")?;
                std::fs::write(
                    d.join("MetaOnly.md"),
                    block("artifact", 0, "2026-01-02T00:00:00Z", ""),
                )?;
                std::fs::write(
                    d.join("New Scene!.md"),
                    block("brand new", 3, "2026-01-02T00:00:00Z", "# new"),
                )?;
                Ok("PERSONA_UPDATE_REQUEST: cross-scene change".into())
            }
            SceneAction::Failure => {
                std::fs::remove_file(d.join("A.md"))?;
                std::fs::write(d.join("Partial.md"), "partial")?;
                Err(AeonMemoryCoreError::Llm("boom".into()))
            }
            SceneAction::FailureEmpty => {
                std::fs::write(d.join("Partial.md"), "partial")?;
                Err(AeonMemoryCoreError::Llm("empty boom".into()))
            }
        }
    }
}
enum PersonaAction {
    Write(&'static str),
    Fail,
}
struct PersonaRunner(PersonaAction);
#[async_trait::async_trait]
impl LlmRunner for PersonaRunner {
    async fn run(&self, p: LlmRunParams) -> AeonMemoryResult<String> {
        let d = PathBuf::from(p.workspace_dir.unwrap());
        match self.0 {
            PersonaAction::Write(s) => {
                std::fs::write(d.join("persona.md"), s)?;
                Ok(String::new())
            }
            PersonaAction::Fail => {
                std::fs::write(d.join("persona.md"), "partial")?;
                Err(AeonMemoryCoreError::Llm("persona boom".into()))
            }
        }
    }
}

#[tokio::test]
async fn scene_success_and_failure_full_trees_match_typescript() {
    let o = oracle();
    let d = temp("scene-ok");
    std::fs::create_dir_all(d.join("scene_blocks")).unwrap();
    std::fs::write(
        d.join("scene_blocks/Keep.md"),
        block("old keep", 2, "2025-01-01T00:00:00Z", "# old"),
    )
    .unwrap();
    std::fs::write(
        d.join("scene_blocks/Delete.md"),
        block("delete", 1, "2025-01-01T00:00:00Z", "# gone"),
    )
    .unwrap();
    std::fs::write(
        d.join("persona.md"),
        "# Persona\nold\n\n---\n## 🗺️ Scene Navigation (Scene Index)\nstale\n",
    )
    .unwrap();
    sync_scene_index(&d).unwrap();
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 7,
            ..Default::default()
        },
    );
    let r = SceneExtractor {
        data_dir: d.clone(),
        runner: &SceneRunner(SceneAction::Success),
        max_scenes: 15,
        backup_count: 2,
        timeout_ms: 300_000,
    }
    .extract(&[SceneMemory {
        content: "m".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
        id: Some("1".into()),
    }])
    .await;
    assert_eq!(
        json!({"memoriesProcessed":r.memories_processed,"success":r.success}),
        o["scene_success"]["result"]
    );
    assert_eq!(snapshot(&d), o["scene_success"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);

    let d = temp("scene-fail");
    std::fs::create_dir_all(d.join("scene_blocks")).unwrap();
    std::fs::write(
        d.join("scene_blocks/A.md"),
        block("a", 1, "2025-01-01T00:00:00Z", "# original"),
    )
    .unwrap();
    sync_scene_index(&d).unwrap();
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 9,
            ..Default::default()
        },
    );
    let r = SceneExtractor {
        data_dir: d.clone(),
        runner: &SceneRunner(SceneAction::Failure),
        max_scenes: 15,
        backup_count: 2,
        timeout_ms: 300_000,
    }
    .extract(&[SceneMemory {
        content: "m".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
        id: None,
    }])
    .await;
    assert_eq!(
        json!({"memoriesProcessed":r.memories_processed,"success":r.success,"error":r.error}),
        o["scene_failure"]["result"]
    );
    assert_eq!(snapshot(&d), o["scene_failure"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);

    let d = temp("scene-fail-prior");
    std::fs::create_dir_all(d.join("scene_blocks")).unwrap();
    let prior = block("prior", 2, "2025-01-01T00:00:00Z", "# last good");
    let backup = d.join(".backup/scene_blocks/scene_blocks_20260102_030405_offset4");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("Prior.md"), prior).unwrap();
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 5,
            ..Default::default()
        },
    );
    let r = SceneExtractor {
        data_dir: d.clone(),
        runner: &SceneRunner(SceneAction::FailureEmpty),
        max_scenes: 15,
        backup_count: 2,
        timeout_ms: 300_000,
    }
    .extract(&[SceneMemory {
        content: "m".into(),
        created_at: "2026-01-02T00:00:00Z".into(),
        id: None,
    }])
    .await;
    assert_eq!(
        json!({"memoriesProcessed":r.memories_processed,"success":r.success,"error":r.error}),
        o["scene_failure_prior_backup"]["result"]
    );
    assert_eq!(snapshot(&d), o["scene_failure_prior_backup"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn persona_first_incremental_skip_and_failure_trees_match_typescript() {
    let o = oracle();
    let d = temp("persona-first");
    seed_scene(&d, "2026-01-02T00:00:00Z");
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 8,
            memories_since_last_persona: 3,
            request_persona_update: true,
            persona_update_reason: "manual".into(),
            ..Default::default()
        },
    );
    let runner = PersonaRunner(PersonaAction::Write("# User\n</system>\nsteady"));
    let r = PersonaGenerator {
        data_dir: d.clone(),
        runner: &runner,
        backup_count: 2,
    }
    .generate(Some("first"))
    .await
    .unwrap();
    assert_eq!(json!(r), o["persona_first"]["result"]);
    assert_eq!(snapshot(&d), o["persona_first"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);

    let d = temp("persona-inc");
    seed_scene(&d, "2026-01-02T00:00:00Z");
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 11,
            last_persona_at: 8,
            last_persona_time: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        },
    );
    std::fs::write(
        d.join("persona.md"),
        "# Old\nbody\n\n---\n## 🗺️ Scene Navigation (Scene Index)\nstale",
    )
    .unwrap();
    let runner = PersonaRunner(PersonaAction::Write("# Updated\nbody2"));
    let r = PersonaGenerator {
        data_dir: d.clone(),
        runner: &runner,
        backup_count: 2,
    }
    .generate(Some("incremental"))
    .await
    .unwrap();
    assert_eq!(json!(r), o["persona_incremental"]["result"]);
    assert_eq!(snapshot(&d), o["persona_incremental"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);

    let d = temp("persona-skip");
    seed_scene(&d, "2025-01-01T00:00:00Z");
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 12,
            last_persona_at: 11,
            last_persona_time: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        },
    );
    std::fs::write(d.join("persona.md"), "# Existing\nbody").unwrap();
    let runner = PersonaRunner(PersonaAction::Fail);
    let r = PersonaGenerator {
        data_dir: d.clone(),
        runner: &runner,
        backup_count: 2,
    }
    .generate(Some("skip"))
    .await
    .unwrap();
    assert_eq!(json!(r), o["persona_skip"]["result"]);
    assert_eq!(snapshot(&d), o["persona_skip"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);

    let d = temp("persona-fail");
    seed_scene(&d, "2026-01-02T00:00:00Z");
    checkpoint(
        &d,
        Checkpoint {
            total_processed: 13,
            last_persona_at: 8,
            last_persona_time: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        },
    );
    std::fs::write(d.join("persona.md"), "# Original\nbody").unwrap();
    let runner = PersonaRunner(PersonaAction::Fail);
    let r = PersonaGenerator {
        data_dir: d.clone(),
        runner: &runner,
        backup_count: 2,
    }
    .generate(Some("failure"))
    .await
    .unwrap();
    assert_eq!(json!(r), o["persona_failure"]["result"]);
    assert_eq!(snapshot(&d), o["persona_failure"]["tree"]);
    let _ = std::fs::remove_dir_all(&d);
}
