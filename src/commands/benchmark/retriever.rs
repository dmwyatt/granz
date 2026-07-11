//! Retrieval backends for the quality benchmark.
//!
//! Each mode returns the full document-level ranked list for a query;
//! scoring (metrics.rs) applies k. Lists reflect current production
//! behavior for the mode: FTS is ranked by bm25 with a recency tiebreak,
//! semantic by best-chunk cosine score, and hybrid by RRF over both.

use anyhow::Result;
use rusqlite::Connection;

use super::metrics::RankedDoc;
use crate::cli::args::QualityMode;
use crate::embed::model::{Embedder, FastEmbedModel};
use crate::embed::search::SemanticSearchResult;
use crate::embed::{ensure_embeddings, EmbeddingIndex, DEFAULT_BATCH_SIZE};
use crate::query::filter::SearchTarget;

pub enum Retriever<'a> {
    Fts {
        conn: &'a Connection,
    },
    Semantic {
        embedder: FastEmbedModel,
        index: EmbeddingIndex,
    },
    Hybrid {
        conn: &'a Connection,
        embedder: FastEmbedModel,
        index: EmbeddingIndex,
    },
}

impl<'a> Retriever<'a> {
    /// Build the retriever for a mode. For semantic and hybrid, the embedding
    /// model and index are loaded once here so per-query latency measures
    /// search, not one-time setup.
    pub fn build(mode: QualityMode, conn: &'a Connection) -> Result<Self> {
        match mode {
            QualityMode::Fts => Ok(Retriever::Fts { conn }),
            QualityMode::Semantic => {
                let embedder = FastEmbedModel::new()?;
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE)?;
                Ok(Retriever::Semantic { embedder, index })
            }
            QualityMode::Hybrid => {
                let embedder = FastEmbedModel::new()?;
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE)?;
                Ok(Retriever::Hybrid { conn, embedder, index })
            }
        }
    }

    /// Run one query, returning the full ranked document list.
    pub fn retrieve(&self, query: &str) -> Result<Vec<RankedDoc>> {
        match self {
            Retriever::Fts { conn } => retrieve_fts(conn, query),
            Retriever::Semantic { embedder, index } => {
                let query_vec = embedder.embed_query(query)?;
                Ok(to_ranked(index.search(&query_vec, 0.0, None)))
            }
            Retriever::Hybrid { conn, embedder, index } => {
                retrieve_hybrid(conn, embedder, index, query)
            }
        }
    }
}

/// FTS keyword search over the same targets `grans search` uses by default
/// (titles, transcripts, notes, panels), in production result order
/// (bm25 relevance, recency tiebreak).
fn retrieve_fts(conn: &Connection, query: &str) -> Result<Vec<RankedDoc>> {
    let docs =
        crate::db::meetings::search_meetings(conn, query, true, true, true, true, None, false)?;
    Ok(docs
        .into_iter()
        .filter_map(|d| d.id)
        .map(|id| RankedDoc {
            document_id: id,
            score: None,
        })
        .collect())
}

/// Hybrid retrieval over the same default targets, in production fusion
/// order (RRF over the FTS and semantic rankings).
fn retrieve_hybrid(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    query: &str,
) -> Result<Vec<RankedDoc>> {
    let targets = SearchTarget::parse_list("titles,transcripts,notes,panels");
    let fused =
        crate::query::hybrid::hybrid_ranked(conn, embedder, index, query, &targets, None, false)?
            .fused;
    Ok(fused
        .into_iter()
        .map(|d| RankedDoc {
            document_id: d.document_id,
            score: Some(d.score as f32),
        })
        .collect())
}

/// Map document-level semantic results (already deduped and sorted by score
/// descending) to the scoring input type.
fn to_ranked(results: Vec<SemanticSearchResult>) -> Vec<RankedDoc> {
    results
        .into_iter()
        .map(|r| RankedDoc {
            document_id: r.document_id,
            score: Some(r.score),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, meetings_state};

    #[test]
    fn fts_retriever_returns_matching_documents() {
        let conn = build_test_db(&meetings_state());
        let retriever = Retriever::Fts { conn: &conn };

        let ranked = retriever.retrieve("machine learning").unwrap();

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].document_id, "doc-1");
        assert_eq!(ranked[0].score, None);
    }

    #[test]
    fn fts_retriever_empty_for_no_match() {
        let conn = build_test_db(&meetings_state());
        let retriever = Retriever::Fts { conn: &conn };

        let ranked = retriever.retrieve("zebra xylophone").unwrap();

        assert!(ranked.is_empty());
    }

    #[test]
    fn hybrid_retriever_fuses_fts_and_semantic() {
        use crate::embed::model::MockEmbedder;
        use crate::embed::store::StoredVector;

        let conn = build_test_db(&meetings_state());
        // doc-1 is the sole semantic candidate; MockEmbedder query vectors
        // have non-negative components, so its cosine vs [1, 1] is >= 0 and
        // it ranks first (and only) on the semantic side.
        let index = EmbeddingIndex {
            vectors: vec![StoredVector {
                chunk_id: 0,
                document_id: "doc-1".to_string(),
                source_type: "transcript_window".to_string(),
                text: String::new(),
                vector: vec![1.0, 1.0],
                metadata_json: None,
            }],
            stats: None,
        };
        let embedder = MockEmbedder { dim: 2, max_length: 512 };

        // "machine learning" matches doc-1 by FTS too (notes), so doc-1 is
        // rank 1 in both lists: RRF score 2/(60 + 1).
        let ranked = retrieve_hybrid(&conn, &embedder, &index, "machine learning").unwrap();

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].document_id, "doc-1");
        let score = ranked[0].score.expect("hybrid results carry RRF scores");
        assert!((score - 2.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn hybrid_retriever_empty_for_no_match() {
        let conn = build_test_db(&meetings_state());
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };
        let embedder = crate::embed::model::MockEmbedder { dim: 2, max_length: 512 };

        let ranked = retrieve_hybrid(&conn, &embedder, &index, "zebra xylophone").unwrap();

        assert!(ranked.is_empty());
    }

    #[test]
    fn to_ranked_preserves_order_and_scores() {
        let results = vec![
            SemanticSearchResult {
                document_id: "d1".into(),
                score: 0.9,
                source_type: "transcript".into(),
                matched_text: String::new(),
                window_start_idx: None,
                window_end_idx: None,
                match_context: None,
            },
            SemanticSearchResult {
                document_id: "d2".into(),
                score: 0.7,
                source_type: "notes".into(),
                matched_text: String::new(),
                window_start_idx: None,
                window_end_idx: None,
                match_context: None,
            },
        ];

        let ranked = to_ranked(results);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].document_id, "d1");
        assert_eq!(ranked[0].score, Some(0.9));
        assert_eq!(ranked[1].document_id, "d2");
        assert_eq!(ranked[1].score, Some(0.7));
    }
}
