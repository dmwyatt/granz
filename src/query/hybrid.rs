//! Hybrid retrieval: fuse FTS keyword and semantic rankings with RRF.
//!
//! Both retrievers run over the same query; their document-level ranked
//! lists are truncated to a candidate pool and fused by rank (see
//! [`crate::query::fusion`]). Used by `grans search --hybrid` and the
//! quality benchmark's hybrid mode.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use crate::embed::model::Embedder;
use crate::embed::EmbeddingIndex;
use crate::query::dates::DateRange;
use crate::query::filter::{semantic_source_filter, SearchTarget};
use crate::query::fusion::{reciprocal_rank_fusion, FusedDoc, RRF_K};

/// How many top documents each retriever contributes to fusion.
pub const CANDIDATE_POOL: usize = 100;

/// Hybrid retrieval output: the fused ranking plus each document's
/// best-matching chunk text from the semantic pass. The rerank stage uses
/// the chunk texts as passages; documents without embedded content (e.g.
/// title-only FTS matches) have no entry.
pub struct HybridRanking {
    pub fused: Vec<FusedDoc>,
    pub best_chunks: HashMap<String, String>,
}

/// Run FTS and semantic retrieval for `query` and fuse the rankings.
/// Fused documents come back best first.
pub fn hybrid_ranked(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    query: &str,
    targets: &[SearchTarget],
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<HybridRanking> {
    let fts_docs = crate::db::meetings::search_meetings(
        conn,
        query,
        targets.contains(&SearchTarget::Titles),
        targets.contains(&SearchTarget::Transcripts),
        targets.contains(&SearchTarget::Notes),
        targets.contains(&SearchTarget::Panels),
        date_range,
        include_deleted,
    )?;
    let fts_ids: Vec<String> = fts_docs.into_iter().filter_map(|d| d.id).collect();

    // No limit: the per-document best chunks must cover every candidate,
    // including FTS-only documents outside the semantic top of the pool.
    // Fusion still truncates each id list to the pool.
    let source_filter = semantic_source_filter(targets);
    let (semantic_results, _) = crate::embed::semantic_search_with_index(
        conn,
        embedder,
        index,
        query,
        date_range,
        0,
        source_filter.as_deref(),
        include_deleted,
    )?;

    let mut semantic_ids = Vec::with_capacity(semantic_results.len());
    let mut best_chunks = HashMap::with_capacity(semantic_results.len());
    for r in semantic_results {
        semantic_ids.push(r.document_id.clone());
        best_chunks.insert(r.document_id, r.matched_text);
    }

    Ok(HybridRanking {
        fused: fuse_candidates(fts_ids, semantic_ids),
        best_chunks,
    })
}

/// Truncate each ranked id list to the candidate pool and fuse with RRF.
fn fuse_candidates(mut fts_ids: Vec<String>, mut semantic_ids: Vec<String>) -> Vec<FusedDoc> {
    fts_ids.truncate(CANDIDATE_POOL);
    semantic_ids.truncate(CANDIDATE_POOL);
    reciprocal_rank_fusion(&[fts_ids, semantic_ids], RRF_K)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use crate::embed::store::StoredVector;
    use serde_json::json;

    /// Embedder whose query vector is fixed, so cosine rankings against
    /// hand-built stored vectors are fully controlled.
    struct FixedEmbedder;

    impl Embedder for FixedEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn embed_query(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }

        fn dimension(&self) -> usize {
            2
        }

        fn model_name(&self) -> &str {
            "fixed-embedder"
        }

        fn max_length(&self) -> usize {
            512
        }
    }

    fn stored(doc_id: &str, text: &str, vector: Vec<f32>) -> StoredVector {
        StoredVector {
            chunk_id: 0,
            document_id: doc_id.to_string(),
            source_type: "transcript_window".to_string(),
            text: text.to_string(),
            vector,
            metadata_json: None,
        }
    }

    /// Three docs: "doc-both" is found by FTS (title) and ranks first
    /// semantically; "doc-fts" only matches by title; "doc-sem" only ranks
    /// semantically. The doc in both lists must fuse to the top.
    fn hybrid_state() -> serde_json::Value {
        json!({
            "documents": {
                "doc-both": {"id": "doc-both", "title": "Kumquat sync A", "created_at": "2026-01-20T10:00:00Z"},
                "doc-fts": {"id": "doc-fts", "title": "Kumquat sync B", "created_at": "2026-01-25T10:00:00Z"},
                "doc-sem": {"id": "doc-sem", "title": "Unrelated", "created_at": "2026-01-15T10:00:00Z"}
            }
        })
    }

    fn hybrid_index() -> EmbeddingIndex {
        EmbeddingIndex {
            vectors: vec![
                // cosine vs [1, 0]: doc-both = 1.0, doc-sem ~= 0.707
                stored("doc-both", "both chunk", vec![1.0, 0.0]),
                stored("doc-sem", "sem chunk", vec![1.0, 1.0]),
            ],
            stats: None,
        }
    }

    fn all_targets() -> Vec<SearchTarget> {
        SearchTarget::parse_list("titles,transcripts,notes,panels")
    }

    #[test]
    fn doc_found_by_both_retrievers_fuses_to_top() {
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        let fused =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, false)
                .unwrap()
                .fused;

        // FTS title tier orders by recency: [doc-fts, doc-both].
        // Semantic orders by cosine: [doc-both, doc-sem].
        // doc-both appears in both lists, so it wins; doc-fts (rank 1 in
        // FTS) and doc-sem (rank 2 in semantic) follow.
        let ids: Vec<&str> = fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids[0], "doc-both");
        assert!(ids.contains(&"doc-fts"));
        assert!(ids.contains(&"doc-sem"));
        assert_eq!(fused.len(), 3);
        // doc-fts holds rank 1 in the FTS list, doc-sem rank 2 in the
        // semantic list, so doc-fts scores higher.
        assert_eq!(ids, vec!["doc-both", "doc-fts", "doc-sem"]);
    }

    #[test]
    fn titles_only_targets_exclude_semantic_results() {
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();
        let targets = SearchTarget::parse_list("titles");

        let fused =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &targets, None, false)
                .unwrap()
                .fused;

        // Semantic search is filtered to no embeddable source types, so
        // only the FTS title matches remain, in FTS order.
        let ids: Vec<&str> = fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-fts", "doc-both"]);
    }

    #[test]
    fn ranking_exposes_best_chunk_text_per_document() {
        let conn = build_test_db(&hybrid_state());
        // doc-sem has two chunks; the higher-cosine one must win.
        let index = EmbeddingIndex {
            vectors: vec![
                stored("doc-both", "both chunk", vec![1.0, 0.0]),
                stored("doc-sem", "weak chunk", vec![0.0, 1.0]),
                stored("doc-sem", "strong chunk", vec![1.0, 0.5]),
            ],
            stats: None,
        };

        let ranking =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, false)
                .unwrap();

        assert_eq!(ranking.best_chunks.get("doc-both").map(String::as_str), Some("both chunk"));
        assert_eq!(ranking.best_chunks.get("doc-sem").map(String::as_str), Some("strong chunk"));
        // doc-fts has no chunks, so it has no passage entry.
        assert!(!ranking.best_chunks.contains_key("doc-fts"));
    }

    #[test]
    fn no_matches_fuse_to_empty() {
        let conn = build_test_db(&hybrid_state());
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };

        let ranking =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "zyzzyva", &all_targets(), None, false)
                .unwrap();

        assert!(ranking.fused.is_empty());
        assert!(ranking.best_chunks.is_empty());
    }

    #[test]
    fn fuse_candidates_truncates_each_list_to_pool() {
        let fts: Vec<String> = (0..150).map(|i| format!("fts-{i:03}")).collect();
        let semantic: Vec<String> = (0..150).map(|i| format!("sem-{i:03}")).collect();

        let fused = fuse_candidates(fts, semantic);

        assert_eq!(fused.len(), 2 * CANDIDATE_POOL);
        let ids: Vec<&str> = fused.iter().map(|d| d.document_id.as_str()).collect();
        assert!(ids.contains(&"fts-099"));
        assert!(!ids.contains(&"fts-100"));
        assert!(ids.contains(&"sem-099"));
        assert!(!ids.contains(&"sem-100"));
    }
}
