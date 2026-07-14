// Canonical error type for aeon-memory-core. Does NOT depend on any storage or HTTP crate.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AeonMemoryCoreError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Store error: {0}")]
    Store(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type AeonMemoryResult<T> = Result<T, AeonMemoryCoreError>;
