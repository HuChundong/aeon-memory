// port of src/core/store/search-utils.ts — RRF merge + FTS helpers.

pub const RRF_K: f64 = 60.0;

/// Merge multiple ranked lists via Reciprocal Rank Fusion (RRF).
/// Each item's RRF score = Σ(1/(k + rank + 1)) across all lists.
/// Returns items in descending RRF score order with scores attached.
/// Port of rrfMerge() from search-utils.ts:38-61
pub fn rrf_merge<T: Clone>(lists: &[Vec<T>], get_id: impl Fn(&T) -> &str, k: f64) -> Vec<(T, f64)> {
    // JavaScript Map preserves first-insertion order and Array.sort is stable.
    // Keep that ordinal explicitly: equal-score results must not inherit the
    // randomized iteration order of Rust's HashMap.
    let mut map: std::collections::HashMap<String, (T, f64, usize)> =
        std::collections::HashMap::new();
    let mut next_ordinal = 0usize;

    for list in lists {
        for (rank, item) in list.iter().enumerate() {
            let id = get_id(item).to_string();
            let score = 1.0 / (k + (rank as f64) + 1.0);
            let entry = map.entry(id);
            entry
                .and_modify(|(_, s, _)| *s += score)
                .or_insert_with(|| {
                    let ordinal = next_ordinal;
                    next_ordinal += 1;
                    (item.clone(), score, ordinal)
                });
        }
    }

    let mut result: Vec<(T, f64, usize)> = map.into_values().collect();
    result.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.2.cmp(&b.2))
    });
    result
        .into_iter()
        .map(|(item, score, _)| (item, score))
        .collect()
}

/// Standard RRF constant (matches TS RRF_K = 60).
pub fn rrf_k60<T: Clone>(lists: &[Vec<T>], get_id: impl Fn(&T) -> &str) -> Vec<(T, f64)> {
    rrf_merge(lists, get_id, RRF_K)
}

/// BM25 rank to 0–1 score conversion (port of bm25RankToScore from sqlite.ts:290-297)
pub fn bm25_rank_to_score(rank: f64) -> f64 {
    if !rank.is_finite() {
        return 1.0 / (1.0 + 999.0);
    }
    if rank < 0.0 {
        let relevance = -rank;
        relevance / (1.0 + relevance)
    } else {
        1.0 / (1.0 + rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_empty() {
        let result: Vec<(String, f64)> = rrf_k60(&[], |s| s.as_str());
        assert!(result.is_empty());
    }

    #[test]
    fn test_rrf_single_list() {
        let list = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = rrf_k60(&[list], |s| s.as_str());
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, "a");
        assert!(result[0].1 > result[1].1);
    }

    #[test]
    fn test_rrf_merge_duplicates() {
        let list1 = vec!["a".to_string(), "b".to_string()];
        let list2 = vec!["b".to_string(), "c".to_string()];
        let result = rrf_k60(&[list1, list2], |s| s.as_str());
        // "b" appears in both lists, so its score > others
        assert_eq!(result.len(), 3);
        assert_eq!(
            result[0].0, "b",
            "b should rank first (appears in both lists)"
        );
    }

    #[test]
    fn test_rrf_custom_k() {
        let list = vec!["a".to_string()];
        let result = rrf_merge(&[list], |s| s.as_str(), 1.0);
        assert!((result[0].1 - 0.5).abs() < 0.001);
    }

    #[test]
    fn equal_scores_keep_typescript_first_seen_order() {
        let list = vec!["first".to_string(), "second".to_string()];
        let result = rrf_merge(&[list, vec!["second".into(), "first".into()]], |s| s, 60.0);
        assert_eq!(
            result.into_iter().map(|(item, _)| item).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn test_bm25_rank_conversion() {
        let score = bm25_rank_to_score(-5.0);
        assert!((score - 5.0 / 6.0).abs() < 0.001);

        let score = bm25_rank_to_score(10.0);
        assert!((score - 1.0 / 11.0).abs() < 0.001);
    }
}
