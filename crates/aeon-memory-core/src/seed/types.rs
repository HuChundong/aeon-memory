#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedMessage {
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedRound {
    pub messages: Vec<NormalizedMessage>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedSession {
    pub session_key: String,
    pub session_id: String,
    pub rounds: Vec<NormalizedRound>,
    pub source_index: usize,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedInput {
    pub sessions: Vec<NormalizedSession>,
    pub total_rounds: usize,
    pub total_messages: usize,
    pub has_timestamps: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    File,
    TopLevel,
    Session,
    Round,
    Message,
    TimestampConsistency,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    pub stage: ValidationStage,
    pub source_index: Option<usize>,
    pub session_key: Option<String>,
    pub round_index: Option<usize>,
    pub message_index: Option<usize>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedProgress {
    pub current_round: usize,
    pub total_rounds: usize,
    pub session_key: String,
    pub stage: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SeedSummary {
    pub sessions_processed: usize,
    pub rounds_processed: usize,
    pub messages_processed: usize,
    pub l0_recorded_count: usize,
    pub idempotent_skips: usize,
    pub failed_rounds: usize,
    pub duration_ms: u64,
    pub output_dir: String,
}
