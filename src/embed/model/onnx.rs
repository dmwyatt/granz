//! nomic-embed-text-v1.5 through ONNX Runtime (fastembed).

use std::cell::RefCell;

use anyhow::Result;

use super::{Embedder, MODEL_NAME, execution_providers, set_hf_cache_dir};

/// Production embedder using fastembed (nomic-embed-text-v1.5).
pub struct FastEmbedModel {
    model: RefCell<fastembed::TextEmbedding>,
    dim: usize,
}

impl FastEmbedModel {
    pub fn new() -> Result<Self> {
        set_hf_cache_dir()?;

        let providers = execution_providers();

        let mut opts =
            fastembed::TextInitOptions::new(fastembed::EmbeddingModel::NomicEmbedTextV15)
                .with_show_download_progress(true)
                .with_max_length(512);

        if !providers.is_empty() {
            opts = opts.with_execution_providers(providers);
        }

        let model = fastembed::TextEmbedding::try_new(opts)?;

        Ok(Self {
            model: RefCell::new(model),
            dim: 768,
        })
    }
}

impl Embedder for FastEmbedModel {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let docs: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        let embeddings = self.model.borrow_mut().embed(docs, None)?;
        Ok(embeddings)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let results = self
            .model
            .borrow_mut()
            .embed(vec![text.to_string()], None)?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned for query"))
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn model_name(&self) -> &str {
        MODEL_NAME
    }

    fn max_length(&self) -> usize {
        512
    }
}
