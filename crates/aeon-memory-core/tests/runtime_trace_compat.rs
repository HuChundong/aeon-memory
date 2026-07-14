//! Exact assertions against `gen-ts-runtime-trace.mjs`, which executes the
//! pinned TypeScript baseline. Regenerate with AEON_MEMORY_TS_BASELINE set.
use aeon_memory_core::pipeline::{
    checkpoint::PipelineSessionState,
    manager::{
        CapturedMessage, Clock, L2Result, PipelineConfig, PipelineManager, PipelineRunner,
        StatePersister,
    },
};
use aeon_memory_core::{
    offload::{inject, l3, storage},
    profile::{build_profile_stable_id, list_local_profiles},
    scene::{
        SceneIndexEntry, format_meta, format_scene_block, generate_scene_navigation,
        normalize_scene_filename, parse_scene_block, strip_scene_navigation,
    },
    types::ProfileType,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/runtime_trace.json")).unwrap()
}

struct FixedClock;
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        1_767_225_845_000
    }
}
struct TraceRunner(Arc<Mutex<Vec<String>>>);
impl PipelineRunner for TraceRunner {
    fn run_l1(&mut self, _: &str, _: &[CapturedMessage]) -> Result<(), String> {
        self.0.lock().unwrap().push("l1".into());
        Ok(())
    }
    fn run_l2(&mut self, _: &str, _: Option<&str>) -> Result<L2Result, String> {
        self.0.lock().unwrap().push("l2".into());
        Ok(L2Result {
            latest_cursor: Some("2026-01-02T04:00:00.000Z".into()),
            skipped: false,
        })
    }
    fn run_l3(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().push("l3".into());
        Ok(())
    }
}
struct TracePersist(Arc<Mutex<Vec<HashMap<String, PipelineSessionState>>>>);
impl StatePersister for TracePersist {
    fn persist(&mut self, s: &HashMap<String, PipelineSessionState>) -> Result<(), String> {
        self.0.lock().unwrap().push(s.clone());
        Ok(())
    }
}

#[test]
fn pipeline_state_trace_matches_ts_with_documented_clock_call_tolerance() {
    let calls = Arc::new(Mutex::new(vec![]));
    let states = Arc::new(Mutex::new(vec![]));
    let mut manager = PipelineManager::new(
        PipelineConfig {
            every_n_conversations: 2,
            enable_warmup: false,
            l1_idle_timeout_ms: 60_000,
            l2_delay_after_l1_ms: 60_000,
            l2_min_interval_ms: 0,
            l2_max_interval_ms: 3_600_000,
            session_active_window_ms: 86_400_000,
        },
        Box::new(FixedClock),
        Box::new(TraceRunner(calls.clone())),
    )
    .with_persister(Box::new(TracePersist(states.clone())));
    manager.start(HashMap::new());
    manager.notify_conversation(
        "s",
        vec![CapturedMessage {
            role: "user".into(),
            content: "one".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
        }],
    );
    manager.notify_conversation(
        "s",
        vec![CapturedMessage {
            role: "assistant".into(),
            content: "two".into(),
            timestamp: "2026-01-01T00:00:01Z".into(),
        }],
    );
    manager.shutdown();
    assert_eq!(&*calls.lock().unwrap(), &["l1", "l2"]);
    let final_state = states.lock().unwrap().last().unwrap()["s"].clone();
    let expected = &oracle()["pipeline"]["finalState"];
    assert_eq!(
        final_state.conversation_count,
        expected["conversation_count"]
    );
    assert_eq!(
        final_state.l2_pending_l1_count,
        expected["l2_pending_l1_count"]
    );
    assert_eq!(final_state.warmup_threshold, expected["warmup_threshold"]);
    // Both oracles use the same fixed epoch. TS invokes Date.now nine times
    // while Rust's injected Clock is stable; call-count drift is bounded to 10 ms.
    assert!(
        (final_state.last_active_time - expected["last_active_time"].as_i64().unwrap()).abs() <= 10
    );
}

#[test]
fn scene_profile_and_offload_pure_outputs_match_ts_exactly() {
    let expected = oracle();
    let scene = &expected["scene"];
    assert_eq!(
        ["Hello World", "中文 场景", "a/b:c"].map(normalize_scene_filename),
        scene["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
            .as_slice()
    );
    let meta = aeon_memory_core::scene::SceneBlockMeta {
        created: "2025-01-01".into(),
        updated: "2026-01-01".into(),
        summary: "S".into(),
        heat: 2,
    };
    assert_eq!(format_meta(&meta), scene["meta"].as_str().unwrap());
    assert_eq!(
        format_scene_block(&meta, "Body"),
        scene["formatted"].as_str().unwrap()
    );
    let raw =
        "---\nsummary: Coding\nkeywords: [rust, ai]\nheat: 3\nupdated: 2026-01-01\n---\nBody\n";
    let parsed = parse_scene_block(raw, "coding.md");
    assert_eq!(parsed.filename, scene["parsed"]["filename"]);
    assert_eq!(parsed.meta.created, scene["parsed"]["meta"]["created"]);
    assert_eq!(parsed.meta.updated, scene["parsed"]["meta"]["updated"]);
    assert_eq!(parsed.meta.summary, scene["parsed"]["meta"]["summary"]);
    assert_eq!(parsed.meta.heat, scene["parsed"]["meta"]["heat"]);
    assert_eq!(parsed.content, scene["parsed"]["content"]);
    let nav = generate_scene_navigation(
        &[SceneIndexEntry {
            filename: "coding.md".into(),
            summary: "Coding".into(),
            heat: 3,
            created: String::new(),
            updated: "2026-01-01".into(),
        }],
        Some(std::path::Path::new("/data")),
    );
    assert_eq!(nav, scene["navigation"]);
    assert_eq!(
        strip_scene_navigation("Persona\n\n<scene-navigation>old</scene-navigation>"),
        scene["stripped"]
    );
    assert_eq!(
        build_profile_stable_id("scope", ProfileType::L2, "coding.md"),
        expected["profile"]["ids"][0]
    );
    assert_eq!(
        build_profile_stable_id("scope", ProfileType::L3, "persona.md"),
        expected["profile"]["ids"][1]
    );
    let dir =
        std::env::temp_dir().join(format!("aeon-memory-profile-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scene_blocks")).unwrap();
    std::fs::write(dir.join("scene_blocks/coding.md"), raw).unwrap();
    std::fs::write(dir.join("persona.md"), "Persona body").unwrap();
    let profiles=list_local_profiles(&dir).into_iter().map(|p|json!({"id":p.id,"type":p.r#type,"filename":p.filename,"content":p.content,"version":p.version})).collect::<Vec<_>>();
    assert_eq!(json!(profiles), expected["profile"]["profiles"]);
    let _ = std::fs::remove_dir_all(dir);
    let messages = vec![
        json!({"role":"user","content":"u"}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"shell","input":{"cmd":"ls"}}]}),
        json!({"role":"tool","tool_call_id":"call_1","content":"ok"}),
    ];
    assert_eq!(
        inject::active_insertion_point(&messages),
        expected["offload"]["activePoint"]
    );
    assert_eq!(
        inject::active_insertion_point(&messages),
        expected["offload"]["historyPoint"]
    );
    assert_eq!(
        l3::normalize_tool_call_id("toolu_abc-123"),
        expected["offload"]["normalized"]
    );
    assert_eq!(
        l3::compact_tool_call("shell: {\"cmd\":\"ls -la /very/long/path\"}"),
        expected["offload"]["compact"]
    );
    let keys = ["agent:main:abc", "bad", "agent:x:swebench-w12-z"].map(storage::parse_session_key);
    assert_eq!(keys[0], Some(("main".into(), "abc".into())));
    assert_eq!(keys[1], None);
    assert_eq!(keys[2], Some(("x-w12".into(), "swebench-w12-z".into())));
}
