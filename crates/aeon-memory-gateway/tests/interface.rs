use std::sync::{Arc, Mutex};

use aeon_memory_gateway::{AeonMemoryService, AppConfig, ROUTES, app, cli, service::*};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use clap::Parser;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[derive(Default)]
struct RecordingService {
    calls: Mutex<Vec<String>>,
}

impl RecordingService {
    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }
}

#[async_trait]
impl AeonMemoryService for RecordingService {
    async fn health(&self) -> ServiceResult<HealthResponse> {
        self.record("health");
        Ok(HealthResponse {
            status: "ok".into(),
            version: "0.1.0".into(),
            uptime: 3,
            stores: StoreHealth {
                vector_store: true,
                embedding_service: true,
            },
        })
    }
    async fn recall(&self, r: RecallRequest) -> ServiceResult<RecallResponse> {
        self.record(format!("recall:{}:{}", r.query, r.session_key));
        if r.query == "empty" {
            return Ok(RecallResponse {
                context: String::new(),
                prepend_context: None,
                strategy: None,
                memory_count: 0,
            });
        }
        Ok(RecallResponse {
            context: "stable official context".into(),
            prepend_context: Some("dynamic official context".into()),
            strategy: Some("hybrid".into()),
            memory_count: 1,
        })
    }
    async fn capture(&self, r: CaptureRequest) -> ServiceResult<CaptureResponse> {
        self.record(format!("capture:{}:{}", r.user_content, r.session_key));
        Ok(CaptureResponse {
            l0_recorded: 2,
            scheduler_notified: true,
        })
    }
    async fn search_memories(&self, r: MemorySearchRequest) -> ServiceResult<MemorySearchResponse> {
        self.record(format!("search-memories:{}", r.query));
        Ok(MemorySearchResponse {
            results: "m".into(),
            total: 1,
            strategy: "hybrid".into(),
        })
    }
    async fn search_conversations(
        &self,
        r: ConversationSearchRequest,
    ) -> ServiceResult<ConversationSearchResponse> {
        self.record(format!("search-conversations:{}", r.query));
        Ok(ConversationSearchResponse {
            results: "c".into(),
            total: 1,
        })
    }
    async fn end_session(&self, r: SessionEndRequest) -> ServiceResult<SessionEndResponse> {
        self.record(format!("session-end:{}", r.session_key));
        Ok(SessionEndResponse { flushed: true })
    }
    async fn seed(&self, _r: SeedRequest) -> ServiceResult<SeedResponse> {
        self.record("seed");
        Ok(SeedResponse {
            sessions_processed: 1,
            rounds_processed: 1,
            messages_processed: 2,
            l0_recorded: 2,
            duration_ms: 1,
            output_dir: "/tmp/out".into(),
        })
    }
    async fn before_prompt(&self, r: BeforePromptRequest) -> ServiceResult<BeforePromptResponse> {
        self.record(format!("before-prompt:{}:{}", r.agent_id, r.session_id));
        Ok(BeforePromptResponse {
            messages: r.messages,
            context: serde_json::json!({"totalTokens": 3, "messagesTokens": 2}),
            active_mmd: None,
            offload_enabled: true,
            l1_entries: vec![],
            l15_judgment: None,
            l2_updated: false,
            compression: serde_json::json!({"applied": false, "mode": "none"}),
        })
    }
    async fn after_tool(&self, r: AfterToolRequest) -> ServiceResult<AfterToolResponse> {
        self.record(format!("after-tool:{}", r.tool.tool_call_id));
        Ok(AfterToolResponse {
            messages: r.messages,
            buffered_pairs: 1,
            l1_entries: vec![],
            l2_updated: false,
            context: serde_json::json!({"totalTokens": 4}),
            compression: serde_json::json!({"mode": "none", "tokensSaved": 0}),
        })
    }
    async fn llm_output(&self, r: LlmOutputRequest) -> ServiceResult<LlmOutputResponse> {
        self.record(format!("llm-output:{}", r.session_id));
        Ok(LlmOutputResponse {
            force_l1: false,
            processed_entries: 0,
            l1_entries: vec![],
            l2_updated: false,
            state: Default::default(),
        })
    }
    async fn status(&self) -> ServiceResult<StatusResponse> {
        self.record("status");
        Ok(StatusResponse {
            l0_records: 2,
            l1_records: 1,
            sessions: 1,
        })
    }
    async fn show_persona(&self) -> ServiceResult<String> {
        self.record("show-persona");
        Ok("persona".into())
    }
    async fn show_scenes(&self) -> ServiceResult<Vec<String>> {
        self.record("show-scenes");
        Ok(vec!["scene".into()])
    }
}

fn post(path: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[test]
fn production_surface_is_exactly_ten_routes() {
    assert_eq!(ROUTES.len(), 10);
    assert!(ROUTES.len() <= 10);
    assert_eq!(
        ROUTES.iter().filter(|(method, _)| *method == "GET").count(),
        1
    );
    assert_eq!(
        ROUTES
            .iter()
            .filter(|(method, _)| *method == "POST")
            .count(),
        9
    );
}

#[tokio::test]
async fn offload_http_routes_preserve_contract_shapes_and_dispatch() {
    let service = Arc::new(RecordingService::default());
    let router = app(service.clone(), AppConfig::default());
    let cases = [
        (
            "/offload/before-prompt",
            r#"{"agent_id":"main","session_id":"s1","system_prompt":"sys","user_prompt":"u","messages":[],"context_window":200000}"#,
            "before-prompt:main:s1",
        ),
        (
            "/offload/after-tool",
            r#"{"agent_id":"main","session_id":"s1","tool":{"toolName":"read","toolCallId":"call_1","params":{},"result":{},"error":null,"timestamp":"2026-07-13T00:00:00Z","durationMs":4},"messages":[],"context_window":200000}"#,
            "after-tool:call_1",
        ),
        (
            "/offload/llm-output",
            r#"{"agent_id":"main","session_id":"s1","assistant_message":{},"usage":{"input_tokens":1},"finish_reason":"tool_use"}"#,
            "llm-output:s1",
        ),
    ];
    for (path, body, expected) in cases {
        let response = router.clone().oneshot(post(path, body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(service.calls.lock().unwrap().last().unwrap(), expected);
    }
}

#[tokio::test]
async fn health_is_public_but_post_routes_require_configured_bearer_token() {
    let service = Arc::new(RecordingService::default());
    let router = app(
        service,
        AppConfig {
            api_key: Some("secret".into()),
            cors_origins: vec![],
        },
    );
    let health = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let missing = router
        .clone()
        .oneshot(post("/recall", r#"{"query":"q","session_key":"s"}"#))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(missing).await["error"],
        "Unauthorized: missing Bearer token"
    );

    let valid = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/recall")
                .header("content-type", "application/json")
                .header("authorization", "Bearer secret")
                .body(Body::from(r#"{"query":"q","session_key":"s"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(valid.status(), StatusCode::OK);
    let json = body_json(valid).await;
    assert_eq!(json["context"], "stable official context");
    assert_eq!(json["prepend_context"], "dynamic official context");
    assert_eq!(json["memory_count"], 1);
    assert_eq!(json["strategy"], "hybrid");
    assert_eq!(json.as_object().unwrap().len(), 4);
}

#[tokio::test]
async fn recall_http_body_preserves_pinned_ts_fields_with_additive_dynamic_context() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/recall_http_oracle.json")).unwrap();
    let router = app(Arc::new(RecordingService::default()), AppConfig::default());

    for (query, fixture) in [("empty", &oracle["empty"]), ("q", &oracle["split"])] {
        let response = router
            .clone()
            .oneshot(post(
                "/recall",
                &serde_json::json!({"query": query, "session_key": "s"}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            fixture["status"].as_u64().unwrap() as u16
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let mut actual = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        let dynamic = actual.as_object_mut().unwrap().remove("prepend_context");
        assert_eq!(actual, fixture["body"]);
        if query == "q" {
            assert_eq!(dynamic, Some(serde_json::json!("dynamic official context")));
        } else {
            assert!(dynamic.is_none());
        }
    }
}

#[tokio::test]
async fn invalid_input_and_unknown_routes_use_compatible_json_errors() {
    let router = app(Arc::new(RecordingService::default()), AppConfig::default());
    let invalid = router
        .clone()
        .oneshot(post("/search/memories", r#"{"query":""}"#))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(invalid).await["error"],
        "Missing required field: query"
    );

    let malformed = router.clone().oneshot(post("/capture", "{")).await.unwrap();
    assert_eq!(malformed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        body_json(malformed).await,
        serde_json::json!({"error": "Invalid JSON body"})
    );

    for (path, message) in [
        ("/recall", "Missing required fields: query, session_key"),
        (
            "/capture",
            "Missing required fields: user_content, assistant_content, session_key",
        ),
        ("/search/memories", "Missing required field: query"),
        ("/search/conversations", "Missing required field: query"),
        ("/session/end", "Missing required field: session_key"),
        ("/seed", "Missing required field: data"),
    ] {
        let response = router.clone().oneshot(post(path, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"error": message}),
            "{path}"
        );
    }

    let missing = router
        .oneshot(
            Request::builder()
                .uri("/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(missing).await["error"], "Not found: GET /stats");
}

#[tokio::test]
async fn cors_matches_ts_for_empty_allowlist_allowed_denied_and_preflight() {
    let service = Arc::new(RecordingService::default());
    let strict_router = app(service.clone(), AppConfig::default());
    let strict = strict_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("origin", "https://ui.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        strict
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    assert!(strict.headers().get("vary").is_none());
    let strict_preflight = strict_router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/recall")
                .header("origin", "https://ui.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(strict_preflight.status(), StatusCode::NO_CONTENT);
    assert!(
        strict_preflight
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    assert!(
        strict_preflight
            .headers()
            .get("access-control-allow-methods")
            .is_none()
    );
    assert!(strict_preflight.headers().get("vary").is_none());

    let allowlist_router = app(
        service,
        AppConfig {
            api_key: None,
            cors_origins: vec!["https://ui.example".into()],
        },
    );
    let allowed = allowlist_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("origin", "https://ui.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        "https://ui.example"
    );
    assert_eq!(
        allowed.headers()["access-control-allow-methods"],
        "GET, POST, OPTIONS"
    );
    assert_eq!(
        allowed.headers()["access-control-allow-headers"],
        "Content-Type, Authorization"
    );
    assert_eq!(allowed.headers()["vary"], "Origin");

    let denied = allowlist_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("origin", "https://denied.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::OK);
    assert!(
        denied
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    assert!(
        denied
            .headers()
            .get("access-control-allow-methods")
            .is_none()
    );
    assert_eq!(denied.headers()["vary"], "Origin");

    let missing_origin = allowlist_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        missing_origin
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    assert_eq!(missing_origin.headers()["vary"], "Origin");

    let allowed_preflight = allowlist_router
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/recall")
                .header("origin", "https://ui.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed_preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        allowed_preflight.headers()["access-control-allow-origin"],
        "https://ui.example"
    );
    assert_eq!(allowed_preflight.headers()["vary"], "Origin");

    let denied_preflight = allowlist_router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/recall")
                .header("origin", "https://denied.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_preflight.status(), StatusCode::NO_CONTENT);
    assert!(
        denied_preflight
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    assert!(
        denied_preflight
            .headers()
            .get("access-control-allow-methods")
            .is_none()
    );
    assert_eq!(denied_preflight.headers()["vary"], "Origin");
}

#[tokio::test]
async fn wildcard_cors_matches_ts_without_vary() {
    let router = app(
        Arc::new(RecordingService::default()),
        AppConfig {
            api_key: None,
            cors_origins: vec!["*".into()],
        },
    );
    let response = router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/anything")
                .header("origin", "https://any.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        response.headers()["access-control-allow-methods"],
        "GET, POST, OPTIONS"
    );
    assert_eq!(
        response.headers()["access-control-allow-headers"],
        "Content-Type, Authorization"
    );
    assert!(response.headers().get("vary").is_none());
}

#[tokio::test]
async fn clap_commands_map_to_service_operations() {
    let service = Arc::new(RecordingService::default());
    let cases = [
        (
            vec![
                "aeon-memory",
                "capture",
                "--user",
                "u",
                "--assistant",
                "a",
                "--session-key",
                "s",
            ],
            "capture:u:s",
        ),
        (
            vec![
                "aeon-memory",
                "recall",
                "--query",
                "q",
                "--session-key",
                "s",
            ],
            "recall:q:s",
        ),
        (
            vec!["aeon-memory", "search", "memories", "--query", "q"],
            "search-memories:q",
        ),
        (
            vec!["aeon-memory", "search", "conversations", "--query", "q"],
            "search-conversations:q",
        ),
        (
            vec!["aeon-memory", "session", "end", "--session-key", "s"],
            "session-end:s",
        ),
        (vec!["aeon-memory", "status"], "status"),
        (vec!["aeon-memory", "show", "persona"], "show-persona"),
        (vec!["aeon-memory", "show", "scenes"], "show-scenes"),
    ];
    for (argv, expected) in cases {
        cli::execute(service.clone(), cli::Cli::try_parse_from(argv).unwrap())
            .await
            .unwrap();
        assert_eq!(service.calls.lock().unwrap().last().unwrap(), expected);
    }
}

#[tokio::test]
async fn seed_cli_reads_json_and_maps_to_service() {
    let service = Arc::new(RecordingService::default());
    let input = std::env::temp_dir().join(format!(
        "aeon-memory-gateway-seed-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&input, r#"{"sessions":[]}"#).unwrap();
    let args = [
        "aeon-memory",
        "seed",
        "--input",
        input.to_str().unwrap(),
        "--session-key",
        "fallback",
    ];
    cli::execute(service.clone(), cli::Cli::try_parse_from(args).unwrap())
        .await
        .unwrap();
    std::fs::remove_file(input).unwrap();
    assert_eq!(service.calls.lock().unwrap().last().unwrap(), "seed");
}

#[tokio::test]
async fn offload_cli_reads_contract_json_and_maps_all_operations() {
    let service = Arc::new(RecordingService::default());
    let dir = std::env::temp_dir().join(format!(
        "aeon-memory-gateway-offload-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let cases = [
        (
            "before-prompt",
            r#"{"agent_id":"a","session_id":"s","system_prompt":"","user_prompt":"","messages":[],"context_window":8}"#,
            "before-prompt:a:s",
        ),
        (
            "after-tool",
            r#"{"agent_id":"a","session_id":"s","tool":{"toolName":"read","toolCallId":"c","params":{},"result":{},"error":null,"timestamp":"2026-07-13T00:00:00Z","durationMs":1},"messages":[],"context_window":8}"#,
            "after-tool:c",
        ),
        (
            "llm-output",
            r#"{"agent_id":"a","session_id":"s","assistant_message":{},"usage":null,"finish_reason":null}"#,
            "llm-output:s",
        ),
    ];
    for (operation, body, expected) in cases {
        let input = dir.join(format!("{operation}.json"));
        std::fs::write(&input, body).unwrap();
        let cli = cli::Cli::try_parse_from([
            "aeon-memory",
            "offload",
            operation,
            "--input",
            input.to_str().unwrap(),
        ])
        .unwrap();
        cli::execute(service.clone(), cli).await.unwrap();
        assert_eq!(service.calls.lock().unwrap().last().unwrap(), expected);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn required_cli_values_are_rejected_by_clap() {
    assert!(cli::Cli::try_parse_from(["aeon-memory", "recall", "--query", "q"]).is_err());
    assert!(cli::Cli::try_parse_from(["aeon-memory", "capture", "--user", "u"]).is_err());
}
