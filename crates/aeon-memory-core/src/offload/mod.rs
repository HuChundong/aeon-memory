//! Host-neutral context offload core. Disabled unless explicitly enabled.

pub mod engine;
pub mod inject;
pub mod l3;
pub mod mermaid;
pub mod parser;
pub mod prompt;
pub mod reclaim;
pub mod storage;
pub mod token;
pub mod types;

pub use engine::OffloadEngine;
pub use types::{OffloadConfig, OffloadEntry, PluginState, ToolPair};
