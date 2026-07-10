//! Pure scoring functions for the search quality benchmark.
//!
//! Retrievers produce a ranked, document-level list per query; these functions
//! score that list against the query's relevance labels. Labels are matched by
//! document ID when the golden set provides IDs, falling back to exact title
//! for older files that only carry titles.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

/// A document in a retriever's ranked result list. One entry per document
/// (retrievers dedupe chunk-level hits before scoring).
#[derive(Debug, Clone)]
pub struct RankedDoc {
    pub document_id: String,
    /// Relevance score, for retrievers that produce one (semantic). None for
    /// modes with no ranking signal (FTS ordered by recency).
    pub score: Option<f32>,
}

/// How a query's relevance labels are matched against result documents.
pub enum LabelMatcher<'a> {
    /// Match by document ID.
    Ids(HashSet<&'a str>),
    /// Match by exact title via a document-id-to-title map.
    Titles {
        titles: HashSet<&'a str>,
        title_map: &'a HashMap<String, String>,
    },
}

impl LabelMatcher<'_> {
    /// The label (ID or title) a result document covers, or None if the
    /// document is not relevant to the query.
    pub fn label_for(&self, doc_id: &str) -> Option<&str> {
        match self {
            LabelMatcher::Ids(ids) => ids.get(doc_id).copied(),
            LabelMatcher::Titles { titles, title_map } => title_map
                .get(doc_id)
                .and_then(|title| titles.get(title.as_str()).copied()),
        }
    }

    /// Number of labels in the query's full label list.
    pub fn label_count(&self) -> usize {
        match self {
            LabelMatcher::Ids(ids) => ids.len(),
            LabelMatcher::Titles { titles, .. } => titles.len(),
        }
    }

    /// Matching method name for reporting ("id" or "title").
    pub fn method(&self) -> &'static str {
        match self {
            LabelMatcher::Ids(_) => "id",
            LabelMatcher::Titles { .. } => "title",
        }
    }
}

/// Scores for a single query against a single retriever's ranked list.
#[derive(Debug, Clone, Serialize)]
pub struct QueryScore {
    /// 1-indexed rank of the first relevant document over the full ranked
    /// list (may exceed k).
    pub best_rank: Option<usize>,
    pub best_score: Option<f32>,
    pub found_in_top_k: bool,
    /// Distinct labels covered within the top k results.
    pub labels_found_at_k: usize,
    pub label_count: usize,
    pub recall_at_k: f64,
    /// 1/best_rank when the first relevant document is within the top k,
    /// otherwise 0 (MRR@k).
    pub reciprocal_rank: f64,
}

/// Score one query's ranked document list against its labels.
///
/// Precondition: the caller validates that the label list is non-empty.
pub fn score_query(ranked: &[RankedDoc], matcher: &LabelMatcher, k: usize) -> QueryScore {
    let mut best_rank: Option<usize> = None;
    let mut best_score: Option<f32> = None;
    let mut labels_covered: HashSet<&str> = HashSet::new();

    for (i, doc) in ranked.iter().enumerate() {
        if let Some(label) = matcher.label_for(&doc.document_id) {
            if best_rank.is_none() {
                best_rank = Some(i + 1);
                best_score = doc.score;
            }
            if i < k {
                labels_covered.insert(label);
            }
        }
    }

    let label_count = matcher.label_count();
    let found_in_top_k = best_rank.is_some_and(|r| r <= k);

    QueryScore {
        best_rank,
        best_score,
        found_in_top_k,
        labels_found_at_k: labels_covered.len(),
        label_count,
        recall_at_k: labels_covered.len() as f64 / label_count as f64,
        reciprocal_rank: match best_rank {
            Some(rank) if rank <= k => 1.0 / rank as f64,
            _ => 0.0,
        },
    }
}

/// Aggregate metrics over a set of scored queries.
#[derive(Debug, Clone, Serialize)]
pub struct AggregateMetrics {
    pub n: usize,
    pub queries_with_match: usize,
    pub hit_rate_at_k: f64,
    pub recall_at_k: f64,
    pub mrr: f64,
}

pub fn aggregate<'a>(scores: impl IntoIterator<Item = &'a QueryScore>) -> AggregateMetrics {
    let scores: Vec<&QueryScore> = scores.into_iter().collect();
    let n = scores.len();
    let queries_with_match = scores.iter().filter(|s| s.found_in_top_k).count();
    let recall_sum: f64 = scores.iter().map(|s| s.recall_at_k).sum();
    let rr_sum: f64 = scores.iter().map(|s| s.reciprocal_rank).sum();

    AggregateMetrics {
        n,
        queries_with_match,
        hit_rate_at_k: queries_with_match as f64 / n as f64,
        recall_at_k: recall_sum / n as f64,
        mrr: rr_sum / n as f64,
    }
}

/// Group scores by stratum name and aggregate each group. Queries without a
/// stratum label should be passed as "unlabeled" by the caller.
pub fn aggregate_by_stratum(strata: &[(&str, &QueryScore)]) -> BTreeMap<String, AggregateMetrics> {
    let mut groups: BTreeMap<&str, Vec<&QueryScore>> = BTreeMap::new();
    for (stratum, score) in strata {
        groups.entry(stratum).or_default().push(score);
    }
    groups
        .into_iter()
        .map(|(stratum, scores)| (stratum.to_string(), aggregate(scores.into_iter())))
        .collect()
}

/// Pairwise win/loss/tie between two modes' per-query best ranks.
#[derive(Debug, Clone, Serialize)]
pub struct WinLossTie {
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
}

/// Compare per-query best ranks of mode A against mode B. A lower rank wins;
/// a missing rank always loses to a present one; equal ranks (or both
/// missing) tie. Slices must be index-aligned by query.
pub fn compare_ranks(a: &[Option<usize>], b: &[Option<usize>]) -> WinLossTie {
    debug_assert_eq!(a.len(), b.len());
    let mut wlt = WinLossTie {
        wins: 0,
        losses: 0,
        ties: 0,
    };
    for (ra, rb) in a.iter().zip(b) {
        match (ra, rb) {
            (Some(ra), Some(rb)) if ra < rb => wlt.wins += 1,
            (Some(ra), Some(rb)) if ra > rb => wlt.losses += 1,
            (Some(_), None) => wlt.wins += 1,
            (None, Some(_)) => wlt.losses += 1,
            _ => wlt.ties += 1,
        }
    }
    wlt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(ids: &[&str]) -> Vec<RankedDoc> {
        ids.iter()
            .map(|id| RankedDoc {
                document_id: id.to_string(),
                score: None,
            })
            .collect()
    }

    fn id_matcher<'a>(ids: &[&'a str]) -> LabelMatcher<'a> {
        LabelMatcher::Ids(ids.iter().copied().collect())
    }

    #[test]
    fn score_query_finds_first_relevant_by_id() {
        let ranked = ranked(&["d1", "d2", "d3"]);
        let matcher = id_matcher(&["d2", "d9"]);
        let score = score_query(&ranked, &matcher, 10);

        assert_eq!(score.best_rank, Some(2));
        assert!(score.found_in_top_k);
        assert_eq!(score.labels_found_at_k, 1);
        assert_eq!(score.label_count, 2);
        assert!((score.recall_at_k - 0.5).abs() < 1e-9);
        assert!((score.reciprocal_rank - 0.5).abs() < 1e-9);
    }

    #[test]
    fn score_query_rank_beyond_k_is_reported_but_not_credited() {
        let ranked = ranked(&["d1", "d2", "d3", "d4", "d5"]);
        let matcher = id_matcher(&["d4"]);
        let score = score_query(&ranked, &matcher, 3);

        assert_eq!(score.best_rank, Some(4));
        assert!(!score.found_in_top_k);
        assert_eq!(score.labels_found_at_k, 0);
        assert_eq!(score.recall_at_k, 0.0);
        assert_eq!(score.reciprocal_rank, 0.0);
    }

    #[test]
    fn score_query_no_match() {
        let ranked = ranked(&["d1", "d2"]);
        let matcher = id_matcher(&["d9"]);
        let score = score_query(&ranked, &matcher, 10);

        assert_eq!(score.best_rank, None);
        assert!(!score.found_in_top_k);
        assert_eq!(score.recall_at_k, 0.0);
        assert_eq!(score.reciprocal_rank, 0.0);
    }

    #[test]
    fn score_query_recall_counts_distinct_labels() {
        let ranked = ranked(&["a", "b", "x", "y"]);
        let matcher = id_matcher(&["a", "b", "c"]);
        let score = score_query(&ranked, &matcher, 4);

        assert_eq!(score.labels_found_at_k, 2);
        assert!((score.recall_at_k - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(score.best_rank, Some(1));
        assert!((score.reciprocal_rank - 1.0).abs() < 1e-9);
    }

    #[test]
    fn score_query_propagates_best_score() {
        let ranked = vec![
            RankedDoc {
                document_id: "d1".into(),
                score: Some(0.9),
            },
            RankedDoc {
                document_id: "d2".into(),
                score: Some(0.8),
            },
        ];
        let matcher = id_matcher(&["d2"]);
        let score = score_query(&ranked, &matcher, 10);

        assert_eq!(score.best_score, Some(0.8));
    }

    #[test]
    fn title_matching_covers_one_label_across_recurring_docs() {
        // Two documents share the "Weekly Sync" title (recurring meeting).
        let title_map: HashMap<String, String> = [
            ("d1".to_string(), "Weekly Sync".to_string()),
            ("d2".to_string(), "Weekly Sync".to_string()),
            ("d3".to_string(), "Planning".to_string()),
        ]
        .into();
        let titles: HashSet<&str> = ["Weekly Sync"].into();
        let matcher = LabelMatcher::Titles {
            titles,
            title_map: &title_map,
        };

        let ranked = ranked(&["d3", "d1", "d2"]);
        let score = score_query(&ranked, &matcher, 3);

        assert_eq!(score.best_rank, Some(2));
        // Both d1 and d2 cover the same title label: counted once.
        assert_eq!(score.labels_found_at_k, 1);
        assert_eq!(score.label_count, 1);
        assert!((score.recall_at_k - 1.0).abs() < 1e-9);
    }

    #[test]
    fn matcher_methods() {
        let ids = id_matcher(&["a", "b"]);
        assert_eq!(ids.method(), "id");
        assert_eq!(ids.label_count(), 2);
        assert_eq!(ids.label_for("a"), Some("a"));
        assert_eq!(ids.label_for("z"), None);

        let title_map: HashMap<String, String> =
            [("d1".to_string(), "Weekly Sync".to_string())].into();
        let titles = LabelMatcher::Titles {
            titles: ["Weekly Sync"].into(),
            title_map: &title_map,
        };
        assert_eq!(titles.method(), "title");
        assert_eq!(titles.label_count(), 1);
        assert_eq!(titles.label_for("d1"), Some("Weekly Sync"));
        assert_eq!(titles.label_for("d2"), None);
    }

    fn qs(best_rank: Option<usize>, k: usize, recall: f64) -> QueryScore {
        let found = best_rank.is_some_and(|r| r <= k);
        QueryScore {
            best_rank,
            best_score: None,
            found_in_top_k: found,
            labels_found_at_k: 0,
            label_count: 1,
            recall_at_k: recall,
            reciprocal_rank: if found {
                1.0 / best_rank.unwrap() as f64
            } else {
                0.0
            },
        }
    }

    #[test]
    fn aggregate_means() {
        let scores = vec![
            qs(Some(1), 10, 1.0),
            qs(Some(4), 10, 0.5),
            qs(None, 10, 0.0),
            qs(Some(20), 10, 0.0), // beyond k: no hit, no rr credit
        ];
        let agg = aggregate(&scores);

        assert_eq!(agg.n, 4);
        assert_eq!(agg.queries_with_match, 2);
        assert!((agg.hit_rate_at_k - 0.5).abs() < 1e-9);
        assert!((agg.recall_at_k - 0.375).abs() < 1e-9);
        assert!((agg.mrr - (1.0 + 0.25) / 4.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_by_stratum_groups() {
        let a = qs(Some(1), 10, 1.0);
        let b = qs(None, 10, 0.0);
        let c = qs(Some(2), 10, 1.0);
        let strata = vec![
            ("exact-term", &a),
            ("paraphrase", &b),
            ("exact-term", &c),
        ];
        let by = aggregate_by_stratum(&strata);

        assert_eq!(by.len(), 2);
        let exact = &by["exact-term"];
        assert_eq!(exact.n, 2);
        assert_eq!(exact.queries_with_match, 2);
        assert!((exact.mrr - 0.75).abs() < 1e-9);
        let para = &by["paraphrase"];
        assert_eq!(para.n, 1);
        assert_eq!(para.queries_with_match, 0);
    }

    #[test]
    fn compare_ranks_win_loss_tie() {
        let a = vec![Some(1), Some(5), None, Some(3), None];
        let b = vec![Some(2), Some(5), Some(9), None, None];
        let wlt = compare_ranks(&a, &b);

        // a wins q1 (1<2) and q4 (Some beats None); loses q3; ties q2 and q5.
        assert_eq!(wlt.wins, 2);
        assert_eq!(wlt.losses, 1);
        assert_eq!(wlt.ties, 2);
    }
}
