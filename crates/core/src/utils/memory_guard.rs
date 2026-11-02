//! Runtime memory guard for pipeline backpressure.
//!
//! Prevents OOM on large repositories by checking available system RAM
//! before spawning CPU-intensive tasks and throttling when necessary.
//!
//! # Performance (NF-1 fix)
//!
//! Each thread keeps one `sysinfo::System` and only calls `refresh_memory()`
//! on it, which reads `/proc/meminfo` (or the platform equivalent).

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// One `sysinfo::System` per thread, created empty: only memory is ever read,
// and `refresh_memory()` fills it in. `System::new_all()` would read every
// process and CPU on each thread that checks memory (the rayon and tokio
// workers among them) and keep that whole process table.
thread_local! {
    static SYS_CACHED: RefCell<sysinfo::System> = RefCell::new(sysinfo::System::new());
}

/// Check free system RAM (in megabytes).
///
/// Uses a thread-local cached `sysinfo::System`.
pub fn free_ram_mb() -> u64 {
    SYS_CACHED.with(|sys| {
        sys.borrow_mut().refresh_memory();
        // sysinfo reports memory in **bytes**,
        // so convert bytes → MB. Getting this wrong silently disables every
        // RAM-based guard and the embedding auto-tuner.
        sys.borrow().available_memory() / (1024 * 1024)
    })
}

/// Total system RAM (in megabytes).
///
/// Uses a thread-local cached `sysinfo::System`. Total memory does not
/// change at runtime, so the first read is cached in the thread-local.
pub fn total_ram_mb() -> u64 {
    SYS_CACHED.with(|sys| {
        sys.borrow_mut().refresh_memory();
        // Bytes → MB (sysinfo ≥ 0.30 reports bytes; see `free_ram_mb`).
        sys.borrow().total_memory() / (1024 * 1024)
    })
}

/// Compute an adaptive free-RAM threshold based on total system memory.
///
/// * < 4 GB total  → threshold = 512 MB  (aggressive on low-end hardware)
/// * 4–8 GB total  → threshold = 1024 MB
/// * 8–16 GB total → threshold = 2048 MB (default for consumer laptops)
/// * > 16 GB total → threshold = 3072 MB
///
/// The threshold is intentionally high: we want to *prevent* the allocator
/// from committing new pages before the OS is forced to swap.  Once swap
/// starts, performance collapses and OOM is likely.
fn adaptive_threshold_mb() -> u64 {
    let total = total_ram_mb();
    match total {
        0..=4096 => 512,
        4097..=8192 => 1024,
        8193..=16384 => 2048,
        _ => 3072,
    }
}

/// A memory-pressure sentinel with preventive and critical modes.
///
/// # Modes
///
/// * **Preventive** (`is_under_pressure`): free RAM dropped below the
///   adaptive threshold.  Callers should back off (sleep, reduce batch
///   sizes, skip optional work).
/// * **Critical** (`is_critical`): free RAM dropped below half the
///   threshold.  Callers should stop allocating and return an error or
///   drop work entirely.
///
/// # Overhead
///
/// Checks are amortised via `check_interval` to avoid the cost of
/// `sysinfo::System::refresh_memory()` on every call.  Critical checks
/// are never amortised — they always read current memory.
#[derive(Debug)]
pub struct MemoryGuard {
    threshold_mb: u64,
    hard_stop_mb: u64,
    check_interval: usize,
    counter: AtomicUsize,
}

impl MemoryGuard {
    /// Create a new guard with an **adaptive** threshold derived from total
    /// system RAM.
    ///
    /// This is the recommended constructor for production code.
    #[must_use]
    pub fn new_adaptive() -> Self {
        let threshold = adaptive_threshold_mb();
        Self {
            threshold_mb: threshold,
            hard_stop_mb: threshold / 2,
            check_interval: 5,
            counter: AtomicUsize::new(0),
        }
    }

    /// Create a new guard that triggers throttling when free RAM drops below
    /// `threshold_mb`.
    ///
    /// Prefer [`Self::new_adaptive`] unless you need an explicit threshold.
    #[must_use]
    pub fn new(threshold_mb: u64) -> Self {
        Self {
            threshold_mb,
            hard_stop_mb: threshold_mb / 2,
            check_interval: 5,
            counter: AtomicUsize::new(0),
        }
    }

    /// Create a guard with a custom check interval.
    ///
    /// Larger intervals reduce `sysinfo` overhead but increase latency before
    /// throttling kicks in.
    #[must_use]
    pub fn with_interval(threshold_mb: u64, check_interval: usize) -> Self {
        Self {
            threshold_mb,
            hard_stop_mb: threshold_mb / 2,
            check_interval,
            counter: AtomicUsize::new(0),
        }
    }

    /// Returns `true` if the system is under memory pressure (free RAM is
    /// below the configured threshold).
    ///
    /// This is intentionally *advisory*: callers should back off (e.g. sleep,
    /// reduce concurrency, or skip optional work) but must not deadlock.
    pub fn is_under_pressure(&self) -> bool {
        // Amortise sysinfo cost: only check every N calls.
        if !self
            .counter
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(self.check_interval)
        {
            return false;
        }

        let free = free_ram_mb();
        let under = free < self.threshold_mb;
        if under {
            tracing::warn!(
                "MemoryGuard: free RAM {} MB < threshold {} MB — throttling",
                free,
                self.threshold_mb
            );
        }
        under
    }

    /// Returns `true` if the system is in **critical** memory pressure
    /// (free RAM is below half the threshold).
    ///
    /// This check is **never amortised** — it always reads current memory.
    /// Callers should treat this as a hard stop signal: drop work, skip
    /// optional allocations, or return an error.
    pub fn is_critical(&self) -> bool {
        let free = free_ram_mb();
        let critical = free < self.hard_stop_mb;
        if critical {
            tracing::error!(
                "MemoryGuard CRITICAL: free RAM {} MB < hard_stop {} MB — stopping work",
                free,
                self.hard_stop_mb
            );
        }
        critical
    }

    /// If pressure is detected, returns a `Duration` the caller should sleep.
    ///
    /// Callers in async contexts must use `tokio::time::sleep(delay).await`
    /// instead of `std::thread::sleep`; callers in sync contexts may use
    /// either. Returning `None` means no throttling is needed.
    ///
    /// Critical pressure returns 5× the normal delay.
    pub fn throttle_delay(&self, duration_ms: u64) -> Option<Duration> {
        if self.is_critical() {
            return Some(Duration::from_millis(duration_ms * 5));
        }
        if self.is_under_pressure() {
            return Some(Duration::from_millis(duration_ms));
        }
        None
    }

    /// Synchronous throttle helper for non-async callers only.
    ///
    /// **Do not call from async code** — use [`Self::throttle_delay`] and
    /// `tokio::time::sleep(...).await` instead to avoid blocking the reactor.
    pub fn throttle_if_needed_sync(&self, duration_ms: u64) {
        if let Some(delay) = self.throttle_delay(duration_ms) {
            std::thread::sleep(delay);
        }
    }

    /// Return the current threshold in megabytes.
    #[must_use]
    pub fn threshold_mb(&self) -> u64 {
        self.threshold_mb
    }

    /// Return the current hard-stop threshold in megabytes.
    #[must_use]
    pub fn hard_stop_mb(&self) -> u64 {
        self.hard_stop_mb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_guard_creation() {
        let guard = MemoryGuard::new(1024);
        assert_eq!(guard.threshold_mb, 1024);
        assert_eq!(guard.hard_stop_mb, 512);
        assert_eq!(guard.check_interval, 5);
    }

    #[test]
    fn test_memory_guard_with_interval() {
        let guard = MemoryGuard::with_interval(512, 3);
        assert_eq!(guard.threshold_mb, 512);
        assert_eq!(guard.hard_stop_mb, 256);
        assert_eq!(guard.check_interval, 3);
    }

    #[test]
    fn test_memory_guard_adaptive() {
        let guard = MemoryGuard::new_adaptive();
        // Adaptive threshold should be one of the known values
        assert!(
            [
                512,  // < 4 GB
                1024, // 4-8 GB
                2048, // 8-16 GB
                3072, // > 16 GB
            ]
            .contains(&guard.threshold_mb)
        );
        assert_eq!(guard.hard_stop_mb, guard.threshold_mb / 2);
    }

    #[test]
    fn test_memory_guard_does_not_panic() {
        // We can't assert on free RAM in a unit test, but we can assert the
        // guard doesn't panic and eventually returns a boolean.
        let guard = MemoryGuard::with_interval(1, 1);
        let _ = guard.is_under_pressure();
        let _ = guard.is_critical();
    }

    #[test]
    fn test_critical_lower_than_pressure() {
        let guard = MemoryGuard::new(1000);
        assert!(guard.hard_stop_mb < guard.threshold_mb);
    }
}
