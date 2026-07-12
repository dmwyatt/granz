//! Hybrid retrieval: fuse FTS keyword and semantic rankings with RRF.
//!
//! Both retrievers run over the same query; their document-level ranked
//! lists are truncated to a candidate pool and fused by rank (see
//! [`crate::query::fusion`]). Used by `grans search` and the quality
//! benchmark's hybrid mode.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use crate::embed::model::Embedder;
use crate::embed::EmbeddingIndex;
use crate::query::dates::DateRange;
use crate::query::filter::{meeting_filter_matches, semantic_source_filter, SearchTarget};
use crate::query::fusion::{reciprocal_rank_fusion, FusedDoc, RRF_K};

/// How many top documents each retriever contributes to fusion.
pub const CANDIDATE_POOL: usize = 100;

/// A document's best-matching chunk from the semantic pass.
#[derive(Debug, Clone)]
pub struct BestChunk {
    pub text: String,
    /// Chunk source: `transcript_window`, `panel_section`, or `notes_paragraph`.
    pub source_type: String,
    /// Panel section heading for `panel_section` chunks.
    pub section_heading: Option<String>,
}

/// Hybrid retrieval output: the fused ranking plus each document's
/// best-matching chunk from the semantic pass. The rerank stage uses the
/// chunk texts as passages and shaped output uses them as semantic-only
/// evidence; documents without embedded content (e.g. title-only FTS
/// matches) have no entry. `keyword_ids` records which documents the FTS
/// retriever surfaced.
pub struct HybridRanking {
    pub fused: Vec<FusedDoc>,
    pub best_chunks: HashMap<String, BestChunk>,
    pub keyword_ids: HashSet<String>,
    /// Uncapped FTS match count, taken before fusion truncates the FTS
    /// list to the candidate pool: the number of meetings `grans grep`
    /// would report for the same query and filters.
    pub keyword_total: usize,
}

/// Run FTS and semantic retrieval for `query` and fuse the rankings.
/// Fused documents come back best first.
///
/// `meeting_filter` (case-insensitive substring of document title or id)
/// restricts both candidate lists before fusion truncates them to the
/// pool, so ranking happens within the requested meeting rather than
/// intersecting it with the global top candidates.
#[allow(clippy::too_many_arguments)]
pub fn hybrid_ranked(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    query: &str,
    targets: &[SearchTarget],
    meeting_filter: Option<&str>,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<HybridRanking> {
    let allowed = meeting_filter.map(|f| allowed_meeting_ids(conn, f)).transpose()?;
    let is_allowed = |id: &str| allowed.as_ref().is_none_or(|set| set.contains(id));

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
    let fts_ids: Vec<String> =
        fts_docs.into_iter().filter_map(|d| d.id).filter(|id| is_allowed(id)).collect();

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
        if !is_allowed(&r.document_id) {
            continue;
        }
        semantic_ids.push(r.document_id.clone());
        best_chunks.insert(
            r.document_id,
            BestChunk {
                text: r.matched_text,
                source_type: r.source_type,
                section_heading: r.section_heading,
            },
        );
    }

    let keyword_ids: HashSet<String> = fts_ids.iter().cloned().collect();
    let keyword_total = fts_ids.len();
    Ok(HybridRanking {
        fused: fuse_candidates(fts_ids, semantic_ids),
        best_chunks,
        keyword_ids,
        keyword_total,
    })
}

/// Ids of documents whose title or id contains `filter`, case-insensitive.
fn allowed_meeting_ids(conn: &Connection, filter: &str) -> Result<HashSet<String>> {
    let filter_lower = filter.to_lowercase();
    Ok(crate::db::meetings::all_meeting_refs(conn)?
        .into_iter()
        .filter(|(id, title)| meeting_filter_matches(&filter_lower, title.as_deref(), Some(id)))
        .map(|(id, _)| id)
        .collect())
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
        SearchTarget::all()
    }

    #[test]
    fn doc_found_by_both_retrievers_fuses_to_top() {
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        let fused =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, None, false)
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
        let targets = vec![SearchTarget::Titles];

        let fused =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &targets, None, None, false)
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
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, None, false)
                .unwrap();

        assert_eq!(
            ranking.best_chunks.get("doc-both").map(|c| c.text.as_str()),
            Some("both chunk")
        );
        assert_eq!(
            ranking.best_chunks.get("doc-sem").map(|c| c.text.as_str()),
            Some("strong chunk")
        );
        assert_eq!(
            ranking.best_chunks.get("doc-sem").map(|c| c.source_type.as_str()),
            Some("transcript_window")
        );
        // doc-fts has no chunks, so it has no passage entry.
        assert!(!ranking.best_chunks.contains_key("doc-fts"));
    }

    #[test]
    fn ranking_records_which_documents_fts_surfaced() {
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        let ranking =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, None, false)
                .unwrap();

        // Both kumquat-titled docs come from FTS; the semantic-only doc
        // does not.
        assert!(ranking.keyword_ids.contains("doc-both"));
        assert!(ranking.keyword_ids.contains("doc-fts"));
        assert!(!ranking.keyword_ids.contains("doc-sem"));
    }

    #[test]
    fn keyword_total_counts_fts_matches_beyond_the_pool() {
        // 120 kumquat-titled docs: fusion truncates the FTS list to the
        // candidate pool, but keyword_total reports the uncapped corpus
        // count backing the grep cross-link.
        let mut docs = serde_json::Map::new();
        for i in 0..120 {
            let id = format!("doc-{i:03}");
            docs.insert(
                id.clone(),
                json!({
                    "id": id,
                    "title": format!("Kumquat filler {i:03}"),
                    "created_at": format!("2026-03-01T{:02}:{:02}:00Z", i / 60, i % 60),
                }),
            );
        }
        let conn = build_test_db(&json!({ "documents": docs }));
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };

        let ranking =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "kumquat", &all_targets(), None, None, false)
                .unwrap();

        assert_eq!(ranking.keyword_total, 120);
        assert_eq!(ranking.fused.len(), CANDIDATE_POOL);
    }

    /// 120 kumquat filler docs plus one older "beta" doc that FTS ranks
    /// last, beyond the candidate pool.
    fn pooled_state_with_beta_target() -> serde_json::Value {
        let mut docs = serde_json::Map::new();
        for i in 0..120 {
            let id = format!("doc-{i:03}");
            docs.insert(
                id.clone(),
                json!({
                    "id": id,
                    "title": format!("Kumquat filler {i:03}"),
                    "created_at": format!("2026-03-01T{:02}:{:02}:00Z", i / 60, i % 60),
                }),
            );
        }
        docs.insert(
            "doc-beta-target".to_string(),
            json!({
                "id": "doc-beta-target",
                "title": "Kumquat beta target",
                "created_at": "2026-01-01T00:00:00Z",
            }),
        );
        json!({ "documents": docs })
    }

    #[test]
    fn meeting_filter_restricts_fts_candidates_before_pooling() {
        // Regression for #65: --meeting used to be applied after the caps,
        // intersecting the requested meeting with the global top of the
        // pool. The beta doc ranks 121st in FTS (past CANDIDATE_POOL), so
        // only a pushdown before truncation can surface it.
        let conn = build_test_db(&pooled_state_with_beta_target());
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };

        let ranking = hybrid_ranked(
            &conn,
            &FixedEmbedder,
            &index,
            "kumquat",
            &all_targets(),
            Some("beta"),
            None,
            false,
        )
        .unwrap();

        let ids: Vec<&str> = ranking.fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-beta-target"]);
        assert_eq!(ranking.keyword_total, 1);

        // Sanity: without the filter the beta doc is truncated out.
        let unfiltered = hybrid_ranked(
            &conn,
            &FixedEmbedder,
            &index,
            "kumquat",
            &all_targets(),
            None,
            None,
            false,
        )
        .unwrap();
        assert!(!unfiltered
            .fused
            .iter()
            .any(|d| d.document_id == "doc-beta-target"));
    }

    #[test]
    fn meeting_filter_applies_to_semantic_candidates() {
        // "doc-sem" only surfaces through the semantic list; the meeting
        // filter must gate that list too.
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        let ranking = hybrid_ranked(
            &conn,
            &FixedEmbedder,
            &index,
            "kumquat",
            &all_targets(),
            Some("unrelated"),
            None,
            false,
        )
        .unwrap();

        let ids: Vec<&str> = ranking.fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-sem"]);
        // Nothing FTS-matched survives the filter.
        assert_eq!(ranking.keyword_total, 0);
        // Excluded documents contribute no best chunks.
        assert!(!ranking.best_chunks.contains_key("doc-both"));
    }

    #[test]
    fn meeting_filter_matches_title_case_insensitively_and_by_id() {
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        // Title substring, different case.
        let by_title = hybrid_ranked(
            &conn, &FixedEmbedder, &index, "kumquat", &all_targets(), Some("SYNC A"), None, false,
        )
        .unwrap();
        let ids: Vec<&str> = by_title.fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-both"]);

        // Id substring.
        let by_id = hybrid_ranked(
            &conn, &FixedEmbedder, &index, "kumquat", &all_targets(), Some("doc-fts"), None, false,
        )
        .unwrap();
        let ids: Vec<&str> = by_id.fused.iter().map(|d| d.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-fts"]);
    }

    #[test]
    fn keyword_total_agrees_with_the_grep_side_count() {
        // keyword_total backs the footer that promises what grep will
        // report, but the two counts are computed on different paths:
        // hybrid_ranked filters ids against an allowed set, grep filters
        // documents with filter_by_meeting. Pin their agreement.
        let conn = build_test_db(&hybrid_state());
        let index = hybrid_index();

        for filter in [None, Some("sync"), Some("sync a"), Some("nowhere")] {
            let ranking = hybrid_ranked(
                &conn,
                &FixedEmbedder,
                &index,
                "kumquat",
                &all_targets(),
                filter,
                None,
                false,
            )
            .unwrap();

            let grep_docs = crate::db::meetings::search_meetings(
                &conn, "kumquat", true, true, true, true, None, false,
            )
            .unwrap();
            let grep_count = crate::query::filter::filter_by_meeting(grep_docs, filter).len();

            assert_eq!(ranking.keyword_total, grep_count, "filter {filter:?}");
        }
    }

    #[test]
    fn no_matches_fuse_to_empty() {
        let conn = build_test_db(&hybrid_state());
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };

        let ranking =
            hybrid_ranked(&conn, &FixedEmbedder, &index, "zyzzyva", &all_targets(), None, None, false)
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
