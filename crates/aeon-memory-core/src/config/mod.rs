// port of src/config.ts (AeonMemoryConfig + parseConfig)
// Standalone CLI/HTTP configuration.

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════════════
// AeonMemoryConfig — the full plugin config
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AeonMemoryConfig {
    pub timezone: String,
    pub capture: CaptureConfig,
    pub extraction: ExtractionConfig,
    pub persona: PersonaConfig,
    pub pipeline: PipelineConfig,
    pub recall: RecallConfig,
    pub embedding: EmbeddingConfig,
    pub store_backend: StoreBackend,
    pub tcvdb: TcvdbConfig,
    pub bm25: BM25Config,
    pub memory_cleanup: MemoryCleanupConfig,
    pub report: ReportConfig,
    pub llm: LlmOverrideConfig,
    pub offload: OffloadConfig,
}

impl Default for AeonMemoryConfig {
    fn default() -> Self {
        Self {
            timezone: "system".to_string(),
            capture: CaptureConfig::default(),
            extraction: ExtractionConfig::default(),
            persona: PersonaConfig::default(),
            pipeline: PipelineConfig::default(),
            recall: RecallConfig::default(),
            embedding: EmbeddingConfig::default(),
            store_backend: StoreBackend::default(),
            tcvdb: TcvdbConfig::default(),
            bm25: BM25Config::default(),
            memory_cleanup: MemoryCleanupConfig::default(),
            report: ReportConfig::default(),
            llm: LlmOverrideConfig::default(),
            offload: OffloadConfig::default(),
        }
    }
}

// ── Capture ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CaptureConfig {
    pub enabled: bool,
    pub exclude_agents: Vec<String>,
    pub l0l1_retention_days: u32,
    pub allow_aggressive_cleanup: bool,
    pub clean_time: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exclude_agents: Vec::new(),
            l0l1_retention_days: 0,
            allow_aggressive_cleanup: false,
            clean_time: "03:00".to_string(),
        }
    }
}

// ── Extraction (L1) ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExtractionConfig {
    pub enabled: bool,
    pub enable_dedup: bool,
    pub max_memories_per_session: u32,
    pub model: Option<String>,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_dedup: true,
            max_memories_per_session: 20,
            model: None,
        }
    }
}

// ── Persona (L2/L3) ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PersonaConfig {
    pub trigger_every_n: u32,
    pub max_scenes: u32,
    pub backup_count: u32,
    pub scene_backup_count: u32,
    pub model: Option<String>,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            trigger_every_n: 50,
            max_scenes: 15,
            backup_count: 3,
            scene_backup_count: 10,
            model: None,
        }
    }
}

// ── Pipeline ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PipelineConfig {
    pub every_n_conversations: u32,
    pub enable_warmup: bool,
    pub l1_idle_timeout_seconds: u32,
    pub l2_delay_after_l1_seconds: u32,
    pub l2_min_interval_seconds: u32,
    pub l2_max_interval_seconds: u32,
    pub session_active_window_hours: u32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            every_n_conversations: 5,
            enable_warmup: true,
            l1_idle_timeout_seconds: 600,
            l2_delay_after_l1_seconds: 10,
            l2_min_interval_seconds: 900,
            l2_max_interval_seconds: 3600,
            session_active_window_hours: 24,
        }
    }
}

// ── Recall ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecallConfig {
    pub enabled: bool,
    pub max_results: u32,
    pub max_chars_per_memory: u32,
    pub max_total_recall_chars: u32,
    pub score_threshold: f64,
    pub strategy: RecallStrategy,
    pub timeout_ms: u64,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_results: 5,
            max_chars_per_memory: 0,
            max_total_recall_chars: 0,
            score_threshold: 0.3,
            strategy: RecallStrategy::Hybrid,
            timeout_ms: 5000,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum RecallStrategy {
    #[serde(rename = "keyword")]
    Keyword,
    #[serde(rename = "embedding")]
    Embedding,
    #[default]
    #[serde(rename = "hybrid")]
    Hybrid,
}

// ── Embedding ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: u32,
    pub send_dimensions: bool,
    pub conflict_recall_top_k: u32,
    pub proxy_url: Option<String>,
    pub max_input_chars: u32,
    pub timeout_ms: u64,
    pub recall_timeout_ms: Option<u64>,
    pub capture_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub config_error: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "none".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            dimensions: 0,
            send_dimensions: true,
            conflict_recall_top_k: 5,
            proxy_url: None,
            max_input_chars: 5000,
            timeout_ms: 10_000,
            recall_timeout_ms: None,
            capture_timeout_ms: None,
            config_error: None,
        }
    }
}

// ── Store backend ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum StoreBackend {
    #[default]
    #[serde(rename = "sqlite")]
    Sqlite,
    #[serde(rename = "tcvdb")]
    Tcvdb,
}

// ── TCVDB ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TcvdbConfig {
    pub url: String,
    pub username: String,
    pub api_key: String,
    pub database: String,
    pub alias: String,
    pub embedding_model: String,
    pub timeout: u64,
    pub ca_pem_path: Option<String>,
}

impl Default for TcvdbConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            username: "root".to_string(),
            api_key: String::new(),
            database: String::new(),
            alias: String::new(),
            embedding_model: "bge-large-zh".to_string(),
            timeout: 10000,
            ca_pem_path: None,
        }
    }
}

// ── BM25 ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BM25Config {
    pub enabled: bool,
    pub language: Bm25Language,
}

impl Default for BM25Config {
    fn default() -> Self {
        Self {
            enabled: true,
            language: Bm25Language::Zh,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum Bm25Language {
    #[default]
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "en")]
    En,
}

// ── Memory cleanup ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MemoryCleanupConfig {
    pub retention_days: Option<u32>,
    pub enabled: bool,
    pub clean_time: String,
}

impl Default for MemoryCleanupConfig {
    fn default() -> Self {
        Self {
            retention_days: None,
            enabled: false,
            clean_time: "03:00".to_string(),
        }
    }
}

// ── Report ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportConfig {
    pub enabled: bool,
    pub r#type: String,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            r#type: "local".to_string(),
        }
    }
}

// ── LLM override ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LlmOverrideConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout_ms: u64,
    pub disable_thinking: DisableThinkingStrategy,
}

impl Default for LlmOverrideConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: 4096,
            timeout_ms: 120_000,
            disable_thinking: DisableThinkingStrategy::Disabled,
        }
    }
}

/// Strategy for disabling LLM thinking/reasoning.
/// Port of src/utils/no-think-fetch.ts DisableThinkingStrategy type.
///
/// serde representation: boolean false = disabled, string = strategy name.
#[derive(Clone, Debug, Default)]
pub enum DisableThinkingStrategy {
    #[default]
    Disabled,
    Vllm,
    DeepSeek,
    DashScope,
    OpenAI,
    Anthropic,
    Kimi,
    Gemini,
}

impl serde::Serialize for DisableThinkingStrategy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Vllm => serializer.serialize_str("vllm"),
            Self::DeepSeek => serializer.serialize_str("deepseek"),
            Self::DashScope => serializer.serialize_str("dashscope"),
            Self::OpenAI => serializer.serialize_str("openai"),
            Self::Anthropic => serializer.serialize_str("anthropic"),
            Self::Kimi => serializer.serialize_str("kimi"),
            Self::Gemini => serializer.serialize_str("gemini"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for DisableThinkingStrategy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Use untagged: accept bool false, or string strategy name
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Bool(bool),
            Str(String),
        }
        match Helper::deserialize(deserializer)? {
            Helper::Bool(false) => Ok(Self::Disabled),
            Helper::Bool(true) => Err(serde::de::Error::custom("disableThinking cannot be true")),
            Helper::Str(s) => match s.as_str() {
                "vllm" => Ok(Self::Vllm),
                "deepseek" => Ok(Self::DeepSeek),
                "dashscope" => Ok(Self::DashScope),
                "openai" => Ok(Self::OpenAI),
                "anthropic" => Ok(Self::Anthropic),
                "kimi" => Ok(Self::Kimi),
                "gemini" => Ok(Self::Gemini),
                _ => Err(serde::de::Error::unknown_variant(
                    &s,
                    &[
                        "vllm",
                        "deepseek",
                        "dashscope",
                        "openai",
                        "anthropic",
                        "kimi",
                        "gemini",
                    ],
                )),
            },
        }
    }
}

// ── Offload ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OffloadConfig {
    pub enabled: bool,
    pub mode: OffloadMode,
    pub model: Option<String>,
    pub temperature: f64,
    pub disable_thinking: DisableThinkingStrategy,
    pub force_trigger_threshold: u32,
    pub data_dir: Option<String>,
    pub default_context_window: u32,
    pub max_pairs_per_batch: u32,
    pub l2_null_threshold: u32,
    pub l2_timeout_seconds: u32,
    pub mild_offload_ratio: f64,
    pub aggressive_compress_ratio: f64,
    pub mmd_max_token_ratio: f64,
    pub backend_url: Option<String>,
    pub backend_api_key: Option<String>,
    pub backend_timeout_ms: u64,
    pub offload_retention_days: u32,
    pub log_max_size_mb: u32,
    pub user_id: Option<String>,
}

impl Default for OffloadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: OffloadMode::Local,
            model: None,
            temperature: 0.2,
            disable_thinking: DisableThinkingStrategy::Disabled,
            force_trigger_threshold: 4,
            data_dir: None,
            default_context_window: 200_000,
            max_pairs_per_batch: 20,
            l2_null_threshold: 4,
            l2_timeout_seconds: 300,
            mild_offload_ratio: 0.5,
            aggressive_compress_ratio: 0.85,
            mmd_max_token_ratio: 0.2,
            backend_url: None,
            backend_api_key: None,
            backend_timeout_ms: 120_000,
            offload_retention_days: 0,
            log_max_size_mb: 50,
            user_id: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum OffloadMode {
    #[default]
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "backend")]
    Backend,
    #[serde(rename = "collect")]
    Collect,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Gateway configuration.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GatewayConfig {
    pub server: ServerConfig,
    pub data: DataConfig,
    pub llm: StandaloneLlmConfig,
    pub memory: AeonMemoryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8420,
            host: "127.0.0.1".to_string(),
            api_key: None,
            cors_origins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DataConfig {
    pub base_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StandaloneLlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout_ms: u64,
    pub disable_thinking: DisableThinkingStrategy,
}

impl Default for StandaloneLlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            max_tokens: 4096,
            timeout_ms: 120_000,
            disable_thinking: DisableThinkingStrategy::Disabled,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Parsing helpers
// ═══════════════════════════════════════════════════════════════════════════════

impl AeonMemoryConfig {
    /// Parse memory config from a raw JSON value (e.g. from plugin config section).
    /// Mirrors src/config.ts parseConfig().
    pub fn from_json_value(mut value: serde_json::Value) -> crate::error::AeonMemoryResult<Self> {
        normalize_legacy_input(&mut value);
        // TS compatibility: missing/invalid offload.mode auto-selects backend
        // when a non-empty backendUrl exists, otherwise local.
        if let Some(offload) = value
            .as_object_mut()
            .and_then(|root| root.get_mut("offload"))
            .and_then(serde_json::Value::as_object_mut)
        {
            let valid_mode = offload
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|mode| matches!(mode, "local" | "backend" | "collect"));
            if !valid_mode {
                let mode = if offload
                    .get("backendUrl")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|url| !url.is_empty())
                {
                    "backend"
                } else {
                    "local"
                };
                offload.insert("mode".into(), serde_json::Value::String(mode.into()));
            }
        }
        let mut cfg: Self = serde_json::from_value(value)?;

        // Validate embedding config like TS parseConfig does
        normalize_embedding(&mut cfg.embedding);

        // Sync memory_cleanup.retention_days from capture.l0l1_retention_days,
        // matching TS parseConfig behavior (memoryCleanup derives from capture.l0l1RetentionDays).
        if cfg.memory_cleanup.retention_days.is_none() && cfg.capture.l0l1_retention_days > 0 {
            cfg.memory_cleanup.retention_days = Some(cfg.capture.l0l1_retention_days);
        }

        // Validate retention days
        Self::validate_cleanup(
            &mut cfg.memory_cleanup,
            cfg.capture.allow_aggressive_cleanup,
        );
        cfg.capture.l0l1_retention_days = cfg.memory_cleanup.retention_days.unwrap_or(0);
        cfg.capture.clean_time = normalize_clean_time(&cfg.capture.clean_time);
        cfg.memory_cleanup.clean_time = cfg.capture.clean_time.clone();
        if cfg.offload.offload_retention_days < 3 {
            cfg.offload.offload_retention_days = 0;
        }

        Ok(cfg)
    }

    pub fn from_json_str(json: &str) -> crate::error::AeonMemoryResult<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Self::from_json_value(value)
    }

    fn validate_cleanup(cleanup: &mut MemoryCleanupConfig, allow_aggressive: bool) {
        if let Some(days) = cleanup.retention_days {
            let valid = days >= 3 || (allow_aggressive && days > 0);
            if valid {
                cleanup.enabled = true;
            } else {
                cleanup.enabled = false;
                cleanup.retention_days = None;
            }
        } else {
            cleanup.enabled = false;
        }
    }
}

fn normalize_legacy_input(value: &mut serde_json::Value) {
    let Some(root) = value.as_object_mut() else {
        return;
    };
    if !matches!(
        root.get("storeBackend").and_then(|v| v.as_str()),
        Some("sqlite" | "tcvdb") | None
    ) {
        root.insert("storeBackend".into(), "sqlite".into());
    }
    if let Some(recall) = root.get_mut("recall").and_then(|v| v.as_object_mut())
        && !matches!(
            recall.get("strategy").and_then(|v| v.as_str()),
            Some("keyword" | "embedding" | "hybrid") | None
        )
    {
        recall.insert("strategy".into(), "hybrid".into());
    }
    if let Some(offload) = root.get_mut("offload").and_then(|v| v.as_object_mut())
        && !matches!(
            offload.get("mode").and_then(|v| v.as_str()),
            Some("local" | "backend" | "collect") | None
        )
    {
        let mode = if offload
            .get("backendUrl")
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.is_empty())
        {
            "backend"
        } else {
            "local"
        };
        offload.insert("mode".into(), mode.into());
    }
}

fn normalize_clean_time(value: &str) -> String {
    let Some((hour, minute)) = value.trim().split_once(':') else {
        return "03:00".into();
    };
    if !(1..=2).contains(&hour.len()) || minute.len() != 2 {
        return "03:00".into();
    }
    let (Ok(hour), Ok(minute)) = (hour.parse::<u8>(), minute.parse::<u8>()) else {
        return "03:00".into();
    };
    if hour > 23 || minute > 59 {
        return "03:00".into();
    }
    format!("{hour:02}:{minute:02}")
}

fn normalize_embedding(cfg: &mut EmbeddingConfig) {
    cfg.config_error = None;
    match cfg.provider.as_str() {
        "none" => cfg.enabled = false,
        "local" => {
            cfg.enabled = false;
            cfg.provider = "none".into();
            cfg.config_error = Some("Local embedding provider is not available in user config. Please configure a remote embedding provider (e.g. openai, deepseek). Embedding has been disabled.".into());
        }
        provider => {
            let mut missing = Vec::new();
            if provider == "qclaw" && cfg.proxy_url.as_deref().is_none_or(str::is_empty) {
                missing.push("proxyUrl")
            }
            if provider == "qclaw" && cfg.base_url.is_empty() {
                missing.push("baseUrl")
            }
            if cfg.api_key.is_empty() {
                missing.push("apiKey")
            }
            if provider != "qclaw" && cfg.base_url.is_empty() {
                missing.push("baseUrl")
            }
            if cfg.model.is_empty() {
                missing.push("model")
            }
            if cfg.dimensions == 0 {
                missing.push("dimensions")
            }
            if !missing.is_empty() {
                cfg.enabled = false;
                let prefix = if provider == "qclaw" {
                    "Embedding provider 'qclaw' requires 'proxyUrl', 'baseUrl', 'apiKey', 'model', and 'dimensions' to be set.".to_string()
                } else {
                    format!(
                        "Remote embedding provider '{provider}' requires 'apiKey', 'baseUrl', 'model', and 'dimensions' to be set."
                    )
                };
                cfg.config_error = Some(format!(
                    "{prefix} Missing: {}. Embedding has been disabled.",
                    missing.join(", ")
                ));
            }
        }
    }
}

impl GatewayConfig {
    /// Load gateway config from a YAML/JSON config file.
    /// Loads YAML or JSON and applies standalone defaults.
    pub fn from_yaml_str(yaml: &str) -> crate::error::AeonMemoryResult<Self> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        Self::from_json_value(serde_json::to_value(value)?)
    }

    pub fn from_json_str(json: &str) -> crate::error::AeonMemoryResult<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Self::from_json_value(value)
    }

    fn from_json_value(mut value: serde_json::Value) -> crate::error::AeonMemoryResult<Self> {
        let root = value.as_object_mut().ok_or_else(|| {
            crate::error::AeonMemoryCoreError::InvalidInput(
                "gateway config must be an object".into(),
            )
        })?;
        let memory = AeonMemoryConfig::from_json_value(
            root.remove("memory")
                .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        )?;
        root.insert("memory".into(), serde_json::to_value(memory)?);
        let mut cfg: Self = serde_json::from_value(value)?;
        if cfg.data.base_dir.is_empty() {
            cfg.data.base_dir = default_gateway_data_dir().to_string_lossy().into_owned();
        }
        Ok(cfg)
    }
}

/// Resolve the standalone data directory.
pub fn resolve_gateway_data_dir(configured: &str) -> std::path::PathBuf {
    resolve_gateway_data_dir_with(configured, |key| std::env::var_os(key))
}

/// Resolve the default standalone store without a configured `data.baseDir`.
pub fn default_gateway_data_dir() -> std::path::PathBuf {
    resolve_gateway_data_dir("")
}

/// Default context-offload root used by the pinned OpenClaw plugin. This is
/// deliberately independent from the standalone memory store root.
pub fn default_offload_data_root() -> std::path::PathBuf {
    default_offload_data_root_with(|key| std::env::var_os(key))
}

fn default_offload_data_root_with(
    get_env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    platform_home_with(get_env)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".openclaw")
        .join("context-offload")
}

fn platform_home_with(
    get_env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let keys = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let keys = ["HOME", "USERPROFILE"];
    keys.into_iter()
        .find_map(|key| get_env(key).filter(|value| !value.is_empty()))
        .map(std::path::PathBuf::from)
}

fn resolve_gateway_data_dir_with(
    configured: &str,
    get_env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    let home = platform_home_with(&get_env);

    let expand = |value: std::ffi::OsString| {
        let path = std::path::PathBuf::from(&value);
        let text = path.to_string_lossy();
        if text == "~" {
            return home.clone().unwrap_or(path);
        }
        if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
            let rest = rest.to_string();
            return home.clone().map_or(path, |base| base.join(rest));
        }
        path
    };

    let selected = get_env("AEON_MEMORY_DATA_DIR")
        .filter(|value| !value.is_empty())
        .map(&expand)
        .or_else(|| {
            (!configured.trim().is_empty())
                .then(|| expand(std::ffi::OsString::from(configured.trim())))
        })
        .unwrap_or_else(|| {
            let Some(home) = home.clone() else {
                return std::env::temp_dir().join(".aeon-memory").join("data");
            };
            home.join(".aeon-memory").join("data")
        });

    if selected.is_absolute() {
        selected
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(selected)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = AeonMemoryConfig::default();
        assert_eq!(cfg.timezone, "system");
        assert!(cfg.capture.enabled);
        assert!(cfg.extraction.enabled);
        assert!(cfg.recall.enabled);
        assert_eq!(
            cfg.recall.strategy as usize,
            RecallStrategy::Hybrid as usize
        );
        assert_eq!(cfg.recall.max_results, 5);
        assert_eq!(cfg.store_backend as usize, StoreBackend::Sqlite as usize);
        assert!(!cfg.offload.enabled);
        assert!(!cfg.llm.enabled);
        assert!(!cfg.embedding.enabled);
        assert_eq!(cfg.pipeline.every_n_conversations, 5);
        assert!(cfg.pipeline.enable_warmup);
    }

    #[test]
    fn test_parse_minimal_config() {
        let json = r#"{"capture": {"enabled": true}}"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();
        assert!(cfg.capture.enabled);
        // All other fields should have defaults
        assert_eq!(cfg.recall.max_results, 5);
        assert_eq!(cfg.pipeline.every_n_conversations, 5);
    }

    fn platform_aware_path(base: &str, tail: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(base).join(tail);
        if !p.is_absolute() {
            // On Windows, forward-slash-only paths like "/srv/override" lack
            // a drive prefix so is_absolute() returns false. Match the
            // resolver behavior: join with current working directory.
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(&p)
        } else {
            p
        }
    }

    #[test]
    fn gateway_data_dir_uses_unix_home() {
        let env = |key: &str| match key {
            "HOME" => Some(std::ffi::OsString::from("/home/alice")),
            _ => None,
        };
        let canonical = resolve_gateway_data_dir_with("", env);
        assert_eq!(
            canonical,
            platform_aware_path("/home/alice", ".aeon-memory/data")
        );
    }

    #[test]
    fn gateway_data_dir_supports_userprofile_and_tilde() {
        let env = |key: &str| match key {
            "USERPROFILE" => Some(std::ffi::OsString::from("/profiles/Alice")),
            "AEON_MEMORY_DATA_DIR" => Some(std::ffi::OsString::from("~/aeon-memory")),
            _ => None,
        };
        let resolved = resolve_gateway_data_dir_with("ignored", env);
        assert_eq!(
            resolved,
            platform_aware_path("/profiles/Alice", "aeon-memory")
        );
    }

    #[test]
    fn gateway_data_dir_environment_precedence() {
        let env = |key: &str| match key {
            "HOME" => Some(std::ffi::OsString::from("/home/alice")),
            "AEON_MEMORY_DATA_DIR" => Some(std::ffi::OsString::from("/srv/override")),
            _ => None,
        };
        assert_eq!(
            resolve_gateway_data_dir_with("/srv/configured", env),
            platform_aware_path("/srv/override", "")
        );
    }

    #[test]
    fn test_parse_full_config() {
        let json = r#"{
            "timezone": "Asia/Shanghai",
            "capture": {
                "enabled": true,
                "excludeAgents": ["bench-*"],
                "l0l1RetentionDays": 7
            },
            "extraction": {
                "enabled": true,
                "enableDedup": true,
                "maxMemoriesPerSession": 10
            },
            "persona": {
                "triggerEveryN": 30,
                "maxScenes": 20
            },
            "pipeline": {
                "everyNConversations": 3,
                "enableWarmup": false
            },
            "recall": {
                "enabled": true,
                "maxResults": 10,
                "strategy": "hybrid"
            },
            "embedding": {
                "enabled": true,
                "provider": "openai",
                "baseUrl": "https://api.openai.com/v1",
                "apiKey": "sk-test",
                "model": "text-embedding-3-small",
                "dimensions": 1536
            },
            "storeBackend": "sqlite",
            "llm": {
                "enabled": true,
                "baseUrl": "https://api.openai.com/v1",
                "apiKey": "sk-llm-test",
                "model": "gpt-4o",
                "maxTokens": 8192
            }
        }"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();

        assert_eq!(cfg.timezone, "Asia/Shanghai");
        assert_eq!(cfg.capture.exclude_agents.len(), 1);
        assert_eq!(cfg.capture.l0l1_retention_days, 7);
        assert!(!cfg.capture.allow_aggressive_cleanup);
        assert_eq!(cfg.extraction.max_memories_per_session, 10);
        assert_eq!(cfg.persona.trigger_every_n, 30);
        assert_eq!(cfg.pipeline.every_n_conversations, 3);
        assert!(!cfg.pipeline.enable_warmup);
        assert_eq!(cfg.recall.max_results, 10);
        assert!(cfg.embedding.enabled);
        assert_eq!(cfg.embedding.provider, "openai");
        assert_eq!(cfg.embedding.model, "text-embedding-3-small");
        assert_eq!(cfg.embedding.dimensions, 1536);
        assert!(cfg.llm.enabled);
        assert_eq!(cfg.llm.max_tokens, 8192);
    }

    #[test]
    fn test_embedding_none_disables() {
        let json = r#"{"embedding": {"provider": "none", "enabled": true}}"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();
        assert!(!cfg.embedding.enabled);
    }

    #[test]
    fn test_invalid_retention_aggressive() {
        // retention=1 without aggressive → disabled
        let json = r#"{"capture": {"enabled": true, "l0l1RetentionDays": 1}}"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();
        assert!(!cfg.memory_cleanup.enabled);
        assert!(cfg.memory_cleanup.retention_days.is_none());
    }

    #[test]
    fn test_retention_with_aggressive() {
        let json = r#"{"capture": {"enabled": true, "l0l1RetentionDays": 1, "allowAggressiveCleanup": true}}"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();
        assert!(cfg.memory_cleanup.enabled);
        assert_eq!(cfg.memory_cleanup.retention_days, Some(1));
    }

    #[test]
    fn test_gateway_config_minimal() {
        let yaml = r#"
server:
  port: 8420
  host: "127.0.0.1"
data:
  baseDir: "/tmp/aeon-memory-test"
llm:
  baseUrl: "https://api.openai.com/v1"
  apiKey: "sk-test"
  model: "gpt-4o"
memory:
  capture:
    enabled: true
"#;
        let cfg = GatewayConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(cfg.server.port, 8420);
        assert_eq!(cfg.llm.model, "gpt-4o");
        assert!(cfg.memory.capture.enabled);
        assert_eq!(cfg.memory.recall.max_results, 5); // default
    }

    #[test]
    fn test_gateway_config_default_data_dir() {
        let json = r#"{"server": {"port": 8420}, "data": {}, "llm": {"apiKey": "sk-test"}}"#;
        let cfg = GatewayConfig::from_json_str(json).unwrap();
        // Should have a default data dir since empty was given
        assert!(!cfg.data.base_dir.is_empty());
    }

    #[test]
    fn test_serde_field_compatibility() {
        // Verify camelCase serialization matches TS naming convention
        let cfg = AeonMemoryConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();

        // TS uses camelCase: everyNConversations (not every_n_conversations)
        assert!(json.get("pipeline").is_some());
        assert!(json["pipeline"].get("everyNConversations").is_some());
        assert!(json["pipeline"].get("every_n_conversations").is_none());

        // storeBackend (not store_backend)
        assert!(json.get("storeBackend").is_some());
        assert!(json.get("store_backend").is_none());
    }

    #[test]
    fn test_offload_default_disabled() {
        let cfg = AeonMemoryConfig::default();
        assert!(!cfg.offload.enabled);
        assert_eq!(cfg.offload.temperature, 0.2);
    }

    #[test]
    fn offload_default_root_is_openclaw_compatible() {
        let root = default_offload_data_root_with(|key| match key {
            "HOME" | "USERPROFILE" => Some(std::ffi::OsString::from("/home/fixture")),
            _ => None,
        });
        assert_eq!(
            root,
            std::path::PathBuf::from("/home/fixture/.openclaw/context-offload")
        );
    }

    #[test]
    fn offload_mode_auto_detection_matches_typescript() {
        let backend = AeonMemoryConfig::from_json_value(serde_json::json!({
            "offload": {"backendUrl": "http://backend"}
        }))
        .unwrap();
        assert!(matches!(backend.offload.mode, OffloadMode::Backend));
        let invalid_with_backend = AeonMemoryConfig::from_json_value(serde_json::json!({
            "offload": {"mode": "invalid", "backendUrl": "http://backend"}
        }))
        .unwrap();
        assert!(matches!(
            invalid_with_backend.offload.mode,
            OffloadMode::Backend
        ));
        let invalid_without_backend = AeonMemoryConfig::from_json_value(serde_json::json!({
            "offload": {"mode": "invalid"}
        }))
        .unwrap();
        assert!(matches!(
            invalid_without_backend.offload.mode,
            OffloadMode::Local
        ));
    }

    #[test]
    fn test_plugin_schema_keys() {
        // Verify that the standalone parser accepts the legacy-compatible schema keys.
        // Shape: { capture: {...}, extraction: {...}, persona: {...}, ... }
        let json = r#"{
            "capture": {"enabled": true, "l0l1RetentionDays": 0},
            "extraction": {"enabled": true},
            "persona": {"triggerEveryN": 50},
            "pipeline": {"everyNConversations": 5},
            "recall": {"enabled": true, "strategy": "hybrid"},
            "embedding": {"provider": "none"},
            "tcvdb": {"url": ""},
            "bm25": {"enabled": true, "language": "zh"},
            "report": {"enabled": false},
            "llm": {"enabled": false},
            "offload": {"enabled": false}
        }"#;
        let cfg = AeonMemoryConfig::from_json_str(json).unwrap();
        assert!(cfg.capture.enabled);
        assert!(cfg.extraction.enabled);
        assert_eq!(cfg.persona.trigger_every_n, 50);
        assert_eq!(cfg.pipeline.every_n_conversations, 5);
        assert!(cfg.recall.enabled);
        assert!(!cfg.embedding.enabled);
        assert!(!cfg.llm.enabled);
        assert!(!cfg.offload.enabled);
        assert_eq!(cfg.bm25.language as usize, Bm25Language::Zh as usize);
    }
}
