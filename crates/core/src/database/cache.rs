//! Database cache layer for query results
//!
//! This module provides LRU caching for database query results to improve
//! performance by avoiding repeated expensive queries.

use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Statistics for cache monitoring
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of entries evicted (tracked externally where possible)
    pub evictions: u64,
    /// Number of entries currently in cache
    pub size: u64,
    /// Maximum cache size
    pub max_size: u64,
    /// Hit rate (0.0 to 1.0)
    pub hit_rate: f64,
}

/// Generic cache with TTL and O(1) LRU eviction.
///
/// Backed by `moka::sync::Cache` which provides lock-free concurrent access,
/// automatic LRU eviction, and TTL expiration without a background task.
pub struct QueryCache {
    /// Underlying moka cache (O(1) ops, built-in LRU + TTL)
    inner: moka::sync::Cache<String, Arc<dyn Any + Send + Sync>>,
    /// Hit count
    hits: AtomicU64,
    /// Miss count
    misses: AtomicU64,
    /// Configured maximum size
    max_size: u64,
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCache")
            .field("size", &self.inner.entry_count())
            .field("max_size", &self.max_size)
            .field("hits", &self.hits.load(Ordering::Relaxed))
            .field("misses", &self.misses.load(Ordering::Relaxed))
            .finish()
    }
}

impl QueryCache {
    /// Create a new query cache with specified size and TTL.
    #[must_use]
    pub fn new(max_size: u64, ttl_seconds: u64) -> Self {
        Self {
            inner: moka::sync::Cache::builder()
                .max_capacity(max_size)
                .time_to_live(Duration::from_secs(ttl_seconds))
                .build(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            max_size,
        }
    }

    /// Get a value from cache.
    #[must_use]
    pub fn get<T>(&self, key: &str) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        match self.inner.get(key) {
            Some(entry) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                if let Some(val) = entry.downcast_ref::<T>().cloned() {
                    return Some(val);
                }
                // Key exists but type doesn't match — warn so callers can debug.
                tracing::warn!(
                    "Cache key '{}' found but type mismatch (requested {})",
                    key,
                    std::any::type_name::<T>()
                );
                None
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Insert a value into cache.
    pub fn insert<T>(&self, key: String, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.inner.insert(key, Arc::new(value));
    }

    /// Invalidate a specific cache entry.
    pub fn invalidate(&self, key: &str) {
        self.inner.invalidate(key);
    }

    /// Clear all cache entries.
    pub fn clear(&self) {
        self.inner.invalidate_all();
    }

    /// Get cache statistics.
    #[must_use]
    pub fn get_stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;

        CacheStats {
            hits,
            misses,
            evictions: 0, // moka does not expose eviction counts
            size: self.inner.entry_count(),
            max_size: self.max_size,
            hit_rate: if total > 0 {
                hits as f64 / total as f64
            } else {
                0.0
            },
        }
    }

    /// Run maintenance tasks (moka handles eviction internally).
    pub fn run_pending_tasks(&self) {
        // moka::sync::Cache processes invalidations and TTL expiry
        // synchronously; no explicit flush is required.
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(10_000, 300)
    }
}

/// Manager for multiple caches
#[derive(Debug)]
pub struct CacheManager {
    /// Query result cache
    pub query_results: QueryCache,
}

impl CacheManager {
    /// Create a new cache manager with default settings
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new cache manager with custom settings
    #[must_use]
    pub fn with_settings(query_cache_size: u64, query_ttl: u64) -> Self {
        Self {
            query_results: QueryCache::new(query_cache_size, query_ttl),
        }
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        self.query_results.clear();
    }

    /// Get statistics for all caches
    #[must_use]
    pub fn get_stats(&self) -> HashMap<String, CacheStats> {
        let mut stats = HashMap::new();
        stats.insert("query_results".to_string(), self.query_results.get_stats());
        stats
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self {
            query_results: QueryCache::new(10_000, 300),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_cache_basic() {
        let cache = QueryCache::new(100, 300);
        cache.insert("test".to_string(), 42i32);

        let result: Option<i32> = cache.get("test");
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_query_cache_miss() {
        let cache = QueryCache::new(100, 300);

        let result: Option<i32> = cache.get("nonexistent");
        assert_eq!(result, None);
    }

    #[test]
    fn test_cache_stats() {
        let cache = QueryCache::new(100, 300);
        cache.insert("test".to_string(), 42i32);

        let _: Option<i32> = cache.get("test");
        let _: Option<i32> = cache.get("missing");

        let stats = cache.get_stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_cache_manager() {
        let manager = CacheManager::new();
        manager
            .query_results
            .insert("key".to_string(), "value".to_string());

        let result: Option<String> = manager.query_results.get("key");
        assert_eq!(result, Some("value".to_string()));
    }

    #[test]
    fn test_cache_ttl_eviction() {
        let cache = QueryCache::new(100, 1); // 1-second TTL
        cache.insert("key".to_string(), 42i32);

        assert_eq!(cache.get::<i32>("key"), Some(42));
        std::thread::sleep(Duration::from_secs(2));
        cache.run_pending_tasks();

        // After TTL, entry may be absent (moka eviction is lazy,
        // but `run_pending_tasks` encourages cleanup).
        let result: Option<i32> = cache.get("key");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_capacity_eviction() {
        let cache = QueryCache::new(3, 300); // max 3 entries
        cache.insert("a".to_string(), 1i32);
        cache.insert("b".to_string(), 2i32);
        cache.insert("c".to_string(), 3i32);
        cache.insert("d".to_string(), 4i32); // should evict oldest

        cache.run_pending_tasks();

        let size = cache.get_stats().size;
        assert!(size <= 3, "expected at most 3 entries, got {}", size);
    }
}
