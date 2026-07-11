//! Cross-encoder reranking models.
//!
//! A reranker scores (query, passage) pairs jointly with a cross-encoder,
//! slower but more accurate than the bi-encoder cosine ranking in
//! [`crate::embed::model`]. The hybrid search pipeline uses it to reorder
//! the top fused candidates (see [`crate::query::rerank`]).

use std::cell::RefCell;

use anyhow::Result;

/// Trait for rerankers scoring (query, document) pairs.
pub trait Reranker {
    /// Score every document against the query. Scores come back in input
    /// order; higher is more relevant. Production models return sigmoid
    /// probabilities in [0, 1].
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>>;
}

/// Reranker model selection. Both candidates from the Phase 3 evaluation
/// stay available so the comparison is re-runnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankModel {
    /// jinaai/jina-reranker-v1-turbo-en: 6-layer English reranker.
    JinaTurbo,
    /// BAAI/bge-reranker-base: 12-layer English/Chinese reranker.
    BgeBase,
}

/// Model used by `grans search --hybrid` (winner of the Phase 3 benchmark).
pub const DEFAULT_RERANK_MODEL: RerankModel = RerankModel::JinaTurbo;

impl RerankModel {
    fn fastembed_model(&self) -> fastembed::RerankerModel {
        match self {
            RerankModel::JinaTurbo => fastembed::RerankerModel::JINARerankerV1TurboEn,
            RerankModel::BgeBase => fastembed::RerankerModel::BGERerankerBase,
        }
    }
}

/// Map a cross-encoder logit to a relevance probability in [0, 1].
fn sigmoid(logit: f32) -> f32 {
    1.0 / (1.0 + (-logit).exp())
}

/// Production reranker using fastembed's `TextRerank`.
pub struct FastEmbedReranker {
    model: RefCell<fastembed::TextRerank>,
}

impl FastEmbedReranker {
    pub fn new(choice: RerankModel) -> Result<Self> {
        super::model::set_hf_cache_dir()?;

        let providers = super::model::execution_providers();

        let mut opts = fastembed::RerankInitOptions::new(choice.fastembed_model())
            .with_show_download_progress(true);
        if !providers.is_empty() {
            opts = opts.with_execution_providers(providers);
        }

        let model = fastembed::TextRerank::try_new(opts)?;

        Ok(Self { model: RefCell::new(model) })
    }
}

impl Reranker for FastEmbedReranker {
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        // fastembed returns results sorted by score; `index` maps each back
        // to its input position.
        let results = self.model.borrow_mut().rerank(query, documents, false, None)?;
        let mut scores = vec![0.0_f32; documents.len()];
        for r in results {
            scores[r.index] = sigmoid(r.score);
        }
        Ok(scores)
    }
}

/// Mock reranker for testing: a document's score is the number of times the
/// query appears in it (case-insensitive), so tests control ordering by
/// repeating the query in passages.
#[cfg(test)]
pub struct MockReranker;

#[cfg(test)]
impl Reranker for MockReranker {
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        let needle = query.to_lowercase();
        Ok(documents
            .iter()
            .map(|d| d.to_lowercase().matches(&needle).count() as f32)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_maps_zero_logit_to_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sigmoid_is_monotonic_and_bounded() {
        assert!(sigmoid(-4.0) < sigmoid(0.0));
        assert!(sigmoid(0.0) < sigmoid(4.0));
        // f32 rounding saturates extreme logits to the interval bounds.
        assert!(sigmoid(-20.0) >= 0.0);
        assert!(sigmoid(20.0) <= 1.0);
    }

    #[test]
    fn mock_reranker_scores_in_input_order() {
        let scores = MockReranker
            .rerank("budget", &["no match", "budget", "budget budget"])
            .unwrap();
        assert_eq!(scores, vec![0.0, 1.0, 2.0]);
    }

    #[test]
    fn mock_reranker_is_case_insensitive() {
        let scores = MockReranker.rerank("Budget", &["the BUDGET meeting"]).unwrap();
        assert_eq!(scores, vec![1.0]);
    }

    #[test]
    fn reranker_usable_as_trait_object() {
        let reranker: &dyn Reranker = &MockReranker;
        let scores = reranker.rerank("q", &["q"]).unwrap();
        assert_eq!(scores.len(), 1);
    }
}
