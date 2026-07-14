use std::{
    fs,
    sync::{Arc, Mutex},
};

use aeon_memory_core::{
    AeonMemoryResult,
    offload::{OffloadConfig, ToolPair},
    types::{LlmRunParams, LlmRunner},
};
use aeon_memory_gateway::{
    adapter::OffloadOperations,
    runtime::EngineOffloadOperations,
    service::{AfterToolRequest, BeforePromptRequest},
};
use async_trait::async_trait;
use serde_json::{Value, json};

#[derive(Default)]
struct LifecycleLlm {
    tasks: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmRunner for LifecycleLlm {
    async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
        self.tasks.lock().unwrap().push(params.task_id.clone());
        Ok(match params.task_id.as_str() {
            "offload-l15" => json!({
                "taskCompleted": false,
                "isContinuation": false,
                "isLongTask": true,
                "continuationMmdFile": null,
                "newTaskLabel": "gateway-lifecycle"
            })
            .to_string(),
            "offload-l1" => {
                let id = params
                    .prompt
                    .lines()
                    .find_map(|line| line.strip_prefix("tool_call_id: "))
                    .unwrap();
                json!([{
                    "tool_call_id": id,
                    "tool_call": "read",
                    "summary": "read result summarized",
                    "timestamp": "2026-07-13T00:00:00Z",
                    "score": 9
                }])
                .to_string()
            }
            "offload-l2" => json!({
                "file_action": "write",
                "mmd_content": "```mermaid\nflowchart TD\n001-N1[\"done\"]\n```",
                "replace_blocks": [],
                "node_mapping": {"call_1": "001-N1"}
            })
            .to_string(),
            other => panic!("unexpected LLM task: {other}"),
        })
    }
}

fn pair() -> ToolPair {
    ToolPair {
        tool_name: "read".into(),
        tool_call_id: "call_1".into(),
        params: json!({"path":"a.txt"}),
        result: json!({"text":"large result"}),
        error: None,
        timestamp: "2026-07-13T00:00:00Z".into(),
        duration_ms: Some(2),
    }
}

#[tokio::test]
async fn gateway_compression_uses_the_same_o200k_counter_as_context_snapshot() {
    let root = std::env::temp_dir().join(format!(
        "aeon-memory-gateway-offload-tokenizer-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let operations = EngineOffloadOperations::new(
        root.clone(),
        true,
        Arc::new(LifecycleLlm::default()),
        OffloadConfig {
            enabled: true,
            mild_offload_ratio: 2.0,
            aggressive_compress_ratio: 2.0,
            emergency_compress_ratio: 2.0,
            ..Default::default()
        },
    );
    let response = operations
        .before_prompt(BeforePromptRequest {
            agent_id: "main".into(),
            session_id: "tokenizer".into(),
            system_prompt: "system prompt 中文 mixed".into(),
            user_prompt: String::new(),
            messages: vec![json!({
                "role":"user",
                "content":"deterministic tokenizer boundary 中文内容 ".repeat(20)
            })],
            context_window: 200_000,
        })
        .await
        .unwrap();
    assert_eq!(response.context["encoding"], "o200k_base");
    assert_eq!(
        response.compression["tokensBefore"],
        response.context["totalTokens"]
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gateway_offload_runs_l15_l1_l2_and_real_l3_compression() {
    let root = std::env::temp_dir().join(format!(
        "aeon-memory-gateway-offload-lifecycle-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let llm = Arc::new(LifecycleLlm::default());
    let config = OffloadConfig {
        enabled: true,
        force_trigger_threshold: 1,
        l2_null_threshold: 1,
        mild_offload_ratio: 0.0,
        aggressive_compress_ratio: 2.0,
        emergency_compress_ratio: 2.0,
        ..Default::default()
    };
    let operations = EngineOffloadOperations::new(root.clone(), true, llm.clone(), config.clone());

    let first = operations
        .before_prompt(BeforePromptRequest {
            agent_id: "main".into(),
            session_id: "s1".into(),
            system_prompt: "system".into(),
            user_prompt: "implement the gateway".into(),
            messages: vec![json!({"role":"user","content":"implement the gateway"})],
            context_window: 10_000,
        })
        .await
        .unwrap();
    assert!(first.l15_judgment.unwrap().is_long_task);
    assert_eq!(first.compression["mode"], "mild");

    let messages: Vec<Value> = vec![
        json!({"role":"user","content":"implement the gateway"}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"call_1"}]}),
        json!({
            "role":"tool",
            "toolCallId":"call_1",
            "content":"large result payload ".repeat(100)
        }),
        json!({"role":"assistant","content":"continue with the next step"}),
        json!({"role":"user","content":"verify the completed work"}),
    ];
    let after = operations
        .after_tool(AfterToolRequest {
            agent_id: "main".into(),
            session_id: "s1".into(),
            tool: pair(),
            messages: messages.clone(),
            context_window: 10_000,
        })
        .await
        .unwrap();
    assert_eq!(after.l1_entries.len(), 1);
    assert!(after.l2_updated);
    assert_eq!(after.compression["applied"], true);
    assert_eq!(after.compression["mode"], "mild");
    assert!(after.compression["replaced"].as_u64().unwrap() >= 1);
    let persisted = fs::read_to_string(root.join("main/offload-s1.jsonl")).unwrap();
    assert!(persisted.contains(r#""offloaded":true"#));

    // Simulate a process restart and a host that submits the original, pristine
    // transcript again. Persistent fast-path state must reproduce the prior
    // replacement before threshold-based compression runs.
    drop(operations);
    let restarted = EngineOffloadOperations::new(root.clone(), true, llm.clone(), config.clone());
    let repeated = restarted
        .before_prompt(BeforePromptRequest {
            agent_id: "main".into(),
            session_id: "s1".into(),
            system_prompt: "system".into(),
            user_prompt: "implement the gateway".into(),
            messages: messages.clone(),
            context_window: 10_000,
        })
        .await
        .unwrap();
    assert!(repeated.l15_judgment.is_none());
    assert!(repeated.active_mmd.is_some());
    assert!(repeated.compression["fastReplaced"].as_u64().unwrap() >= 1);
    assert!(repeated.messages.iter().any(|message| {
        message.get("toolCallId").and_then(Value::as_str) == Some("call_1")
            && message.get("_offloaded").and_then(Value::as_bool) == Some(true)
    }));

    // A later aggressive decision upgrades the same persistent status to
    // "deleted". After another restart, pristine host history must have both
    // the tool-use assistant and tool-result removed on the fast path.
    drop(restarted);
    let aggressive = EngineOffloadOperations::new(
        root.clone(),
        true,
        llm.clone(),
        OffloadConfig {
            mild_offload_ratio: 2.0,
            aggressive_compress_ratio: 0.01,
            emergency_compress_ratio: 2.0,
            ..config.clone()
        },
    );
    let aggressively_compressed = aggressive
        .before_prompt(BeforePromptRequest {
            agent_id: "main".into(),
            session_id: "s1".into(),
            system_prompt: "system".into(),
            user_prompt: "implement the gateway".into(),
            messages: messages.clone(),
            context_window: 1_000,
        })
        .await
        .unwrap();
    assert_eq!(aggressively_compressed.compression["mode"], "aggressive");
    let persisted = fs::read_to_string(root.join("main/offload-s1.jsonl")).unwrap();
    assert!(persisted.contains(r#""offloaded":"deleted""#));

    drop(aggressive);
    let deleted_restarted = EngineOffloadOperations::new(root.clone(), true, llm.clone(), config);
    let deleted_replay = deleted_restarted
        .before_prompt(BeforePromptRequest {
            agent_id: "main".into(),
            session_id: "s1".into(),
            system_prompt: "system".into(),
            user_prompt: "implement the gateway".into(),
            messages,
            context_window: 10_000,
        })
        .await
        .unwrap();
    assert!(deleted_replay.compression["fastDeleted"].as_u64().unwrap() >= 2);
    assert!(!deleted_replay.messages.iter().any(|message| {
        message.get("toolCallId").and_then(Value::as_str) == Some("call_1")
            || message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|block| block.get("id").and_then(Value::as_str) == Some("call_1"))
    }));
    assert_eq!(
        llm.tasks.lock().unwrap().as_slice(),
        ["offload-l15", "offload-l1", "offload-l2"]
    );
    let _ = fs::remove_dir_all(root);
}
