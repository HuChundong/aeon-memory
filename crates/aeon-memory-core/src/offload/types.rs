use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OffloadEntry {
    pub timestamp: String,
    pub node_id: Option<String>,
    pub tool_call: String,
    pub summary: String,
    #[serde(default)]
    pub result_ref: String,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Persistent L3 status, matching the TypeScript JSONL contract:
    /// `true` for a confirmed mild replacement and `"deleted"` for an
    /// aggressive/emergency deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offloaded: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolPair {
    pub tool_name: String,
    pub tool_call_id: String,
    pub params: Value,
    pub result: Value,
    pub error: Option<String>,
    pub timestamp: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginState {
    pub active_mmd_file: Option<String>,
    pub active_mmd_id: Option<String>,
    pub mmd_counter: u64,
    pub last_session_key: Option<String>,
    pub last_offloaded_tool_call_id: Option<String>,
    pub last_l2_trigger_time: Option<String>,
    #[serde(default)]
    pub entry_counter: usize,
    #[serde(default)]
    pub l15_boundaries: Vec<L15Boundary>,
    /// Deduplicates repeated before-prompt callbacks for the same user prompt.
    #[serde(default)]
    pub last_l15_prompt_hash: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct L15Boundary {
    pub start_index: usize,
    pub result: BoundaryResult,
    pub target_mmd: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BoundaryResult {
    Long,
    Short,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct OffloadConfig {
    /// Offload is opt-in. This is intentionally false by default.
    pub enabled: bool,
    pub force_trigger_threshold: usize,
    pub default_context_window: usize,
    pub max_pairs_per_batch: usize,
    pub l2_null_threshold: usize,
    pub l2_timeout_seconds: u64,
    pub l2_wait_retry_seconds: u64,
    pub l2_time_trigger_requires_new_offload: bool,
    pub mild_offload_ratio: f64,
    pub mild_offload_scan_ratio: f64,
    pub mild_score_top_ratio: f64,
    pub mild_current_task_ratio: f64,
    pub aggressive_compress_ratio: f64,
    pub aggressive_delete_ratio: f64,
    pub emergency_compress_ratio: f64,
    pub emergency_target_ratio: f64,
    pub mmd_max_token_ratio: f64,
    pub retention_days: u64,
    pub log_max_size_mb: u64,
}

impl Default for OffloadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            force_trigger_threshold: 4,
            default_context_window: 200_000,
            max_pairs_per_batch: 20,
            l2_null_threshold: 4,
            l2_timeout_seconds: 300,
            l2_wait_retry_seconds: 120,
            l2_time_trigger_requires_new_offload: true,
            mild_offload_ratio: 0.5,
            mild_offload_scan_ratio: 0.7,
            mild_score_top_ratio: 0.4,
            mild_current_task_ratio: 0.8,
            aggressive_compress_ratio: 0.85,
            aggressive_delete_ratio: 0.4,
            emergency_compress_ratio: 0.95,
            emergency_target_ratio: 0.6,
            mmd_max_token_ratio: 0.2,
            retention_days: 30,
            log_max_size_mb: 100,
        }
    }
}
