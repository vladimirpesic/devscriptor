//! Unified codebase analysis pipeline
//!
//! This module provides the canonical orchestration for analyzing a codebase:
//! 1. Discover files
//! 2. Parse files and store them: entities, the references they make, imports
//!    and the nesting of members in types, then embeddings
//! 3. Derive the edges from the stored references ([`crate::graph::resolve`]):
//!    every reference of the files the run parsed, and every other reference
//!    under the root whose resolution the run may have changed
//!
//! Both the CLI and integration tests should use this pipeline to ensure
//! they exercise the exact same production code paths.

use crate::database::embeddings::entity_to_text;
use crate::database::{Database, DatabaseConfig, FileRecord, ReindexGuard};
use crate::embeddings::{EmbeddingConfig, EmbeddingService};
use crate::entities::{Entity, EntityId};
use crate::error::Result;
use crate::graph::resolve::{Resolution, derive_edges_for_new_sources, rebuild_edges_for_sources};
use crate::parser::cross_file::CrossFileMode;
use crate::parser::symbol_table::DEFAULT_AMBIGUOUS_FANOUT;
use crate::parser::{ParserConfig, TreeSitterParser};
use crate::processing::scanner::{FileEntry, FileFilter, FileScanner};
use crate::utils::memory_guard::MemoryGuard;
use futures::StreamExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{info, warn};

/// Configuration for the codebase analysis pipeline
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Parser configuration
    pub parser: ParserConfig,
    /// Embedding configuration (None = skip embedding generation)
    pub embedding: Option<EmbeddingConfig>,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Whether to extract cross-file relationships
    pub enable_cross_file: bool,
    /// Cross-file analysis mode
    pub cross_file_mode: CrossFileMode,
    /// Batch size for file processing
    pub file_batch_size: usize,
    /// Batch size for embedding generation
    pub embedding_batch_size: usize,
    /// Maximum file size in bytes (0 = unlimited)
    pub max_file_size: u64,
    /// Whether to use gitignore filtering
    pub use_gitignore: bool,
    /// File extensions to include (empty = all)
    pub include_extensions: Vec<String>,
    /// Exclude patterns matched against file paths
    pub exclude_patterns: Vec<String>,
    /// Maximum concurrent file parsing tasks (0 = auto = num_cpus)
    pub max_concurrency: usize,
    /// Enable incremental scanning (skip unchanged files using scan_manifest)
    pub incremental: bool,
    /// Free-RAM threshold in MB for back-pressure throttling
    pub memory_guard_mb: u64,
    /// Bulk-load mode: drop FTS5 sync triggers before parsing, then rebuild
    /// the full-text index once at the end.  Significantly faster for fresh
    /// scans because per-row FTS trigger overhead is replaced by a single
    /// `INSERT INTO entities_fts(entities_fts) VALUES('rebuild')` call.
    /// Do NOT enable on incremental scans — the index would lose existing rows.
    pub bulk_load: bool,

    /// Unload the ONNX model from RAM immediately after all embeddings are
    /// persisted.  Frees the shared model (~1.3 GB resident during embedding).
    ///
    /// Set to `true` for one-shot CLI scans where the process exits shortly
    /// after scanning completes.  Set to `false` (default) for long-lived
    /// server processes (LSP, MCP) that perform interactive semantic search —
    /// unloading would cause a cold-reload penalty on the first search after
    /// each scan.
    pub unload_model_after_scan: bool,

    /// Skip embedding generation entirely.
    ///
    /// When `true`, the ONNX model is never loaded and no vectors are written
    /// to `entity_vectors`.  FTS5 full-text search and exact/fuzzy name search
    /// remain fully functional.  Semantic (vector) search will return an error
    /// until embeddings are generated in a subsequent run.
    ///
    /// **Use this as the default on consumer hardware when first scanning a
    /// large repo.** Embeddings can be added later via `devscriptor embed`.
    pub skip_embeddings: bool,

    /// Which entities receive a semantic embedding. Pruning non-semantic noise
    /// (constants, usages, …) cuts embedding cost and improves search precision;
    /// skipped entities remain findable via lexical/FTS search. See
    /// [`EmbeddingSelection`](crate::embeddings::EmbeddingSelection).
    pub embedding_selection: crate::embeddings::EmbeddingSelection,

    /// How many of several equally good definitions of a name a reference is
    /// linked to, in an order that is the same in every run and checkout. Each
    /// of those edges is marked ambiguous, so a caller of overloads reaches
    /// all of them and tools can tell a guess from a resolved edge. 1 links
    /// only the first; 0 counts as 1. Default: [`DEFAULT_AMBIGUOUS_FANOUT`].
    pub ambiguous_fanout: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            parser: ParserConfig::default(),
            embedding: None,
            database: DatabaseConfig::default(),
            enable_cross_file: true,
            cross_file_mode: CrossFileMode::Auto,
            file_batch_size: 50,
            // ONNX inference chunk size. Peak activation memory scales with
            // this × seq_len², so it is memory-bounded at 16 (see
            // EmbeddingConfig::batch_size). Raise only with ≥ 8 GB free RAM.
            embedding_batch_size: 16,
            max_file_size: crate::utils::validation::DEFAULT_MAX_FILE_SIZE,
            use_gitignore: true,
            include_extensions: Vec::new(),
            exclude_patterns: Vec::new(),
            // In tests limit concurrency to 2 to bound AST memory; in
            // production 0 means "auto = num_cpus".
            max_concurrency: if cfg!(test) { 2 } else { 0 },
            incremental: false,
            // 0 = use adaptive threshold based on total system RAM.
            memory_guard_mb: 0,
            bulk_load: false,
            unload_model_after_scan: false,
            skip_embeddings: false,
            embedding_selection: crate::embeddings::EmbeddingSelection::from_env_or(
                crate::embeddings::EmbeddingSelection::Standard,
            ),
            ambiguous_fanout: DEFAULT_AMBIGUOUS_FANOUT,
        }
    }
}

/// The settings of a [`PipelineConfig`] that a project's configuration file
/// sets, and that may change while a pipeline lives
/// ([`CodebasePipeline::set_scope`]): which files a run takes in, how their
/// references resolve across files, which entities it embeds.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanScope {
    /// File extensions to include (empty = all).
    pub include_extensions: Vec<String>,
    /// Exclude patterns, relative to the scanned root.
    pub exclude_patterns: Vec<String>,
    /// Maximum file size in bytes (0 = unlimited).
    pub max_file_size: u64,
    /// Whether git's ignore files apply.
    pub use_gitignore: bool,
    /// Which references resolve across files.
    pub cross_file_mode: CrossFileMode,
    /// Which entities receive a vector.
    pub embedding_selection: crate::embeddings::EmbeddingSelection,
}

impl ScanScope {
    /// The scope `config` sets.
    #[must_use]
    pub fn of(config: &PipelineConfig) -> Self {
        Self {
            include_extensions: config.include_extensions.clone(),
            exclude_patterns: config.exclude_patterns.clone(),
            max_file_size: config.max_file_size,
            use_gitignore: config.use_gitignore,
            cross_file_mode: config.cross_file_mode,
            embedding_selection: config.embedding_selection,
        }
    }
}

/// Result of a codebase analysis run
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Total files discovered
    pub files_discovered: usize,
    /// Files successfully parsed
    pub files_parsed: usize,
    /// Files that failed parsing
    pub files_failed: usize,
    /// Files skipped during incremental scan (unchanged)
    pub files_skipped: usize,
    /// Total entities extracted
    pub entity_count: usize,
    /// Relationship rows this run inserted (per-file and cross-file, forward
    /// and reverse). An edge that was already stored is not counted, so an
    /// incremental re-scan of an unchanged tree reports 0.
    pub relationship_count: usize,
    /// Of `relationship_count`, the rows whose two endpoints lie in different
    /// files.
    pub cross_file_relationship_count: usize,
    /// Embeddings generated
    pub embedding_count: usize,
    /// Embeddings served from cache (no ONNX inference required)
    pub embedding_cache_hits: usize,
    /// Entity counts by type
    pub entity_type_counts: HashMap<String, usize>,
    /// Entity counts by language
    pub entities_by_language: HashMap<String, usize>,
    /// Processing time in seconds
    pub processing_time_secs: f64,
    /// Errors encountered
    pub errors: Vec<String>,
}

impl PipelineResult {
    /// Create an empty result
    #[must_use]
    pub fn new() -> Self {
        Self {
            files_discovered: 0,
            files_parsed: 0,
            files_failed: 0,
            files_skipped: 0,
            entity_count: 0,
            relationship_count: 0,
            cross_file_relationship_count: 0,
            embedding_count: 0,
            embedding_cache_hits: 0,
            entity_type_counts: HashMap::new(),
            entities_by_language: HashMap::new(),
            processing_time_secs: 0.0,
            errors: Vec::new(),
        }
    }
}

impl Default for PipelineResult {
    fn default() -> Self {
        Self::new()
    }
}

/// What parsing a set of files leaves for the steps after it.
struct ParseOutput {
    /// `(path, hash, size_bytes, mtime_secs)` of each parsed file, for the
    /// manifest.
    processed: Vec<(String, String, u64, u64)>,
    /// When references resolve across files, the ids of the entities stored
    /// for the parsed files, in the order they were parsed.
    stored: Vec<EntityId>,
}

/// Unified codebase analysis pipeline.
///
/// This is the canonical production pipeline. Both the CLI and tests
/// should instantiate and run this to ensure they exercise identical code paths.
pub struct CodebasePipeline {
    config: PipelineConfig,
    /// What a project's configuration may change while the pipeline lives
    /// ([`Self::set_scope`]); taken from `config` at first.
    scope: parking_lot::RwLock<ScanScope>,
    parser: TreeSitterParser,
    db: Arc<Database>,
    embedding_service: Option<Arc<EmbeddingService>>,
    /// Held by each scan and re-index run, so that runs through one pipeline
    /// never overlap: each derives its edges from the index as the run
    /// before it left it.
    runs: tokio::sync::Mutex<()>,
}

/// The scan root reduced to one canonical spelling.
///
/// Every stored `file_path` is this root plus a relative remainder, so the root's
/// spelling decides the identity of every entity in the index. `devscriptor scan .`
/// passes a relative root, and the MCP `scan_codebase` tool an absolute one;
/// reduced to one spelling, a scan by either replaces the rows the other wrote,
/// where the re-scan cleanup's `DELETE FROM entities WHERE file_path IN (...)`,
/// which compares the literal string, would otherwise add a second copy of every
/// entity.
///
/// Falls back to a lexical absolutization when the directory cannot be resolved
/// — a caller may be probing a path that does not exist yet, and an unstable
/// result is worse than an unresolved one.
fn canonical_root(root: &Path) -> PathBuf {
    if let Ok(resolved) = dunce::canonicalize(root) {
        return resolved;
    }
    if root.is_absolute() {
        return root.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(root))
        .unwrap_or_else(|_| root.to_path_buf())
}

/// `path` spelled as a scan stores it: its folder with symlinks resolved, as
/// a scan's root is, and its own name as given — the name of a file since
/// deleted, or of a symlink, which a scan lists by its own name. A path
/// reached through a symlinked folder (a watcher's event, macOS's `/var`,
/// which is `/private/var`) is then the file a scan of the folder stored.
fn as_stored(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => dunce::canonicalize(parent)
            .unwrap_or_else(|_| as_stored(parent))
            .join(name),
        _ => path.to_path_buf(),
    }
}

/// The names whose definitions differ between `before` and `after`, two
/// reads of [`Database::get_definitions_in_files`] for the same files: added,
/// removed, other entities, or the same in another order. A reference to any
/// other name resolves as it did, whatever else changed in those files.
fn changed_names(before: Vec<(String, EntityId)>, after: &[(String, EntityId)]) -> Vec<String> {
    let mut old: HashMap<String, Vec<EntityId>> = HashMap::new();
    for (name, id) in before {
        old.entry(name).or_default().push(id);
    }
    let mut new: HashMap<&str, Vec<EntityId>> = HashMap::new();
    for (name, id) in after {
        new.entry(name.as_str()).or_default().push(*id);
    }
    let mut changed: Vec<String> = new
        .iter()
        .filter(|(name, ids)| old.get(**name) != Some(*ids))
        .map(|(name, _)| (*name).to_string())
        .collect();
    changed.extend(
        old.into_keys()
            .filter(|name| !new.contains_key(name.as_str())),
    );
    changed.sort_unstable();
    changed
}

impl CodebasePipeline {
    /// Create a new pipeline with the given configuration.
    ///
    /// If the system is under critical memory pressure and embeddings are
    /// configured, the model load is skipped and a warning is emitted.  This
    /// prevents the embedding model's memory spike (~1.3 GB) from pushing a
    /// marginal system over the edge into OOM.
    pub fn new(config: PipelineConfig) -> Result<Self> {
        let parser = TreeSitterParser::new(config.parser.clone())?
            .with_ambiguous_fanout(config.ambiguous_fanout);
        let db = Arc::new(Database::new(config.database.clone())?);

        let embedding_service = if config.skip_embeddings {
            if config.embedding.is_some() {
                warn!(
                    "skip_embeddings=true: ONNX model will NOT be loaded. \
                     FTS5 keyword search is available; semantic search requires \
                     a subsequent `devscriptor embed` run."
                );
            }
            None
        } else {
            Self::maybe_create_embedding_service(config.embedding.as_ref())
        };

        Ok(Self {
            scope: parking_lot::RwLock::new(ScanScope::of(&config)),
            config,
            parser,
            db,
            embedding_service,
            runs: tokio::sync::Mutex::new(()),
        })
    }

    /// Create a pipeline with an existing database (for tests that share a DB).
    pub fn with_database(config: PipelineConfig, db: Arc<Database>) -> Result<Self> {
        let parser = TreeSitterParser::new(config.parser.clone())?
            .with_ambiguous_fanout(config.ambiguous_fanout);

        let embedding_service = if config.skip_embeddings {
            None
        } else {
            Self::maybe_create_embedding_service(config.embedding.as_ref())
        };

        Ok(Self {
            scope: parking_lot::RwLock::new(ScanScope::of(&config)),
            config,
            parser,
            db,
            embedding_service,
            runs: tokio::sync::Mutex::new(()),
        })
    }

    /// Create a pipeline re-using an existing database **and** embedding service.
    ///
    /// This is used by integration tests that need to analyse multiple
    /// directories into the same database without reloading the model each
    /// time (FIX-TEST).
    pub fn with_database_and_service(
        config: PipelineConfig,
        db: Arc<Database>,
        embedding_service: Option<Arc<EmbeddingService>>,
    ) -> Result<Self> {
        let parser = TreeSitterParser::new(config.parser.clone())?
            .with_ambiguous_fanout(config.ambiguous_fanout);
        Ok(Self {
            scope: parking_lot::RwLock::new(ScanScope::of(&config)),
            config,
            parser,
            db,
            embedding_service,
            runs: tokio::sync::Mutex::new(()),
        })
    }

    /// Helper: create an embedding service unless the system is in critical
    /// memory pressure.
    fn maybe_create_embedding_service(
        embedding_config: Option<&EmbeddingConfig>,
    ) -> Option<Arc<EmbeddingService>> {
        let ec = embedding_config?;

        let guard = MemoryGuard::new_adaptive();
        if guard.is_critical() {
            warn!(
                "Memory pressure detected before model load ({} MB free < {} MB hard stop). \
                 Skipping embeddings to prevent OOM. Use `--skip-embeddings` to suppress this warning.",
                guard.threshold_mb(),
                guard.hard_stop_mb()
            );
            return None;
        }

        Some(Arc::new(EmbeddingService::new(ec.clone())))
    }

    /// Run the full pipeline on a directory.
    ///
    /// Parsing is done in parallel (bounded concurrency) and entities are
    /// streamed to the database to keep memory bounded on large repos.
    /// Phase 3 derives the edges from the stored references, a chunk of
    /// entities at a time: those of every entity the run parsed and, when the
    /// index already held entities under `root`, those of every other entity
    /// there that refers to a name whose definitions the run changed (see
    /// [`Self::reindex_paths`]).
    pub async fn analyze_directory(&self, root: &Path) -> Result<PipelineResult> {
        let start = OffsetDateTime::now_utc();
        let mut result = PipelineResult::new();

        // Settle the root's spelling before anything derives a path from it.
        // Every stored `file_path` is this root plus a relative remainder, so two
        // spellings of the same directory produce two spellings of every file —
        // and the re-scan cleanup compares literal strings, so it never matches
        // across them. See [`canonical_root`].
        let canonical = canonical_root(root);
        let root = canonical.as_path();

        // No re-index of single files meanwhile.
        let _run = self.runs.lock().await;

        // ========== PHASE 1: DISCOVER FILES ==========
        info!("Phase 1: Discovering files in {}", root.display());
        let scanner = self.scanner();

        let entries = scanner.scan(root, None).await?;
        result.files_discovered = entries.len();
        info!("Discovered {} files", entries.len());

        // ========== PHASE 1b: RE-INDEX DETECTION & INCREMENTAL FILTERING ==========
        // A database that already holds entities is being re-indexed, in either
        // mode: each re-parsed file then has its own edges replaced and its
        // vanished entities dropped, and files gone since the last scan are
        // deleted. A fresh index (empty database) has nothing to replace.
        let reindex = self.config.incremental
            || self
                .db
                .get_stats()
                .await
                .map(|(_, e, _)| e > 0)
                .unwrap_or(false);

        let mut hash_map: HashMap<PathBuf, String> = HashMap::new();

        // Build manifest map keyed by path: (hash, size, mtime).
        let manifest_map: HashMap<String, (String, u64, u64)> = if reindex {
            self.db
                .load_scan_manifest()
                .await?
                .into_iter()
                .map(|e| (e.path, (e.content_hash, e.size_bytes, e.mtime_secs)))
                .collect()
        } else {
            HashMap::new()
        };

        // Files this root had at the last scan that no longer exist. Only paths
        // under this root count: the database may hold other projects, whose
        // files a scan of this root never sees and must not delete. A file that
        // still exists but was not discovered (a narrower `--include`, a new
        // ignore rule) is left alone.
        let removed_paths: Vec<String> = {
            let discovered_paths: std::collections::HashSet<String> = entries
                .iter()
                .map(|e| e.path.to_string_lossy().to_string())
                .collect();
            let mut removed: Vec<String> = manifest_map
                .keys()
                .filter(|p| {
                    Path::new(p.as_str()).starts_with(root)
                        && !discovered_paths.contains(*p)
                        && !Path::new(p.as_str()).exists()
                })
                .cloned()
                .collect();
            removed.sort();
            removed
        };
        if !removed_paths.is_empty() {
            info!(
                "Re-index: {} files removed since last scan",
                removed_paths.len()
            );
        }

        let entries = if self.config.incremental {
            // Fast-path: skip hashing when size AND mtime match the manifest.
            // Only hash files whose (size, mtime) differs — this avoids reading
            // file content for the majority of unchanged files in large repos.
            let needs_hash: Vec<&FileEntry> = entries
                .iter()
                .filter(|entry| {
                    let path_str = entry.path.to_string_lossy().to_string();
                    match manifest_map.get(&path_str) {
                        Some((_, saved_size, saved_mtime))
                            if *saved_size == entry.size && *saved_mtime == entry.modified =>
                        {
                            false // size+mtime match: no need to hash
                        }
                        _ => true, // changed or new: must hash
                    }
                })
                .collect();

            let hash_futures: Vec<_> = needs_hash
                .iter()
                .map(|entry| {
                    let path = entry.path.clone();
                    tokio::task::spawn_blocking(move || {
                        match crate::utils::security::compute_file_hash(&path) {
                            Ok(hash) => Ok((path, hash)),
                            Err(e) => Err((path, e.to_string())),
                        }
                    })
                })
                .collect();

            let hash_results = futures::future::join_all(hash_futures).await;
            for res in hash_results {
                match res {
                    Ok(Ok((path, hash))) => {
                        hash_map.insert(path, hash);
                    }
                    Ok(Err((path, e))) => {
                        warn!("Failed to hash {}: {}", path.display(), e);
                    }
                    Err(e) => {
                        warn!("Hash computation task failed: {}", e);
                    }
                }
            }

            let mut skipped = 0usize;
            let filtered: Vec<FileEntry> = entries
                .into_iter()
                .filter(|entry| {
                    let path_str = entry.path.to_string_lossy().to_string();

                    // Fast-path: unchanged by size+mtime, no hash computed.
                    if let Some((_, saved_size, saved_mtime)) = manifest_map.get(&path_str)
                        && *saved_size == entry.size
                        && *saved_mtime == entry.modified
                    {
                        skipped += 1;
                        return false;
                    }

                    // Slow-path: check content hash.
                    if let Some(hash) = hash_map.get(&entry.path)
                        && let Some((saved_hash, _, _)) = manifest_map.get(&path_str)
                        && saved_hash == hash
                    {
                        skipped += 1;
                        return false;
                    }

                    true
                })
                .collect();

            result.files_skipped = skipped;
            info!(
                "Incremental: skipped {} unchanged files ({} hashed, {} mtime-fast-pathed), processing {}",
                skipped,
                hash_map.len(),
                skipped.saturating_sub(hash_map.len()),
                filtered.len()
            );
            filtered
        } else {
            entries
        };

        // ========== PHASE 1d: BULK-LOAD SETUP ==========
        if self.config.bulk_load && !self.config.incremental {
            info!("Bulk-load mode: dropping FTS5 sync triggers for fast batch insert");
            self.db.begin_bulk_load().await?;
        }

        // ========== PHASE 2: PARSE FILES (parallel, bounded) ==========
        // On a re-index, each batch replaces its files' own edges and then drops
        // the entities (and vectors) that vanished from them.
        let reindex_root = reindex.then_some(root);
        let hashes = self.config.incremental.then_some(&hash_map);
        let files: Vec<String> = entries
            .iter()
            .map(FileEntry::path_str)
            .chain(removed_paths.iter().cloned())
            .collect();
        // An answer read from the index while this run writes these files
        // may hold their older entities: it learns so once the run is over.
        let mut reindexing = self.db.reindexing(files.clone());
        let before = self.definitions_before(root, &files).await?;
        let ParseOutput { processed, stored } = self
            .parse_and_insert(entries, reindex_root, hashes, &mut result)
            .await?;

        // ========== PHASE 2b: UPDATE MANIFEST & CLEANUP REMOVED FILES ==========
        self.update_manifest(&processed, self.config.incremental)
            .await?;
        // Removed-file cleanup, on any re-index, full or incremental: the
        // manifest says what the previous scan saw.
        self.remove_files(&removed_paths).await?;

        // ========== PHASE 3: RELATIONSHIPS FROM REFERENCES ==========
        if self.resolves_references() {
            self.relink(root, &files, before, stored, &mut reindexing, &mut result)
                .await?;
        }

        // ========== PHASE 3b: FTS REBUILD (bulk-load mode) ==========
        if self.config.bulk_load && !self.config.incremental {
            info!("Bulk-load mode: rebuilding FTS5 index and restoring triggers");
            if let Err(e) = self.db.end_bulk_load().await {
                warn!("FTS5 rebuild failed: {e}");
            }
        }

        // `devscriptor mcp` watches the scanned root it runs in.
        self.db.add_indexed_root(root).await?;

        // ========== PHASE 4: CLEANUP ORPHANED VECTORS ==========
        if self.embedding_service.is_some() {
            match self.db.cleanup_orphan_vectors().await {
                Ok(removed) if removed > 0 => {
                    warn!("Cleaned up {removed} orphaned embedding vectors left by previous runs");
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("Failed to cleanup orphan vectors: {e}");
                }
            }
        }

        let end = OffsetDateTime::now_utc();
        result.processing_time_secs = (end - start).as_seconds_f64();

        // Optionally unload the ONNX model after all embeddings are persisted.
        // Only do this when the caller has opted in — long-lived servers (LSP,
        // MCP) must keep the model loaded for interactive search queries.
        if self.config.unload_model_after_scan
            && let Some(ref svc) = self.embedding_service
        {
            svc.unload_model();
            info!(
                "Embedding model unloaded after scan (unload_model_after_scan=true). \
                 Freed the shared model (~1.3 GB). Semantic search remains available via stored vectors."
            );
        }

        Ok(result)
    }

    /// Re-index `paths`, files under `root` known to have changed, without
    /// walking the tree: what the watcher and the query-time freshness check
    /// call.
    ///
    /// A path a scan of `root` would not index (another extension, an exclude
    /// pattern, an ignore rule, a hidden directory, outside `root`) is skipped.
    /// A file whose size and modification time match the manifest is left
    /// alone; one whose content does only has those refreshed in the
    /// manifest. Any other file is parsed and stored with the semantics of a
    /// re-index (its own edges replaced, vanished entities dropped, vectors
    /// kept unless the body changed). A path that no longer exists, file or
    /// directory, is removed from the index and the manifest.
    ///
    /// The edges are then derived again from the stored references (see
    /// [`crate::graph::resolve`]) for the entities of the re-parsed files, and
    /// for every entity elsewhere under `root` with a reference to a name
    /// whose definitions the run added, removed or changed: a call in an
    /// unchanged file to a function an edited file now defines is linked.
    /// Resolution sees only the entities under `root`, as a scan of it does.
    ///
    /// # Errors
    /// Returns an error if parsing or a database operation fails.
    pub async fn reindex_paths(&self, root: &Path, paths: &[PathBuf]) -> Result<PipelineResult> {
        let start = OffsetDateTime::now_utc();
        let canonical = canonical_root(root);
        let root = canonical.as_path();

        // One re-index at a time, and none during a full scan.
        let _run = self.runs.lock().await;
        let mut result = self.reindex_locked(root, paths).await?;
        result.processing_time_secs = (OffsetDateTime::now_utc() - start).as_seconds_f64();
        Ok(result)
    }

    /// [`Self::reindex_paths`], with the run lock held.
    async fn reindex_locked(&self, root: &Path, paths: &[PathBuf]) -> Result<PipelineResult> {
        let mut result = PipelineResult::new();
        let mut wanted: Vec<PathBuf> = paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    as_stored(path)
                } else {
                    root.join(path)
                }
            })
            .filter(|path| path.starts_with(root) && path != root)
            .collect();
        wanted.sort();
        wanted.dedup();
        result.files_discovered = wanted.len();

        // Gone: what the manifest has at a missing path, or inside it when it
        // was a directory.
        let missing: Vec<String> = wanted
            .iter()
            .filter(|path| std::fs::symlink_metadata(path).is_err())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let mut removed: Vec<String> = self
            .db
            .get_scan_manifest_entries(&missing)
            .await?
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        for path in &missing {
            removed.extend(self.db.scan_manifest_paths_under(Path::new(path)).await?);
        }
        removed.sort();
        removed.dedup();

        // Present: the files a scan of `root` would index.
        let scanner = self.scanner();
        let found = {
            let root = root.to_path_buf();
            tokio::task::spawn_blocking(move || scanner.entries_for(&root, &wanted))
                .await
                .map_err(|e| crate::error::Error::task_join("find the files to re-index", e))?
        };

        // Of those, the ones whose content changed since the manifest.
        let found_paths: Vec<String> = found.iter().map(FileEntry::path_str).collect();
        let known: HashMap<String, (String, u64, u64)> = self
            .db
            .get_scan_manifest_entries(&found_paths)
            .await?
            .into_iter()
            .map(|entry| {
                (
                    entry.path,
                    (entry.content_hash, entry.size_bytes, entry.mtime_secs),
                )
            })
            .collect();
        let mut candidates = Vec::new();
        for entry in found {
            let unchanged = known
                .get(&entry.path_str())
                .is_some_and(|(_, size, mtime)| *size == entry.size && *mtime == entry.modified);
            if unchanged {
                result.files_skipped += 1;
            } else {
                candidates.push(entry);
            }
        }
        let hashed = futures::future::join_all(candidates.into_iter().map(|entry| {
            tokio::task::spawn_blocking(move || {
                let hash = crate::utils::security::compute_file_hash(&entry.path);
                (entry, hash)
            })
        }))
        .await;
        let mut hashes: HashMap<PathBuf, String> = HashMap::new();
        let mut changed: Vec<FileEntry> = Vec::new();
        // Touched but not changed: the manifest takes the new size and time,
        // so the next check takes the fast path.
        let mut refreshed: Vec<(String, String, String, u64, u64)> = Vec::new();
        let now = OffsetDateTime::now_utc().to_string();
        for joined in hashed {
            let (entry, hash) =
                joined.map_err(|e| crate::error::Error::task_join("hash a file to re-index", e))?;
            let hash = match hash {
                Ok(hash) => hash,
                Err(e) => {
                    warn!("Failed to hash {}: {e}", entry.path.display());
                    result.files_failed += 1;
                    continue;
                }
            };
            let path = entry.path_str();
            if known.get(&path).is_some_and(|(saved, ..)| *saved == hash) {
                result.files_skipped += 1;
                refreshed.push((path, hash, now.clone(), entry.size, entry.modified));
            } else {
                hashes.insert(entry.path.clone(), hash);
                changed.push(entry);
            }
        }
        if !refreshed.is_empty() {
            self.db.batch_upsert_scan_manifest(refreshed).await?;
        }
        if changed.is_empty() && removed.is_empty() {
            return Ok(result);
        }
        info!(
            "Re-index of {}: {} changed, {} removed",
            root.display(),
            changed.len(),
            removed.len()
        );
        let files: Vec<String> = changed
            .iter()
            .map(FileEntry::path_str)
            .chain(removed.iter().cloned())
            .collect();
        // An answer read from the index while this run writes these files
        // may hold their older entities: it learns so once the run is over.
        let mut reindexing = self.db.reindexing(files.clone());
        let before = self.definitions_before(root, &files).await?;

        let ParseOutput { processed, stored } = self
            .parse_and_insert(changed, Some(root), Some(&hashes), &mut result)
            .await?;
        self.update_manifest(&processed, true).await?;
        self.remove_files(&removed).await?;
        if self.resolves_references() {
            self.relink(root, &files, before, stored, &mut reindexing, &mut result)
                .await?;
        }
        Ok(result)
    }

    /// Phase 2: parse `entries` in parallel (bounded) and store them in
    /// batches: files, entities, references, imports and the relationships
    /// within each file, then their embeddings once the batch is persisted.
    /// When references resolve across files, the relationships stored are
    /// only the nesting of members in types: [`Self::relink`] derives the
    /// rest once every file is stored. With `reindex_root` each batch replaces
    /// what the index held for its files (their own edges, vanished entities,
    /// vectors of changed bodies). `hashes` are the files' content hashes when
    /// already computed, for the manifest; otherwise the parsed content is
    /// hashed.
    async fn parse_and_insert(
        &self,
        entries: Vec<FileEntry>,
        reindex_root: Option<&Path>,
        hashes: Option<&HashMap<PathBuf, String>>,
        result: &mut PipelineResult,
    ) -> Result<ParseOutput> {
        info!("Phase 2: Parsing files...");
        let phase2_start = std::time::Instant::now();
        // Accumulate time spent purely in embedding generation and DB insertion
        // so the phase breakdown distinguishes parse/extract cost from ONNX
        // inference cost from DB-write cost.
        let mut embed_elapsed = std::time::Duration::ZERO;
        let mut insert_elapsed = std::time::Duration::ZERO;
        let mut batch_num: u64 = 0;

        let reindex = reindex_root.is_some();
        let resolves = self.resolves_references();
        // (path, hash, size_bytes, mtime_secs)
        let mut processed_paths: Vec<(String, String, u64, u64)> = Vec::new();
        let mut stored: Vec<EntityId> = Vec::new();

        let concurrency = if self.config.max_concurrency > 0 {
            self.config.max_concurrency
        } else {
            // Cap default concurrency to avoid excessive memory usage from
            // concurrently held ParsedFile instances (tree + content).
            num_cpus::get().max(2)
        };
        let parser = self.parser.clone();
        let db = Arc::clone(&self.db);
        let batch_size = self.config.file_batch_size;
        let memory_guard = if self.config.memory_guard_mb > 0 {
            MemoryGuard::new(self.config.memory_guard_mb)
        } else {
            MemoryGuard::new_adaptive()
        };

        // Accumulate embedding inputs inline so we never hold all entities
        // in memory at once.
        let mut embed_texts: Vec<String> = Vec::new();
        let mut embed_ids: Vec<crate::entities::EntityId> = Vec::new();
        // Which entities are worth a semantic vector (applied per file below).
        // Non-selected entities stay findable via lexical/FTS search.
        let embedding_selection = self.scope.read().embedding_selection;
        if self.embedding_service.is_some() {
            info!("Embedding selection: {}", embedding_selection.as_str());
        }

        let stream = futures::stream::iter(entries)
            .map(move |entry: FileEntry| {
                let parser = parser.clone();
                let entry_path = entry.path.clone();
                async move {
                    match parser.parse_file(&entry_path).await {
                        Ok(parsed) => Ok((entry, parsed)),
                        Err(e) => Err((entry, e)),
                    }
                }
            })
            .buffer_unordered(concurrency);

        futures::pin_mut!(stream);

        let mut batch: Vec<FileRecord> = Vec::new();

        while let Some(res) = stream.next().await {
            // Back-pressure: if memory is low, yield to the async runtime
            // rather than blocking the thread with std::thread::sleep.
            if let Some(delay) = memory_guard.throttle_delay(500) {
                tokio::time::sleep(delay).await;
            }

            match res {
                Ok((entry, mut parsed)) => {
                    result.files_parsed += 1;
                    let entity_count = parsed.entities.len();
                    result.entity_count += entity_count;

                    // Periodic progress log so multi-hour runs on large corpora
                    // (e.g. OpenJDK) are observable instead of silent for hours.
                    if result.files_parsed.is_multiple_of(5000) {
                        let secs = phase2_start.elapsed().as_secs_f64();
                        let rate = if secs > 0.0 {
                            result.files_parsed as f64 / secs
                        } else {
                            0.0
                        };
                        info!(
                            "Phase 2 progress: {} files, {} entities, {} embeddings ({:.0} files/s, {:.1}s elapsed)",
                            result.files_parsed,
                            result.entity_count,
                            result.embedding_count,
                            rate,
                            secs,
                        );
                    }

                    let lang = parsed.file_info.language.clone();
                    *result.entities_by_language.entry(lang).or_insert(0) += entity_count;

                    for entity in &parsed.entities {
                        let type_name = entity.entity_type().to_string();
                        *result.entity_type_counts.entry(type_name).or_insert(0) += 1;
                    }

                    // Track successfully parsed files for manifest seeding.
                    // With pre-computed file hashes (from disk) use those;
                    // otherwise hash the content we already have in memory so
                    // we avoid a redundant disk read.  Both paths produce the
                    // same XXH3-64 hex for UTF-8 source files.
                    {
                        let path_str = entry.path.to_string_lossy().to_string();
                        let hash = match hashes {
                            Some(hashes) => hashes.get(&entry.path).cloned(),
                            None => {
                                let h = xxhash_rust::xxh3::xxh3_64(parsed.content.as_bytes());
                                Some(format!("{h:016x}"))
                            }
                        };
                        if let Some(h) = hash {
                            processed_paths.push((path_str, h, entry.size, entry.modified));
                        }
                    }

                    // Drop the AST tree and full file content now that all extraction
                    // (entities, relationships, imports) is complete, to bound memory.
                    parsed.tree = None;
                    parsed.content = String::new();

                    // Accumulate embedding inputs for this file's entities.
                    // CRITICAL: Embeddings are flushed AFTER the batch is
                    // persisted to DB to prevent orphaned vectors in
                    // entity_vectors when the DB transaction rolls back due to
                    // FK violations or other errors.
                    if self.embedding_service.is_some() {
                        for entity in &parsed.entities {
                            // Selective embedding: only enqueue entities worth a
                            // semantic vector. Skipped entities stay searchable
                            // via name/FTS (search falls back to keyword).
                            if crate::embeddings::should_embed(entity, embedding_selection) {
                                embed_texts.push(entity_to_text(entity));
                                embed_ids.push(entity.id());
                            }
                        }
                    }

                    let mut record = FileRecord::from(parsed);
                    if resolves {
                        stored.extend(record.entities.iter().map(Entity::id));
                        record
                            .relationships
                            .retain(|relationship| relationship.relationship_type.is_nesting());
                    }
                    batch.push(record);

                    if batch.len() >= batch_size {
                        batch_num += 1;
                        let batch_files = batch.len();
                        let batch_entities: usize =
                            batch.iter().map(|record| record.entities.len()).sum();

                        // STEP 1: Persist files/entities/relationships first.
                        let t_ins = std::time::Instant::now();
                        result.relationship_count +=
                            db.batch_insert_parsed_files(&batch, reindex_root).await?;
                        let ins = t_ins.elapsed();
                        insert_elapsed += ins;

                        // STEP 1b: on a re-index, reconcile these re-parsed files —
                        // drop entities (and their vectors) that vanished from
                        // each. Stable ids mean only genuinely removed entities are
                        // touched; unchanged ones keep their vectors.
                        if reindex {
                            let recon: Vec<(String, Vec<crate::entities::EntityId>)> = batch
                                .iter()
                                .map(|record| {
                                    (
                                        record.file_info.path.clone(),
                                        record.entities.iter().map(Entity::id).collect(),
                                    )
                                })
                                .collect();
                            db.reconcile_file_entities(&recon).await?;
                        }

                        // STEP 2: Only NOW generate & store embeddings for
                        // entities that are confirmed persisted.
                        let mut emb = std::time::Duration::ZERO;
                        if let Some(ref service) = self.embedding_service {
                            let t = std::time::Instant::now();
                            let (stored, hits) = self
                                .flush_embedding_batch(service, &mut embed_texts, &mut embed_ids)
                                .await?;
                            emb = t.elapsed();
                            embed_elapsed += emb;
                            result.embedding_count += stored;
                            result.embedding_cache_hits += hits;
                        }

                        // Per-batch breakdown every 20 batches (≈1000 files) so
                        // the parse/insert/embed split is visible during long runs
                        // without flooding the log.
                        if batch_num.is_multiple_of(20) {
                            info!(
                                "Batch {batch_num}: {batch_files} files / {batch_entities} entities | insert {}ms, embed {}ms | totals: {} files, {} entities, {} embeddings, {:.0}s elapsed",
                                ins.as_millis(),
                                emb.as_millis(),
                                result.files_parsed,
                                result.entity_count,
                                result.embedding_count,
                                phase2_start.elapsed().as_secs_f64(),
                            );
                        }
                        batch.clear();
                    }
                }
                Err((entry, e)) => {
                    result.files_failed += 1;
                    let msg = format!("{}: {}", entry.path.display(), e);
                    if result.errors.len() < 100 {
                        result.errors.push(msg.clone());
                    }
                    warn!("{}", msg);
                }
            }
        }

        // Flush any remaining parsed files in a final transaction FIRST.
        if !batch.is_empty() {
            result.relationship_count += db.batch_insert_parsed_files(&batch, reindex_root).await?;
            if reindex {
                let recon: Vec<(String, Vec<crate::entities::EntityId>)> = batch
                    .iter()
                    .map(|record| {
                        (
                            record.file_info.path.clone(),
                            record.entities.iter().map(Entity::id).collect(),
                        )
                    })
                    .collect();
                db.reconcile_file_entities(&recon).await?;
            }
        }

        // Only NOW flush any remaining embeddings for the persisted entities.
        if !embed_texts.is_empty()
            && let Some(ref service) = self.embedding_service
        {
            let t = std::time::Instant::now();
            let (stored, hits) = self
                .flush_embedding_batch(service, &mut embed_texts, &mut embed_ids)
                .await?;
            embed_elapsed += t.elapsed();
            result.embedding_count += stored;
            result.embedding_cache_hits += hits;
        }

        info!(
            "Phase 2 complete in {:.1}s: {} files, {} entities | DB insert {:.1}s, embedding {:.1}s for {} stored (remainder ≈ parse/extract)",
            phase2_start.elapsed().as_secs_f64(),
            result.files_parsed,
            result.entity_count,
            insert_elapsed.as_secs_f64(),
            embed_elapsed.as_secs_f64(),
            result.embedding_count,
        );

        Ok(ParseOutput {
            processed: processed_paths,
            stored,
        })
    }

    /// Record `processed` (path, hash, size, mtime) in the scan manifest: the
    /// baseline later incremental runs compare files against.
    async fn update_manifest(
        &self,
        processed: &[(String, String, u64, u64)],
        incremental: bool,
    ) -> Result<()> {
        // Always seed the manifest, incremental or not, so that the first
        // incremental re-scan has a baseline rather than re-parsing every file.
        if processed.is_empty() {
            return Ok(());
        }
        // All upserts in a single transaction.
        let now = OffsetDateTime::now_utc().to_string();
        let manifest_entries: Vec<(String, String, String, u64, u64)> = processed
            .iter()
            .map(|(path, hash, size, mtime)| {
                (path.clone(), hash.clone(), now.clone(), *size, *mtime)
            })
            .collect();
        self.db.batch_upsert_scan_manifest(manifest_entries).await?;
        if incremental {
            info!(
                "Incremental: updated manifest for {} files",
                processed.len()
            );
        } else {
            info!("Seeded manifest for {} files", processed.len());
        }
        Ok(())
    }

    /// Delete what the index holds for `removed`, manifest paths whose files
    /// are gone: entities, the edges touching them, vectors and `files` rows,
    /// and their manifest rows.
    async fn remove_files(&self, removed: &[String]) -> Result<()> {
        if removed.is_empty() {
            return Ok(());
        }
        let (entities_deleted, rels_deleted) =
            self.db.delete_entities_by_file_paths(removed).await?;
        self.db.delete_scan_manifest_entries(removed).await?;
        info!(
            "Re-index: cleaned up {} removed files ({} entities, {} relationships)",
            removed.len(),
            entities_deleted,
            rels_deleted
        );
        Ok(())
    }

    /// Whether references resolve across files: cross-file analysis is on
    /// and its mode is not [`CrossFileMode::None`]. Otherwise each file keeps
    /// only the edges within it.
    fn resolves_references(&self) -> bool {
        self.config.enable_cross_file && self.scope.read().cross_file_mode != CrossFileMode::None
    }

    /// The definitions in `files` before a run writes them
    /// ([`Database::get_definitions_in_files`]), for [`Self::relink`] to tell
    /// which names the run changed. `None` when references do not resolve
    /// across files, or when nothing is indexed under `root` yet: every entity
    /// there after the run is then one it stored, so no reference elsewhere
    /// needs resolving again.
    async fn definitions_before(
        &self,
        root: &Path,
        files: &[String],
    ) -> Result<Option<Vec<(String, EntityId)>>> {
        if !self.resolves_references() || self.db.count_entities_under(root).await? == 0 {
            return Ok(None);
        }
        Ok(Some(self.db.get_definitions_in_files(files).await?))
    }

    /// Phase 3 of a run over `files` (parsed or removed, under `root`): derive
    /// from the stored references every edge the run may have changed. Those
    /// are the edges of the entities it stored (`stored`, in the order they
    /// were parsed), and of every other entity under `root` with a reference
    /// to a name whose definitions in `files` the run changed (`before`, from
    /// [`Self::definitions_before`]). The files of the latter join the run in
    /// `reindexing`. Without `before`, every source is new: its edges are only
    /// inserted ([`derive_edges_for_new_sources`]).
    async fn relink(
        &self,
        root: &Path,
        files: &[String],
        before: Option<Vec<(String, EntityId)>>,
        mut sources: Vec<EntityId>,
        reindexing: &mut ReindexGuard<'_>,
        result: &mut PipelineResult,
    ) -> Result<()> {
        info!("Phase 3: Deriving relationships from references...");
        let started = std::time::Instant::now();
        let parsed = sources.len();
        // Nothing was indexed under the root before: every source is new.
        let fresh = before.is_none();
        let changed = match before {
            Some(before) => {
                let after = self.db.get_definitions_in_files(files).await?;
                changed_names(before, &after)
            }
            None => Vec::new(),
        };
        if !changed.is_empty() {
            // A file that failed to parse keeps its entities, so its
            // references count as made elsewhere.
            let mut seen: std::collections::HashSet<EntityId> = sources.iter().copied().collect();
            let mut other_files = Vec::new();
            for (source, file) in self.db.get_sources_referring_to(root, &changed).await? {
                if seen.insert(source) {
                    sources.push(source);
                    other_files.push(file);
                }
            }
            other_files.sort_unstable();
            other_files.dedup();
            reindexing.extend(other_files);
        }

        let entities = self.db.count_entities_under(root).await?;
        let resolution = Resolution {
            root,
            cross_file_calls: self.scope.read().cross_file_mode.effective(entities)
                == CrossFileMode::Deep,
            fanout: self.config.ambiguous_fanout,
        };
        let (inserted, cross_file) = if fresh {
            derive_edges_for_new_sources(&self.db, &sources, resolution).await?
        } else {
            rebuild_edges_for_sources(&self.db, &sources, resolution).await?
        };
        result.relationship_count += inserted;
        result.cross_file_relationship_count += cross_file;
        info!(
            "Phase 3 complete in {:.1}s: {inserted} relationships stored ({cross_file} across files) \
             from the references of {} entities ({} not parsed in this run, referring to {} \
             changed names)",
            started.elapsed().as_secs_f64(),
            sources.len(),
            sources.len() - parsed,
            changed.len(),
        );
        Ok(())
    }

    /// Flush accumulated embedding texts and entity IDs to the database.
    ///
    /// 1. Hashes every text with XXH3 and checks the `embedding_cache` table.
    ///    Cache hits are written directly to `entity_vectors`, skipping inference.
    /// 2. Cache misses are chunked by `embedding_batch_size` and fanned out
    ///    concurrently on the shared tract plan via `buffer_unordered`.
    /// 3. Fresh embeddings are stored in both `entity_vectors` and the cache.
    async fn flush_embedding_batch(
        &self,
        service: &Arc<EmbeddingService>,
        texts: &mut Vec<String>,
        ids: &mut Vec<crate::entities::EntityId>,
    ) -> Result<(usize, usize)> {
        use crate::database::vector;
        use xxhash_rust::xxh3::xxh3_64;

        if texts.is_empty() {
            return Ok((0, 0));
        }

        let all_texts = std::mem::take(texts);
        let all_ids = std::mem::take(ids);
        let db_pool = self.db.conn_pool().clone();

        // --- Step 1: cache lookup ---
        let text_hashes: Vec<String> = all_texts
            .iter()
            .map(|t| format!("{:016x}", xxh3_64(t.as_bytes())))
            .collect();

        let cached = vector::lookup_embedding_cache(&db_pool, &text_hashes).await?;

        let mut total_stored = 0usize;
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_ids: Vec<crate::entities::EntityId> = Vec::new();
        let mut miss_hashes: Vec<String> = Vec::new();

        // Write cache hits directly; collect cache misses for ONNX.
        let mut hit_items: Vec<(crate::entities::EntityId, Vec<f32>)> = Vec::new();
        for ((text_hash, id), text) in text_hashes
            .into_iter()
            .zip(all_ids.iter().copied())
            .zip(all_texts.iter())
        {
            if let Some(vec) = cached.get(&text_hash) {
                hit_items.push((id, vec.clone()));
            } else {
                miss_texts.push(text.clone());
                miss_ids.push(id);
                miss_hashes.push(text_hash);
            }
        }

        let cache_hits = hit_items.len();
        if !hit_items.is_empty() {
            match vector::upsert_embeddings_batch(&db_pool, &hit_items).await {
                Ok(stored) => total_stored += stored,
                Err(e) => warn!("Failed to store cached embeddings: {e}"),
            }
            tracing::debug!(
                "Embedding cache: {cache_hits} hits, {} misses",
                miss_texts.len()
            );
        }

        if miss_texts.is_empty() {
            return Ok((total_stored, cache_hits));
        }

        // Chunk texts of similar length together, since a batch is padded to
        // its longest text (see `TractEmbedder::embed`); the longest first,
        // since the chunks run several at a time and the longest, started
        // last, would run on alone.
        let mut order: Vec<usize> = (0..miss_texts.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(miss_texts[i].len()));
        let miss_ids: Vec<crate::entities::EntityId> = order.iter().map(|&i| miss_ids[i]).collect();
        let miss_hashes: Vec<String> = order
            .iter()
            .map(|&i| std::mem::take(&mut miss_hashes[i]))
            .collect();
        let miss_texts: Vec<String> = order
            .iter()
            .map(|&i| std::mem::take(&mut miss_texts[i]))
            .collect();

        // --- Step 2: ONNX inference for cache misses ---
        let chunk_size = self.config.embedding_batch_size.max(1);
        let total_chunks = miss_texts.len().div_ceil(chunk_size);

        // The fan-out width is the provider's actual concurrency (RAM- and
        // core-aware), so every concurrency slot is busy at once.
        let session_count = service.session_count().max(1);

        type ChunkResult = Result<(usize, Vec<(String, Vec<f32>)>)>;
        let results: Vec<ChunkResult> = futures::stream::iter(
            miss_texts
                .chunks(chunk_size)
                .zip(miss_ids.chunks(chunk_size))
                .zip(miss_hashes.chunks(chunk_size))
                .enumerate()
                .map(|(chunk_idx, ((text_chunk, id_chunk), hash_chunk))| {
                    let service_clone = Arc::clone(service);
                    let texts_for_embedding = text_chunk.to_vec();
                    let id_vec: Vec<crate::entities::EntityId> = id_chunk.to_vec();
                    let hash_vec: Vec<String> = hash_chunk.to_vec();
                    let pool = db_pool.clone();

                    async move {
                        let embeddings = tokio::task::spawn_blocking(move || {
                            service_clone.generate_embeddings_batch(&texts_for_embedding)
                        })
                        .await
                        .map_err(|e| {
                            crate::error::Error::Embedding(format!(
                                "Join error on chunk {chunk_idx}: {e}"
                            ))
                        })??;

                        let items: Vec<(crate::entities::EntityId, Vec<f32>)> =
                            id_vec.into_iter().zip(embeddings.clone()).collect();

                        let stored = vector::upsert_embeddings_batch(&pool, &items)
                            .await
                            .map_err(|e| {
                                warn!(
                                    "Failed to store embedding chunk {chunk_idx}/{total_chunks}: {e}"
                                );
                                e
                            })?;

                        // Pair hashes with their freshly computed vectors for cache storage.
                        let cache_pairs: Vec<(String, Vec<f32>)> =
                            hash_vec.into_iter().zip(embeddings).collect();

                        Ok((stored, cache_pairs))
                    }
                })
                .collect::<Vec<_>>(),
        )
        .buffer_unordered(session_count)
        .collect()
        .await;

        let mut all_cache_pairs: Vec<(String, Vec<f32>)> = Vec::new();
        for result in results {
            match result {
                Ok((n, pairs)) => {
                    total_stored += n;
                    all_cache_pairs.extend(pairs);
                }
                Err(e) => warn!("Embedding chunk error: {e}"),
            }
        }

        // --- Step 3: populate the cache with fresh vectors ---
        if !all_cache_pairs.is_empty()
            && let Err(e) = vector::store_embedding_cache(&db_pool, &all_cache_pairs).await
        {
            warn!("Failed to update embedding cache: {e}");
        }

        Ok((total_stored, cache_hits))
    }

    /// The scanner this pipeline's scans use: its filter (extensions, exclude
    /// patterns, size limit) and ignore rules, git's only with `use_gitignore`.
    #[must_use]
    pub fn scanner(&self) -> FileScanner {
        let mut scanner = FileScanner::new(self.build_file_filter());
        scanner.set_respect_gitignore(self.scope.read().use_gitignore);
        scanner
    }

    /// The pipeline's scope: which files its runs take in, how references
    /// resolve, which entities it embeds.
    #[must_use]
    pub fn scope(&self) -> ScanScope {
        self.scope.read().clone()
    }

    /// Change the pipeline's scope, as when a project's configuration file
    /// changes under a long-lived server. Waits for the run in progress, so
    /// that no run mixes two scopes; the next one takes the new scope.
    pub async fn set_scope(&self, scope: ScanScope) {
        let _run = self.runs.lock().await;
        *self.scope.write() = scope;
    }

    /// Build a file filter from config
    ///
    /// `include_extensions` is a **whitelist**: when it is non-empty the filter's
    /// default set (every supported language) is replaced, so only the requested
    /// extensions are scanned. `FileFilter::add_extension` only *inserts* into
    /// that default set, so appending without clearing first would silently scan
    /// everything — the documented `--include` / `extensions` behaviour would be
    /// a no-op. An empty list keeps the defaults ("all supported languages").
    fn build_file_filter(&self) -> FileFilter {
        let scope = self.scope.read();
        let mut filter = FileFilter::new();
        if scope.max_file_size > 0 {
            filter.max_file_size = scope.max_file_size;
        }
        if !scope.include_extensions.is_empty() {
            filter.include_extensions.clear();
            for ext in &scope.include_extensions {
                filter.add_extension(ext.trim_start_matches('.'));
            }
        }
        for pattern in &scope.exclude_patterns {
            filter.add_exclude(pattern);
        }
        filter
    }

    /// Get a reference to the database
    #[must_use]
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get a reference to the embedding service, if configured.
    #[must_use]
    pub fn embedding_service(&self) -> Option<&Arc<EmbeddingService>> {
        self.embedding_service.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert!(config.enable_cross_file);
        assert_eq!(config.file_batch_size, 50);
        assert_eq!(config.embedding_batch_size, 16);
        assert!(matches!(config.cross_file_mode, CrossFileMode::Auto));
        // 0 means "use adaptive threshold based on total RAM"
        assert_eq!(config.memory_guard_mb, 0);
    }

    #[test]
    fn test_pipeline_result_new() {
        let result = PipelineResult::new();
        assert_eq!(result.files_discovered, 0);
        assert_eq!(result.entity_count, 0);
        assert!(result.errors.is_empty());
    }

    /// `devscriptor scan .` and the MCP `scan_codebase` tool name the same
    /// directory two different ways; stored under two `file_path` strings, the
    /// same file would never match across them in the re-scan
    /// `DELETE ... WHERE file_path IN (...)`, a literal string comparison, and
    /// one `scan_codebase` call on an already-scanned project would double the
    /// index.
    #[test]
    fn a_relative_and_an_absolute_root_resolve_to_the_same_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(absolute).unwrap();
        let from_dot = canonical_root(Path::new("."));
        let from_absolute = canonical_root(absolute);
        std::env::set_current_dir(previous).unwrap();

        assert_eq!(
            from_dot, from_absolute,
            "two spellings of one directory must produce one root"
        );
        assert!(from_dot.is_absolute(), "{from_dot:?}");
    }

    /// A root that does not exist must still resolve stably rather than being
    /// left as typed.
    #[test]
    fn an_unresolvable_root_is_still_made_absolute() {
        let root = canonical_root(Path::new("no/such/dir"));
        assert!(root.is_absolute(), "{root:?}");
        assert!(root.ends_with("no/such/dir"), "{root:?}");
    }

    #[test]
    fn an_absolute_unresolvable_root_is_left_alone() {
        // Absolute on this platform: on Windows a path needs a drive.
        let missing = if cfg!(windows) {
            Path::new(r"C:\no\such\dir")
        } else {
            Path::new("/no/such/dir")
        };
        assert_eq!(canonical_root(missing), missing);
    }
}
