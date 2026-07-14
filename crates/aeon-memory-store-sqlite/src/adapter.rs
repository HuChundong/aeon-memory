//! Concrete host-neutral adapters used to assemble [`aeon_memory_core::AeonMemoryCore`].

use aeon_memory_core::aeon_memory_core::{CoreRuntime, CoreSearchStore, CoreSeedService};
use aeon_memory_core::hooks::AutoCapture;
#[cfg(test)]
use aeon_memory_core::hooks::auto_capture::LocalCaptureRecorder;
use aeon_memory_core::hooks::auto_capture::{CaptureScheduler, CaptureStore};
use aeon_memory_core::hooks::auto_recall::RecallStore;
use aeon_memory_core::persona::{PersonaGenerator, PersonaTrigger};
use aeon_memory_core::pipeline::checkpoint::{
    mark_l1_extraction_complete, merge_pipeline_states, mutate_checkpoint, read_checkpoint,
};
use aeon_memory_core::pipeline::manager::CapturedMessage;
use aeon_memory_core::pipeline::manager::PipelineManager;
use aeon_memory_core::pipeline::manager::{L2Result, PipelineRunner, StatePersister};
use aeon_memory_core::record::l0_recorder::ConversationMessage;
use aeon_memory_core::record::l1_extractor::{
    L1ExtractionOptions, L1Services, extract_l1_memories,
};
use aeon_memory_core::scene::{SceneExtractor, SceneMemory};
use aeon_memory_core::seed::runtime::{CaptureOutcome, SeedRound};
use aeon_memory_core::seed::runtime::{SeedRuntime, execute_seed};
use aeon_memory_core::seed::types::{NormalizedInput, SeedSummary};
use aeon_memory_core::types::*;
use aeon_memory_core::{AeonMemoryCoreError, AeonMemoryResult};
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::VectorStore;

/// Cloneable synchronization boundary around the SQLite store. A single value
/// implements every store view required by capture, recall and the facade.
#[derive(Clone)]
pub struct SqliteCoreStore {
    inner: Arc<Mutex<VectorStore>>,
}

impl SqliteCoreStore {
    pub fn new(store: VectorStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
        }
    }

    pub fn initialize(
        &self,
        provider: Option<&EmbeddingProviderInfo>,
    ) -> AeonMemoryResult<StoreInitResult> {
        self.lock()?.init(provider)
    }

    pub fn close(&self) -> AeonMemoryResult<()> {
        self.lock()?.close();
        Ok(())
    }

    fn lock(&self) -> AeonMemoryResult<MutexGuard<'_, VectorStore>> {
        self.inner
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("SQLite store lock poisoned".into()))
    }

    pub fn with_store<T>(
        &self,
        f: impl FnOnce(&mut VectorStore) -> AeonMemoryResult<T>,
    ) -> AeonMemoryResult<T> {
        let mut store = self.lock()?;
        f(&mut store)
    }
    pub fn claim_seed_key(&self, key: &str) -> AeonMemoryResult<bool> {
        self.lock()?.claim_seed_key(key)
    }
    pub fn release_seed_key(&self, key: &str) -> AeonMemoryResult<()> {
        self.lock()?.release_seed_key(key)
    }
}

/// Production implementation of the scheduler side effects.  The manager is
/// synchronous by design, so each async LLM stage is executed on a dedicated
/// current-thread runtime; this also keeps it safe when notification originates
/// from an Axum Tokio worker.
pub struct ProductionPipelineRunner {
    /// Optional secondary index. JSONL remains the source of truth and the
    /// pipeline must keep running when SQLite/sqlite-vec is unavailable.
    pub store: Option<SqliteCoreStore>,
    pub data_dir: PathBuf,
    pub l1_llm: Arc<dyn LlmRunner>,
    pub scene_llm: Arc<dyn LlmRunner>,
    pub persona_llm: Arc<dyn LlmRunner>,
    pub embedding: Arc<dyn EmbeddingService>,
    pub embedding_provider: Option<EmbeddingProviderInfo>,
    pub l1_running: Arc<Mutex<HashSet<String>>>,
    pub l1_options: L1ExtractionOptions,
    pub max_scenes: usize,
    pub persona_backup_count: usize,
    pub scene_backup_count: usize,
    pub persona_trigger_every_n: u64,
}

pub struct CheckpointStatePersister {
    pub data_dir: PathBuf,
}

impl StatePersister for CheckpointStatePersister {
    fn persist(
        &mut self,
        states: &std::collections::HashMap<
            String,
            aeon_memory_core::pipeline::checkpoint::PipelineSessionState,
        >,
    ) -> Result<(), String> {
        merge_pipeline_states(&self.data_dir.to_string_lossy(), states)
            .map_err(|error| error.to_string())
    }
}

impl ProductionPipelineRunner {
    fn block_on<T: Send + 'static>(
        f: impl FnOnce() -> AeonMemoryResult<T> + Send + 'static,
    ) -> Result<T, String> {
        std::thread::spawn(f)
            .join()
            .map_err(|_| "pipeline worker panicked".to_owned())?
            .map_err(|error| error.to_string())
    }
}

/// JSONL-only store view used by L1 extraction when SQLite initialization
/// fails. Writers still append the canonical JSONL record before these
/// intentionally empty secondary-index operations are invoked.
#[derive(Default)]
struct DegradedPipelineStore;

impl IMemoryStore for DegradedPipelineStore {
    fn supports_deferred_embedding(&self) -> bool {
        false
    }
    fn init(&mut self, _: Option<&EmbeddingProviderInfo>) -> AeonMemoryResult<StoreInitResult> {
        Ok(StoreInitResult {
            needs_reindex: false,
            reason: None,
        })
    }
    fn is_degraded(&self) -> bool {
        true
    }
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities::default()
    }
    fn close(&mut self) {}
    fn upsert_l1(&mut self, _: &L1RecordRow, _: Option<&[f32]>) -> AeonMemoryResult<bool> {
        Ok(false)
    }
    fn delete_l1(&mut self, _: &str) -> AeonMemoryResult<bool> {
        Ok(false)
    }
    fn count_l1(&self) -> AeonMemoryResult<i64> {
        Ok(0)
    }
    fn query_l1_records(&self, _: &L1QueryFilter) -> AeonMemoryResult<Vec<L1RecordRow>> {
        Ok(Vec::new())
    }
    fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        Ok(Vec::new())
    }
    fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
        Ok(Vec::new())
    }
    fn upsert_l0(&mut self, _: &L0Record, _: Option<&[f32]>) -> AeonMemoryResult<bool> {
        Ok(false)
    }
    fn delete_l0(&mut self, _: &str) -> AeonMemoryResult<bool> {
        Ok(false)
    }
    fn count_l0(&self) -> AeonMemoryResult<i64> {
        Ok(0)
    }
    fn query_l0_for_l1(
        &self,
        _: &str,
        _: Option<i64>,
        _: i64,
    ) -> AeonMemoryResult<Vec<L0QueryRow>> {
        Ok(Vec::new())
    }
    fn search_l0_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L0SearchResult>> {
        Ok(Vec::new())
    }
    fn reindex_all(
        &mut self,
        _: &mut dyn FnMut(&str) -> AeonMemoryResult<Vec<f32>>,
        _: Option<&mut dyn FnMut(usize, usize, ReindexLayer)>,
    ) -> AeonMemoryResult<ReindexResult> {
        Ok(ReindexResult::default())
    }
    fn is_fts_available(&self) -> bool {
        false
    }
}

impl PipelineRunner for ProductionPipelineRunner {
    fn run_l1(&mut self, session: &str, _messages: &[CapturedMessage]) -> Result<(), String> {
        let data_dir = self.data_dir.clone();
        let llm = Arc::clone(&self.l1_llm);
        let embedding = Arc::clone(&self.embedding);
        let embedding_provider = self.embedding_provider.clone();
        let sqlite_available = self.store.is_some();
        let l1_options = self.l1_options.clone();
        let session = session.to_owned();
        let running_session = session.clone();
        self.l1_running
            .lock()
            .map_err(|_| "L1 activity registry poisoned".to_owned())?
            .insert(session.clone());
        let result = Self::block_on(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| AeonMemoryCoreError::Store(error.to_string()))?;
            let checkpoint = read_checkpoint(&data_dir.to_string_lossy())?;
            let runner_state = checkpoint
                .runner_states
                .get(&session)
                .cloned()
                .unwrap_or_default();
            let cursor = (runner_state.last_l1_cursor > 0).then_some(runner_state.last_l1_cursor);
            // L1 owns a separate SQLite connection when available. Otherwise
            // it reads the canonical L0 JSONL files, matching TS degraded mode.
            let mut sqlite_store = sqlite_available.then(|| {
                VectorStore::new(
                    &data_dir.join("vectors.db").to_string_lossy(),
                    embedding.dimensions(),
                )
            });
            if let Some(vector_store) = sqlite_store.as_mut() {
                vector_store.init(embedding_provider.as_ref())?;
                if vector_store.is_degraded() {
                    sqlite_store = None;
                }
            }
            let rows = if let Some(vector_store) = sqlite_store.as_ref() {
                let mut rows = vector_store.query_l0_for_l1(&session, cursor, 50)?;
                // SQLite returns newest first so that LIMIT keeps the newest
                // fifty. The extractor contract is chronological.
                rows.reverse();
                rows
            } else {
                let mut rows = aeon_memory_core::record::l0_recorder::read_conversation_records(
                    &session,
                    &data_dir.to_string_lossy(),
                )?
                .into_iter()
                .filter(|row| {
                    let recorded = chrono::DateTime::parse_from_rfc3339(&row.recorded_at)
                        .map(|value| value.timestamp_millis())
                        .unwrap_or_default();
                    cursor.is_none_or(|cursor| recorded > cursor)
                })
                .map(|row| L0QueryRow {
                    record_id: row.id,
                    session_key: row.session_key,
                    session_id: row.session_id,
                    role: row.role,
                    message_text: row.content,
                    recorded_at: row.recorded_at,
                    timestamp: row.timestamp,
                })
                .collect::<Vec<_>>();
                // The JSONL reader is already chronological. Match
                // readConversationMessagesGroupedBySessionId(..., 50) by
                // retaining its newest tail without reversing it.
                if rows.len() > 50 {
                    rows.drain(..rows.len() - 50);
                }
                rows
            };
            if rows.is_empty() {
                return Ok(());
            }

            let max_recorded_at = rows
                .iter()
                .filter_map(|row| chrono::DateTime::parse_from_rfc3339(&row.recorded_at).ok())
                .map(|value| value.timestamp_millis())
                .max();
            let mut groups: Vec<(String, Vec<ConversationMessage>)> = Vec::new();
            for row in rows {
                let message = ConversationMessage {
                    id: row.record_id,
                    role: row.role,
                    content: row.message_text,
                    timestamp: row.timestamp,
                };
                if let Some((_, messages)) = groups.iter_mut().find(|(id, _)| *id == row.session_id)
                {
                    messages.push(message);
                } else {
                    groups.push((row.session_id, vec![message]));
                }
            }

            let mut stored = 0_u64;
            let mut degraded_store = DegradedPipelineStore;
            let mut previous_scene =
                (!runner_state.last_scene_name.is_empty()).then_some(runner_state.last_scene_name);
            for (session_id, messages) in groups {
                let vector_store: &mut dyn IMemoryStore = match sqlite_store.as_mut() {
                    Some(store) => store,
                    None => &mut degraded_store,
                };
                let result = runtime.block_on(extract_l1_memories(
                    &messages,
                    &session,
                    &session_id,
                    &data_dir.to_string_lossy(),
                    previous_scene.as_deref(),
                    &l1_options,
                    L1Services {
                        vector_store,
                        embedding_service: embedding.as_ref(),
                        llm_runner: llm.as_ref(),
                    },
                ))?;
                stored += u64::from(result.stored_count);
                if result.last_scene_name.is_some() {
                    previous_scene = result.last_scene_name;
                }
            }
            mark_l1_extraction_complete(
                &data_dir.to_string_lossy(),
                &session,
                stored,
                max_recorded_at,
                previous_scene.as_deref(),
            )?;
            Ok(())
        });
        self.l1_running
            .lock()
            .map_err(|_| "L1 activity registry poisoned".to_owned())?
            .remove(&running_session);
        result
    }

    fn run_l2(&mut self, session: &str, cursor: Option<&str>) -> Result<L2Result, String> {
        let rows = if let Some(store) = self.store.as_ref() {
            store
                .with_store(|store| {
                    store.query_l1_records(&L1QueryFilter {
                        session_key: Some(session.to_owned()),
                        updated_after: cursor.map(str::to_owned),
                        ..Default::default()
                    })
                })
                .map_err(|error| error.to_string())?
        } else {
            aeon_memory_core::record::l1_writer::read_memory_records(
                session,
                &self.data_dir.to_string_lossy(),
            )
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|record| cursor.is_none_or(|cursor| record.updated_at.as_str() > cursor))
            .map(|record| L1RecordRow {
                record_id: record.id,
                content: record.content,
                r#type: record.r#type,
                priority: record.priority,
                scene_name: record.scene_name,
                session_key: record.session_key,
                session_id: record.session_id,
                timestamp_str: record.timestamps.first().cloned().unwrap_or_default(),
                timestamp_start: record.timestamps.iter().min().cloned().unwrap_or_default(),
                timestamp_end: record.timestamps.iter().max().cloned().unwrap_or_default(),
                created_time: record.created_at,
                updated_time: record.updated_at,
                metadata_json: record.metadata.to_string(),
            })
            .collect()
        };
        if rows.is_empty() {
            return Ok(L2Result {
                latest_cursor: cursor.map(str::to_owned),
                skipped: true,
            });
        }
        let latest = rows.iter().map(|row| row.updated_time.clone()).max();
        let memories = rows
            .into_iter()
            .map(|row| SceneMemory {
                content: row.content,
                created_at: row.created_time,
                id: Some(row.record_id),
            })
            .collect::<Vec<_>>();
        let data_dir = self.data_dir.clone();
        let llm = Arc::clone(&self.scene_llm);
        let max_scenes = self.max_scenes;
        let scene_backup_count = self.scene_backup_count;
        Self::block_on(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| AeonMemoryCoreError::Store(error.to_string()))?;
            let result = runtime.block_on(
                SceneExtractor {
                    data_dir: data_dir.clone(),
                    runner: llm.as_ref(),
                    max_scenes,
                    backup_count: scene_backup_count,
                    timeout_ms: 300_000,
                }
                .extract(&memories),
            );
            if result.success {
                if result.memories_processed > 0 {
                    mutate_checkpoint(&data_dir.to_string_lossy(), |checkpoint| {
                        checkpoint.scenes_processed += 1;
                    })?;
                }
                Ok(L2Result {
                    latest_cursor: latest,
                    skipped: false,
                })
            } else {
                Err(AeonMemoryCoreError::Llm(
                    result
                        .error
                        .unwrap_or_else(|| "scene extraction failed".into()),
                ))
            }
        })
    }

    fn run_l3(&mut self) -> Result<(), String> {
        let data_dir = self.data_dir.clone();
        let llm = Arc::clone(&self.persona_llm);
        let backup_count = self.persona_backup_count;
        let trigger_every_n = self.persona_trigger_every_n;
        Self::block_on(move || {
            let trigger = PersonaTrigger {
                data_dir: data_dir.clone(),
                interval: trigger_every_n,
            }
            .should_generate()?;
            if !trigger.should {
                return Ok(());
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| AeonMemoryCoreError::Store(error.to_string()))?;
            runtime
                .block_on(
                    PersonaGenerator {
                        data_dir,
                        runner: llm.as_ref(),
                        backup_count,
                    }
                    .generate(Some(&trigger.reason)),
                )
                .map(|_| ())
        })
    }
}

impl CaptureStore for SqliteCoreStore {
    fn supports_deferred_embedding(&self) -> bool {
        true
    }
    fn upsert_l0(&self, record: &L0Record, embedding: Option<&[f32]>) -> AeonMemoryResult<bool> {
        self.lock()?.upsert_l0(record, embedding)
    }
    fn update_l0_embedding(&self, record_id: &str, embedding: &[f32]) -> AeonMemoryResult<bool> {
        let store = self.lock()?;
        if !store.capabilities().vector_search {
            return Err(AeonMemoryCoreError::Store(
                "deferred embedding requires sqlite-vec".into(),
            ));
        }
        let conn = store.lock()?;
        crate::l0::update_l0_embedding(&conn, record_id, embedding).map_err(super::map_err)
    }
}

impl RecallStore for SqliteCoreStore {
    fn is_fts_available(&self) -> bool {
        self.lock().is_ok_and(|s| s.is_fts_available())
    }
    fn search_l1_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        self.lock()?.search_l1_fts(query, limit)
    }
    fn search_l1_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>> {
        self.lock()?.search_l1_vector(embedding, limit)
    }
}

impl CoreSearchStore for SqliteCoreStore {
    fn is_fts_available(&self) -> bool {
        RecallStore::is_fts_available(self)
    }
    fn search_l1_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        RecallStore::search_l1_fts(self, query, limit)
    }
    fn search_l1_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>> {
        RecallStore::search_l1_vector(self, embedding, limit)
    }
    fn search_l0_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L0FtsResult>> {
        let store = self.lock()?;
        if !store.is_fts_available() {
            return Ok(Vec::new());
        }
        let conn = store.lock()?;
        crate::l0::search_l0_fts(&conn, query, limit).map_err(super::map_err)
    }
    fn search_l0_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L0SearchResult>> {
        let store = self.lock()?;
        if !store.capabilities().vector_search {
            return Ok(Vec::new());
        }
        let conn = store.lock()?;
        crate::l0::search_l0_vector(&conn, embedding, limit).map_err(super::map_err)
    }
    fn count_l0(&self) -> AeonMemoryResult<i64> {
        self.lock()?.count_l0()
    }
    fn count_l1(&self) -> AeonMemoryResult<i64> {
        self.lock()?.count_l1()
    }
}

enum PipelineCommand {
    Notify {
        session_key: String,
        messages: Vec<CapturedMessage>,
    },
    FlushSession {
        session_key: String,
        done: tokio::sync::oneshot::Sender<()>,
    },
    Barrier(tokio::sync::oneshot::Sender<()>),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

/// Runtime/scheduler bridge for the deterministic pipeline manager.
///
/// All manager and runner work lives on one ordered background actor, matching
/// the TS global `SerialQueue` for L1. Capture only sends a command; a slow LLM
/// can neither block the HTTP response nor prevent another session from being
/// accepted into the queue. Flush and shutdown use acknowledgements so their
/// observable completion semantics remain synchronous to their callers.
pub struct PipelineCoreRuntime {
    manager: Arc<Mutex<PipelineManager>>,
    sessions: Mutex<HashSet<String>>,
    command_tx: tokio::sync::mpsc::UnboundedSender<PipelineCommand>,
    command_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<PipelineCommand>>>,
    timer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    l1_running: Arc<Mutex<HashSet<String>>>,
}

impl PipelineCoreRuntime {
    pub fn new(manager: PipelineManager) -> Self {
        Self::new_with_l1_running(manager, Arc::new(Mutex::new(HashSet::new())))
    }

    pub fn new_with_l1_running(
        manager: PipelineManager,
        l1_running: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            manager: Arc::new(Mutex::new(manager)),
            sessions: Mutex::new(HashSet::new()),
            command_tx,
            command_rx: Mutex::new(Some(command_rx)),
            timer_task: Mutex::new(None),
            l1_running,
        }
    }
    async fn barrier(&self) -> AeonMemoryResult<()> {
        let (done, wait) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(PipelineCommand::Barrier(done))
            .map_err(|_| AeonMemoryCoreError::Store("pipeline worker is not running".into()))?;
        wait.await.map_err(|_| {
            AeonMemoryCoreError::Store("pipeline worker stopped before barrier".into())
        })
    }
}

impl CaptureScheduler for PipelineCoreRuntime {
    fn notify_conversation(
        &self,
        session_key: &str,
        messages: &[ConversationMessage],
    ) -> AeonMemoryResult<()> {
        self.sessions
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("session registry poisoned".into()))?
            .insert(session_key.to_owned());
        // TS resets notifications received while the same session's L1 is
        // already awaiting the model; replaying those after success would
        // create an extra long-term memory from that in-flight capture.
        if self
            .l1_running
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("L1 activity registry poisoned".into()))?
            .contains(session_key)
        {
            return Ok(());
        }
        self.command_tx
            .send(PipelineCommand::Notify {
                session_key: session_key.to_owned(),
                messages: messages
                    .iter()
                    .map(|message| CapturedMessage {
                        role: message.role.clone(),
                        content: message.content.clone(),
                        timestamp: chrono::DateTime::from_timestamp_millis(message.timestamp)
                            .unwrap_or_default()
                            .to_rfc3339(),
                    })
                    .collect(),
            })
            .map_err(|_| AeonMemoryCoreError::Store("pipeline worker is not running".into()))
    }
}

#[async_trait]
impl CoreRuntime for PipelineCoreRuntime {
    async fn initialize(&self) -> AeonMemoryResult<()> {
        let mut task = self
            .timer_task
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("timer task lock poisoned".into()))?;
        if task.is_none() {
            let manager = Arc::clone(&self.manager);
            let mut commands = self
                .command_rx
                .lock()
                .map_err(|_| AeonMemoryCoreError::Store("pipeline command lock poisoned".into()))?
                .take()
                .ok_or_else(|| {
                    AeonMemoryCoreError::Store("pipeline runtime cannot be restarted".into())
                })?;
            *task = Some(tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    tokio::select! {
                        command = commands.recv() => match command {
                            Some(PipelineCommand::Notify { session_key, messages }) => {
                                let worker_manager = Arc::clone(&manager);
                                let _ = tokio::task::spawn_blocking(move || {
                                    if let Ok(mut manager) = worker_manager.lock() {
                                        manager.notify_conversation(&session_key, messages);
                                        manager.run_due();
                                    }
                                }).await;
                            }
                            Some(PipelineCommand::FlushSession { session_key, done }) => {
                                let worker_manager = Arc::clone(&manager);
                                let _ = tokio::task::spawn_blocking(move || {
                                    if let Ok(mut manager) = worker_manager.lock() {
                                        manager.flush_session(&session_key);
                                    }
                                }).await;
                                let _ = done.send(());
                            }
                            Some(PipelineCommand::Barrier(done)) => {
                                let _ = done.send(());
                            }
                            Some(PipelineCommand::Shutdown(done)) => {
                                let worker_manager = Arc::clone(&manager);
                                let _ = tokio::task::spawn_blocking(move || {
                                    if let Ok(mut manager) = worker_manager.lock() {
                                        manager.shutdown();
                                    }
                                }).await;
                                let _ = done.send(());
                                break;
                            }
                            None => break,
                        },
                        _ = tick.tick() => {
                            let worker_manager = Arc::clone(&manager);
                            let _ = tokio::task::spawn_blocking(move || {
                                if let Ok(mut manager) = worker_manager.lock() {
                                    manager.run_due();
                                }
                            }).await;
                        }
                    }
                }
            }));
        }
        Ok(())
    }
    async fn destroy(&self) -> AeonMemoryResult<()> {
        let (done, wait) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(PipelineCommand::Shutdown(done))
            .map_err(|_| AeonMemoryCoreError::Store("pipeline worker is not running".into()))?;
        wait.await.map_err(|_| {
            AeonMemoryCoreError::Store("pipeline worker stopped during shutdown".into())
        })?;
        let task = {
            self.timer_task
                .lock()
                .map_err(|_| AeonMemoryCoreError::Store("timer task lock poisoned".into()))?
                .take()
        };
        if let Some(task) = task {
            task.await.map_err(|error| {
                AeonMemoryCoreError::Store(format!("timer task failed: {error}"))
            })?;
        }
        Ok(())
    }
    async fn flush_session(&self, session_key: &str) -> AeonMemoryResult<()> {
        let (done, wait) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(PipelineCommand::FlushSession {
                session_key: session_key.to_owned(),
                done,
            })
            .map_err(|_| AeonMemoryCoreError::Store("pipeline worker is not running".into()))?;
        wait.await.map_err(|_| {
            AeonMemoryCoreError::Store("pipeline worker stopped before session flush".into())
        })?;
        self.sessions
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("session registry poisoned".into()))?
            .remove(session_key);
        Ok(())
    }
    fn active_sessions(&self) -> AeonMemoryResult<u64> {
        Ok(self
            .sessions
            .lock()
            .map_err(|_| AeonMemoryCoreError::Store("session registry poisoned".into()))?
            .len() as u64)
    }
}

/// Seed adapter that uses the same capture and scheduler instances as live
/// traffic, rather than a seed-only persistence shortcut.
pub struct ProductionSeedRuntime {
    capture: Arc<AutoCapture>,
    pipeline: Arc<PipelineCoreRuntime>,
    store: Option<SqliteCoreStore>,
    seen: HashSet<String>,
    started: bool,
}

impl ProductionSeedRuntime {
    pub fn new(
        capture: Arc<AutoCapture>,
        pipeline: Arc<PipelineCoreRuntime>,
        store: Option<SqliteCoreStore>,
    ) -> Self {
        Self {
            capture,
            pipeline,
            store,
            seen: HashSet::new(),
            started: false,
        }
    }
}

#[async_trait]
impl SeedRuntime for ProductionSeedRuntime {
    async fn start(&mut self) -> AeonMemoryResult<()> {
        self.pipeline.initialize().await?;
        self.started = true;
        Ok(())
    }

    async fn capture_round(&mut self, round: SeedRound<'_>) -> AeonMemoryResult<CaptureOutcome> {
        let key = round.idempotency_key;
        let inserted = if let Some(store) = self.store.as_ref() {
            store.claim_seed_key(&key)?
        } else {
            self.seen.insert(key.clone())
        };
        if !inserted {
            return Ok(CaptureOutcome {
                l0_recorded_count: 0,
                idempotent_skip: true,
            });
        }
        let messages = round.messages.iter().map(|message| serde_json::json!({
            "role": message.role,
            "content": message.content,
            "timestamp": message.timestamp,
            "id": format!("seed-{}-{}-{}", round.session_id, round.round_index, message.timestamp),
        })).collect::<Vec<_>>();
        let result = self
            .capture
            .perform(
                &messages,
                round.session_key,
                Some(round.session_id),
                None,
                None,
                None,
                false,
            )
            .await;
        if result.is_err() {
            if let Some(store) = self.store.as_ref() {
                store.release_seed_key(&key)?;
            } else {
                self.seen.remove(&key);
            }
        }
        let result = result?;
        Ok(CaptureOutcome {
            l0_recorded_count: result.l0_recorded_count as usize,
            idempotent_skip: false,
        })
    }

    async fn wait_l1_idle(&mut self, session_keys: &[String]) -> AeonMemoryResult<()> {
        // Observe completion of every capture command submitted before this
        // call without forcing residual below-threshold buffers through L1.
        let _ = session_keys;
        self.pipeline.barrier().await
    }

    async fn destroy(&mut self) -> AeonMemoryResult<()> {
        self.capture.drain().await?;
        if self.started {
            self.pipeline.destroy().await?;
            self.started = false;
        }
        Ok(())
    }
}

type SeedFactory = dyn Fn(
        &Path,
        Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> AeonMemoryResult<(Box<dyn SeedRuntime>, usize)>
    + Send
    + Sync;

/// Creates a fresh seed runtime per request, preserving replay safety in the
/// concrete runtime while allowing `CoreSeedService::seed` to take `&self`.
pub struct RuntimeSeedService {
    factory: Arc<SeedFactory>,
    _every_n_conversations: usize,
    output_dir: PathBuf,
}

impl RuntimeSeedService {
    pub fn new(
        factory: Arc<SeedFactory>,
        every_n_conversations: usize,
        output_dir: PathBuf,
    ) -> Self {
        Self {
            factory,
            _every_n_conversations: every_n_conversations,
            output_dir,
        }
    }
}

#[async_trait]
impl CoreSeedService for RuntimeSeedService {
    async fn seed(
        &self,
        input: &NormalizedInput,
        config_override: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> AeonMemoryResult<SeedSummary> {
        let now = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let output_dir = self.output_dir.join(format!("seed-{now}"));
        let (mut runtime, every_n_conversations) = (self.factory)(&output_dir, config_override)?;
        execute_seed(
            runtime.as_mut(),
            input,
            every_n_conversations,
            &output_dir,
            &mut |_| {},
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aeon_memory_core::pipeline::checkpoint::{Checkpoint, write_checkpoint};
    use aeon_memory_core::scene::{SceneIndexEntry, write_scene_index};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct SlowRunner {
        delay: Duration,
        events: Arc<Mutex<Vec<String>>>,
        l1_runs: Arc<AtomicUsize>,
    }

    struct PersonaFileLlm {
        prompts: Arc<Mutex<Vec<String>>>,
    }

    struct SceneNoopLlm {
        timeouts: Arc<Mutex<Vec<Option<u64>>>>,
    }

    struct PromptCaptureLlm {
        prompts: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LlmRunner for PromptCaptureLlm {
        async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
            self.prompts.lock().unwrap().push(params.prompt);
            Ok("[]".into())
        }
    }

    #[async_trait]
    impl LlmRunner for SceneNoopLlm {
        async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
            self.timeouts.lock().unwrap().push(params.timeout_ms);
            Ok(String::new())
        }
    }

    #[async_trait]
    impl LlmRunner for PersonaFileLlm {
        async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
            self.prompts.lock().unwrap().push(params.prompt);
            std::fs::write(
                PathBuf::from(params.workspace_dir.unwrap()).join("persona.md"),
                "# User Profile\n\nGenerated from scenes.",
            )?;
            Ok(String::new())
        }
    }

    impl PipelineRunner for SlowRunner {
        fn run_l1(&mut self, session: &str, _messages: &[CapturedMessage]) -> Result<(), String> {
            std::thread::sleep(self.delay);
            self.events.lock().unwrap().push(format!("l1:{session}"));
            self.l1_runs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn run_l2(&mut self, session: &str, _: Option<&str>) -> Result<L2Result, String> {
            self.events.lock().unwrap().push(format!("l2:{session}"));
            Ok(L2Result {
                skipped: true,
                ..Default::default()
            })
        }

        fn run_l3(&mut self) -> Result<(), String> {
            self.events.lock().unwrap().push("l3".into());
            Ok(())
        }
    }

    fn pipeline_runtime(
        every_n_conversations: u32,
        delay: Duration,
    ) -> (
        PipelineCoreRuntime,
        Arc<Mutex<Vec<String>>>,
        Arc<AtomicUsize>,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let l1_runs = Arc::new(AtomicUsize::new(0));
        let manager = PipelineManager::new(
            aeon_memory_core::pipeline::manager::PipelineConfig {
                every_n_conversations,
                enable_warmup: false,
                l1_idle_timeout_ms: 60_000,
                l2_delay_after_l1_ms: 60_000,
                l2_min_interval_ms: 0,
                l2_max_interval_ms: 3_600_000,
                session_active_window_ms: 86_400_000,
            },
            Box::new(aeon_memory_core::pipeline::manager::SystemClock),
            Box::new(SlowRunner {
                delay,
                events: events.clone(),
                l1_runs: l1_runs.clone(),
            }),
        );
        (PipelineCoreRuntime::new(manager), events, l1_runs)
    }

    fn captured(content: &str) -> ConversationMessage {
        ConversationMessage {
            id: content.into(),
            role: "user".into(),
            content: content.into(),
            timestamp: 1,
        }
    }

    fn vector_store(name: &str) -> (SqliteCoreStore, PathBuf) {
        let path =
            crate::test_support::unique_dir("aeon-memory-adapter").join(format!("{name}.db"));
        let store = SqliteCoreStore::new(VectorStore::new(&path.to_string_lossy(), 2));
        store.initialize(None).unwrap();
        (store, path)
    }

    fn cleanup(path: &std::path::Path) {
        crate::test_support::cleanup_db(path);
    }

    #[test]
    fn degraded_jsonl_newest_fifty_preserve_ts_prompt_order() {
        aeon_memory_core::utils::time::init_time_module("UTC");
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-degraded-order-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        let lines = (0..60)
            .map(|index| {
                serde_json::json!({
                    "sessionKey": "degraded-60",
                    "sessionId": if index < 30 { "round-a" } else { "round-b" },
                    "recordedAt": format!("2026-01-01T00:00:{index:02}Z"),
                    "id": format!("m{index:02}"),
                    "role": if index % 2 == 0 { "user" } else { "assistant" },
                    "content": format!("ordered-message-{index:02}"),
                    "timestamp": 1000 + index,
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(root.join("conversations/2026-01-01.jsonl"), lines + "\n").unwrap();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let llm: Arc<dyn LlmRunner> = Arc::new(PromptCaptureLlm {
            prompts: Arc::clone(&prompts),
        });
        let embedding: Arc<dyn EmbeddingService> =
            Arc::new(aeon_memory_core::embedding::openai::NoopEmbeddingService::new());
        let mut runner = ProductionPipelineRunner {
            store: None,
            data_dir: root.clone(),
            l1_llm: Arc::clone(&llm),
            scene_llm: Arc::clone(&llm),
            persona_llm: llm,
            embedding,
            embedding_provider: None,
            l1_running: Arc::new(Mutex::new(HashSet::new())),
            l1_options: L1ExtractionOptions {
                enable_dedup: false,
                ..Default::default()
            },
            max_scenes: 15,
            persona_backup_count: 3,
            scene_backup_count: 10,
            persona_trigger_every_n: 50,
        };
        runner.run_l1("degraded-60", &[]).unwrap();

        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../../aeon-memory-core/tests/fixtures/degraded_l1_oracle.json"
        ))
        .unwrap();
        let expected_groups = oracle["groups"].as_array().unwrap();
        let prompts = prompts.lock().unwrap();
        assert_eq!(prompts.len(), expected_groups.len());
        for (prompt, group) in prompts.iter().zip(expected_groups) {
            assert_eq!(prompt, group["prompt"].as_str().unwrap());
        }
        assert_eq!(
            expected_groups[0]["messages"][0]["id"],
            serde_json::json!("m10")
        );
        assert_eq!(
            expected_groups[1]["messages"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["id"],
            serde_json::json!("m59")
        );
        assert!(
            !prompts
                .iter()
                .any(|prompt| prompt.contains("ordered-message-09"))
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("ordered-message-59"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn production_l3_runner_gates_persona_llm_with_checkpoint_trigger() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-production-persona-trigger-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scene_blocks")).unwrap();
        std::fs::write(root.join("scene_blocks/work.md"), "# Work\n\nUses Rust.").unwrap();
        std::fs::write(root.join("persona.md"), "existing persona body").unwrap();
        write_scene_index(
            &root,
            &[SceneIndexEntry {
                filename: "work.md".into(),
                summary: "work".into(),
                heat: 3,
                created: "2026-07-13T00:00:00Z".into(),
                updated: "2026-07-13T00:00:00Z".into(),
            }],
        )
        .unwrap();
        write_checkpoint(
            root.to_str().unwrap(),
            &Checkpoint {
                total_processed: 5,
                scenes_processed: 3,
                last_persona_at: 3,
                memories_since_last_persona: 4,
                ..Default::default()
            },
        )
        .unwrap();

        let db = root.join("vectors.db");
        let store = SqliteCoreStore::new(VectorStore::new(&db.to_string_lossy(), 0));
        store.initialize(None).unwrap();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let llm: Arc<dyn LlmRunner> = Arc::new(PersonaFileLlm {
            prompts: prompts.clone(),
        });
        let embedding: Arc<dyn EmbeddingService> =
            Arc::new(aeon_memory_core::record::l1_dedup::RecordingMockEmbedding::new(Vec::new()));
        let mut runner = ProductionPipelineRunner {
            store: Some(store),
            data_dir: root.clone(),
            l1_llm: Arc::clone(&llm),
            scene_llm: Arc::clone(&llm),
            persona_llm: llm,
            embedding,
            embedding_provider: None,
            l1_running: Arc::new(Mutex::new(HashSet::new())),
            l1_options: Default::default(),
            max_scenes: 15,
            persona_backup_count: 3,
            scene_backup_count: 10,
            persona_trigger_every_n: 5,
        };

        runner.run_l3().unwrap();
        assert!(prompts.lock().unwrap().is_empty());

        mutate_checkpoint(root.to_str().unwrap(), |checkpoint| {
            checkpoint.memories_since_last_persona = 5;
        })
        .unwrap();
        runner.run_l3().unwrap();
        let prompts = prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../../aeon-memory-core/tests/fixtures/persona_trigger_oracle.json"
        ))
        .unwrap();
        let expected_reason = oracle
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["case"]["name"] == "threshold")
            .unwrap()["result"]["reason"]
            .as_str()
            .unwrap();
        assert!(prompts[0].contains(expected_reason));
        drop(prompts);
        let checkpoint = read_checkpoint(root.to_str().unwrap()).unwrap();
        assert_eq!(checkpoint.memories_since_last_persona, 0);
        assert_eq!(checkpoint.last_persona_at, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn production_l2_runner_uses_scene_backup_count_and_five_minute_timeout() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-production-scene-settings-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scene_blocks")).unwrap();
        std::fs::write(root.join("scene_blocks/existing.md"), "existing scene").unwrap();
        let backup_root = root.join(".backup/scene_blocks");
        for index in 0..12 {
            let dir = backup_root.join(format!(
                "scene_blocks_20260101_0000{index:02}_offset{index}"
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("old.md"), "old").unwrap();
        }

        let db = root.join("vectors.db");
        let store = SqliteCoreStore::new(VectorStore::new(&db.to_string_lossy(), 0));
        store.initialize(None).unwrap();
        store
            .with_store(|store| {
                store.upsert_l1(
                    &L1RecordRow {
                        record_id: "l1-scene".into(),
                        content: "User prefers deterministic Rust tests.".into(),
                        r#type: "preference".into(),
                        priority: 80.0,
                        scene_name: String::new(),
                        session_key: "agent:a:normal".into(),
                        session_id: String::new(),
                        timestamp_str: String::new(),
                        timestamp_start: String::new(),
                        timestamp_end: String::new(),
                        created_time: "2026-07-13T00:00:00Z".into(),
                        updated_time: "2026-07-13T00:00:01Z".into(),
                        metadata_json: "{}".into(),
                    },
                    None,
                )?;
                Ok(())
            })
            .unwrap();
        let timeouts = Arc::new(Mutex::new(Vec::new()));
        let llm: Arc<dyn LlmRunner> = Arc::new(SceneNoopLlm {
            timeouts: timeouts.clone(),
        });
        let embedding: Arc<dyn EmbeddingService> =
            Arc::new(aeon_memory_core::record::l1_dedup::RecordingMockEmbedding::new(Vec::new()));
        let mut runner = ProductionPipelineRunner {
            store: Some(store),
            data_dir: root.clone(),
            l1_llm: Arc::clone(&llm),
            scene_llm: Arc::clone(&llm),
            persona_llm: llm,
            embedding,
            embedding_provider: None,
            l1_running: Arc::new(Mutex::new(HashSet::new())),
            l1_options: Default::default(),
            max_scenes: 15,
            persona_backup_count: 1,
            scene_backup_count: 10,
            persona_trigger_every_n: 50,
        };

        let result = runner.run_l2("agent:a:normal", None).unwrap();
        assert!(!result.skipped);
        assert_eq!(&*timeouts.lock().unwrap(), &[Some(300_000)]);
        assert_eq!(std::fs::read_dir(&backup_root).unwrap().count(), 10);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_l1_never_blocks_capture_and_flush_waits_for_completion() {
        let (runtime, events, runs) = pipeline_runtime(1, Duration::from_millis(250));
        runtime.initialize().await.unwrap();

        let started = Instant::now();
        runtime.notify_conversation("a", &[captured("a")]).unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "capture notification executed slow L1 inline"
        );

        tokio::time::sleep(Duration::from_millis(25)).await;
        let second = Instant::now();
        runtime.notify_conversation("b", &[captured("b")]).unwrap();
        assert!(
            second.elapsed() < Duration::from_millis(50),
            "another session was blocked by the running L1"
        );

        let flush_started = Instant::now();
        runtime.flush_session("a").await.unwrap();
        assert!(flush_started.elapsed() >= Duration::from_millis(400));
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(&*events.lock().unwrap(), &["l1:a", "l1:b"]);
        // Per-session flush removes only the requested session.
        assert_eq!(runtime.active_sessions().unwrap(), 1);

        runtime.destroy().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn filtered_session_capture_persists_l0_but_end_session_never_runs_l1() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-filtered-capture-pipeline-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".metadata")).unwrap();
        let db = root.join("vectors.db");
        let store = SqliteCoreStore::new(VectorStore::new(&db.to_string_lossy(), 0));
        store.initialize(None).unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        let l1_runs = Arc::new(AtomicUsize::new(0));
        let manager = PipelineManager::new(
            aeon_memory_core::pipeline::manager::PipelineConfig {
                every_n_conversations: 1,
                enable_warmup: false,
                l1_idle_timeout_ms: 60_000,
                l2_delay_after_l1_ms: 60_000,
                l2_min_interval_ms: 0,
                l2_max_interval_ms: 3_600_000,
                session_active_window_ms: 86_400_000,
            },
            Box::new(aeon_memory_core::pipeline::manager::SystemClock),
            Box::new(SlowRunner {
                delay: Duration::ZERO,
                events: events.clone(),
                l1_runs: l1_runs.clone(),
            }),
        )
        .with_session_filter(aeon_memory_core::utils::session_filter::SessionFilter::new(
            &[],
        ));
        let runtime = Arc::new(PipelineCoreRuntime::new(manager));
        runtime.initialize().await.unwrap();
        let capture = AutoCapture::new(
            Arc::new(LocalCaptureRecorder {
                data_dir: root.to_string_lossy().into_owned(),
            }),
            Some(Arc::new(store.clone())),
            None,
            Some(runtime.clone()),
        );
        let key = "agent:a:subagent:worker";
        let result = capture
            .perform(
                &[
                    serde_json::json!({"id":"u","role":"user","content":"remember this","timestamp":1000}),
                    serde_json::json!({"id":"a","role":"assistant","content":"noted","timestamp":1001}),
                ],
                key,
                Some("sid"),
                None,
                None,
                None,
                false,
            )
            .await
            .unwrap();
        assert_eq!(result.l0_recorded_count, 2);
        assert!(result.scheduler_notified);
        runtime.flush_session(key).await.unwrap();
        assert_eq!(store.with_store(|store| store.count_l0()).unwrap(), 2);
        assert_eq!(l1_runs.load(Ordering::SeqCst), 0);
        assert!(events.lock().unwrap().is_empty());
        runtime.destroy().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_drains_residual_below_threshold_work() {
        let (runtime, events, runs) = pipeline_runtime(5, Duration::from_millis(150));
        runtime.initialize().await.unwrap();
        runtime
            .notify_conversation("residual", &[captured("pending")])
            .unwrap();

        let started = Instant::now();
        runtime.destroy().await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(&*events.lock().unwrap(), &["l1:residual"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crash_restart_recovers_pre_l1_pending_as_official_delayed_l2() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-pipeline-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".metadata")).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let config = aeon_memory_core::pipeline::manager::PipelineConfig {
            every_n_conversations: 5,
            enable_warmup: false,
            l1_idle_timeout_ms: 60_000,
            l2_delay_after_l1_ms: 0,
            l2_min_interval_ms: 0,
            l2_max_interval_ms: 3_600_000,
            session_active_window_ms: 86_400_000,
        };
        let manager = PipelineManager::new(
            config.clone(),
            Box::new(aeon_memory_core::pipeline::manager::SystemClock),
            Box::new(SlowRunner {
                delay: Duration::ZERO,
                events: events.clone(),
                l1_runs: runs.clone(),
            }),
        )
        .with_persister(Box::new(CheckpointStatePersister {
            data_dir: root.clone(),
        }));
        let runtime = PipelineCoreRuntime::new(manager);
        runtime.initialize().await.unwrap();
        runtime
            .notify_conversation("pending", &[captured("not-yet-l1")])
            .unwrap();
        runtime.barrier().await.unwrap();
        let before = read_checkpoint(&root.to_string_lossy()).unwrap();
        assert_eq!(before.pipeline_states["pending"].conversation_count, 1);

        // Simulate process loss: do not call the graceful destroy path.
        drop(runtime);
        tokio::task::yield_now().await;

        let restored = read_checkpoint(&root.to_string_lossy())
            .unwrap()
            .pipeline_states;
        let mut restarted = PipelineManager::new(
            config,
            Box::new(aeon_memory_core::pipeline::manager::SystemClock),
            Box::new(SlowRunner {
                delay: Duration::ZERO,
                events: events.clone(),
                l1_runs: runs,
            }),
        );
        restarted.start(restored);
        let recovered = restarted.session_state("pending").unwrap();
        assert_eq!(recovered.conversation_count, 0);
        assert_eq!(recovered.l2_pending_l1_count, 1);
        restarted.run_due();
        assert_eq!(&*events.lock().unwrap(), &["l2:pending"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deferred_embedding_is_written_to_vec0_and_searchable() {
        let (store, path) = vector_store("deferred");
        assert!(store.lock().unwrap().capabilities().vector_search);
        let record = L0Record {
            id: "deferred-1".into(),
            session_key: "session-a".into(),
            session_id: "sid".into(),
            role: "user".into(),
            message_text: "remember the blue bicycle".into(),
            recorded_at: "2026-07-13T00:00:00Z".into(),
            timestamp: 1,
        };
        assert!(CaptureStore::upsert_l0(&store, &record, None).unwrap());
        assert!(CaptureStore::update_l0_embedding(&store, &record.id, &[1.0, 0.0]).unwrap());

        let found = CoreSearchStore::search_l0_vector(&store, &[1.0, 0.0], 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].record_id, record.id);
        assert!(found[0].score > 0.99);
        cleanup(&path);
    }

    #[test]
    fn deferred_embedding_rejects_missing_record_without_orphan_vec_row() {
        let (store, path) = vector_store("missing");
        assert!(!CaptureStore::update_l0_embedding(&store, "missing", &[1.0, 0.0]).unwrap());
        assert!(
            CoreSearchStore::search_l0_vector(&store, &[1.0, 0.0], 5)
                .unwrap()
                .is_empty()
        );
        cleanup(&path);
    }

    #[test]
    fn core_search_store_exposes_l0_fts() {
        let (store, path) = vector_store("fts");
        let record = L0Record {
            id: "fts-1".into(),
            session_key: "session-a".into(),
            session_id: "sid".into(),
            role: "assistant".into(),
            message_text: "unique telescope observation".into(),
            recorded_at: "2026-07-13T00:00:00Z".into(),
            timestamp: 1,
        };
        CaptureStore::upsert_l0(&store, &record, None).unwrap();
        let found = CoreSearchStore::search_l0_fts(&store, "telescope", 5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].record_id, record.id);
        cleanup(&path);
    }
}
