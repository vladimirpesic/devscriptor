//! Pipeline throughput benchmark on a real directory.
//!
//! Runs the EXACT production `CodebasePipeline` on a directory so the
//! parse/insert/embed breakdown (logged per-batch by the pipeline) is visible.
//! Embedding is toggled with `EMBED=0|1` so parse+insert cost can be isolated
//! from ONNX inference cost.
//!
//! Usage:
//!   EMBED=0 cargo run --release --example bench_pipeline -- <dir>
//!   EMBED=1 cargo run --release --example bench_pipeline -- <dir>

use devscriptor_core::database::{Database, DatabaseConfig, adaptive_cache_size};
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench_pipeline <dir>");
    let embed = std::env::var("EMBED").map(|v| v != "0").unwrap_or(true);
    let db_path = format!(
        "/tmp/bench_pipeline_{}.db",
        if embed { "embed" } else { "noembed" }
    );
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{db_path}-wal"));
    let _ = std::fs::remove_file(format!("{db_path}-shm"));

    let db_config = DatabaseConfig {
        path: db_path.clone(),
        cache_size: adaptive_cache_size(),
        enable_vector: true,
        ..DatabaseConfig::default()
    };
    let db = Arc::new(Database::new(db_config).expect("db"));

    let service = if embed {
        // Selection level comes from DEVSCRIPTOR_EMBEDDING_SELECTION via the
        // PipelineConfig default, so the bench can compare selection levels.
        Some(Arc::new(EmbeddingService::new(EmbeddingConfig {
            show_download_progress: false,
            batch_size: 16,
        })))
    } else {
        None
    };

    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db_path.clone(),
            cache_size: adaptive_cache_size(),
            enable_vector: true,
            ..DatabaseConfig::default()
        },
        parser: ParserConfig::default(),
        enable_cross_file: false, // isolate Phase 2 throughput
        file_batch_size: 50,
        embedding_batch_size: 16,
        use_gitignore: true,
        include_extensions: vec![
            "java".into(),
            "c".into(),
            "cpp".into(),
            "h".into(),
            "hpp".into(),
        ],
        exclude_patterns: vec![
            ".git/".into(),
            "build/".into(),
            "*.class".into(),
            "*.jar".into(),
        ],
        max_concurrency: 2,
        incremental: false,
        ..PipelineConfig::default()
    };

    let pipeline = CodebasePipeline::with_database_and_service(config, Arc::clone(&db), service)
        .expect("pipeline");

    println!("=== bench: dir={dir} embed={embed} ===");
    let t = std::time::Instant::now();
    let result = pipeline
        .analyze_directory(std::path::Path::new(&dir))
        .await
        .expect("analyze");
    let secs = t.elapsed().as_secs_f64();
    println!(
        "DONE embed={embed}: {} files, {} entities, {} embeddings in {:.1}s = {:.1} files/s",
        result.files_parsed,
        result.entity_count,
        result.embedding_count,
        secs,
        result.files_parsed as f64 / secs,
    );
}
