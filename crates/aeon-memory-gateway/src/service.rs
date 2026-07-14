use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Internal(String),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecallRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub session_key: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecallResponse {
    pub context: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepend_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    pub memory_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureRequest {
    #[serde(default)]
    pub user_content: String,
    #[serde(default)]
    pub assistant_content: String,
    #[serde(default)]
    pub session_key: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub messages: Option<Vec<Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaptureResponse {
    pub l0_recorded: usize,
    pub scheduler_notified: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemorySearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default, rename = "type")]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub scene: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemorySearchResponse {
    pub results: String,
    pub total: usize,
    pub strategy: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationSearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub session_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConversationSearchResponse {
    pub results: String,
    pub total: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionEndRequest {
    #[serde(default)]
    pub session_key: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionEndResponse {
    pub flushed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeedRequest {
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub strict_round_role: Option<bool>,
    #[serde(default)]
    pub auto_fill_timestamps: Option<bool>,
    #[serde(default)]
    pub config_override: Option<serde_json::Map<String, Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SeedResponse {
    pub sessions_processed: usize,
    pub rounds_processed: usize,
    pub messages_processed: usize,
    pub l0_recorded: usize,
    pub duration_ms: u64,
    pub output_dir: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoreHealth {
    #[serde(rename = "vectorStore")]
    pub vector_store: bool,
    #[serde(rename = "embeddingService")]
    pub embedding_service: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime: u64,
    pub stores: StoreHealth,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatusResponse {
    pub l0_records: u64,
    pub l1_records: u64,
    pub sessions: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BeforePromptRequest {
    pub agent_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub user_prompt: String,
    #[serde(default)]
    pub messages: Vec<Value>,
    pub context_window: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BeforePromptResponse {
    pub messages: Vec<Value>,
    pub context: Value,
    pub active_mmd: Option<String>,
    pub offload_enabled: bool,
    #[serde(default)]
    pub l1_entries: Vec<aeon_memory_core::offload::OffloadEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l15_judgment: Option<aeon_memory_core::offload::parser::TaskJudgment>,
    pub l2_updated: bool,
    pub compression: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AfterToolRequest {
    pub agent_id: String,
    pub session_id: String,
    pub tool: aeon_memory_core::offload::ToolPair,
    #[serde(default)]
    pub messages: Vec<Value>,
    pub context_window: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AfterToolResponse {
    pub messages: Vec<Value>,
    pub buffered_pairs: usize,
    pub l1_entries: Vec<aeon_memory_core::offload::OffloadEntry>,
    pub l2_updated: bool,
    pub context: Value,
    pub compression: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LlmOutputRequest {
    pub agent_id: String,
    pub session_id: String,
    pub assistant_message: Value,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LlmOutputResponse {
    pub force_l1: bool,
    pub processed_entries: usize,
    #[serde(default)]
    pub l1_entries: Vec<aeon_memory_core::offload::OffloadEntry>,
    pub l2_updated: bool,
    pub state: aeon_memory_core::offload::PluginState,
}

#[async_trait]
pub trait AeonMemoryService: Send + Sync + 'static {
    async fn health(&self) -> ServiceResult<HealthResponse>;
    async fn recall(&self, request: RecallRequest) -> ServiceResult<RecallResponse>;
    async fn capture(&self, request: CaptureRequest) -> ServiceResult<CaptureResponse>;
    async fn search_memories(
        &self,
        request: MemorySearchRequest,
    ) -> ServiceResult<MemorySearchResponse>;
    async fn search_conversations(
        &self,
        request: ConversationSearchRequest,
    ) -> ServiceResult<ConversationSearchResponse>;
    async fn end_session(&self, request: SessionEndRequest) -> ServiceResult<SessionEndResponse>;
    async fn seed(&self, request: SeedRequest) -> ServiceResult<SeedResponse>;
    async fn before_prompt(
        &self,
        request: BeforePromptRequest,
    ) -> ServiceResult<BeforePromptResponse>;
    async fn after_tool(&self, request: AfterToolRequest) -> ServiceResult<AfterToolResponse>;
    async fn llm_output(&self, request: LlmOutputRequest) -> ServiceResult<LlmOutputResponse>;

    async fn status(&self) -> ServiceResult<StatusResponse>;
    async fn show_persona(&self) -> ServiceResult<String>;
    async fn show_scenes(&self) -> ServiceResult<Vec<String>>;
}
