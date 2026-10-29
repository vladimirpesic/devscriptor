//! `devscriptor embed` — generate semantic embedding vectors for all
//! entities in an existing database.
//!
//! Always uses the INT8-quantized `jinaai/jina-embeddings-v2-base-code`
//! (768-dim, ~160 MB on disk, ~1.3 GB resident with its weights in f32). Runs
//! within ≥ 4 GB of available RAM.
//!
//! The command is fully **idempotent**: it checks `entity_vectors` and only
//! processes entities that are still missing vectors. Re-running after a
//! partial failure resumes from where it left off.
//!
//! ## Modes
//!
//! - Default: run inline, blocking until complete.
//! - `--daemon`: fork a background process, return immediately.
//!   Progress is written to the `embedding_jobs` table.
//! - `--status`: read and display the latest `embedding_jobs` row.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use devscriptor_core::config::ProjectConfig;
use devscriptor_core::database::Database;
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};
use futures::StreamExt as _;

use crate::commands::common::{database_config, validate_path};
use devscriptor_core::utils::validation::PathValidationOptions;

/// The fewest chunks per concurrent worker a job is cut into. See
/// [`run_embed`].
const BATCHES_PER_WORKER: usize = 3;

/// The most entities a worker takes at once: one database read, then its
/// texts embedded `batch_size` at a time, then one write.
const CHUNK_ENTITIES: usize = 16;

/// Arguments accepted by `devscriptor embed`.
#[derive(Debug, Clone)]
pub struct EmbedArgs {
    /// Path to the existing database file.
    pub database: PathBuf,
    /// The configuration file `--config` named, passed on to a background
    /// process.
    pub config: Option<PathBuf>,
    /// The project's settings: which entities to embed, and the pool size.
    pub project: ProjectConfig,
    /// Entities per inference run; 0 = automatic (one).
    pub batch_size: usize,
    /// Re-embed entities that already have vectors (overwrite).
    pub force: bool,
    /// Fork a background process and return immediately.
    pub daemon: bool,
    /// Show progress of the latest background job.
    pub status: bool,
}

/// Execute `devscriptor embed`.
pub async fn execute(args: EmbedArgs) -> Result<()> {
    if args.status {
        return show_status(&args.database, &args.project).await;
    }

    if args.daemon {
        return spawn_daemon(&args).await;
    }

    run_embed(args, None).await
}

/// Print the latest `embedding_jobs` row.
async fn show_status(database: &PathBuf, project: &ProjectConfig) -> Result<()> {
    validate_path(database, &PathValidationOptions::default()).context("Invalid database path")?;

    if !database.exists() {
        anyhow::bail!(
            "Database not found at {}.\n\
             Run `devscriptor scan <path>` first to build the index.",
            database.display()
        );
    }

    let db = Arc::new(
        Database::new(database_config(database, project)).context("Failed to open database")?,
    );

    match db.latest_embedding_job().await? {
        None => {
            println!("No embedding jobs recorded. Run `devscriptor embed --daemon` to start one.");
        }
        Some(job) => {
            let pct = job
                .done
                .checked_mul(100)
                .and_then(|n| n.checked_div(job.total))
                .unwrap_or(0);
            println!();
            println!("⚡ Embedding job status");
            println!("   ID         : {}", job.id);
            println!("   Status     : {}", job.status);
            println!("   Progress   : {}/{} ({pct}%)", job.done, job.total);
            println!("   Started    : {}", job.started_at);
            println!("   Updated    : {}", job.updated_at);
            println!();
        }
    }

    Ok(())
}

/// Spawn `devscriptor embed --run-daemon` as a detached child process.
///
/// The child inherits the same database path, configuration file, batch size,
/// and force flag.
/// It runs the full embedding loop and writes progress to `embedding_jobs`.
async fn spawn_daemon(args: &EmbedArgs) -> Result<()> {
    validate_path(&args.database, &PathValidationOptions::default())
        .context("Invalid database path")?;

    if !args.database.exists() {
        anyhow::bail!(
            "Database not found at {}.\n\
             Run `devscriptor scan <path>` first to build the index.",
            args.database.display()
        );
    }

    // Re-exec the current binary with --run-daemon (internal flag, not
    // exposed in the CLI help text).  We pass the db path and tuning flags.
    let exe = std::env::current_exe().context("Cannot determine current executable path")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--database").arg(&args.database);
    if let Some(config) = &args.config {
        cmd.arg("--config").arg(std::path::absolute(config)?);
    }
    cmd.arg("embed")
        .arg("--batch-size")
        .arg(args.batch_size.to_string());
    if args.force {
        cmd.arg("--force");
    }
    // Internal flag: run the embedding loop without forking again.
    cmd.arg("--run-daemon");

    // Detach: redirect stdio to /dev/null (or nul on Windows).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Spawn as a new session so it survives terminal close.
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
    }
    #[cfg(not(unix))]
    {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }

    let child = cmd
        .spawn()
        .context("Failed to spawn background embed process")?;
    let pid = child.id();

    println!();
    println!("⚡ Embedding daemon started (PID {pid})");
    println!("   Database : {}", args.database.display());
    println!("   Progress : devscriptor embed --status");
    println!();

    Ok(())
}

/// Execute `devscriptor embed`.
///
/// When called with `Some(job_id)` the function writes progress to the
/// `embedding_jobs` table (daemon mode). When called with `None` it runs
/// inline without any job tracking.
pub async fn run_embed(args: EmbedArgs, job_id: Option<String>) -> Result<()> {
    validate_path(&args.database, &PathValidationOptions::default())
        .context("Invalid database path")?;

    if !args.database.exists() {
        anyhow::bail!(
            "Database not found at {}.\n\
             Run `devscriptor scan <path>` first to build the index.",
            args.database.display()
        );
    }

    use devscriptor_core::embeddings::provider::JinaEmbeddingProvider;
    let batch_size = if args.batch_size == 0 {
        JinaEmbeddingProvider::compute_batch_size()
    } else {
        args.batch_size
    };
    let concurrency = JinaEmbeddingProvider::compute_session_count_for(batch_size);
    let threads = JinaEmbeddingProvider::compute_thread_count();
    let selection = args.project.embedding_selection();

    println!();
    println!("⚡ Devscriptor Embed");
    println!("   Database : {}", args.database.display());
    println!("   Model    : jinaai/jina-embeddings-v2-base-code (768-dim)");
    println!(
        "   Tuning   : {concurrency} concurrent run(s) × {batch_size} text(s), \
         {threads} matmul thread(s)"
    );
    println!("   Selection: {}", selection.as_str());

    // --- Open database ---
    let db = Arc::new(
        Database::new(database_config(&args.database, &args.project))
            .context("Failed to open database")?,
    );

    // --- Count pending entities ---
    let (unembedded_ids, total_entities) = db
        .get_unembedded_entity_ids(args.force, selection)
        .await
        .context("Failed to query database")?;

    // Register / update the job row if we're in daemon mode.
    let effective_job_id = match &job_id {
        Some(id) => id.clone(),
        None => {
            // Create a transient job row so `--status` always has something to read.
            db.create_embedding_job(total_entities)
                .await
                .unwrap_or_default()
        }
    };

    if unembedded_ids.is_empty() {
        println!();
        println!("✅ All {total_entities} entities already have embedding vectors.");
        println!("   Semantic search is fully available.");
        let _ = db.finish_embedding_job(&effective_job_id, true).await;
        return Ok(());
    }

    let to_embed = unembedded_ids.len();
    // `total_entities` counts every row; candidates are only the entities the
    // embedding selection embeds, so the difference is not work already done.
    println!(
        "   Entities : {to_embed} to embed  ({} not embedded by policy or already vectorized)",
        total_entities.saturating_sub(to_embed)
    );
    println!();
    println!("⏳ Loading embedding model…  (this may take 30–60 seconds)");

    let embedding_config = EmbeddingConfig {
        batch_size,
        ..EmbeddingConfig::default()
    };
    let svc = Arc::new(EmbeddingService::new(embedding_config));

    // Warmup / verify the model loaded before committing to a long run.
    svc.generate_embedding("warmup")
        .context("Failed to load embedding model. Is the model downloaded?\nRun `devscriptor doctor` or scripts/download-embedding-model.sh.")?;
    println!("✅ Model loaded.\n");

    // Enough chunks to keep every worker busy to the end: a small job cut into
    // a few large chunks left most workers idle while the last ones ran.
    let chunk_size = batch_size
        .max(CHUNK_ENTITIES)
        .min(to_embed.div_ceil(concurrency.max(1) * BATCHES_PER_WORKER))
        .max(1);
    let start = std::time::Instant::now();
    let stored_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cached_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let force = args.force;

    // Fan out chunks across `concurrency` blocking workers so every CPU core is
    // used. tract runs each batch single-threaded, so throughput comes from
    // embedding several chunks at once — the width is the RAM-aware
    // `compute_session_count()` computed above (mirrors the `scan --embed` path).
    // Each in-flight chunk holds its own model activations, so this is also the
    // memory lever: `concurrency` batches resident at peak.
    let owned_chunks: Vec<Vec<devscriptor_core::entities::EntityId>> = unembedded_ids
        .chunks(chunk_size)
        .map(<[devscriptor_core::entities::EntityId]>::to_vec)
        .collect();

    let results: Vec<Result<()>> =
        futures::stream::iter(owned_chunks.into_iter().map(|id_chunk| {
            let db = Arc::clone(&db);
            let svc = Arc::clone(&svc);
            let stored_counter = Arc::clone(&stored_counter);
            let cached_counter = Arc::clone(&cached_counter);
            let effective_job_id = effective_job_id.clone();
            async move {
                let texts_and_ids = db
                    .get_entity_texts_for_ids(&id_chunk)
                    .await
                    .context("Failed to load entity texts")?;

                if texts_and_ids.is_empty() {
                    return Ok(());
                }

                // A text embedded before, by any path (`scan --embed`, the
                // watcher, the MCP server or an earlier run), has its vector in
                // the embedding cache: reuse it instead of running the model.
                // `--force` re-embeds everything.
                let hashes: Vec<String> = texts_and_ids
                    .iter()
                    .map(|(_, text)| {
                        format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()))
                    })
                    .collect();
                let mut cached = if force {
                    std::collections::HashMap::new()
                } else {
                    db.lookup_embedding_cache(&hashes)
                        .await
                        .context("Failed to read the embedding cache")?
                };

                let mut items: Vec<(devscriptor_core::entities::EntityId, Vec<f32>)> =
                    Vec::with_capacity(texts_and_ids.len());
                let mut misses: Vec<(devscriptor_core::entities::EntityId, String, String)> =
                    Vec::new();
                for ((id, text), hash) in texts_and_ids.into_iter().zip(hashes) {
                    match cached.remove(&hash) {
                        Some(vector) => items.push((id, vector)),
                        None => misses.push((id, text, hash)),
                    }
                }
                let hits = items.len();

                let mut cache_pairs: Vec<(String, Vec<f32>)> = Vec::with_capacity(misses.len());
                if !misses.is_empty() {
                    let texts: Vec<String> = misses.iter().map(|(_, t, _)| t.clone()).collect();
                    let embeddings =
                        tokio::task::spawn_blocking(move || svc.generate_embeddings_batch(&texts))
                            .await
                            .context("ONNX inference task panicked")?
                            .context("Embedding generation failed")?;
                    for ((id, _, hash), vector) in misses.into_iter().zip(embeddings) {
                        cache_pairs.push((hash, vector.clone()));
                        items.push((id, vector));
                    }
                }

                let stored = db
                    .upsert_embeddings_batch(&items)
                    .await
                    .context("Failed to write vectors")?;

                if !cache_pairs.is_empty()
                    && let Err(e) = db.store_embedding_cache(&cache_pairs).await
                {
                    tracing::warn!("Failed to update embedding cache: {e}");
                }
                cached_counter.fetch_add(hits, std::sync::atomic::Ordering::Relaxed);

                let done =
                    stored_counter.fetch_add(stored, std::sync::atomic::Ordering::Relaxed) + stored;

                // Progress + job-table update. Chunks complete out of order, so base
                // the percentage on the running total rather than a chunk index.
                let _ = db.update_embedding_job(&effective_job_id, done).await;
                let progress_pct = done * 100 / to_embed.max(1);
                let elapsed = start.elapsed().as_secs_f64();
                let rate = if elapsed > 0.0 {
                    done as f64 / elapsed
                } else {
                    0.0
                };
                print!("\r   [{progress_pct:>3}%] {done}/{to_embed} entities  ({rate:.1}/sec)    ");
                #[allow(clippy::explicit_write)]
                {
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                Ok(())
            }
        }))
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // Surface the first error; partial progress is safe (embedding is idempotent
    // — a re-run resumes from the entities still missing vectors).
    for r in results {
        r?;
    }
    let total_stored = stored_counter.load(std::sync::atomic::Ordering::Relaxed);
    let total_cached = cached_counter.load(std::sync::atomic::Ordering::Relaxed);

    // Unload the shared model immediately to free its RAM (~1.3 GB resident during embedding).
    svc.unload_model();

    let _ = db.finish_embedding_job(&effective_job_id, true).await;
    // Leave the vectors in the `.db` file, not in its write-ahead log.
    if let Err(e) = db.checkpoint().await {
        tracing::warn!("Could not checkpoint the database: {e}");
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!();
    println!();
    println!("✅ Embedding complete!");
    println!("   Vectors stored : {total_stored}");
    if total_cached > 0 {
        println!("   From the cache : {total_cached} (text embedded before)");
    }
    println!("   Time elapsed   : {elapsed:.1}s");
    if elapsed > 0.0 {
        println!(
            "   Throughput     : {:.1} entities/sec",
            total_stored as f64 / elapsed
        );
    }
    println!();
    println!("🔍 Semantic (vector) search is now available.");
    println!("   Try: devscriptor find-similar \"your query\"");

    Ok(())
}
