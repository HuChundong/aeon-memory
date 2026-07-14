//! Host-neutral port of `src/core/hooks/auto-recall.ts`.

use crate::config::{AeonMemoryConfig, RecallStrategy};
use crate::error::{AeonMemoryCoreError, AeonMemoryResult};
use crate::fts_query::build_fts_query;
use crate::scene::{generate_scene_navigation, read_scene_index, strip_scene_navigation};
use crate::search::rrf_k60;
use crate::types::{EmbeddingService, L1FtsResult, L1SearchResult, RecallResult, RecalledMemory};
use crate::utils::sanitize::sanitize_text;
use crate::utils::time::format_for_llm;
use std::path::PathBuf;
use std::sync::Arc;

const TRUNCATION_SUFFIX: &str =
    "…（已截断；可用 aeon_memory_search 或 aeon_conversation_search 查看详情）";
const MIN_TRUNCATED_LINE_CHARS: usize = 40;
pub const MEMORY_TOOLS_GUIDE: &str = r#"<memory-tools-guide>
## 记忆工具调用指南

当上方注入的记忆片段不足以回答用户问题时，可主动调用以下工具获取更多信息：

- **aeon_memory_search**：搜索结构化记忆（L1），适用于回忆用户偏好、历史事件节点、规则等关键信息。
- **aeon_conversation_search**：搜索原始对话（L0），适用于查找具体消息原文、时间线、上下文细节；也可用于补充或校验 memory_search 的结果。
- **read_file**（Scene Navigation 中的路径）：当已定位到相关情境，且需要该场景的完整画像、事件经过或阶段结论时使用。

### ⚠️ 调用次数限制
每轮对话中，aeon_memory_search 和 aeon_conversation_search **合计最多调用 3 次**。
- 首次搜索无结果时，可换关键词或换工具重试，但总调用次数不要超过 3 次。
- 若 3 次搜索后仍无结果，说明该信息不在记忆中，请直接根据已有信息回复用户，不要继续搜索。
</memory-tools-guide>"#;

pub trait RecallStore: Send + Sync {
    fn is_fts_available(&self) -> bool;
    fn search_l1_fts(&self, query: &str, limit: i64) -> AeonMemoryResult<Vec<L1FtsResult>>;
    fn search_l1_vector(
        &self,
        embedding: &[f32],
        limit: i64,
    ) -> AeonMemoryResult<Vec<L1SearchResult>>;
}

#[derive(Clone)]
pub struct AutoRecall {
    pub config: AeonMemoryConfig,
    pub data_dir: PathBuf,
    pub store: Option<Arc<dyn RecallStore>>,
    pub embedding: Option<Arc<dyn EmbeddingService>>,
}

impl AutoRecall {
    pub async fn perform(&self, user_text: &str) -> AeonMemoryResult<Option<RecallResult>> {
        let hook = self.clone();
        let text = user_text.to_owned();
        let timeout = std::time::Duration::from_millis(self.config.recall.timeout_ms);
        let result = match tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || hook.perform_inner(&text)),
        )
        .await
        {
            Ok(joined) => joined.map_err(|error| {
                AeonMemoryCoreError::Store(format!("recall task failed: {error}"))
            })??,
            Err(_) => return Ok(None),
        };
        if result.prepend_context.is_none() && result.append_system_context.is_none() {
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }

    fn perform_inner(&self, user_text: &str) -> AeonMemoryResult<RecallResult> {
        let reported_strategy = match self.config.recall.strategy {
            RecallStrategy::Keyword => "keyword",
            RecallStrategy::Embedding => "embedding",
            RecallStrategy::Hybrid => "hybrid",
        };
        let mut effective = reported_strategy;
        let clean = sanitize_text(user_text);
        let mut ranked = if clean.encode_utf16().count() < 2 {
            effective = "skipped";
            Vec::new()
        } else {
            // The TS hook treats all search-provider failures as an empty L1
            // result so stable persona/scene context remains available.
            self.search(&clean, &mut effective).unwrap_or_default()
        };
        ranked.truncate(self.config.recall.max_results as usize);
        let mut lines = ranked
            .iter()
            .map(|record| format_memory_line(&record.memory))
            .collect::<Vec<_>>();
        lines = apply_budget(
            lines,
            self.config.recall.max_chars_per_memory as usize,
            self.config.recall.max_total_recall_chars as usize,
        );
        // TS derives reporting metadata from the final, budgeted display
        // lines (including truncation) and currently reports score=0.
        let recalled = lines
            .iter()
            .map(|line| recalled_from_line(line))
            .collect::<Vec<_>>();

        let persona_path = self.data_dir.join("persona.md");
        let persona = match std::fs::read_to_string(&persona_path) {
            Ok(raw) => {
                let body = strip_scene_navigation(&raw).trim().to_owned();
                (!body.is_empty()).then_some(body)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let scenes = read_scene_index(&self.data_dir);
        let navigation =
            (!scenes.is_empty()).then(|| generate_scene_navigation(&scenes, Some(&self.data_dir)));

        let prepend_context = (!lines.is_empty()).then(|| {
            format!(
                "<relevant-memories>\n以下是当前对话召回的相关记忆，不代表当前任务进程，仅作为参考：\n\n{}\n</relevant-memories>",
                lines.join("\n")
            )
        });
        let mut stable = Vec::new();
        if let Some(body) = &persona {
            stable.push(format!("<user-persona>\n{body}\n</user-persona>"));
        }
        if let Some(nav) = navigation {
            stable.push(format!("<scene-navigation>\n{nav}\n</scene-navigation>"));
        }
        if !stable.is_empty() || prepend_context.is_some() {
            stable.push(MEMORY_TOOLS_GUIDE.to_owned());
        }
        Ok(RecallResult {
            prepend_context,
            append_system_context: (!stable.is_empty()).then(|| stable.join("\n\n")),
            recalled_l1_memories: recalled,
            recalled_l3_persona: persona,
            // TS reports the configured strategy even when searchMemories
            // internally degrades embedding/hybrid execution to keyword.
            // `skipped` is the sole reporting override for the short-query
            // gate.
            recall_strategy: if effective == "skipped" {
                "skipped"
            } else {
                reported_strategy
            }
            .to_owned(),
        })
    }

    fn search(
        &self,
        query: &str,
        effective: &mut &'static str,
    ) -> AeonMemoryResult<Vec<RankedMemory>> {
        let embedding_available = self.store.is_some() && self.embedding.is_some();
        if matches!(*effective, "embedding" | "hybrid") && !embedding_available {
            *effective = "keyword";
        }
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let max_results = i64::from(self.config.recall.max_results);
        if *effective == "keyword" {
            return keyword(
                store.as_ref(),
                query,
                max_results * 2,
                self.config.recall.score_threshold,
            );
        }
        let embedding = self.embedding.as_ref().expect("availability checked");
        let vector = embedding.embed(query)?;
        if *effective == "embedding" {
            return Ok(store
                .search_l1_vector(&vector, max_results * 2)?
                .into_iter()
                .filter(|item| item.score >= self.config.recall.score_threshold)
                .map(RankedMemory::from_vector)
                .collect());
        }
        let candidate_k = max_results * 3;
        let keywords = keyword_raw(store.as_ref(), query, candidate_k)?;
        let vectors = store.search_l1_vector(&vector, candidate_k)?;
        let keyword_ranked = keywords
            .into_iter()
            .map(RankedMemory::from_fts)
            .collect::<Vec<_>>();
        let vector_ranked = vectors
            .into_iter()
            .map(RankedMemory::from_vector)
            .collect::<Vec<_>>();
        Ok(
            rrf_k60(&[keyword_ranked, vector_ranked], |item| item.id.as_str())
                .into_iter()
                .map(|(mut item, score)| {
                    item.score = score;
                    item
                })
                .collect(),
        )
    }
}

fn recalled_from_line(line: &str) -> RecalledMemory {
    let Some(rest) = line.strip_prefix("- [") else {
        return RecalledMemory {
            content: line.to_owned(),
            score: 0.0,
            r#type: "unknown".into(),
        };
    };
    let Some((tag, content)) = rest.split_once("] ") else {
        return RecalledMemory {
            content: line.to_owned(),
            score: 0.0,
            r#type: "unknown".into(),
        };
    };
    // Mirrors TS's non-greedy content capture before an optional activity
    // suffix. The suffix itself is presentation-only metadata.
    let content = content
        .find(" (活动时间:")
        .map_or(content, |index| &content[..index])
        .trim()
        .to_owned();
    RecalledMemory {
        content,
        score: 0.0,
        r#type: tag.split('|').next().unwrap_or(tag).to_owned(),
    }
}

fn keyword(
    store: &dyn RecallStore,
    query: &str,
    limit: i64,
    threshold: f64,
) -> AeonMemoryResult<Vec<RankedMemory>> {
    let raw = keyword_raw(store, query, limit)?;
    let mut filtered = raw
        .iter()
        .filter(|item| item.score >= threshold)
        .cloned()
        .collect::<Vec<_>>();
    if filtered.is_empty() && raw.len() <= limit as usize / 2 {
        filtered = raw;
    }
    Ok(filtered.into_iter().map(RankedMemory::from_fts).collect())
}

fn keyword_raw(
    store: &dyn RecallStore,
    query: &str,
    limit: i64,
) -> AeonMemoryResult<Vec<L1FtsResult>> {
    if !store.is_fts_available() {
        return Ok(Vec::new());
    }
    let Some(fts) = build_fts_query(query) else {
        return Ok(Vec::new());
    };
    store.search_l1_fts(&fts, limit)
}

#[derive(Clone)]
struct Memory {
    r#type: String,
    content: String,
    scene_name: String,
    timestamp: String,
    metadata_json: String,
}

#[derive(Clone)]
struct RankedMemory {
    id: String,
    score: f64,
    memory: Memory,
}

impl RankedMemory {
    fn from_fts(item: L1FtsResult) -> Self {
        Self {
            id: item.record_id,
            score: item.score,
            memory: Memory {
                r#type: item.r#type,
                content: item.content,
                scene_name: item.scene_name,
                timestamp: item.timestamp_str,
                metadata_json: item.metadata_json,
            },
        }
    }
    fn from_vector(item: L1SearchResult) -> Self {
        Self {
            id: item.record_id,
            score: item.score,
            memory: Memory {
                r#type: item.r#type,
                content: item.content,
                scene_name: item.scene_name,
                timestamp: item.timestamp_str,
                metadata_json: item.metadata_json,
            },
        }
    }
}

fn format_memory_line(memory: &Memory) -> String {
    let tag = if memory.scene_name.is_empty() {
        memory.r#type.clone()
    } else {
        format!("{}|{}", memory.r#type, memory.scene_name)
    };
    let mut line = format!("- [{tag}] {}", memory.content);
    let metadata = serde_json::from_str::<serde_json::Value>(&memory.metadata_json).ok();
    let start = metadata
        .as_ref()
        .and_then(|value| value.get("activity_start_time"))
        .and_then(|value| value.as_str())
        .and_then(format_timestamp);
    let end = metadata
        .as_ref()
        .and_then(|value| value.get("activity_end_time"))
        .and_then(|value| value.as_str())
        .and_then(format_timestamp);
    let point = format_timestamp(&memory.timestamp);
    match (start, end, point) {
        (Some(a), Some(b), _) => line.push_str(&format!(" (活动时间: {a} ~ {b})")),
        (Some(a), None, _) => line.push_str(&format!(" (活动时间: {a}起)")),
        (None, Some(b), _) => line.push_str(&format!(" (活动时间: 至{b})")),
        (None, None, Some(at)) => line.push_str(&format!(" (活动时间: {at})")),
        _ => {}
    }
    line
}

fn format_timestamp(value: &str) -> Option<String> {
    if value.is_empty() || chrono::DateTime::parse_from_rfc3339(value).is_err() {
        return None;
    }
    if value.len() >= 10
        && (value.len() == 10 || value.get(11..16).is_none_or(|time| time == "00:00"))
    {
        return Some(value[..10].to_owned());
    }
    Some(format_for_llm(value))
}

fn apply_budget(lines: Vec<String>, per_line: usize, total: usize) -> Vec<String> {
    if per_line == 0 && total == 0 {
        return lines;
    }
    let mut result = Vec::new();
    let mut used = 0usize;
    for line in lines {
        let line = if per_line > 0 {
            truncate(&line, per_line)
        } else {
            line
        };
        if total == 0 {
            result.push(line);
            continue;
        }
        let separator = usize::from(!result.is_empty());
        let remaining = total.saturating_sub(used + separator);
        if remaining == 0 {
            break;
        }
        if line.encode_utf16().count() > remaining {
            if remaining >= MIN_TRUNCATED_LINE_CHARS {
                let bounded = truncate(&line, remaining);
                result.push(bounded);
            }
            break;
        }
        used += separator + line.encode_utf16().count();
        result.push(line);
    }
    result
}

fn truncate(value: &str, max: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        return value.to_owned();
    }
    let suffix = TRUNCATION_SUFFIX.chars().count();
    if max <= suffix {
        return chars[..max].iter().collect();
    }
    let prefix = chars[..max - suffix].iter().collect::<String>();
    format!("{}{TRUNCATION_SUFFIX}", prefix.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RecallStrategy;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Embed;
    impl EmbeddingService for Embed {
        fn embed(&self, _: &str) -> AeonMemoryResult<Vec<f32>> {
            Ok(vec![1.0])
        }
        fn embed_batch(&self, _: &[String]) -> AeonMemoryResult<Vec<Vec<f32>>> {
            unreachable!()
        }
        fn dimensions(&self) -> u32 {
            1
        }
    }
    struct Store;
    impl RecallStore for Store {
        fn is_fts_available(&self) -> bool {
            true
        }
        fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
            Ok(vec![
                fts("both", "keyword memory"),
                fts("fts", "fts memory"),
            ])
        }
        fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
            Ok(vec![
                vector("both", "vector memory"),
                vector("vec", "vec memory"),
            ])
        }
    }
    fn fts(id: &str, content: &str) -> L1FtsResult {
        L1FtsResult {
            record_id: id.into(),
            content: content.into(),
            r#type: "episodic".into(),
            priority: 1.0,
            scene_name: "Work".into(),
            score: 0.9,
            timestamp_str: "2026-07-13T00:00:00Z".into(),
            timestamp_start: String::new(),
            timestamp_end: String::new(),
            session_key: "s".into(),
            session_id: "i".into(),
            metadata_json: r#"{"activity_start_time":"2026-07-01"}"#.into(),
        }
    }
    fn vector(id: &str, content: &str) -> L1SearchResult {
        let item = fts(id, content);
        L1SearchResult {
            record_id: item.record_id,
            content: item.content,
            r#type: item.r#type,
            priority: item.priority,
            scene_name: item.scene_name,
            score: item.score,
            timestamp_str: item.timestamp_str,
            timestamp_start: item.timestamp_start,
            timestamp_end: item.timestamp_end,
            session_key: item.session_key,
            session_id: item.session_id,
            metadata_json: item.metadata_json,
        }
    }
    fn temp_dir() -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aeon-memory-auto-recall-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".metadata")).unwrap();
        dir
    }

    #[tokio::test]
    async fn hybrid_rrf_and_stable_dynamic_context_match_ts_semantics() {
        let dir = temp_dir();
        std::fs::write(dir.join("persona.md"), "# Persona\ncalm").unwrap();
        let mut config = AeonMemoryConfig::default();
        config.recall.strategy = RecallStrategy::Hybrid;
        config.recall.max_results = 3;
        let hook = AutoRecall {
            config,
            data_dir: dir,
            store: Some(Arc::new(Store)),
            embedding: Some(Arc::new(Embed)),
        };
        let result = hook.perform("project plan").await.unwrap().unwrap();
        let prepend = result.prepend_context.unwrap();
        assert!(prepend.contains("keyword memory"));
        assert!(prepend.starts_with("<relevant-memories>"));
        let stable = result.append_system_context.unwrap();
        assert!(stable.starts_with("<user-persona>"));
        assert!(stable.ends_with("</memory-tools-guide>"));
        assert_eq!(result.recalled_l1_memories[0].score, 0.0);
        assert_eq!(result.recall_strategy, "hybrid");
    }

    #[tokio::test]
    async fn embedding_strategy_executes_keyword_fallback_but_reports_configured_value() {
        let mut config = AeonMemoryConfig::default();
        config.recall.strategy = RecallStrategy::Embedding;
        let hook = AutoRecall {
            config,
            data_dir: temp_dir(),
            store: Some(Arc::new(Store)),
            embedding: None,
        };
        let result = hook.perform("project plan").await.unwrap().unwrap();
        assert_eq!(result.recall_strategy, "embedding");
        assert!(result.prepend_context.unwrap().contains("keyword memory"));
    }

    struct SlowStore;
    impl RecallStore for SlowStore {
        fn is_fts_available(&self) -> bool {
            true
        }
        fn search_l1_fts(&self, _: &str, _: i64) -> AeonMemoryResult<Vec<L1FtsResult>> {
            std::thread::sleep(std::time::Duration::from_millis(40));
            Ok(vec![])
        }
        fn search_l1_vector(&self, _: &[f32], _: i64) -> AeonMemoryResult<Vec<L1SearchResult>> {
            unreachable!()
        }
    }
    #[tokio::test]
    async fn timeout_skips_injection_without_blocking_turn() {
        let mut config = AeonMemoryConfig::default();
        config.recall.strategy = RecallStrategy::Keyword;
        config.recall.timeout_ms = 5;
        let hook = AutoRecall {
            config,
            data_dir: temp_dir(),
            store: Some(Arc::new(SlowStore)),
            embedding: None,
        };
        let started = std::time::Instant::now();
        assert!(hook.perform("project plan").await.unwrap().is_none());
        assert!(started.elapsed() < std::time::Duration::from_millis(30));
    }

    #[test]
    fn budget_uses_codepoints_per_line_but_js_utf16_for_total() {
        let lines = vec!["😀".repeat(100), "b".repeat(100)];
        let bounded = apply_budget(lines, 60, 101);
        assert_eq!(bounded.len(), 1);
        assert!(bounded[0].chars().count() <= 60);
        assert!(bounded[0].encode_utf16().count() > 101);
    }
}
