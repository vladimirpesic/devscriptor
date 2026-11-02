//! Embedding service for semantic code search.
//!
//! Devscriptor uses **exactly one** embedding model: the INT8-quantized ONNX
//! export of `jinaai/jina-embeddings-v2-base-code` (768-dim,
//! `onnx/model_quantized.onnx`, ~160 MB on disk), whose matrix products run in
//! f32 over its weights dequantized at load (~1.25 GB resident when loaded).
//!
//! There is no fallback model and no adaptive model selection. This is a fixed
//! architectural decision; see docs/decisions.md.
//!
//! Which entities are embedded at all is governed by [`EmbeddingSelection`]:
//! substantive code units are embedded; non-semantic noise is skipped and stays
//! findable via the lexical `name` / `name_tokens` / FTS5 paths. This is
//! orthogonal to the model — it changes *which entities* get a vector, never the
//! model or its 768 dimensions.
//!
//! # Provider Abstraction
//!
//! The [`provider::EmbeddingProvider`] trait abstracts over the embedding
//! backend so callers are decoupled from any concrete backend. Use
//! [`JinaEmbeddingProvider`](provider::JinaEmbeddingProvider) for the standard local
//! model, or implement the trait for testing (see [`MockEmbeddingProvider`]).

mod dequantize;
pub mod mock;
pub mod model_fetch;
pub mod provider;
pub mod selection;
pub mod tract_backend;

use crate::embeddings::provider::EmbeddingProvider;
use crate::error::Result;
pub use mock::MockEmbeddingProvider;
pub use selection::{EmbeddingSelection, should_embed};
use std::sync::Arc;

/// Embedding dimension for `jinaai/jina-embeddings-v2-base-code`.
/// This is the only supported model; the constant is immutable.
pub const EMBEDDING_DIMENSION: usize = 768;

/// Hugging Face model code of the one supported embedding model.
pub const MODEL_CODE: &str = "jinaai/jina-embeddings-v2-base-code";

/// The vector space of the vectors Devscriptor stores: the model, its
/// revision, and how it computes. A change of either makes the stored vectors
/// incomparable with new ones, so an index stamped with another space drops
/// its vectors and its embedding cache when opened
/// ([`align_embedding_space`](crate::database::schema::align_embedding_space)).
pub const EMBEDDING_SPACE: &str = "jina-embeddings-v2-base-code@516f4baf/int8-weights/f32-compute";

/// Free RAM (MB) below which the inline embedding paths — `scan --embed`,
/// `watch --embed` and the MCP `start_watching` tool — leave embedding out
/// rather than risk running out of memory mid-scan. `devscriptor embed` (or
/// the MCP `generate_embeddings` tool) fills the vectors in later.
pub const MIN_INLINE_EMBED_FREE_MB: u64 = 4096;

/// Configuration for the embedding service.
///
/// The model is always `JinaEmbeddingsV2BaseCode` (768-dim). There are no
/// other supported models — do not add fallback model configuration here.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Show download progress bar.
    pub show_download_progress: bool,
    /// Texts per inference run, passed to `TractEmbedder::embed()`.
    ///
    /// One by default ([`compute_batch_size`](provider::JinaEmbeddingProvider::compute_batch_size)):
    /// a run is padded to its longest text, so batches of real code embed
    /// slower, and peak activation memory scales with `batch_size × seq_len²`.
    /// A batch changes no vector: the matrix products run in f32.
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            show_download_progress: true,
            batch_size: provider::JinaEmbeddingProvider::compute_batch_size(),
        }
    }
}

/// High-level embedding service that delegates to a [`provider::EmbeddingProvider`].
///
/// Constructed with [`EmbeddingService::new`] (uses [`provider::JinaEmbeddingProvider`])
/// or [`EmbeddingService::with_provider`] (test doubles, etc.).
#[derive(Clone)]
pub struct EmbeddingService {
    provider: Arc<dyn provider::EmbeddingProvider>,
    model_name: String,
}

impl EmbeddingService {
    /// Create a new embedding service backed by `JinaEmbeddingProvider`.
    ///
    /// The ONNX model is **lazy-loaded** on the first call to
    /// [`generate_embedding`](Self::generate_embedding) or
    /// [`generate_embeddings_batch`](Self::generate_embeddings_batch).
    /// This method never loads the model or occupies its ~1.3 GB of RAM.
    #[must_use]
    pub fn new(config: EmbeddingConfig) -> Self {
        let provider = Arc::new(provider::JinaEmbeddingProvider::new(config));
        let model_name = provider.model_name();
        Self {
            provider,
            model_name,
        }
    }

    /// Create a service backed by an arbitrary [`EmbeddingProvider`] implementation.
    ///
    /// Use this to plug in a [`MockEmbeddingProvider`] for tests or any other
    /// implementation that does not load a real model.
    #[must_use]
    pub fn with_provider<P: provider::EmbeddingProvider + 'static>(provider: P) -> Self {
        let provider = Arc::new(provider);
        let model_name = provider.model_name();
        Self {
            provider,
            model_name,
        }
    }

    /// Create a service with default configuration (Jina 768-dim, batch=16).
    #[must_use]
    pub fn default_service() -> Self {
        Self::new(EmbeddingConfig::default())
    }

    /// Returns `true` if the ONNX model sessions are currently loaded in RAM.
    #[must_use]
    pub fn is_model_loaded(&self) -> bool {
        self.provider.is_loaded()
    }

    /// Unload the shared model to free its RAM (~1.3 GB resident during embedding).
    ///
    /// The model is lazily reloaded on the next inference call.
    /// Call this after bulk embedding operations are complete.
    pub fn unload_model(&self) {
        self.provider.unload();
    }

    /// Number of embed batches to run concurrently — the `buffer_unordered`
    /// fan-out width.
    ///
    /// Equals `JinaEmbeddingProvider::compute_session_count()` for the production
    /// provider.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.provider.session_count()
    }

    /// Generate an embedding for a single piece of text.
    ///
    /// # Errors
    /// Returns an error if the model cannot be loaded or inference fails.
    pub fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        self.provider.generate_embedding(text)
    }

    /// Generate embeddings for multiple texts in one batched inference call.
    ///
    /// Significantly more efficient than repeated [`generate_embedding`](Self::generate_embedding)
    /// calls because the batch amortises per-call tokenization and inference setup.
    ///
    /// # Errors
    /// Returns an error if the model cannot be loaded or inference fails.
    pub fn generate_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.provider.generate_embeddings_batch(texts)
    }

    /// Return the dimensionality of produced vectors (always 768 for Jina).
    ///
    /// # Errors
    /// Returns an error if the model metadata cannot be queried.
    pub fn get_dimensions(&self) -> Result<usize> {
        self.provider.dimensions()
    }

    /// Return the model identifier string.
    #[must_use]
    pub fn get_model_name(&self) -> String {
        self.model_name.clone()
    }
}

impl Default for EmbeddingService {
    fn default() -> Self {
        Self::default_service()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_service_default_not_loaded() {
        let service = EmbeddingService::default();
        assert!(!service.is_model_loaded());
    }

    #[test]
    fn embedding_config_default_is_jina_768() {
        let config = EmbeddingConfig::default();
        // One text per run (see EmbeddingConfig::batch_size docs).
        if std::env::var("DEVSCRIPTOR_EMBEDDING_BATCH").is_err() {
            assert_eq!(config.batch_size, 1);
        }
        assert!(config.show_download_progress);
    }

    #[test]
    fn embedding_dimension_constant_is_768() {
        assert_eq!(EMBEDDING_DIMENSION, 768);
    }
}
