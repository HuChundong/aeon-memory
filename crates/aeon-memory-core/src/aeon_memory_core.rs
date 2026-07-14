//! Host-neutral facade shared by the CLI and HTTP gateway.

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::hooks::{AutoCapture, AutoRecall};
use crate::seed::types::{NormalizedInput, SeedSummary};
use crate::tools::conversation_search::{
    ConversationSearchStore, execute_conversation_search, format_conversation_search_response,
};
use crate::tools::memory_search::{
    MemorySearchStore, execute_memory_search, format_memory_search_response,
};
use crate::types::{
    CompletedTurn, ConversationSearchParams, EmbeddingService, L0FtsResult, L0SearchResult,
    L1FtsResult, L1SearchResult, MemorySearchParams, RecallResult,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait CoreRuntime: Send + Sync {
    async fn initialize(&self) -> AeonMemoryResult<()>;
    async fn destroy(&self) -> AeonMemoryResult<()>;
    async fn flush_session(&self, session_key: &str) -> AeonMemoryResult<()>;
    fn active_sessions(&self) -> AeonMemoryResult<u64>;
}

pub trait CoreSearchStore: Send + Sync {
    fn is_fts_available(&self) -> bool;
    fn search_l1_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>>;
    fn search_l1_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>>;
    fn search_l0_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L0FtsResult>>;
    fn search_l0_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L0SearchResult>>;
    fn count_l0(&self) -> AeonMemoryResult<i64>;
    fn count_l1(&self) -> AeonMemoryResult<i64>;
}

struct SearchView<'a>(&'a dyn CoreSearchStore);
impl MemorySearchStore for SearchView<'_> {
    fn is_fts_available(&self) -> bool {
        self.0.is_fts_available()
    }
    fn search_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        self.0.search_l1_fts(query, limit)
    }
    fn search_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>> {
        self.0.search_l1_vector(embedding, limit)
    }
}
impl ConversationSearchStore for SearchView<'_> {
    fn is_fts_available(&self) -> bool {
        self.0.is_fts_available()
    }
    fn search_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L0FtsResult>> {
        self.0.search_l0_fts(query, limit)
    }
    fn search_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L0SearchResult>> {
        self.0.search_l0_vector(embedding, limit)
    }
}

#[async_trait]
pub trait CoreSeedService: Send + Sync {
    async fn seed(
        &self,
        input: &NormalizedInput,
        config_override: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> AeonMemoryResult<SeedSummary>;
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchResponse {
    pub text: String,
    pub total: usize,
    pub strategy: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AeonMemoryStatus {
    pub initialized: bool,
    pub l0_records: u64,
    pub l1_records: u64,
    pub sessions: u64,
}

pub struct AeonMemoryCoreOptions {
    pub data_dir: PathBuf,
    pub capture: Arc<AutoCapture>,
    pub recall: Arc<AutoRecall>,
    pub runtime: Arc<dyn CoreRuntime>,
    pub search_store: Option<Arc<dyn CoreSearchStore>>,
    pub embedding: Option<Arc<dyn EmbeddingService>>,
    pub seed_service: Option<Arc<dyn CoreSeedService>>,
}

pub struct AeonMemoryCore {
    data_dir: PathBuf,
    capture: Arc<AutoCapture>,
    recall: Arc<AutoRecall>,
    runtime: Arc<dyn CoreRuntime>,
    search_store: Option<Arc<dyn CoreSearchStore>>,
    embedding: Option<Arc<dyn EmbeddingService>>,
    seed_service: Option<Arc<dyn CoreSeedService>>,
    initialized: Mutex<bool>,
}

impl AeonMemoryCore {
    pub fn new(options: AeonMemoryCoreOptions) -> Self {
        Self {
            data_dir: options.data_dir,
            capture: options.capture,
            recall: options.recall,
            runtime: options.runtime,
            search_store: options.search_store,
            embedding: options.embedding,
            seed_service: options.seed_service,
            initialized: Mutex::new(false),
        }
    }

    pub async fn initialize(&self) -> AeonMemoryResult<()> {
        if self.is_initialized()? {
            return Ok(());
        }
        for directory in ["conversations", "records", "scene_blocks", ".metadata"] {
            std::fs::create_dir_all(self.data_dir.join(directory))?;
        }
        self.runtime.initialize().await?;
        *self.state()? = true;
        Ok(())
    }

    pub async fn destroy(&self) -> AeonMemoryResult<()> {
        if !self.is_initialized()? {
            return Ok(());
        }
        self.capture.drain().await?;
        self.runtime.destroy().await?;
        *self.state()? = false;
        Ok(())
    }

    pub async fn handle_before_recall(
        &self,
        user_text: &str,
        _session_key: &str,
    ) -> AeonMemoryResult<RecallResult> {
        self.require_initialized()?;
        Ok(self.recall.perform(user_text).await?.unwrap_or_default())
    }

    pub async fn handle_turn_committed(
        &self,
        turn: &CompletedTurn,
    ) -> AeonMemoryResult<crate::types::CaptureResult> {
        self.require_initialized()?;
        self.capture
            .perform(
                &turn.messages,
                &turn.session_key,
                turn.session_id.as_deref(),
                Some(&turn.user_text),
                turn.original_user_message_count,
                turn.started_at,
                turn.skip_cursor,
            )
            .await
    }

    pub fn search_memories(&self, params: &MemorySearchParams) -> AeonMemoryResult<SearchResponse> {
        self.require_initialized()?;
        let view = self.search_store.as_deref().map(SearchView);
        let result = execute_memory_search(
            &params.query,
            params.limit.unwrap_or(5) as usize,
            params.r#type.as_deref(),
            params.scene.as_deref(),
            view.as_ref().map(|value| value as &dyn MemorySearchStore),
            self.embedding.as_deref(),
        );
        Ok(SearchResponse {
            text: format_memory_search_response(&result),
            total: result.total,
            strategy: result.strategy,
        })
    }

    pub fn search_conversations(
        &self,
        params: &ConversationSearchParams,
    ) -> AeonMemoryResult<SearchResponse> {
        self.require_initialized()?;
        let view = self.search_store.as_deref().map(SearchView);
        let result = execute_conversation_search(
            &params.query,
            params.limit.unwrap_or(5) as usize,
            params.session_key.as_deref(),
            view.as_ref()
                .map(|value| value as &dyn ConversationSearchStore),
            self.embedding.as_deref(),
        );
        Ok(SearchResponse {
            text: format_conversation_search_response(&result),
            total: result.total,
            strategy: result.strategy,
        })
    }

    pub async fn seed(
        &self,
        input: &NormalizedInput,
        config_override: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> AeonMemoryResult<SeedSummary> {
        self.require_initialized()?;
        self.seed_service
            .as_ref()
            .ok_or_else(|| {
                AeonMemoryCoreError::InvalidInput("seed service is not configured".into())
            })?
            .seed(input, config_override)
            .await
    }

    pub async fn handle_session_end(&self, session_key: &str) -> AeonMemoryResult<()> {
        self.require_initialized()?;
        if !session_key.is_empty() {
            self.runtime.flush_session(session_key).await?;
        }
        Ok(())
    }

    pub fn status(&self) -> AeonMemoryResult<AeonMemoryStatus> {
        let initialized = self.is_initialized()?;
        let (l0, l1) = match &self.search_store {
            Some(store) if initialized => (store.count_l0()? as u64, store.count_l1()? as u64),
            _ => (0, 0),
        };
        Ok(AeonMemoryStatus {
            initialized,
            l0_records: l0,
            l1_records: l1,
            sessions: if initialized {
                self.runtime.active_sessions()?
            } else {
                0
            },
        })
    }

    pub fn persona(&self) -> AeonMemoryResult<Option<String>> {
        match std::fs::read_to_string(self.data_dir.join("persona.md")) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn scenes(&self) -> AeonMemoryResult<Vec<String>> {
        let directory = self.data_dir.join("scene_blocks");
        let mut scenes = match std::fs::read_dir(directory) {
            Ok(items) => items
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter_map(|entry| {
                    entry
                        .file_type()
                        .ok()
                        .filter(|kind| kind.is_file())
                        .and_then(|_| entry.file_name().to_str().map(str::to_owned))
                        .filter(|name| name.ends_with(".md"))
                })
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        scenes.sort();
        Ok(scenes)
    }

    fn state(&self) -> AeonMemoryResult<std::sync::MutexGuard<'_, bool>> {
        self.initialized
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("core lifecycle lock poisoned".into()))
    }
    fn is_initialized(&self) -> AeonMemoryResult<bool> {
        Ok(*self.state()?)
    }
    fn require_initialized(&self) -> AeonMemoryResult<()> {
        if self.is_initialized()? {
            Ok(())
        } else {
            Err(AeonMemoryCoreError::InvalidInput(
                "AeonMemoryCore must be initialized before use".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AeonMemoryConfig;
    use crate::hooks::{CaptureRecorder, CaptureScheduler};
    use crate::record::l0_recorder::ConversationMessage;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Recorder;
    impl CaptureRecorder for Recorder {
        fn capture(
            &self,
            _: &[serde_json::Value],
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<i64>,
            _: bool,
        ) -> AeonMemoryResult<Vec<ConversationMessage>> {
            Ok(vec![ConversationMessage {
                id: "one".into(),
                role: "user".into(),
                content: "remember this".into(),
                timestamp: 1,
            }])
        }
    }
    #[derive(Default)]
    struct Scheduler(AtomicUsize);
    impl CaptureScheduler for Scheduler {
        fn notify_conversation(
            &self,
            _: &str,
            _: &[crate::record::l0_recorder::ConversationMessage],
        ) -> AeonMemoryResult<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    #[derive(Default)]
    struct Runtime {
        initialized: AtomicBool,
        destroyed: AtomicBool,
        flushed: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl CoreRuntime for Runtime {
        async fn initialize(&self) -> AeonMemoryResult<()> {
            self.initialized.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn destroy(&self) -> AeonMemoryResult<()> {
            self.destroyed.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn flush_session(&self, session_key: &str) -> AeonMemoryResult<()> {
            self.flushed.lock().unwrap().push(session_key.into());
            Ok(())
        }
        fn active_sessions(&self) -> AeonMemoryResult<u64> {
            Ok(2)
        }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aeon-memory-facade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn facade_lifecycle_capture_recall_session_and_profiles_are_composed() {
        let dir = temp_dir();
        let runtime = Arc::new(Runtime::default());
        let scheduler = Arc::new(Scheduler::default());
        let capture = Arc::new(AutoCapture::new(
            Arc::new(Recorder),
            None,
            None,
            Some(scheduler.clone()),
        ));
        let recall = Arc::new(AutoRecall {
            config: AeonMemoryConfig::default(),
            data_dir: dir.clone(),
            store: None,
            embedding: None,
        });
        let core = AeonMemoryCore::new(AeonMemoryCoreOptions {
            data_dir: dir.clone(),
            capture,
            recall,
            runtime: runtime.clone(),
            search_store: None,
            embedding: None,
            seed_service: None,
        });
        assert!(!core.status().unwrap().initialized);
        core.initialize().await.unwrap();
        std::fs::write(dir.join("persona.md"), "persona").unwrap();
        std::fs::write(dir.join("scene_blocks/z.md"), "z").unwrap();
        std::fs::write(dir.join("scene_blocks/a.md"), "a").unwrap();
        let turn = CompletedTurn {
            user_text: "remember this".into(),
            assistant_text: "ok".into(),
            messages: vec![],
            session_key: "s1".into(),
            session_id: None,
            started_at: None,
            original_user_message_count: None,
            skip_cursor: false,
        };
        assert_eq!(
            core.handle_turn_committed(&turn)
                .await
                .unwrap()
                .l0_recorded_count,
            1
        );
        assert_eq!(scheduler.0.load(Ordering::SeqCst), 1);
        core.handle_session_end("s1").await.unwrap();
        assert_eq!(runtime.flushed.lock().unwrap().as_slice(), ["s1"]);
        assert_eq!(core.status().unwrap().sessions, 2);
        assert_eq!(core.persona().unwrap().as_deref(), Some("persona"));
        assert_eq!(core.scenes().unwrap(), ["a.md", "z.md"]);
        core.destroy().await.unwrap();
        assert!(runtime.destroyed.load(Ordering::SeqCst));
        assert!(!core.status().unwrap().initialized);
    }
}
