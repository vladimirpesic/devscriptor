//! Embedding provider trait and implementations.
//!
//! This module defines the [`EmbeddingProvider`] trait which abstracts over
//! different embedding backends (tract, mock, etc.). The trait is used by
//! [`EmbeddingService`](super::EmbeddingService) so callers are decoupled from
//! any concrete backend.

use std::sync::Arc;

use crate::embeddings::tract_backend::TractEmbedder;
use crate::error::{Error, Result};

/// Abstraction over an embedding backend.
///
/// Implementations are responsible for loading their model (if any), generating
/// embeddings, and reporting metadata such as dimensionality.
///
/// # Concurrency
///
/// The trait requires `Send + Sync` because implementations are typically stored
/// inside an `Arc` and accessed from multiple async tasks (often via
/// `tokio::task::spawn_blocking`).
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding for a single piece of text.
    ///
    /// # Errors
    /// Returns [`Error::Embedding`] when the backend fails to produce a vector.
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>>;

    /// Generate embeddings for many texts in one batch.
    ///
    /// # Errors
    /// Returns [`Error::Embedding`] when the backend fails.
    fn generate_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Return the dimensionality of the produced vectors.
    ///
    /// # Errors
    /// Returns [`Error::Embedding`] when the dimension cannot be determined.
    fn dimensions(&self) -> Result<usize>;

    /// Human-readable model identifier (e.g. `jinaai/jina-embeddings-v2-base-code`).
    fn model_name(&self) -> String;

    /// Returns `true` when the provider's model is loaded in memory.
    fn is_loaded(&self) -> bool {
        true
    }

    /// Unload the model from memory to free resources.
    fn unload(&self) {}

    /// Recommended number of embed batches to run concurrently (see
    /// [`JinaEmbeddingProvider::compute_session_count`]).
    fn session_count(&self) -> usize {
        1
    }
}

/// Local embedding provider backed by the pure-Rust `tract` engine.
///
/// # One shared model
///
/// tract's runnable plan has a concurrency-safe `run(&self)`, so the model is
/// loaded **once** and shared behind an `Arc`: ~1.25 GB resident with its
/// weights in f32, and each concurrent run adds only its activations, not
/// another model copy. Every parallel embed task runs on that one plan; the only
/// lock is a brief read to clone the `Arc`, never held during inference.
///
/// # Parallelism
///
/// [`session_count`](Self::session_count) reports a RAM-aware number of
/// *concurrent runs* the pipeline should schedule (`buffer_unordered`), since
/// each in-flight run consumes activation memory.
pub struct JinaEmbeddingProvider {
    config: super::EmbeddingConfig,
    model: parking_lot::RwLock<Option<Arc<TractEmbedder>>>,
}

impl JinaEmbeddingProvider {
    /// Create a new provider. The model is lazily loaded on first use.
    #[must_use]
    pub fn new(config: super::EmbeddingConfig) -> Self {
        Self {
            config,
            model: parking_lot::RwLock::new(None),
        }
    }

    /// Shared model + fixed working-set resident (MB), loaded once: the model
    /// with its weights dequantized to f32 measured 1.25 GB after load.
    /// Everything beyond this scales with the number and size of in-flight
    /// runs.
    const MODEL_BASE_MB: u64 = 1300;

    /// Worst-case activation memory per embedded entity at the 512-token cap
    /// (`seq_len² × hidden`), in MB. Measured ~86 MB/item for near-max-length code
    /// entities; rounded up for headroom. A run costs ≈ `batch × PER_ITEM_MB`,
    /// so this is what makes the auto-tuner safe on all-long-entity workloads.
    const PER_ITEM_MB: u64 = 100;

    /// RAM left untouched for the OS, an editor, language servers, the SQLite
    /// cache, and the rest of the process.
    const HEADROOM_MB: u64 = 1536;

    /// Runs at once unless RAM says fewer (see
    /// [`compute_session_count`](Self::compute_session_count)).
    const PREFERRED_SESSIONS: usize = 2;

    /// The most runs at once `DEVSCRIPTOR_EMBEDDING_SESSIONS` can ask for.
    const MAX_SESSIONS: usize = 8;

    /// Concurrent embed count: two runs at once, each with its share of the
    /// cores as matmul threads ([`compute_thread_count`](Self::compute_thread_count)).
    /// The model's f32 matrix products speed up with threads, and two runs
    /// keep the cores busy between them: on 12 cores, 2 runs × 6 threads
    /// embedded 5.4 texts/s, against 4.9 for 1 × 12, 4.7 for 3 × 4, 3.8 for
    /// 4 × 3 and 1.5 for 8 × 1.
    ///
    /// Steps **down** to one run if two runs of
    /// [`compute_batch_size`](Self::compute_batch_size) texts would not fit the
    /// RAM budget — so it never risks OOM, even on an all-long-entity
    /// workload. `DEVSCRIPTOR_EMBEDDING_SESSIONS` overrides (clamped 1..=8);
    /// `cfg!(test)` forces 1.
    pub fn compute_session_count() -> usize {
        Self::compute_session_count_for(Self::compute_batch_size())
    }

    /// [`compute_session_count`](Self::compute_session_count) for runs of
    /// `batch_size` texts, such as an explicit `--batch-size`.
    pub fn compute_session_count_for(batch_size: usize) -> usize {
        if let Ok(env) = std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS")
            && let Ok(n) = env.parse::<usize>()
        {
            return n.clamp(1, Self::MAX_SESSIONS);
        }
        if cfg!(test) {
            return 1;
        }
        Self::sessions_for(crate::utils::free_ram_mb(), num_cpus::get(), batch_size)
    }

    /// The concurrent runs of `batch_size` texts for a machine with `free_mb`
    /// of free RAM and `cores` cores: [`Self::PREFERRED_SESSIONS`], fewer if
    /// the cores or the RAM left beside the model and the headroom say so.
    fn sessions_for(free_mb: u64, cores: usize, batch_size: usize) -> usize {
        let budget = free_mb
            .saturating_sub(Self::HEADROOM_MB)
            .saturating_sub(Self::MODEL_BASE_MB);
        let per_run_mb = batch_size.max(1) as u64 * Self::PER_ITEM_MB;
        let mut runs = Self::PREFERRED_SESSIONS.min(cores).max(1);
        while runs > 1 && runs as u64 * per_run_mb > budget {
            runs -= 1;
        }
        runs
    }

    /// Texts per inference run: one, unless `DEVSCRIPTOR_EMBEDDING_BATCH` (or
    /// an explicit `--batch-size`) says otherwise.
    ///
    /// A batch is padded to its longest text, and real code varies in length:
    /// on 200 entities, one text per run embedded 5.6 texts/s, against 3.2 in
    /// batches of 4, 2.3 of 8 and 2.0 of 16, which also took up to 0.8 GB more
    /// memory. Batching changes no vector: the matrix products
    /// run in f32, so a text's vector does not depend on the texts beside it.
    /// Throughput comes from running
    /// [`compute_session_count`](Self::compute_session_count) texts at once,
    /// each with its share of the cores.
    pub fn compute_batch_size() -> usize {
        if let Ok(env) = std::env::var("DEVSCRIPTOR_EMBEDDING_BATCH")
            && let Ok(n) = env.parse::<usize>()
            && n >= 1
        {
            return n.min(256);
        }
        1
    }

    /// Intra-run matmul threads (tract `multithread-mm`): the cores shared out
    /// among the concurrent runs, so that together they use every core.
    /// `DEVSCRIPTOR_EMBEDDING_THREADS` overrides; `cfg!(test)` forces 1.
    pub fn compute_thread_count() -> usize {
        if let Ok(env) = std::env::var("DEVSCRIPTOR_EMBEDDING_THREADS")
            && let Ok(n) = env.parse::<usize>()
            && n >= 1
        {
            return n;
        }
        if cfg!(test) {
            return 1;
        }
        (num_cpus::get() / Self::compute_session_count().max(1)).max(1)
    }

    /// Return the shared embedder, loading it (once) on first use.
    fn embedder(&self) -> Result<Arc<TractEmbedder>> {
        if let Some(m) = self.model.read().as_ref() {
            return Ok(Arc::clone(m));
        }
        let mut guard = self.model.write();
        if let Some(m) = guard.as_ref() {
            return Ok(Arc::clone(m));
        }
        // Resolve the local model cache and provision it from the Devscriptor
        // mirror if any files are missing. With `model-download` disabled this
        // only verifies the files are already present (airgapped builds).
        let cache_dir = super::model_fetch::model_cache_dir()?;
        super::model_fetch::ensure_model_present(&cache_dir, self.config.show_download_progress)?;
        super::tract_backend::set_matmul_threads(Self::compute_thread_count());
        tracing::info!("Loading Jina embedding model (tract) ...");
        let embedder = Arc::new(TractEmbedder::load(&cache_dir)?);
        *guard = Some(Arc::clone(&embedder));
        tracing::info!(
            "Embedding model loaded (one shared plan, {} dims)",
            embedder.dim()
        );
        Ok(embedder)
    }
}

impl EmbeddingProvider for JinaEmbeddingProvider {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let embedder = self.embedder()?;
        embedder
            .embed(&[text], 1)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Embedding("No embedding generated".to_string()))
    }

    fn generate_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let embedder = self.embedder()?;
        let docs: Vec<&str> = texts.iter().map(String::as_str).collect();
        embedder.embed(&docs, self.config.batch_size)
    }

    fn dimensions(&self) -> Result<usize> {
        Ok(super::EMBEDDING_DIMENSION)
    }

    fn model_name(&self) -> String {
        super::MODEL_CODE.to_string()
    }

    fn is_loaded(&self) -> bool {
        self.model.read().is_some()
    }

    fn unload(&self) {
        *self.model.write() = None;
        tracing::info!("JinaEmbeddingProvider: model unloaded");
    }

    fn session_count(&self) -> usize {
        Self::compute_session_count_for(self.config.batch_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_model_name_and_dims() {
        let provider = JinaEmbeddingProvider::new(super::super::EmbeddingConfig::default());
        assert_eq!(provider.model_name(), "jinaai/jina-embeddings-v2-base-code");
        assert_eq!(provider.dimensions().unwrap(), 768);
    }

    #[test]
    fn test_provider_is_object_safe() {
        let provider = JinaEmbeddingProvider::new(super::super::EmbeddingConfig::default());
        let _: &dyn EmbeddingProvider = &provider;
    }

    #[test]
    fn test_session_count_is_in_range() {
        if std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").is_ok() {
            return;
        }
        let count = JinaEmbeddingProvider::compute_session_count();
        assert!(
            (1..=8).contains(&count),
            "session_count {count} out of expected range [1, 8]"
        );
    }

    #[test]
    fn test_session_count_is_one_in_tests_without_override() {
        if std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").is_ok() {
            return;
        }
        assert_eq!(JinaEmbeddingProvider::compute_session_count(), 1);
    }

    /// Env var override takes effect even inside `cfg!(test)`. Must run serially
    /// (mutates a process-global env var).
    #[test]
    fn test_session_count_env_override_in_tests() {
        let prev = std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").ok();
        // SAFETY: single-threaded test; no concurrent readers of this var.
        unsafe { std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", "3") };
        let count = JinaEmbeddingProvider::compute_session_count();
        // SAFETY: restoring previous state.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", v),
                None => std::env::remove_var("DEVSCRIPTOR_EMBEDDING_SESSIONS"),
            }
        }
        assert_eq!(count, 3, "env override must win over cfg!(test)");
    }

    /// One text per run unless overridden. Must run serially with the other
    /// env-var tests (it reads a process-global variable).
    #[test]
    fn test_the_default_batch_is_one_text() {
        if std::env::var("DEVSCRIPTOR_EMBEDDING_BATCH").is_err() {
            assert_eq!(JinaEmbeddingProvider::compute_batch_size(), 1);
        }
    }

    /// The auto-tuner's plan (runs × one text) never exceeds free RAM at or
    /// above the 4 GB floor, runs two at once when RAM allows, and steps down
    /// to one when RAM or the cores are short.
    #[test]
    fn test_auto_tuner_is_memory_safe() {
        use JinaEmbeddingProvider as P;
        for &(free, cores) in &[(4096u64, 4usize), (4096, 12), (8192, 8), (16384, 16)] {
            let runs = P::sessions_for(free, cores, 1);
            let used = P::HEADROOM_MB + P::MODEL_BASE_MB + runs as u64 * P::PER_ITEM_MB;
            assert!(
                used <= free,
                "plan {free}MB/{cores}c OOM: {runs} runs, {used} MB"
            );
            assert_eq!(runs, P::PREFERRED_SESSIONS, "{free} MB holds two runs");
        }
        assert_eq!(P::sessions_for(8192, 1, 1), 1, "one core, one run");
        assert_eq!(P::sessions_for(2_900, 8, 1), 1, "tight RAM, one run");
        assert_eq!(
            P::sessions_for(3_100, 8, 4),
            1,
            "two runs of four texts do not fit"
        );
    }
}
