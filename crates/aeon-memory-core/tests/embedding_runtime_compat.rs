use aeon_memory_core::AeonMemoryCoreError;
use aeon_memory_core::embedding::openai::{OpenAiEmbeddingConfig, OpenAiEmbeddingService};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream.read(&mut chunk).unwrap();
        bytes.extend_from_slice(&chunk[..count]);
        let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..end]);
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if bytes.len() >= end + 4 + length {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

fn write_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn normalize_integral_json_numbers(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_integral_json_numbers),
        Value::Object(values) => values
            .values_mut()
            .for_each(normalize_integral_json_numbers),
        Value::Number(number) => {
            if let Some(number) = number.as_f64()
                && number.fract() == 0.0
            {
                *value = serde_json::json!(number as i64);
            }
        }
        _ => {}
    }
}

#[test]
fn pinned_ts_remote_embedding_transcript_covers_protocol_contract() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/embedding_runtime_oracle.json")).unwrap();
    let requests = oracle["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 4, "empty batch must not issue HTTP");
    let success = requests.iter().find(|r| r["name"] == "success").unwrap();
    assert_eq!(success["authorization"], "Bearer secret");
    assert_eq!(success["body"]["input"], serde_json::json!(["A😀", "xy"]));
    assert_eq!(success["body"]["model"], "fixture-model");
    assert_eq!(success["body"]["dimensions"], 3);
    let output = oracle["output"].as_array().unwrap();
    let success = output.iter().find(|r| r["name"] == "success").unwrap();
    assert_eq!(
        success["value"],
        serde_json::json!([[0.6000000238418579, 0, 0.800000011920929], [0, 1]])
    );
    assert_eq!(
        output.iter().find(|r| r["name"] == "empty").unwrap()["value"],
        serde_json::json!([])
    );
    assert_eq!(
        output.iter().find(|r| r["name"] == "empty_batch").unwrap()["value"],
        serde_json::json!([])
    );
    assert!(
        output.iter().find(|r| r["name"] == "missing").unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("missing 'data' array")
    );
    let malformed = output.iter().find(|r| r["name"] == "malformed").unwrap();
    assert_eq!(malformed["ok"], true);
    assert_eq!(malformed["value"], serde_json::json!([[1, 0]]));
}

#[test]
fn rust_http_transcript_matches_pinned_ts_oracle() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/embedding_runtime_oracle.json")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let name = path.split('/').nth(1).unwrap().to_string();
            let body = request.split("\r\n\r\n").nth(1).unwrap();
            let authorization = request
                .lines()
                .find_map(|line| {
                    let (header, value) = line.split_once(':')?;
                    header
                        .eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_string())
                })
                .unwrap();
            requests.push(serde_json::json!({
                "name": name,
                "authorization": authorization,
                "body": serde_json::from_str::<Value>(body).unwrap(),
            }));
            let response = match name.as_str() {
                "success" => {
                    r#"{"data":[{"index":1,"embedding":[0,5]},{"index":0,"embedding":[3,0,4]}]}"#
                }
                "empty" => r#"{"data":[]}"#,
                "missing" => r#"{}"#,
                "malformed" => r#"{"data":[{"index":0,"embedding":[1,"invalid"]}]}"#,
                _ => unreachable!(),
            };
            write_response(&mut stream, response);
        }
        requests
    });

    let mut output = Vec::new();
    for name in ["success", "empty", "missing", "malformed"] {
        let service = OpenAiEmbeddingService::new(OpenAiEmbeddingConfig {
            provider: "openai".into(),
            base_url: format!("http://{address}/{name}"),
            proxy_url: None,
            api_key: "secret".into(),
            model: "fixture-model".into(),
            dimensions: 3,
            send_dimensions: true,
            max_input_chars: 3,
            timeout_ms: 1_000,
        });
        let texts = if name == "success" {
            vec!["A😀B".to_string(), "xy".to_string()]
        } else {
            vec!["x".to_string()]
        };
        match service.embed_batch(&texts) {
            Ok(value) => output.push(serde_json::json!({"name": name, "ok": true, "value": value})),
            Err(error) => {
                let message = match error {
                    AeonMemoryCoreError::Embedding(message) => message,
                    other => other.to_string(),
                };
                output.push(serde_json::json!({"name": name, "ok": false, "error": message}));
            }
        }
    }
    let empty = OpenAiEmbeddingService::new(OpenAiEmbeddingConfig {
        provider: "openai".into(),
        base_url: "http://127.0.0.1:1/unused".into(),
        proxy_url: None,
        api_key: "secret".into(),
        model: "fixture-model".into(),
        dimensions: 3,
        send_dimensions: true,
        max_input_chars: 5_000,
        timeout_ms: 1_000,
    });
    output.push(
        serde_json::json!({"name":"empty_batch","ok":true,"value":empty.embed_batch(&[]).unwrap()}),
    );

    assert_eq!(
        serde_json::json!(server.join().unwrap()),
        oracle["requests"]
    );
    let mut output = serde_json::json!(output);
    normalize_integral_json_numbers(&mut output);
    assert_eq!(output, oracle["output"]);
}
