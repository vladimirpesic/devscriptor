//! Memory probe: empirically measure peak RSS of the real Jina embedding
//! service as a function of (batch_size, input_length).
//!
//! Run under a memory cap, e.g.:
//!   systemd-run --user --scope -p MemoryMax=6G -p MemorySwapMax=0 \
//!     <bin> --include-ignored --nocapture --test-threads=1
//!
//! This isolates embedding memory from parsing so we can choose safe
//! MAX_EMBED_TEXT_CHARS / batch_size values from data, not guesses.

#[cfg(feature = "slow-tests")]
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};

/// Peak resident set size (high-water mark) of this process, in MB.
#[cfg(feature = "slow-tests")]
fn vmhwm_mb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            // format: "VmHWM:\t   123456 kB"
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb / 1024;
        }
    }
    0
}

/// Build `count` synthetic "code" texts, each `chars` characters long.
#[cfg(feature = "slow-tests")]
fn synth_batch(count: usize, chars: usize) -> Vec<String> {
    // Vaguely code-like content so the tokenizer behaves realistically.
    let unit = "fn process_item(item: &Item) -> Result<Output> { let x = compute(item.value); ";
    let mut base = String::with_capacity(chars + unit.len());
    while base.len() < chars {
        base.push_str(unit);
    }
    base.truncate(chars);
    (0..count)
        .map(|i| format!("// entity {i}\n{base}"))
        .collect()
}

#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires slow-tests + local model"
)]
#[tokio::test]
async fn mem_probe_embedding() {
    #[cfg(feature = "slow-tests")]
    {
        println!("VmHWM at start: {} MB", vmhwm_mb());

        let svc = EmbeddingService::new(EmbeddingConfig {
            show_download_progress: false,
            batch_size: 16,
        });

        // Force model load with a tiny input.
        let _ = svc.generate_embedding("warmup").expect("warmup");
        println!("VmHWM after model load: {} MB", vmhwm_mb());

        // (count, chars) escalation. Peak RSS printed after each.
        let cases = [
            (8usize, 1_000usize),
            (16, 1_000),
            (16, 2_000),
            (16, 4_000),
            (16, 8_000),
            (64, 2_000),
            (64, 8_000),
        ];

        for (count, chars) in cases {
            let batch = synth_batch(count, chars);
            match svc.generate_embeddings_batch(&batch) {
                Ok(v) => println!(
                    "  embed count={count:<3} chars={chars:<6} -> {} vecs, VmHWM={} MB",
                    v.len(),
                    vmhwm_mb()
                ),
                Err(e) => println!("  embed count={count:<3} chars={chars:<6} -> ERROR: {e}"),
            }
        }

        println!("FINAL VmHWM: {} MB", vmhwm_mb());
    }
}
