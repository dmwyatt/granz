//! Retrieval backends for the quality benchmark.
//!
//! Each mode returns the full document-level ranked list for a query;
//! scoring (metrics.rs) applies k. Lists reflect current production
//! behavior for the mode: FTS is ranked by bm25 with a recency tiebreak,
//! semantic by best-chunk cosine score, hybrid by RRF over both, and the
//! rerank modes by cross-encoder score blended with the fusion prior over
//! the top of the fused pool.

use anyhow::Result;
use rusqlite::Connection;

use super::metrics::RankedDoc;
use crate::cli::args::QualityMode;
use crate::embed::config::EmbedSpec;
use crate::embed::model::{Embedder, FastEmbedModel};
use crate::embed::rerank::{FastEmbedReranker, RerankModel, Reranker};
use crate::embed::search::SemanticSearchResult;
use crate::embed::{ensure_embeddings, EmbeddingIndex, DEFAULT_BATCH_SIZE};
use crate::query::adjust::{RankingConfig, RankingContext};
use crate::query::filter::SearchTarget;
use crate::query::rerank::RerankCandidate;

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
    HybridRerank {
        conn: &'a Connection,
        embedder: FastEmbedModel,
        index: EmbeddingIndex,
        reranker: Box<FastEmbedReranker>,
        ctx: RankingContext,
        cfg: RankingConfig,
    },
}

impl<'a> Retriever<'a> {
    /// Build the retriever for a mode. For semantic, hybrid, and rerank
    /// modes, the models, index, and ranking context are loaded once here
    /// so per-query latency measures search, not one-time setup. The
    /// embedding spec resolves from the database's stored metadata, so a
    /// snapshot embedded with a variant scheme is benchmarked as-is
    /// instead of being silently re-embedded with this binary's defaults.
    pub fn build(mode: QualityMode, conn: &'a Connection, cfg: RankingConfig) -> Result<Self> {
        match mode {
            QualityMode::Fts => Ok(Retriever::Fts { conn }),
            QualityMode::Semantic => {
                let embedder = FastEmbedModel::new()?;
                let spec = EmbedSpec::resolve_stored(conn, embedder.max_length());
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE, &spec)?;
                Ok(Retriever::Semantic { embedder, index })
            }
            QualityMode::Hybrid => {
                let embedder = FastEmbedModel::new()?;
                let spec = EmbedSpec::resolve_stored(conn, embedder.max_length());
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE, &spec)?;
                Ok(Retriever::Hybrid { conn, embedder, index })
            }
            QualityMode::RerankJina | QualityMode::RerankBge => {
                let embedder = FastEmbedModel::new()?;
                let spec = EmbedSpec::resolve_stored(conn, embedder.max_length());
                let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE, &spec)?;
                let model = match mode {
                    QualityMode::RerankJina => RerankModel::JinaTurbo,
                    _ => RerankModel::BgeBase,
                };
                let reranker = Box::new(FastEmbedReranker::new(model)?);
                let ctx = RankingContext::load(conn)?;
                Ok(Retriever::HybridRerank { conn, embedder, index, reranker, ctx, cfg })
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
            Retriever::HybridRerank { conn, embedder, index, reranker, ctx, cfg } => {
                retrieve_hybrid_rerank(conn, embedder, index, reranker.as_ref(), query, ctx, cfg)
            }
        }
    }

    /// Per-candidate rerank detail (fused rank, RRF score, passage, rerank
    /// score) for modes with a rerank stage; `None` for the others.
    pub fn retrieve_detailed(&self, query: &str) -> Result<Option<Vec<RerankCandidate>>> {
        match self {
            Retriever::HybridRerank { conn, embedder, index, reranker, ctx, cfg } => {
                Ok(Some(retrieve_hybrid_rerank_detailed(
                    conn,
                    embedder,
                    index,
                    reranker.as_ref(),
                    query,
                    ctx,
                    cfg,
                )?))
            }
            _ => Ok(None),
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
    let targets = SearchTarget::all();
    let fused =
        crate::query::hybrid::hybrid_ranked(conn, embedder, index, query, &targets, None, None, false)?
            .fused;
    Ok(fused
        .into_iter()
        .map(|d| RankedDoc {
            document_id: d.document_id,
            score: Some(d.score as f32),
        })
        .collect())
}

/// Reranked hybrid retrieval over the same default targets, with each
/// candidate's fusion components: production fusion, then the
/// cross-encoder scores the top candidates.
fn retrieve_hybrid_rerank_detailed(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    reranker: &dyn Reranker,
    query: &str,
    ctx: &RankingContext,
    cfg: &RankingConfig,
) -> Result<Vec<RerankCandidate>> {
    let targets = SearchTarget::all();
    let ranking =
        crate::query::hybrid::hybrid_ranked(conn, embedder, index, query, &targets, None, None, false)?;
    crate::query::rerank::rerank_hybrid_detailed(conn, reranker, query, &ranking, ctx, cfg)
}

/// Reranked hybrid retrieval in production result shape (the
/// `grans search` default path).
fn retrieve_hybrid_rerank(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    reranker: &dyn Reranker,
    query: &str,
    ctx: &RankingContext,
    cfg: &RankingConfig,
) -> Result<Vec<RankedDoc>> {
    Ok(retrieve_hybrid_rerank_detailed(conn, embedder, index, reranker, query, ctx, cfg)?
        .into_iter()
        .map(|c| RankedDoc {
            document_id: c.document_id,
            score: Some(c.rerank_score),
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

    /// Boost-free config so these tests pin fusion + cross-encoder
    /// behavior independent of the adopted default weights.
    fn no_boost() -> RankingConfig {
        RankingConfig { title_boost_weight: 0.0, ..Default::default() }
    }

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
    fn hybrid_rerank_retriever_reorders_by_cross_encoder() {
        use crate::embed::model::MockEmbedder;
        use crate::embed::rerank::MockReranker;
        use crate::embed::store::StoredVector;

        let conn = build_test_db(&meetings_state());
        // Query "standup": FTS matches doc-2 by title, and MockEmbedder's
        // query vector has a larger first component, so doc-1 (vector
        // [1, 0]) outranks doc-2 ([0, 1]) semantically. Fusion puts doc-2
        // (in both lists) first. The passages reverse that: doc-1's chunk
        // repeats the query three times, doc-2's not at all (its single
        // occurrence is the title).
        let stored = |doc_id: &str, text: &str, vector: Vec<f32>| StoredVector {
            chunk_id: 0,
            document_id: doc_id.to_string(),
            source_type: "transcript_window".to_string(),
            text: text.to_string(),
            vector,
            metadata_json: None,
        };
        let index = EmbeddingIndex {
            vectors: vec![
                stored("doc-1", "standup standup standup", vec![1.0, 0.0]),
                stored("doc-2", "planning", vec![0.0, 1.0]),
            ],
            stats: None,
        };
        let embedder = MockEmbedder { dim: 2, max_length: 512 };

        let ranked =
            retrieve_hybrid_rerank(&conn, &embedder, &index, &MockReranker, "standup", &RankingContext::default(), &no_boost()).unwrap();

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].document_id, "doc-1");
        assert_eq!(ranked[0].score, Some(3.0));
        assert_eq!(ranked[1].document_id, "doc-2");
        assert_eq!(ranked[1].score, Some(1.0));
    }

    #[test]
    fn hybrid_rerank_detailed_carries_fusion_components() {
        use crate::embed::model::MockEmbedder;
        use crate::embed::rerank::MockReranker;
        use crate::embed::store::StoredVector;

        // Same setup as the reorder test: fusion puts doc-2 first, the
        // cross-encoder reverses that, so doc-1 must carry fused_rank 2.
        let conn = build_test_db(&meetings_state());
        let stored = |doc_id: &str, text: &str, vector: Vec<f32>| StoredVector {
            chunk_id: 0,
            document_id: doc_id.to_string(),
            source_type: "transcript_window".to_string(),
            text: text.to_string(),
            vector,
            metadata_json: None,
        };
        let index = EmbeddingIndex {
            vectors: vec![
                stored("doc-1", "standup standup standup", vec![1.0, 0.0]),
                stored("doc-2", "planning", vec![0.0, 1.0]),
            ],
            stats: None,
        };
        let embedder = MockEmbedder { dim: 2, max_length: 512 };

        let detailed =
            retrieve_hybrid_rerank_detailed(&conn, &embedder, &index, &MockReranker, "standup", &RankingContext::default(), &no_boost())
                .unwrap();

        assert_eq!(detailed.len(), 2);
        assert_eq!(detailed[0].document_id, "doc-1");
        assert_eq!(detailed[0].fused_rank, 2);
        assert_eq!(detailed[0].rerank_score, 3.0);
        assert!(detailed[0].fused_score > 0.0);
        assert!(detailed[0].passage.contains("standup standup standup"));
        assert_eq!(detailed[1].document_id, "doc-2");
        assert_eq!(detailed[1].fused_rank, 1);
    }

    #[test]
    fn non_rerank_retriever_has_no_candidate_detail() {
        let conn = build_test_db(&meetings_state());
        let retriever = Retriever::Fts { conn: &conn };

        let detailed = retriever.retrieve_detailed("machine learning").unwrap();

        assert!(detailed.is_none());
    }

    #[test]
    fn hybrid_rerank_retriever_empty_for_no_match() {
        use crate::embed::model::MockEmbedder;
        use crate::embed::rerank::MockReranker;

        let conn = build_test_db(&meetings_state());
        let index = EmbeddingIndex { vectors: Vec::new(), stats: None };
        let embedder = MockEmbedder { dim: 2, max_length: 512 };

        let ranked =
            retrieve_hybrid_rerank(&conn, &embedder, &index, &MockReranker, "zebra xylophone", &RankingContext::default(), &no_boost())
                .unwrap();

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
                section_heading: None,
            },
            SemanticSearchResult {
                document_id: "d2".into(),
                score: 0.7,
                source_type: "notes".into(),
                matched_text: String::new(),
                window_start_idx: None,
                window_end_idx: None,
                match_context: None,
                section_heading: None,
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
