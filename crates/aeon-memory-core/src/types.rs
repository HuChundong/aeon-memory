// Standalone core contracts shared by the CLI and HTTP transports.

pub type Logger = std::sync::Arc<dyn Log + Send + Sync>;

/// Serialize a finite JSON number with JavaScript `JSON.stringify`'s common
/// integer spelling: `50.0` becomes `50`, while fractions such as `70.5`
/// remain fractional. L1 priorities originate in parsed JSON, so non-finite
/// values cannot enter this path.
pub fn serialize_js_number<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value <= i64::MAX as f64 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

pub trait Log: Send + Sync {
    fn debug(&self, msg: &str);
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str);
    fn error(&self, msg: &str);
}

// ── LLM runner ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileToolPolicy {
    /// Scene extraction may read/edit only the files that existed when the run
    /// started, and may write direct-child Markdown scene files.
    Scene { readable_files: Vec<String> },
    /// Persona generation may only write/edit `persona.md`; scene data is
    /// already embedded in the prompt and no file reads are required.
    Persona,
}

#[derive(Clone, Debug)]
pub struct LlmRunParams {
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub task_id: String,
    pub timeout_ms: Option<u64>,
    pub max_tokens: Option<u32>,
    pub workspace_dir: Option<String>,
    pub file_tool_policy: Option<FileToolPolicy>,
    pub instance_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LlmRunnerCreateOptions {
    pub model_ref: Option<String>,
    pub enable_tools: bool,
}

#[async_trait::async_trait]
pub trait LlmRunner: Send + Sync {
    async fn run(&self, params: LlmRunParams) -> crate::error::AeonMemoryResult<String>;

    async fn run_offload_l1(
        &self,
        params: LlmRunParams,
        _recent_messages: &str,
        _tool_pairs: &[crate::offload::types::ToolPair],
    ) -> crate::error::AeonMemoryResult<String> {
        self.run(params).await
    }

    async fn run_offload_l15(
        &self,
        params: LlmRunParams,
        _recent_messages: &str,
        _current_mmd: Option<(&str, &str, &str)>,
        _available_mmd_metas: &[crate::offload::prompt::MmdMeta],
    ) -> crate::error::AeonMemoryResult<String> {
        self.run(params).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_offload_l2(
        &self,
        params: LlmRunParams,
        _existing_mmd: Option<&str>,
        _new_entries: &[crate::offload::types::OffloadEntry],
        _recent_history: Option<&str>,
        _current_turn: Option<&str>,
        _task_label: &str,
        _mmd_prefix: &str,
        _mmd_char_count: usize,
    ) -> crate::error::AeonMemoryResult<String> {
        self.run(params).await
    }
}

pub trait LlmRunnerFactory: Send + Sync {
    fn create_runner(&self, opts: LlmRunnerCreateOptions) -> Box<dyn LlmRunner>;
}

// ── Completed turn ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CompletedTurn {
    pub user_text: String,
    pub assistant_text: String,
    pub messages: Vec<serde_json::Value>,
    pub session_key: String,
    pub session_id: Option<String>,
    pub started_at: Option<i64>,
    pub original_user_message_count: Option<u32>,
    /// When true, the cursor-based dedup filter is skipped so every message
    /// in this turn is captured as a new entry.  Used by the HTTP shorthand
    /// (messages=None) where each call is an independent single turn.
    pub skip_cursor: bool,
}

// ── Recall result ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RecalledMemory {
    pub content: String,
    pub score: f64,
    pub r#type: String,
}

#[derive(Clone, Debug, Default)]
pub struct RecallResult {
    pub prepend_context: Option<String>,
    pub append_system_context: Option<String>,
    pub recalled_l1_memories: Vec<RecalledMemory>,
    pub recalled_l3_persona: Option<String>,
    pub recall_strategy: String,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureResult {
    pub l0_recorded_count: u32,
    pub scheduler_notified: bool,
    pub l0_vectors_written: u32,
    pub filtered_messages: Vec<FilteredMessage>,
}

#[derive(Clone, Debug)]
pub struct FilteredMessage {
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

// ── Search parameters ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MemorySearchParams {
    pub query: String,
    pub limit: Option<u32>,
    pub r#type: Option<String>,
    pub scene: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ConversationSearchParams {
    pub query: String,
    pub limit: Option<u32>,
    pub session_key: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Store types (port of src/core/store/types.ts)
// ═══════════════════════════════════════════════════════════════════════════════

// ── L1 types ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L1SearchResult {
    pub record_id: String,
    pub content: String,
    pub r#type: String,
    #[serde(serialize_with = "serialize_js_number")]
    pub priority: f64,
    pub scene_name: String,
    pub score: f64,
    pub timestamp_str: String,
    pub timestamp_start: String,
    pub timestamp_end: String,
    pub session_key: String,
    pub session_id: String,
    pub metadata_json: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L1FtsResult {
    pub record_id: String,
    pub content: String,
    pub r#type: String,
    #[serde(serialize_with = "serialize_js_number")]
    pub priority: f64,
    pub scene_name: String,
    pub score: f64,
    pub timestamp_str: String,
    pub timestamp_start: String,
    pub timestamp_end: String,
    pub session_key: String,
    pub session_id: String,
    pub metadata_json: String,
}

#[derive(Clone, Debug, Default)]
pub struct L1QueryFilter {
    pub session_key: Option<String>,
    pub session_id: Option<String>,
    pub updated_after: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L1RecordRow {
    pub record_id: String,
    pub content: String,
    pub r#type: String,
    #[serde(serialize_with = "serialize_js_number")]
    pub priority: f64,
    pub scene_name: String,
    pub session_key: String,
    pub session_id: String,
    pub timestamp_str: String,
    pub timestamp_start: String,
    pub timestamp_end: String,
    pub created_time: String,
    pub updated_time: String,
    pub metadata_json: String,
}

// ── L0 types ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct L0Record {
    pub id: String,
    pub session_key: String,
    pub session_id: String,
    pub role: String,
    pub message_text: String,
    pub recorded_at: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L0SearchResult {
    pub record_id: String,
    pub session_key: String,
    pub session_id: String,
    pub role: String,
    pub message_text: String,
    pub score: f64,
    pub recorded_at: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L0FtsResult {
    pub record_id: String,
    pub session_key: String,
    pub session_id: String,
    pub role: String,
    pub message_text: String,
    pub score: f64,
    pub recorded_at: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct L0QueryRow {
    pub record_id: String,
    pub session_key: String,
    pub session_id: String,
    pub role: String,
    pub message_text: String,
    pub recorded_at: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug)]
pub struct L0SessionGroup {
    pub session_id: String,
    pub messages: Vec<L0SessionMessage>,
}

#[derive(Clone, Debug)]
pub struct L0SessionMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub recorded_at_ms: i64,
}

// ── Profile types ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProfileRecord {
    pub id: String,
    pub r#type: ProfileType,
    pub filename: String,
    pub content: String,
    pub content_md5: String,
    pub agent_id: Option<String>,
    pub version: i32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ProfileType {
    #[serde(rename = "l2")]
    L2,
    #[serde(rename = "l3")]
    L3,
}

#[derive(Clone, Debug)]
pub struct ProfileSyncRecord {
    pub profile: ProfileRecord,
    pub baseline_version: Option<i32>,
}

// ── Store capabilities ───────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct StoreCapabilities {
    pub vector_search: bool,
    pub fts_search: bool,
    pub native_hybrid_search: bool,
    pub sparse_vectors: bool,
}

// ── Store init result ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct StoreInitResult {
    pub needs_reindex: bool,
    pub reason: Option<String>,
}

/// Layer currently being rebuilt by [`IMemoryStore::reindex_all`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReindexLayer {
    L1,
    L0,
}

/// Number of records processed while rebuilding vector indexes.
///
/// These are processed counts, matching the TypeScript store contract: a
/// record whose embedding or vector write fails is skipped, but still counts
/// towards progress and the returned total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReindexResult {
    pub l1_count: usize,
    pub l0_count: usize,
}

/// Formatable memory for recall injection (port of auto-recall.ts FormatableMemory).
#[derive(Clone, Debug)]
pub struct FormatableMemory {
    pub r#type: String,
    pub content: String,
    pub scene_name: Option<String>,
    pub activity_start_time: Option<String>,
    pub activity_end_time: Option<String>,
    pub timestamp: Option<String>,
}

// ── Embedding provider info ──────────────────────────────────────────────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingProviderInfo {
    pub provider: String,
    pub model: String,
}

/// Object-safe trait for embedding services.
pub trait EmbeddingService: Send + Sync {
    fn embed(&self, text: &str)
    -> std::result::Result<Vec<f32>, crate::error::AeonMemoryCoreError>;
    fn embed_batch(
        &self,
        texts: &[String],
    ) -> std::result::Result<Vec<Vec<f32>>, crate::error::AeonMemoryCoreError>;
    fn dimensions(&self) -> u32;
}

// ═══════════════════════════════════════════════════════════════════════════════
// IMemoryStore trait (port of src/core/store/types.ts IMemoryStore)
// ═══════════════════════════════════════════════════════════════════════════════

/// Unified memory store interface. Implementations:
/// - `SqliteMemoryStore` — local SQLite + sqlite-vec + FTS5
/// - Future: TCVDB backend
///
/// All methods are fault-tolerant: they return empty results or `false` on
/// failure rather than panicking, unless explicitly documented otherwise.
pub trait IMemoryStore: Send + Sync {
    /// Whether this store supports deferred (background) embedding updates.
    fn supports_deferred_embedding(&self) -> bool;

    /// Initialize the store.
    fn init(
        &mut self,
        provider_info: Option<&EmbeddingProviderInfo>,
    ) -> Result<StoreInitResult, crate::error::AeonMemoryCoreError>;

    /// Whether the store is in degraded mode.
    fn is_degraded(&self) -> bool;

    /// Get store capabilities.
    fn capabilities(&self) -> StoreCapabilities;

    /// Close the store and release resources.
    fn close(&mut self);

    // ── L1 operations ──

    fn upsert_l1(
        &mut self,
        record: &crate::types::L1RecordRow,
        embedding: Option<&[f32]>,
    ) -> Result<bool, crate::error::AeonMemoryCoreError>;
    fn delete_l1(&mut self, record_id: &str) -> Result<bool, crate::error::AeonMemoryCoreError>;
    fn count_l1(&self) -> Result<i64, crate::error::AeonMemoryCoreError>;
    fn delete_l1_expired(
        &mut self,
        _cutoff_iso: &str,
    ) -> Result<i64, crate::error::AeonMemoryCoreError> {
        Ok(0)
    }
    fn query_l1_records(
        &self,
        filter: &L1QueryFilter,
    ) -> Result<Vec<L1RecordRow>, crate::error::AeonMemoryCoreError>;
    fn search_l1_fts(
        &self,
        fts_query: &str,
        limit: i64,
    ) -> Result<Vec<L1FtsResult>, crate::error::AeonMemoryCoreError>;
    fn search_l1_vector(
        &self,
        query_embedding: &[f32],
        top_k: i64,
    ) -> Result<Vec<L1SearchResult>, crate::error::AeonMemoryCoreError>;

    // ── L0 operations ──

    fn upsert_l0(
        &mut self,
        record: &L0Record,
        embedding: Option<&[f32]>,
    ) -> Result<bool, crate::error::AeonMemoryCoreError>;
    fn delete_l0(&mut self, record_id: &str) -> Result<bool, crate::error::AeonMemoryCoreError>;
    fn count_l0(&self) -> Result<i64, crate::error::AeonMemoryCoreError>;
    fn delete_l0_expired(
        &mut self,
        _cutoff_iso: &str,
    ) -> Result<i64, crate::error::AeonMemoryCoreError> {
        Ok(0)
    }
    fn query_l0_for_l1(
        &self,
        session_key: &str,
        after_recorded_at_ms: Option<i64>,
        limit: i64,
    ) -> Result<Vec<L0QueryRow>, crate::error::AeonMemoryCoreError>;
    fn search_l0_vector(
        &self,
        query_embedding: &[f32],
        top_k: i64,
    ) -> Result<Vec<L0SearchResult>, crate::error::AeonMemoryCoreError>;

    // ── Re-index ──

    /// Re-embed every L1 record followed by every L0 conversation.
    /// Individual embedding/vector-write failures are non-fatal and skipped.
    fn reindex_all(
        &mut self,
        embed_fn: &mut dyn FnMut(&str) -> Result<Vec<f32>, crate::error::AeonMemoryCoreError>,
        on_progress: Option<&mut dyn FnMut(usize, usize, ReindexLayer)>,
    ) -> Result<ReindexResult, crate::error::AeonMemoryCoreError>;

    // ── FTS availability ──

    fn is_fts_available(&self) -> bool;
}
