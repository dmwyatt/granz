//! Reciprocal rank fusion (RRF) for combining ranked retrieval lists.
//!
//! RRF fuses lists by rank alone, which sidesteps the incomparability of
//! bm25 and cosine scores: `score(d) = Σ 1/(k + rank_i(d))` over every list
//! that contains `d`, with 1-based ranks.

/// RRF constant. 60 is the standard value from Cormack et al. (2009) and
/// works well without per-corpus tuning.
pub const RRF_K: f64 = 60.0;

/// A document with its fused relevance score (higher is better).
#[derive(Debug, Clone, PartialEq)]
pub struct FusedDoc {
    pub document_id: String,
    pub score: f64,
}

/// Fuse ranked document-id lists (best first) with reciprocal rank fusion.
///
/// A duplicate occurrence of a document within one list is ignored; only its
/// best rank in that list contributes. Results sort by score descending, with
/// exact ties broken by the document's best rank across lists, then by
/// document id so the order is deterministic.
pub fn reciprocal_rank_fusion(lists: &[Vec<String>], k: f64) -> Vec<FusedDoc> {
    use std::collections::hash_map::Entry;
    use std::collections::{HashMap, HashSet};

    // doc id -> (accumulated score, best rank seen in any list)
    let mut scores: HashMap<&str, (f64, usize)> = HashMap::new();

    for list in lists {
        let mut seen_in_list: HashSet<&str> = HashSet::new();
        for (i, doc_id) in list.iter().enumerate() {
            if !seen_in_list.insert(doc_id) {
                continue;
            }
            let rank = i + 1;
            let contribution = 1.0 / (k + rank as f64);
            match scores.entry(doc_id) {
                Entry::Occupied(mut e) => {
                    let (score, best_rank) = e.get_mut();
                    *score += contribution;
                    *best_rank = (*best_rank).min(rank);
                }
                Entry::Vacant(e) => {
                    e.insert((contribution, rank));
                }
            }
        }
    }

    let mut fused: Vec<(&str, f64, usize)> = scores
        .into_iter()
        .map(|(doc_id, (score, best_rank))| (doc_id, score, best_rank))
        .collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(b.0))
    });

    fused
        .into_iter()
        .map(|(doc_id, score, _)| FusedDoc {
            document_id: doc_id.to_string(),
            score,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(docs: &[FusedDoc]) -> Vec<&str> {
        docs.iter().map(|d| d.document_id.as_str()).collect()
    }

    #[test]
    fn doc_in_both_lists_outranks_single_list_docs() {
        let lists = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["b".to_string(), "c".to_string()],
        ];
        let fused = reciprocal_rank_fusion(&lists, RRF_K);
        assert_eq!(ids(&fused), vec!["b", "a", "c"]);
    }

    #[test]
    fn score_is_reciprocal_of_k_plus_one_based_rank() {
        let lists = vec![vec!["x".to_string(), "y".to_string()]];
        let fused = reciprocal_rank_fusion(&lists, 60.0);
        assert_eq!(fused[0].document_id, "x");
        assert!((fused[0].score - 1.0 / 61.0).abs() < 1e-12);
        assert!((fused[1].score - 1.0 / 62.0).abs() < 1e-12);
    }

    #[test]
    fn empty_lists_fuse_to_empty() {
        let fused = reciprocal_rank_fusion(&[Vec::new(), Vec::new()], RRF_K);
        assert!(fused.is_empty());
    }

    #[test]
    fn no_lists_fuse_to_empty() {
        let fused = reciprocal_rank_fusion(&[], RRF_K);
        assert!(fused.is_empty());
    }

    #[test]
    fn duplicate_within_one_list_keeps_best_rank_only() {
        let lists = vec![vec!["a".to_string(), "a".to_string(), "b".to_string()]];
        let fused = reciprocal_rank_fusion(&lists, 60.0);
        assert_eq!(fused.len(), 2);
        // "a" scores as rank 1 only, not rank 1 + rank 2.
        assert!((fused[0].score - 1.0 / 61.0).abs() < 1e-12);
        // "b" still ranks 3rd in the input list.
        assert!((fused[1].score - 1.0 / 63.0).abs() < 1e-12);
    }

    #[test]
    fn single_list_order_is_preserved() {
        let lists = vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]];
        let fused = reciprocal_rank_fusion(&lists, RRF_K);
        assert_eq!(ids(&fused), vec!["a", "b", "c"]);
    }

    #[test]
    fn equal_scores_break_ties_by_best_rank_then_id() {
        // With k=0: "y" scores 1/2 + 1/2 = 1.0 across two lists, "z" scores
        // 1/1 = 1.0 in one. Equal scores, but "z" holds the better single
        // rank, so it wins even though "y" < "z" as an id. "pad" scores 2.0
        // and stays clear of the tie.
        let lists = vec![
            vec!["z".to_string()],
            vec!["pad".to_string(), "y".to_string()],
            vec!["pad".to_string(), "y".to_string()],
        ];
        let fused = reciprocal_rank_fusion(&lists, 0.0);
        assert_eq!(ids(&fused), vec!["pad", "z", "y"]);

        // Identical score and best rank: falls back to document id.
        let lists = vec![vec!["b".to_string()], vec!["a".to_string()]];
        let fused = reciprocal_rank_fusion(&lists, RRF_K);
        assert_eq!(ids(&fused), vec!["a", "b"]);
    }
}
