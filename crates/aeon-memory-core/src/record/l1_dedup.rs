// port of src/core/record/l1-dedup.ts — full async LLM-driven conflict detection.
// Calls: embedding.embedBatch → vectorStore.searchL1Vector → LLM judgment → parse → actions

use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::prompt::l1_dedup::{L1_DEDUP_SYSTEM_PROMPT, format_dedup_prompt};
use crate::record::l1_writer::MemoryRecord;
use crate::types::{
    EmbeddingProviderInfo, IMemoryStore, L0QueryRow, L0Record, L0SearchResult, L1FtsResult,
    L1QueryFilter, L1RecordRow, L1SearchResult, LlmRunParams, LlmRunner, ReindexLayer,
    ReindexResult, StoreCapabilities, StoreInitResult,
};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum DedupAction {
    Store,
    Update {
        target_ids: Vec<String>,
        merged_content: String,
        merged_type: Option<String>,
        merged_priority: Option<f64>,
        merged_timestamps: Option<Vec<String>>,
    },
    Merge {
        target_ids: Vec<String>,
        merged_content: String,
        merged_type: Option<String>,
        merged_priority: Option<f64>,
        merged_timestamps: Option<Vec<String>>,
    },
    Skip,
}

pub struct CandidateMatch {
    pub new_memory: MemoryRecord,
    pub candidates: Vec<MemoryRecord>,
}

/// Full batch dedup: candidates → LLM judgment → actions.
/// Port of batchDedup() from l1-dedup.ts:51-136
pub async fn batch_dedup(
    memories: &[MemoryRecord],
    _session_key: &str,
    vector_store: &mut dyn IMemoryStore,
    embedding_service: &dyn crate::types::EmbeddingService,
    llm_runner: &dyn LlmRunner,
    enable_dedup: bool,
    conflict_recall_top_k: u32,
) -> AeonMemoryResult<Vec<DedupAction>> {
    if !enable_dedup || memories.is_empty() {
        return Ok(memories.iter().map(|_| DedupAction::Store).collect());
    }

    let has_data = vector_store.count_l1().unwrap_or(0) > 0;
    let has_fts = vector_store.is_fts_available() || vector_store.capabilities().fts_search;
    let has_vector = has_data && vector_store.capabilities().vector_search;

    if !has_data && !has_fts {
        return Ok(memories.iter().map(|_| DedupAction::Store).collect());
    }

    // Phase 1: Find candidates for each memory. TS embeds the entire incoming
    // batch once, and degrades vector failures to FTS when available.
    let mut all_candidates: Vec<Vec<MemoryRecord>> = Vec::new();
    let new_ids: std::collections::HashSet<String> =
        memories.iter().map(|m| m.id.clone()).collect();

    let batch_vectors = if has_vector {
        embedding_service
            .embed_batch(
                &memories
                    .iter()
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>(),
            )
            .ok()
    } else {
        None
    };
    for (index, mem) in memories.iter().enumerate() {
        let candidates = if let Some(vectors) = &batch_vectors {
            if let Some(vector) = vectors.get(index) {
                vector_store
                    .search_l1_vector(
                        vector,
                        i64::from(conflict_recall_top_k) + new_ids.len() as i64,
                    )?
                    .into_iter()
                    .filter(|r| !new_ids.contains(&r.record_id))
                    .take(conflict_recall_top_k as usize)
                    .map(memory_from_vector_result)
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            find_candidates(
                mem,
                vector_store,
                embedding_service,
                false,
                has_fts,
                5,
                &new_ids,
            )?
        };
        all_candidates.push(candidates);
    }

    // Phase 2: LLM judgment
    if all_candidates.iter().all(Vec::is_empty) {
        return Ok(memories.iter().map(|_| DedupAction::Store).collect());
    }

    {
        let existing_contents: Vec<&str> = all_candidates
            .iter()
            .flat_map(|cs| cs.iter())
            .map(|c| c.content.as_str())
            .collect();
        let existing_text = if existing_contents.is_empty() {
            "无".to_string()
        } else {
            existing_contents.join("\n---\n")
        };
        let new_texts: Vec<&str> = memories.iter().map(|m| m.content.as_str()).collect();
        let prompt = format_dedup_prompt(&existing_text, &new_texts.join("\n---\n"));

        let response = match llm_runner
            .run(LlmRunParams {
                prompt,
                system_prompt: Some(L1_DEDUP_SYSTEM_PROMPT.to_string()),
                task_id: "l1-conflict-detection".to_string(),
                timeout_ms: Some(180_000),
                max_tokens: None,
                workspace_dir: None,
                file_tool_policy: None,
                instance_id: None,
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return Ok(memories.iter().map(|_| DedupAction::Store).collect()),
        };

        // Parse LLM response as JSON array of decisions
        let decisions = parse_dedup_response(&response, memories)?;
        Ok(decisions)
    }
}

fn find_candidates(
    mem: &MemoryRecord,
    vs: &mut dyn IMemoryStore,
    embedding_svc: &dyn crate::types::EmbeddingService,
    has_vector: bool,
    has_fts: bool,
    top_k: u32,
    exclude_ids: &std::collections::HashSet<String>,
) -> AeonMemoryResult<Vec<MemoryRecord>> {
    if has_vector {
        let query = embedding_svc.embed(&mem.content)?;
        let results = vs.search_l1_vector(&query, i64::from(top_k) + exclude_ids.len() as i64)?;
        return Ok(results
            .into_iter()
            .filter(|r| !exclude_ids.contains(&r.record_id))
            .take(top_k as usize)
            .map(memory_from_vector_result)
            .collect());
    }
    if !has_fts {
        return Ok(Vec::new());
    }
    let fts_query = crate::fts_query::build_fts_query(&mem.content);
    if let Some(q) = fts_query {
        let results = vs.search_l1_fts(&q, top_k as i64).unwrap_or_default();
        return Ok(results
            .into_iter()
            .filter(|r| !exclude_ids.contains(&r.record_id))
            .map(|r| MemoryRecord {
                id: r.record_id,
                content: r.content,
                r#type: r.r#type,
                priority: r.priority,
                scene_name: r.scene_name,
                source_message_ids: vec![],
                metadata: serde_json::from_str(&r.metadata_json)
                    .unwrap_or(Value::Object(Default::default())),
                timestamps: vec![r.timestamp_str],
                created_at: String::new(),
                updated_at: String::new(),
                session_key: r.session_key,
                session_id: r.session_id,
            })
            .take(top_k as usize)
            .collect());
    }
    Ok(Vec::new())
}

fn memory_from_vector_result(r: L1SearchResult) -> MemoryRecord {
    MemoryRecord {
        id: r.record_id,
        content: r.content,
        r#type: r.r#type,
        priority: r.priority,
        scene_name: r.scene_name,
        source_message_ids: vec![],
        metadata: serde_json::from_str(&r.metadata_json)
            .unwrap_or(Value::Object(Default::default())),
        timestamps: vec![r.timestamp_str],
        created_at: String::new(),
        updated_at: String::new(),
        session_key: r.session_key,
        session_id: r.session_id,
    }
}

fn parse_dedup_response(
    response: &str,
    memories: &[MemoryRecord],
) -> AeonMemoryResult<Vec<DedupAction>> {
    let cleaned = crate::utils::sanitize::sanitize_json_for_parse(response);
    let json: Value = serde_json::from_str(&cleaned).unwrap_or(Value::Array(Vec::new()));

    let arr = match json {
        Value::Array(ref a) => a.clone(),
        _ => return Ok(memories.iter().map(|_| DedupAction::Store).collect()),
    };

    let by_id: std::collections::HashMap<&str, &Value> = arr
        .iter()
        .filter_map(|item| {
            item.get("record_id")
                .and_then(Value::as_str)
                .map(|id| (id, item))
        })
        .filter(|(id, _)| !id.is_empty())
        .collect();
    let mut actions: Vec<DedupAction> = Vec::with_capacity(memories.len());
    for mem in memories {
        let action = if let Some(item) = by_id.get(mem.id.as_str()) {
            let action_str = item
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("store");
            match action_str {
                "skip" => DedupAction::Skip,
                "update" => {
                    let target_ids = item
                        .get("target_ids")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let content = item
                        .get("merged_content")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&mem.content)
                        .to_owned();
                    DedupAction::Update {
                        target_ids,
                        merged_content: content,
                        merged_type: valid_merged_type(item),
                        merged_priority: item.get("merged_priority").and_then(Value::as_f64),
                        merged_timestamps: string_array(item.get("merged_timestamps")),
                    }
                }
                "merge" => {
                    let target_ids = item
                        .get("target_ids")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let content = item
                        .get("merged_content")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&mem.content)
                        .to_owned();
                    DedupAction::Merge {
                        target_ids,
                        merged_content: content,
                        merged_type: valid_merged_type(item),
                        merged_priority: item.get("merged_priority").and_then(Value::as_f64),
                        merged_timestamps: string_array(item.get("merged_timestamps")),
                    }
                }
                _ => DedupAction::Store,
            }
        } else {
            DedupAction::Store
        };
        actions.push(action);
    }

    Ok(actions)
}

fn valid_merged_type(item: &Value) -> Option<String> {
    item.get("merged_type")
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "persona" | "episodic" | "instruction"))
        .map(str::to_owned)
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value.and_then(Value::as_array).map(|values| {
        values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

// ── Recording mocks for testing ──

pub struct RecordingMockStore {
    pub upsert_l1_calls: std::sync::Mutex<Vec<L1RecordRow>>,
    pub search_l1_fts_calls: std::sync::Mutex<Vec<String>>,
    pub count_l1_calls: std::sync::Mutex<u32>,
    pub fts_results: std::sync::Mutex<Vec<L1FtsResult>>,
    pub vector_enabled: std::sync::Mutex<bool>,
    pub vector_results: std::sync::Mutex<Vec<L1SearchResult>>,
    pub vector_search_calls: std::sync::Mutex<Vec<Vec<f32>>>,
    pub vector_search_top_k_calls: std::sync::Mutex<Vec<i64>>,
    pub upsert_embeddings: std::sync::Mutex<Vec<Option<Vec<f32>>>>,
}

impl RecordingMockStore {
    pub fn new() -> Self {
        Self {
            upsert_l1_calls: std::sync::Mutex::new(Vec::new()),
            search_l1_fts_calls: std::sync::Mutex::new(Vec::new()),
            count_l1_calls: std::sync::Mutex::new(0),
            fts_results: std::sync::Mutex::new(Vec::new()),
            vector_enabled: std::sync::Mutex::new(false),
            vector_results: std::sync::Mutex::new(Vec::new()),
            vector_search_calls: std::sync::Mutex::new(Vec::new()),
            vector_search_top_k_calls: std::sync::Mutex::new(Vec::new()),
            upsert_embeddings: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for RecordingMockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IMemoryStore for RecordingMockStore {
    fn supports_deferred_embedding(&self) -> bool {
        true
    }
    fn init(
        &mut self,
        _info: Option<&EmbeddingProviderInfo>,
    ) -> std::result::Result<StoreInitResult, AeonMemoryCoreError> {
        Ok(StoreInitResult {
            needs_reindex: false,
            reason: None,
        })
    }
    fn is_degraded(&self) -> bool {
        false
    }
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            vector_search: *self.vector_enabled.lock().unwrap(),
            fts_search: true,
            native_hybrid_search: false,
            sparse_vectors: false,
        }
    }
    fn close(&mut self) {}
    fn upsert_l1(
        &mut self,
        record: &L1RecordRow,
        embedding: Option<&[f32]>,
    ) -> std::result::Result<bool, AeonMemoryCoreError> {
        self.upsert_l1_calls.lock().unwrap().push(record.clone());
        self.upsert_embeddings
            .lock()
            .unwrap()
            .push(embedding.map(<[f32]>::to_vec));
        Ok(true)
    }
    fn delete_l1(&mut self, _record_id: &str) -> std::result::Result<bool, AeonMemoryCoreError> {
        Ok(true)
    }
    fn count_l1(&self) -> std::result::Result<i64, AeonMemoryCoreError> {
        *self.count_l1_calls.lock().unwrap() += 1;
        Ok(5)
    }
    fn query_l1_records(
        &self,
        _filter: &L1QueryFilter,
    ) -> std::result::Result<Vec<L1RecordRow>, AeonMemoryCoreError> {
        Ok(Vec::new())
    }
    fn search_l1_fts(
        &self,
        fts_query: &str,
        _limit: i64,
    ) -> std::result::Result<Vec<L1FtsResult>, AeonMemoryCoreError> {
        self.search_l1_fts_calls
            .lock()
            .unwrap()
            .push(fts_query.to_string());
        Ok(self.fts_results.lock().unwrap().clone())
    }
    fn search_l1_vector(
        &self,
        query_embedding: &[f32],
        top_k: i64,
    ) -> std::result::Result<Vec<L1SearchResult>, AeonMemoryCoreError> {
        self.vector_search_calls
            .lock()
            .unwrap()
            .push(query_embedding.to_vec());
        self.vector_search_top_k_calls.lock().unwrap().push(top_k);
        Ok(self.vector_results.lock().unwrap().clone())
    }
    fn upsert_l0(
        &mut self,
        _record: &L0Record,
        _embedding: Option<&[f32]>,
    ) -> std::result::Result<bool, AeonMemoryCoreError> {
        Ok(true)
    }
    fn delete_l0(&mut self, _record_id: &str) -> std::result::Result<bool, AeonMemoryCoreError> {
        Ok(true)
    }
    fn count_l0(&self) -> std::result::Result<i64, AeonMemoryCoreError> {
        Ok(0)
    }
    fn query_l0_for_l1(
        &self,
        _session_key: &str,
        _after_recorded_at_ms: Option<i64>,
        _limit: i64,
    ) -> std::result::Result<Vec<L0QueryRow>, AeonMemoryCoreError> {
        Ok(Vec::new())
    }
    fn search_l0_vector(
        &self,
        _query_embedding: &[f32],
        _top_k: i64,
    ) -> AeonMemoryResult<Vec<L0SearchResult>> {
        Ok(Vec::new())
    }
    fn reindex_all(
        &mut self,
        _embed_fn: &mut dyn FnMut(&str) -> AeonMemoryResult<Vec<f32>>,
        _on_progress: Option<&mut dyn FnMut(usize, usize, ReindexLayer)>,
    ) -> AeonMemoryResult<ReindexResult> {
        Ok(ReindexResult::default())
    }
    fn is_fts_available(&self) -> bool {
        true
    }
}

pub struct RecordingMockLlm {
    pub calls: std::sync::Mutex<Vec<LlmRunParams>>,
    pub response: String,
}

pub struct RecordingMockEmbedding {
    pub calls: std::sync::Mutex<Vec<String>>,
    pub value: Vec<f32>,
}

impl RecordingMockEmbedding {
    pub fn new(value: Vec<f32>) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            value,
        }
    }
}

impl crate::types::EmbeddingService for RecordingMockEmbedding {
    fn embed(&self, text: &str) -> AeonMemoryResult<Vec<f32>> {
        self.calls.lock().unwrap().push(text.to_string());
        Ok(self.value.clone())
    }
    fn embed_batch(&self, texts: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
        for text in texts {
            self.calls.lock().unwrap().push(text.clone());
        }
        Ok(texts.iter().map(|_| self.value.clone()).collect())
    }
    fn dimensions(&self) -> u32 {
        self.value.len() as u32
    }
}

impl RecordingMockLlm {
    pub fn new(response: &str) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            response: response.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl LlmRunner for RecordingMockLlm {
    async fn run(&self, params: LlmRunParams) -> AeonMemoryResult<String> {
        self.calls.lock().unwrap().push(params.clone());
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> L1FtsResult {
        L1FtsResult {
            record_id: "existing-1".into(),
            content: "existing coffee preference".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "preferences".into(),
            score: 1.0,
            timestamp_str: "2026-07-13T00:00:00Z".into(),
            timestamp_start: String::new(),
            timestamp_end: String::new(),
            session_key: "sk".into(),
            session_id: String::new(),
            metadata_json: "{}".into(),
        }
    }

    #[tokio::test]
    async fn test_dedup_disabled_returns_store_all() {
        let mems = vec![MemoryRecord {
            id: "m1".into(),
            content: "test".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "".into(),
            source_message_ids: vec![],
            metadata: serde_json::json!({}),
            timestamps: vec![],
            created_at: String::new(),
            updated_at: String::new(),
            session_key: "sk".into(),
            session_id: "".into(),
        }];
        let mut store = RecordingMockStore::new();
        let embedding = RecordingMockEmbedding::new(vec![1.0, 0.0]);
        let llm = RecordingMockLlm::new("[]");
        let actions = batch_dedup(&mems, "sk", &mut store, &embedding, &llm, false, 5)
            .await
            .unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0], DedupAction::Store);
    }

    #[tokio::test]
    async fn test_dedup_calls_store_and_llm() {
        let mems = vec![MemoryRecord {
            id: "m1".into(),
            content: "user likes coffee".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "".into(),
            source_message_ids: vec![],
            metadata: serde_json::json!({}),
            timestamps: vec![],
            created_at: String::new(),
            updated_at: String::new(),
            session_key: "sk".into(),
            session_id: "".into(),
        }];
        let mut store = RecordingMockStore::new();
        store.fts_results.lock().unwrap().push(candidate());
        let llm = RecordingMockLlm::new(r#"[{"record_id":"m1","action":"store","target_ids":[]}]"#);
        let embedding = RecordingMockEmbedding::new(vec![1.0, 0.0]);

        let actions = batch_dedup(&mems, "sk", &mut store, &embedding, &llm, true, 5)
            .await
            .unwrap();
        assert_eq!(actions.len(), 1);

        // Verify store was called for FTS search and count
        assert!(
            *store.count_l1_calls.lock().unwrap() > 0,
            "count_l1 should be called"
        );
        assert!(
            !store.search_l1_fts_calls.lock().unwrap().is_empty(),
            "search_l1_fts should be called"
        );
        // Verify LLM was called
        assert_eq!(llm.calls.lock().unwrap().len(), 1);
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls[0].task_id, "l1-conflict-detection");
        assert_eq!(
            calls[0].system_prompt.as_deref(),
            Some(L1_DEDUP_SYSTEM_PROMPT)
        );
        assert!(calls[0].prompt.contains("existing coffee preference"));
    }

    #[tokio::test]
    async fn test_dedup_llm_decision_skip() {
        let mems = vec![MemoryRecord {
            id: "m1".into(),
            content: "user likes coffee".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "".into(),
            source_message_ids: vec![],
            metadata: serde_json::json!({}),
            timestamps: vec![],
            created_at: String::new(),
            updated_at: String::new(),
            session_key: "sk".into(),
            session_id: "".into(),
        }];
        let mut store = RecordingMockStore::new();
        store.fts_results.lock().unwrap().push(candidate());
        let llm = RecordingMockLlm::new(r#"[{"record_id":"m1","action":"skip","target_ids":[]}]"#);
        let embedding = RecordingMockEmbedding::new(vec![1.0, 0.0]);

        let actions = batch_dedup(&mems, "sk", &mut store, &embedding, &llm, true, 5)
            .await
            .unwrap();
        assert_eq!(actions[0], DedupAction::Skip);
    }

    #[tokio::test]
    async fn vector_recall_uses_exact_embedding_and_llm_update_target() {
        let mems = vec![MemoryRecord {
            id: "m1".into(),
            content: "new preference".into(),
            r#type: "persona".into(),
            priority: 60.0,
            scene_name: "prefs".into(),
            source_message_ids: vec!["msg1".into()],
            metadata: serde_json::json!({}),
            timestamps: vec![],
            created_at: String::new(),
            updated_at: String::new(),
            session_key: "sk".into(),
            session_id: String::new(),
        }];
        let mut store = RecordingMockStore::new();
        *store.vector_enabled.lock().unwrap() = true;
        store.vector_results.lock().unwrap().push(L1SearchResult {
            record_id: "existing-1".into(),
            content: "old preference".into(),
            r#type: "persona".into(),
            priority: 50.0,
            scene_name: "prefs".into(),
            score: 0.98,
            timestamp_str: "2026-01-01T00:00:00Z".into(),
            timestamp_start: String::new(),
            timestamp_end: String::new(),
            session_key: "sk".into(),
            session_id: String::new(),
            metadata_json: "{}".into(),
        });
        let embedding = RecordingMockEmbedding::new(vec![0.25, 0.75]);
        let llm = RecordingMockLlm::new(
            r#"[{"record_id":"m1","action":"update","target_ids":["existing-1"],"merged_content":"updated preference","merged_priority":70.5}]"#,
        );

        let actions = batch_dedup(&mems, "sk", &mut store, &embedding, &llm, true, 5)
            .await
            .unwrap();

        assert_eq!(
            store.vector_search_calls.lock().unwrap().as_slice(),
            &[vec![0.25, 0.75]]
        );
        assert_eq!(
            embedding.calls.lock().unwrap().as_slice(),
            &["new preference"]
        );
        assert_eq!(
            actions,
            vec![DedupAction::Update {
                target_ids: vec!["existing-1".into()],
                merged_content: "updated preference".into(),
                merged_type: None,
                merged_priority: Some(70.5),
                merged_timestamps: None,
            }]
        );
        let calls = llm.calls.lock().unwrap();
        assert_eq!(calls[0].task_id, "l1-conflict-detection");
        assert!(calls[0].prompt.contains("old preference"));
        assert!(calls[0].prompt.contains("new preference"));
    }
}
