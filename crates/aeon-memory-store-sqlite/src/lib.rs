pub mod adapter;
pub mod connection;
pub mod l0;
pub mod l1;
pub mod schema;

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        path::{Path, PathBuf},
        sync::{
            LazyLock,
            atomic::{AtomicU64, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    static RUN_ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aeon-memory-store-sqlite-tests-{}-{started}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    });

    pub fn unique_dir(prefix: &str) -> PathBuf {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = RUN_ROOT.join(format!("{prefix}-{nonce}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn cleanup_db(path: impl AsRef<Path>) {
        let path = path.as_ref();
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

use connection::StoreConnection;

fn map_err(e: rusqlite::Error) -> AeonMemoryCoreError {
    AeonMemoryCoreError::Store(e.to_string())
}
use aeon_memory_core::AeonMemoryResult;
use aeon_memory_core::error::AeonMemoryCoreError;
use aeon_memory_core::types::*;
use std::sync::MutexGuard;

pub struct VectorStore {
    store: Option<StoreConnection>,
    db_path: String,
    dimensions: u32,
    degraded: bool,
    fts_available: bool,
    vec_available: bool,
}

impl VectorStore {
    pub fn new(db_path: &str, dimensions: u32) -> Self {
        Self {
            store: None,
            db_path: db_path.to_string(),
            dimensions,
            degraded: false,
            fts_available: false,
            vec_available: false,
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, rusqlite::Connection>, AeonMemoryCoreError> {
        Ok(self
            .store
            .as_ref()
            .ok_or_else(|| AeonMemoryCoreError::Store("store not initialized".to_string()))?
            .conn
            .lock()
            .unwrap())
    }

    pub fn claim_seed_key(&self, key: &str) -> AeonMemoryResult<bool> {
        let conn = self.lock()?;
        conn.execute("CREATE TABLE IF NOT EXISTS seed_idempotency (key TEXT PRIMARY KEY, created_at TEXT NOT NULL)", [])
            .map_err(map_err)?;
        Ok(conn.execute("INSERT OR IGNORE INTO seed_idempotency(key, created_at) VALUES (?1, datetime('now'))", [key]).map_err(map_err)? == 1)
    }

    pub fn release_seed_key(&self, key: &str) -> AeonMemoryResult<()> {
        self.lock()?
            .execute("DELETE FROM seed_idempotency WHERE key = ?1", [key])
            .map_err(map_err)?;
        Ok(())
    }

    fn init_inner(
        &mut self,
        provider_info: Option<&EmbeddingProviderInfo>,
        vec_available_override: Option<bool>,
    ) -> AeonMemoryResult<StoreInitResult> {
        let store = StoreConnection::open(&self.db_path)
            .map_err(|e| AeonMemoryCoreError::Store(format!("open DB failed: {}", e)))?;

        let schema_result = {
            let c = store.conn.lock().unwrap();

            if self.dimensions > 0 {
                self.vec_available =
                    vec_available_override.unwrap_or_else(|| schema::try_load_vec_extension(&c));
                if !self.vec_available {
                    self.degraded = true;
                    drop(c);
                    self.store = Some(store);
                    return Ok(StoreInitResult {
                        needs_reindex: false,
                        reason: Some("sqlite-vec load failed".into()),
                    });
                }
            }

            let schema_result = schema::initialize_schema(&c, provider_info, self.dimensions)
                .map_err(|e| AeonMemoryCoreError::Store(format!("schema init failed: {}", e)))?;

            if self.vec_available && self.dimensions > 0 {
                schema::create_vec_tables(&c, self.dimensions).map_err(|error| {
                    AeonMemoryCoreError::Store(format!("schema init failed: {error}"))
                })?;
            }

            if let Some(info) = provider_info {
                schema::write_embedding_meta(
                    &c,
                    &schema::EmbeddingMeta {
                        provider: info.provider.clone(),
                        model: info.model.clone(),
                        dimensions: self.dimensions,
                    },
                )
                .map_err(|e| {
                    AeonMemoryCoreError::Store(format!("embedding metadata write failed: {e}"))
                })?;
            }

            self.fts_available = schema_result.fts_available;
            self.degraded = !self.vec_available && self.dimensions > 0;
            schema_result
        };

        self.store = Some(store);

        Ok(StoreInitResult {
            needs_reindex: schema_result.needs_reindex,
            reason: schema_result.reason,
        })
    }
}

impl IMemoryStore for VectorStore {
    fn supports_deferred_embedding(&self) -> bool {
        true
    }

    fn init(
        &mut self,
        provider_info: Option<&EmbeddingProviderInfo>,
    ) -> AeonMemoryResult<StoreInitResult> {
        self.init_inner(provider_info, None)
    }

    fn is_degraded(&self) -> bool {
        self.degraded
    }

    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            vector_search: self.vec_available,
            fts_search: self.fts_available,
            native_hybrid_search: false,
            sparse_vectors: false,
        }
    }

    fn close(&mut self) {
        self.store = None;
    }

    // ── L1 ──

    fn upsert_l1(
        &mut self,
        record: &L1RecordRow,
        embedding: Option<&[f32]>,
    ) -> AeonMemoryResult<bool> {
        let c = self.lock()?;
        l1::upsert_l1(&c, record, embedding).map_err(map_err)
    }

    fn delete_l1(&mut self, record_id: &str) -> AeonMemoryResult<bool> {
        let c = self.lock()?;
        l1::delete_l1(&c, record_id).map_err(map_err)
    }

    fn count_l1(&self) -> AeonMemoryResult<i64> {
        let c = self.lock()?;
        l1::count_l1(&c).map_err(map_err)
    }
    fn delete_l1_expired(&mut self, cutoff_iso: &str) -> AeonMemoryResult<i64> {
        let c = self.lock()?;
        l1::delete_l1_expired(&c, cutoff_iso).map_err(map_err)
    }

    fn query_l1_records(&self, filter: &L1QueryFilter) -> AeonMemoryResult<Vec<L1RecordRow>> {
        let c = self.lock()?;
        l1::query_l1_records(&c, filter).map_err(map_err)
    }

    fn search_l1_fts(&self, fts_query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        if !self.fts_available {
            return Ok(Vec::new());
        }
        let c = self.lock()?;
        l1::search_l1_fts(&c, fts_query, limit).map_err(map_err)
    }

    fn search_l1_vector(
        &self,
        query_embedding: &[f32],
        top_k: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>> {
        if !self.vec_available {
            return Ok(Vec::new());
        }
        let c = self.lock()?;
        l1::search_l1_vector(&c, query_embedding, top_k).map_err(map_err)
    }

    fn upsert_l0(
        &mut self,
        record: &L0Record,
        embedding: Option<&[f32]>,
    ) -> AeonMemoryResult<bool> {
        let c = self.lock()?;
        l0::upsert_l0(&c, record, embedding).map_err(map_err)
    }

    fn delete_l0(&mut self, record_id: &str) -> AeonMemoryResult<bool> {
        let c = self.lock()?;
        l0::delete_l0(&c, record_id).map_err(map_err)
    }

    fn count_l0(&self) -> AeonMemoryResult<i64> {
        let c = self.lock()?;
        l0::count_l0(&c).map_err(map_err)
    }
    fn delete_l0_expired(&mut self, cutoff_iso: &str) -> AeonMemoryResult<i64> {
        let c = self.lock()?;
        l0::delete_l0_expired(&c, cutoff_iso).map_err(map_err)
    }

    fn query_l0_for_l1(
        &self,
        session_key: &str,
        after_recorded_at_ms: Option<i64>,
        limit: i64,
    ) -> AeonMemoryResult<Vec<L0QueryRow>> {
        let c = self.lock()?;
        match after_recorded_at_ms {
            Some(ms) => {
                let dt = chrono::DateTime::from_timestamp_millis(ms)
                    .ok_or_else(|| AeonMemoryCoreError::InvalidInput("invalid timestamp".into()))?;
                let cursor = dt.to_rfc3339();
                l0::query_l0_after(&c, session_key, &cursor, limit).map_err(map_err)
            }
            None => l0::query_l0_for_session(&c, session_key, limit).map_err(map_err),
        }
    }

    fn search_l0_vector(
        &self,
        query_embedding: &[f32],
        top_k: i64,
    ) -> AeonMemoryResult<Vec<L0SearchResult>> {
        if !self.vec_available {
            return Ok(Vec::new());
        }
        let c = self.lock()?;
        l0::search_l0_vector(&c, query_embedding, top_k).map_err(map_err)
    }

    fn reindex_all(
        &mut self,
        embed_fn: &mut dyn FnMut(&str) -> AeonMemoryResult<Vec<f32>>,
        mut on_progress: Option<&mut dyn FnMut(usize, usize, ReindexLayer)>,
    ) -> AeonMemoryResult<ReindexResult> {
        // TypeScript returns zero counts when vec0 is unavailable/degraded.
        if self.degraded || !self.vec_available {
            return Ok(ReindexResult::default());
        }

        let c = self.lock()?;
        let l1_rows = (|| -> Result<Vec<(String, String, String)>, rusqlite::Error> {
            let mut stmt = c.prepare("SELECT record_id, content, updated_time FROM l1_records")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })()
        .unwrap_or_default();

        let mut l1_done = 0;
        for (record_id, content, updated_time) in &l1_rows {
            if let Ok(embedding) = embed_fn(content) {
                let vector = serde_json::to_string(&embedding)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)));
                if let Ok(vector) = vector {
                    let write = (|| -> Result<(), rusqlite::Error> {
                        c.execute_batch("BEGIN")?;
                        let result = (|| {
                            c.execute("DELETE FROM l1_vec WHERE record_id = ?1", [record_id])?;
                            c.execute(
                                "INSERT INTO l1_vec (record_id, embedding, updated_time) VALUES (?1, ?2, ?3)",
                                rusqlite::params![record_id, vector, updated_time],
                            )?;
                            Ok(())
                        })();
                        match result {
                            Ok(()) => c.execute_batch("COMMIT"),
                            Err(error) => {
                                let _ = c.execute_batch("ROLLBACK");
                                Err(error)
                            }
                        }
                    })();
                    let _ = write;
                }
            }
            l1_done += 1;
            if let Some(callback) = on_progress.as_deref_mut() {
                callback(l1_done, l1_rows.len(), ReindexLayer::L1);
            }
        }

        let l0_rows = (|| -> Result<Vec<(String, String, String)>, rusqlite::Error> {
            let mut stmt =
                c.prepare("SELECT record_id, message_text, recorded_at FROM l0_conversations")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })()
        .unwrap_or_default();

        let mut l0_done = 0;
        for (record_id, message_text, recorded_at) in &l0_rows {
            if let Ok(embedding) = embed_fn(message_text) {
                let vector = serde_json::to_string(&embedding)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)));
                if let Ok(vector) = vector {
                    let write = (|| -> Result<(), rusqlite::Error> {
                        c.execute_batch("BEGIN")?;
                        let result = (|| {
                            c.execute("DELETE FROM l0_vec WHERE record_id = ?1", [record_id])?;
                            c.execute(
                                "INSERT INTO l0_vec (record_id, embedding, recorded_at) VALUES (?1, ?2, ?3)",
                                rusqlite::params![record_id, vector, recorded_at],
                            )?;
                            Ok(())
                        })();
                        match result {
                            Ok(()) => c.execute_batch("COMMIT"),
                            Err(error) => {
                                let _ = c.execute_batch("ROLLBACK");
                                Err(error)
                            }
                        }
                    })();
                    let _ = write;
                }
            }
            l0_done += 1;
            if let Some(callback) = on_progress.as_deref_mut() {
                callback(l0_done, l0_rows.len(), ReindexLayer::L0);
            }
        }

        Ok(ReindexResult {
            l1_count: l1_done,
            l0_count: l0_done,
        })
    }

    fn is_fts_available(&self) -> bool {
        self.fts_available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> VectorStore {
        let dir = crate::test_support::unique_dir("aeon-memory-test-store-trait");
        let path = dir.join(name).to_string_lossy().to_string();
        VectorStore::new(&path, 0)
    }

    #[test]
    fn test_trait_lifecycle() {
        let mut store = test_store("lifecycle.db");
        store.init(None).unwrap();
        assert!(store.is_fts_available());
        assert!(!store.capabilities().vector_search);
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn missing_vec_degrades_before_creating_partial_schema() {
        let mut store = test_store("missing_vec_degraded.db");
        store.dimensions = 3;
        let result = store
            .init_inner(Some(&provider("openai", "fixture")), Some(false))
            .unwrap();
        assert!(store.is_degraded());
        assert_eq!(result.reason.as_deref(), Some("sqlite-vec load failed"));
        let connection = store.store.as_ref().unwrap().conn.lock().unwrap();
        let schema_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name IN ('embedding_meta','l1_records','l0_conversations')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_tables, 0);
        drop(connection);
        store.close();
        cleanup(&store.db_path);
    }

    fn provider(provider: &str, model: &str) -> EmbeddingProviderInfo {
        EmbeddingProviderInfo {
            provider: provider.into(),
            model: model.into(),
        }
    }

    #[test]
    fn init_propagates_provider_model_and_dimension_drift() {
        let name = "embedding_drift.db";
        let mut store = test_store(name);
        store.dimensions = 4;
        let first = store.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(!first.needs_reindex);
        store.close();

        let mut stable = VectorStore::new(&store.db_path, 4);
        let stable_result = stable.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(!stable_result.needs_reindex);
        assert_eq!(stable_result.reason, None);
        stable.close();

        let mut changed = VectorStore::new(&store.db_path, 8);
        let changed_result = changed.init(Some(&provider("other", "model-b"))).unwrap();
        assert!(changed_result.needs_reindex);
        assert_eq!(
            changed_result.reason.as_deref(),
            Some("provider: mock → other, model: model-a → model-b, dimensions: 4 → 8")
        );
        changed.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn init_flags_legacy_database_with_records_and_no_embedding_meta() {
        let name = "legacy_reindex.db";
        let mut store = test_store(name);
        store.init(None).unwrap();
        store
            .upsert_l0(
                &L0Record {
                    id: "legacy-l0".into(),
                    session_key: "s".into(),
                    session_id: String::new(),
                    role: "user".into(),
                    message_text: "legacy".into(),
                    recorded_at: "2026-07-13T00:00:00Z".into(),
                    timestamp: 1,
                },
                None,
            )
            .unwrap();
        store.close();

        let mut upgraded = VectorStore::new(&store.db_path, 4);
        let result = upgraded.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(result.needs_reindex);
        assert_eq!(
            result.reason.as_deref(),
            Some("legacy DB without embedding_meta — cannot verify vector compatibility")
        );
        upgraded.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn reindex_all_runs_l1_then_l0_and_skips_individual_failures() {
        let name = "reindex_all.db";
        let mut store = test_store(name);
        store.dimensions = 4;
        store.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(
            !store.is_degraded(),
            "sqlite-vec fixture must load for this test"
        );

        for (id, content) in [("l1-a", "first"), ("l1-b", "fail")] {
            store
                .upsert_l1(
                    &L1RecordRow {
                        record_id: id.into(),
                        content: content.into(),
                        r#type: "fact".into(),
                        priority: 50.0,
                        scene_name: String::new(),
                        session_key: "s".into(),
                        session_id: String::new(),
                        timestamp_str: String::new(),
                        timestamp_start: String::new(),
                        timestamp_end: String::new(),
                        created_time: "2026-07-13T00:00:00Z".into(),
                        updated_time: "2026-07-13T00:00:01Z".into(),
                        metadata_json: "{}".into(),
                    },
                    None,
                )
                .unwrap();
        }
        for (id, text) in [("l0-a", "third"), ("l0-b", "fourth")] {
            store
                .upsert_l0(
                    &L0Record {
                        id: id.into(),
                        session_key: "s".into(),
                        session_id: String::new(),
                        role: "user".into(),
                        message_text: text.into(),
                        recorded_at: "2026-07-13T00:00:02Z".into(),
                        timestamp: 2,
                    },
                    None,
                )
                .unwrap();
        }

        let mut embedded = Vec::new();
        let mut embed = |text: &str| {
            embedded.push(text.to_string());
            if text == "fail" {
                Err(AeonMemoryCoreError::Store("mock embedding failure".into()))
            } else if text == "fourth" {
                // A per-record vector write failure (wrong dimensions) is
                // skipped just like an embedding-service failure.
                Ok(vec![1.0, 0.0])
            } else {
                Ok(vec![1.0, 0.0, 0.0, 0.0])
            }
        };
        let mut progress = Vec::new();
        let result = store
            .reindex_all(
                &mut embed,
                Some(&mut |done, total, layer| progress.push((done, total, layer))),
            )
            .unwrap();

        assert_eq!(
            result,
            ReindexResult {
                l1_count: 2,
                l0_count: 2
            }
        );
        assert_eq!(embedded, ["first", "fail", "third", "fourth"]);
        assert_eq!(
            progress,
            [
                (1, 2, ReindexLayer::L1),
                (2, 2, ReindexLayer::L1),
                (1, 2, ReindexLayer::L0),
                (2, 2, ReindexLayer::L0),
            ]
        );
        let c = store.lock().unwrap();
        assert_eq!(schema::table_row_count(&c, "l1_vec"), 1);
        assert_eq!(schema::table_row_count(&c, "l0_vec"), 1);
        drop(c);
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn test_trait_l0_write_read() {
        let mut store = test_store("trait_l0.db");
        store.init(None).unwrap();
        let rec = L0Record {
            id: "t1".into(),
            session_key: "sk".into(),
            session_id: "".into(),
            role: "user".into(),
            message_text: "hello".into(),
            recorded_at: "2026-07-13T00:00:00Z".into(),
            timestamp: 1000,
        };
        assert!(store.upsert_l0(&rec, None).unwrap());
        assert_eq!(store.count_l0().unwrap(), 1);
        let rows = store.query_l0_for_l1("sk", None, 10).unwrap();
        assert_eq!(rows[0].message_text, "hello");
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn test_trait_l1_write_read() {
        let mut store = test_store("trait_l1.db");
        store.init(None).unwrap();
        let rec = L1RecordRow {
            record_id: "t1".into(),
            content: "memory".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "sc".into(),
            session_key: "sk".into(),
            session_id: "".into(),
            timestamp_str: "2026-07-13".into(),
            timestamp_start: String::new(),
            timestamp_end: String::new(),
            created_time: "2026-07-13T00:00:00Z".into(),
            updated_time: "2026-07-13T00:00:01Z".into(),
            metadata_json: "{}".into(),
        };
        assert!(store.upsert_l1(&rec, None).unwrap());
        assert_eq!(store.count_l1().unwrap(), 1);
        let rows = store
            .query_l1_records(&L1QueryFilter {
                session_key: Some("sk".into()),
                session_id: None,
                updated_after: None,
            })
            .unwrap();
        assert_eq!(rows[0].content, "memory");
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn real_vec0_l1_upsert_replaces_vector_with_timestamp_and_rolls_back_on_failure() {
        let mut store = test_store("real_vec0_l1_upsert.db");
        store.dimensions = 3;
        store.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(!store.is_degraded(), "sqlite-vec fixture must load");

        let mut record = L1RecordRow {
            record_id: "l1-real".into(),
            content: "first".into(),
            r#type: "fact".into(),
            priority: 50.0,
            scene_name: String::new(),
            session_key: "s".into(),
            session_id: String::new(),
            timestamp_str: String::new(),
            timestamp_start: String::new(),
            timestamp_end: String::new(),
            created_time: "2026-07-14T00:00:00Z".into(),
            updated_time: "2026-07-14T00:00:01Z".into(),
            metadata_json: "{}".into(),
        };
        store.upsert_l1(&record, Some(&[1.0, 0.0, 0.0])).unwrap();

        record.content = "second".into();
        record.updated_time = "2026-07-14T00:00:02Z".into();
        store.upsert_l1(&record, Some(&[0.0, 1.0, 0.0])).unwrap();
        {
            let conn = store.lock().unwrap();
            let (count, updated): (i64, String) = conn
                .query_row(
                    "SELECT COUNT(*), max(updated_time) FROM l1_vec WHERE record_id = ?1",
                    ["l1-real"],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((count, updated.as_str()), (1, "2026-07-14T00:00:02Z"));
        }
        assert_eq!(
            store.search_l1_vector(&[0.0, 1.0, 0.0], 1).unwrap()[0].record_id,
            "l1-real"
        );

        // A zero vector updates metadata only and leaves the existing vector
        // (including its vec0 timestamp) untouched, matching sqlite.ts.
        record.content = "metadata-only".into();
        record.updated_time = "2026-07-14T00:00:03Z".into();
        store.upsert_l1(&record, Some(&[0.0, 0.0, 0.0])).unwrap();
        let mut zero_only = record.clone();
        zero_only.record_id = "l1-zero-only".into();
        store.upsert_l1(&zero_only, Some(&[0.0, 0.0, 0.0])).unwrap();
        {
            let conn = store.lock().unwrap();
            let updated: String = conn
                .query_row(
                    "SELECT updated_time FROM l1_vec WHERE record_id = ?1",
                    ["l1-real"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(updated, "2026-07-14T00:00:02Z");
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM l1_vec WHERE record_id = ?1",
                    ["l1-zero-only"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
            conn.execute(
                "INSERT INTO l1_vec (record_id, embedding, updated_time) VALUES (?1, ?2, ?3)",
                rusqlite::params!["l1-zero-only", "[0,0,0]", "2026-07-14T00:00:03Z"],
            )
            .unwrap();
        }
        assert_eq!(
            store.search_l1_vector(&[0.0, 1.0, 0.0], 1).unwrap()[0].record_id,
            "l1-real"
        );

        // vec0 rejects the wrong dimensions. The metadata write must be
        // rolled back together with the delete, preserving both prior rows.
        record.content = "must-roll-back".into();
        record.updated_time = "2026-07-14T00:00:04Z".into();
        assert!(store.upsert_l1(&record, Some(&[1.0, 0.0])).is_err());
        let conn = store.lock().unwrap();
        let content: String = conn
            .query_row(
                "SELECT content FROM l1_records WHERE record_id = ?1",
                ["l1-real"],
                |row| row.get(0),
            )
            .unwrap();
        let updated: String = conn
            .query_row(
                "SELECT updated_time FROM l1_vec WHERE record_id = ?1",
                ["l1-real"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content, "metadata-only");
        assert_eq!(updated, "2026-07-14T00:00:02Z");
        drop(conn);
        assert!(store.delete_l1("l1-real").unwrap());
        let conn = store.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM l1_vec WHERE record_id = ?1",
                ["l1-real"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        drop(conn);
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn real_vec0_l0_upsert_and_deferred_update_replace_atomically() {
        let mut store = test_store("real_vec0_l0_upsert.db");
        store.dimensions = 3;
        store.init(Some(&provider("mock", "model-a"))).unwrap();
        assert!(!store.is_degraded(), "sqlite-vec fixture must load");

        let mut record = L0Record {
            id: "l0-real".into(),
            session_key: "s".into(),
            session_id: String::new(),
            role: "user".into(),
            message_text: "first".into(),
            recorded_at: "2026-07-14T00:00:01Z".into(),
            timestamp: 1,
        };
        store.upsert_l0(&record, Some(&[1.0, 0.0, 0.0])).unwrap();

        record.message_text = "metadata-only".into();
        record.recorded_at = "2026-07-14T00:00:02Z".into();
        store.upsert_l0(&record, Some(&[0.0, 0.0, 0.0])).unwrap();
        let mut zero_only = record.clone();
        zero_only.id = "l0-zero-only".into();
        store.upsert_l0(&zero_only, Some(&[0.0, 0.0, 0.0])).unwrap();
        {
            let conn = store.lock().unwrap();
            let (count, recorded): (i64, String) = conn
                .query_row(
                    "SELECT COUNT(*), max(recorded_at) FROM l0_vec WHERE record_id = ?1",
                    ["l0-real"],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!((count, recorded.as_str()), (1, "2026-07-14T00:00:01Z"));
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM l0_vec WHERE record_id = ?1",
                    ["l0-zero-only"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
            conn.execute(
                "INSERT INTO l0_vec (record_id, embedding, recorded_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["l0-zero-only", "[0,0,0]", "2026-07-14T00:00:02Z"],
            )
            .unwrap();
        }
        {
            let conn = store.lock().unwrap();
            assert!(!l0::update_l0_embedding(&conn, "l0-real", &[0.0, 0.0, 0.0]).unwrap());
            assert!(l0::update_l0_embedding(&conn, "l0-real", &[0.0, 1.0, 0.0]).unwrap());
        }
        {
            let conn = store.lock().unwrap();
            let recorded: String = conn
                .query_row(
                    "SELECT recorded_at FROM l0_vec WHERE record_id = ?1",
                    ["l0-real"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(recorded, "2026-07-14T00:00:02Z");
        }
        assert_eq!(
            store.search_l0_vector(&[0.0, 1.0, 0.0], 1).unwrap()[0].record_id,
            "l0-real"
        );

        record.message_text = "must-roll-back".into();
        record.recorded_at = "2026-07-14T00:00:03Z".into();
        assert!(store.upsert_l0(&record, Some(&[1.0, 0.0])).is_err());
        let conn = store.lock().unwrap();
        let message: String = conn
            .query_row(
                "SELECT message_text FROM l0_conversations WHERE record_id = ?1",
                ["l0-real"],
                |row| row.get(0),
            )
            .unwrap();
        let recorded: String = conn
            .query_row(
                "SELECT recorded_at FROM l0_vec WHERE record_id = ?1",
                ["l0-real"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(message, "metadata-only");
        assert_eq!(recorded, "2026-07-14T00:00:02Z");
        drop(conn);
        assert!(store.delete_l0("l0-real").unwrap());
        let conn = store.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM l0_vec WHERE record_id = ?1",
                ["l0-real"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        drop(conn);
        store.close();
        cleanup(&store.db_path);
    }

    #[test]
    fn cleaner_enforces_minimums_and_deletes_expired_rows_from_all_indexes() {
        let mut store = test_store("cleaner.db");
        store.init(None).unwrap();
        for i in 0..51 {
            store
                .upsert_l0(
                    &L0Record {
                        id: format!("l0-{i}"),
                        session_key: "s".into(),
                        session_id: String::new(),
                        role: "user".into(),
                        message_text: format!("m{i}"),
                        recorded_at: if i < 2 {
                            "2026-07-10T00:00:00Z"
                        } else {
                            "2026-07-13T00:00:00Z"
                        }
                        .into(),
                        timestamp: i,
                    },
                    None,
                )
                .unwrap();
        }
        for i in 0..21 {
            store
                .upsert_l1(
                    &L1RecordRow {
                        record_id: format!("l1-{i}"),
                        content: format!("m{i}"),
                        r#type: "fact".into(),
                        priority: 50.0,
                        scene_name: String::new(),
                        session_key: "s".into(),
                        session_id: String::new(),
                        timestamp_str: String::new(),
                        timestamp_start: String::new(),
                        timestamp_end: String::new(),
                        created_time: "2026-07-10T00:00:00Z".into(),
                        updated_time: if i < 2 {
                            "2026-07-10T00:00:00Z"
                        } else {
                            "2026-07-13T00:00:00Z"
                        }
                        .into(),
                        metadata_json: "{}".into(),
                    },
                    None,
                )
                .unwrap();
        }
        let base = crate::test_support::unique_dir("aeon-memory-cleaner-store");
        let stats = aeon_memory_core::utils::memory_cleaner::run_once(
            &base,
            2,
            chrono::DateTime::parse_from_rfc3339("2026-07-13T12:00:00Z")
                .unwrap()
                .timestamp_millis(),
            Some(&mut store),
        )
        .unwrap();
        assert_eq!((stats.removed_l0, stats.removed_l1), (2, 2));
        assert_eq!(
            (store.count_l0().unwrap(), store.count_l1().unwrap()),
            (49, 19)
        );
        store.close();
        cleanup(&store.db_path);
        let _ = std::fs::remove_dir_all(base);
    }

    fn cleanup(path: &str) {
        crate::test_support::cleanup_db(path);
    }
}
