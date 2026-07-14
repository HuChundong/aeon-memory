//! Host-neutral port of `src/core/hooks/auto-capture.ts`.

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::pipeline::checkpoint::checkpoint_transaction;
use crate::record::l0_recorder::{
    ConversationMessage, RecordConversationParams, record_conversation,
};
use crate::types::{CaptureResult, EmbeddingService, FilteredMessage, L0Record};
use crate::utils::time::now_instant_iso;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

static RECORD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[allow(clippy::too_many_arguments)]
pub trait CaptureRecorder: Send + Sync {
    fn capture(
        &self,
        messages: &[serde_json::Value],
        session_key: &str,
        session_id: Option<&str>,
        original_user_text: Option<&str>,
        original_user_message_count: Option<u32>,
        plugin_start_timestamp: Option<i64>,
        skip_cursor: bool,
    ) -> AeonMemoryResult<Vec<ConversationMessage>>;
}

/// File-backed L0 recorder. Cursor read, JSONL append and cursor advance share
/// one critical section, matching CheckpointManager.captureAtomically in TS.
pub struct LocalCaptureRecorder {
    pub data_dir: String,
}

impl CaptureRecorder for LocalCaptureRecorder {
    fn capture(
        &self,
        messages: &[serde_json::Value],
        session_key: &str,
        session_id: Option<&str>,
        original_user_text: Option<&str>,
        original_user_message_count: Option<u32>,
        plugin_start_timestamp: Option<i64>,
        skip_cursor: bool,
    ) -> AeonMemoryResult<Vec<ConversationMessage>> {
        checkpoint_transaction(&self.data_dir, |checkpoint| {
            let state = checkpoint
                .runner_states
                .entry(session_key.to_owned())
                .or_default();
            let cursor = if skip_cursor {
                0
            } else if state.last_captured_timestamp > 0 {
                state.last_captured_timestamp
            } else {
                plugin_start_timestamp.unwrap_or(0)
            };
            let filtered = record_conversation(RecordConversationParams {
                session_key,
                session_id,
                raw_messages: messages,
                base_dir: &self.data_dir,
                original_user_text,
                after_timestamp: Some(cursor),
                original_user_message_count,
            })?;
            let max_timestamp = filtered.iter().map(|message| message.timestamp).max();
            if let Some(max) = max_timestamp {
                state.last_captured_timestamp = max;
                checkpoint.last_captured_timestamp = checkpoint.last_captured_timestamp.max(max);
                checkpoint.total_processed += filtered.len() as u64;
                checkpoint.l0_conversations_count += 1;
            }
            Ok((filtered, max_timestamp.is_some()))
        })
    }
}

pub trait CaptureStore: Send + Sync {
    fn supports_deferred_embedding(&self) -> bool;
    fn upsert_l0(&self, record: &L0Record, embedding: Option<&[f32]>) -> AeonMemoryResult<bool>;
    fn update_l0_embedding(&self, record_id: &str, embedding: &[f32]) -> AeonMemoryResult<bool>;
}

pub trait CaptureScheduler: Send + Sync {
    fn notify_conversation(
        &self,
        session_key: &str,
        messages: &[ConversationMessage],
    ) -> AeonMemoryResult<()>;
}

pub struct AutoCapture {
    recorder: Arc<dyn CaptureRecorder>,
    store: Option<Arc<dyn CaptureStore>>,
    embedding: Option<Arc<dyn EmbeddingService>>,
    scheduler: Option<Arc<dyn CaptureScheduler>>,
    background: Mutex<Vec<JoinHandle<AeonMemoryResult<()>>>>,
}

impl AutoCapture {
    pub fn new(
        recorder: Arc<dyn CaptureRecorder>,
        store: Option<Arc<dyn CaptureStore>>,
        embedding: Option<Arc<dyn EmbeddingService>>,
        scheduler: Option<Arc<dyn CaptureScheduler>>,
    ) -> Self {
        Self {
            recorder,
            store,
            embedding,
            scheduler,
            background: Mutex::new(Vec::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn perform(
        &self,
        messages: &[serde_json::Value],
        session_key: &str,
        session_id: Option<&str>,
        original_user_text: Option<&str>,
        original_user_message_count: Option<u32>,
        plugin_start_timestamp: Option<i64>,
        skip_cursor: bool,
    ) -> AeonMemoryResult<CaptureResult> {
        let filtered = self.recorder.capture(
            messages,
            session_key,
            session_id,
            original_user_text,
            original_user_message_count,
            plugin_start_timestamp,
            skip_cursor,
        )?;
        let mut written = 0u32;
        let mut deferred = Vec::new();
        if let Some(store) = &self.store {
            let supports_deferred = store.supports_deferred_embedding();
            // The TypeScript runtime assigns one recorded_at value to the
            // whole captured turn. Besides wire compatibility, this preserves
            // its stable reverse-on-read ordering when timestamps tie.
            let recorded_at = now_instant_iso();
            for (index, message) in filtered.iter().enumerate() {
                let record = L0Record {
                    id: record_id(session_key, index),
                    session_key: session_key.to_owned(),
                    session_id: session_id.unwrap_or("").to_owned(),
                    role: message.role.clone(),
                    message_text: message.content.clone(),
                    recorded_at: recorded_at.clone(),
                    timestamp: message.timestamp,
                };
                let embedding = if supports_deferred {
                    None
                } else if let Some(service) = &self.embedding {
                    if service.dimensions() == 0 {
                        None
                    } else {
                        Some(service.embed(&message.content)?)
                    }
                } else {
                    None
                };
                if store.upsert_l0(&record, embedding.as_deref())? {
                    written += 1;
                    if supports_deferred {
                        deferred.push((record.id, message.content.clone()));
                    }
                }
            }
            if !deferred.is_empty()
                && let Some(service) = &self.embedding
            {
                let service = Arc::clone(service);
                let store = Arc::clone(store);
                let task = tokio::task::spawn_blocking(move || {
                    let result = (|| {
                        let texts = deferred
                            .iter()
                            .map(|(_, content)| content.clone())
                            .collect::<Vec<_>>();
                        let embeddings = service.embed_batch(&texts)?;
                        if embeddings.len() != deferred.len() {
                            return Err(AeonMemoryCoreError::Embedding(format!(
                                "embedding batch returned {} vectors for {} records",
                                embeddings.len(),
                                deferred.len()
                            )));
                        }
                        for ((id, _), vector) in deferred.iter().zip(embeddings.iter()) {
                            if !store.update_l0_embedding(id, vector)? {
                                return Err(AeonMemoryCoreError::Store(format!(
                                    "deferred embedding update rejected for {id}"
                                )));
                            }
                        }
                        Ok::<(), AeonMemoryCoreError>(())
                    })();
                    // Deferred embedding is a best-effort secondary index in
                    // TS. Metadata/FTS capture has already committed, so a
                    // provider failure must not fail shutdown.
                    if let Err(error) = result {
                        eprintln!(
                            "[aeon-memory] [capture] deferred embedding failed (non-fatal): {error}"
                        );
                    }
                    Ok(())
                });
                self.background
                    .lock()
                    .map_err(|_| AeonMemoryCoreError::Store("background registry poisoned".into()))?
                    .push(task);
            }
        }
        let scheduler_notified = if let Some(scheduler) = &self.scheduler {
            // Match the original runtime contract: capture persists L0 before
            // notifying, and the production L1 runner reloads the canonical
            // conversation from storage instead of consuming an in-memory
            // copy. Passing messages here would duplicate/diverge that input.
            scheduler.notify_conversation(session_key, &[])?;
            true
        } else {
            false
        };
        Ok(CaptureResult {
            l0_recorded_count: filtered.len() as u32,
            scheduler_notified,
            l0_vectors_written: written,
            filtered_messages: filtered
                .into_iter()
                .map(|message| FilteredMessage {
                    role: message.role,
                    content: message.content,
                    timestamp: message.timestamp,
                })
                .collect(),
        })
    }

    /// Drain all deferred embedding writes. Provider/update failures are
    /// handled inside each task and remain non-fatal, matching TS.
    pub async fn drain(&self) -> AeonMemoryResult<()> {
        let tasks = {
            let mut guard = self
                .background
                .lock()
                .map_err(|_| AeonMemoryCoreError::Store("background registry poisoned".into()))?;
            std::mem::take(&mut *guard)
        };
        for task in tasks {
            task.await.map_err(|error| {
                AeonMemoryCoreError::Store(format!("background task failed: {error}"))
            })??;
        }
        Ok(())
    }
}

fn record_id(session_key: &str, index: usize) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = RECORD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "l0_{session_key}_{millis}_{index}_{:06x}",
        sequence & 0xff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::l0_recorder::ConversationMessage;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            Ok(vec![
                ConversationMessage {
                    id: "a".into(),
                    role: "user".into(),
                    content: "alpha".into(),
                    timestamp: 1,
                },
                ConversationMessage {
                    id: "b".into(),
                    role: "assistant".into(),
                    content: "beta".into(),
                    timestamp: 2,
                },
            ])
        }
    }
    #[derive(Default)]
    struct Store {
        upserts: AtomicUsize,
        updates: AtomicUsize,
    }
    impl CaptureStore for Store {
        fn supports_deferred_embedding(&self) -> bool {
            true
        }
        fn upsert_l0(&self, _: &L0Record, embedding: Option<&[f32]>) -> AeonMemoryResult<bool> {
            assert!(embedding.is_none());
            self.upserts.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
        fn update_l0_embedding(&self, _: &str, embedding: &[f32]) -> AeonMemoryResult<bool> {
            assert_eq!(embedding, [1.0, 2.0]);
            self.updates.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }
    struct Embed;
    impl EmbeddingService for Embed {
        fn embed(&self, _: &str) -> AeonMemoryResult<Vec<f32>> {
            unreachable!()
        }
        fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 2.0]).collect())
        }
        fn dimensions(&self) -> u32 {
            2
        }
    }
    #[derive(Default)]
    struct Scheduler(AtomicUsize);
    impl CaptureScheduler for Scheduler {
        fn notify_conversation(
            &self,
            session_key: &str,
            messages: &[ConversationMessage],
        ) -> AeonMemoryResult<()> {
            assert_eq!(session_key, "session");
            assert!(
                messages.is_empty(),
                "pipeline notification must reload persisted L0"
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn capture_orders_metadata_then_deferred_vectors_and_notifies_once() {
        let store = Arc::new(Store::default());
        let scheduler = Arc::new(Scheduler::default());
        let hook = AutoCapture::new(
            Arc::new(Recorder),
            Some(store.clone()),
            Some(Arc::new(Embed)),
            Some(scheduler.clone()),
        );
        let result = hook
            .perform(&[], "session", Some("sid"), None, None, None, false)
            .await
            .unwrap();
        assert_eq!(result.l0_recorded_count, 2);
        assert_eq!(result.l0_vectors_written, 2);
        assert!(result.scheduler_notified);
        assert_eq!(store.upserts.load(Ordering::SeqCst), 2);
        assert_eq!(scheduler.0.load(Ordering::SeqCst), 1);
        hook.drain().await.unwrap();
        assert_eq!(store.updates.load(Ordering::SeqCst), 2);
    }

    struct BadEmbed;
    impl EmbeddingService for BadEmbed {
        fn embed(&self, _: &str) -> AeonMemoryResult<Vec<f32>> {
            unreachable!()
        }
        fn embed_batch(&self, _: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
            Ok(vec![])
        }
        fn dimensions(&self) -> u32 {
            2
        }
    }
    #[tokio::test]
    async fn deferred_failures_are_fail_soft_at_shutdown() {
        let hook = AutoCapture::new(
            Arc::new(Recorder),
            Some(Arc::new(Store::default())),
            Some(Arc::new(BadEmbed)),
            None,
        );
        hook.perform(&[], "session", None, None, None, None, false)
            .await
            .unwrap();
        hook.drain().await.unwrap();
    }

    // ── skip_cursor regression tests through LocalCaptureRecorder ──────────

    const SAME_MS: i64 = 1_234_567_890_000;
    const LATER_MS: i64 = 1_234_567_891_000;

    fn two_msgs(tag: &str, ts: i64) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"id": format!("{tag}_u"), "role":"user", "content": format!("{tag}_user"), "timestamp": ts}),
            serde_json::json!({"id": format!("{tag}_a"), "role":"assistant", "content": format!("{tag}_asst"), "timestamp": ts + 1}),
        ]
    }

    fn make_recorder(dir: &std::path::Path) -> LocalCaptureRecorder {
        LocalCaptureRecorder {
            data_dir: dir.to_string_lossy().into_owned(),
        }
    }

    /// A) Same session, two explicit-turn captures with identical millisecond
    ///    timestamps.  skip_cursor=true → both calls persist 2 rows (4 total).
    #[test]
    fn skip_cursor_local_recorder_true_preserves_both_turns() {
        let dir = std::env::temp_dir().join("aeon-memory-skip-true");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = make_recorder(&dir);

        let r1 = rec
            .capture(&two_msgs("a", SAME_MS), "s", None, None, None, None, true)
            .unwrap();
        assert_eq!(r1.len(), 2, "first explicit turn: 2 rows");

        let r2 = rec
            .capture(&two_msgs("b", SAME_MS), "s", None, None, None, None, true)
            .unwrap();
        assert_eq!(
            r2.len(),
            2,
            "second explicit turn with same ms: also 2 rows"
        );

        let stored =
            crate::record::l0_recorder::read_conversation_records("s", dir.to_str().unwrap())
                .unwrap();
        assert_eq!(stored.len(), 4, "JSONL must contain 4 rows, not 2");

        let cp = crate::pipeline::checkpoint::read_checkpoint(&dir.to_string_lossy()).unwrap();
        let state = cp.runner_states.get("s").unwrap();
        assert_eq!(
            state.last_captured_timestamp,
            SAME_MS + 1,
            "checkpoint cursor must be SAME_MS+1 (the max ms timestamp)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B) Same payload, skip_cursor=false → second call filters everything out.
    #[test]
    fn skip_cursor_local_recorder_false_dedup() {
        let dir = std::env::temp_dir().join("aeon-memory-skip-false");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = make_recorder(&dir);
        let msgs = two_msgs("x", SAME_MS);

        let r1 = rec
            .capture(&msgs, "s", None, None, None, None, false)
            .unwrap();
        assert_eq!(r1.len(), 2);

        let r2 = rec
            .capture(&msgs, "s", None, None, None, None, false)
            .unwrap();
        assert_eq!(
            r2.len(),
            0,
            "skip_cursor=false must dedup identical payload"
        );

        let stored =
            crate::record::l0_recorder::read_conversation_records("s", dir.to_str().unwrap())
                .unwrap();
        assert_eq!(
            stored.len(),
            2,
            "JSONL must still have only the first 2 rows"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C) First skip_cursor=true (old ms), then skip_cursor=false snapshot
    ///    containing old + newer-ms messages.  Only the newer two are
    ///    returned; checkpoint cursor advances to the later ms value.
    #[test]
    fn skip_cursor_true_then_false_mixed_ms() {
        let dir = std::env::temp_dir().join("aeon-memory-skip-mixed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = make_recorder(&dir);

        // Explicit turn (skip_cursor=true) with old ms
        let r1 = rec
            .capture(&two_msgs("t1", SAME_MS), "s", None, None, None, None, true)
            .unwrap();
        assert_eq!(r1.len(), 2);

        // Snapshot (skip_cursor=false) with old messages + 2 new with later ms
        let mut snapshot = two_msgs("t1", SAME_MS); // same ids → SQL upsert
        snapshot.extend(two_msgs("t2", LATER_MS));
        let r2 = rec
            .capture(&snapshot, "s", None, None, None, None, false)
            .unwrap();

        assert_eq!(
            r2.len(),
            2,
            "snapshot must capture only the 2 newer-ms messages"
        );
        assert_eq!(r2[0].content, "t2_user");
        assert_eq!(r2[1].content, "t2_asst");

        let cp = crate::pipeline::checkpoint::read_checkpoint(&dir.to_string_lossy()).unwrap();
        let state = cp.runner_states.get("s").unwrap();
        assert_eq!(
            state.last_captured_timestamp,
            LATER_MS + 1,
            "checkpoint cursor must advance to the newer ms value, not a nano-scale number"
        );
        assert!(
            state.last_captured_timestamp < SAME_MS + 10_000,
            "cursor must stay in millisecond range, no nanoleak"
        );

        let stored =
            crate::record::l0_recorder::read_conversation_records("s", dir.to_str().unwrap())
                .unwrap();
        assert_eq!(stored.len(), 4);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
