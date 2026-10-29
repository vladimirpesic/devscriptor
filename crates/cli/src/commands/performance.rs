//! Performance command implementation

use anyhow::Result;
use devscriptor_core::utils::performance::{
    MemoryMonitor, PerformanceMonitor, PerformanceProfiler,
};

/// Show performance statistics
pub fn stats() -> Result<()> {
    println!("\n{}", "═".repeat(60));
    println!(" Performance Statistics");
    println!("{}", "═".repeat(60));

    let mut monitor = PerformanceMonitor::new("CLI Self-Check");
    let profiler = PerformanceProfiler::new();

    println!("\nSystem Resources:");
    println!("  CPU cores: {}", num_cpus::get());
    let current_mem = MemoryMonitor::current();
    if current_mem > 0 {
        println!(
            "  Current memory usage: {}",
            devscriptor_core::utils::performance::format_bytes(current_mem)
        );
    } else {
        println!("  Current memory usage: (unavailable on this platform)");
    }

    monitor.checkpoint("system-info");

    // Tiny synthetic benchmark to populate the profiler
    {
        let _t = devscriptor_core::utils::performance::ScopedTimer::with_profiler(
            "cpu_warmup",
            &profiler,
        );
        // Pure CPU work: iterative hash-like mixing
        let mut acc: u64 = 0x1234_5678_9ABC_DEF0;
        for i in 0..10_000_000u64 {
            acc = acc.wrapping_mul(31).wrapping_add(i);
            acc = acc.rotate_left(7);
        }
        std::hint::black_box(acc);
    }

    monitor.checkpoint("CPU warm-up benchmark");

    println!("\nProfiler Report:");
    println!("{}", profiler.report());

    println!("\n{}", monitor.report());

    Ok(())
}
