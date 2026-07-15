use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn bind_llm() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let mut raw = vec![0_u8; 64 * 1024];
            let read = stream.read(&mut raw).unwrap_or_default();
            let request = String::from_utf8_lossy(&raw[..read]);
            let content = if request.contains("call-1") {
                r#"[{"tool_call_id":"call-1","tool_call":"shell","summary":"listed files","score":8}]"#
            } else {
                "[]"
            };
            let body = serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

fn bind_pipeline_llm() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let mut raw = vec![0_u8; 128 * 1024];
            let read = stream.read(&mut raw).unwrap_or_default();
            let request = String::from_utf8_lossy(&raw[..read]);
            let content = if request.contains("Memory Consolidation Architect") {
                r#"{"scenes":[]}"#
            } else if request.contains("Persona Architect") {
                "# Shutdown Test Persona"
            } else if request.contains("记忆冲突检测器") {
                "[]"
            } else {
                r#"[{"scene_name":"Graceful Shutdown Scene","message_ids":["mock"],"memories":[{"content":"pending shutdown memory","type":"episodic","priority":80,"source_message_ids":["mock"],"metadata":{}}]}]"#
            };
            let body = serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.lines().skip(1).find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
    }
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
    origin: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let body = body.unwrap_or("");
    let mut headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(token) = token {
        headers.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(origin) = origin {
        headers.push_str(&format!("Origin: {origin}\r\n"));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    Ok(HttpResponse {
        status,
        headers: head.to_owned(),
        body: body.to_owned(),
    })
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if request(port, "GET", "/health", None, None, None).is_ok_and(|r| r.status == 200) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("aeon-memory-server did not become ready");
}

fn spawn_server(config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_aeon-memory-server"))
        .args(["--config", &config.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn stop_gracefully(child: &mut Child) {
    stop_with_signal(child, "-INT");
}

fn stop_with_signal(child: &mut Child, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "server did not shut down cleanly: {status}"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("server ignored graceful {signal}");
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "Unix signal handling not available on Windows"
)]
fn real_server_covers_ten_routes_security_cors_shutdown_and_restart() {
    let version = Command::new(env!("CARGO_BIN_EXE_aeon-memory-server"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("aeon-memory-server {}", env!("CARGO_PKG_VERSION"))
    );

    let root = std::env::temp_dir().join(format!("aeon-memory-process-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let server_port = free_port();
    let (llm_port, _llm) = bind_llm();
    let config = root.join("aeon-memory.yaml");
    fs::write(
        &config,
        format!(
            r#"server:
  host: 127.0.0.1
  port: {server_port}
  apiKey: secret
  corsOrigins: ["https://allowed.example"]
data:
  baseDir: "{0}"
llm:
  baseUrl: "http://127.0.0.1:{llm_port}/v1"
  apiKey: test
  model: mock
memory:
  timezone: UTC
  storeBackend: sqlite
  pipeline:
    everyNConversations: 100
    enableWarmup: false
  recall:
    strategy: keyword
  embedding:
    enabled: false
  offload:
    enabled: true
    dataDir: "{0}/offload"
    forceTriggerThreshold: 99
"#,
            root.display()
        ),
    )
    .unwrap();

    let mut server = spawn_server(&config);
    wait_ready(server_port);

    let health = request(
        server_port,
        "GET",
        "/health",
        None,
        None,
        Some("https://allowed.example"),
    )
    .unwrap();
    assert_eq!(health.status, 200);
    let health_body: serde_json::Value = serde_json::from_str(&health.body).unwrap();
    assert_eq!(health_body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        health.header("access-control-allow-origin"),
        Some("https://allowed.example")
    );
    assert_eq!(health.header("vary"), Some("Origin"));
    let denied_origin = request(
        server_port,
        "GET",
        "/health",
        None,
        None,
        Some("https://denied.example"),
    )
    .unwrap();
    assert_eq!(denied_origin.status, 200);
    assert_eq!(denied_origin.header("access-control-allow-origin"), None);
    assert_eq!(denied_origin.header("vary"), Some("Origin"));
    let allowed_preflight = request(
        server_port,
        "OPTIONS",
        "/recall",
        None,
        None,
        Some("https://allowed.example"),
    )
    .unwrap();
    assert_eq!(allowed_preflight.status, 204);
    assert_eq!(
        allowed_preflight.header("access-control-allow-origin"),
        Some("https://allowed.example")
    );
    assert_eq!(
        allowed_preflight.header("access-control-allow-methods"),
        Some("GET, POST, OPTIONS")
    );
    assert_eq!(
        allowed_preflight.header("access-control-allow-headers"),
        Some("Content-Type, Authorization")
    );
    assert_eq!(allowed_preflight.header("vary"), Some("Origin"));
    let denied_preflight = request(
        server_port,
        "OPTIONS",
        "/recall",
        None,
        None,
        Some("https://denied.example"),
    )
    .unwrap();
    assert_eq!(denied_preflight.status, 204);
    assert_eq!(denied_preflight.header("access-control-allow-origin"), None);
    assert_eq!(denied_preflight.header("vary"), Some("Origin"));
    assert_eq!(
        request(
            server_port,
            "POST",
            "/recall",
            Some(r#"{"query":"q","session_key":"s"}"#),
            None,
            None
        )
        .unwrap()
        .status,
        401
    );
    assert_eq!(
        request(
            server_port,
            "POST",
            "/recall",
            Some(r#"{"query":"q","session_key":"s"}"#),
            Some("wrong"),
            None
        )
        .unwrap()
        .status,
        401
    );

    let capture = request(server_port, "POST", "/capture", Some(r#"{"user_content":"remember cobalt bicycle","assistant_content":"noted","session_key":"s","session_id":"sid"}"#), Some("secret"), None).unwrap();
    assert_eq!(capture.status, 200, "{}", capture.body);
    let cases = [
        ("/recall", r#"{"query":"cobalt bicycle","session_key":"s"}"#),
        ("/search/memories", r#"{"query":"cobalt"}"#),
        (
            "/search/conversations",
            r#"{"query":"cobalt","session_key":"s"}"#,
        ),
        ("/session/end", r#"{"session_key":"s"}"#),
        (
            "/seed",
            r#"{"data":{"sessions":[{"sessionKey":"seed-s","sessionId":"seed-id","conversations":[[{"role":"user","content":"seed fact","timestamp":1783900000000},{"role":"assistant","content":"ok","timestamp":1783900000001}]]}]}}"#,
        ),
        (
            "/offload/before-prompt",
            r#"{"agent_id":"a","session_id":"os","system_prompt":"sys","user_prompt":"u","messages":[],"context_window":200000}"#,
        ),
        (
            "/offload/after-tool",
            r#"{"agent_id":"a","session_id":"os","tool":{"toolName":"shell","toolCallId":"call-1","params":{"cmd":"ls"},"result":"file.txt","error":null,"timestamp":"2026-07-13T00:00:00Z","durationMs":1},"messages":[],"context_window":200000}"#,
        ),
        (
            "/offload/llm-output",
            r#"{"agent_id":"a","session_id":"os","assistant_message":{"role":"assistant","content":"done"}}"#,
        ),
    ];
    for (path, body) in cases {
        let response = request(server_port, "POST", path, Some(body), Some("secret"), None)
            .unwrap_or_else(|error| panic!("{path} request failed: {error}"));
        assert_eq!(response.status, 200, "{path}: {}", response.body);
    }
    assert!(root.join("vectors.db").is_file());
    assert!(
        root.join("conversations")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    assert!(
        !root.join("offload/a/offload-os.jsonl").is_file(),
        "llm_output is observational and the below-threshold pending pair is memory-only"
    );
    stop_gracefully(&mut server);

    let mut restarted = spawn_server(&config);
    wait_ready(server_port);
    let persisted = request(
        server_port,
        "POST",
        "/search/conversations",
        Some(r#"{"query":"cobalt","session_key":"s"}"#),
        Some("secret"),
        None,
    )
    .unwrap();
    assert_eq!(persisted.status, 200, "{}", persisted.body);
    assert!(
        serde_json::from_str::<serde_json::Value>(&persisted.body).unwrap()["total"]
            .as_u64()
            .unwrap()
            >= 1
    );
    stop_gracefully(&mut restarted);

    let without_cors = fs::read_to_string(&config)
        .unwrap()
        .replace("  corsOrigins: [\"https://allowed.example\"]\n", "");
    fs::write(&config, without_cors).unwrap();
    let mut strict = spawn_server(&config);
    wait_ready(server_port);
    let strict_preflight = request(
        server_port,
        "OPTIONS",
        "/recall",
        None,
        None,
        Some("https://allowed.example"),
    )
    .unwrap();
    assert_eq!(strict_preflight.status, 204);
    assert_eq!(strict_preflight.header("access-control-allow-origin"), None);
    assert_eq!(
        strict_preflight.header("access-control-allow-methods"),
        None
    );
    assert_eq!(strict_preflight.header("vary"), None);
    stop_gracefully(&mut strict);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "Unix signal handling not available on Windows"
)]
fn sigint_flushes_pending_l2_before_real_server_exits() {
    let root = std::env::temp_dir().join(format!(
        "aeon-memory-process-shutdown-e2e-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let server_port = free_port();
    let (llm_port, _llm) = bind_pipeline_llm();
    let config = root.join("aeon-memory.yaml");
    fs::write(
        &config,
        format!(
            r#"server:
  host: 127.0.0.1
  port: {server_port}
data:
  baseDir: "{}"
llm:
  baseUrl: "http://127.0.0.1:{llm_port}/v1"
  apiKey: test
  model: mock
  timeoutMs: 10000
memory:
  timezone: UTC
  storeBackend: sqlite
  pipeline:
    everyNConversations: 5
    enableWarmup: true
    l2DelayAfterL1Seconds: 3600
    l2MinIntervalSeconds: 0
  recall:
    strategy: keyword
  embedding:
    enabled: false
"#,
            root.display()
        ),
    )
    .unwrap();

    let mut server = spawn_server(&config);
    wait_ready(server_port);
    for session in ["shutdown-alpha", "shutdown-beta"] {
        let response = request(
            server_port,
            "POST",
            "/capture",
            Some(&format!(
                r#"{{"user_content":"remember {session}","assistant_content":"noted","session_key":"{session}","session_id":"{session}"}}"#
            )),
            None,
            None,
        )
        .unwrap();
        assert_eq!(response.status, 200, "{}", response.body);
    }

    let pending_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let checkpoint =
            aeon_memory_core::pipeline::checkpoint::read_checkpoint(&root.to_string_lossy())
                .unwrap();
        let both_pending = ["shutdown-alpha", "shutdown-beta"].iter().all(|session| {
            checkpoint
                .pipeline_states
                .get(*session)
                .is_some_and(|state| state.l2_pending_l1_count == 1)
        });
        if both_pending {
            assert_eq!(checkpoint.scenes_processed, 0);
            break;
        }
        assert!(
            Instant::now() < pending_deadline,
            "both sessions never reached pending L2: {checkpoint:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    stop_gracefully(&mut server);

    let checkpoint =
        aeon_memory_core::pipeline::checkpoint::read_checkpoint(&root.to_string_lossy()).unwrap();
    assert_eq!(checkpoint.total_memories_extracted, 2);
    assert_eq!(checkpoint.scenes_processed, 2);
    for session in ["shutdown-alpha", "shutdown-beta"] {
        let state = &checkpoint.pipeline_states[session];
        assert_eq!(state.l2_pending_l1_count, 0);
        assert!(!state.last_extraction_time.is_empty());
        assert!(!state.last_extraction_updated_time.is_empty());
    }
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "Unix signal handling not available on Windows"
)]
fn sigterm_gracefully_drains_pending_pipeline_and_exits_zero() {
    let root = std::env::temp_dir().join(format!(
        "aeon-memory-process-sigterm-e2e-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let server_port = free_port();
    let (llm_port, _llm) = bind_pipeline_llm();
    let config = root.join("aeon-memory.yaml");
    fs::write(
        &config,
        format!(
            r#"server:
  host: 127.0.0.1
  port: {server_port}
data:
  baseDir: "{}"
llm:
  baseUrl: "http://127.0.0.1:{llm_port}/v1"
  apiKey: test
  model: mock
memory:
  timezone: UTC
  pipeline:
    everyNConversations: 5
    enableWarmup: false
  embedding:
    enabled: false
"#,
            root.display()
        ),
    )
    .unwrap();

    let mut server = spawn_server(&config);
    wait_ready(server_port);
    let response = request(
        server_port,
        "POST",
        "/capture",
        Some(
            r#"{"user_content":"remember SIGTERM","assistant_content":"noted","session_key":"sigterm","session_id":"sigterm"}"#,
        ),
        None,
        None,
    )
    .unwrap();
    assert_eq!(response.status, 200, "{}", response.body);

    stop_with_signal(&mut server, "-TERM");

    let checkpoint =
        aeon_memory_core::pipeline::checkpoint::read_checkpoint(&root.to_string_lossy()).unwrap();
    assert_eq!(checkpoint.total_memories_extracted, 1);
    let _ = fs::remove_dir_all(root);
}
