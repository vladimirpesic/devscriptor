//! `devscriptor watch` — keep the knowledge graph current in real time.
//!
//! Watches a project directory and runs an **incremental** re-scan whenever
//! source files change (create / modify / delete / rename). Filesystem events
//! are debounced so a burst of edits collapses into a single re-scan. Runs until
//! interrupted with Ctrl-C.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use devscriptor_core::config::LocatedConfig;
use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::embeddings::{
    EmbeddingConfig, MIN_INLINE_EMBED_FREE_MB as MIN_EMBED_FREE_MB,
};
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use devscriptor_core::utils::memory_guard::free_ram_mb;
use devscriptor_core::utils::validation::PathValidationOptions;
use devscriptor_core::watcher::{CodeWatcher, WatchConfig};

use crate::commands::common::validate_path;

/// Arguments accepted by `devscriptor watch`.
#[derive(Debug, Clone)]
pub struct WatchArgs {
    /// Directory to watch.
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
    /// Re-embed changed entities inline after each re-scan (requires ~4 GB RAM).
    pub embed: bool,
    /// Debounce window in milliseconds.
    pub debounce_ms: u64,
}

pub async fn execute(args: WatchArgs) -> Result<()> {
    let _validated_path = validate_path(&args.path, &PathValidationOptions::must_be_directory())?;
    let _validated_db = validate_path(&args.database, &PathValidationOptions::default())?;

    // Decide embedding strategy. Default: structural-only (fast, no model).
    // With --embed we keep the model resident across re-scans (unload = false)
    // so interactive semantic search stays warm, provided there is enough RAM.
    let free_mb = free_ram_mb();
    let embed = args.embed && free_mb >= MIN_EMBED_FREE_MB;
    if args.embed && !embed {
        eprintln!(
            "⚠️  Only {free_mb} MB free (< {MIN_EMBED_FREE_MB} MB required) — inline embedding \
             disabled; the watcher will keep the structural graph current only."
        );
    }

    let mut config = PipelineConfig {
        database: DatabaseConfig {
            path: args.database.to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        },
        embedding: if embed {
            Some(EmbeddingConfig::default())
        } else {
            None
        },
        skip_embeddings: !embed,
        // Keep the model loaded for the lifetime of the watcher.
        unload_model_after_scan: false,
        // Incremental is the whole point: only changed files are re-parsed and
        // stale/deleted entities are reconciled.
        incremental: true,
        enable_cross_file: true,
        // bulk_load is unsafe on incremental scans (it would drop FTS rows).
        bulk_load: false,
        ..PipelineConfig::default()
    };
    if let Some(project) = &args.project {
        project.config.apply_to(&mut config);
    }
    // The flags outrank the project's file.
    if !args.include.is_empty() {
        config.include_extensions = args.include.clone();
    }
    if !args.exclude.is_empty() {
        config.exclude_patterns = args.exclude.clone();
    }

    println!();
    println!("👁  Devscriptor Watch");
    println!("   Path      : {}", args.path.display());
    println!("   Database  : {}", args.database.display());
    if let Some(project) = &args.project {
        println!("   Config    : {}", project.path.display());
    }
    println!(
        "   Embeddings: {}",
        if embed {
            "inline (model resident)"
        } else {
            "structural only"
        }
    );
    println!("   Debounce  : {} ms", args.debounce_ms);

    // Watch first, then prime the graph with one full incremental scan, so the
    // DB reflects the current tree from the start and an edit made while that
    // scan runs triggers a re-scan.
    let pipeline = Arc::new(CodebasePipeline::new(config)?);
    let session = CodeWatcher::new(
        args.path.clone(),
        Arc::clone(&pipeline),
        WatchConfig {
            debounce: Duration::from_millis(args.debounce_ms),
            ..WatchConfig::default()
        },
    )
    .start()?;
    let state = session.state();
    match state.fallback_reason() {
        Some(reason) => println!(
            "   Mode      : polling {} directories ({reason})",
            state.watched_directories()
        ),
        None => println!(
            "   Mode      : file events, {} directories",
            state.watched_directories()
        ),
    }
    println!();
    match pipeline.analyze_directory(&args.path).await {
        Ok(r) => println!(
            "✅ Initial scan: {} files, {} entities, {} relationships written",
            r.files_parsed + r.files_skipped,
            r.entity_count,
            r.relationship_count
        ),
        Err(e) => eprintln!("⚠️  Initial scan failed: {e}"),
    }

    println!("   Watching for changes… press Ctrl-C to stop.");
    println!();

    let run = session.run(|rescan| {
        let r = &rescan.result;
        println!(
            "🔄 Updated: {} re-parsed, {} unchanged, {} entities, {} relationships written ({:.2}s)",
            r.files_parsed,
            r.files_skipped,
            r.entity_count,
            r.relationship_count,
            r.processing_time_secs,
        );
    });

    tokio::select! {
        () = run => {}
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("🛑 Stopping watcher.");
        }
    }

    Ok(())
}
