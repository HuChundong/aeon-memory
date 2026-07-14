// port of src/core/store/sqlite.ts:198-230 (buildFtsQuery) + src/utils/sanitize.ts ZH_STOP_WORDS
// Uses jieba-rs for Chinese word segmentation, with pure-unicode fallback.

use jieba_rs::Jieba;
use std::sync::LazyLock;

#[cfg(test)]
static JIEBA_INITIALIZATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

static JIEBA: LazyLock<Jieba> = LazyLock::new(|| {
    #[cfg(test)]
    JIEBA_INITIALIZATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Jieba::new()
});

fn jieba() -> &'static Jieba {
    &JIEBA
}

const ZH_STOP_WORDS: &[&str] = &[
    "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一", "一个", "上", "也",
    "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好", "自己", "这", "他", "她",
    "它", "们", "那", "吗", "吧", "呢", "啊", "呀", "哦", "嗯",
];

/// Build an FTS5 MATCH query from raw text. Port of buildFtsQuery() in sqlite.ts:198-230.
/// Uses jieba for Chinese segmentation; jieba is always available in this crate.
pub fn build_fts_query(raw: &str) -> Option<String> {
    let tokens: Vec<String> = jieba()
        .cut_for_search(raw, true)
        .iter()
        .map(|t| (*t.word).to_string())
        .filter(|t| {
            if t.is_empty() {
                return false;
            }
            if !t.chars().any(|c| c.is_alphanumeric()) {
                return false;
            }
            if ZH_STOP_WORDS.contains(&t.as_str()) {
                return false;
            }
            true
        })
        .collect();

    // Deduplicate
    let mut unique: Vec<String> = Vec::new();
    for t in tokens {
        if !unique.contains(&t) {
            unique.push(t);
        }
    }

    // The TypeScript implementation only uses its Unicode-regex fallback when
    // jieba is unavailable. Rust always has jieba-rs here, so an empty token
    // set (including a query made solely of stop words) must remain empty.
    if unique.is_empty() {
        return None;
    }

    let quoted: Vec<String> = unique
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    Some(quoted.join(" OR "))
}

/// Tokenize text for FTS5 indexing (write-side). Port of tokenizeForFts().
pub fn tokenize_for_fts(raw: &str) -> String {
    let tokens: Vec<String> = jieba()
        .cut_for_search(raw, true)
        .iter()
        .map(|t| (*t.word).to_string())
        .collect();
    tokens.join(" ")
}

/// Score cutoff for search results (matches TS scoreThreshold default of 0.3)
pub const DEFAULT_SCORE_THRESHOLD: f64 = 0.3;

/// Apply type filter to L1 search results
pub fn apply_type_filter<'a>(
    items: &'a [MemorySearchItem],
    type_filter: &str,
) -> Vec<&'a MemorySearchItem> {
    items.iter().filter(|i| i.r#type == type_filter).collect()
}

/// Apply scene filter to L1 search results (case-insensitive contains)
pub fn apply_scene_filter<'a>(
    items: &'a [MemorySearchItem],
    scene_filter: &str,
) -> Vec<&'a MemorySearchItem> {
    let lower = scene_filter.to_lowercase();
    items
        .iter()
        .filter(|i| i.scene_name.to_lowercase().contains(&lower))
        .collect()
}

use crate::types::{L1FtsResult, L1SearchResult};

#[derive(Clone, Debug)]
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

pub struct MemorySearchResult {
    pub results: Vec<MemorySearchItem>,
    pub total: u32,
    pub strategy: String,
    pub message: Option<String>,
}

impl MemorySearchItem {
    pub fn from_fts(r: &L1FtsResult) -> Self {
        Self {
            id: r.record_id.clone(),
            content: r.content.clone(),
            r#type: r.r#type.clone(),
            priority: r.priority,
            scene_name: r.scene_name.clone(),
            score: r.score,
            created_at: r.timestamp_start.clone(),
            updated_at: r.timestamp_end.clone(),
        }
    }
    pub fn from_vector(r: &L1SearchResult) -> Self {
        Self {
            id: r.record_id.clone(),
            content: r.content.clone(),
            r#type: r.r#type.clone(),
            priority: r.priority,
            scene_name: r.scene_name.clone(),
            score: r.score,
            created_at: r.timestamp_start.clone(),
            updated_at: r.timestamp_end.clone(),
        }
    }
}

pub fn format_search_response(result: &MemorySearchResult) -> String {
    if let Some(ref msg) = result.message {
        return msg.clone();
    }
    if result.results.is_empty() {
        return "No matching memories found.".to_string();
    }

    let mut lines = vec![
        format!("Found {} matching memories:", result.total),
        String::new(),
    ];
    for item in &result.results {
        let score_str = format!(" (score: {:.3})", item.score);
        let scene_str = if !item.scene_name.is_empty() {
            format!(" [scene: {}]", item.scene_name)
        } else {
            String::new()
        };
        let priority_str = if item.priority >= 0.0 {
            format!(" (priority: {})", item.priority)
        } else {
            " (global instruction)".to_string()
        };
        lines.push(format!(
            "- **[{}]**{}{}{}",
            item.r#type, priority_str, scene_str, score_str
        ));
        lines.push(format!("  {}", item.content));
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_fts_query() {
        let q = build_fts_query("hello world");
        assert!(q.is_some());
        let q = q.unwrap();
        assert!(q.contains("hello"));
        assert!(q.contains("world"));
        assert!(q.contains("OR"));
    }

    #[test]
    fn test_build_fts_query_chinese() {
        let q = build_fts_query("用户喜欢编程");
        assert!(q.is_some(), "Query should produce tokens");
        let q = q.unwrap();
        assert!(q.contains("编程"), "Should contain '编程' token: {}", q);
        assert!(q.contains('"'), "Should have quoted tokens");
    }

    #[test]
    fn test_build_fts_empty() {
        assert!(build_fts_query("").is_none());
        assert!(build_fts_query("   ").is_none());
    }

    #[test]
    fn test_build_fts_pure_stop_words_matches_ts() {
        assert_eq!(build_fts_query("的 了 和"), None);
    }

    #[test]
    fn test_tokenize_for_fts() {
        let t = tokenize_for_fts("用户喜欢编程");
        assert!(t.contains("编程"));
    }

    #[test]
    fn process_wide_jieba_is_initialized_once_across_query_and_index_paths() {
        let workers = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..4 {
                        assert!(tokenize_for_fts("用户希望优化数据库查询性能").contains("数据库"));
                        assert!(build_fts_query("数据库查询优化").is_some());
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            JIEBA_INITIALIZATIONS.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "query and index tokenization must share one process-wide Jieba dictionary"
        );
    }

    #[test]
    fn test_format_search_empty() {
        let r = MemorySearchResult {
            results: vec![],
            total: 0,
            strategy: "none".into(),
            message: None,
        };
        assert_eq!(format_search_response(&r), "No matching memories found.");
    }

    #[test]
    fn test_apply_type_filter() {
        let items = vec![
            MemorySearchItem {
                id: "1".into(),
                content: "a".into(),
                r#type: "persona".into(),
                priority: 50.0,
                scene_name: "".into(),
                score: 1.0,
                created_at: "".into(),
                updated_at: "".into(),
            },
            MemorySearchItem {
                id: "2".into(),
                content: "b".into(),
                r#type: "episodic".into(),
                priority: 50.0,
                scene_name: "".into(),
                score: 1.0,
                created_at: "".into(),
                updated_at: "".into(),
            },
        ];
        let filtered = apply_type_filter(&items, "persona");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "1");
    }
}
