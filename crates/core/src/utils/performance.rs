//! Performance Profiling and Optimization Utilities for Devscriptor-Core
//!
//! This module provides utilities for timing code execution, monitoring memory usage,
//! and profiling function performance.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Timer for measuring code execution time
#[derive(Debug)]
pub struct PerformanceTimer {
    name: String,
    start_time: Instant,
    checkpoints: Vec<(String, Duration)>,
}

impl PerformanceTimer {
    /// Create a new performance timer
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start_time: Instant::now(),
            checkpoints: Vec::new(),
        }
    }

    /// Record a checkpoint
    pub fn checkpoint(&mut self, name: impl Into<String>) -> Duration {
        let elapsed = self.start_time.elapsed();
        self.checkpoints.push((name.into(), elapsed));
        elapsed
    }

    /// Get the total elapsed time
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get all checkpoints
    pub fn checkpoints(&self) -> &[(String, Duration)] {
        &self.checkpoints
    }

    /// Reset the timer
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.checkpoints.clear();
    }

    /// Get a report of all timing information
    pub fn report(&self) -> TimerReport {
        TimerReport {
            name: self.name.clone(),
            total_time: self.elapsed(),
            checkpoints: self.checkpoints.clone(),
        }
    }
}

impl Default for PerformanceTimer {
    fn default() -> Self {
        Self::new("Operation")
    }
}

/// Report from a performance timer
#[derive(Debug, Clone)]
pub struct TimerReport {
    /// Timer name
    pub name: String,
    /// Total elapsed time
    pub total_time: Duration,
    /// Checkpoints
    pub checkpoints: Vec<(String, Duration)>,
}

impl fmt::Display for TimerReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Performance Report: {}", self.name)?;
        writeln!(f, "  Total Time: {:.3}s", self.total_time.as_secs_f64())?;

        if !self.checkpoints.is_empty() {
            writeln!(f, "  Checkpoints:")?;
            let mut prev = Duration::ZERO;
            for (name, time) in &self.checkpoints {
                let delta = *time - prev;
                writeln!(
                    f,
                    "    {}: {:.3}s (+{:.3}s)",
                    name,
                    time.as_secs_f64(),
                    delta.as_secs_f64()
                )?;
                prev = *time;
            }
        }

        Ok(())
    }
}

/// Memory usage statistics
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    /// Initial memory in bytes
    pub initial_bytes: usize,
    /// Current memory in bytes
    pub current_bytes: usize,
    /// Peak memory in bytes
    pub peak_bytes: usize,
}

impl MemoryStats {
    /// Get memory difference from initial
    pub fn delta(&self) -> isize {
        self.current_bytes as isize - self.initial_bytes as isize
    }

    /// Get memory difference as a formatted string
    pub fn delta_formatted(&self) -> String {
        let delta = self.delta();
        if delta >= 0 {
            format!("+{}", format_bytes(delta as usize))
        } else {
            format!("-{}", format_bytes(delta.unsigned_abs()))
        }
    }
}

/// Format bytes into human-readable format
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

/// Memory monitor for tracking memory usage
#[derive(Debug)]
pub struct MemoryMonitor {
    initial_memory: usize,
    peak_memory: Arc<Mutex<usize>>,
    checkpoints: Arc<Mutex<Vec<(String, usize)>>>,
}

impl MemoryMonitor {
    /// Create a new memory monitor
    pub fn new() -> Self {
        let current = Self::current();
        Self {
            initial_memory: current,
            peak_memory: Arc::new(Mutex::new(current)),
            checkpoints: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start monitoring memory
    pub fn start(&self) {
        let current = Self::current();
        *self.peak_memory.lock() = current;
        self.checkpoints.lock().push(("Start".to_string(), current));
    }

    /// Record a memory checkpoint
    pub fn checkpoint(&self, name: impl Into<String>) -> usize {
        let current = Self::current();

        // Update peak
        {
            let mut peak = self.peak_memory.lock();
            if current > *peak {
                *peak = current;
            }
        }

        // Record checkpoint
        self.checkpoints.lock().push((name.into(), current));

        current
    }

    /// Get current memory usage
    pub fn current() -> usize {
        // Try to get resident set size from /proc/self/statm on Linux
        #[cfg(target_os = "linux")]
        {
            if let Ok(contents) = std::fs::read_to_string("/proc/self/statm")
                && let Some(first) = contents.split_whitespace().next()
                && let Ok(pages) = first.parse::<usize>()
            {
                // Use the runtime page size for correctness on non-standard kernels
                let raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                if raw < 0 {
                    return 0;
                }
                let page_size = raw as usize;
                return pages * page_size.max(1);
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            // On macOS, try `task_info` via libc if available
            #[cfg(target_os = "macos")]
            {
                use mach2::kern_return::KERN_SUCCESS;
                use mach2::task::task_info;
                use mach2::task_info::{
                    MACH_TASK_BASIC_INFO, MACH_TASK_BASIC_INFO_COUNT, mach_task_basic_info,
                    task_info_t,
                };
                use mach2::traps::mach_task_self;

                let mut info = std::mem::MaybeUninit::<mach_task_basic_info>::uninit();
                let mut count = MACH_TASK_BASIC_INFO_COUNT;
                let ret = unsafe {
                    task_info(
                        mach_task_self(),
                        MACH_TASK_BASIC_INFO,
                        info.as_mut_ptr() as task_info_t,
                        &mut count,
                    )
                };
                if ret == KERN_SUCCESS {
                    let info = unsafe { info.assume_init() };
                    return info.resident_size as usize;
                }
            }
        }

        // Fallback: return 0 on unsupported platforms
        0
    }

    /// Get current memory usage (instance method)
    pub fn current_memory(&self) -> usize {
        Self::current()
    }

    /// Get peak memory usage
    pub fn peak_memory(&self) -> usize {
        *self.peak_memory.lock()
    }

    /// Get initial memory usage
    pub fn initial_memory(&self) -> usize {
        self.initial_memory
    }

    /// Get all checkpoints
    pub fn checkpoints(&self) -> Vec<(String, usize)> {
        self.checkpoints.lock().clone()
    }

    /// Get memory statistics
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            initial_bytes: self.initial_memory,
            current_bytes: Self::current(),
            peak_bytes: self.peak_memory(),
        }
    }

    /// Get a memory usage report
    pub fn report(&self) -> MemoryReport {
        let stats = self.stats();
        MemoryReport {
            initial_bytes: stats.initial_bytes,
            current_bytes: stats.current_bytes,
            peak_bytes: stats.peak_bytes,
            delta_bytes: stats.delta(),
            checkpoints: self.checkpoints(),
        }
    }

    /// Reset the monitor
    pub fn reset(&mut self) {
        self.initial_memory = Self::current();
        *self.peak_memory.lock() = self.initial_memory;
        self.checkpoints.lock().clear();
    }
}

impl Default for MemoryMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Memory usage report
#[derive(Debug, Clone)]
pub struct MemoryReport {
    /// Initial memory in bytes
    pub initial_bytes: usize,
    /// Current memory in bytes
    pub current_bytes: usize,
    /// Peak memory in bytes
    pub peak_bytes: usize,
    /// Memory difference from initial
    pub delta_bytes: isize,
    /// Memory checkpoints
    pub checkpoints: Vec<(String, usize)>,
}

impl fmt::Display for MemoryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Memory Usage Report:")?;
        writeln!(f, "  Initial: {}", format_bytes(self.initial_bytes))?;
        writeln!(f, "  Current: {}", format_bytes(self.current_bytes))?;
        writeln!(f, "  Peak: {}", format_bytes(self.peak_bytes))?;
        writeln!(
            f,
            "  Delta: {}{}",
            if self.delta_bytes >= 0 { "+" } else { "" },
            format_bytes(self.delta_bytes.unsigned_abs())
        )?;

        if !self.checkpoints.is_empty() {
            writeln!(f, "  Checkpoints:")?;
            for (name, bytes) in &self.checkpoints {
                writeln!(f, "    {}: {}", name, format_bytes(*bytes))?;
            }
        }

        Ok(())
    }
}

/// Function timing information
#[derive(Debug, Clone)]
pub struct FunctionTiming {
    /// Function name
    pub name: String,
    /// Total time spent in function
    pub total_time: Duration,
    /// Number of calls
    pub call_count: u64,
    /// Average time per call
    pub average_time: Duration,
    /// Minimum time
    pub min_time: Duration,
    /// Maximum time
    pub max_time: Duration,
}

/// Performance profiler for tracking function performance
#[derive(Debug, Clone, Default)]
pub struct PerformanceProfiler {
    timings: Arc<Mutex<HashMap<String, FunctionTiming>>>,
}

impl PerformanceProfiler {
    /// Create a new performance profiler
    pub fn new() -> Self {
        Self {
            timings: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record a function timing
    pub fn record(&self, name: impl Into<String>, duration: Duration) {
        let name = name.into();
        let mut timings = self.timings.lock();

        let timing = timings.entry(name.clone()).or_insert(FunctionTiming {
            name: name.clone(),
            total_time: Duration::ZERO,
            call_count: 0,
            average_time: Duration::ZERO,
            min_time: duration,
            max_time: duration,
        });

        timing.total_time += duration;
        timing.call_count += 1;
        timing.average_time = timing.total_time / timing.call_count as u32;
        timing.min_time = timing.min_time.min(duration);
        timing.max_time = timing.max_time.max(duration);
    }

    /// Get timing for a specific function
    pub fn get_timing(&self, name: &str) -> Option<FunctionTiming> {
        self.timings.lock().get(name).cloned()
    }

    /// Get all timings
    pub fn get_all_timings(&self) -> Vec<FunctionTiming> {
        self.timings.lock().values().cloned().collect()
    }

    /// Get a report sorted by total time
    pub fn report(&self) -> ProfilerReport {
        let mut timings = self.get_all_timings();
        timings.sort_by_key(|b| std::cmp::Reverse(b.total_time));

        ProfilerReport { timings }
    }

    /// Clear all timings
    pub fn clear(&self) {
        self.timings.lock().clear();
    }
}

/// Profiler report
#[derive(Debug, Clone)]
pub struct ProfilerReport {
    /// Function timings
    pub timings: Vec<FunctionTiming>,
}

impl fmt::Display for ProfilerReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Performance Profile Report:")?;
        writeln!(
            f,
            "  {:<30} {:>10} {:>12} {:>12} {:>12}",
            "Function", "Calls", "Total", "Avg", "Max"
        )?;
        writeln!(f, "  {}", "-".repeat(80))?;

        for timing in &self.timings {
            writeln!(
                f,
                "  {:<30} {:>10} {:>11.3}s {:>11.3}s {:>11.3}s",
                timing.name,
                timing.call_count,
                timing.total_time.as_secs_f64(),
                timing.average_time.as_secs_f64(),
                timing.max_time.as_secs_f64(),
            )?;
        }

        Ok(())
    }
}

/// A scoped timer that records timing when dropped
pub struct ScopedTimer<'a> {
    name: String,
    start: Instant,
    profiler: Option<&'a PerformanceProfiler>,
}

impl<'a> ScopedTimer<'a> {
    /// Create a new scoped timer
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start: Instant::now(),
            profiler: None,
        }
    }

    /// Create a new scoped timer with profiler
    pub fn with_profiler(name: impl Into<String>, profiler: &'a PerformanceProfiler) -> Self {
        Self {
            name: name.into(),
            start: Instant::now(),
            profiler: Some(profiler),
        }
    }
}

impl Drop for ScopedTimer<'_> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        if let Some(profiler) = self.profiler {
            profiler.record(&self.name, duration);
        }
    }
}

/// A combined timer and memory monitor
#[derive(Debug)]
pub struct PerformanceMonitor {
    timer: PerformanceTimer,
    memory: MemoryMonitor,
}

impl PerformanceMonitor {
    /// Create a new performance monitor
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            timer: PerformanceTimer::new(name),
            memory: MemoryMonitor::new(),
        }
    }

    /// Record a checkpoint
    pub fn checkpoint(&mut self, name: impl Into<String>) {
        let name_str: String = name.into();
        self.timer.checkpoint(&name_str);
        self.memory.checkpoint(&name_str);
    }

    /// Get a combined report
    pub fn report(&self) -> CombinedReport {
        CombinedReport {
            timer_report: self.timer.report(),
            memory_report: self.memory.report(),
        }
    }
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self::new("Operation")
    }
}

/// Combined timer and memory report
#[derive(Debug, Clone)]
pub struct CombinedReport {
    /// Timer report
    pub timer_report: TimerReport,
    /// Memory report
    pub memory_report: MemoryReport,
}

impl fmt::Display for CombinedReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.timer_report)?;
        writeln!(f)?;
        write!(f, "{}", self.memory_report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_performance_timer() {
        let mut timer = PerformanceTimer::new("Test");

        thread::sleep(Duration::from_millis(10));
        timer.checkpoint("point1");

        thread::sleep(Duration::from_millis(10));
        timer.checkpoint("point2");

        let report = timer.report();
        assert_eq!(report.checkpoints.len(), 2);
        assert!(report.total_time >= Duration::from_millis(20));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0.00 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_scoped_timer() {
        let profiler = PerformanceProfiler::new();

        {
            let _timer = ScopedTimer::with_profiler("test_func", &profiler);
            thread::sleep(Duration::from_millis(10));
        }

        let timing = profiler.get_timing("test_func").unwrap();
        assert_eq!(timing.call_count, 1);
        assert!(timing.total_time >= Duration::from_millis(10));
    }

    #[test]
    fn test_performance_profiler() {
        let profiler = PerformanceProfiler::new();

        profiler.record("func1", Duration::from_millis(100));
        profiler.record("func1", Duration::from_millis(200));
        profiler.record("func2", Duration::from_millis(50));

        let report = profiler.report();
        assert_eq!(report.timings.len(), 2);

        let func1 = profiler.get_timing("func1").unwrap();
        assert_eq!(func1.call_count, 2);
        assert_eq!(func1.total_time, Duration::from_millis(300));
    }
}
