//! `devscriptor scan` — parse a codebase and build the knowledge graph.
//!
//! ## Two-phase design
//!
//! Scanning and embedding are **separate phases**:
//!
//! ```text
//! devscriptor scan .          # fast: parse → entities → FTS5 → done (<30s typical)
//! devscriptor embed           # slow: load ONNX → embed → unload (background/overnight)
//! ```
//!
//! `scan` is always fast because it defaults to `--skip-embeddings`.  Users who
//! want a single combined run can pass `--embed` to generate vectors inline.
//! The adaptive model selector will refuse to load the model if free RAM is
//! insufficient, printing a clear recovery message.

use std::path::PathBuf;

use anyhow::Result;
use devscriptor_core::config::LocatedConfig;
use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::embeddings::EmbeddingConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig, PipelineResult};
use devscriptor_core::utils::memory_guard::free_ram_mb;
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::validate_path;

/// Arguments accepted by `devscriptor scan`.
#[derive(Debug, Clone)]
pub struct ScanArgs {
    /// Directory to scan.
    pub path: PathBuf,
    /// Database path.
    pub database: PathBuf,
    /// The project's configuration file, if it has one: its settings apply
    /// unless the flags below set them.
    pub project: Option<LocatedConfig>,
    /// File extensions to include (empty = the project's, else all supported
    /// extensions).
    pub include: Vec<String>,
    /// Glob-style exclude patterns (empty = the project's).
    pub exclude: Vec<String>,
    /// Maximum file size in MB (none = the project's, else 50 MB).
    pub max_file_size: Option<usize>,
    /// Batch size for file processing.
    pub batch_size: Option<usize>,
    /// Enable incremental scan (skip unchanged files).
    pub incremental: bool,
    /// Generate embedding vectors inline after scanning.
    /// When false (default) a separate `devscriptor embed` run is required
    /// for semantic search.
    pub embed: bool,
    /// Force `skip_embeddings = true` even if `--embed` was specified and RAM
    /// would otherwise allow embedding.
    pub skip_embeddings: bool,
    /// Enable bulk-load mode (faster FTS5 rebuild for fresh scans).
    pub bulk_load: bool,
    /// Unload the ONNX model immediately after all embeddings are stored.
    /// Recommended for one-shot CLI scans.  Defaults to true when `--embed`
    /// is passed.
    pub unload_model_after_scan: bool,
    /// Print a full quality-metrics summary after scanning (entity/language
    /// distribution, cache-hit rate, peak RSS).
    pub metrics: bool,
}

/// Execute `devscriptor scan`.
pub async fn execute(args: ScanArgs) -> Result<()> {
    let _validated_path = validate_path(&args.path, &PathValidationOptions::must_be_directory())?;
    let _validated_db = validate_path(&args.database, &PathValidationOptions::default())?;

    let free_mb = free_ram_mb();

    println!();
    println!("🔍 Devscriptor Scan");
    println!("   Path     : {}", args.path.display());
    println!("   Database : {}", args.database.display());
    if let Some(project) = &args.project {
        println!("   Config   : {}", project.path.display());
    }
    println!("   Free RAM : {} MB", free_mb);

    // --- Decide embedding strategy ---
    let (embed_config, skip_embeds, embed_advice) = decide_embedding_strategy(&args, free_mb);

    if skip_embeds {
        println!("   Embeddings : ⏭  skipped — FTS5 keyword search available immediately");
        if let Some(ref advice) = embed_advice {
            println!("   {advice}");
        }
    } else {
        let model_name = embed_config
            .as_ref()
            .map(|_| devscriptor_core::embeddings::MODEL_CODE.to_string())
            .unwrap_or_else(|| "none".into());
        println!("   Embeddings : ✅ inline — model: {model_name}");
    }

    println!();

    let config = build_pipeline_config(&args, embed_config, skip_embeds);
    let pipeline = CodebasePipeline::new(config)?;
    let result = pipeline.analyze_directory(&args.path).await?;
    // Leave the whole index in the `.db` file, not in its write-ahead log.
    if let Err(e) = pipeline.db().checkpoint().await {
        tracing::warn!("Could not checkpoint the database: {e}");
    }

    // What the index holds now, across every scan of this database: the run's
    // own numbers count only what it stored.
    let totals = match pipeline.db().get_stats().await {
        Ok(totals) => Some(totals),
        Err(e) => {
            tracing::warn!("Could not read the index totals: {e}");
            None
        }
    };
    print_results(&result, skip_embeds, totals);
    if args.metrics {
        print_metrics(&result);
    }

    if skip_embeds {
        println!();
        println!("💡 To enable semantic search, run:");
        println!(
            "   devscriptor embed --database {}",
            args.database.display()
        );
    }

    Ok(())
}

/// Decide which embedding config to use, whether to skip, and an optional
/// user-facing advice string.
fn decide_embedding_strategy(
    args: &ScanArgs,
    free_mb: u64,
) -> (Option<EmbeddingConfig>, bool, Option<String>) {
    // Explicit --skip-embeddings always wins.
    if args.skip_embeddings {
        return (None, true, None);
    }

    // If --embed not passed, skip by default (fast scan).
    if !args.embed {
        return (
            None,
            true,
            Some(
                "💡 Tip: pass --embed to generate semantic vectors during scan, or run \
                 `devscriptor embed` separately."
                    .to_string(),
            ),
        );
    }

    // --embed was passed: use Jina 768-dim (the only supported model).
    // Require ≥ 4 GB free RAM; otherwise skip and advise.
    use devscriptor_core::embeddings::MIN_INLINE_EMBED_FREE_MB as MIN_FREE_MB;
    if free_mb < MIN_FREE_MB {
        let advice = format!(
            "⚠️  Only {free_mb} MB free (< {MIN_FREE_MB} MB required) — embedding \
             skipped to prevent OOM.\n\
             Free up memory and run `devscriptor embed` separately.",
        );
        return (None, true, Some(advice));
    }
    (Some(EmbeddingConfig::default()), false, None)
}

/// Build the full [`PipelineConfig`] from CLI arguments.
fn build_pipeline_config(
    args: &ScanArgs,
    embed_config: Option<EmbeddingConfig>,
    skip_embeds: bool,
) -> PipelineConfig {
    let db_path = args.database.to_string_lossy().to_string();
    if args.database.as_os_str().len() != db_path.len() {
        eprintln!(
            "⚠️  Warning: Database path contains non-UTF-8 characters and was lossily \
             converted: {db_path}"
        );
    }

    let mut config = PipelineConfig {
        database: DatabaseConfig {
            path: db_path,
            ..DatabaseConfig::default()
        },
        embedding: embed_config,
        skip_embeddings: skip_embeds,
        enable_cross_file: true,
        incremental: args.incremental,
        bulk_load: args.bulk_load,
        unload_model_after_scan: args.unload_model_after_scan || args.embed,
        ..PipelineConfig::default()
    };
    if let Some(project) = &args.project {
        project.config.apply_to(&mut config);
    }

    // The flags outrank the project's file.
    if let Some(max_size_mb) = args.max_file_size {
        config.max_file_size = max_size_mb as u64 * 1024 * 1024;
    }
    if let Some(batch_size) = args.batch_size {
        config.file_batch_size = batch_size;
    }
    if !args.include.is_empty() {
        config.include_extensions = args.include.clone();
    }
    if !args.exclude.is_empty() {
        config.exclude_patterns = args.exclude.clone();
    }

    config
}

/// Print a structured summary of the scan result. `totals` is what the index
/// holds after the run, as `(files, entities, relationships)`.
fn print_results(
    result: &PipelineResult,
    skip_embeds: bool,
    totals: Option<(usize, usize, usize)>,
) {
    println!("✅ Scan complete!");
    println!("   Files discovered : {}", result.files_discovered);
    println!("   Files parsed     : {}", result.files_parsed);
    if result.files_skipped > 0 {
        println!(
            "   Files skipped    : {} (incremental)",
            result.files_skipped
        );
    }
    if result.files_failed > 0 {
        println!("   Files failed     : {}", result.files_failed);
    }
    println!("   Entities         : {}", result.entity_count);
    println!(
        "   Relationships    : {} stored this run",
        result.relationship_count
    );
    if result.cross_file_relationship_count > 0 {
        println!(
            "      └─ across files : {}",
            result.cross_file_relationship_count
        );
    }

    if !skip_embeds && result.embedding_count > 0 {
        println!("   Vectors stored   : {}", result.embedding_count);
    }

    if let Some((files, entities, relationships)) = totals {
        println!(
            "   Index totals     : {files} files, {entities} entities, {relationships} relationships"
        );
    }

    println!("   Time             : {:.2}s", result.processing_time_secs);

    // Entity type breakdown (top 10).
    if !result.entity_type_counts.is_empty() {
        println!();
        println!("📊 Entity types:");
        let mut types: Vec<_> = result.entity_type_counts.iter().collect();
        types.sort_by(|a, b| b.1.cmp(a.1));
        for (entity_type, count) in types.iter().take(10) {
            println!("   {entity_type:<20} {count}");
        }
        if types.len() > 10 {
            println!("   … and {} more types", types.len() - 10);
        }
    }

    if !result.errors.is_empty() {
        println!();
        println!("⚠️  Parse errors (first 10):");
        for error in result.errors.iter().take(10) {
            println!("   {error}");
        }
        if result.errors.len() > 10 {
            println!("   … and {} more", result.errors.len() - 10);
        }
    }
}

/// Print the extended quality-metrics summary emitted by `--metrics`.
///
/// Covers: full entity-type and language distributions, embedding cache-hit
/// rate, and peak RSS read from `/proc/self/status` (Linux).
fn print_metrics(result: &PipelineResult) {
    println!();
    println!("📈 Quality metrics");

    // --- Language distribution ---
    if !result.entities_by_language.is_empty() {
        println!();
        println!("   Language distribution:");
        let mut langs: Vec<_> = result.entities_by_language.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (lang, count) in &langs {
            let pct = (*count * 100).checked_div(result.entity_count).unwrap_or(0);
            println!("   {lang:<20} {count:>6}  ({pct}%)");
        }
    }

    // --- Full entity-type distribution ---
    if !result.entity_type_counts.is_empty() {
        println!();
        println!("   Entity-type distribution:");
        let mut types: Vec<_> = result.entity_type_counts.iter().collect();
        types.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (etype, count) in &types {
            let pct = (*count * 100).checked_div(result.entity_count).unwrap_or(0);
            println!("   {etype:<20} {count:>6}  ({pct}%)");
        }
    }

    // --- Embedding cache-hit rate ---
    let total_embedding_ops = result.embedding_count + result.embedding_cache_hits;
    if let Some(hit_pct) = (result.embedding_cache_hits * 100).checked_div(total_embedding_ops) {
        println!();
        println!(
            "   Embedding cache hits : {} / {}  ({}%)",
            result.embedding_cache_hits, total_embedding_ops, hit_pct
        );
    }

    // --- Peak RSS (Linux only) ---
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmRSS:") || line.starts_with("VmPeak:") {
                println!("   {line}");
            }
        }
    }
}
