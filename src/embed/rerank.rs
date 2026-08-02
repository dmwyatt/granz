//! Cross-encoder reranking models.
//!
//! A reranker scores (query, passage) pairs jointly with a cross-encoder,
//! slower but more accurate than the bi-encoder cosine ranking in
//! [`crate::embed::model`]. The hybrid search pipeline uses it to reorder
//! the top fused candidates (see [`crate::query::rerank`]).

use std::cell::RefCell;
use std::thread::JoinHandle;

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

/// Model used by `grans search` (winner of the Phase 3 benchmark).
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

/// Init options for a reranker model: data-dir cache, download progress,
/// hardware execution providers. `TextRerank` resolves its cache from these
/// options alone (it ignores HF_HOME, unlike `TextEmbedding`), so the cache
/// directory must be explicit.
fn init_options(choice: RerankModel) -> Result<fastembed::RerankInitOptions> {
    let mut opts = fastembed::RerankInitOptions::new(choice.fastembed_model())
        .with_cache_dir(super::model::hf_cache_dir()?)
        .with_show_download_progress(true);

    let providers = super::model::execution_providers();
    if !providers.is_empty() {
        opts = opts.with_execution_providers(providers);
    }
    Ok(opts)
}

impl FastEmbedReranker {
    pub fn new(choice: RerankModel) -> Result<Self> {
        let model = fastembed::TextRerank::try_new(init_options(choice)?)?;
        Ok(Self {
            model: RefCell::new(model),
        })
    }
}

/// A reranker loading on a background thread.
///
/// The load depends on nothing the search pipeline produces, so it can run
/// while the query is embedded and retrieval fuses its candidates.
/// [`join`](Self::join) then waits only for whatever load time did not fit
/// behind that work, which on a warm model cache is none of it.
pub struct PendingReranker {
    handle: JoinHandle<Result<FastEmbedReranker>>,
}

impl PendingReranker {
    /// Begin loading `choice`.
    ///
    /// Before spawning anything this resolves and creates the model cache
    /// directory on disk and, on the process's first call, writes the
    /// `HF_HOME` environment variable via
    /// [`set_hf_cache_dir`](super::model::set_hf_cache_dir).
    pub fn spawn(choice: RerankModel) -> Result<Self> {
        // The loader thread reads the process environment (platform paths,
        // hf-hub's endpoint and token variables), so set_hf_cache_dir must
        // run before the thread exists; the full ordering argument lives on
        // that function.
        super::model::set_hf_cache_dir()?;
        Ok(Self {
            handle: std::thread::spawn(move || FastEmbedReranker::new(choice)),
        })
    }

    /// Wait for the model, or for whatever went wrong loading it.
    pub fn join(self) -> Result<FastEmbedReranker> {
        join_loader(self.handle)
    }
}

/// Wait for a loader thread, re-raising a panic on the joining thread so
/// moving the load off-thread does not turn a crash into a caught error.
fn join_loader<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.join() {
        Ok(loaded) => loaded,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

impl Reranker for FastEmbedReranker {
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        // fastembed returns results sorted by score; `index` maps each back
        // to its input position.
        let results = self
            .model
            .borrow_mut()
            .rerank(query, documents, false, None)?;
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
    fn init_options_cache_models_in_the_data_dir() {
        // TextRerank resolves its cache from the init options alone (it
        // ignores HF_HOME, unlike TextEmbedding), so the data-dir cache
        // must be set explicitly or models land in a CWD-relative
        // .fastembed_cache.
        let opts = init_options(RerankModel::JinaTurbo).unwrap();
        let expected = crate::platform::data_dir().unwrap().join("fastembed_cache");
        assert_eq!(opts.cache_dir, expected);
    }

    #[test]
    fn reranker_can_be_loaded_off_thread() {
        // PendingReranker moves a loaded model back to the caller. If a
        // fastembed upgrade makes TextRerank thread-bound, this stops
        // compiling rather than failing at runtime.
        fn assert_send<T: Send>() {}
        assert_send::<FastEmbedReranker>();
    }

    #[test]
    fn join_loader_returns_the_loaded_value() {
        let handle = std::thread::spawn(|| Ok(7_u32));
        assert_eq!(join_loader(handle).unwrap(), 7);
    }

    #[test]
    fn join_loader_propagates_a_loader_error() {
        let handle = std::thread::spawn(|| Err::<u32, _>(anyhow::anyhow!("model download failed")));
        let err = join_loader(handle).unwrap_err();
        assert_eq!(err.to_string(), "model download failed");
    }

    #[test]
    #[should_panic(expected = "ort blew up")]
    fn join_loader_repanics_when_the_loader_panics() {
        let handle = std::thread::spawn(|| -> Result<u32> { panic!("ort blew up") });
        let _ = join_loader(handle);
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
        let scores = MockReranker
            .rerank("Budget", &["the BUDGET meeting"])
            .unwrap();
        assert_eq!(scores, vec![1.0]);
    }

    #[test]
    fn reranker_usable_as_trait_object() {
        let reranker: &dyn Reranker = &MockReranker;
        let scores = reranker.rerank("q", &["q"]).unwrap();
        assert_eq!(scores.len(), 1);
    }
}
