//! Production composition shared by both binaries.

use aeon_memory_core::{
    AeonMemoryCore, AeonMemoryCoreError,
    aeon_memory_core::{AeonMemoryCoreOptions, CoreRuntime},
    config::{DisableThinkingStrategy, GatewayConfig, OffloadMode, default_offload_data_root},
    embedding::openai::{NoopEmbeddingService, OpenAiEmbeddingConfig, OpenAiEmbeddingService},
    hooks::auto_capture::LocalCaptureRecorder,
    hooks::{AutoCapture, AutoRecall},
    llm::openai::{OpenAiLlmRunner, StandaloneLlmConfig},
    offload::{
        OffloadConfig, OffloadEngine,
        prompt::MmdMeta,
        reclaim::{ReclaimConfig, reclaim},
        types::{OffloadEntry, ToolPair},
    },
    offload::{
        inject, l3, prompt,
        storage::{self, StorageContext},
        token::{O200kTokenizer, snapshot},
    },
    pipeline::{
        checkpoint::read_checkpoint,
        manager::{PipelineConfig, PipelineManager, SystemClock},
    },
    record::l1_extractor::L1ExtractionOptions,
    types::{EmbeddingProviderInfo, EmbeddingService, IMemoryStore, LlmRunParams, LlmRunner},
    utils::manifest::{
        Manifest, ManifestSqliteInfo, ManifestStoreInfo, read_manifest, write_manifest,
    },
    utils::session_filter::SessionFilter,
};
use aeon_memory_store_sqlite::{
    VectorStore,
    adapter::{
        CheckpointStatePersister, PipelineCoreRuntime, ProductionPipelineRunner,
        ProductionSeedRuntime, RuntimeSeedService, SqliteCoreStore,
    },
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    adapter::{CleanerOperations, ComponentHealth, CoreService, OffloadOperations},
    service::*,
};

struct DisabledPipelineRuntime;

#[async_trait]
impl CoreRuntime for DisabledPipelineRuntime {
    async fn initialize(&self) -> aeon_memory_core::AeonMemoryResult<()> {
        Ok(())
    }

    async fn destroy(&self) -> aeon_memory_core::AeonMemoryResult<()> {
        Ok(())
    }

    async fn flush_session(&self, _session_key: &str) -> aeon_memory_core::AeonMemoryResult<()> {
        Ok(())
    }

    fn active_sessions(&self) -> aeon_memory_core::AeonMemoryResult<u64> {
        Ok(0)
    }
}

pub async fn build_core(config: &GatewayConfig) -> Result<Arc<CoreService>, AeonMemoryCoreError> {
    aeon_memory_core::utils::time::init_time_module(&config.memory.timezone);
    let data_dir = PathBuf::from(&config.data.base_dir);
    std::fs::create_dir_all(&data_dir)?;
    if config.llm.base_url.trim().is_empty()
        || config.llm.api_key.trim().is_empty()
        || config.llm.model.trim().is_empty()
    {
        return Err(AeonMemoryCoreError::InvalidInput(
            "external LLM baseUrl, apiKey and model are required".into(),
        ));
    }
    let llm_cfg = StandaloneLlmConfig {
        base_url: config.llm.base_url.clone(),
        api_key: config.llm.api_key.clone(),
        model: config.llm.model.clone(),
        max_tokens: config.llm.max_tokens,
        timeout_ms: config.llm.timeout_ms,
        disable_thinking: match config.llm.disable_thinking {
            DisableThinkingStrategy::Disabled => None,
            DisableThinkingStrategy::Vllm => Some("vllm".into()),
            DisableThinkingStrategy::DeepSeek => Some("deepseek".into()),
            DisableThinkingStrategy::DashScope => Some("dashscope".into()),
            DisableThinkingStrategy::OpenAI => Some("openai".into()),
            DisableThinkingStrategy::Anthropic => Some("anthropic".into()),
            DisableThinkingStrategy::Kimi => Some("kimi".into()),
            DisableThinkingStrategy::Gemini => Some("gemini".into()),
        },
    };
    // The runner exposes restricted file tools only for calls that explicitly
    // provide both a workspace_dir and a FileToolPolicy (L2 scenes and L3
    // persona). Text-only L1, dedup, and offload calls provide neither and
    // therefore send no tool schemas.
    let l1_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_cfg.clone(),
        scoped_model(config.memory.extraction.model.as_deref()),
        true,
    ));
    let scene_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_cfg.clone(),
        scoped_model(config.memory.extraction.model.as_deref()),
        true,
    ));
    let persona_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_cfg.clone(),
        scoped_model(config.memory.persona.model.as_deref()),
        true,
    ));
    let (embedding, provider): (Arc<dyn EmbeddingService>, Option<EmbeddingProviderInfo>) =
        if config.memory.embedding.enabled {
            let svc = OpenAiEmbeddingService::new(OpenAiEmbeddingConfig {
                provider: config.memory.embedding.provider.clone(),
                base_url: config.memory.embedding.base_url.clone(),
                // Match the pinned TS sqlite factory, which does not forward
                // proxyUrl into its production embedding service. Direct
                // OpenAiEmbeddingService construction still supports qclaw.
                proxy_url: None,
                api_key: config.memory.embedding.api_key.clone(),
                model: config.memory.embedding.model.clone(),
                dimensions: config.memory.embedding.dimensions,
                send_dimensions: config.memory.embedding.send_dimensions,
                max_input_chars: config.memory.embedding.max_input_chars,
                timeout_ms: config
                    .memory
                    .embedding
                    .capture_timeout_ms
                    .unwrap_or(config.memory.embedding.timeout_ms),
            });
            let info = svc.provider_info();
            (Arc::new(svc), Some(info))
        } else {
            (Arc::new(NoopEmbeddingService::new()), None)
        };
    let candidate_store = SqliteCoreStore::new(VectorStore::new(
        &data_dir.join("vectors.db").to_string_lossy(),
        embedding.dimensions(),
    ));
    // The pinned TS factory treats store initialization failure and degraded
    // sqlite-vec/schema state as an unavailable optional component. It keeps
    // the host alive but removes both store and embedding from the pipeline.
    let store = match candidate_store.initialize(provider.as_ref()) {
        Ok(_) if candidate_store.with_store(|store| Ok(!store.is_degraded()))? => {
            Some(candidate_store)
        }
        Ok(_) | Err(AeonMemoryCoreError::Store(_)) => None,
        Err(error) => return Err(error),
    };
    let vector_store_healthy = store.is_some();
    if vector_store_healthy {
        ensure_sqlite_manifest(&data_dir);
    }
    let cleaner = if config.memory.memory_cleanup.enabled
        && let Some(days) = config.memory.memory_cleanup.retention_days
    {
        Some(spawn_memory_cleaner(
            store.clone(),
            data_dir.clone(),
            days,
            config.memory.memory_cleanup.clean_time.clone(),
            config.memory.timezone.clone(),
        ) as Arc<dyn CleanerOperations>)
    } else {
        None
    };
    // A disabled embedding provider is absence, not a zero-dimensional
    // provider. Passing Noop as `Some` would schedule deferred empty-vector
    // writes and incorrectly require sqlite-vec during capture/seed.
    let live_embedding =
        (config.memory.embedding.enabled && store.is_some()).then(|| Arc::clone(&embedding));
    let (runtime, capture_scheduler): (
        Arc<dyn CoreRuntime>,
        Option<Arc<dyn aeon_memory_core::hooks::auto_capture::CaptureScheduler>>,
    ) = if config.memory.extraction.enabled {
        let pc = &config.memory.pipeline;
        let l1_running = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let runner = ProductionPipelineRunner {
            store: store.clone(),
            data_dir: data_dir.clone(),
            l1_llm: Arc::clone(&l1_llm),
            scene_llm: Arc::clone(&scene_llm),
            persona_llm: Arc::clone(&persona_llm),
            embedding: store.as_ref().map_or_else(
                || Arc::new(NoopEmbeddingService::new()) as Arc<dyn EmbeddingService>,
                |_| Arc::clone(&embedding),
            ),
            embedding_provider: store.as_ref().and(provider.clone()),
            l1_running: Arc::clone(&l1_running),
            l1_options: L1ExtractionOptions {
                enable_dedup: config.memory.extraction.enable_dedup,
                max_memories_per_session: config.memory.extraction.max_memories_per_session
                    as usize,
                conflict_recall_top_k: config.memory.embedding.conflict_recall_top_k,
            },
            max_scenes: config.memory.persona.max_scenes as usize,
            persona_backup_count: config.memory.persona.backup_count as usize,
            scene_backup_count: config.memory.persona.scene_backup_count as usize,
            persona_trigger_every_n: u64::from(config.memory.persona.trigger_every_n),
        };
        let mut manager = PipelineManager::new(
            PipelineConfig {
                every_n_conversations: pc.every_n_conversations,
                enable_warmup: pc.enable_warmup,
                l1_idle_timeout_ms: i64::from(pc.l1_idle_timeout_seconds) * 1000,
                l2_delay_after_l1_ms: i64::from(pc.l2_delay_after_l1_seconds) * 1000,
                l2_min_interval_ms: i64::from(pc.l2_min_interval_seconds) * 1000,
                l2_max_interval_ms: i64::from(pc.l2_max_interval_seconds) * 1000,
                session_active_window_ms: i64::from(pc.session_active_window_hours) * 3_600_000,
            },
            Box::new(SystemClock),
            Box::new(runner),
        )
        .with_session_filter(SessionFilter::new(&config.memory.capture.exclude_agents))
        .with_persister(Box::new(CheckpointStatePersister {
            data_dir: data_dir.clone(),
        }));
        manager.start(read_checkpoint(&config.data.base_dir)?.pipeline_states);
        let pipeline = Arc::new(PipelineCoreRuntime::new_with_l1_running(
            manager, l1_running,
        ));
        (
            pipeline.clone() as Arc<dyn CoreRuntime>,
            Some(pipeline as Arc<dyn aeon_memory_core::hooks::auto_capture::CaptureScheduler>),
        )
    } else {
        (Arc::new(DisabledPipelineRuntime), None)
    };
    // L0 capture always owns the atomic checkpoint cursor, even when L1+
    // extraction is disabled. Only scheduler notification is conditional.
    let recorder = Arc::new(LocalCaptureRecorder {
        data_dir: config.data.base_dir.clone(),
    });
    let capture = Arc::new(AutoCapture::new(
        recorder,
        store.as_ref().map(|store| {
            Arc::new(store.clone()) as Arc<dyn aeon_memory_core::hooks::auto_capture::CaptureStore>
        }),
        live_embedding.clone(),
        capture_scheduler,
    ));
    let recall = Arc::new(AutoRecall {
        config: config.memory.clone(),
        data_dir: data_dir.clone(),
        store: store.as_ref().map(|store| {
            Arc::new(store.clone()) as Arc<dyn aeon_memory_core::hooks::auto_recall::RecallStore>
        }),
        embedding: if live_embedding.is_some() {
            Some(Arc::new(OpenAiEmbeddingService::new(OpenAiEmbeddingConfig {
                provider: config.memory.embedding.provider.clone(),
                base_url: config.memory.embedding.base_url.clone(),
                // Production sqlite wiring follows the pinned TS factory and
                // therefore calls baseUrl directly even for qclaw.
                proxy_url: None,
                api_key: config.memory.embedding.api_key.clone(),
                model: config.memory.embedding.model.clone(),
                dimensions: config.memory.embedding.dimensions,
                send_dimensions: config.memory.embedding.send_dimensions,
                max_input_chars: config.memory.embedding.max_input_chars,
                timeout_ms: config
                    .memory
                    .embedding
                    .recall_timeout_ms
                    .unwrap_or(config.memory.embedding.timeout_ms),
            })) as Arc<dyn EmbeddingService>)
        } else {
            None
        },
    });
    let seed_config = config.clone();
    let seed_llm_config = llm_cfg.clone();
    let seed_embedding = Arc::clone(&embedding);
    let seed_provider = provider.clone();
    let seed = Arc::new(RuntimeSeedService::new(
        Arc::new(move |seed_dir, config_override| {
            let mut effective = seed_config.clone();
            if let Some(config_override) = config_override {
                let mut memory = serde_json::to_value(&effective.memory)
                    .map_err(|error| AeonMemoryCoreError::InvalidInput(error.to_string()))?;
                let target = memory.as_object_mut().ok_or_else(|| {
                    AeonMemoryCoreError::InvalidInput("memory config must be an object".into())
                })?;
                for (key, value) in config_override {
                    if let (
                        Some(serde_json::Value::Object(base)),
                        serde_json::Value::Object(over),
                    ) = (target.get_mut(key), value)
                    {
                        for (nested_key, nested_value) in over {
                            let compatible = base.get(nested_key).is_none_or(|current| {
                                std::mem::discriminant(current)
                                    == std::mem::discriminant(nested_value)
                            });
                            if compatible && !nested_value.is_null() {
                                base.insert(nested_key.clone(), nested_value.clone());
                            }
                        }
                    } else {
                        let compatible = target.get(key).is_none_or(|current| {
                            std::mem::discriminant(current) == std::mem::discriminant(value)
                        });
                        if compatible && !value.is_null() {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                }
                effective.memory = serde_json::from_value(memory).map_err(|error| {
                    AeonMemoryCoreError::InvalidInput(format!(
                        "invalid seed config_override: {error}"
                    ))
                })?;
            }
            let every_n = effective.memory.pipeline.every_n_conversations as usize;
            let runtime = build_seed_runtime(
                seed_dir,
                &effective,
                seed_llm_config.clone(),
                Arc::clone(&seed_embedding),
                seed_provider.as_ref(),
            )?;
            Ok((runtime, every_n))
        }),
        config.memory.pipeline.every_n_conversations as usize,
        data_dir.clone(),
    ));
    let core = Arc::new(AeonMemoryCore::new(AeonMemoryCoreOptions {
        data_dir: data_dir.clone(),
        capture,
        recall,
        runtime,
        search_store: store.as_ref().map(|store| {
            Arc::new(store.clone()) as Arc<dyn aeon_memory_core::aeon_memory_core::CoreSearchStore>
        }),
        embedding: live_embedding.clone(),
        seed_service: Some(seed as Arc<dyn aeon_memory_core::aeon_memory_core::CoreSeedService>),
    }));
    let offload_source = &config.memory.offload;
    let offload_root = offload_source
        .data_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(default_offload_data_root);
    let offload_runner: Option<Arc<dyn LlmRunner>> = match offload_source.mode {
        OffloadMode::Local => {
            let mut local_config = llm_cfg.clone();
            if let Some(model) = offload_source.model.as_deref() {
                local_config.model = model
                    .split_once('/')
                    .map_or(model, |(_, model_id)| model_id)
                    .to_owned();
            }
            local_config.timeout_ms = offload_source.backend_timeout_ms;
            local_config.disable_thinking = disable_thinking_name(&offload_source.disable_thinking);
            Some(Arc::new(
                OpenAiLlmRunner::new(local_config, None, false)
                    .with_temperature(offload_source.temperature),
            ))
        }
        OffloadMode::Backend | OffloadMode::Collect => offload_source
            .backend_url
            .as_ref()
            .filter(|url| !url.trim().is_empty())
            .map(|url| {
                Arc::new(BackendOffloadRunner {
                    base_url: url.clone(),
                    api_key: offload_source.backend_api_key.clone(),
                    user_id: resolve_offload_user_id(offload_source.user_id.as_deref()),
                    timeout_ms: offload_source.backend_timeout_ms,
                }) as Arc<dyn LlmRunner>
            }),
    };
    let offload = Arc::new(EngineOffloadOperations::new_configured(
        offload_root,
        offload_source.enabled,
        offload_runner,
        matches!(offload_source.mode, OffloadMode::Collect),
        offload_config(config),
    ));
    if offload_source.enabled {
        offload.start_reclaim_scheduler(
            Duration::from_secs(5 * 60),
            Duration::from_secs(24 * 60 * 60),
        );
    }
    let service = Arc::new(CoreService::new(
        core,
        Some(offload),
        cleaner,
        ComponentHealth {
            // TS reports whether a usable store exists. A provider drift marks
            // vectors for reindex but does not remove the SQLite/FTS store.
            vector_store: vector_store_healthy,
            embedding_service: live_embedding.is_some(),
        },
    ));
    service
        .initialize()
        .await
        .map_err(|error| AeonMemoryCoreError::Store(error.to_string()))?;
    Ok(service)
}

fn disable_thinking_name(strategy: &DisableThinkingStrategy) -> Option<String> {
    match strategy {
        DisableThinkingStrategy::Disabled => None,
        DisableThinkingStrategy::Vllm => Some("vllm".into()),
        DisableThinkingStrategy::DeepSeek => Some("deepseek".into()),
        DisableThinkingStrategy::DashScope => Some("dashscope".into()),
        DisableThinkingStrategy::OpenAI => Some("openai".into()),
        DisableThinkingStrategy::Anthropic => Some("anthropic".into()),
        DisableThinkingStrategy::Kimi => Some("kimi".into()),
        DisableThinkingStrategy::Gemini => Some("gemini".into()),
    }
}

fn scoped_model(model: Option<&str>) -> Option<String> {
    model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| model.split_once('/').map_or(model, |(_, id)| id).to_owned())
}

fn resolve_offload_user_id(explicit: Option<&str>) -> Option<String> {
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        return Some(explicit.to_owned());
    }
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) if !address.is_loopback() => Some(address.to_string()),
        _ => None,
    }
}

struct MemoryCleanerTask {
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[async_trait]
impl CleanerOperations for MemoryCleanerTask {
    async fn shutdown(&self) -> ServiceResult<()> {
        let stop = self
            .stop
            .lock()
            .map_err(|_| ServiceError::Internal("cleaner stop lock poisoned".into()))?
            .take();
        if let Some(stop) = stop {
            let _ = stop.send(());
        }
        let task = self
            .task
            .lock()
            .map_err(|_| ServiceError::Internal("cleaner task lock poisoned".into()))?
            .take();
        if let Some(task) = task {
            task.await
                .map_err(|error| ServiceError::Internal(format!("cleaner task failed: {error}")))?;
        }
        Ok(())
    }
}

fn spawn_memory_cleaner(
    store: Option<SqliteCoreStore>,
    data_dir: std::path::PathBuf,
    retention_days: u32,
    clean_time: String,
    timezone: String,
) -> Arc<MemoryCleanerTask> {
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let next_ms = aeon_memory_core::utils::memory_cleaner::next_run_at_ms(
                &clean_time,
                now_ms,
                &timezone,
            )
            .unwrap_or(now_ms + 60 * 60 * 1000);
            let delay = std::time::Duration::from_millis(next_ms.saturating_sub(now_ms) as u64);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = &mut stopped => break,
            }
            if let Some(store) = store.as_ref() {
                let _ = store.with_store(|s| {
                    aeon_memory_core::utils::memory_cleaner::run_once(
                        &data_dir,
                        retention_days,
                        chrono::Utc::now().timestamp_millis(),
                        Some(s),
                    )
                    .map(|_| ())
                });
            } else {
                let _ = aeon_memory_core::utils::memory_cleaner::run_once(
                    &data_dir,
                    retention_days,
                    chrono::Utc::now().timestamp_millis(),
                    None,
                );
            }
        }
    });
    Arc::new(MemoryCleanerTask {
        stop: Mutex::new(Some(stop)),
        task: Mutex::new(Some(task)),
    })
}

fn build_seed_runtime(
    data_dir: &std::path::Path,
    config: &GatewayConfig,
    llm_config: StandaloneLlmConfig,
    embedding: Arc<dyn EmbeddingService>,
    provider: Option<&EmbeddingProviderInfo>,
) -> Result<Box<dyn aeon_memory_core::seed::runtime::SeedRuntime>, AeonMemoryCoreError> {
    std::fs::create_dir_all(data_dir)?;
    let candidate_store = SqliteCoreStore::new(VectorStore::new(
        &data_dir.join("vectors.db").to_string_lossy(),
        embedding.dimensions(),
    ));
    let store = match candidate_store.initialize(provider) {
        Ok(_) if candidate_store.with_store(|store| Ok(!store.is_degraded()))? => {
            Some(candidate_store)
        }
        Ok(_) | Err(AeonMemoryCoreError::Store(_)) => None,
        Err(error) => return Err(error),
    };
    if store.is_some() {
        ensure_sqlite_manifest(data_dir);
    }
    let pc = &config.memory.pipeline;
    let l1_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_config.clone(),
        scoped_model(config.memory.extraction.model.as_deref()),
        true,
    ));
    let scene_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_config.clone(),
        scoped_model(config.memory.extraction.model.as_deref()),
        true,
    ));
    let persona_llm: Arc<dyn LlmRunner> = Arc::new(OpenAiLlmRunner::new(
        llm_config,
        scoped_model(config.memory.persona.model.as_deref()),
        true,
    ));
    let l1_running = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let runner = ProductionPipelineRunner {
        store: store.clone(),
        data_dir: data_dir.to_path_buf(),
        l1_llm,
        scene_llm,
        persona_llm,
        embedding: store.as_ref().map_or_else(
            || Arc::new(NoopEmbeddingService::new()) as Arc<dyn EmbeddingService>,
            |_| Arc::clone(&embedding),
        ),
        embedding_provider: store.as_ref().and(provider.cloned()),
        l1_running: Arc::clone(&l1_running),
        l1_options: L1ExtractionOptions {
            enable_dedup: config.memory.extraction.enable_dedup,
            max_memories_per_session: config.memory.extraction.max_memories_per_session as usize,
            conflict_recall_top_k: config.memory.embedding.conflict_recall_top_k,
        },
        max_scenes: config.memory.persona.max_scenes as usize,
        persona_backup_count: config.memory.persona.backup_count as usize,
        scene_backup_count: config.memory.persona.scene_backup_count as usize,
        persona_trigger_every_n: u64::from(config.memory.persona.trigger_every_n),
    };
    let mut manager = PipelineManager::new(
        PipelineConfig {
            every_n_conversations: pc.every_n_conversations,
            enable_warmup: pc.enable_warmup,
            l1_idle_timeout_ms: i64::from(pc.l1_idle_timeout_seconds) * 1000,
            l2_delay_after_l1_ms: i64::from(pc.l2_delay_after_l1_seconds) * 1000,
            l2_min_interval_ms: i64::from(pc.l2_min_interval_seconds) * 1000,
            l2_max_interval_ms: i64::from(pc.l2_max_interval_seconds) * 1000,
            session_active_window_ms: i64::from(pc.session_active_window_hours) * 3_600_000,
        },
        Box::new(SystemClock),
        Box::new(runner),
    )
    .with_persister(Box::new(CheckpointStatePersister {
        data_dir: data_dir.to_path_buf(),
    }));
    manager.start(Default::default());
    let runtime = Arc::new(PipelineCoreRuntime::new_with_l1_running(
        manager, l1_running,
    ));
    let recorder = Arc::new(LocalCaptureRecorder {
        data_dir: data_dir.to_string_lossy().into_owned(),
    });
    let live_embedding = config
        .memory
        .embedding
        .enabled
        .then(|| Arc::clone(&embedding));
    let capture = Arc::new(AutoCapture::new(
        recorder,
        store.as_ref().map(|store| {
            Arc::new(store.clone()) as Arc<dyn aeon_memory_core::hooks::auto_capture::CaptureStore>
        }),
        live_embedding.filter(|_| store.is_some()),
        config.memory.extraction.enabled.then(|| {
            runtime.clone() as Arc<dyn aeon_memory_core::hooks::auto_capture::CaptureScheduler>
        }),
    ));
    Ok(Box::new(ProductionSeedRuntime::new(
        capture, runtime, store,
    )))
}

fn ensure_sqlite_manifest(data_dir: &std::path::Path) {
    let base = data_dir.to_string_lossy();
    if read_manifest(&base).is_none() {
        let _ = write_manifest(
            &base,
            &Manifest::new(ManifestStoreInfo {
                r#type: "sqlite".into(),
                sqlite: Some(ManifestSqliteInfo {
                    path: "vectors.db".into(),
                }),
                tcvdb: None,
            }),
        );
    }
}

fn offload_config(config: &GatewayConfig) -> OffloadConfig {
    let source = &config.memory.offload;
    OffloadConfig {
        enabled: source.enabled,
        force_trigger_threshold: source.force_trigger_threshold as usize,
        default_context_window: source.default_context_window as usize,
        max_pairs_per_batch: source.max_pairs_per_batch as usize,
        l2_null_threshold: source.l2_null_threshold as usize,
        l2_timeout_seconds: source.l2_timeout_seconds as u64,
        mild_offload_ratio: source.mild_offload_ratio,
        aggressive_compress_ratio: source.aggressive_compress_ratio,
        mmd_max_token_ratio: source.mmd_max_token_ratio,
        retention_days: source.offload_retention_days as u64,
        log_max_size_mb: source.log_max_size_mb as u64,
        ..Default::default()
    }
}

struct BackendOffloadRunner {
    base_url: String,
    api_key: Option<String>,
    user_id: Option<String>,
    timeout_ms: u64,
}

impl BackendOffloadRunner {
    async fn post(&self, path: &str, body: Value) -> Result<Value, AeonMemoryCoreError> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let api_key = self.api_key.clone();
        let user_id = self.user_id.clone();
        let timeout = std::time::Duration::from_millis(self.timeout_ms);
        tokio::task::spawn_blocking(move || {
            let agent = ureq::AgentBuilder::new().timeout(timeout).build();
            let mut request = agent.post(&url).set("Content-Type", "application/json");
            if let Some(key) = api_key.as_deref().filter(|key| !key.is_empty()) {
                request = request.set("Authorization", &format!("Bearer {key}"));
            }
            if let Some(user_id) = user_id.as_deref().filter(|id| !id.is_empty()) {
                request = request.set("X-User-Id", user_id);
            }
            let response = request
                .send_string(&body.to_string())
                .map_err(|error| AeonMemoryCoreError::Http(error.to_string()))?;
            let raw = response
                .into_string()
                .map_err(|error| AeonMemoryCoreError::Http(error.to_string()))?;
            serde_json::from_str(&raw).map_err(Into::into)
        })
        .await
        .map_err(|error| AeonMemoryCoreError::Http(error.to_string()))?
    }
}

#[async_trait]
impl LlmRunner for BackendOffloadRunner {
    async fn run(&self, _params: LlmRunParams) -> Result<String, AeonMemoryCoreError> {
        Err(AeonMemoryCoreError::InvalidInput(
            "backend offload runner only accepts structured offload calls".into(),
        ))
    }

    async fn run_offload_l1(
        &self,
        _params: LlmRunParams,
        recent_messages: &str,
        tool_pairs: &[ToolPair],
    ) -> Result<String, AeonMemoryCoreError> {
        let pairs = tool_pairs
            .iter()
            .map(|pair| {
                json!({
                    "toolName": pair.tool_name,
                    "toolCallId": pair.tool_call_id,
                    "params": pair.params,
                    "result": pair.result,
                    "timestamp": pair.timestamp,
                })
            })
            .collect::<Vec<_>>();
        let response = self
            .post(
                "/offload/v1/l1/summarize",
                json!({"recentMessages": recent_messages, "toolPairs": pairs}),
            )
            .await?;
        serde_json::to_string(response.get("entries").unwrap_or(&Value::Null)).map_err(Into::into)
    }

    async fn run_offload_l15(
        &self,
        _params: LlmRunParams,
        recent_messages: &str,
        current_mmd: Option<(&str, &str, &str)>,
        available_mmd_metas: &[MmdMeta],
    ) -> Result<String, AeonMemoryCoreError> {
        let current = current_mmd.map(|(filename, content, path)| {
            json!({"filename": filename, "content": content, "path": path})
        });
        let response = self
            .post(
                "/offload/v1/l15/judge",
                json!({
                    "recentMessages": recent_messages,
                    "currentMmd": current,
                    "availableMmdMetas": available_mmd_metas,
                }),
            )
            .await?;
        serde_json::to_string(&response).map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_offload_l2(
        &self,
        _params: LlmRunParams,
        existing_mmd: Option<&str>,
        new_entries: &[OffloadEntry],
        recent_history: Option<&str>,
        current_turn: Option<&str>,
        task_label: &str,
        mmd_prefix: &str,
        mmd_char_count: usize,
    ) -> Result<String, AeonMemoryCoreError> {
        let entries = new_entries
            .iter()
            .map(|entry| {
                json!({
                    "tool_call_id": entry.tool_call_id,
                    "tool_call": entry.tool_call,
                    "summary": entry.summary,
                    "timestamp": entry.timestamp,
                })
            })
            .collect::<Vec<_>>();
        let response = self
            .post(
                "/offload/v1/l2/generate",
                json!({
                    "existingMmd": existing_mmd,
                    "newEntries": entries,
                    "recentHistory": recent_history,
                    "currentTurn": current_turn,
                    "taskLabel": task_label,
                    "mmdPrefix": mmd_prefix,
                    "mmdCharCount": mmd_char_count,
                }),
            )
            .await?;
        let normalized = json!({
            "file_action": response.get("fileAction").or_else(|| response.get("file_action")),
            "mmd_content": response.get("mmdContent").or_else(|| response.get("mmd_content")),
            "replace_blocks": response.get("replaceBlocks").or_else(|| response.get("replace_blocks")),
            "node_mapping": response.get("nodeMapping").or_else(|| response.get("node_mapping")),
        });
        serde_json::to_string(&normalized).map_err(Into::into)
    }
}

pub struct EngineOffloadOperations {
    root: PathBuf,
    enabled: bool,
    llm: Option<Arc<dyn LlmRunner>>,
    collect_mode: bool,
    config: OffloadConfig,
    engines: Mutex<HashMap<String, OffloadEngine>>,
    reclaim_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl EngineOffloadOperations {
    pub fn new(
        root: PathBuf,
        enabled: bool,
        llm: Arc<dyn LlmRunner>,
        config: OffloadConfig,
    ) -> Self {
        Self::new_configured(root, enabled, Some(llm), false, config)
    }

    pub fn new_configured(
        root: PathBuf,
        enabled: bool,
        llm: Option<Arc<dyn LlmRunner>>,
        collect_mode: bool,
        config: OffloadConfig,
    ) -> Self {
        Self {
            root,
            enabled,
            llm,
            collect_mode,
            config,
            engines: Mutex::new(HashMap::new()),
            reclaim_task: Mutex::new(None),
        }
    }

    /// Start the TS-compatible reclaim cadence. Durations are arguments so the
    /// production 5m/24h schedule can be exercised with real short timers in
    /// deterministic tests.
    pub fn start_reclaim_scheduler(&self, initial_delay: Duration, interval: Duration) {
        if self.config.retention_days < 3 {
            return;
        }
        let Ok(mut slot) = self.reclaim_task.lock() else {
            return;
        };
        if let Some(previous) = slot.take() {
            previous.abort();
        }
        let root = self.root.clone();
        let config = ReclaimConfig {
            retention_days: self.config.retention_days,
            log_max_size_mb: self.config.log_max_size_mb,
        };
        *slot = Some(tokio::spawn(async move {
            tokio::time::sleep(initial_delay).await;
            loop {
                let reclaim_root = root.clone();
                let _ = tokio::task::spawn_blocking(move || reclaim(&reclaim_root, config)).await;
                tokio::time::sleep(interval).await;
            }
        }));
    }
    fn key(agent: &str, session: &str) -> String {
        format!("{agent}\0{session}")
    }
    fn take(&self, agent: &str, session: &str) -> ServiceResult<OffloadEngine> {
        if !self.enabled {
            return Err(ServiceError::InvalidInput("offload is disabled".into()));
        }
        let key = Self::key(agent, session);
        if let Some(engine) = self
            .engines
            .lock()
            .map_err(|_| ServiceError::Internal("offload lock poisoned".into()))?
            .remove(&key)
        {
            return Ok(engine);
        }
        OffloadEngine::load(
            StorageContext::new(&self.root, agent, session),
            self.config.clone(),
        )
        .map_err(core_error)
    }
    fn put(&self, agent: &str, session: &str, engine: OffloadEngine) -> ServiceResult<()> {
        self.engines
            .lock()
            .map_err(|_| ServiceError::Internal("offload lock poisoned".into()))?
            .insert(Self::key(agent, session), engine);
        Ok(())
    }
}
fn core_error(error: AeonMemoryCoreError) -> ServiceError {
    match error {
        AeonMemoryCoreError::InvalidInput(v) => ServiceError::InvalidInput(v),
        other => ServiceError::Internal(other.to_string()),
    }
}

fn prompt_hash(prompt: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    prompt.hash(&mut hash);
    hash.finish()
}

fn mmd_l15_context(
    engine: &OffloadEngine,
) -> (Option<(String, String, String)>, Vec<prompt::MmdMeta>) {
    let mut files = if engine.ctx.mmds_dir.exists() {
        fs::read_dir(&engine.ctx.mmds_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mmd"))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    files.sort_by_key(|entry| entry.file_name());
    let mut current = None;
    let metas = files
        .into_iter()
        .filter_map(|entry| {
            let filename = entry.file_name().into_string().ok()?;
            let path = entry.path();
            let content = fs::read_to_string(&path).ok()?;
            if engine.state.active_mmd_file.as_deref() == Some(filename.as_str()) {
                current = Some((
                    filename.clone(),
                    content.clone(),
                    path.to_string_lossy().into_owned(),
                ));
            }
            let metadata = content
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("%%{"))
                .and_then(|line| line.strip_suffix("}%%"))
                .and_then(|raw| {
                    serde_json::from_str::<serde_json::Value>(&format!("{{{raw}}}")).ok()
                });
            let task_goal = metadata
                .as_ref()
                .and_then(|value| value.get("taskGoal"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(prompt::MmdMeta {
                filename,
                path: path.to_string_lossy().into_owned(),
                task_goal,
                done_count: content.matches("done").count(),
                doing_count: content.matches("doing").count(),
                todo_count: content.matches("todo").count(),
                updated_time: entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(chrono::DateTime::<chrono::Utc>::from)
                    .map(|time| time.to_rfc3339()),
                node_summaries: Vec::new(),
            })
        })
        .collect();
    (current, metas)
}

fn apply_compression(
    engine: &OffloadEngine,
    messages: &mut Vec<serde_json::Value>,
    context_window: usize,
    system: &str,
) -> ServiceResult<serde_json::Value> {
    let entries = storage::read_entries(&engine.ctx).map_err(core_error)?;
    let confirmed = storage::confirmed_offload_ids(&entries);
    let deleted = storage::deleted_offload_ids(&entries);
    let fast_path = l3::fast_path_reapply(messages, &entries, &confirmed, &deleted);
    let current_nodes: HashSet<String> = entries
        .iter()
        .filter_map(|entry| entry.node_id.clone())
        .collect();
    let result = l3::compress(
        messages,
        &entries,
        &current_nodes,
        context_window,
        system,
        &engine.config,
        &O200kTokenizer,
    );
    let mut status_updates: HashMap<String, Value> = HashMap::new();
    for id in &result.replaced_tool_call_ids {
        status_updates.insert(id.clone(), Value::Bool(true));
    }
    for id in &result.deleted_tool_call_ids {
        status_updates.insert(id.clone(), Value::String("deleted".into()));
    }
    storage::mark_offload_status(&engine.ctx, &status_updates).map_err(core_error)?;
    let mode = match result.mode {
        l3::CompressionMode::None => "none",
        l3::CompressionMode::Mild => "mild",
        l3::CompressionMode::Aggressive => "aggressive",
        l3::CompressionMode::Emergency => "emergency",
    };
    Ok(json!({
        "applied": result.mode != l3::CompressionMode::None,
        "mode": mode,
        "replaced": result.replaced,
        "deleted": result.deleted,
        "tokensBefore": result.tokens_before,
        "tokensAfter": result.tokens_after,
        "tokensSaved": result.tokens_before.saturating_sub(result.tokens_after),
        "fastReplaced": fast_path.applied,
        "fastDeleted": fast_path.deleted,
    }))
}

#[async_trait]
impl OffloadOperations for EngineOffloadOperations {
    async fn before_prompt(
        &self,
        request: BeforePromptRequest,
    ) -> ServiceResult<BeforePromptResponse> {
        let mut engine = self.take(&request.agent_id, &request.session_id)?;
        let mut messages = request.messages;
        let recent = serde_json::to_string(&messages).unwrap_or_default();
        let l1_entries = match self.llm.as_deref() {
            Some(llm) => match engine.flush_l1(llm, &recent, true).await {
                Ok(entries) => entries,
                Err(error) => {
                    self.put(&request.agent_id, &request.session_id, engine)?;
                    return Err(core_error(error));
                }
            },
            None => Vec::new(),
        };
        let hash = prompt_hash(&request.user_prompt);
        let l15_judgment = if self.llm.is_some()
            && !request.user_prompt.trim().is_empty()
            && engine.state.last_l15_prompt_hash != Some(hash)
        {
            engine.state.last_l15_prompt_hash = Some(hash);
            let (current, metas) = mmd_l15_context(&engine);
            let current = current
                .as_ref()
                .map(|(file, content, path)| (file.as_str(), content.as_str(), path.as_str()));
            engine
                .judge_l15_with_retry(self.llm.as_deref().unwrap(), &recent, current, &metas)
                .await
                .ok()
        } else {
            None
        };
        let l2_updated = match self.llm.as_deref() {
            Some(llm) => match engine
                .run_l2(
                    llm,
                    Some(&recent),
                    Some(&request.user_prompt),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(updated) => updated > 0,
                Err(error) => {
                    self.put(&request.agent_id, &request.session_id, engine)?;
                    return Err(core_error(error));
                }
            },
            None => false,
        };
        let compression = if self.collect_mode {
            json!({"applied": false, "mode": "collect", "replaced": 0, "deleted": 0})
        } else {
            match apply_compression(
                &engine,
                &mut messages,
                request.context_window,
                &request.system_prompt,
            ) {
                Ok(compression) => compression,
                Err(error) => {
                    self.put(&request.agent_id, &request.session_id, engine)?;
                    return Err(error);
                }
            }
        };
        if !self.collect_mode {
            inject::inject_active(
                &mut messages,
                &engine.ctx,
                engine.state.active_mmd_file.as_deref(),
                &O200kTokenizer,
                request.context_window,
                engine.config.mmd_max_token_ratio,
            )
            .map_err(core_error)?;
        }
        let context = serde_json::to_value(snapshot(
            "before-prompt",
            &messages,
            Some(&request.system_prompt),
            Some(&request.user_prompt),
        ))
        .map_err(|e| ServiceError::Internal(e.to_string()))?;
        let active_mmd = engine.state.active_mmd_file.clone();
        self.put(&request.agent_id, &request.session_id, engine)?;
        Ok(BeforePromptResponse {
            messages,
            context,
            active_mmd,
            offload_enabled: true,
            l1_entries,
            l15_judgment,
            l2_updated,
            compression,
        })
    }
    async fn after_tool(&self, mut request: AfterToolRequest) -> ServiceResult<AfterToolResponse> {
        let mut engine = self.take(&request.agent_id, &request.session_id)?;
        engine.buffer_persisted(request.tool).map_err(core_error)?;
        let recent = serde_json::to_string(&request.messages).unwrap_or_default();
        let entries = if let Some(llm) = self.llm.as_deref() {
            engine
                .flush_l1(llm, &recent, false)
                .await
                .map_err(core_error)?
        } else {
            Vec::new()
        };
        let updated = if let Some(llm) = self.llm.as_deref() {
            engine
                .run_l2(llm, Some(&recent), None, chrono::Utc::now())
                .await
                .map_err(core_error)?
        } else {
            0
        };
        if !self.collect_mode {
            inject::inject_active(
                &mut request.messages,
                &engine.ctx,
                engine.state.active_mmd_file.as_deref(),
                &O200kTokenizer,
                request.context_window,
                engine.config.mmd_max_token_ratio,
            )
            .map_err(core_error)?;
        }
        let compression = if self.collect_mode {
            json!({"applied": false, "mode": "collect", "replaced": 0, "deleted": 0})
        } else {
            apply_compression(&engine, &mut request.messages, request.context_window, "")?
        };
        let buffered_pairs = engine.pending_count();
        let context = serde_json::to_value(snapshot("after-tool", &request.messages, None, None))
            .map_err(|e| ServiceError::Internal(e.to_string()))?;
        self.put(&request.agent_id, &request.session_id, engine)?;
        Ok(AfterToolResponse {
            messages: request.messages,
            buffered_pairs,
            l1_entries: entries,
            l2_updated: updated > 0,
            context,
            compression,
        })
    }
    async fn llm_output(&self, request: LlmOutputRequest) -> ServiceResult<LlmOutputResponse> {
        // The pinned TS llm_output hook is observational only: it reports the
        // pending count to debug logs and explicitly defers L1/L2 work to the
        // next input/after-tool boundary. Provider usage is accepted by the
        // host-neutral DTO but is not persisted by the TS implementation.
        let engine = self.take(&request.agent_id, &request.session_id)?;
        let state = engine.state.clone();
        self.put(&request.agent_id, &request.session_id, engine)?;
        Ok(LlmOutputResponse {
            force_l1: false,
            processed_entries: 0,
            l1_entries: Vec::new(),
            l2_updated: false,
            state,
        })
    }

    async fn shutdown(&self) -> ServiceResult<()> {
        let task = self
            .reclaim_task
            .lock()
            .map_err(|_| ServiceError::Internal("reclaim task lock poisoned".into()))?
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod offload_config_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn oracle() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/offload_production_oracle.json"
        ))
        .unwrap()
    }

    fn runtime_oracle() -> Value {
        serde_json::from_str(include_str!(
            "../../aeon-memory-core/tests/fixtures/offload_runtime_oracle.json"
        ))
        .unwrap()
    }

    fn mock_once(response_body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8(bytes).unwrap()).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });
        (format!("http://{address}"), rx)
    }

    fn params() -> LlmRunParams {
        LlmRunParams {
            prompt: "prompt".into(),
            system_prompt: Some("system".into()),
            task_id: "test".into(),
            timeout_ms: Some(1_000),
            max_tokens: None,
            workspace_dir: None,
            file_tool_policy: None,
            instance_id: None,
        }
    }

    #[tokio::test]
    async fn backend_mode_uses_backend_protocol_auth_and_user_identity() {
        let (base_url, requests) = mock_once(
            r#"{"entries":[{"tool_call_id":"call-1","tool_call":"read({})","summary":"ok","timestamp":"T","score":5}]}"#,
        );
        let runner = BackendOffloadRunner {
            base_url,
            api_key: Some("backend-secret".into()),
            user_id: Some("user-42".into()),
            timeout_ms: 2_345,
        };
        let raw = runner
            .run_offload_l1(
                params(),
                "recent",
                &[ToolPair {
                    tool_name: "read".into(),
                    tool_call_id: "call-1".into(),
                    params: json!({"path":"a"}),
                    result: json!({"content":"b"}),
                    error: None,
                    timestamp: "T".into(),
                    duration_ms: None,
                }],
            )
            .await
            .unwrap();
        assert!(raw.starts_with('['));
        let request = requests.recv().unwrap();
        let oracle = oracle();
        let path = oracle["backend"]["l1Path"].as_str().unwrap();
        assert!(request.starts_with(&format!("POST {path} HTTP/1.1")));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer backend-secret")
        );
        assert!(request.to_ascii_lowercase().contains("x-user-id: user-42"));
        assert!(request.contains(r#""recentMessages":"recent""#));
        assert!(request.contains(r#""toolCallId":"call-1""#));
    }

    #[tokio::test]
    async fn backend_mode_never_falls_through_to_main_llm_endpoint() {
        let (_main_url, main_requests) =
            mock_once(r#"{"choices":[{"message":{"content":"wrong"}}]}"#);
        let (backend_url, backend_requests) = mock_once(
            r#"{"entries":[{"tool_call_id":"call-b","tool_call":"read({})","summary":"backend","timestamp":"T","score":5}]}"#,
        );
        let runner = BackendOffloadRunner {
            base_url: backend_url,
            api_key: None,
            user_id: None,
            timeout_ms: 2_000,
        };
        runner
            .run_offload_l1(
                params(),
                "recent",
                &[ToolPair {
                    tool_name: "read".into(),
                    tool_call_id: "call-b".into(),
                    params: json!({}),
                    result: json!({}),
                    error: None,
                    timestamp: "T".into(),
                    duration_ms: None,
                }],
            )
            .await
            .unwrap();
        assert!(
            backend_requests
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .contains("/offload/v1/l1/summarize")
        );
        assert!(
            main_requests
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
    }

    #[tokio::test]
    async fn backend_l15_and_l2_use_structured_protocol_and_normalize_responses() {
        let (l15_url, l15_requests) = mock_once(
            r#"{"taskCompleted":false,"isContinuation":false,"isLongTask":true,"continuationMmdFile":null,"newTaskLabel":"task"}"#,
        );
        let l15 = BackendOffloadRunner {
            base_url: l15_url,
            api_key: None,
            user_id: None,
            timeout_ms: 2_000,
        };
        let raw = l15
            .run_offload_l15(params(), "recent", None, &[])
            .await
            .unwrap();
        assert!(
            aeon_memory_core::offload::parser::parse_l15(&raw)
                .unwrap()
                .is_long_task
        );
        assert!(
            l15_requests
                .recv()
                .unwrap()
                .contains("/offload/v1/l15/judge")
        );

        let (l2_url, l2_requests) = mock_once(
            r#"{"fileAction":"write","mmdContent":"flowchart TD\n  001-N1[done]","nodeMapping":{"call-1":"001-N1"}}"#,
        );
        let l2 = BackendOffloadRunner {
            base_url: l2_url,
            api_key: None,
            user_id: None,
            timeout_ms: 2_000,
        };
        let raw = l2
            .run_offload_l2(
                params(),
                None,
                &[OffloadEntry {
                    timestamp: "T".into(),
                    node_id: None,
                    tool_call: "read({})".into(),
                    summary: "done".into(),
                    result_ref: "ref".into(),
                    tool_call_id: "call-1".into(),
                    session_key: None,
                    score: Some(5.0),
                    offloaded: None,
                }],
                Some("history"),
                Some("turn"),
                "task",
                "001",
                0,
            )
            .await
            .unwrap();
        let parsed = aeon_memory_core::offload::parser::parse_l2(&raw).unwrap();
        assert_eq!(parsed.node_mapping["call-1"], "001-N1");
        let request = l2_requests.recv().unwrap();
        assert!(request.contains("/offload/v1/l2/generate"));
        assert!(request.contains(r#""taskLabel":"task""#));
    }

    #[tokio::test]
    async fn local_mode_uses_independent_model_temperature_and_thinking_policy() {
        let (base_url, requests) = mock_once(r#"{"choices":[{"message":{"content":"[]"}}]}"#);
        let runner = OpenAiLlmRunner::new(
            StandaloneLlmConfig {
                base_url,
                api_key: "main-key".into(),
                model: "offload-model".into(),
                max_tokens: 4096,
                timeout_ms: 2_000,
                disable_thinking: Some("deepseek".into()),
            },
            None,
            false,
        )
        .with_temperature(0.73);
        runner.run(params()).await.unwrap();
        let request = requests.recv().unwrap();
        assert!(request.contains(r#""model":"offload-model""#));
        assert!(request.contains(r#""temperature":0.73"#));
        assert!(request.contains(r#""enable_thinking":false"#));
        assert_eq!(oracle()["defaults"]["temperature"], 0.2);
    }

    #[tokio::test]
    async fn collect_mode_preserves_collection_in_custom_data_dir_without_mutation() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-offload-collect-custom-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let operations = EngineOffloadOperations::new_configured(
            root.clone(),
            true,
            None,
            true,
            OffloadConfig::default(),
        );
        let messages = vec![json!({"role":"assistant","content":"keep me"})];
        let response = operations
            .after_tool(AfterToolRequest {
                agent_id: "agent-a".into(),
                session_id: "session-a".into(),
                tool: ToolPair {
                    tool_name: "read".into(),
                    tool_call_id: "collect-1".into(),
                    params: json!({"path":"x"}),
                    result: json!({"content":"y"}),
                    error: None,
                    timestamp: "2026-07-13T00:00:00Z".into(),
                    duration_ms: None,
                },
                messages: messages.clone(),
                context_window: 100,
            })
            .await
            .unwrap();
        assert_eq!(response.messages, messages);
        assert_eq!(response.buffered_pairs, 1);
        assert_eq!(response.compression["mode"], "collect");
        assert_eq!(oracle()["defaults"]["collectCompressionEnabled"], false);
        assert!(
            !root.join("agent-a").join("pending-session-a.json").exists(),
            "TS pending pairs are process-local and must not be checkpointed"
        );
        assert!(!root.exists() || !root.join("agent-a").join("pending-session-a.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reclaim_scheduler_uses_injected_real_delays_and_shutdown_cancels() {
        assert_eq!(runtime_oracle()["reclaimInitialDelayMs"], 300_000);
        assert_eq!(runtime_oracle()["reclaimIntervalMs"], 86_400_000);
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-offload-reclaim-scheduler-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("debug.log");
        std::fs::write(&log, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let operations = EngineOffloadOperations::new_configured(
            root.clone(),
            true,
            None,
            false,
            OffloadConfig {
                retention_days: 3,
                log_max_size_mb: 1,
                ..Default::default()
            },
        );
        operations.start_reclaim_scheduler(Duration::from_millis(20), Duration::from_millis(20));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::metadata(&log).unwrap().len() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        operations.shutdown().await.unwrap();

        std::fs::write(&log, vec![b'y'; 1024 * 1024 + 1]).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 1024 * 1024 + 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn llm_output_is_observational_and_matches_pinned_ts_oracle() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-offload-llm-output-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let operations = EngineOffloadOperations::new_configured(
            root.clone(),
            true,
            None,
            false,
            OffloadConfig::default(),
        );
        let mut engine = OffloadEngine::load(
            StorageContext::new(&root, "main", "s1"),
            OffloadConfig::default(),
        )
        .unwrap();
        assert!(engine.buffer(ToolPair {
            tool_name: "read".into(),
            tool_call_id: "pending-1".into(),
            params: json!({}),
            result: json!("raw"),
            error: None,
            timestamp: "2026-07-13T00:00:00Z".into(),
            duration_ms: None,
        }));
        operations.put("main", "s1", engine).unwrap();
        let response = operations
            .llm_output(LlmOutputRequest {
                agent_id: "main".into(),
                session_id: "s1".into(),
                assistant_message: json!({"role":"assistant","content":"done"}),
                usage: Some(json!({"input_tokens":1234,"output_tokens":80})),
                finish_reason: Some("tool_use".into()),
            })
            .await
            .unwrap();
        assert!(!response.force_l1);
        assert_eq!(response.force_l1, runtime_oracle()["llmOutputForcesL1"]);
        assert_eq!(response.processed_entries, 0);
        assert!(response.l1_entries.is_empty());
        assert!(!response.l2_updated);
        assert_eq!(response.l2_updated, runtime_oracle()["llmOutputRunsL2"]);
        assert_eq!(
            operations.take("main", "s1").unwrap().pending_count(),
            runtime_oracle()["pendingAfterLlmOutput"].as_u64().unwrap() as usize
        );
        assert!(!root.join("main").join("offload-s1.jsonl").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn memory_cleaner_shutdown_cancels_and_awaits_the_background_task() {
        let root = std::env::temp_dir().join(format!(
            "aeon-memory-cleaner-shutdown-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cleaner = spawn_memory_cleaner(None, root.clone(), 3, "23:59".into(), "UTC".into());
        tokio::time::timeout(Duration::from_millis(250), cleaner.shutdown())
            .await
            .expect("cleaner shutdown must interrupt the daily sleep")
            .unwrap();
        assert!(cleaner.stop.lock().unwrap().is_none());
        assert!(cleaner.task.lock().unwrap().is_none());
        // Repeated shutdown is idempotent and cannot resurrect the old task.
        cleaner.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
