use std::env;
use std::sync::OnceLock;

use anyhow::Result;

use crate::platform;

mod onnx;

/// The embedder production code loads. Its vectors are what
/// [`MODEL_NAME`] promises regardless of the runtime behind it.
pub use onnx::FastEmbedModel as ProductionEmbedder;

/// Trait for embedding models.
pub trait Embedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
    fn dimension(&self) -> usize;
    fn model_name(&self) -> &str;
    /// Maximum input length in tokens that the model accepts.
    fn max_length(&self) -> usize;
}

/// Identity of the production embedder, exposed so status checks can compare
/// stored metadata against it without loading the ONNX model.
pub const MODEL_NAME: &str = "nomic-embed-text-v1.5";

/// The model cache directory (in the platform data directory), created if
/// missing. Models must cache here rather than in a CWD-relative directory.
pub(crate) fn hf_cache_dir() -> Result<std::path::PathBuf> {
    let cache_dir = platform::data_dir()?.join("fastembed_cache");
    std::fs::create_dir_all(&cache_dir)?;
    Ok(cache_dir)
}

/// Set HF_HOME so fastembed's hf-hub downloads land in [`hf_cache_dir`].
///
/// Callers must invoke this before spawning any thread that might read the
/// process environment. Model loader threads do read it, not for HF_HOME
/// but for the platform paths (XDG_DATA_HOME, HOME, USERPROFILE) and
/// hf-hub's HF_ENDPOINT and HF_TOKEN, and setenv racing any getenv is
/// undefined behavior regardless of which variable each touches. The write
/// happens at most once per process; later calls only re-resolve the cache
/// directory, so a failed [`hf_cache_dir`] is retried rather than memoized.
pub(crate) fn set_hf_cache_dir() -> Result<()> {
    static HF_HOME: OnceLock<()> = OnceLock::new();

    let cache_dir = hf_cache_dir()?;
    HF_HOME.get_or_init(|| {
        // SAFETY: sound only under the contract above. The first caller
        // runs before any environment-reading thread exists, so
        // thread::spawn's happens-before edge (not this OnceLock, which
        // those readers never touch) orders the write ahead of every
        // getenv on a later thread. What the OnceLock adds is that the
        // write cannot recur once loader threads are running: every call
        // after the first is read-only.
        unsafe {
            env::set_var("HF_HOME", cache_dir);
        }
    });
    Ok(())
}

/// Hardware execution providers enabled by cargo features. CPU is always
/// the implicit fallback in ort.
pub(crate) fn execution_providers() -> Vec<ort::execution_providers::ExecutionProviderDispatch> {
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

    providers
}

/// Whether this binary was built with a hardware execution provider
/// (cuda/directml/coreml). When false, embedding runs on CPU.
pub(crate) fn has_hardware_provider() -> bool {
    !execution_providers().is_empty()
}

/// Whether a CPU embedding run is worth warning about.
///
/// It is not on macOS, even though macOS does embed on CPU. The only
/// hardware provider Apple Silicon offers is CoreML, and CoreML loses to
/// the CPU on this model (see the `coreml` feature note in Cargo.toml), so
/// release builds leave it off on purpose. Pointing a macOS user at the
/// GPU build features would be advice to go build a slower binary. A macOS
/// user who opted into `--features coreml` anyway is excluded by the
/// hardware-provider check and gets no warning either.
pub(crate) fn should_warn_cpu_only() -> bool {
    !has_hardware_provider() && !cfg!(target_os = "macos")
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
