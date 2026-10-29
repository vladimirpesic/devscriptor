//! Common test utilities for Devscriptor integration tests
#![allow(dead_code)] // Items are used by multiple independent test modules.
//!
//! This module provides shared infrastructure for heavy integration tests
//! that need to parse files, populate databases, and generate embeddings.
//!
//! ## Embedding strategy
//!
//! By default all tests use `MockEmbeddingProvider` — a deterministic, instant
//! provider that produces unit-normalised 768-dim vectors from text hashes.
//! This means:
//!
//! * Tests that assert entity counts, relationship counts, FTS5 search, DB
//!   integrity, and vector KNN queries all work without loading the
//!   ONNX model; the suite runs in **< 30 seconds**.
//!
//! * Tests that genuinely need semantic similarity quality (i.e., the result
//!   ordering reflects code meaning) must opt into the real model by calling
//!   [`real_embedding_service`] and gating with `#[cfg(feature = "slow-tests")]`.
//!
//! All analysis routes through [`CodebasePipeline`], so tests exercise the
//! exact streaming, bounded-memory code path production runs.

pub mod fixture_client;
pub mod metrics;

use devscriptor_core::database::{DatabaseConfig, adaptive_cache_size};
#[cfg(feature = "slow-tests")]
use devscriptor_core::embeddings::EmbeddingConfig;
use devscriptor_core::embeddings::{EmbeddingService, MockEmbeddingProvider};
use devscriptor_core::error::Result;
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig, PipelineResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Analysis results container shared across all tests
#[derive(Clone)]
pub struct AnalysisResult {
    pub status: String,
    pub files_processed: usize,
    pub total_entities: usize,
    pub total_relationships: usize,
    pub embeddings_generated: usize,
    pub analysis_time_secs: f64,
    pub db_path: PathBuf,
    pub stats: DatabaseStats,
    pub errors: Vec<String>,
    /// Re-usable embedding service so tests do not reload the model (memory fix).
    pub embedding_service: Option<EmbeddingService>,
}

/// Database statistics for reporting
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct DatabaseStats {
    pub total_entities: usize,
    pub total_functions: usize,
    pub total_classes: usize,
    pub total_structs: usize,
    pub total_enums: usize,
    pub total_interfaces: usize,
    pub total_traits: usize,
    pub total_files: usize,
    pub total_relationships: usize,
    pub entities_by_language: HashMap<String, usize>,
}

/// Shared state for once-per-class analysis (multilanguage tests)
static MULTILANG_STATE: tokio::sync::OnceCell<Arc<Mutex<Option<AnalysisResult>>>> =
    tokio::sync::OnceCell::const_new();

/// Shared state for once-per-class analysis (semantic search tests)
static SEMANTIC_STATE: tokio::sync::OnceCell<Arc<Mutex<Option<AnalysisResult>>>> =
    tokio::sync::OnceCell::const_new();

/// Mapping of file extensions to expected languages.
/// Kept for tests that assert per-language coverage.
pub fn language_mappings() -> Vec<(&'static str, &'static str)> {
    vec![
        ("sh", "Bash"),
        ("c", "C"),
        ("cpp", "C++"),
        ("cs", "C#"),
        ("cu", "CUDA"),
        ("dart", "Dart"),
        ("fs", "F#"),
        ("go", "Go"),
        ("gql", "GraphQL"),
        ("html", "HTML"),
        ("java", "Java"),
        ("js", "JavaScript"),
        ("kt", "Kotlin"),
        ("m", "Objective-C"),
        ("php", "PHP"),
        ("py", "Python"),
        ("rb", "Ruby"),
        ("rs", "Rust"),
        ("scala", "Scala"),
        ("sol", "Solidity"),
        ("sql", "SQL"),
        ("svelte", "Svelte"),
        ("swift", "Swift"),
        ("vue", "Vue"),
        ("ts", "TypeScript"),
        ("tsx", "TSX"),
        ("vb", "VB.NET"),
    ]
}

/// Get test files directory
pub fn test_files_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_files")
}

/// Get project root for database output
pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Standard database path used by both multilanguage and semantic search tests
pub fn default_db_path() -> PathBuf {
    project_root().join("test_multilanguage_comprehensive.db")
}

/// Get the test model cache directory (used by `real_embedding_service`).
pub fn model_cache_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bundle_model = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("bundle").join("embedding-model"))
        .filter(|p| p.exists());

    if let Some(bundle_path) = bundle_model {
        return bundle_path;
    }

    manifest_dir.join(".model_cache")
}

/// Check whether the production Jina embedding model is cached locally.
pub fn is_model_downloaded() -> bool {
    let cache_dir = model_cache_dir();
    if !cache_dir.exists() {
        return false;
    }

    let model_name = "jina-embeddings-v2-base-code";
    let model_dir = cache_dir.join(model_name);

    if model_dir.exists() && model_dir.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return true;
    }

    cache_dir.read_dir().is_ok_and(|entries| {
        entries.filter_map(std::result::Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            name.contains("jina") && name.contains("embeddings")
        })
    })
}

/// No-op: with the mock provider the model never needs to be downloaded.
///
/// Kept for callers in `slow-tests`-gated code that still call this for the
/// real model path.  Under the mock-by-default regime it returns immediately.
pub async fn ensure_model_downloaded() {
    // Fast path: mock provider requires no model file.
    // The real-model path (slow-tests) downloads via EmbeddingService::new().
    tracing::debug!("ensure_model_downloaded: mock provider active, no download needed");
    let _ = (); // intentional no-op

    // With `slow-tests`, the real model is downloaded once, before the tests
    // that need it.
    #[cfg(feature = "slow-tests")]
    {
        if is_model_downloaded() {
            tracing::info!(
                "   ✅ Embedding model already cached at: {}",
                model_cache_dir().display()
            );
            return;
        }

        tracing::info!("   📦 Downloading embedding model for slow-tests...");
        tracing::info!("      Model: jinaai/jina-embeddings-v2-base-code");
        tracing::info!("      Cache: {}", model_cache_dir().display());

        let embedding_config = EmbeddingConfig {
            show_download_progress: true,
            batch_size: 16,
        };
        let pipeline = CodebasePipeline::new(PipelineConfig {
            embedding: Some(embedding_config),
            ..PipelineConfig::default()
        })
        .expect("pipeline should construct for model download");

        match pipeline
            .embedding_service()
            .expect("embedding service")
            .generate_embedding("test")
        {
            Ok(_) => {
                tracing::info!("   ✅ Model downloaded and loaded successfully");
            }
            Err(e) => {
                tracing::info!("   ⚠️  Model download may have failed: {e}");
            }
        }
    } // end #[cfg(feature = "slow-tests")] block
}

/// Build the standard [`PipelineConfig`] used by integration tests.
///
/// Embeddings are generated using [`MockEmbeddingProvider`] by default.
/// This means the pipeline never loads the ONNX model, keeping
/// the full test suite under 30 seconds on any hardware.
fn test_pipeline_config(db_path: &Path) -> PipelineConfig {
    // `embedding: None` combined with an explicit EmbeddingService via
    // `with_database_and_service` is the clean way to inject the mock.
    // We leave the field None here; `perform_analysis_at` injects the mock service.
    PipelineConfig {
        database: DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            cache_size: adaptive_cache_size(),
            ..DatabaseConfig::default()
        },
        // No EmbeddingConfig in pipeline config — we inject a mock service directly
        // so the pipeline never tries to construct a JinaEmbeddingProvider.
        embedding: None,
        parser: ParserConfig {
            enable_cache: true,
            cache_size: 10,
            mmap_threshold: 10 * 1024 * 1024,
            enable_cleaning: true,
            enable_validation: true,
            skip_invalid_entities: true,
            emit_decorators: false,
        },
        enable_cross_file: true,
        file_batch_size: 50,
        embedding_batch_size: 16,
        use_gitignore: false,
        max_concurrency: 2,
        ..Default::default()
    }
}

/// Create a mock embedding service backed by [`MockEmbeddingProvider`].
///
/// Use this in all tests that need *some* embedding vectors but do not
/// require real semantic quality (counts, KNN structure, DB integrity, etc.).
pub fn mock_embedding_service() -> EmbeddingService {
    EmbeddingService::with_provider(MockEmbeddingProvider::new())
}

/// Create the real ONNX-backed embedding service.
///
/// Only call this from tests gated with `#[cfg(feature = "slow-tests")]`
/// that genuinely need jina-embeddings-v2-base-code semantic quality.
/// Loading the quantized model is fast and requires ~760 MB of RAM.
#[cfg(feature = "slow-tests")]
pub fn real_embedding_service() -> EmbeddingService {
    EmbeddingService::new(EmbeddingConfig {
        show_download_progress: true,
        batch_size: 16,
    })
}

/// Convert a [`PipelineResult`] into the [`AnalysisResult`] the tests read.
fn pipeline_result_to_analysis(
    result: PipelineResult,
    db_path: PathBuf,
    embedding_service: Option<EmbeddingService>,
) -> AnalysisResult {
    let mut stats = DatabaseStats {
        total_entities: result.entity_count,
        total_relationships: result.relationship_count,
        total_files: result.files_parsed,
        entities_by_language: result.entities_by_language.clone(),
        ..Default::default()
    };

    for (type_name, count) in &result.entity_type_counts {
        match type_name.as_str() {
            "function" => stats.total_functions += *count,
            "class" => stats.total_classes += *count,
            "struct" => stats.total_structs += *count,
            "enum" => stats.total_enums += *count,
            "interface" => stats.total_interfaces += *count,
            "trait" => stats.total_traits += *count,
            _ => {}
        }
    }

    AnalysisResult {
        status: "analyzed".to_string(),
        files_processed: result.files_parsed,
        total_entities: result.entity_count,
        total_relationships: result.relationship_count,
        embeddings_generated: result.embedding_count,
        analysis_time_secs: result.processing_time_secs,
        db_path,
        stats,
        errors: result.errors,
        embedding_service,
    }
}

/// Perform the complete multilanguage analysis once and cache results.
///
/// Under the `slow-tests` feature flag the production
/// `jinaai/jina-embeddings-v2-base-code` ONNX model is loaded so that
/// semantic search quality can be validated with real cosine-similarity
/// scores.  Without the flag a [`MockEmbeddingProvider`] is used instead,
/// completing in seconds without requiring the ONNX model.
pub async fn perform_analysis_at(db_path: Option<PathBuf>) -> Result<AnalysisResult> {
    let start_time = Instant::now();
    let db_path = db_path.unwrap_or_else(default_db_path);

    // Remove any existing database to avoid entity duplication across test runs
    let _ = std::fs::remove_file(&db_path);

    let config = test_pipeline_config(&db_path);
    let db = Arc::new(devscriptor_core::database::Database::new(
        config.database.clone(),
    )?);

    // ── Embedding service selection ──────────────────────────────────────────
    // slow-tests: load the real Jina model (downloads ONNX on first run).
    // default:    use the mock provider (instant, no model file required).
    #[cfg(feature = "slow-tests")]
    let (svc_arc, label) = {
        tracing::info!("\n🚀 Starting comprehensive analysis (CodebasePipeline + Jina ONNX)…");
        tracing::info!("   Database: {}", db_path.display());
        tracing::info!("   Embedding model: jinaai/jina-embeddings-v2-base-code (768-dim)");
        let svc = real_embedding_service();
        // Eagerly trigger model load so we get a clear timing boundary and a
        // helpful error message if the model files are missing.
        if let Err(e) = svc.generate_embedding("warmup") {
            tracing::warn!("Jina model load failed ({e}); falling back to MockEmbeddingProvider");
            let mock = Arc::new(mock_embedding_service());
            let label = "mock-fallback";
            (mock, label)
        } else {
            tracing::info!("   ✅ Jina model loaded successfully");
            let arc = Arc::new(svc);
            let label = "jina-onnx";
            (arc, label)
        }
    };

    #[cfg(not(feature = "slow-tests"))]
    let (svc_arc, label) = {
        tracing::info!("\n🚀 Starting comprehensive analysis (CodebasePipeline + MockEmbedding)…");
        tracing::info!("   Database: {}", db_path.display());
        (Arc::new(mock_embedding_service()), "mock")
    };

    tracing::info!("   Embedding provider: {label}");

    let pipeline = CodebasePipeline::with_database_and_service(
        config,
        Arc::clone(&db),
        Some(Arc::clone(&svc_arc)),
    )?;

    let result = pipeline.analyze_directory(&test_files_dir()).await?;

    tracing::info!(
        "\n✅ Pipeline completed in {:.2}s",
        start_time.elapsed().as_secs_f64()
    );

    let embedding_service = Some((*svc_arc).clone());
    Ok(pipeline_result_to_analysis(
        result,
        db_path,
        embedding_service,
    ))
}

/// Get or create shared multilanguage analysis result.
/// Uses a `tokio::sync::OnceCell` so the analysis only runs once per test process.
///
/// If the fixture server is running, fetches from it instead of re-analysing.
pub async fn get_multilang_analysis_result() -> Result<AnalysisResult> {
    // Try fixture server first (shared across test binaries)
    if let Some(result) = fixture_client::try_fetch_multilang().await {
        tracing::info!("Using multilang result from fixture server");
        return Ok(result);
    }

    let state = MULTILANG_STATE
        .get_or_init(|| async { Arc::new(Mutex::new(None)) })
        .await;

    let mut guard = state.lock().await;

    if guard.is_none() {
        let result = perform_analysis_at(None).await?;
        *guard = Some(result);
    }

    Ok(guard.clone().unwrap())
}

/// Get or create shared semantic-search analysis result.
/// Uses a separate `OnceCell` from the multilanguage tests so the two test
/// suites can run independently without clobbering each other's database.
///
/// If the fixture server is running, fetches from it instead of re-analysing.
pub async fn get_semantic_analysis_result() -> Result<AnalysisResult> {
    // Try fixture server first (shared across test binaries)
    if let Some(result) = fixture_client::try_fetch_semantic().await {
        tracing::info!("Using semantic result from fixture server");
        return Ok(result);
    }

    let state = SEMANTIC_STATE
        .get_or_init(|| async { Arc::new(Mutex::new(None)) })
        .await;

    let mut guard = state.lock().await;

    if guard.is_none() {
        let result = perform_analysis_at(None).await?;
        *guard = Some(result);
    }

    Ok(guard.clone().unwrap())
}
