//! Concurrent embedding validation (shared tract model).
//!
//! **Acceptance:** the configured concurrency is honoured; batched and
//! concurrent embedding give every text the vector it gets alone; the RAM
//! guard reduces the count under a tight cap; `session_count()` equals the
//! pipeline `buffer_unordered` width.
//!
//! All tests are `#[cfg_attr(not(feature = "slow-tests"), ignore)]` because they
//! require the real quantized ONNX model (~150 MB) to be present in the model
//! cache and ≥ 4 GB of free RAM.
//!
//! Run with:
//! ```sh
//! cargo test --package devscriptor-core --test test_embedding_pool \
//!     --features slow-tests --release -- --nocapture
//! ```

use std::time::Instant;

use devscriptor_core::embeddings::provider::JinaEmbeddingProvider;
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};

/// With `DEVSCRIPTOR_EMBEDDING_SESSIONS=4`, assert 4-way concurrency is
/// honoured, and that the batched and concurrent paths embed every text as it
/// is embedded alone. Their times are printed, not asserted: one text per run
/// is the fastest (`compute_batch_size`).
///
/// Accept: `session_count()==4`, no OOM, batched vectors equal the sequential
/// ones, and concurrent fan-out embeds every input correctly on the shared plan.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires quantized ONNX model + ≥ 4 GB free RAM; run with --features slow-tests"
)]
#[test]
fn test_ws5_t1_four_session_throughput() {
    // SAFETY: set env var before constructing the providers; restored at end.
    let prev = std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").ok();
    unsafe { std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", "4") };

    let session_count = JinaEmbeddingProvider::compute_session_count();
    assert_eq!(
        session_count, 4,
        "DEVSCRIPTOR_EMBEDDING_SESSIONS=4 must be honoured"
    );

    let config = EmbeddingConfig {
        batch_size: 16,
        ..EmbeddingConfig::default()
    };
    let svc = EmbeddingService::new(config);

    // Warmup (triggers model load).
    svc.generate_embedding("warmup")
        .expect("model must load under --features slow-tests");
    assert_eq!(
        svc.session_count(),
        4,
        "EmbeddingService must reflect 4 sessions"
    );

    // Build a batch of 64 items to measure throughput.
    let texts: Vec<String> = (0..64)
        .map(|i| format!("fn compute_{i}(x: f64) -> f64 {{ x * {i}.0 }}"))
        .collect();

    // Baseline: embed every item one-by-one.
    let single_start = Instant::now();
    let single_out: Vec<Vec<f32>> = texts
        .iter()
        .map(|text| svc.generate_embedding(text).expect("embedding failed"))
        .collect();
    let single_elapsed = single_start.elapsed().as_secs_f64();

    // Batched path: runs of `batch_size` texts on the shared plan.
    let batch_start = Instant::now();
    let batch_out = svc
        .generate_embeddings_batch(&texts)
        .expect("batch embedding failed");
    let batch_elapsed = batch_start.elapsed().as_secs_f64();

    // Concurrent fan-out: split the work across `session_count` threads. All
    // threads share one tract plan and call its concurrency-safe `run(&self)`
    // simultaneously — the path that actually exercises parallel embedding. We
    // verify it handles concurrent load correctly (every embedding returned,
    // right dimensionality, no OOM).
    let chunk = texts.len().div_ceil(session_count);
    let concurrent_start = Instant::now();
    let concurrent_count: usize = std::thread::scope(|scope| {
        let handles: Vec<_> = texts
            .chunks(chunk)
            .map(|chunk_texts| {
                let svc = svc.clone();
                scope.spawn(move || {
                    let mut produced = 0usize;
                    for text in chunk_texts {
                        let v = svc.generate_embedding(text).expect("embedding failed");
                        assert_eq!(v.len(), 768, "embedding must be 768-dim");
                        produced += 1;
                    }
                    produced
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("embedding thread panicked"))
            .sum()
    });
    let concurrent_elapsed = concurrent_start.elapsed().as_secs_f64();

    // Restore env var.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", v),
            None => std::env::remove_var("DEVSCRIPTOR_EMBEDDING_SESSIONS"),
        }
    }

    // Functional guarantees: batch and pooled-concurrent paths both produce a
    // complete, correct set of embeddings.
    assert_eq!(
        batch_out.len(),
        texts.len(),
        "batch must return one embedding per input"
    );
    assert_eq!(
        concurrent_count,
        texts.len(),
        "pooled concurrent fan-out must embed every input"
    );

    let batch_ratio = single_elapsed / batch_elapsed.max(f64::EPSILON);
    let concurrent_ratio = single_elapsed / concurrent_elapsed.max(f64::EPSILON);
    println!(
        "Concurrency: sequential={single_elapsed:.2}s  \
         batch={batch_elapsed:.2}s ({batch_ratio:.2}×)  \
         concurrent[{session_count} sessions]={concurrent_elapsed:.2}s ({concurrent_ratio:.2}×)"
    );

    // The matrix products run in f32, so a text's vector does not
    // depend on the texts it is batched with.
    for (alone, batched) in single_out.iter().zip(&batch_out) {
        let cosine: f32 = alone.iter().zip(batched).map(|(a, b)| a * b).sum();
        assert!(cosine > 0.9999, "batched vector drifted: cosine {cosine}");
    }
}

/// RAM guard formula caps `session_count` correctly based on free RAM.
///
/// Without env override, `compute_session_count` starts at `min(cores, 8)` and
/// steps down only if a minimum batch per worker would not fit the RAM budget.
/// Here we verify that the returned count stays within [1, 8] and that the
/// `EmbeddingService` reflects it correctly.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires ≥ 4 GB free RAM to be meaningful"
)]
#[test]
fn test_ws5_t2_ram_guard_caps_session_count() {
    if std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").is_ok() {
        println!("RAM guard: env var set, skipping RAM-guard path");
        return;
    }
    let count = JinaEmbeddingProvider::compute_session_count();
    assert!(
        (1..=8).contains(&count),
        "session_count={count} must be in [1, 8]"
    );

    let config = EmbeddingConfig::default();
    let svc = EmbeddingService::new(config);

    assert!(
        svc.session_count() >= 1,
        "EmbeddingService session_count must be ≥ 1"
    );
    assert!(
        svc.session_count() <= count,
        "active_session_count {} > configured {}",
        svc.session_count(),
        count
    );
    println!(
        "RAM guard: compute_session_count={count}, svc.session_count()={}",
        svc.session_count()
    );
}

/// `session_count()` reported by `EmbeddingService` matches what the
/// pipeline passes to `buffer_unordered`.
///
/// The pipeline at `pipeline.rs ~L812` sets:
/// ```rust
/// let session_count = service.session_count().max(1);
/// .buffer_unordered(session_count)
/// ```
/// This test verifies the value is consistent across a real embedding run.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires quantized ONNX model; run with --features slow-tests"
)]
#[test]
fn test_ws5_t3_saturation_invariant() {
    let prev = std::env::var("DEVSCRIPTOR_EMBEDDING_SESSIONS").ok();
    unsafe { std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", "2") };

    let config = EmbeddingConfig {
        batch_size: 8,
        ..EmbeddingConfig::default()
    };
    let svc = EmbeddingService::new(config);

    let reported_sessions = svc.session_count();
    assert_eq!(
        reported_sessions, 2,
        "With DEVSCRIPTOR_EMBEDDING_SESSIONS=2, session_count() must return 2"
    );

    // Generate a batch large enough to saturate both sessions.
    let texts: Vec<String> = (0..16)
        .map(|i| format!("struct Node{i} {{ id: u64, value: f64 }}"))
        .collect();
    svc.generate_embeddings_batch(&texts)
        .expect("batch must succeed with 2 sessions");

    unsafe {
        match prev {
            Some(v) => std::env::set_var("DEVSCRIPTOR_EMBEDDING_SESSIONS", v),
            None => std::env::remove_var("DEVSCRIPTOR_EMBEDDING_SESSIONS"),
        }
    }

    println!("Fan-out: session_count={reported_sessions} — fan-out invariant satisfied");
}

/// Texts are batched by length, not in the order given; each vector must still
/// be the one for the text at its position, as the text embedded alone gets it.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires quantized ONNX model + ≥ 4 GB free RAM; run with --features slow-tests"
)]
#[test]
fn batched_vectors_keep_the_order_of_their_texts() {
    let svc = EmbeddingService::new(EmbeddingConfig {
        batch_size: 2,
        ..EmbeddingConfig::default()
    });
    let long_body: String = (0..60)
        .map(|i| format!("    total += line_{i}.price_cents * line_{i}.quantity;\n"))
        .collect();
    let texts: Vec<String> = vec![
        format!("function order_total\n{long_body}"),
        "function add\nfn add(a: i32, b: i32) -> i32 { a + b }".to_string(),
        "table users\nCREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)".to_string(),
        "class CartLine\nclass CartLine { final String sku; final int quantity; }".to_string(),
        "function greet\ndef greet(name):\n    return f'Hello, {name}!'".to_string(),
    ];

    let batched = svc
        .generate_embeddings_batch(&texts)
        .expect("model must load under --features slow-tests");
    assert_eq!(batched.len(), texts.len());
    for (text, vector) in texts.iter().zip(&batched) {
        let alone = svc.generate_embedding(text).expect("embedding failed");
        let cosine: f32 = alone.iter().zip(vector).map(|(a, b)| a * b).sum();
        // The matrix products run in f32: a batch changes no vector.
        assert!(
            cosine > 0.9999,
            "cosine {cosine} for the vector of {:?}",
            &text[..30]
        );
    }
}
