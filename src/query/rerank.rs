//! Cross-encoder rerank stage for hybrid search.
//!
//! Takes the top fused candidates from [`crate::query::hybrid`], builds a
//! (title + best chunk) passage per document, and reorders them by
//! cross-encoder relevance blended with the RRF fusion prior (see
//! [`FUSION_BLEND_WEIGHT`]). The reranker probability is the user-facing
//! score for hybrid results; the blend affects ordering only.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::embed::rerank::Reranker;
use crate::query::hybrid::HybridRanking;

/// How many top fused candidates the reranker scores. Documents fused
/// below this cutoff are dropped from reranked results.
pub const RERANK_POOL: usize = 50;

/// Weight of the RRF fusion score in the final ranking, blended as
/// `rerank_score + FUSION_BLEND_WEIGHT * fused_score`. RRF scores top out
/// near 2/(60+1), so 30 scales the prior to roughly the cross-encoder's
/// [0, 1] range. Without the prior, queries where the cross-encoder is
/// unconfident (max score well under 0.8) get noise-dominated orderings
/// that can bury documents fusion ranked highly; the sweep on the 93-query
/// golden set picked 30 (hit-rate@10 0.935, MRR@10 0.804, no fusion top-3
/// document leaving the top 10).
pub const FUSION_BLEND_WEIGHT: f32 = 30.0;

/// A reranked document with its relevance score.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankedDoc {
    pub document_id: String,
    pub score: f32,
}

/// One reranked candidate with the components that produced its position:
/// where fusion placed it, its RRF score, and the passage the cross-encoder
/// judged. Serialized by the benchmark's --dump-candidates output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RerankCandidate {
    pub document_id: String,
    /// 1-based rank in the fused list.
    pub fused_rank: usize,
    /// RRF score from fusion.
    pub fused_score: f64,
    pub passage: String,
    pub rerank_score: f32,
}

impl RerankCandidate {
    /// The ranking score: cross-encoder probability plus the weighted
    /// fusion prior. Ordering only; the user-facing score stays
    /// [`RerankCandidate::rerank_score`].
    pub fn blended_score(&self) -> f32 {
        self.rerank_score + FUSION_BLEND_WEIGHT * self.fused_score as f32
    }
}

/// Build the passage the reranker judges for one document: the title and
/// the best-matching chunk, whichever exist.
fn build_passage(title: Option<&str>, best_chunk: Option<&str>) -> String {
    match (title, best_chunk) {
        (Some(t), Some(c)) => format!("{t}\n\n{c}"),
        (Some(t), None) => t.to_string(),
        (None, Some(c)) => c.to_string(),
        (None, None) => String::new(),
    }
}

/// Rerank the top [`RERANK_POOL`] fused candidates of a hybrid ranking,
/// keeping each candidate's fusion components: fetch titles, build
/// passages, score with the cross-encoder. Sorted best-first by blended
/// score; ties keep the fused order.
pub fn rerank_hybrid_detailed(
    conn: &Connection,
    reranker: &dyn Reranker,
    query: &str,
    ranking: &HybridRanking,
) -> Result<Vec<RerankCandidate>> {
    let pool = &ranking.fused[..ranking.fused.len().min(RERANK_POOL)];
    let pool_ids: Vec<String> = pool.iter().map(|d| d.document_id.clone()).collect();
    let docs = crate::db::meetings::get_meetings_by_ids(conn, &pool_ids)?;
    let titles: HashMap<String, Option<String>> =
        docs.into_iter().filter_map(|doc| doc.id.map(|id| (id, doc.title))).collect();

    // Candidates are built in fused order so the stable sort below keeps
    // that order for ties; documents missing from the db drop out.
    let mut candidates: Vec<RerankCandidate> = pool
        .iter()
        .enumerate()
        .filter_map(|(i, fused)| {
            let title = titles.get(&fused.document_id)?;
            Some(RerankCandidate {
                document_id: fused.document_id.clone(),
                fused_rank: i + 1,
                fused_score: fused.score,
                passage: build_passage(
                    title.as_deref(),
                    ranking.best_chunks.get(&fused.document_id).map(String::as_str),
                ),
                rerank_score: 0.0,
            })
        })
        .collect();

    let passages: Vec<&str> = candidates.iter().map(|c| c.passage.as_str()).collect();
    let scores = reranker.rerank(query, &passages)?;
    for (candidate, score) in candidates.iter_mut().zip(scores) {
        candidate.rerank_score = score;
    }
    candidates.sort_by(|a, b| {
        b.blended_score().partial_cmp(&a.blended_score()).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(candidates)
}

/// Rerank the top [`RERANK_POOL`] fused candidates of a hybrid ranking:
/// fetch titles, build passages, score with the cross-encoder.
pub fn rerank_hybrid(
    conn: &Connection,
    reranker: &dyn Reranker,
    query: &str,
    ranking: &HybridRanking,
) -> Result<Vec<RerankedDoc>> {
    Ok(rerank_hybrid_detailed(conn, reranker, query, ranking)?
        .into_iter()
        .map(|c| RerankedDoc { document_id: c.document_id, score: c.rerank_score })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use crate::embed::rerank::MockReranker;
    use crate::query::fusion::FusedDoc;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn passage_combines_title_and_chunk() {
        assert_eq!(build_passage(Some("Title"), Some("chunk text")), "Title\n\nchunk text");
        assert_eq!(build_passage(Some("Title"), None), "Title");
        assert_eq!(build_passage(None, Some("chunk text")), "chunk text");
        assert_eq!(build_passage(None, None), "");
    }

    fn fused(ids: &[&str]) -> Vec<FusedDoc> {
        ids.iter()
            .map(|id| FusedDoc { document_id: id.to_string(), score: 0.0 })
            .collect()
    }

    #[test]
    fn hybrid_rerank_scores_titles_and_chunks() {
        // doc-chunk matches the query twice via its chunk; doc-title once
        // via its title; doc-none not at all. Fused order is the reverse
        // of relevance so reordering is observable.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-none": {"id": "doc-none", "title": "Unrelated", "created_at": "2026-01-01T10:00:00Z"},
                "doc-title": {"id": "doc-title", "title": "Kumquat sync", "created_at": "2026-01-02T10:00:00Z"},
                "doc-chunk": {"id": "doc-chunk", "title": "Planning", "created_at": "2026-01-03T10:00:00Z"}
            }
        }));
        let ranking = HybridRanking {
            fused: fused(&["doc-none", "doc-title", "doc-chunk"]),
            best_chunks: HashMap::from([(
                "doc-chunk".to_string(),
                "kumquat kumquat".to_string(),
            )]),
        };

        let reranked = rerank_hybrid(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        let ids: Vec<&str> = reranked.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-chunk", "doc-title", "doc-none"]);
    }

    #[test]
    fn detailed_carries_fusion_components_and_sorts_by_blended_score() {
        // Fused order is the reverse of cross-encoder relevance: doc-chunk
        // matches "kumquat" twice via its chunk, doc-title once via its
        // title, doc-none not at all.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-none": {"id": "doc-none", "title": "Unrelated", "created_at": "2026-01-01T10:00:00Z"},
                "doc-title": {"id": "doc-title", "title": "Kumquat sync", "created_at": "2026-01-02T10:00:00Z"},
                "doc-chunk": {"id": "doc-chunk", "title": "Planning", "created_at": "2026-01-03T10:00:00Z"}
            }
        }));
        let ranking = HybridRanking {
            fused: vec![
                FusedDoc { document_id: "doc-none".to_string(), score: 0.03 },
                FusedDoc { document_id: "doc-title".to_string(), score: 0.02 },
                FusedDoc { document_id: "doc-chunk".to_string(), score: 0.01 },
            ],
            best_chunks: HashMap::from([(
                "doc-chunk".to_string(),
                "kumquat kumquat".to_string(),
            )]),
        };

        let detailed =
            rerank_hybrid_detailed(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        assert_eq!(detailed.len(), 3);
        assert_eq!(detailed[0].document_id, "doc-chunk");
        assert_eq!(detailed[0].fused_rank, 3);
        assert_eq!(detailed[0].fused_score, 0.01);
        assert_eq!(detailed[0].passage, "Planning\n\nkumquat kumquat");
        assert_eq!(detailed[0].rerank_score, 2.0);
        assert_eq!(detailed[1].document_id, "doc-title");
        assert_eq!(detailed[1].fused_rank, 2);
        assert_eq!(detailed[1].passage, "Kumquat sync");
        assert_eq!(detailed[1].rerank_score, 1.0);
        assert_eq!(detailed[2].document_id, "doc-none");
        assert_eq!(detailed[2].fused_rank, 1);
        assert_eq!(detailed[2].rerank_score, 0.0);
    }

    #[test]
    fn blend_prefers_strong_fusion_prior_in_close_calls() {
        // doc-deep outscores doc-fused on the cross-encoder (2 vs 1 query
        // occurrences), but doc-fused carries a far stronger fusion prior,
        // so the blended ordering must put doc-fused first.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-fused": {"id": "doc-fused", "title": "Kumquat sync", "created_at": "2026-01-01T10:00:00Z"},
                "doc-deep": {"id": "doc-deep", "title": "Planning", "created_at": "2026-01-02T10:00:00Z"}
            }
        }));
        let ranking = HybridRanking {
            fused: vec![
                FusedDoc { document_id: "doc-fused".to_string(), score: 0.1 },
                FusedDoc { document_id: "doc-deep".to_string(), score: 0.0 },
            ],
            best_chunks: HashMap::from([(
                "doc-deep".to_string(),
                "kumquat kumquat".to_string(),
            )]),
        };

        let detailed =
            rerank_hybrid_detailed(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        // Blends: doc-fused 1 + 30 * 0.1 = 4, doc-deep 2 + 30 * 0 = 2.
        assert_eq!(detailed[0].document_id, "doc-fused");
        assert_eq!(detailed[0].rerank_score, 1.0);
        assert_eq!(detailed[1].document_id, "doc-deep");
        assert_eq!(detailed[1].rerank_score, 2.0);
    }

    #[test]
    fn detailed_ties_keep_fused_order() {
        // Both passages score 1, so the fused order must survive the sort
        // regardless of the order the database returns rows in.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-second": {"id": "doc-second", "title": "Kumquat retro", "created_at": "2026-01-01T10:00:00Z"},
                "doc-first": {"id": "doc-first", "title": "Kumquat sync", "created_at": "2026-01-02T10:00:00Z"}
            }
        }));
        let ranking = HybridRanking {
            fused: fused(&["doc-first", "doc-second"]),
            best_chunks: HashMap::new(),
        };

        let detailed =
            rerank_hybrid_detailed(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        let ids: Vec<&str> = detailed.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-first", "doc-second"]);
        assert_eq!(detailed[0].fused_rank, 1);
        assert_eq!(detailed[1].fused_rank, 2);
    }

    #[test]
    fn detailed_empty_ranking_reranks_to_empty() {
        let conn = build_test_db(&json!({ "documents": {} }));
        let ranking = HybridRanking { fused: Vec::new(), best_chunks: HashMap::new() };

        let detailed =
            rerank_hybrid_detailed(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        assert!(detailed.is_empty());
    }

    #[test]
    fn hybrid_rerank_considers_only_the_pool() {
        // 60 fused documents; the strongest match sits below the pool
        // cutoff, so it must not appear in the reranked output.
        let mut docs = serde_json::Map::new();
        for i in 0..60 {
            let id = format!("doc-{i:02}");
            docs.insert(
                id.clone(),
                json!({"id": id, "title": format!("Meeting {i}"), "created_at": "2026-01-01T10:00:00Z"}),
            );
        }
        let conn = build_test_db(&json!({ "documents": docs }));

        let ids: Vec<String> = (0..60).map(|i| format!("doc-{i:02}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let ranking = HybridRanking {
            fused: fused(&id_refs),
            best_chunks: HashMap::from([("doc-59".to_string(), "kumquat".to_string())]),
        };

        let reranked = rerank_hybrid(&conn, &MockReranker, "kumquat", &ranking).unwrap();

        assert_eq!(reranked.len(), RERANK_POOL);
        assert!(reranked.iter().all(|d| d.document_id != "doc-59"));
    }
}
