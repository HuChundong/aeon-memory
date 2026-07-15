// port of src/core/store/embedding.ts (OpenAIEmbeddingService)

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::types::EmbeddingProviderInfo;
use serde_json::Value;

const MAX_BATCH_SIZE: usize = 256;
const MAX_RETRIES: usize = 3;
const DEFAULT_API_TIMEOUT_MS: u64 = 10_000;

/// Configuration for OpenAI-compatible embedding service.
#[derive(Clone, Debug)]
pub struct OpenAiEmbeddingConfig {
    pub provider: String,
    pub base_url: String,
    pub proxy_url: Option<String>,
    pub api_key: String,
    pub model: String,
    pub dimensions: u32,
    pub send_dimensions: bool,
    pub max_input_chars: u32,
    pub timeout_ms: u64,
}

/// Embedding service using OpenAI-compatible HTTP API.
pub struct OpenAiEmbeddingService {
    config: OpenAiEmbeddingConfig,
}

impl OpenAiEmbeddingService {
    pub fn new(config: OpenAiEmbeddingConfig) -> Self {
        Self { config }
    }

    fn remote_url(&self) -> String {
        format!("{}/embeddings", self.config.base_url.trim_end_matches('/'))
    }

    fn build_url(&self) -> String {
        if self.config.provider == "qclaw"
            && let Some(proxy) = self
                .config
                .proxy_url
                .as_deref()
                .filter(|url| !url.trim().is_empty())
        {
            return proxy.to_owned();
        }
        self.remote_url()
    }

    fn build_body(&self, texts: &[String]) -> Value {
        let mut body = serde_json::json!({
            "input": texts,
            "model": self.config.model,
        });
        if self.config.send_dimensions && self.config.dimensions > 0 {
            body["dimensions"] = serde_json::json!(self.config.dimensions);
        }
        body
    }

    fn truncate_input(&self, text: &str) -> String {
        // JavaScript's String.length/string.slice use UTF-16 code units.  Keep
        // the same limit when the cut falls on a valid Unicode scalar boundary.
        let limit = self.config.max_input_chars as usize;
        if limit == 0 {
            return text.to_string();
        }
        if text.encode_utf16().count() <= limit {
            return text.to_string();
        }
        let mut used = 0;
        text.chars()
            .take_while(|ch| {
                let width = ch.len_utf16();
                let keep = used + width <= limit;
                if keep {
                    used += width;
                }
                keep
            })
            .collect()
    }

    fn effective_timeout_ms(&self) -> u64 {
        if self.config.timeout_ms == 0 {
            DEFAULT_API_TIMEOUT_MS
        } else {
            self.config.timeout_ms
        }
    }

    fn sanitize_and_normalize(vec: &[f64]) -> Vec<f32> {
        let arr: Vec<f64> = vec
            .iter()
            .map(|v| if v.is_finite() { *v } else { 0.0 })
            .collect();
        let magnitude = arr.iter().map(|v| v * v).sum::<f64>().sqrt();
        if magnitude < 1e-10 {
            return arr.into_iter().map(|v| v as f32).collect();
        }
        arr.into_iter().map(|v| (v / magnitude) as f32).collect()
    }

    /// Get embedding for a single text.
    pub fn embed(&self, text: &str) -> AeonMemoryResult<Vec<f32>> {
        let texts = vec![text.to_string()];
        let results = self.embed_batch(&texts)?;
        Ok(results.into_iter().next().unwrap_or_default())
    }

    /// Get embeddings for multiple texts.
    pub fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        // The TS implementation returns before constructing or sending a
        // request for an empty input batch.
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let texts: Vec<String> = texts.iter().map(|text| self.truncate_input(text)).collect();
        if texts.len() > MAX_BATCH_SIZE {
            let mut results = Vec::with_capacity(texts.len());
            for chunk in texts.chunks(MAX_BATCH_SIZE) {
                results.extend(self.call_api(chunk)?);
            }
            return Ok(results);
        }

        self.call_api(&texts)
    }

    fn call_api(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        let url = self.build_url();
        let body = self.build_body(texts);
        let body_str = serde_json::to_string(&body)?;
        let mut attempt = 0usize;
        let mut response = loop {
            let mut request = ureq::post(&url)
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", self.config.api_key))
                .header("Accept", "application/json");
            if self.config.provider == "qclaw" && url != self.remote_url() {
                request = request.header("Remote-URL", self.remote_url());
            }
            let request = request
                .config()
                .timeout_global(Some(std::time::Duration::from_millis(
                    self.effective_timeout_ms(),
                )))
                // Keep the v2 behaviour that lets retry logic inspect the response body.
                .http_status_as_error(false)
                .build();
            match request.send(&body_str) {
                Ok(mut response) => {
                    let code = response.status().as_u16();
                    if !(400..600).contains(&code) {
                        break response;
                    }
                    let response_body = response.body_mut().read_to_string().unwrap_or_default();
                    let error = AeonMemoryCoreError::Embedding(format!(
                        "Embedding API returned HTTP {}: {}",
                        code,
                        response_body.chars().take(500).collect::<String>()
                    ));
                    if (400..500).contains(&code) && code != 429 {
                        return Err(error);
                    }
                    if attempt >= MAX_RETRIES {
                        return Err(error);
                    }
                    // The pinned TS implementation immediately retries HTTP
                    // 429/5xx responses; only thrown network/timeout failures
                    // enter its backoff branch.
                    attempt += 1;
                }
                Err(error) => {
                    let error =
                        AeonMemoryCoreError::Embedding(format!("HTTP request failed: {}", error));
                    if attempt >= MAX_RETRIES {
                        return Err(error);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(
                        500 * (attempt + 1) as u64,
                    ));
                    attempt += 1;
                }
            }
        };

        let response_text = response.body_mut().read_to_string().map_err(|e| {
            AeonMemoryCoreError::Embedding(format!("Failed to read response: {}", e))
        })?;

        let json: Value = serde_json::from_str(&response_text)
            .map_err(|e| AeonMemoryCoreError::Embedding(format!("Invalid JSON: {}", e)))?;

        let data = json["data"].as_array().ok_or_else(|| {
            AeonMemoryCoreError::Embedding(
                "Embedding API returned unexpected format: missing 'data' array".to_string(),
            )
        })?;

        // Sort by index to preserve order
        let mut sorted: Vec<(usize, Vec<f32>)> = data
            .iter()
            .map(|item| {
                let idx = item["index"].as_u64().ok_or_else(|| {
                    AeonMemoryCoreError::Embedding(
                        "Invalid embedding item: missing numeric 'index'".to_string(),
                    )
                })? as usize;
                let embedding = item["embedding"]
                    .as_array()
                    .ok_or_else(|| {
                        AeonMemoryCoreError::Embedding(
                            "Invalid embedding item: missing 'embedding' array".to_string(),
                        )
                    })?
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(0.0))
                    .collect::<Vec<_>>();
                Ok((idx, Self::sanitize_and_normalize(&embedding)))
            })
            .collect::<AeonMemoryResult<_>>()?;
        sorted.sort_by_key(|(idx, _)| *idx);

        Ok(sorted.into_iter().map(|(_, emb)| emb).collect())
    }

    pub fn dimensions(&self) -> u32 {
        self.config.dimensions
    }

    pub fn provider_info(&self) -> EmbeddingProviderInfo {
        EmbeddingProviderInfo {
            provider: self.config.provider.clone(),
            model: self.config.model.clone(),
        }
    }
}

impl crate::types::EmbeddingService for OpenAiEmbeddingService {
    fn embed(&self, text: &str) -> AeonMemoryResult<Vec<f32>> {
        OpenAiEmbeddingService::embed(self, text)
    }

    fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        OpenAiEmbeddingService::embed_batch(self, texts)
    }

    fn dimensions(&self) -> u32 {
        OpenAiEmbeddingService::dimensions(self)
    }
}

/// No-op embedding service for backends with server-side embedding.
#[derive(Default)]
pub struct NoopEmbeddingService;

impl NoopEmbeddingService {
    pub fn new() -> Self {
        Self
    }

    pub fn embed(&self, _text: &str) -> AeonMemoryResult<Vec<f32>> {
        Ok(Vec::new())
    }

    pub fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| Vec::new()).collect())
    }

    pub fn dimensions(&self) -> u32 {
        0
    }

    pub fn provider_info(&self) -> EmbeddingProviderInfo {
        EmbeddingProviderInfo {
            provider: "noop".to_string(),
            model: "server-side".to_string(),
        }
    }
}

impl crate::types::EmbeddingService for NoopEmbeddingService {
    fn embed(&self, text: &str) -> AeonMemoryResult<Vec<f32>> {
        NoopEmbeddingService::embed(self, text)
    }
    fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        NoopEmbeddingService::embed_batch(self, texts)
    }
    fn dimensions(&self) -> u32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            bytes.extend_from_slice(&buffer[..count]);
            let header_end = bytes.windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(header_end) = header_end {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + content_length {
                    break;
                }
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn write_response(stream: &mut TcpStream, status: &str, body: &str) {
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(reply.as_bytes()).unwrap();
    }

    fn mock_server(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            write_response(&mut stream, "200 OK", response);
            request
        });
        (format!("http://{}", address), handle)
    }

    fn remote_config(base_url: String) -> OpenAiEmbeddingConfig {
        OpenAiEmbeddingConfig {
            provider: "openai".into(),
            base_url,
            proxy_url: None,
            api_key: "secret".into(),
            model: "fixture-model".into(),
            dimensions: 3,
            send_dimensions: true,
            max_input_chars: 3,
            timeout_ms: 1_000,
        }
    }

    #[test]
    fn qclaw_uses_proxy_url_and_remote_url_header() {
        let (proxy_url, request) = mock_server(r#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#);
        let service = OpenAiEmbeddingService::new(OpenAiEmbeddingConfig {
            provider: "qclaw".into(),
            base_url: "https://remote.example/v1/".into(),
            proxy_url: Some(proxy_url),
            api_key: "proxy-secret".into(),
            model: "qclaw-embedding".into(),
            dimensions: 3,
            send_dimensions: true,
            max_input_chars: 5_000,
            timeout_ms: 1_000,
        });
        assert_eq!(service.embed("hello").unwrap(), vec![1.0, 0.0, 0.0]);
        let request = request.join().unwrap();
        assert!(request.starts_with("POST / HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("remote-url: https://remote.example/v1/embeddings")
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer proxy-secret")
        );
        assert_eq!(service.provider_info().provider, "qclaw");
    }

    #[test]
    fn configured_timeout_aborts_delayed_embedding_http() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
        let mut config = remote_config(format!("http://{address}"));
        config.timeout_ms = 40;
        let service = OpenAiEmbeddingService::new(config);
        let started = std::time::Instant::now();
        let error = service.embed("hello").unwrap_err();
        assert!(started.elapsed() >= std::time::Duration::from_millis(1_500));
        assert!(error.to_string().contains("HTTP request failed"));
        server.join().unwrap();
    }

    #[test]
    fn retries_429_and_5xx_until_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in [
                ("429 Too Many Requests", r#"{"error":"rate limited"}"#),
                ("503 Service Unavailable", r#"{"error":"later"}"#),
                ("200 OK", r#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                requests.push(read_request(&mut stream));
                write_response(&mut stream, status, body);
            }
            requests
        });
        let service = OpenAiEmbeddingService::new(remote_config(format!("http://{address}")));
        assert_eq!(service.embed("retry").unwrap(), vec![1.0, 0.0, 0.0]);
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[test]
    fn retries_network_failure_with_ts_backoff() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let _ = read_request(&mut first);
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            let request = read_request(&mut second);
            write_response(
                &mut second,
                "200 OK",
                r#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#,
            );
            request
        });
        let service = OpenAiEmbeddingService::new(remote_config(format!("http://{address}")));
        let started = std::time::Instant::now();
        assert_eq!(service.embed("retry").unwrap(), vec![1.0, 0.0, 0.0]);
        assert!(started.elapsed() >= std::time::Duration::from_millis(500));
        assert!(!server.join().unwrap().is_empty());
    }

    #[test]
    fn splits_257_texts_into_ts_sized_batches_and_preserves_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut lengths = Vec::new();
            for offset in [0usize, 256] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let body = request.split("\r\n\r\n").nth(1).unwrap();
                let input = serde_json::from_str::<Value>(body).unwrap()["input"]
                    .as_array()
                    .unwrap()
                    .clone();
                lengths.push(input.len());
                let data = input
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        serde_json::json!({"index": index, "embedding": [offset + index + 1, 1]})
                    })
                    .collect::<Vec<_>>();
                write_response(
                    &mut stream,
                    "200 OK",
                    &serde_json::json!({"data": data}).to_string(),
                );
            }
            lengths
        });
        let mut config = remote_config(format!("http://{address}"));
        config.max_input_chars = 0;
        let service = OpenAiEmbeddingService::new(config);
        let texts = (0..257)
            .map(|index| format!("text-{index}"))
            .collect::<Vec<_>>();
        let result = service.embed_batch(&texts).unwrap();
        assert_eq!(server.join().unwrap(), vec![256, 1]);
        assert_eq!(result.len(), 257);
        assert!((result[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!(result[256][0] > 0.99);
    }

    #[test]
    fn zero_limits_match_ts_defaults() {
        let (url, request) = mock_server(r#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#);
        let mut config = remote_config(url);
        config.max_input_chars = 0;
        config.timeout_ms = 0;
        let service = OpenAiEmbeddingService::new(config);
        assert_eq!(service.effective_timeout_ms(), DEFAULT_API_TIMEOUT_MS);
        service.embed("not truncated").unwrap();
        let request = request.join().unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap()["input"],
            serde_json::json!(["not truncated"])
        );
    }

    #[test]
    fn test_build_url() {
        let config = OpenAiEmbeddingConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            proxy_url: None,
            api_key: "sk-test".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
            send_dimensions: true,
            max_input_chars: 5000,
            timeout_ms: 10000,
        };
        let svc = OpenAiEmbeddingService::new(config);
        assert_eq!(svc.build_url(), "https://api.openai.com/v1/embeddings");
    }

    #[test]
    fn test_build_body() {
        let config = OpenAiEmbeddingConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            proxy_url: None,
            api_key: "sk-test".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
            send_dimensions: true,
            max_input_chars: 5000,
            timeout_ms: 10000,
        };
        let svc = OpenAiEmbeddingService::new(config);
        let body = svc.build_body(&["hello".to_string()]);
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["dimensions"], 1536);
        assert_eq!(body["input"][0], "hello");
    }

    #[test]
    fn test_truncate_input() {
        let config = OpenAiEmbeddingConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            proxy_url: None,
            api_key: "sk-test".into(),
            model: "test".into(),
            dimensions: 768,
            send_dimensions: true,
            max_input_chars: 10,
            timeout_ms: 10000,
        };
        let svc = OpenAiEmbeddingService::new(config);
        assert_eq!(svc.truncate_input("short"), "short");
        assert_eq!(
            svc.truncate_input("a very long input text that should be truncated"),
            "a very long input text that should be truncated"
                .chars()
                .take(10)
                .collect::<String>()
        );
    }

    #[test]
    fn test_sanitize_normalize() {
        let vec = vec![3.0, 0.0, 4.0];
        let result = OpenAiEmbeddingService::sanitize_and_normalize(&vec);
        // 3/5 = 0.6, 0, 4/5 = 0.8
        assert!((result[0] - 0.6).abs() < 0.001);
        assert!((result[1] - 0.0).abs() < 0.001);
        assert!((result[2] - 0.8).abs() < 0.001);

        // Zero vector: should stay zero
        let zero = vec![0.0, 0.0];
        let result = OpenAiEmbeddingService::sanitize_and_normalize(&zero);
        assert_eq!(result, vec![0.0, 0.0]);
    }

    #[test]
    fn test_sanitize_nan() {
        let vec = vec![f64::NAN, 1.0];
        let result = OpenAiEmbeddingService::sanitize_and_normalize(&vec);
        assert!(result[0] == 0.0);
        assert!((result[1] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_noop_embedding() {
        let svc = NoopEmbeddingService::new();
        assert_eq!(svc.embed("test").unwrap().len(), 0);
        assert_eq!(svc.embed_batch(&["a".into(), "b".into()]).unwrap().len(), 2);
        assert_eq!(svc.dimensions(), 0);
    }

    #[test]
    fn empty_batch_does_not_issue_http_request() {
        let svc = OpenAiEmbeddingService::new(remote_config("http://127.0.0.1:1".into()));
        assert_eq!(svc.embed_batch(&[]).unwrap(), Vec::<Vec<f32>>::new());
    }

    #[test]
    fn remote_protocol_sorts_normalizes_and_preserves_dimension_mismatch() {
        // TS does not enforce the configured output dimension; it sorts the
        // response by index and normalizes every vector it receives.
        let (base_url, request) = mock_server(
            r#"{"data":[{"index":1,"embedding":[0,5]},{"index":0,"embedding":[3,0,4]}]}"#,
        );
        let svc = OpenAiEmbeddingService::new(remote_config(base_url));
        let output = svc.embed_batch(&["A😀B".into(), "xy".into()]).unwrap();
        assert_eq!(output, vec![vec![0.6, 0.0, 0.8], vec![0.0, 1.0]]);

        let request = request.join().unwrap();
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["input"], serde_json::json!(["A😀", "xy"]));
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["dimensions"], 3);
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret")
        );
    }

    #[test]
    fn remote_protocol_rejects_missing_or_malformed_data() {
        let (base_url, request) = mock_server(r#"{}"#);
        let svc = OpenAiEmbeddingService::new(remote_config(base_url));
        assert!(svc.embed("x").unwrap_err().to_string().contains("data"));
        request.join().unwrap();

        let (base_url, request) =
            mock_server(r#"{"data":[{"index":0,"embedding":[1,"invalid"]}]}"#);
        let svc = OpenAiEmbeddingService::new(remote_config(base_url));
        assert_eq!(svc.embed("x").unwrap(), vec![1.0, 0.0]);
        request.join().unwrap();
    }
}
