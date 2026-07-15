use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aeon_memory_core::config::{GatewayConfig, RecallStrategy};
use aeon_memory_gateway::{
    AeonMemoryService,
    runtime::build_core,
    service::{CaptureRequest, MemorySearchRequest, RecallRequest, SessionEndRequest},
};

fn l0_ids(request: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for (offset, _) in request.match_indices("l0_") {
        let id = request[offset..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            .collect::<String>();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

async fn layered_llm_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut bytes = Vec::new();
                let mut chunk = [0_u8; 4096];
                let body_end = loop {
                    let read = stream.read(&mut chunk).await.unwrap_or_default();
                    if read == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let body_end = header_end + 4 + content_length;
                    if bytes.len() >= body_end {
                        break body_end;
                    }
                };
                let request = String::from_utf8_lossy(&bytes[..body_end]);
                let message = if request.contains("Memory Consolidation Architect") {
                    if request.contains("\"tool_call_id\":\"scene-write\"") {
                        serde_json::json!({
                            "role": "assistant",
                            "content": "[PERSONA_UPDATE_REQUEST] reason: new durable preference [/PERSONA_UPDATE_REQUEST]"
                        })
                    } else {
                        let arguments = serde_json::json!({
                            "path": "Cycling.md",
                            "content": "-----META-START-----\ncreated: 2026-07-13T00:00:00Z\nupdated: 2026-07-13T00:00:00Z\nsummary: The user's cobalt bicycle preference\nheat: 3\n-----META-END-----\n\n# Cycling\n\nThe user prefers a cobalt bicycle."
                        })
                        .to_string();
                        serde_json::json!({
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "scene-write",
                                "type": "function",
                                "function": {"name": "write_to_file", "arguments": arguments}
                            }]
                        })
                    }
                } else if request.contains("Persona Architect") {
                    if request.contains("\"tool_call_id\":\"persona-write\"") {
                        serde_json::json!({"role": "assistant", "content": "Persona updated."})
                    } else {
                        let arguments = serde_json::json!({
                            "path": "persona.md",
                            "content": "# User Persona\n\nThe user prefers a cobalt bicycle."
                        })
                        .to_string();
                        serde_json::json!({
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "persona-write",
                                "type": "function",
                                "function": {"name": "write_to_file", "arguments": arguments}
                            }]
                        })
                    }
                } else if request.contains("记忆冲突检测器") {
                    serde_json::json!({"role": "assistant", "content": "[]"})
                } else {
                    let ids = l0_ids(&request);
                    let content = serde_json::json!([{
                        "scene_name": "Cycling",
                        "message_ids": ids,
                        "memories": [{
                            "content": "The user prefers a cobalt bicycle",
                            "type": "persona",
                            "priority": 90,
                            "source_message_ids": ids,
                            "metadata": {}
                        }]
                    }])
                    .to_string();
                    serde_json::json!({"role": "assistant", "content": content})
                };
                let body = serde_json::json!({
                    "choices": [{"message": message}]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{address}/v1")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capture_pipeline_and_recall_preserve_all_long_term_layers_and_channels() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aeon-memory-layered-recall-{unique}"));
    let mut config = GatewayConfig::default();
    config.data.base_dir = root.to_string_lossy().into_owned();
    config.llm.base_url = layered_llm_server().await;
    config.llm.api_key = "test-key".into();
    config.llm.model = "deterministic-mock".into();
    config.memory.embedding.enabled = false;
    config.memory.recall.strategy = RecallStrategy::Keyword;
    config.memory.pipeline.enable_warmup = false;
    config.memory.pipeline.every_n_conversations = 100;
    config.memory.pipeline.l2_delay_after_l1_seconds = 0;
    config.memory.pipeline.l2_min_interval_seconds = 0;
    let service = build_core(&config).await.unwrap();

    let captured = service
        .capture(CaptureRequest {
            user_content: "Remember that I prefer a cobalt bicycle.".into(),
            assistant_content: "I will remember your cobalt bicycle preference.".into(),
            session_key: "layered-session".into(),
            session_id: Some("layered-session-id".into()),
            user_id: None,
            messages: None,
        })
        .await
        .unwrap();
    assert_eq!(captured.l0_recorded, 2, "capture must persist the L0 turn");
    assert!(captured.scheduler_notified);

    service
        .end_session(SessionEndRequest {
            session_key: "layered-session".into(),
            user_id: None,
        })
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(root.join("persona.md").is_file() && root.join(".metadata/scene_index.json").is_file())
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "L1 -> L2 -> L3 pipeline did not materialize scene and persona"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let recalled = service
        .recall(RecallRequest {
            query: "cobalt bicycle".into(),
            session_key: "layered-session".into(),
            user_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        recalled.memory_count, 1,
        "official recall body reports L1 count"
    );
    assert_eq!(recalled.strategy.as_deref(), Some("keyword"));
    let recalled_dynamic = recalled
        .prepend_context
        .as_deref()
        .expect("recall must expose the strategy-selected dynamic L1 payload");
    assert!(recalled_dynamic.contains("cobalt bicycle"));
    let dynamic = service
        .search_memories(MemorySearchRequest {
            query: "cobalt bicycle".into(),
            limit: Some(5),
            memory_type: None,
            scene: None,
        })
        .await
        .unwrap();
    assert_eq!(
        dynamic.total, 1,
        "L1 remains available through the official search API"
    );
    assert!(dynamic.results.contains("Found 1 matching memories:"));
    assert!(
        dynamic
            .results
            .contains("The user prefers a cobalt bicycle")
    );

    let stable = recalled.context;
    assert!(stable.contains("<user-persona>"));
    assert!(stable.contains("# User Persona"));
    assert!(stable.contains("<scene-navigation>"));
    assert!(stable.contains("scene_blocks/Cycling.md"));
    assert!(stable.contains("<memory-tools-guide>"));
    assert!(stable.contains("The user prefers a cobalt bicycle."));

    let _ = std::fs::remove_dir_all(root);
}
