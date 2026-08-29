//! nomic-embed-text-v1.5 through llama.cpp on Metal, the embedder on macOS.
//!
//! No ONNX execution provider beats the CPU on Apple Silicon for this model
//! (#142), while llama.cpp's Metal path runs it at several times the ONNX
//! CPU throughput (#144). The f16 GGUF carries the same weights, and with
//! mean pooling, L2 normalization, no task prefix, and the model's own rope
//! settings its vectors agree with the ONNX ones to a median cosine of
//! 1.0000 (#144). A database embedded on either runtime therefore searches
//! correctly on the other, which is why the runtime is not part of
//! [`MODEL_NAME`] and a Mac joining a synced database does not re-embed.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::ops::Range;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;

use super::{Embedder, MODEL_NAME, gguf};

/// Same cap as the ONNX path. Above it the two runtimes diverge, because
/// fastembed truncates and llama.cpp would not.
const MAX_LENGTH: usize = 512;

/// Tokens per `llama_decode` call. A non-causal model needs every token of
/// a call in one micro-batch, and `llama-bench` on an M1 Max peaks near
/// this size (pp512 17.3k t/s, pp2048 13.1k t/s, in #144).
const BATCH_TOKENS: usize = 2048;

/// Sequences per `llama_decode` call, which is also the context's
/// `n_seq_max`. Short chunks would otherwise pack past what one call can
/// address.
const BATCH_SEQS: usize = 64;

/// Production embedder on macOS.
///
/// The model is loaded once per process and lives for its remainder, like
/// the backend; each instance owns a decode context over it.
pub struct LlamaEmbedModel {
    model: &'static LlamaModel,
    ctx: RefCell<LlamaContext<'static>>,
    dim: usize,
}

impl LlamaEmbedModel {
    pub fn new() -> Result<Self> {
        let model = model()?;
        let dim = usize::try_from(model.n_embd()).context("model reports a negative n_embd")?;
        let ctx = model
            .new_context(backend(), context_params())
            .context("creating llama.cpp context")?;
        log::debug!(
            "llama.cpp embedder ready (gpu offload supported: {})",
            backend().supports_gpu_offload()
        );
        Ok(Self {
            model,
            ctx: RefCell::new(ctx),
            dim,
        })
    }

    fn tokenize(&self, text: &str) -> Result<Vec<LlamaToken>> {
        let tokens = self
            .model
            .str_to_token(text, AddBos::Always)
            .with_context(|| format!("tokenizing {} bytes", text.len()))?;
        Ok(fit_to_max_length(tokens, MAX_LENGTH))
    }
}

fn backend() -> &'static LlamaBackend {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    BACKEND.get_or_init(|| {
        let mut backend = LlamaBackend::init()
            .expect("llama backend is initialized exactly once, through this OnceLock");
        // Left on, llama.cpp prints the whole model card and every buffer
        // allocation to stderr on load.
        backend.void_logs();
        backend
    })
}

fn model() -> Result<&'static LlamaModel> {
    static MODEL: OnceLock<LlamaModel> = OnceLock::new();
    if let Some(model) = MODEL.get() {
        return Ok(model);
    }
    let path = gguf::ensure_model_file(&gguf::NOMIC_F16)?;
    let params = LlamaModelParams::default().with_n_gpu_layers(1000);
    let loaded = LlamaModel::load_from_file(backend(), &path, &params)
        .with_context(|| format!("loading {}", path.display()))?;
    Ok(MODEL.get_or_init(|| loaded))
}

fn context_params() -> LlamaContextParams {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get() as i32);
    // llama.cpp divides n_ctx across n_seq_max, so each sequence's share must
    // still hold a full-length input.
    let n_ctx = (MAX_LENGTH * BATCH_SEQS) as u32;
    LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(n_ctx))
        .with_n_batch(BATCH_TOKENS as u32)
        .with_n_ubatch(BATCH_TOKENS as u32)
        .with_n_seq_max(BATCH_SEQS as u32)
        .with_n_threads(threads)
        .with_n_threads_batch(threads)
        .with_pooling_type(LlamaPoolingType::Mean)
        .with_embeddings(true)
}

impl Embedder for LlamaEmbedModel {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let sequences = texts
            .iter()
            .map(|t| self.tokenize(t))
            .collect::<Result<Vec<_>>>()?;
        let lens: Vec<usize> = sequences.iter().map(Vec::len).collect();

        let mut ctx = self.ctx.borrow_mut();
        let mut out = Vec::with_capacity(texts.len());
        for range in plan_batches(&lens, BATCH_TOKENS, BATCH_SEQS) {
            out.extend(decode_sequences(&mut ctx, &sequences[range])?);
        }
        Ok(out)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_batch(&[text])?
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
        MAX_LENGTH
    }
}

/// Run one decode over `sequences` and pool each into a unit vector.
fn decode_sequences(
    ctx: &mut LlamaContext<'_>,
    sequences: &[Vec<LlamaToken>],
) -> Result<Vec<Vec<f32>>> {
    let n_tokens = sequences.iter().map(Vec::len).sum();
    let mut batch = LlamaBatch::new(n_tokens, 1);
    for (seq_id, tokens) in (0..).zip(sequences) {
        batch.add_sequence(tokens, seq_id, false)?;
    }
    ctx.decode(&mut batch).context("llama_decode failed")?;

    (0..sequences.len())
        .map(|i| {
            let pooled = ctx.embeddings_seq_ith(i as i32)?;
            Ok(l2_normalize(pooled))
        })
        .collect()
}

/// Truncate a tokenized input to `max` tokens the way fastembed does:
/// the trailing separator survives, the content before it is cut.
fn fit_to_max_length(mut tokens: Vec<LlamaToken>, max: usize) -> Vec<LlamaToken> {
    if tokens.len() > max {
        let sep = tokens[tokens.len() - 1];
        tokens.truncate(max - 1);
        tokens.push(sep);
    }
    tokens
}

/// Split consecutive sequences (given by token count) into decode calls,
/// each within both budgets. A sequence longer than `max_tokens` gets a
/// call to itself rather than being dropped.
fn plan_batches(lens: &[usize], max_tokens: usize, max_seqs: usize) -> Vec<Range<usize>> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut tokens = 0;
    for (i, &len) in lens.iter().enumerate() {
        let full = i > start && (tokens + len > max_tokens || i - start >= max_seqs);
        if full {
            batches.push(start..i);
            start = i;
            tokens = 0;
        }
        tokens += len;
    }
    if start < lens.len() {
        batches.push(start..lens.len());
    }
    batches
}

fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(ids: impl IntoIterator<Item = i32>) -> Vec<LlamaToken> {
        ids.into_iter().map(LlamaToken).collect()
    }

    #[test]
    fn short_input_is_left_alone() {
        let input = tokens([101, 7, 8, 102]);
        assert_eq!(fit_to_max_length(input.clone(), 4), input);
    }

    #[test]
    fn long_input_keeps_its_separator() {
        let input = tokens([101, 1, 2, 3, 4, 102]);
        assert_eq!(fit_to_max_length(input, 4), tokens([101, 1, 2, 102]));
    }

    #[test]
    fn batches_split_on_the_token_budget() {
        assert_eq!(
            plan_batches(&[300, 300, 300, 300], 700, 64),
            vec![0..2, 2..4]
        );
    }

    #[test]
    fn batches_split_on_the_sequence_budget() {
        assert_eq!(
            plan_batches(&[1, 1, 1, 1, 1], 1000, 2),
            vec![0..2, 2..4, 4..5]
        );
    }

    #[test]
    fn oversized_sequence_gets_its_own_batch() {
        assert_eq!(
            plan_batches(&[10, 900, 10], 500, 64),
            vec![0..1, 1..2, 2..3]
        );
    }

    #[test]
    fn no_sequences_means_no_batches() {
        assert!(plan_batches(&[], 500, 64).is_empty());
    }

    #[test]
    fn normalized_vector_has_unit_length() {
        assert_eq!(l2_normalize(&[3.0, 4.0]), vec![0.6, 0.8]);
    }

    #[test]
    fn zero_vector_stays_zero() {
        assert_eq!(l2_normalize(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn a_full_length_input_fits_one_decode_call() {
        assert!(BATCH_TOKENS >= MAX_LENGTH);
    }
}
