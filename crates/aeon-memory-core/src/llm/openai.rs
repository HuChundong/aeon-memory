// OpenAI-compatible standalone LLM runner.

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::types::{
    FileToolPolicy, LlmRunParams, LlmRunner, LlmRunnerCreateOptions, LlmRunnerFactory,
};
use crate::utils::no_think_fetch;
use serde_json::Value;
use std::{
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

const DEFAULT_MAX_RETRIES: usize = 2;
const INITIAL_RETRY_DELAY_MS: u64 = 2_000;
const MAX_TOOL_ITERATIONS: usize = 20;

/// Configuration for standalone LLM runner.
#[derive(Clone, Debug)]
pub struct StandaloneLlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout_ms: u64,
    pub disable_thinking: Option<String>,
}

/// HTTP-based LLM runner for OpenAI-compatible APIs.
/// Uses ureq (sync) wrapped in spawn_blocking to avoid blocking the async runtime.
pub struct OpenAiLlmRunner {
    config: StandaloneLlmConfig,
    model: String,
    enable_tools: bool,
    temperature: Option<f64>,
}

impl OpenAiLlmRunner {
    pub fn new(config: StandaloneLlmConfig, model: Option<String>, enable_tools: bool) -> Self {
        Self {
            model: model.unwrap_or_else(|| config.model.clone()),
            config,
            enable_tools,
            temperature: None,
        }
    }

    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    fn build_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    fn build_body(&self, params: &LlmRunParams) -> Value {
        let mut messages = Vec::new();
        if let Some(ref sys) = params.system_prompt {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        messages.push(serde_json::json!({"role": "user", "content": params.prompt}));

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": params.max_tokens.unwrap_or(self.config.max_tokens),
        });
        if let Some(temperature) = self.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }

        if self.enable_tools && params.workspace_dir.is_some() && params.file_tool_policy.is_some()
        {
            body["tools"] = serde_json::json!([
                {"type": "function", "function": {"name": "read_file", "description": "Read the contents of a file at the given relative path", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}}},
                {"type": "function", "function": {"name": "write_to_file", "description": "Write content to a file at the given relative path; creates or overwrites", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"], "additionalProperties": false}}},
                {"type": "function", "function": {"name": "replace_in_file", "description": "Replace an exact substring in a file with new content", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "old_str": {"type": "string"}, "new_str": {"type": "string"}}, "required": ["path", "old_str", "new_str"], "additionalProperties": false}}},
            ]);
        }

        if let Some(ref strategy) = self.config.disable_thinking {
            no_think_fetch::apply_no_think_strategy(strategy, &mut body);
        }

        body
    }

    /// Synchronous HTTP call (used inside spawn_blocking).
    fn call_api_json_sync(
        body: &Value,
        config: &StandaloneLlmConfig,
        url: &str,
        deadline: Instant,
    ) -> AeonMemoryResult<Value> {
        let body_str = serde_json::to_string(body)?;
        let mut retry_delay_ms = INITIAL_RETRY_DELAY_MS;
        let mut attempt = 0_usize;

        let response = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AeonMemoryCoreError::Llm("LLM request timed out".into()));
            }
            let request = ureq::post(url)
                .set("Content-Type", "application/json")
                .set("Authorization", &format!("Bearer {}", config.api_key))
                .set("Accept", "application/json")
                .timeout(remaining);
            match request.send_string(&body_str) {
                Ok(response) => break response,
                Err(ureq::Error::Status(code, response)) => {
                    let retry_after = Self::retry_after_delay(&response, retry_delay_ms);
                    let body = response.into_string().unwrap_or_default();
                    let error = AeonMemoryCoreError::Llm(format!(
                        "LLM API returned HTTP {}: {}",
                        code,
                        body.chars().take(500).collect::<String>()
                    ));
                    if !Self::is_retryable_status(code) || attempt >= DEFAULT_MAX_RETRIES {
                        return Err(error);
                    }
                    Self::wait_before_retry(retry_after, deadline, error)?;
                }
                Err(error) => {
                    // Vercel AI SDK marks fetch/network failures retryable.
                    let error = AeonMemoryCoreError::Llm(format!("HTTP request failed: {error}"));
                    if attempt >= DEFAULT_MAX_RETRIES {
                        return Err(error);
                    }
                    Self::wait_before_retry(
                        Duration::from_millis(retry_delay_ms),
                        deadline,
                        error,
                    )?;
                }
            }
            attempt += 1;
            retry_delay_ms = retry_delay_ms.saturating_mul(2);
        };

        let response_text = response
            .into_string()
            .map_err(|e| AeonMemoryCoreError::Llm(format!("Failed to read response: {}", e)))?;

        serde_json::from_str(&response_text)
            .map_err(|e| AeonMemoryCoreError::Llm(format!("Invalid JSON response: {}", e)))
    }

    fn call_api_sync(
        body: &Value,
        config: &StandaloneLlmConfig,
        url: &str,
    ) -> AeonMemoryResult<String> {
        let json = Self::call_api_json_sync(
            body,
            config,
            url,
            Instant::now() + Duration::from_millis(config.timeout_ms),
        )?;

        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();

        Ok(text)
    }

    fn run_tool_loop_sync(
        mut body: Value,
        config: &StandaloneLlmConfig,
        url: &str,
        workspace: &Path,
        mut policy: FileToolPolicy,
    ) -> AeonMemoryResult<String> {
        let root = workspace.canonicalize().map_err(|error| {
            AeonMemoryCoreError::InvalidInput(format!("invalid tool workspace: {error}"))
        })?;
        let deadline = Instant::now() + Duration::from_millis(config.timeout_ms);
        for _ in 0..MAX_TOOL_ITERATIONS {
            let json = Self::call_api_json_sync(&body, config, url, deadline)?;
            let message = json
                .pointer("/choices/0/message")
                .cloned()
                .ok_or_else(|| AeonMemoryCoreError::Llm("LLM response missing message".into()))?;
            let tool_calls = message
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if tool_calls.is_empty() {
                return Ok(message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned());
            }
            let messages = body
                .get_mut("messages")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| AeonMemoryCoreError::Llm("LLM request missing messages".into()))?;
            messages.push(message);
            for call in tool_calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool-call");
                let function = call.get("function").unwrap_or(&Value::Null);
                let name = function.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let result = match serde_json::from_str::<Value>(arguments) {
                    Ok(arguments) => Self::execute_file_tool(&root, &mut policy, name, &arguments),
                    Err(error) => Err(format!("invalid tool arguments: {error}")),
                };
                let content = match result {
                    Ok(value) => serde_json::json!({"success": true, "result": value}),
                    Err(error) => serde_json::json!({"success": false, "error": error}),
                };
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": content.to_string(),
                }));
            }
        }
        Err(AeonMemoryCoreError::Llm(format!(
            "LLM exceeded {MAX_TOOL_ITERATIONS} file-tool iterations"
        )))
    }

    fn execute_file_tool(
        root: &Path,
        policy: &mut FileToolPolicy,
        name: &str,
        arguments: &Value,
    ) -> Result<Value, String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "path is required".to_owned())?;
        Self::validate_policy_path(policy, name, path)?;
        match name {
            "read_file" => {
                let target = Self::restricted_existing_path(root, path)?;
                std::fs::read_to_string(target)
                    .map(Value::String)
                    .map_err(|error| error.to_string())
            }
            "write_to_file" => {
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "content is required".to_owned())?;
                if content.trim().is_empty() {
                    return Err("content cannot be empty or whitespace-only".into());
                }
                let target = Self::restricted_write_path(root, path)?;
                std::fs::write(target, content).map_err(|error| error.to_string())?;
                if let FileToolPolicy::Scene { readable_files } = policy
                    && !readable_files.iter().any(|allowed| allowed == path)
                {
                    readable_files.push(path.to_owned());
                }
                Ok(serde_json::json!({"bytes": content.len()}))
            }
            "replace_in_file" => {
                let target = Self::restricted_existing_path(root, path)?;
                let old = arguments
                    .get("old_str")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "old_str cannot be empty".to_owned())?;
                let new = arguments
                    .get("new_str")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "new_str is required".to_owned())?;
                let mut content = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
                if !content.contains(old) {
                    return Err(format!("old_str not found in file {path:?}"));
                }
                content = content.replacen(old, new, 1);
                if content.trim().is_empty() {
                    return Err("replacement cannot make a file empty".into());
                }
                std::fs::write(target, &content).map_err(|error| error.to_string())?;
                Ok(serde_json::json!({"bytes": content.len()}))
            }
            _ => Err(format!("unsupported tool {name:?}")),
        }
    }

    fn validate_policy_path(policy: &FileToolPolicy, tool: &str, path: &str) -> Result<(), String> {
        let relative = Self::relative_path(path)?;
        if relative.components().count() != 1 {
            return Err("tool path must be a direct child of the workspace".into());
        }
        match policy {
            FileToolPolicy::Scene { readable_files } => {
                if relative.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    return Err("scene tools may only operate on .md files".into());
                }
                if matches!(tool, "read_file" | "replace_in_file")
                    && !readable_files.iter().any(|allowed| allowed == path)
                {
                    return Err("scene read/edit is limited to the initial file allowlist".into());
                }
            }
            FileToolPolicy::Persona => {
                if path != "persona.md" {
                    return Err("persona tools may only operate on persona.md".into());
                }
                if tool == "read_file" {
                    return Err(
                        "persona file reads are disabled; existing content is in the prompt".into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn relative_path(path: &str) -> Result<PathBuf, String> {
        let path = Path::new(path);
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("path must be a normalized relative path inside the workspace".into());
        }
        Ok(path.to_path_buf())
    }

    fn restricted_existing_path(root: &Path, path: &str) -> Result<PathBuf, String> {
        let target = root.join(Self::relative_path(path)?);
        let metadata = std::fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("target must be a regular non-symlink file".into());
        }
        let canonical = target.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(root) {
            return Err("path escapes workspace boundary".into());
        }
        Ok(canonical)
    }

    fn restricted_write_path(root: &Path, path: &str) -> Result<PathBuf, String> {
        let target = root.join(Self::relative_path(path)?);
        let parent = target
            .parent()
            .ok_or_else(|| "path has no parent".to_owned())?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !parent.starts_with(root) {
            return Err("path escapes workspace boundary".into());
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&target)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err("target must be a regular non-symlink file".into());
        }
        Ok(target)
    }

    fn is_retryable_status(status: u16) -> bool {
        matches!(status, 408 | 409 | 429) || status >= 500
    }

    fn retry_after_delay(response: &ureq::Response, fallback_ms: u64) -> Duration {
        let header_ms = response
            .header("retry-after-ms")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| !value.is_nan());
        let retry_after_ms = header_ms.or_else(|| {
            let value = response.header("retry-after")?;
            if let Ok(seconds) = value.parse::<f64>() {
                return (seconds.is_finite() && seconds >= 0.0).then_some(seconds * 1_000.0);
            }
            let target = chrono::DateTime::parse_from_rfc2822(value).ok()?;
            Some((target.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64)
        });
        let chosen = retry_after_ms
            .filter(|delay| {
                !delay.is_nan()
                    && *delay >= 0.0
                    && (*delay < 60_000.0 || *delay < fallback_ms as f64)
            })
            .unwrap_or(fallback_ms as f64);
        Duration::from_millis(chosen as u64)
    }

    fn wait_before_retry(
        delay: Duration,
        deadline: Instant,
        last_error: AeonMemoryCoreError,
    ) -> AeonMemoryResult<()> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= delay {
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
            return Err(last_error);
        }
        std::thread::sleep(delay);
        Ok(())
    }
}

#[async_trait::async_trait]
impl LlmRunner for OpenAiLlmRunner {
    async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
        let body = self.build_body(&params);
        let url = self.build_url();
        let mut config = self.config.clone();
        config.timeout_ms = params.timeout_ms.unwrap_or(config.timeout_ms);

        // Run the synchronous HTTP call on a dedicated thread to avoid
        // blocking the async runtime. This is the spawn_blocking boundary.
        let tool_context = self
            .enable_tools
            .then(|| {
                params
                    .workspace_dir
                    .zip(params.file_tool_policy)
                    .map(|(workspace, policy)| (PathBuf::from(workspace), policy))
            })
            .flatten();
        let result = tokio::task::spawn_blocking(move || match tool_context {
            Some((workspace, policy)) => {
                Self::run_tool_loop_sync(body, &config, &url, &workspace, policy)
            }
            None => Self::call_api_sync(&body, &config, &url),
        })
        .await
        .map_err(|e| AeonMemoryCoreError::Llm(format!("spawn_blocking failed: {}", e)))??;

        Ok(result)
    }
}

/// Factory for creating OpenAiLlmRunner instances.
pub struct OpenAiLlmRunnerFactory {
    config: StandaloneLlmConfig,
}

impl OpenAiLlmRunnerFactory {
    pub fn new(config: StandaloneLlmConfig) -> Self {
        Self { config }
    }
}

impl LlmRunnerFactory for OpenAiLlmRunnerFactory {
    fn create_runner(&self, opts: LlmRunnerCreateOptions) -> Box<dyn LlmRunner> {
        let model = opts
            .model_ref
            .as_ref()
            .and_then(|r| r.split('/').nth(1))
            .map(|m| m.to_string());
        Box::new(OpenAiLlmRunner::new(
            self.config.clone(),
            model,
            opts.enable_tools,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn scripted_server(
        responses: Vec<(u16, &'static str, &'static str)>,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recorded = calls.clone();
        let handle = std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                recorded.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let reason = match status {
                    200 => "OK",
                    400 => "Bad Request",
                    503 => "Service Unavailable",
                    _ => "Internal Server Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{headers}connection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });
        (format!("http://{address}/v1"), calls, handle)
    }

    fn remote_config(base_url: String) -> StandaloneLlmConfig {
        StandaloneLlmConfig {
            base_url,
            api_key: "sk-test".into(),
            model: "test-model".into(),
            max_tokens: 128,
            timeout_ms: 1_000,
            disable_thinking: None,
        }
    }

    #[test]
    fn test_build_url() {
        let config = StandaloneLlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 30000,
            disable_thinking: None,
        };
        let runner = OpenAiLlmRunner::new(config, None, false);
        assert_eq!(
            runner.build_url(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_build_body_basic() {
        let config = StandaloneLlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 30000,
            disable_thinking: None,
        };
        let runner = OpenAiLlmRunner::new(config, None, false);
        let params = LlmRunParams {
            prompt: "Hello".into(),
            system_prompt: Some("Be helpful".into()),
            task_id: "test-1".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        };
        let body = runner.build_body(&params);
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][0]["role"], "system");
    }

    #[test]
    fn test_build_body_no_system() {
        let config = StandaloneLlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 30000,
            disable_thinking: None,
        };
        let runner = OpenAiLlmRunner::new(config, None, false);
        let params = LlmRunParams {
            prompt: "Hello".into(),
            system_prompt: None,
            task_id: "test-2".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        };
        let body = runner.build_body(&params);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_build_body_with_tools() {
        let config = StandaloneLlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 30000,
            disable_thinking: None,
        };
        let runner = OpenAiLlmRunner::new(config, None, true);
        let params = LlmRunParams {
            prompt: "Write".into(),
            system_prompt: None,
            task_id: "test".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: Some("/tmp/restricted-workspace".into()),
            file_tool_policy: Some(FileToolPolicy::Scene {
                readable_files: vec![],
            }),
            instance_id: None,
        };
        let body = runner.build_body(&params);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["function"]["name"], "read_file");
        assert_eq!(tools[1]["function"]["name"], "write_to_file");
        assert_eq!(tools[2]["function"]["name"], "replace_in_file");
    }

    #[test]
    fn tool_enabled_runner_stays_text_only_without_workspace() {
        let runner = OpenAiLlmRunner::new(
            StandaloneLlmConfig {
                base_url: "https://api.openai.com/v1".into(),
                api_key: "sk-test".into(),
                model: "gpt-4o".into(),
                max_tokens: 4096,
                timeout_ms: 30000,
                disable_thinking: None,
            },
            None,
            true,
        );
        let body = runner.build_body(&LlmRunParams {
            prompt: "extract text".into(),
            system_prompt: None,
            task_id: "l1".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        });
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn restricted_tools_match_standalone_contract_and_reject_unsafe_paths() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("aeon-memory-file-tools-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut policy = FileToolPolicy::Scene {
            readable_files: vec!["scene.md".into()],
        };

        OpenAiLlmRunner::execute_file_tool(
            &root,
            &mut policy,
            "write_to_file",
            &serde_json::json!({"path":"scene.md","content":"old body"}),
        )
        .unwrap();
        assert_eq!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut policy,
                "read_file",
                &serde_json::json!({"path":"scene.md"}),
            )
            .unwrap(),
            "old body"
        );
        OpenAiLlmRunner::execute_file_tool(
            &root,
            &mut policy,
            "replace_in_file",
            &serde_json::json!({
                "path":"scene.md",
                "old_str":"old",
                "new_str":"new"
            }),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("scene.md")).unwrap(),
            "new body"
        );

        for path in ["../outside.md", "/tmp/outside.md", "./scene.md"] {
            assert!(
                OpenAiLlmRunner::execute_file_tool(
                    &root,
                    &mut policy,
                    "write_to_file",
                    &serde_json::json!({"path":path,"content":"blocked"}),
                )
                .is_err(),
                "unsafe path must be rejected: {path}"
            );
        }
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut policy,
                "write_to_file",
                &serde_json::json!({"path":"empty.md","content":"  \n"}),
            )
            .is_err()
        );
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut policy,
                "write_to_file",
                &serde_json::json!({"path":"notes.txt","content":"blocked"}),
            )
            .is_err(),
            "scene policy only permits Markdown files"
        );
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut policy,
                "write_to_file",
                &serde_json::json!({"path":"nested/scene.md","content":"blocked"}),
            )
            .is_err(),
            "scene policy only permits direct children"
        );
        OpenAiLlmRunner::execute_file_tool(
            &root,
            &mut policy,
            "write_to_file",
            &serde_json::json!({"path":"new.md","content":"new scene"}),
        )
        .unwrap();
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut policy,
                "read_file",
                &serde_json::json!({"path":"new.md"}),
            )
            .is_ok(),
            "files created during the run become readable/editable"
        );

        let mut persona = FileToolPolicy::Persona;
        OpenAiLlmRunner::execute_file_tool(
            &root,
            &mut persona,
            "write_to_file",
            &serde_json::json!({"path":"persona.md","content":"# Persona"}),
        )
        .unwrap();
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut persona,
                "read_file",
                &serde_json::json!({"path":"persona.md"}),
            )
            .is_err(),
            "persona content is preloaded and direct reads are disabled"
        );
        assert!(
            OpenAiLlmRunner::execute_file_tool(
                &root,
                &mut persona,
                "write_to_file",
                &serde_json::json!({"path":"scene.md","content":"blocked"}),
            )
            .is_err(),
            "persona policy permits only persona.md"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", root.join("escape")).unwrap();
            assert!(
                OpenAiLlmRunner::execute_file_tool(
                    &root,
                    &mut policy,
                    "write_to_file",
                    &serde_json::json!({"path":"escape/file.md","content":"blocked"}),
                )
                .is_err()
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn test_build_body_no_think() {
        let config = StandaloneLlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 30000,
            disable_thinking: Some("deepseek".into()),
        };
        let runner = OpenAiLlmRunner::new(config, None, false);
        let params = LlmRunParams {
            prompt: "Hi".into(),
            system_prompt: None,
            task_id: "t".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        };
        let body = runner.build_body(&params);
        assert_eq!(body["enable_thinking"], false);
    }

    /// Concurrency test: runs two LLM calls in parallel using tokio::spawn.
    /// Verifies that both complete without blocking the scheduler.
    #[tokio::test]
    async fn test_concurrent_async_runner() {
        let config = StandaloneLlmConfig {
            base_url: "http://localhost:19999/v1".into(), // unreachable, will fail fast
            api_key: "sk-test".into(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
            timeout_ms: 100,
            disable_thinking: None,
        };
        let runner = std::sync::Arc::new(OpenAiLlmRunner::new(config, None, false));

        let runner1 = runner.clone();
        let runner2 = runner.clone();

        let h1 = tokio::spawn(async move {
            let params = LlmRunParams {
                prompt: "test".into(),
                system_prompt: None,
                task_id: "concurrent-1".into(),
                timeout_ms: None,
                max_tokens: None,
                workspace_dir: None,
                file_tool_policy: None,
                instance_id: None,
            };
            runner1.run(params).await
        });

        let h2 = tokio::spawn(async move {
            let params = LlmRunParams {
                prompt: "test".into(),
                system_prompt: None,
                task_id: "concurrent-2".into(),
                timeout_ms: None,
                max_tokens: None,
                workspace_dir: None,
                file_tool_policy: None,
                instance_id: None,
            };
            runner2.run(params).await
        });

        // Both should complete (both should fail with connection refused)
        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();
        assert!(r1.is_err(), "Both calls should fail (port unreachable)");
        assert!(r2.is_err(), "Both calls should fail (port unreachable)");
    }

    #[test]
    fn retries_503_then_returns_success_like_ai_sdk() {
        let _guard = http_test_lock();
        let success = r#"{"choices":[{"message":{"content":"recovered"}}]}"#;
        let (base_url, calls, server) = scripted_server(vec![
            (
                503,
                "retry-after-ms: 0\r\n",
                r#"{"error":{"message":"temporary"}}"#,
            ),
            (200, "", success),
        ]);
        let mut config = remote_config(base_url.clone());
        config.timeout_ms = 10_000;
        let result = OpenAiLlmRunner::call_api_sync(
            &serde_json::json!({"model":"test"}),
            &config,
            &format!("{base_url}/chat/completions"),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result, "recovered");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn does_not_retry_non_retryable_400() {
        let _guard = http_test_lock();
        let (base_url, calls, server) = scripted_server(vec![(
            400,
            "retry-after-ms: 0\r\n",
            r#"{"error":{"message":"bad request"}}"#,
        )]);
        let config = remote_config(base_url.clone());
        let error = OpenAiLlmRunner::call_api_sync(
            &serde_json::json!({"model":"test"}),
            &config,
            &format!("{base_url}/chat/completions"),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(
            error.to_string().contains("HTTP 400"),
            "unexpected error: {error}"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn tool_loop_stops_after_twenty_model_steps() {
        let _guard = http_test_lock();
        let tool_call = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"loop","type":"function","function":{"name":"write_to_file","arguments":"{\"path\":\"loop.md\",\"content\":\"x\"}"}}]}}]}"#;
        let (base_url, calls, server) = scripted_server(
            (0..MAX_TOOL_ITERATIONS)
                .map(|_| (200, "", tool_call))
                .collect(),
        );
        let mut config = remote_config(base_url.clone());
        config.timeout_ms = 5_000;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("aeon-memory-tool-limit-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        let runner = OpenAiLlmRunner::new(config.clone(), None, true);
        let params = LlmRunParams {
            prompt: "loop".into(),
            system_prompt: None,
            task_id: "tool-loop".into(),
            timeout_ms: None,
            max_tokens: None,
            workspace_dir: Some(root.to_string_lossy().into_owned()),
            file_tool_policy: Some(FileToolPolicy::Scene {
                readable_files: vec![],
            }),
            instance_id: None,
        };
        let error = OpenAiLlmRunner::run_tool_loop_sync(
            runner.build_body(&params),
            &config,
            &format!("{base_url}/chat/completions"),
            &root,
            params.file_tool_policy.unwrap(),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("20 file-tool iterations"));
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            MAX_TOOL_ITERATIONS
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
