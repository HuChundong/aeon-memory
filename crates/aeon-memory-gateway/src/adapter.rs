//! Composition boundary between transports and the host-neutral core facade.

use std::{sync::Arc, time::Instant};

use aeon_memory_core::{
    AeonMemoryCore, AeonMemoryCoreError,
    seed::input::validate_and_normalize_raw,
    types::{CompletedTurn, ConversationSearchParams, MemorySearchParams},
};
use async_trait::async_trait;

use crate::service::*;

#[async_trait]
pub trait OffloadOperations: Send + Sync + 'static {
    async fn before_prompt(
        &self,
        request: BeforePromptRequest,
    ) -> ServiceResult<BeforePromptResponse>;
    async fn after_tool(&self, request: AfterToolRequest) -> ServiceResult<AfterToolResponse>;
    async fn llm_output(&self, request: LlmOutputRequest) -> ServiceResult<LlmOutputResponse>;
    async fn shutdown(&self) -> ServiceResult<()> {
        Ok(())
    }
}

#[async_trait]
pub trait CleanerOperations: Send + Sync + 'static {
    async fn shutdown(&self) -> ServiceResult<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ComponentHealth {
    pub vector_store: bool,
    pub embedding_service: bool,
}

pub struct CoreService {
    core: Arc<AeonMemoryCore>,
    offload: Option<Arc<dyn OffloadOperations>>,
    cleaner: Option<Arc<dyn CleanerOperations>>,
    health: ComponentHealth,
    started_at: Instant,
}

impl CoreService {
    pub fn new(
        core: Arc<AeonMemoryCore>,
        offload: Option<Arc<dyn OffloadOperations>>,
        cleaner: Option<Arc<dyn CleanerOperations>>,
        health: ComponentHealth,
    ) -> Self {
        Self {
            core,
            offload,
            cleaner,
            health,
            started_at: Instant::now(),
        }
    }

    pub async fn initialize(&self) -> ServiceResult<()> {
        self.core.initialize().await.map_err(map_core_error)
    }

    /// Drain capture work and flush the L0 -> L1 -> L2 pipeline before the
    /// transport releases the process. This is intentionally kept off the
    /// HTTP-facing trait: lifecycle ownership belongs to the host process.
    pub async fn shutdown(&self) -> ServiceResult<()> {
        if let Some(cleaner) = self.cleaner.as_deref() {
            cleaner.shutdown().await?;
        }
        if let Some(offload) = self.offload.as_deref() {
            offload.shutdown().await?;
        }
        self.core.destroy().await.map_err(map_core_error)
    }

    fn offload(&self) -> ServiceResult<&dyn OffloadOperations> {
        self.offload
            .as_deref()
            .ok_or_else(|| ServiceError::InvalidInput("offload service is not configured".into()))
    }
}

fn map_core_error(error: AeonMemoryCoreError) -> ServiceError {
    match error {
        AeonMemoryCoreError::InvalidInput(message) => ServiceError::InvalidInput(message),
        other => ServiceError::Internal(other.to_string()),
    }
}

#[async_trait]
impl AeonMemoryService for CoreService {
    async fn health(&self) -> ServiceResult<HealthResponse> {
        let status = self.core.status().map_err(map_core_error)?;
        Ok(HealthResponse {
            status: if status.initialized { "ok" } else { "starting" }.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            uptime: self.started_at.elapsed().as_secs(),
            stores: StoreHealth {
                vector_store: status.initialized && self.health.vector_store,
                embedding_service: status.initialized && self.health.embedding_service,
            },
        })
    }

    async fn recall(&self, request: RecallRequest) -> ServiceResult<RecallResponse> {
        let result = self
            .core
            .handle_before_recall(&request.query, &request.session_key)
            .await
            .map_err(map_core_error)?;
        Ok(RecallResponse {
            context: result.append_system_context.unwrap_or_default(),
            // Keep the original stable `context` contract while exposing the
            // strategy-selected and budgeted L1 payload to HTTP host adapters.
            // This avoids adapters re-running a semantically different search.
            prepend_context: result.prepend_context,
            strategy: (!result.recall_strategy.is_empty()).then_some(result.recall_strategy),
            memory_count: result.recalled_l1_memories.len(),
        })
    }

    async fn capture(&self, request: CaptureRequest) -> ServiceResult<CaptureResponse> {
        let has_explicit_messages = request.messages.is_some();
        let messages = request.messages.unwrap_or_else(|| {
            vec![
                serde_json::json!({"role": "user", "content": request.user_content}),
                serde_json::json!({"role": "assistant", "content": request.assistant_content}),
            ]
        });
        let result = self
            .core
            .handle_turn_committed(&CompletedTurn {
                user_text: request.user_content,
                assistant_text: request.assistant_content,
                messages,
                session_key: request.session_key,
                session_id: request.session_id,
                started_at: None,
                original_user_message_count: None,
                skip_cursor: !has_explicit_messages,
            })
            .await
            .map_err(map_core_error)?;
        Ok(CaptureResponse {
            l0_recorded: result.l0_recorded_count as usize,
            scheduler_notified: result.scheduler_notified,
        })
    }

    async fn search_memories(
        &self,
        request: MemorySearchRequest,
    ) -> ServiceResult<MemorySearchResponse> {
        let result = self
            .core
            .search_memories(&MemorySearchParams {
                query: request.query,
                limit: request.limit.map(|value| value as u32),
                r#type: request.memory_type,
                scene: request.scene,
            })
            .map_err(map_core_error)?;
        Ok(MemorySearchResponse {
            results: result.text,
            total: result.total,
            strategy: result.strategy,
        })
    }

    async fn search_conversations(
        &self,
        request: ConversationSearchRequest,
    ) -> ServiceResult<ConversationSearchResponse> {
        let result = self
            .core
            .search_conversations(&ConversationSearchParams {
                query: request.query,
                limit: request.limit.map(|value| value as u32),
                session_key: request.session_key,
            })
            .map_err(map_core_error)?;
        Ok(ConversationSearchResponse {
            results: result.text,
            total: result.total,
        })
    }

    async fn end_session(&self, request: SessionEndRequest) -> ServiceResult<SessionEndResponse> {
        self.core
            .handle_session_end(&request.session_key)
            .await
            .map_err(map_core_error)?;
        Ok(SessionEndResponse { flushed: true })
    }

    async fn seed(&self, request: SeedRequest) -> ServiceResult<SeedResponse> {
        let normalized = validate_and_normalize_raw(
            &request.data,
            request.session_key.as_deref(),
            request.strict_round_role.unwrap_or(false),
            request.auto_fill_timestamps.unwrap_or(true),
        )
        .map_err(|error| ServiceError::InvalidInput(error.to_string()))?;
        let result = self
            .core
            .seed(&normalized, request.config_override.as_ref())
            .await
            .map_err(map_core_error)?;
        Ok(SeedResponse {
            sessions_processed: result.sessions_processed,
            rounds_processed: result.rounds_processed,
            messages_processed: result.messages_processed,
            l0_recorded: result.l0_recorded_count,
            duration_ms: result.duration_ms,
            output_dir: result.output_dir,
        })
    }

    async fn before_prompt(
        &self,
        request: BeforePromptRequest,
    ) -> ServiceResult<BeforePromptResponse> {
        self.offload()?.before_prompt(request).await
    }

    async fn after_tool(&self, request: AfterToolRequest) -> ServiceResult<AfterToolResponse> {
        self.offload()?.after_tool(request).await
    }

    async fn llm_output(&self, request: LlmOutputRequest) -> ServiceResult<LlmOutputResponse> {
        self.offload()?.llm_output(request).await
    }

    async fn status(&self) -> ServiceResult<StatusResponse> {
        let status = self.core.status().map_err(map_core_error)?;
        Ok(StatusResponse {
            l0_records: status.l0_records,
            l1_records: status.l1_records,
            sessions: status.sessions,
        })
    }

    async fn show_persona(&self) -> ServiceResult<String> {
        self.core
            .persona()
            .map_err(map_core_error)?
            .ok_or_else(|| ServiceError::NotFound("persona is not available".into()))
    }

    async fn show_scenes(&self) -> ServiceResult<Vec<String>> {
        self.core.scenes().map_err(map_core_error)
    }
}
