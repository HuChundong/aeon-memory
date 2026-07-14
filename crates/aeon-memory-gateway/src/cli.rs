use std::{path::PathBuf, sync::Arc};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::service::*;

#[derive(Debug, Parser)]
#[command(name = "aeon-memory", version, about = "Lightweight Aeon Memory CLI")]
pub struct Cli {
    /// Gateway YAML/JSON configuration. When omitted, uses TS-compatible discovery.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Seed(SeedArgs),
    Capture(CaptureArgs),
    Recall(RecallArgs),
    Search(SearchArgs),
    Session(SessionArgs),
    Status,
    Show(ShowArgs),
    Offload(OffloadArgs),
}

#[derive(Debug, Args)]
pub struct SeedArgs {
    #[arg(long)]
    pub input: PathBuf,
    #[arg(long)]
    pub session_key: Option<String>,
    #[arg(long)]
    pub strict_round_role: bool,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub auto_fill_timestamps: bool,
}

#[derive(Debug, Args)]
pub struct CaptureArgs {
    #[arg(long)]
    pub user: String,
    #[arg(long)]
    pub assistant: String,
    #[arg(long)]
    pub session_key: String,
    #[arg(long)]
    pub session_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct RecallArgs {
    #[arg(long)]
    pub query: String,
    #[arg(long)]
    pub session_key: String,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[command(subcommand)]
    pub target: SearchTarget,
}

#[derive(Debug, Subcommand)]
pub enum SearchTarget {
    Memories(SearchMemoriesArgs),
    Conversations(SearchConversationsArgs),
}

#[derive(Debug, Args)]
pub struct SearchMemoriesArgs {
    #[arg(long)]
    pub query: String,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long = "type")]
    pub memory_type: Option<String>,
    #[arg(long)]
    pub scene: Option<String>,
}

#[derive(Debug, Args)]
pub struct SearchConversationsArgs {
    #[arg(long)]
    pub query: String,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long)]
    pub session_key: Option<String>,
}

#[derive(Debug, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    End(SessionEndArgs),
}

#[derive(Debug, Args)]
pub struct SessionEndArgs {
    #[arg(long)]
    pub session_key: String,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[command(subcommand)]
    pub target: ShowTarget,
}

#[derive(Debug, Subcommand)]
pub enum ShowTarget {
    Persona,
    Scenes,
}

#[derive(Debug, Args)]
pub struct OffloadArgs {
    #[command(subcommand)]
    pub operation: OffloadOperation,
}

#[derive(Debug, Subcommand)]
pub enum OffloadOperation {
    BeforePrompt(JsonInputArgs),
    AfterTool(JsonInputArgs),
    LlmOutput(JsonInputArgs),
}

#[derive(Debug, Args)]
pub struct JsonInputArgs {
    /// JSON request body matching OFFLOAD_API_CONTRACT.md.
    #[arg(long)]
    pub input: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub async fn execute(service: Arc<dyn AeonMemoryService>, cli: Cli) -> Result<String, CliError> {
    match cli.command {
        Command::Seed(args) => {
            let bytes = std::fs::read(&args.input).map_err(|source| CliError::Read {
                path: args.input.clone(),
                source,
            })?;
            let data = serde_json::from_slice(&bytes).map_err(|source| CliError::Json {
                path: args.input,
                source,
            })?;
            json(
                &service
                    .seed(SeedRequest {
                        data,
                        session_key: args.session_key,
                        strict_round_role: Some(args.strict_round_role),
                        auto_fill_timestamps: Some(args.auto_fill_timestamps),
                        config_override: None,
                    })
                    .await?,
            )
        }
        Command::Capture(args) => json(
            &service
                .capture(CaptureRequest {
                    user_content: args.user,
                    assistant_content: args.assistant,
                    session_key: args.session_key,
                    session_id: args.session_id,
                    user_id: None,
                    messages: None,
                })
                .await?,
        ),
        Command::Recall(args) => json(
            &service
                .recall(RecallRequest {
                    query: args.query,
                    session_key: args.session_key,
                    user_id: None,
                })
                .await?,
        ),
        Command::Search(args) => match args.target {
            SearchTarget::Memories(args) => json(
                &service
                    .search_memories(MemorySearchRequest {
                        query: args.query,
                        limit: args.limit,
                        memory_type: args.memory_type,
                        scene: args.scene,
                    })
                    .await?,
            ),
            SearchTarget::Conversations(args) => json(
                &service
                    .search_conversations(ConversationSearchRequest {
                        query: args.query,
                        limit: args.limit,
                        session_key: args.session_key,
                    })
                    .await?,
            ),
        },
        Command::Session(args) => match args.command {
            SessionCommand::End(args) => json(
                &service
                    .end_session(SessionEndRequest {
                        session_key: args.session_key,
                        user_id: None,
                    })
                    .await?,
            ),
        },
        Command::Status => json(&service.status().await?),
        Command::Show(args) => match args.target {
            ShowTarget::Persona => Ok(service.show_persona().await?),
            ShowTarget::Scenes => json(&service.show_scenes().await?),
        },
        Command::Offload(args) => match args.operation {
            OffloadOperation::BeforePrompt(args) => {
                let request = read_json(&args.input)?;
                json(&service.before_prompt(request).await?)
            }
            OffloadOperation::AfterTool(args) => {
                let request = read_json(&args.input)?;
                json(&service.after_tool(request).await?)
            }
            OffloadOperation::LlmOutput(args) => {
                let request = read_json(&args.input)?;
                json(&service.llm_output(request).await?)
            }
        },
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, CliError> {
    let bytes = std::fs::read(path).map_err(|source| CliError::Read {
        path: path.clone(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| CliError::Json {
        path: path.clone(),
        source,
    })
}

fn json(value: &impl Serialize) -> Result<String, CliError> {
    // All public response DTOs are serializable; this path cannot fail unless
    // a future DTO introduces a custom failing serializer.
    Ok(serde_json::to_string_pretty(value).expect("response DTO must serialize"))
}
