use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aeon_memory_core::config::GatewayConfig;
use aeon_memory_core::pipeline::checkpoint::read_checkpoint;
use aeon_memory_core::types::{IMemoryStore, L1QueryFilter};
use aeon_memory_gateway::{
    AeonMemoryService,
    runtime::build_core,
    service::{CaptureRequest, SessionEndRequest},
};

#[derive(Clone)]
struct MockState {
    l1_calls: Arc<AtomicUsize>,
    first_503: Arc<tokio::sync::Semaphore>,
    events: Arc<Mutex<Vec<String>>>,
    l1_id_counts: Arc<Mutex<Vec<usize>>>,
    retry_blocked: Arc<tokio::sync::Semaphore>,
}

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

async fn retrying_llm_server() -> (String, MockState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = MockState {
        l1_calls: Arc::new(AtomicUsize::new(0)),
        first_503: Arc::new(tokio::sync::Semaphore::new(0)),
        events: Arc::new(Mutex::new(Vec::new())),
        l1_id_counts: Arc::new(Mutex::new(Vec::new())),
        retry_blocked: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let server_state = state.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let state = server_state.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut bytes = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).await.unwrap_or_default();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    let Some(headers_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&bytes[..headers_end]);
                    let content_length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    });
                    if content_length.is_none_or(|length| bytes.len() >= headers_end + 4 + length) {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&bytes);
                let (task, status, headers, content) = if request.contains("记忆冲突检测器")
                {
                    ("dedup", 200, "", "[]".to_owned())
                } else if request.contains("Memory Consolidation Architect") {
                    ("l2", 200, "", r#"{"scenes":[]}"#.to_owned())
                } else if request.contains("Persona Architect") {
                    ("l3", 200, "", "# Test Persona".to_owned())
                } else {
                    let call = state.l1_calls.fetch_add(1, Ordering::SeqCst);
                    let ids = l0_ids(&request);
                    state.l1_id_counts.lock().unwrap().push(ids.len());
                    if call == 0 {
                        state.first_503.add_permits(1);
                        (
                            "l1",
                            503,
                            "retry-after-ms: 1000\r\n",
                            r#"{"error":{"message":"temporary"}}"#.to_owned(),
                        )
                    } else if call == 1 {
                        // Block the retry until the test releases it
                        let _ = state.retry_blocked.acquire().await;
                        let content = serde_json::json!([{
                            "scene_name": "Differential Test Scene",
                            "message_ids": l0_ids(&request),
                            "memories": [{
                                "content": "The user is running the differential parity test",
                                "type": "episodic",
                                "priority": 80,
                                "source_message_ids": l0_ids(&request),
                                "metadata": {}
                            }]
                        }])
                        .to_string();
                        ("l1", 200, "", content)
                    } else {
                        let content = serde_json::json!([{
                            "scene_name": "Differential Test Scene",
                            "message_ids": ids,
                            "memories": [{
                                "content": "The user is running the differential parity test",
                                "type": "episodic",
                                "priority": 80,
                                "source_message_ids": ids,
                                "metadata": {}
                            }]
                        }])
                        .to_string();
                        ("l1", 200, "", content)
                    }
                };
                state
                    .events
                    .lock()
                    .unwrap()
                    .push(format!("{task}:{status}"));
                let body = serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": content}}]
                })
                .to_string();
                let reason = if status == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{headers}connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}/v1"), state)
}

async fn capture(service: &dyn AeonMemoryService, session: &str, turn: u32) {
    service
        .capture(CaptureRequest {
            user_content: format!("turn {turn}: remember parity value {turn}"),
            assistant_content: "acknowledged".into(),
            session_key: session.into(),
            session_id: Some(session.into()),
            user_id: None,
            messages: None,
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_during_capture_matches_ts_dedup_and_durable_state() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("aeon-memory-dedup-retry-{unique}"));
    let (base_url, mock) = retrying_llm_server().await;
    let mut config = GatewayConfig::default();
    config.data.base_dir = root.to_string_lossy().into_owned();
    config.llm.base_url = base_url;
    config.llm.api_key = "test-key".into();
    config.llm.model = "mock".into();
    config.memory.embedding.enabled = false;
    config.memory.pipeline.enable_warmup = false;
    config.memory.pipeline.every_n_conversations = 100;
    config.memory.pipeline.l1_idle_timeout_seconds = 60;
    config.memory.pipeline.l2_delay_after_l1_seconds = 0;
    config.memory.pipeline.l2_min_interval_seconds = 0;
    let service = build_core(&config).await.unwrap();

    capture(service.as_ref(), "s0", 0).await;
    let flushing = {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            service
                .end_session(SessionEndRequest {
                    session_key: "s0".into(),
                    user_id: None,
                })
                .await
                .unwrap();
        })
    };
    // Wait for the first L1 call to get the 503 response
    let permit = tokio::time::timeout(Duration::from_secs(5), mock.first_503.acquire())
        .await
        .unwrap()
        .unwrap();
    permit.forget();

    // The L1 retry is now blocked on retry_blocked semaphore.
    // Capture must complete even while the retry is still in-flight.
    tokio::time::timeout(Duration::from_secs(5), capture(service.as_ref(), "s0", 2))
        .await
        .expect("capture must not wait behind an in-flight L1 retry");

    // Now release the retry so the end_session can finish
    mock.retry_blocked.add_permits(1);
    capture(service.as_ref(), "s1", 1).await;
    capture(service.as_ref(), "s1", 3).await;
    let mut before_flush =
        aeon_memory_store_sqlite::VectorStore::new(&root.join("vectors.db").to_string_lossy(), 0);
    before_flush.init(None).unwrap();
    assert_eq!(
        before_flush.query_l0_for_l1("s1", None, 50).unwrap().len(),
        4,
        "both awaited captures must persist all s1 L0 rows before flush"
    );
    drop(before_flush);
    flushing.await.unwrap();
    service
        .end_session(SessionEndRequest {
            session_key: "s1".into(),
            user_id: None,
        })
        .await
        .unwrap();
    service.shutdown().await.unwrap();

    let checkpoint = read_checkpoint(&root.to_string_lossy()).unwrap();
    assert_eq!(checkpoint.total_memories_extracted, 2);
    assert_eq!(checkpoint.memories_since_last_persona, 2);
    assert!(checkpoint.runner_states["s0"].last_l1_cursor > 0);
    assert!(checkpoint.runner_states["s1"].last_l1_cursor > 0);

    let mut store =
        aeon_memory_store_sqlite::VectorStore::new(&root.join("vectors.db").to_string_lossy(), 0);
    store.init(None).unwrap();
    let rows = store.query_l1_records(&L1QueryFilter::default()).unwrap();
    assert_eq!(rows.len(), 2, "TS stores one L1 record per session");
    assert_eq!(rows.iter().filter(|row| row.session_key == "s0").count(), 1);
    assert_eq!(rows.iter().filter(|row| row.session_key == "s1").count(), 1);
    assert!(
        store
            .search_l1_fts("differential OR parity OR test", 10)
            .unwrap()
            .len()
            >= 2,
        "L1 candidates remain searchable after L2/L3"
    );

    let records_path = root
        .join("records")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let records = std::fs::read_to_string(records_path).unwrap();
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    for record in &records {
        let ids = record["source_message_ids"].as_array().unwrap();
        let unique = ids.iter().collect::<HashSet<_>>();
        assert_eq!(ids.len(), unique.len(), "source ids must not repeat");
        let expected = if record["sessionKey"] == "s0" { 2 } else { 4 };
        assert_eq!(ids.len(), expected, "L0 groups must stay turn-complete");
    }

    let events = mock.events.lock().unwrap().clone();
    assert!(events.starts_with(&["l1:503".into(), "l1:200".into()]));
    assert_eq!(
        events.iter().filter(|event| *event == "dedup:200").count(),
        1
    );
    assert_eq!(
        *mock.l1_id_counts.lock().unwrap(),
        [2, 2, 4],
        "s0 initial/retry and s1 L1 requests must receive complete L0 groups"
    );
    let _ = std::fs::remove_dir_all(root);
}
