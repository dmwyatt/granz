use std::cell::RefCell;
use std::env;

use anyhow::Result;

use crate::platform;

/// Trait for embedding models.
pub trait Embedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
    fn dimension(&self) -> usize;
    fn model_name(&self) -> &str;
    /// Maximum input length in tokens that the model accepts.
    fn max_length(&self) -> usize;
}

/// Production embedder using fastembed (nomic-embed-text-v1.5).
pub struct FastEmbedModel {
    model: RefCell<fastembed::TextEmbedding>,
    dim: usize,
}

impl FastEmbedModel {
    pub fn new() -> Result<Self> {
        // Set HF_HOME to ensure fastembed caches models in a consistent location
        // rather than dropping cache directories in the current working directory.
        // This uses the platform-specific data directory (e.g., ~/.local/share/grans/fastembed_cache)
        let cache_dir = platform::data_dir()?.join("fastembed_cache");
        std::fs::create_dir_all(&cache_dir)?;

        // SAFETY: We're setting HF_HOME before any other threads are spawned from FastEmbedModel,
        // and this is only called during model initialization (single-threaded context).
        unsafe {
            env::set_var("HF_HOME", cache_dir);
        }

        let providers = Self::execution_providers();

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

    fn execution_providers() -> Vec<ort::execution_providers::ExecutionProviderDispatch> {
        #[allow(unused_mut)]
        let mut providers = Vec::new();

        #[cfg(feature = "cuda")]
        {
            use ort::execution_providers::CUDAExecutionProvider;
            providers.push(
                ort::execution_providers::ExecutionProviderDispatch::from(
                    CUDAExecutionProvider::default(),
                )
                .error_on_failure(),
            );
        }

        #[cfg(feature = "directml")]
        {
            use ort::execution_providers::DirectMLExecutionProvider;
            providers.push(
                ort::execution_providers::ExecutionProviderDispatch::from(
                    DirectMLExecutionProvider::default(),
                )
                .error_on_failure(),
            );
        }

        #[cfg(feature = "coreml")]
        {
            use ort::execution_providers::CoreMLExecutionProvider;
            providers.push(
                ort::execution_providers::ExecutionProviderDispatch::from(
                    CoreMLExecutionProvider::default(),
                )
                .error_on_failure(),
            );
        }

        // CPU is always the implicit fallback in ort
        providers
    }
}

impl Embedder for FastEmbedModel {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let docs: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        let embeddings = self.model.borrow_mut().embed(docs, None)?;
        Ok(embeddings)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.model.borrow_mut().embed(vec![text.to_string()], None)?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned for query"))
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn model_name(&self) -> &str {
        "nomic-embed-text-v1.5"
    }

    fn max_length(&self) -> usize {
        512
    }
}

/// Mock embedder for testing — returns deterministic vectors based on text length.
#[cfg(test)]
pub struct MockEmbedder {
    pub dim: usize,
    pub max_length: usize,
}

#[cfg(test)]
impl Default for MockEmbedder {
    fn default() -> Self {
        Self {
            dim: 768,
            max_length: 512,
        }
    }
}

#[cfg(test)]
impl Embedder for MockEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.make_vector(t)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.make_vector(text))
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn model_name(&self) -> &str {
        "mock-embedder"
    }

    fn max_length(&self) -> usize {
        self.max_length
    }
}

#[cfg(test)]
impl MockEmbedder {
    fn make_vector(&self, text: &str) -> Vec<f32> {
        // Simple deterministic vector: use character bytes to seed dimensions
        let bytes = text.as_bytes();
        let mut vec = vec![0.0_f32; self.dim];
        for (i, &b) in bytes.iter().enumerate() {
            vec[i % self.dim] += b as f32 / 255.0;
        }
        // Normalize
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }
        vec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_embedder_dimension() {
        let embedder = MockEmbedder {
            dim: 768,
            max_length: 512,
        };
        assert_eq!(embedder.dimension(), 768);
    }

    #[test]
    fn test_mock_embedder_max_length() {
        let embedder = MockEmbedder {
            dim: 768,
            max_length: 256,
        };
        assert_eq!(embedder.max_length(), 256);
    }

    #[test]
    fn test_mock_embedder_default_max_length() {
        let embedder = MockEmbedder::default();
        assert_eq!(embedder.max_length(), 512);
    }

    #[test]
    fn test_mock_embedder_batch() {
        let embedder = MockEmbedder {
            dim: 4,
            max_length: 512,
        };
        let results = embedder.embed_batch(&["hello", "world"]).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 4);
        assert_eq!(results[1].len(), 4);
    }

    #[test]
    fn test_mock_embedder_deterministic() {
        let embedder = MockEmbedder {
            dim: 4,
            max_length: 512,
        };
        let v1 = embedder.embed_query("test").unwrap();
        let v2 = embedder.embed_query("test").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_mock_embedder_normalized() {
        let embedder = MockEmbedder {
            dim: 8,
            max_length: 512,
        };
        let v = embedder.embed_query("normalize me").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_mock_embedder_different_texts_different_vectors() {
        let embedder = MockEmbedder {
            dim: 8,
            max_length: 512,
        };
        let v1 = embedder.embed_query("hello").unwrap();
        let v2 = embedder.embed_query("goodbye").unwrap();
        assert_ne!(v1, v2);
    }
}
