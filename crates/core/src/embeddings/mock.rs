//! Fast deterministic embedding provider for tests.
//!
//! `MockEmbeddingProvider` produces 768-dimensional f32 vectors derived from
//! the input text via XXH3 hashing.  It is designed to:
//!
//! * Be **instantaneous** — no ONNX model is loaded or downloaded.
//! * Be **deterministic** — the same text always yields the same vector,
//!   so tests can assert that entity A is "closer" to entity B than to C by
//!   constructing inputs whose hashes differ predictably.
//! * Have **unit-norm** — each vector is L2-normalised, matching the
//!   convention used by jina-embeddings-v2-base-code, so cosine-distance
//!   queries against `entity_vectors` produce meaningful results in tests.
//!
//! **Do not use in production.** The mock vectors carry no semantic meaning;
//! only structural / indexing tests are valid with this provider.

use crate::embeddings::EMBEDDING_DIMENSION;
use crate::embeddings::provider::EmbeddingProvider;
use crate::error::Result;
use xxhash_rust::xxh3::xxh3_64;

/// Deterministic, zero-latency embedding provider for test and CI use.
pub struct MockEmbeddingProvider {
    dim: usize,
}

impl MockEmbeddingProvider {
    /// Create a mock provider with the standard embedding dimension.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dim: EMBEDDING_DIMENSION,
        }
    }

    /// Create a mock provider with a custom dimension (useful for schema tests).
    #[must_use]
    pub fn with_dim(dim: usize) -> Self {
        Self { dim }
    }

    /// Produce a unit-normalised vector from text by seeding an LCG from the
    /// XXH3 hash.  Different seeds → different pseudo-random floats → different
    /// unit vectors.  The same text always yields exactly the same vector.
    fn text_to_vec(&self, text: &str) -> Vec<f32> {
        let seed = xxh3_64(text.as_bytes());
        let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut v: Vec<f32> = (0..self.dim)
            .map(|_| {
                // 64-bit xorshift LCG step
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Map [0, u64::MAX] → [-1.0, 1.0]
                (state as i64 as f32) / (i64::MAX as f32)
            })
            .collect();

        // L2-normalise so cosine distance is well-defined
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

impl Default for MockEmbeddingProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingProvider for MockEmbeddingProvider {
    fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.text_to_vec(text))
    }

    fn generate_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.text_to_vec(t)).collect())
    }

    fn dimensions(&self) -> Result<usize> {
        Ok(self.dim)
    }

    fn model_name(&self) -> String {
        format!("mock-embedding-{}d", self.dim)
    }

    fn is_loaded(&self) -> bool {
        true // always ready, never needs to load
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_is_deterministic() {
        let p = MockEmbeddingProvider::new();
        let v1 = p.generate_embedding("hello world").unwrap();
        let v2 = p.generate_embedding("hello world").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_mock_different_inputs_different_vectors() {
        let p = MockEmbeddingProvider::new();
        let v1 = p.generate_embedding("function foo()").unwrap();
        let v2 = p.generate_embedding("class Bar {}").unwrap();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_mock_correct_dimension() {
        let p = MockEmbeddingProvider::new();
        let v = p.generate_embedding("test").unwrap();
        assert_eq!(v.len(), EMBEDDING_DIMENSION);
    }

    #[test]
    fn test_mock_is_unit_normalised() {
        let p = MockEmbeddingProvider::new();
        let v = p.generate_embedding("some code entity").unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
    }

    #[test]
    fn test_mock_batch_matches_single() {
        let p = MockEmbeddingProvider::new();
        let texts = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let batch = p.generate_embeddings_batch(&texts).unwrap();
        for (t, bv) in texts.iter().zip(batch.iter()) {
            let sv = p.generate_embedding(t).unwrap();
            assert_eq!(*bv, sv);
        }
    }
}
