use aeon_memory_core::config::{AeonMemoryConfig, RecallStrategy};
use aeon_memory_core::error::AeonMemoryResult;
use aeon_memory_core::hooks::{AutoRecall, RecallStore};
use aeon_memory_core::types::{L1FtsResult, L1SearchResult};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
struct Store;
impl RecallStore for Store {
    fn is_fts_available(&self) -> bool {
        true
    }
    fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        Ok(vec![
            row("high", &"A".repeat(90), "persona", 0.95),
            row("low", "second memory", "episodic", 0.8),
        ])
    }
    fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
        Ok(vec![])
    }
}
struct AstralStore;
impl RecallStore for AstralStore {
    fn is_fts_available(&self) -> bool {
        true
    }
    fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        let mut emoji = row("emoji", &"😀".repeat(20), "persona", 0.95);
        emoji.scene_name.clear();
        let mut tail = row("tail", "tail", "episodic", 0.8);
        tail.scene_name.clear();
        Ok(vec![emoji, tail])
    }
    fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
        Ok(vec![])
    }
}
struct FailingStore;
impl RecallStore for FailingStore {
    fn is_fts_available(&self) -> bool {
        true
    }
    fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
        Err(aeon_memory_core::AeonMemoryCoreError::Store(
            "fts unavailable".into(),
        ))
    }
    fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
        Ok(vec![])
    }
}
fn row(id: &str, content: &str, kind: &str, score: f64) -> L1FtsResult {
    L1FtsResult {
        record_id: id.into(),
        content: content.into(),
        r#type: kind.into(),
        priority: 1.0,
        scene_name: if kind == "persona" { "prefs" } else { "work" }.into(),
        score,
        timestamp_str: String::new(),
        timestamp_start: String::new(),
        timestamp_end: String::new(),
        session_key: "s".into(),
        session_id: "i".into(),
        metadata_json: "{}".into(),
    }
}
fn dir() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "aeon-memory-recall-runtime-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
#[tokio::test]
async fn rust_populated_recall_matches_pinned_ts_final_contract() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/recall_runtime_oracle.json")).unwrap();
    let mut c = AeonMemoryConfig::default();
    c.recall.strategy = RecallStrategy::Keyword;
    c.recall.max_results = 2;
    c.recall.score_threshold = 0.3;
    c.recall.max_chars_per_memory = 60;
    c.recall.max_total_recall_chars = 101;
    c.recall.timeout_ms = 30000;
    let got = AutoRecall {
        config: c,
        data_dir: dir(),
        store: Some(Arc::new(Store)),
        embedding: None,
    }
    .perform("memory query")
    .await
    .unwrap()
    .unwrap();
    let ts = &oracle["populated"];
    assert_eq!(
        got.prepend_context.as_deref(),
        ts["prependContext"].as_str()
    );
    assert_eq!(
        got.append_system_context.as_deref(),
        ts["appendSystemContext"].as_str()
    );
    assert_eq!(got.recall_strategy, ts["recallStrategy"]);
    assert_eq!(got.recalled_l1_memories.len(), 2);
    for (i, m) in got.recalled_l1_memories.iter().enumerate() {
        assert_eq!(m.content, ts["recalledL1Memories"][i]["content"]);
        assert_eq!(m.score, 0.0);
        assert_eq!(m.r#type, ts["recalledL1Memories"][i]["type"]);
    }
    assert_eq!(oracle["timeout"], serde_json::Value::Null);
    assert_eq!(oracle["hybrid"]["recallStrategy"], "hybrid");
    assert_eq!(
        oracle["hybrid"]["recalledL1Memories"][0]["content"],
        "A".repeat(90)
    );
    assert_eq!(
        oracle["hybrid"]["recalledL1Memories"][1]["content"],
        "vector memory"
    );
    assert_eq!(oracle["vector"]["recallStrategy"], "embedding");
    assert_eq!(
        oracle["vector"]["recalledL1Memories"][0]["content"],
        "vector memory"
    );
    assert_eq!(oracle["embeddingCalls"][0][1]["timeoutMs"], 222);
}

#[tokio::test]
async fn hybrid_without_embedding_executes_keyword_but_reports_configured_strategy() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/recall_runtime_oracle.json")).unwrap();
    let mut config = AeonMemoryConfig::default();
    config.recall.strategy = RecallStrategy::Hybrid;
    config.recall.score_threshold = 0.01;
    config.recall.max_chars_per_memory = 0;
    config.recall.max_total_recall_chars = 0;
    let got = AutoRecall {
        config,
        data_dir: dir(),
        store: Some(Arc::new(Store)),
        embedding: None,
    }
    .perform("memory query")
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        got.recall_strategy,
        oracle["hybridFallback"]["recallStrategy"]
    );
    assert_eq!(got.recall_strategy, "hybrid");
    assert_eq!(
        got.prepend_context.as_deref(),
        oracle["hybridFallback"]["prependContext"].as_str()
    );
}

#[tokio::test]
async fn astral_query_gate_and_total_budget_match_js_utf16_oracle() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/recall_runtime_oracle.json")).unwrap();
    let mut config = AeonMemoryConfig::default();
    config.recall.strategy = RecallStrategy::Keyword;
    config.recall.max_results = 2;
    config.recall.score_threshold = 0.3;
    config.recall.max_chars_per_memory = 0;
    config.recall.max_total_recall_chars = 45;
    let data_dir = dir();
    std::fs::write(data_dir.join("persona.md"), "# Stable persona\n").unwrap();
    let gate = AutoRecall {
        config: config.clone(),
        data_dir: data_dir.clone(),
        store: Some(Arc::new(Store)),
        embedding: None,
    }
    .perform("😀")
    .await
    .unwrap()
    .unwrap();
    assert_eq!(gate.recall_strategy, oracle["astralGate"]["recallStrategy"]);
    assert_eq!(
        gate.append_system_context.as_deref(),
        oracle["astralGate"]["appendSystemContext"].as_str()
    );

    let got = AutoRecall {
        config,
        data_dir,
        store: Some(Arc::new(AstralStore)),
        embedding: None,
    }
    .perform("😀a")
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        got.prepend_context.as_deref(),
        oracle["astralBudget"]["prependContext"].as_str()
    );
    assert_eq!(
        got.recall_strategy,
        oracle["astralBudget"]["recallStrategy"]
    );
    assert_eq!(got.recalled_l1_memories.len(), 1);
}

#[tokio::test]
async fn search_failure_keeps_stable_context_and_matches_ts_fail_soft_contract() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/recall_runtime_oracle.json")).unwrap();
    let data_dir = dir();
    std::fs::write(data_dir.join("persona.md"), "# Stable persona\n").unwrap();
    let mut config = AeonMemoryConfig::default();
    config.recall.strategy = RecallStrategy::Keyword;
    let got = AutoRecall {
        config,
        data_dir,
        store: Some(Arc::new(FailingStore)),
        embedding: None,
    }
    .perform("memory query")
    .await
    .unwrap()
    .unwrap();
    assert!(got.prepend_context.is_none());
    assert_eq!(
        got.append_system_context.as_deref(),
        oracle["searchFailure"]["appendSystemContext"].as_str()
    );
    assert_eq!(
        got.recall_strategy,
        oracle["searchFailure"]["recallStrategy"]
    );
}
