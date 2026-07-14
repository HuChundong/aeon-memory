use aeon_memory_core::{
    AeonMemoryCoreError, AeonMemoryResult,
    offload::{
        OffloadEngine, inject, l3, prompt,
        storage::{self, StorageContext},
        token::{self, Tokenizer},
        types::{BoundaryResult, L15Boundary, OffloadConfig, OffloadEntry, ToolPair},
    },
    types::{LlmRunParams, LlmRunner},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    sync::Mutex,
};

struct QueueLlm(Mutex<VecDeque<AeonMemoryResult<String>>>);
#[async_trait]
impl LlmRunner for QueueLlm {
    async fn run(&self, _: LlmRunParams) -> AeonMemoryResult<String> {
        self.0.lock().unwrap().pop_front().unwrap()
    }
}
fn runtime_oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/offload_runtime_oracle.json")).unwrap()
}
fn ctx(name: &str) -> StorageContext {
    let root = std::env::temp_dir().join(format!(
        "aeon-memory-offload-complete-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    StorageContext::new(root, "agent", "session")
}
fn pair(id: &str) -> ToolPair {
    ToolPair {
        tool_name: "read".into(),
        tool_call_id: id.into(),
        params: json!({"path":"a"}),
        result: json!({"ok":true}),
        error: None,
        timestamp: "2026-07-13T08:00:00+08:00".into(),
        duration_ms: Some(1),
    }
}
fn entry(id: &str, score: f64, node: Option<&str>) -> OffloadEntry {
    OffloadEntry {
        timestamp: "2026-07-13T00:00:00Z".into(),
        node_id: node.map(str::to_owned),
        tool_call: "read({})".into(),
        summary: "summary".into(),
        result_ref: "refs/a.md".into(),
        tool_call_id: id.into(),
        session_key: None,
        score: Some(score),
        offloaded: None,
    }
}

#[test]
fn persisted_status_replays_replacement_and_deletion_on_pristine_history() {
    let mut confirmed_entry = entry("tool_u_1", 9.0, Some("N1"));
    confirmed_entry.offloaded = Some(Value::Bool(true));
    let mut deleted_entry = entry("gone_2", 9.0, Some("N2"));
    deleted_entry.offloaded = Some(Value::String("deleted".into()));
    let entries = vec![confirmed_entry, deleted_entry];
    let confirmed = storage::confirmed_offload_ids(&entries);
    let deleted = storage::deleted_offload_ids(&entries);
    assert!(confirmed.contains("toolu1"));
    assert!(deleted.contains("gone2"));

    let pristine = vec![
        json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu1","input":{"path":"raw"}}]}),
        json!({"role":"tool","toolCallId":"toolu1","content":"raw confirmed result"}),
        json!({"role":"assistant","content":[
            {"type":"text","text":"keep this text"},
            {"type":"tool_use","id":"gone2","input":{"path":"delete"}},
            {"type":"tool_use","id":"toolu1","input":{"path":"compress"}}
        ]}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"gone2","input":{}}]}),
        json!({"role":"tool","toolCallId":"gone2","content":"raw deleted result"}),
        json!({"role":"user","content":"latest request"}),
    ];
    let mut first_replay = pristine.clone();
    let first = l3::fast_path_reapply(&mut first_replay, &entries, &confirmed, &deleted);
    assert_eq!(first.applied, 2);
    assert_eq!(first.deleted, 2);
    assert_eq!(first_replay.len(), 4);
    assert!(first_replay[0]["_offloaded"].as_bool().unwrap());
    assert!(
        first_replay[1]["content"]
            .as_str()
            .unwrap()
            .contains("Summary:")
    );
    let mixed = first_replay[2]["content"].as_array().unwrap();
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed[0]["text"], "keep this text");
    assert_eq!(mixed[1]["id"], "toolu1");
    assert_eq!(mixed[1]["input"]["_offloaded"], true);

    let mut second_replay = pristine;
    let second = l3::fast_path_reapply(&mut second_replay, &entries, &confirmed, &deleted);
    assert_eq!(second, first);
    assert_eq!(second_replay, first_replay);
}

#[test]
fn compression_status_roundtrips_through_offload_jsonl() {
    let context = ctx("persistent-compression-status");
    storage::append_entry(&context, &entry("tool_u_1", 9.0, Some("N1"))).unwrap();
    storage::append_entry(&context, &entry("gone_2", 9.0, Some("N2"))).unwrap();
    let updates = HashMap::from([
        ("toolu1".to_string(), Value::Bool(true)),
        ("gone2".to_string(), Value::String("deleted".into())),
    ]);
    storage::mark_offload_status(&context, &updates).unwrap();
    let reloaded = storage::read_entries(&context).unwrap();
    assert_eq!(reloaded[0].offloaded, Some(Value::Bool(true)));
    assert_eq!(reloaded[1].offloaded, Some(Value::String("deleted".into())));
    assert!(storage::confirmed_offload_ids(&reloaded).contains("toolu1"));
    assert!(storage::deleted_offload_ids(&reloaded).contains("gone2"));
}

#[test]
fn l1_prompt_is_ts_exact_fixture() {
    let got = prompt::build_l1_user_prompt("ctx", &[pair("call_1")]);
    let expected = "## 最近的对话上下文（用于理解当前任务）：\nctx\n\n## Tool call/result pairs to summarize:\n--- Tool Pair 1 ---\ntool_call_id: call_1\ntimestamp: 2026-07-13T08:00:00+08:00\nTool: read\nParams: {\"path\":\"a\"}\nResult: {\"ok\":true}\n\nSummarize each pair into the JSON array format described.";
    assert_eq!(got, expected);
    assert!(prompt::l1_system_prompt().starts_with("你是一个专为 AI 编码助手"));
    assert!(prompt::l15_system_prompt().contains("任务生命周期门神"));
    assert!(prompt::l2_system_prompt().contains("任务拓扑架构师"));
    assert!(prompt::l2_system_prompt().contains("```mermaid"));
    assert!(!prompt::l2_system_prompt().contains("\\`"));
}

#[test]
fn all_offload_prompts_are_byte_exact_to_legacy_runtime_replay() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/offload_prompt_legacy_replay.json")).unwrap();
    assert_eq!(
        prompt::l1_system_prompt(),
        expected["l1System"].as_str().unwrap()
    );
    let replay_pair = ToolPair {
        tool_name: "shell".into(),
        tool_call_id: "call-1".into(),
        params: json!({"cmd":"ls"}),
        result: json!("file.txt"),
        error: None,
        timestamp: "2026-07-13T00:00:00Z".into(),
        duration_ms: None,
    };
    assert_eq!(
        prompt::build_l1_user_prompt("ctx", &[replay_pair]),
        expected["l1User"].as_str().unwrap()
    );
    let metas = vec![prompt::MmdMeta {
        filename: "002-old.mmd".into(),
        path: "/tmp/002-old.mmd".into(),
        task_goal: "old".into(),
        done_count: 1,
        doing_count: 1,
        todo_count: 0,
        updated_time: Some("2026-07-12T00:00:00Z".into()),
        node_summaries: vec![prompt::NodeSummary {
            node_id: "002-N1".into(),
            status: "done".into(),
            summary: "ok".into(),
        }],
    }];
    assert_eq!(
        prompt::l15_system_prompt(),
        expected["l15System"].as_str().unwrap()
    );
    assert_eq!(
        prompt::build_l15_user_prompt(
            "ctx",
            Some((
                "001-task.mmd",
                "flowchart TD\n  N1[\"x\"]",
                "/tmp/001-task.mmd"
            )),
            &metas,
        ),
        expected["l15User"].as_str().unwrap()
    );
    let entry = OffloadEntry {
        timestamp: "2026-07-13T00:00:00Z".into(),
        node_id: None,
        tool_call: "shell".into(),
        summary: "listed files".into(),
        result_ref: String::new(),
        tool_call_id: "call-1".into(),
        session_key: None,
        score: None,
        offloaded: None,
    };
    assert_eq!(
        prompt::l2_system_prompt(),
        expected["l2System"].as_str().unwrap()
    );
    assert_eq!(
        prompt::build_l2_user_prompt(
            Some("flowchart TD\n  N1[\"x\"]"),
            &[entry],
            Some("ctx"),
            Some("turn"),
            "task",
            "001",
            2100,
        ),
        expected["l2User"].as_str().unwrap()
    );
}

#[tokio::test]
async fn l1_failure_keeps_pair_retryable_and_success_commits() {
    let c = ctx("l1");
    let mut e = OffloadEngine::load(
        c.clone(),
        OffloadConfig {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    e.buffer(pair("call_1"));
    let llm=QueueLlm(Mutex::new(VecDeque::from([Err(AeonMemoryCoreError::InvalidInput("offline".into())),Ok("[{\"tool_call_id\":\"call_1\",\"tool_call\":\"read\",\"summary\":\"ok\",\"timestamp\":\"2026-07-13T08:00:00+08:00\",\"score\":9}]".into())])));
    assert!(e.flush_l1(&llm, "ctx", true).await.unwrap().is_empty());
    assert_eq!(e.pending_count(), 1);
    let rows = e.flush_l1(&llm, "ctx", true).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(e.pending_count(), 0);
    assert!(!rows[0].result_ref.is_empty());
    assert_eq!(storage::read_entries(&c).unwrap().len(), 1);
}

#[test]
fn pending_tool_pairs_are_process_local_and_clear_on_reload() {
    let ctx = ctx("pending-reload");
    let mut first = OffloadEngine::load(ctx.clone(), OffloadConfig::default()).unwrap();
    assert!(first.buffer_persisted(pair("persisted-call")).unwrap());
    drop(first);
    assert!(!ctx.data_dir.join("pending-session.json").exists());
    let second = OffloadEngine::load(ctx, OffloadConfig::default()).unwrap();
    assert_eq!(
        second.pending_count(),
        runtime_oracle()["pendingFiles"].as_array().unwrap().len()
    );
}

#[tokio::test]
async fn l1_third_consecutive_failure_writes_degraded_fallback() {
    let c = ctx("l1-degraded");
    let mut engine = OffloadEngine::load(c.clone(), OffloadConfig::default()).unwrap();
    engine.buffer(pair("call-degraded"));
    let llm = QueueLlm(Mutex::new(VecDeque::from([
        Err(AeonMemoryCoreError::InvalidInput("offline-1".into())),
        Err(AeonMemoryCoreError::InvalidInput("offline-2".into())),
        Err(AeonMemoryCoreError::InvalidInput("offline-3".into())),
    ])));
    assert!(engine.flush_l1(&llm, "ctx", true).await.unwrap().is_empty());
    assert!(engine.flush_l1(&llm, "ctx", true).await.unwrap().is_empty());
    assert_eq!(runtime_oracle()["l1FailureFallbackAttempt"], 3);
    let rows = engine.flush_l1(&llm, "ctx", true).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].score, Some(0.0));
    assert!(rows[0].summary.starts_with("[L1 degraded] read:"));
    assert!(!rows[0].result_ref.is_empty());
    assert_eq!(engine.pending_count(), 0);
    assert_eq!(storage::read_entries(&c).unwrap(), rows);
}

#[tokio::test]
async fn l1_flush_drains_selected_pairs_in_batches_of_five() {
    assert_eq!(runtime_oracle()["l1BatchSize"], 5);
    let c = ctx("l1-batches");
    let mut engine = OffloadEngine::load(
        c.clone(),
        OffloadConfig {
            max_pairs_per_batch: 20,
            ..Default::default()
        },
    )
    .unwrap();
    for index in 0..12 {
        engine.buffer(pair(&format!("call-{index}")));
    }
    let response = |start: usize, end: usize| {
        serde_json::to_string(
            &(start..end)
                .map(|index| {
                    json!({
                        "tool_call_id": format!("call-{index}"),
                        "tool_call": "read({})",
                        "summary": format!("summary-{index}"),
                        "timestamp": "2026-07-13T08:00:00+08:00",
                        "score": 9
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let llm = QueueLlm(Mutex::new(VecDeque::from([
        Ok(response(0, 5)),
        Ok(response(5, 10)),
        Ok(response(10, 12)),
    ])));
    let rows = engine.flush_l1(&llm, "ctx", true).await.unwrap();
    assert_eq!(rows.len(), 12);
    assert_eq!(engine.pending_count(), 0);
    assert_eq!(storage::read_entries(&c).unwrap().len(), 12);
}

#[tokio::test]
async fn l15_and_l2_run_complete_state_machine() {
    let c = ctx("state");
    let mut e = OffloadEngine::load(
        c.clone(),
        OffloadConfig {
            enabled: true,
            l2_null_threshold: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let j=QueueLlm(Mutex::new(VecDeque::from([Ok("{\"taskCompleted\":true,\"isContinuation\":false,\"isLongTask\":true,\"continuationMmdFile\":null,\"newTaskLabel\":\"refactor-api\"}".into())])));
    e.judge_l15(&j, "recent", None, &[]).await.unwrap();
    let active = e.state.active_mmd_file.clone().unwrap();
    assert_eq!(active, "001-refactor-api.mmd");
    storage::append_entry(&c, &entry("x", 8.0, None)).unwrap();
    e.state.entry_counter = 1;
    e.state.l15_boundaries = vec![L15Boundary {
        start_index: 0,
        result: BoundaryResult::Long,
        target_mmd: Some(active.clone()),
    }];
    storage::save_state(&c, &e.state).unwrap();
    let l2=QueueLlm(Mutex::new(VecDeque::from([Ok("{\"file_action\":\"write\",\"mmd_content\":\"```mermaid\\nflowchart TD\\n001-N1[\\\"done\\\"]\\n```\",\"replace_blocks\":[],\"node_mapping\":{\"x\":\"001-N1\"}}".into())])));
    assert_eq!(
        e.run_l2(&l2, None, None, chrono::Utc::now()).await.unwrap(),
        1
    );
    assert_eq!(
        storage::read_entries(&c).unwrap()[0].node_id.as_deref(),
        Some("001-N1")
    );
    assert!(c.mmds_dir.join(active).exists());
}

struct Length;
impl Tokenizer for Length {
    fn encoding(&self) -> &str {
        "fixture-length"
    }
    fn count(&self, s: &str) -> usize {
        s.chars().count()
    }
}
#[test]
fn tokenizer_trait_preserves_exact_snapshot_algorithm() {
    let msgs = vec![json!({"role":"user","content":"hi","details":"not sent"})];
    let s = token::snapshot_with(&Length, "x", &msgs, Some("sys"), Some("hi"));
    assert_eq!(s.encoding, "fixture-length");
    assert_eq!(s.system_tokens, 3);
    assert_eq!(s.user_prompt_tokens, 0);
    assert_eq!(
        s.messages_tokens,
        "{\"content\":\"hi\",\"role\":\"user\"}".chars().count() + 1
    );
}

#[test]
fn mmd_injection_obeys_budget_and_pair_safe_position() {
    let c = ctx("inject");
    c.ensure_dirs().unwrap();
    fs::write(c.mmds_dir.join("001-x.mmd"), "flowchart TD\n001-N1[\"x\"]").unwrap();
    let mut msgs = vec![
        json!({"role":"user","content":"do it"}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"x"}]}),
        json!({"role":"tool","toolCallId":"x","content":"result"}),
    ];
    let n = inject::inject_active(&mut msgs, &c, Some("001-x.mmd"), &Length, 10_000, 0.2).unwrap();
    assert!(n > 0);
    let idx = msgs
        .iter()
        .position(|m| m.get(inject::MMD_MARKER).is_some())
        .unwrap();
    assert_eq!(idx, 1);
    let mut too_small = vec![json!({"role":"user","content":"x"})];
    assert_eq!(
        inject::inject_active(&mut too_small, &c, Some("001-x.mmd"), &Length, 10, 0.01).unwrap(),
        0
    );
}

#[test]
fn l3_selects_mild_aggressive_and_emergency() {
    let entries = vec![entry("a", 9.0, Some("old"))];
    let current = HashSet::from(["current".into()]);
    let base = vec![
        json!({"role":"user","content":"u"}),
        json!({"role":"tool","toolCallId":"a","content":"x".repeat(300)}),
        json!({"role":"assistant","content":"yyyyyyyyyyyyyyyyyyyyyyyy"}),
        json!({"role":"assistant","content":"zzzzzzzzzzzzzzzzzzzzzzzz"}),
        json!({"role":"assistant","content":"qqqqqqqqqqqqqqqqqqqqqqqq"}),
        json!({"role":"assistant","content":"pppppppppppppppppppppppp"}),
        json!({"role":"assistant","content":"rrrrrrrrrrrrrrrrrrrrrrrr"}),
    ];
    let mut mild = base.clone();
    let cfg = OffloadConfig {
        mild_offload_ratio: 0.05,
        mild_offload_scan_ratio: 1.0,
        aggressive_compress_ratio: 0.9,
        emergency_compress_ratio: 0.95,
        ..Default::default()
    };
    let r = l3::compress(&mut mild, &entries, &current, 1000, "", &cfg, &Length);
    assert_eq!(r.mode, l3::CompressionMode::Mild);
    assert_eq!(r.replaced, 1);
    let mut aggressive = base.clone();
    let cfg = OffloadConfig {
        mild_offload_ratio: 0.01,
        aggressive_compress_ratio: 0.05,
        emergency_compress_ratio: 0.95,
        ..Default::default()
    };
    assert_eq!(
        l3::compress(&mut aggressive, &entries, &current, 1000, "", &cfg, &Length).mode,
        l3::CompressionMode::Aggressive
    );
    let mut emergency = base;
    let cfg = OffloadConfig {
        mild_offload_ratio: 0.01,
        aggressive_compress_ratio: 0.01,
        emergency_compress_ratio: 0.02,
        emergency_target_ratio: 0.01,
        ..Default::default()
    };
    assert_eq!(
        l3::compress(&mut emergency, &entries, &current, 1000, "", &cfg, &Length).mode,
        l3::CompressionMode::Emergency
    );
}
