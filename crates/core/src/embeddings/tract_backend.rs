//! Pure-Rust embedding backend built on [`tract`](https://github.com/sonos/tract).
//!
//! tract runs the INT8-quantized `jina-embeddings-v2-base-code` ONNX graph in
//! pure Rust, so the Devscriptor binary has **no** C++ / `libstdc++` /
//! `libonnxruntime` dependency and links against an old glibc cleanly (see
//! docs/building.md). Its matrix products run in f32, over the weights
//! dequantized at load (`embeddings/dequantize.rs`).
//!
//! ## Concurrency
//!
//! A [`TractEmbedder`] holds a single optimized runnable plan whose `run(&self)`
//! is concurrency-safe (each call builds its own execution state). One embedder
//! is therefore loaded **once** and shared behind an `Arc` across every parallel
//! embed task — there is no per-session weight duplication. The loaded model is
//! ~1.25 GB resident (measured); each concurrent run adds only its activations,
//! not another model copy.
//!
//! ## Equivalence
//!
//! Against the unquantized model of the same revision, the pooled+normalized
//! vectors of 600 entities have a mean cosine similarity of 0.9963, and 96.9% of
//! their ten nearest neighbours are the same; the integer products of the
//! quantized graph give 0.9919 and 92.8%.

use std::path::Path;
use std::sync::Arc;

use tract_onnx::prelude::*;

use crate::error::{Error, Result};

/// Maximum input sequence length (tokens). Activation memory scales with
/// sequence length, so this is pinned rather than left to the tokenizer default.
/// 512 tokens (~2 KB of code) is ample for entity-level semantic search.
const MAX_LENGTH: usize = 512;

/// Output dimensionality of `jina-embeddings-v2-base-code`.
const DIM: usize = super::EMBEDDING_DIMENSION;

/// Set tract's shared matmul thread pool, once per process.
///
/// tract runs single-threaded by default; the `multithread-mm` feature lets each
/// `run` spread its (large) matmuls across a shared rayon pool. `threads` comes
/// from [`JinaEmbeddingProvider::compute_thread_count`](super::provider::JinaEmbeddingProvider::compute_thread_count):
/// it is > 1 only when cross-batch concurrency is RAM-limited below the core
/// count, so this backfills the idle cores at no extra memory. The pool is global
/// and shared across all concurrent runs, so this is idempotent.
pub fn set_matmul_threads(threads: usize) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if threads > 1 {
            tract_linalg::multithread::set_default_executor(
                tract_linalg::multithread::Executor::multithread(threads),
            );
        }
        tracing::info!("tract matmul threads: {threads}");
    });
}

/// A loaded, optimized tract plan plus its tokenizer. Cheap to share via `Arc`.
pub struct TractEmbedder {
    plan: Arc<TypedRunnableModel>,
    tokenizer: tokenizers::Tokenizer,
}

impl TractEmbedder {
    /// Load the INT8 ONNX model, with its matrix products rewritten to run in
    /// f32, and the tokenizer from the local model cache.
    ///
    /// The graph's `batch`/`sequence` dimensions are kept symbolic and optimized
    /// as such (pinning them to concrete values trips a shape-unification bug in
    /// this model's attention-mask subgraph); concrete shapes are supplied per
    /// run instead.
    ///
    /// # Errors
    /// Returns [`Error::Embedding`] if the ONNX or tokenizer files are missing or
    /// fail to load/optimize.
    pub fn load(cache_dir: &Path) -> Result<Self> {
        let onnx = cache_dir.join("onnx/model_quantized.onnx");
        let failed = |e: TractError| {
            Error::Embedding(format!("tract failed to load {}: {e:#}", onnx.display()))
        };
        let mut proto = tract_onnx::onnx()
            .proto_model_for_path(&onnx)
            .map_err(failed)?;
        let (dequantized, quantized) = super::dequantize::dequantize_matmuls(&mut proto)?;
        tracing::debug!("{dequantized} of {quantized} quantized matrix products run in f32");
        let plan = tract_onnx::onnx()
            .model_for_proto_model(&proto)
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(failed)?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(cache_dir.join("tokenizer.json"))
            .map_err(|e| Error::Embedding(format!("failed to load tokenizer.json: {e}")))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_LENGTH,
                ..Default::default()
            }))
            .map_err(|e| Error::Embedding(format!("tokenizer truncation config: {e}")))?;

        Ok(Self { plan, tokenizer })
    }

    /// Vector dimensionality.
    pub const fn dim(&self) -> usize {
        DIM
    }

    /// Embed `texts` in batches of `batch_size`, formed from texts of similar
    /// length (see `length_sorted_batches`). The vectors come back in the
    /// order of `texts`.
    ///
    /// # Errors
    /// Returns [`Error::Embedding`] on tokenization or inference failure.
    pub fn embed(&self, texts: &[&str], batch_size: usize) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| Error::Embedding(format!("tokenization failed: {e}")))?;
        let lengths: Vec<usize> = encodings.iter().map(|e| e.get_ids().len()).collect();

        let mut out = vec![Vec::new(); texts.len()];
        for batch in length_sorted_batches(&lengths, batch_size) {
            let members: Vec<&tokenizers::Encoding> =
                batch.iter().map(|&i| &encodings[i]).collect();
            for (&i, vector) in batch.iter().zip(self.embed_batch(&members)?) {
                out[i] = vector;
            }
        }
        Ok(out)
    }

    /// Embed one padded batch of tokenized texts: tract inference → masked
    /// mean-pool → L2-normalize, the mean pooling jina-embeddings-v2 models are
    /// used with.
    fn embed_batch(&self, encodings: &[&tokenizers::Encoding]) -> Result<Vec<Vec<f32>>> {
        if encodings.is_empty() {
            return Ok(Vec::new());
        }

        let n = encodings.len();
        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(1)
            .max(1);

        // Right-pad to the batch's longest sequence; padded positions get
        // attention_mask 0 so they are ignored by the model and the pooling.
        let mut ids = vec![0i64; n * max_len];
        let mut mask = vec![0i64; n * max_len];
        for (r, enc) in encodings.iter().enumerate() {
            for (t, (&id, &m)) in enc
                .get_ids()
                .iter()
                .zip(enc.get_attention_mask())
                .enumerate()
            {
                ids[r * max_len + t] = i64::from(id);
                mask[r * max_len + t] = i64::from(m);
            }
        }

        let ids_t: Tensor = tract_ndarray::Array2::from_shape_vec((n, max_len), ids)
            .map_err(|e| Error::Embedding(format!("input shape: {e}")))?
            .into();
        let mask_t: Tensor = tract_ndarray::Array2::from_shape_vec((n, max_len), mask.clone())
            .map_err(|e| Error::Embedding(format!("mask shape: {e}")))?
            .into();

        let outputs = self
            .plan
            .run(tvec!(ids_t.into(), mask_t.into()))
            .map_err(|e| Error::Embedding(format!("tract inference failed: {e:#}")))?;

        // Output is the last hidden state, shape [n, max_len, DIM], f32.
        let hidden = outputs[0].clone().into_tensor();
        let total: usize = hidden.shape().iter().product();
        let data = unsafe {
            std::slice::from_raw_parts(
                hidden
                    .as_ptr::<f32>()
                    .map_err(|e| Error::Embedding(format!("output not f32: {e}")))?,
                total,
            )
        };

        Ok((0..n)
            .map(|r| mean_pool_normalize(data, r, max_len, &mask))
            .collect())
    }
}

/// The order to embed texts of token counts `lengths` in, cut into batches of
/// `batch_size`: shortest first, so that each batch holds texts of similar
/// length.
///
/// A batch is padded to its longest text, and its cost grows with that length
/// — its attention with the square of it. Batched in the order given, one long
/// entity would make every short one beside it pay for its length.
fn length_sorted_batches(lengths: &[usize], batch_size: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&i| lengths[i]);
    order
        .chunks(batch_size.max(1))
        .map(<[usize]>::to_vec)
        .collect()
}

/// Masked mean-pool row `r` of a `[n, seq, DIM]` flat buffer, then L2-normalize.
fn mean_pool_normalize(hidden: &[f32], r: usize, seq: usize, mask: &[i64]) -> Vec<f32> {
    let base = r * seq * DIM;
    let mut out = vec![0f32; DIM];
    let mut count = 0f32;
    for t in 0..seq {
        if mask[r * seq + t] != 0 {
            count += 1.0;
            let off = base + t * DIM;
            for (o, h) in out.iter_mut().zip(&hidden[off..off + DIM]) {
                *o += *h;
            }
        }
    }
    if count > 0.0 {
        for o in &mut out {
            *o /= count;
        }
    }
    let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for o in &mut out {
            *o /= norm;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts are batched shortest first, each exactly once, ties in their
    /// order.
    #[test]
    fn batches_hold_texts_of_similar_length() {
        let lengths = [500, 12, 480, 9, 30, 12, 512];
        assert_eq!(
            length_sorted_batches(&lengths, 3),
            vec![vec![3, 1, 5], vec![4, 2, 0], vec![6]]
        );
        assert_eq!(length_sorted_batches(&lengths, 0).len(), lengths.len());
        assert!(length_sorted_batches(&[], 4).is_empty());
    }
}
