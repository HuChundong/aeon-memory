use std::path::{Path, PathBuf};

use aeon_memory_core::config::{
    DisableThinkingStrategy, GatewayConfig, default_gateway_data_dir, resolve_gateway_data_dir,
};

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {message}")]
    Invalid { path: String, message: String },
    #[error("no gateway config found; checked: {searched}")]
    NotFound { searched: String },
    #[error("missing external LLM configuration: {0}")]
    MissingLlm(&'static str),
}

pub fn load_config(path: &Path) -> Result<GatewayConfig, StartupError> {
    let display = path.display().to_string();
    let content = std::fs::read_to_string(path).map_err(|source| StartupError::Read {
        path: display.clone(),
        source,
    })?;
    let (mut config, cors_origins_configured) =
        parse_config_content(path, &content).map_err(|message| StartupError::Invalid {
            path: display,
            message,
        })?;
    apply_environment_overrides(&mut config, !cors_origins_configured);
    config.data.base_dir = resolve_gateway_data_dir(&config.data.base_dir)
        .to_string_lossy()
        .into_owned();
    validate_external_llm(&config)?;
    Ok(config)
}

fn parse_config_content(path: &Path, content: &str) -> Result<(GatewayConfig, bool), String> {
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        let mut value: serde_json::Value =
            serde_json::from_str(content).map_err(|error| error.to_string())?;
        let cors_origins_configured = value
            .get("server")
            .and_then(|server| server.get("corsOrigins"))
            .is_some();
        expand_json_env(&mut value, &|key| std::env::var(key).ok());
        let config = GatewayConfig::from_json_str(
            &serde_json::to_string(&value).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok((config, cors_origins_configured))
    } else {
        let mut value: serde_yaml::Value =
            serde_yaml::from_str(content).map_err(|error| error.to_string())?;
        let cors_origins_configured = value
            .get("server")
            .and_then(|server| server.get("corsOrigins"))
            .is_some();
        expand_yaml_env(&mut value, &|key| std::env::var(key).ok());
        let config = GatewayConfig::from_yaml_str(
            &serde_yaml::to_string(&value).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok((config, cors_origins_configured))
    }
}

fn placeholder_name(value: &str) -> Option<&str> {
    value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .filter(|name| !name.is_empty() && name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()))
}

fn expand_json_env(value: &mut serde_json::Value, get_env: &impl Fn(&str) -> Option<String>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(name) = placeholder_name(text) {
                *text = get_env(name).unwrap_or_default();
            }
        }
        serde_json::Value::Array(values) => {
            values
                .iter_mut()
                .for_each(|value| expand_json_env(value, get_env));
        }
        serde_json::Value::Object(values) => {
            values
                .values_mut()
                .for_each(|value| expand_json_env(value, get_env));
        }
        _ => {}
    }
}

fn expand_yaml_env(value: &mut serde_yaml::Value, get_env: &impl Fn(&str) -> Option<String>) {
    match value {
        serde_yaml::Value::String(text) => {
            if let Some(name) = placeholder_name(text) {
                *text = get_env(name).unwrap_or_default();
            }
        }
        serde_yaml::Value::Sequence(values) => {
            values
                .iter_mut()
                .for_each(|value| expand_yaml_env(value, get_env));
        }
        serde_yaml::Value::Mapping(values) => {
            values
                .values_mut()
                .for_each(|value| expand_yaml_env(value, get_env));
        }
        serde_yaml::Value::Tagged(value) => expand_yaml_env(&mut value.value, get_env),
        _ => {}
    }
}

fn trimmed_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u64(key: &str) -> Option<u64> {
    trimmed_env(key).and_then(|value| {
        let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
    })
}

fn apply_environment_overrides(config: &mut GatewayConfig, allow_cors_env: bool) {
    if let Some(port) =
        env_u64("AEON_MEMORY_GATEWAY_PORT").and_then(|value| u16::try_from(value).ok())
    {
        config.server.port = port;
    }
    if let Some(host) = trimmed_env("AEON_MEMORY_GATEWAY_HOST") {
        config.server.host = host;
    }
    if let Some(api_key) = trimmed_env("AEON_MEMORY_GATEWAY_API_KEY") {
        config.server.api_key = Some(api_key);
    }
    if allow_cors_env && let Some(origins) = trimmed_env("AEON_MEMORY_CORS_ORIGINS") {
        config.server.cors_origins = origins
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    if let Some(base_url) = trimmed_env("AEON_MEMORY_LLM_BASE_URL") {
        config.llm.base_url = base_url;
    }
    if let Some(api_key) = trimmed_env("AEON_MEMORY_LLM_API_KEY") {
        config.llm.api_key = api_key;
    }
    if let Some(model) = trimmed_env("AEON_MEMORY_LLM_MODEL") {
        config.llm.model = model;
    }
    if let Some(max_tokens) =
        env_u64("AEON_MEMORY_LLM_MAX_TOKENS").and_then(|value| u32::try_from(value).ok())
    {
        config.llm.max_tokens = max_tokens;
    }
    if let Some(timeout_ms) = env_u64("AEON_MEMORY_LLM_TIMEOUT_MS") {
        config.llm.timeout_ms = timeout_ms;
    }
    if let Some(strategy) = trimmed_env("AEON_MEMORY_LLM_DISABLE_THINKING") {
        config.llm.disable_thinking = match strategy.to_ascii_lowercase().as_str() {
            "true" | "1" | "vllm" => DisableThinkingStrategy::Vllm,
            "deepseek" => DisableThinkingStrategy::DeepSeek,
            "dashscope" => DisableThinkingStrategy::DashScope,
            "openai" => DisableThinkingStrategy::OpenAI,
            "anthropic" => DisableThinkingStrategy::Anthropic,
            "kimi" => DisableThinkingStrategy::Kimi,
            "gemini" => DisableThinkingStrategy::Gemini,
            _ => DisableThinkingStrategy::Disabled,
        };
    }
}

/// Discover and load the standalone gateway configuration using the public TS
/// precedence. An explicit CLI path always wins, including when it does not
/// exist (so the resulting error names the requested file).
pub fn discover_and_load_config(
    explicit: Option<&Path>,
) -> Result<(PathBuf, GatewayConfig), StartupError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = discover_config_path_with(
        explicit,
        |key| std::env::var_os(key),
        &cwd,
        |candidate| candidate.is_file(),
    );
    match path {
        Ok(path) => {
            let config = load_config(&path)?;
            Ok((path, config))
        }
        Err(StartupError::NotFound { .. }) => {
            let mut config =
                GatewayConfig::from_yaml_str("{}").map_err(|error| StartupError::Invalid {
                    path: "<environment>".to_string(),
                    message: error.to_string(),
                })?;
            apply_environment_overrides(&mut config, true);
            config.data.base_dir = resolve_gateway_data_dir(&config.data.base_dir)
                .to_string_lossy()
                .into_owned();
            validate_external_llm(&config)?;
            Ok((PathBuf::from("<environment>"), config))
        }
        Err(error) => Err(error),
    }
}

fn discover_config_path_with(
    explicit: Option<&Path>,
    get_env: impl Fn(&str) -> Option<std::ffi::OsString>,
    cwd: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf, StartupError> {
    #[cfg(windows)]
    let home_keys = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let home_keys = ["HOME", "USERPROFILE"];
    let home = home_keys
        .into_iter()
        .find_map(|key| get_env(key).filter(|value| !value.is_empty()))
        .map(PathBuf::from);
    let expand = |path: &Path| {
        let expanded = expand_home(path, home.as_deref());
        if expanded.is_absolute() {
            expanded
        } else {
            cwd.join(expanded)
        }
    };

    if let Some(path) = explicit {
        return Ok(expand(path));
    }
    if let Some(path) = get_env("AEON_MEMORY_GATEWAY_CONFIG").filter(|value| !value.is_empty()) {
        let path = expand(Path::new(&path));
        if exists(&path) {
            return Ok(path);
        }
    }

    let default_data_dir = default_gateway_data_dir();
    let candidates = [
        cwd.join("aeon-memory.yaml"),
        cwd.join("aeon-memory.json"),
        default_data_dir.join("aeon-memory.yaml"),
        default_data_dir.join("aeon-memory.json"),
    ];
    if let Some(path) = candidates.iter().find(|candidate| exists(candidate)) {
        return Ok(path.clone());
    }
    Err(StartupError::NotFound {
        searched: candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    })
}

fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home.map_or_else(|| path.to_path_buf(), Path::to_path_buf);
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.map_or_else(|| path.to_path_buf(), |base| base.join(rest));
    }
    path.to_path_buf()
}

pub fn validate_external_llm(config: &GatewayConfig) -> Result<(), StartupError> {
    if config.llm.base_url.trim().is_empty() {
        return Err(StartupError::MissingLlm("llm.baseUrl is empty"));
    }
    if config.llm.api_key.trim().is_empty() {
        return Err(StartupError::MissingLlm("llm.apiKey is empty"));
    }
    if config.llm.model.trim().is_empty() {
        return Err(StartupError::MissingLlm("llm.model is empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<std::ffi::OsString> {
        None
    }

    #[test]
    fn explicit_config_wins_even_before_existence_check() {
        let path = discover_config_path_with(
            Some(Path::new("chosen/config.json")),
            no_env,
            Path::new("/cwd"),
            |_| false,
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/cwd/chosen/config.json"));
    }

    #[test]
    fn environment_config_expands_userprofile() {
        let env = |key: &str| match key {
            "USERPROFILE" => Some(std::ffi::OsString::from("/profiles/alice")),
            "AEON_MEMORY_GATEWAY_CONFIG" => Some(std::ffi::OsString::from("~/aeon-memory.json")),
            _ => None,
        };
        let path = discover_config_path_with(None, env, Path::new("/cwd"), |candidate| {
            candidate == Path::new("/profiles/alice/aeon-memory.json")
        })
        .unwrap();
        assert_eq!(path, PathBuf::from("/profiles/alice/aeon-memory.json"));
    }

    #[test]
    fn cwd_yaml_precedes_json() {
        let path = discover_config_path_with(None, no_env, Path::new("/cwd"), |candidate| {
            candidate == Path::new("/cwd/aeon-memory.yaml")
                || candidate == Path::new("/cwd/aeon-memory.json")
        })
        .unwrap();
        assert_eq!(path, PathBuf::from("/cwd/aeon-memory.yaml"));
    }

    #[test]
    fn not_found_error_lists_compatible_names() {
        let error = discover_config_path_with(None, no_env, Path::new("/cwd"), |_| false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("aeon-memory.yaml"));
        assert!(error.contains("aeon-memory.json"));
    }

    #[test]
    fn loads_original_yaml_without_data_section() {
        let root =
            std::env::temp_dir().join(format!("aeon-memory-config-compat-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("aeon-memory.yaml");
        std::fs::write(
            &path,
            r#"server:
  port: 8420
llm:
  baseUrl: https://example.invalid/v1
  apiKey: test
  model: test
memory:
  storeBackend: sqlite
"#,
        )
        .unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(config.server.port, 8420);
        let normalized = config.data.base_dir.replace('\\', "/");
        assert!(normalized.ends_with("aeon-memory/data"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_gateway_yaml_and_json_use_typescript_memory_normalization() {
        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/embedding_gateway_oracle.json"
        ))
        .unwrap();
        for (name, content) in [
            (
                "aeon-memory.yaml",
                r#"llm:
  baseUrl: https://example.invalid/v1
  apiKey: test
  model: test
memory:
  recall:
    strategy: definitely-invalid
  embedding:
    enabled: true
    provider: none
"#,
            ),
            (
                "aeon-memory.json",
                r#"{"llm":{"baseUrl":"https://example.invalid/v1","apiKey":"test","model":"test"},"memory":{"recall":{"strategy":"definitely-invalid"},"embedding":{"enabled":true,"provider":"none"}}}"#,
            ),
        ] {
            let root = std::env::temp_dir().join(format!(
                "aeon-memory-config-normalize-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(name);
            std::fs::write(&path, content).unwrap();
            let config = load_config(&path).unwrap();
            assert!(matches!(
                config.memory.recall.strategy,
                aeon_memory_core::config::RecallStrategy::Hybrid
            ));
            assert_eq!(
                config.memory.embedding.enabled,
                oracle["config"]["providerNoneEnabled"].as_bool().unwrap()
            );
            assert_eq!(config.memory.embedding.provider, "none");
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn expands_original_whole_value_environment_placeholders() {
        let mut yaml: serde_yaml::Value = serde_yaml::from_str(
            "llm:\n  apiKey: '${API_KEY}'\n  model: fixed\narray: ['${MISSING}']\n",
        )
        .unwrap();
        expand_yaml_env(&mut yaml, &|key| {
            (key == "API_KEY").then(|| "secret".to_string())
        });
        assert_eq!(yaml["llm"]["apiKey"].as_str(), Some("secret"));
        assert_eq!(yaml["llm"]["model"].as_str(), Some("fixed"));
        assert_eq!(yaml["array"][0].as_str(), Some(""));
    }
}
