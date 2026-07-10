//! Retrieval backends for the quality benchmark.
//!
//! Each mode returns the full document-level ranked list for a query;
//! scoring (metrics.rs) applies k. Lists reflect current production
//! behavior for the mode: FTS has no relevance ranking yet (Phase 1),
//! semantic is ranked by best-chunk cosine score.

use anyhow::Result;
use rusqlite::Connection;

use super::metrics::RankedDoc;
use crate::cli::args::QualityMode;
use crate::embed::model::{Embedder, FastEmbedModel};
use crate::embed::search::SemanticSearchResult;
use crate::embed::{ensure_embeddings, EmbeddingIndex, DEFAULT_BATCH_SIZE};

pub enum Retriever<'a> {
    Fts {
        conn: &'a Connection,
    },
    Semantic {
        embedder: FastEmbedModel,
        index: EmbeddingIndex,
    },
}

impl<'a> Retriever<'a> {
    /// Build the retriever for a mode. For semantic, the embedding model and
    /// index are loaded once here so per-query latency measures search, not
    /// one-time setup.
    pub fn build(mode: QualityMode, conn: &'a Connection) -> Result<Self> {
        match mode {
            QualityMode::Fts => Ok(Retriever::Fts { conn }),
            QualityMode::Semantic => {
                let embedder = FastEmbedModel::new()?;
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE)?;
                Ok(Retriever::Semantic { embedder, index })
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
        }
    }
}

/// FTS keyword search over the same targets `grans search` uses by default
/// (titles, transcripts, notes, panels), in production result order
/// (created_at DESC).
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
