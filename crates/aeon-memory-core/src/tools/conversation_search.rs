use crate::error::AeonMemoryResult;
use crate::fts_query::build_fts_query;
use crate::search::rrf_k60;
use crate::types::{EmbeddingService, L0FtsResult, L0SearchResult};

const NO_SEARCH_CAPABILITY: &str = "Embedding service is not configured and FTS is not available. Conversation search requires an embedding provider or FTS5 support. Please configure an embedding provider in the embedding.provider setting (e.g. openai_compatible).";

pub trait ConversationSearchStore: Send + Sync {
    fn is_fts_available(&self) -> bool;
    fn search_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L0FtsResult>>;
    fn search_vector(&self, embedding: &[f32], limit: i64)
    -> AeonMemoryResult<Vec<L0SearchResult>>;
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConversationSearchItem {
    pub id: String,
    pub session_key: String,
    pub role: String,
    pub content: String,
    pub score: f64,
    pub recorded_at: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConversationSearchResult {
    pub results: Vec<ConversationSearchItem>,
    pub total: usize,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn from_fts(r: L0FtsResult) -> ConversationSearchItem {
    ConversationSearchItem {
        id: r.record_id,
        session_key: r.session_key,
        role: r.role,
        content: r.message_text,
        score: r.score,
        recorded_at: r.recorded_at,
    }
}
fn from_vector(r: L0SearchResult) -> ConversationSearchItem {
    ConversationSearchItem {
        id: r.record_id,
        session_key: r.session_key,
        role: r.role,
        content: r.message_text,
        score: r.score,
        recorded_at: r.recorded_at,
    }
}

pub fn execute_conversation_search(
    query: &str,
    limit: usize,
    session_filter: Option<&str>,
    store: Option<&dyn ConversationSearchStore>,
    embedding: Option<&dyn EmbeddingService>,
) -> ConversationSearchResult {
    if query.trim().is_empty() || store.is_none() {
        return empty("none", None);
    }
    let store = store.expect("checked above");
    let has_fts = store.is_fts_available();
    let has_embedding = embedding.is_some();
    if !has_fts && !has_embedding {
        return empty("none", Some(NO_SEARCH_CAPABILITY.to_owned()));
    }
    let multiplier = if session_filter.is_some() { 4 } else { 3 };
    let candidate_k = limit.saturating_mul(multiplier).min(i64::MAX as usize) as i64;
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
    if let Some(session) = session_filter {
        results.retain(|r| r.session_key == session);
    }
    results.truncate(limit);
    ConversationSearchResult {
        total: results.len(),
        results,
        strategy: strategy.to_owned(),
        message: None,
    }
}

fn empty(strategy: &str, message: Option<String>) -> ConversationSearchResult {
    ConversationSearchResult {
        results: Vec::new(),
        total: 0,
        strategy: strategy.to_owned(),
        message,
    }
}

pub fn format_conversation_search_response(result: &ConversationSearchResult) -> String {
    if let Some(message) = &result.message {
        return message.clone();
    }
    if result.results.is_empty() {
        return "No matching conversation messages found.".to_owned();
    }
    let mut lines = vec![
        format!("Found {} matching message(s):", result.total),
        String::new(),
    ];
    for item in &result.results {
        let date = if item.recorded_at.is_empty() {
            String::new()
        } else {
            format!(" [{}]", item.recorded_at)
        };
        lines.push("---".to_owned());
        lines.push(format!(
            "**[{}]** Session: {}{} (score: {:.3})",
            item.role, item.session_key, date, item.score
        ));
        lines.push(String::new());
        lines.push(item.content.clone());
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
            Ok(vec![0.5])
        }
        fn embed_batch(&self, _: &[String]) -> Result<Vec<Vec<f32>>, AeonMemoryCoreError> {
            unreachable!()
        }
        fn dimensions(&self) -> u32 {
            1
        }
    }
    struct Store;
    impl ConversationSearchStore for Store {
        fn is_fts_available(&self) -> bool {
            true
        }
        fn search_fts(&self, _: &str, limit: i64) -> AeonMemoryResult<Vec<L0FtsResult>> {
            assert_eq!(limit, 8);
            Ok(vec![fts("both", "wanted")])
        }
        fn search_vector(&self, _: &[f32], limit: i64) -> AeonMemoryResult<Vec<L0SearchResult>> {
            assert_eq!(limit, 8);
            Ok(vec![vector("other", "else"), vector("both", "wanted")])
        }
    }
    fn fts(id: &str, session: &str) -> L0FtsResult {
        L0FtsResult {
            record_id: id.into(),
            session_key: session.into(),
            session_id: "sid".into(),
            role: "user".into(),
            message_text: format!("text-{id}"),
            score: 0.7,
            recorded_at: "2024-01-01".into(),
            timestamp: 1,
        }
    }
    fn vector(id: &str, session: &str) -> L0SearchResult {
        let f = fts(id, session);
        L0SearchResult {
            record_id: f.record_id,
            session_key: f.session_key,
            session_id: f.session_id,
            role: f.role,
            message_text: f.message_text,
            score: f.score,
            recorded_at: f.recorded_at,
            timestamp: f.timestamp,
        }
    }
    #[test]
    fn hybrid_rrf_and_session_filter() {
        let r = execute_conversation_search("hello", 2, Some("wanted"), Some(&Store), Some(&Embed));
        assert_eq!(r.strategy, "hybrid");
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].id, "both");
        let text = format_conversation_search_response(&r);
        assert!(text.contains("Session: wanted"));
        assert!(text.contains("text-both"));
    }
}
