use crate::error::AeonMemoryResult;
use crate::fts_query::build_fts_query;
use crate::search::rrf_k60;
use crate::types::{EmbeddingService, L1FtsResult, L1SearchResult};

const NO_SEARCH_CAPABILITY: &str = "Embedding service is not configured and FTS is not available. Memory search requires an embedding provider or FTS5 support. Please configure an embedding provider in the embedding.provider setting (e.g. openai_compatible).";

pub trait MemorySearchStore: Send + Sync {
    fn is_fts_available(&self) -> bool;
    fn search_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>>;
    fn search_vector(&self, embedding: &[f32], limit: i64)
    -> AeonMemoryResult<Vec<L1SearchResult>>;
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemorySearchItem {
    pub id: String,
    pub content: String,
    pub r#type: String,
    pub priority: f64,
    pub scene_name: String,
    pub score: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemorySearchResult {
    pub results: Vec<MemorySearchItem>,
    pub total: usize,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn from_fts(r: L1FtsResult) -> MemorySearchItem {
    MemorySearchItem {
        id: r.record_id,
        content: r.content,
        r#type: r.r#type,
        priority: r.priority,
        scene_name: r.scene_name,
        score: r.score,
        created_at: r.timestamp_start,
        updated_at: r.timestamp_end,
    }
}

fn from_vector(r: L1SearchResult) -> MemorySearchItem {
    MemorySearchItem {
        id: r.record_id,
        content: r.content,
        r#type: r.r#type,
        priority: r.priority,
        scene_name: r.scene_name,
        score: r.score,
        created_at: r.timestamp_start,
        updated_at: r.timestamp_end,
    }
}

pub fn execute_memory_search(
    query: &str,
    limit: usize,
    type_filter: Option<&str>,
    scene_filter: Option<&str>,
    store: Option<&dyn MemorySearchStore>,
    embedding: Option<&dyn EmbeddingService>,
) -> MemorySearchResult {
    if query.trim().is_empty() || store.is_none() {
        return empty("none", None);
    }
    let store = store.expect("checked above");
    let has_fts = store.is_fts_available();
    let has_embedding = embedding.is_some();
    if !has_fts && !has_embedding {
        return empty("none", Some(NO_SEARCH_CAPABILITY.to_owned()));
    }

    let candidate_k = limit.saturating_mul(3).min(i64::MAX as usize) as i64;
    let fts_items = if has_fts {
        build_fts_query(query)
            .and_then(|q| store.search_fts(&q, candidate_k).ok())
            .unwrap_or_default()
            .into_iter()
            .map(from_fts)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let vector_items = embedding
        .and_then(|svc| svc.embed(query).ok())
        .and_then(|v| store.search_vector(&v, candidate_k).ok())
        .unwrap_or_default()
        .into_iter()
        .map(from_vector)
        .collect::<Vec<_>>();

    let fts_ok = !fts_items.is_empty();
    let vector_ok = !vector_items.is_empty();
    let strategy = match (fts_ok, vector_ok) {
        (true, true) => "hybrid",
        (false, true) => "embedding",
        (true, false) => "fts",
        (false, false) => return empty(if has_embedding { "embedding" } else { "fts" }, None),
    };
    let mut results = if strategy == "hybrid" {
        rrf_k60(&[fts_items, vector_items], |item| item.id.as_str())
            .into_iter()
            .map(|(mut item, score)| {
                item.score = score;
                item
            })
            .collect()
    } else if fts_ok {
        fts_items
    } else {
        vector_items
    };

    if let Some(expected) = type_filter {
        results.retain(|r| r.r#type == expected);
    }
    if let Some(fragment) = scene_filter {
        let fragment = fragment.to_lowercase();
        results.retain(|r| r.scene_name.to_lowercase().contains(&fragment));
    }
    results.truncate(limit);
    MemorySearchResult {
        total: results.len(),
        results,
        strategy: strategy.to_owned(),
        message: None,
    }
}

fn empty(strategy: &str, message: Option<String>) -> MemorySearchResult {
    MemorySearchResult {
        results: Vec::new(),
        total: 0,
        strategy: strategy.to_owned(),
        message,
    }
}

pub fn format_memory_search_response(result: &MemorySearchResult) -> String {
    if let Some(message) = &result.message {
        return message.clone();
    }
    if result.results.is_empty() {
        return "No matching memories found.".to_owned();
    }
    let mut lines = vec![
        format!("Found {} matching memories:", result.total),
        String::new(),
    ];
    for item in &result.results {
        let priority = if item.priority >= 0.0 {
            format!(" (priority: {})", item.priority)
        } else {
            " (global instruction)".to_owned()
        };
        let scene = if item.scene_name.is_empty() {
            String::new()
        } else {
            format!(" [scene: {}]", item.scene_name)
        };
        lines.push(format!(
            "- **[{}]**{}{} (score: {:.3})",
            item.r#type, priority, scene, item.score
        ));
        lines.push(format!("  {}", item.content));
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AeonMemoryCoreError;

    struct Embed;
    impl EmbeddingService for Embed {
        fn embed(&self, _: &str) -> Result<Vec<f32>, AeonMemoryCoreError> {
            Ok(vec![1.0, 0.0])
        }
        fn embed_batch(&self, _: &[String]) -> Result<Vec<Vec<f32>>, AeonMemoryCoreError> {
            unreachable!()
        }
        fn dimensions(&self) -> u32 {
            2
        }
    }
    struct Store;
    impl MemorySearchStore for Store {
        fn is_fts_available(&self) -> bool {
            true
        }
        fn search_fts(&self, _: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
            assert_eq!(limit, 6);
            Ok(vec![fts("both", "episodic", "Work")])
        }
        fn search_vector(
            &self,
            embedding: &[f32],
            limit: i64,
        ) -> AeonMemoryResult<Vec<L1SearchResult>> {
            assert_eq!(embedding, [1.0, 0.0]);
            assert_eq!(limit, 6);
            Ok(vec![vector("vec"), vector("both")])
        }
    }
    fn fts(id: &str, kind: &str, scene: &str) -> L1FtsResult {
        L1FtsResult {
            record_id: id.into(),
            content: format!("content-{id}"),
            r#type: kind.into(),
            priority: 50.0,
            scene_name: scene.into(),
            score: 0.8,
            timestamp_str: "t".into(),
            timestamp_start: "start".into(),
            timestamp_end: "end".into(),
            session_key: "s".into(),
            session_id: "i".into(),
            metadata_json: "{}".into(),
        }
    }
    fn vector(id: &str) -> L1SearchResult {
        let f = fts(id, "episodic", "work notes");
        L1SearchResult {
            record_id: f.record_id,
            content: f.content,
            r#type: f.r#type,
            priority: f.priority,
            scene_name: f.scene_name,
            score: f.score,
            timestamp_str: f.timestamp_str,
            timestamp_start: f.timestamp_start,
            timestamp_end: f.timestamp_end,
            session_key: f.session_key,
            session_id: f.session_id,
            metadata_json: f.metadata_json,
        }
    }

    #[test]
    fn hybrid_uses_rrf_then_filters_and_limits() {
        let result = execute_memory_search(
            "project",
            2,
            Some("episodic"),
            Some("WORK"),
            Some(&Store),
            Some(&Embed),
        );
        assert_eq!(result.strategy, "hybrid");
        assert_eq!(result.results[0].id, "both");
        assert!(result.results[0].score > result.results[1].score);
        assert!(format_memory_search_response(&result).contains("Found 2 matching memories:"));
    }
    #[test]
    fn capability_message_is_exact() {
        let result = execute_memory_search("x", 5, None, None, Some(&NoCaps), None);
        assert_eq!(result.strategy, "none");
        assert_eq!(result.message.as_deref(), Some(NO_SEARCH_CAPABILITY));
    }
    struct NoCaps;
    impl MemorySearchStore for NoCaps {
        fn is_fts_available(&self) -> bool {
            false
        }
        fn search_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
            unreachable!()
        }
        fn search_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
            unreachable!()
        }
    }
}
