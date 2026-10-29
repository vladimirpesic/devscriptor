//! SQLite database layer with vector search support

pub mod cache;
pub mod connection;
pub mod embeddings;
pub mod fts_query;
pub mod operations;
mod reindex_log;
pub mod schema;
pub mod vector;

use crate::entities::{Entity, EntityId, FileInfo, Relationship, SymbolRef};
use crate::error::{Error, Result};
use crate::parser::ImportInfo;

/// Compute a SQLite cache size based on total system RAM.
///
/// Returns a negative value suitable for `PRAGMA cache_size` (negative = KiB).
/// From the total RAM, as `utils::total_ram_mb()` reports it.
///
/// | Total RAM | Cache Size |
/// |-----------|------------|
/// | < 4 GB    | 5 MB       |
/// | 4–8 GB    | 10 MB      |
/// | 8–16 GB   | 20 MB      |
/// | > 16 GB   | 50 MB      |
pub fn adaptive_cache_size() -> i32 {
    let total_mb = crate::utils::total_ram_mb() as i32;

    let cache_kb = match total_mb {
        0..=4096 => 5 * 1024,
        4097..=8192 => 10 * 1024,
        8193..=16384 => 20 * 1024,
        _ => 50 * 1024,
    };

    -cache_kb
}

/// What the index stores of one parsed file: its `files` row, its entities,
/// the relationships among them, the references they make and the file's
/// imports ([`Database::batch_insert_parsed_files`]).
#[derive(Debug, Clone)]
pub struct FileRecord {
    /// The file's `files` row.
    pub file_info: FileInfo,
    /// The entities defined in the file.
    pub entities: Vec<Entity>,
    /// Relationships between the file's own entities.
    pub relationships: Vec<Relationship>,
    /// The references its entities make, resolved or not.
    pub refs: Vec<SymbolRef>,
    /// The file's imports.
    pub imports: Vec<ImportInfo>,
}

/// A stored reference, with the file it was made in and that file's
/// language: what resolving it needs ([`Database::get_symbol_refs_for_sources`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRef {
    /// The reference.
    pub reference: SymbolRef,
    /// The file it was made in, as stored.
    pub file_path: String,
    /// The language of that file.
    pub language: String,
}

/// An entity's identity without its data: what a relationship's endpoint is
/// named by ([`Database::get_entity_names_by_ids`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityHeader {
    /// The entity's id.
    pub id: EntityId,
    /// Its name.
    pub name: String,
    /// Its type (`function`, `class`, …).
    pub entity_type: String,
    /// The file it is defined in, as stored.
    pub file_path: String,
    /// The line it starts on, counted from 1.
    pub start_line: u32,
}

/// Database configuration.
///
/// There is no vector dimension to configure: `entity_vectors` is always
/// `FLOAT[768]` (see docs/decisions.md).
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Database file path
    pub path: String,
    /// Enable WAL mode
    pub enable_wal: bool,
    /// Synchronous mode: NORMAL, FULL, or OFF
    pub synchronous: String,
    /// Cache size in pages (negative = KB)
    pub cache_size: i32,
    /// Enable vector extension
    pub enable_vector: bool,
    /// Query timeout in seconds (0 = no timeout)
    pub query_timeout_seconds: u64,
    /// Connections in the pool: a project's `database.pool_size`. The
    /// environment's `DEVSCRIPTOR_DB_POOL_SIZE` outranks it; without either,
    /// twice the CPU count, from 16 to 64.
    pub pool_size: Option<u32>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: "devscriptor.db".to_string(),
            enable_wal: true,
            synchronous: "NORMAL".to_string(),
            cache_size: adaptive_cache_size(),
            enable_vector: true,
            query_timeout_seconds: 30,
            pool_size: None,
        }
    }
}

/// Main database interface
pub struct Database {
    pool: connection::ConnectionPool,
    config: DatabaseConfig,
    /// The files this process re-indexed, and when ([`Self::reindexed_since`]).
    reindex_log: parking_lot::Mutex<reindex_log::ReindexLog>,
}

/// A re-index run in progress over a set of files: dropped after the run's
/// last write, whether it finished, failed or was cancelled, it records the
/// run for [`Database::reindexed_since`] ([`Database::reindexing`]).
#[must_use = "the run is recorded when the guard is dropped"]
pub struct ReindexGuard<'a> {
    database: &'a Database,
    paths: Vec<String>,
}

impl ReindexGuard<'_> {
    /// Count `paths` as files of the run too: files whose edges it rewrites
    /// though it did not re-parse them.
    pub fn extend(&mut self, paths: impl IntoIterator<Item = String>) {
        self.paths.extend(paths);
    }
}

impl Drop for ReindexGuard<'_> {
    fn drop(&mut self) {
        self.database
            .reindex_log
            .lock()
            .record(std::mem::take(&mut self.paths));
    }
}

impl Database {
    /// Create or open a database
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or initialized
    pub fn new(config: DatabaseConfig) -> Result<Self> {
        // The environment, else the project's setting, else adaptive: twice
        // the CPU count, at least 16 and at most 64. Larger pools allow more
        // concurrent spawn_blocking DB tasks.
        let pool_size = std::env::var("DEVSCRIPTOR_DB_POOL_SIZE")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&size| size > 0)
            .or(config.pool_size.filter(|&size| size > 0))
            .unwrap_or_else(|| {
                let by_cpu = (num_cpus::get() as u32 * 2).max(16);
                by_cpu.min(64) // hard cap at 64 to avoid descriptor exhaustion
            });
        let pool = connection::ConnectionPool::new(&config.path, pool_size)?;

        let db = Self {
            pool,
            config,
            reindex_log: parking_lot::Mutex::default(),
        };
        db.initialize()?;

        Ok(db)
    }

    /// Initialize database schema and extensions
    fn initialize(&self) -> Result<()> {
        let conn = self.pool.get()?;

        // Every per-connection PRAGMA (synchronous, cache_size, temp_store,
        // busy_timeout, foreign_keys) is set in `ConnectionPool::new`'s
        // `with_init`, so it applies to every pooled connection;
        // journal_mode=WAL, a property of the file, is set there once.

        // An index of another schema version is dropped here and made again,
        // empty, below.
        if schema::drop_index_of_other_version(&conn)? {
            tracing::warn!(
                "{}: the index was written by another version of Devscriptor and \
                 was emptied; scan the project again (Context Graph notes are kept)",
                self.config.path
            );
        }

        if self.config.enable_vector {
            vector::initialize(&conn)?;
        }

        schema::create_tables(&conn)?;
        schema::record_schema_version(&conn)?;
        let dropped = schema::align_embedding_space(&conn, crate::embeddings::EMBEDDING_SPACE)?;
        if dropped > 0 {
            tracing::warn!(
                "{dropped} vectors of another embedding model or arithmetic were dropped; \
                 `devscriptor embed` makes them again"
            );
        }

        crate::context_graph::initialize_context_tables_on_conn(&conn)?;

        Ok(())
    }

    /// How many re-index runs this process has finished. Taken before an
    /// answer is read from the index, it tells [`Self::reindexed_since`]
    /// which re-indexes the answer may have missed.
    #[must_use]
    pub fn index_epoch(&self) -> u64 {
        self.reindex_log.lock().epoch()
    }

    /// Of `paths`, the files that a re-index run of this process finished
    /// after `epoch` ([`Self::index_epoch`]) covered: an answer read from the
    /// index since `epoch` may hold their older entities. All of `paths` when
    /// the record no longer reaches back to `epoch`. Runs of other processes
    /// are not seen.
    #[must_use]
    pub fn reindexed_since(&self, epoch: u64, paths: &[String]) -> Vec<String> {
        self.reindex_log.lock().since(epoch, paths)
    }

    /// Start a re-index run over `paths`, before its first write. The run
    /// counts as finished for [`Self::reindexed_since`] when the returned
    /// guard is dropped, after its last write.
    pub fn reindexing(&self, paths: Vec<String>) -> ReindexGuard<'_> {
        ReindexGuard {
            database: self,
            paths,
        }
    }

    /// Return the database file path configured at construction.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.config.path
    }

    /// Insert or update a file
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn upsert_file(&self, file_info: &FileInfo) -> Result<()> {
        operations::upsert_file(&self.pool, file_info).await
    }

    /// Insert entities
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn insert_entities(&self, entities: &[Entity]) -> Result<()> {
        operations::insert_entities(&self.pool, entities).await
    }

    /// Insert `relationships` in one transaction, skipping any already stored
    /// (same source, target and kind) and any whose source or target is not
    /// stored. Returns, for each, whether it was inserted. The edges derived
    /// for entities just stored replace nothing, and are inserted this way.
    ///
    /// # Errors
    /// Returns an error if the database write fails; nothing is changed then.
    pub async fn insert_relationships(
        &self,
        relationships: Vec<Relationship>,
    ) -> Result<Vec<bool>> {
        operations::insert_relationships(&self.pool, relationships).await
    }

    /// Batch-insert parsed files in a single transaction: each file's row,
    /// entities, relationships, references and imports. A file's references
    /// and imports replace the ones stored for it.
    ///
    /// `reindex_root` is the scan root when the files are being re-indexed into
    /// a database that already holds them: each file's previously derived edges
    /// are then replaced rather than added to. `None` for a fresh index. See
    /// [`operations::batch_insert_parsed_files`].
    ///
    /// Returns the number of relationship rows inserted; an edge already
    /// stored is not counted.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn batch_insert_parsed_files(
        &self,
        batch: &[FileRecord],
        reindex_root: Option<&std::path::Path>,
    ) -> Result<usize> {
        operations::batch_insert_parsed_files(&self.pool, batch, reindex_root).await
    }

    /// Search entities by name (fuzzy)
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_entities(&self, query: &str, limit: usize) -> Result<Vec<Entity>> {
        operations::search_entities(&self.pool, query, limit).await
    }

    /// Search entities by exact name match
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_entities_by_exact_name(
        &self,
        name: &str,
        limit: usize,
    ) -> Result<Vec<Entity>> {
        operations::get_entities_by_exact_name(&self.pool, name, limit).await
    }

    /// Vector similarity search.
    ///
    /// Use [`Self::vector_search_filtered`] to restrict results to a specific
    /// entity type, which avoids a full linear scan at large scales.
    ///
    /// # Errors
    /// Returns an error if vector search is disabled or the database operation fails
    pub async fn vector_search(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(Entity, f32)>> {
        self.vector_search_filtered(embedding, limit, None).await
    }

    /// Vector similarity search with an optional entity-type pre-filter.
    ///
    /// # Errors
    /// Returns an error if vector search is disabled or the database operation fails
    pub async fn vector_search_filtered(
        &self,
        embedding: &[f32],
        limit: usize,
        entity_type: Option<&str>,
    ) -> Result<Vec<(Entity, f32)>> {
        if !self.config.enable_vector {
            return Err(Error::VectorDb("Vector search disabled".to_string()));
        }
        vector::search_similar_filtered(&self.pool, embedding, limit, entity_type).await
    }

    /// Full-text search using FTS5
    ///
    /// # Arguments
    /// * `query` - Search query string
    /// * `limit` - Maximum number of results
    ///
    /// # Errors
    /// Returns an error if the database operation fails or times out
    pub async fn full_text_search(&self, query: &str, limit: usize) -> Result<Vec<Entity>> {
        // The query as FTS5 must see it: see `fts_query`.
        let Some(query) = fts_query::match_query(query) else {
            return Ok(Vec::new());
        };
        let pool = self.pool.clone();
        let timeout_seconds = self.config.query_timeout_seconds;

        let task = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            // One JOIN query that keeps BM25's rank order: the entities are joined to
            // the ranked FTS rows and ordered by their rank.
            let mut stmt = conn.prepare(
                "SELECT e.type, e.data
                 FROM entities e
                 INNER JOIN (
                     SELECT rowid, rank
                     FROM entities_fts
                     WHERE entities_fts MATCH ?1
                     ORDER BY rank
                     LIMIT ?2
                 ) fts ON e.rowid = fts.rowid
                 ORDER BY fts.rank",
            )?;

            let entities = stmt
                .query_map(
                    rusqlite::params![query, limit as i64],
                    operations::row_to_entity,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(entities)
        });

        // Apply timeout if configured
        if timeout_seconds > 0 {
            match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_seconds), task)
                .await
            {
                Ok(result) => result.map_err(|e| Error::task_join("full-text search", e))?,
                Err(_) => Err(Error::timeout(timeout_seconds, "full-text search")),
            }
        } else {
            task.await
                .map_err(|e| Error::task_join("full-text search", e))?
        }
    }

    /// Load the full scan_manifest table.
    pub async fn load_scan_manifest(&self) -> Result<Vec<operations::ScanManifestEntry>> {
        operations::load_scan_manifest(&self.pool).await
    }

    /// Upsert a single scan_manifest row.
    pub async fn upsert_scan_manifest(
        &self,
        path: &str,
        content_hash: &str,
        last_parsed_at: &str,
        size_bytes: u64,
        mtime_secs: u64,
    ) -> Result<()> {
        operations::upsert_scan_manifest(
            &self.pool,
            path,
            content_hash,
            last_parsed_at,
            size_bytes,
            mtime_secs,
        )
        .await
    }

    /// Upsert multiple scan_manifest rows in one transaction.
    pub async fn batch_upsert_scan_manifest(
        &self,
        entries: Vec<(String, String, String, u64, u64)>,
    ) -> Result<()> {
        operations::batch_upsert_scan_manifest(&self.pool, entries).await
    }

    /// The manifest rows of `paths` (those it has): what the last scan saw of
    /// each file.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn get_scan_manifest_entries(
        &self,
        paths: &[String],
    ) -> Result<Vec<operations::ScanManifestEntry>> {
        operations::get_scan_manifest_entries(&self.pool, paths).await
    }

    /// The manifest paths inside directory `dir`.
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn scan_manifest_paths_under(&self, dir: &std::path::Path) -> Result<Vec<String>> {
        operations::scan_manifest_paths_under(&self.pool, dir).await
    }

    /// Record `root` as a directory scanned into this database
    /// (`metadata.indexed_roots`, a sorted JSON array without duplicates).
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn add_indexed_root(&self, root: &std::path::Path) -> Result<()> {
        operations::add_indexed_root(&self.pool, root).await
    }

    /// The references the entities `source_ids` make, in the order they were
    /// stored.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_symbol_refs_for_sources(
        &self,
        source_ids: &[EntityId],
    ) -> Result<Vec<StoredRef>> {
        operations::get_symbol_refs_for_sources(&self.pool, source_ids).await
    }

    /// The imports of each of `paths` that has any, as the parser extracted
    /// them, by path.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_file_imports(
        &self,
        paths: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<ImportInfo>>> {
        operations::get_file_imports(&self.pool, paths).await
    }

    /// Every entity under `root` named one of `names`, as a symbol-table
    /// entry: the candidates a reference to those names, made in the project
    /// at `root`, resolves among.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_symbol_entries_named(
        &self,
        names: &[String],
        root: &std::path::Path,
    ) -> Result<Vec<crate::parser::SymbolEntry>> {
        operations::get_symbol_entries_named(&self.pool, names, root).await
    }

    /// The type each of the entities `ids` is a member of (its
    /// `parent_class`), for those that are members: the type a `self` or
    /// `this` call made in one names.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_member_owners(&self, ids: &[EntityId]) -> Result<Vec<(EntityId, String)>> {
        operations::get_member_owners(&self.pool, ids).await
    }

    /// The name and id of every entity of the files `paths`, ordered by file,
    /// then as resolution orders definitions of one name in a file: by start
    /// line, then id.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_definitions_in_files(
        &self,
        paths: &[String],
    ) -> Result<Vec<(String, EntityId)>> {
        operations::get_definitions_in_files(&self.pool, paths).await
    }

    /// The entities under `root` with a reference that resolves by one of
    /// `names`, each with its file: a reference to the name, or to the alias
    /// an import of the file binds to it.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_sources_referring_to(
        &self,
        root: &std::path::Path,
        names: &[String],
    ) -> Result<Vec<(EntityId, String)>> {
        operations::get_sources_referring_to(&self.pool, root, names).await
    }

    /// Where under `root` a reference resolves by one of `names` — by the
    /// name, or by the alias an import of the file binds to it — or an
    /// import names one, as the member it imports or the name it binds, as
    /// `(file, 1-based line, language)`, sorted. What
    /// `get_references` adds to a language server's answer when the server
    /// could not see a file.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_reference_sites(
        &self,
        root: &std::path::Path,
        names: &[String],
    ) -> Result<Vec<(String, u32, String)>> {
        operations::get_reference_sites(&self.pool, root, names).await
    }

    /// Where under `root` a function calls one of `names` — by the name, or
    /// by the alias an import of the file binds to it — as `(file, 1-based
    /// line, language, caller)`, sorted: the caller is the entity the call is
    /// made in. What `get_incoming_calls` adds to a language server's answer
    /// when the server could not see a file.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_call_sites(
        &self,
        root: &std::path::Path,
        names: &[String],
    ) -> Result<Vec<(String, u32, String, String)>> {
        operations::get_call_sites(&self.pool, root, names).await
    }

    /// How many entities are stored under `root`.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn count_entities_under(&self, root: &std::path::Path) -> Result<usize> {
        operations::count_entities_under(&self.pool, root).await
    }

    /// Replace the edges derived from the references of `source_ids` with
    /// `edges`, in one transaction: their forward edges other than the
    /// nesting of members in types (`has_method`, `method_of`, which come from
    /// the file's structure), and the reverse rows whose target they are.
    /// Returns, for each of `edges`, whether it was inserted; one already
    /// stored, or whose ends are not, is skipped.
    ///
    /// # Errors
    /// Returns an error if the database write fails; nothing is changed then.
    pub async fn replace_edges_of_sources(
        &self,
        source_ids: &[EntityId],
        edges: Vec<Relationship>,
    ) -> Result<Vec<bool>> {
        operations::replace_edges_of_sources(&self.pool, source_ids, edges).await
    }

    /// The directories scanned into this database ([`Self::add_indexed_root`]).
    ///
    /// # Errors
    /// Returns an error if the database operation fails.
    pub async fn indexed_roots(&self) -> Result<Vec<std::path::PathBuf>> {
        let roots: Vec<String> = operations::get_metadata(&self.pool, "indexed_roots")
            .await?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Ok(roots.into_iter().map(std::path::PathBuf::from).collect())
    }

    /// Delete scan_manifest rows for removed files.
    pub async fn delete_scan_manifest_entries(&self, paths: &[String]) -> Result<usize> {
        operations::delete_scan_manifest_entries(&self.pool, paths).await
    }

    /// Delete everything indexed for files that no longer exist: entities,
    /// the edges touching them, their vectors and their `files` rows.
    ///
    /// Returns `(entities_deleted, relationships_deleted)`.
    pub async fn delete_entities_by_file_paths(&self, paths: &[String]) -> Result<(usize, usize)> {
        operations::delete_entities_by_file_paths(&self.pool, paths).await
    }

    /// Reconcile re-parsed files: drop entities (and their vectors) that vanished
    /// from each file, keeping the surviving ones — and their embeddings —
    /// intact. `files` is `(file_path, surviving_entity_ids)`. Returns the number
    /// of entities deleted. See [`operations::reconcile_file_entities`].
    pub async fn reconcile_file_entities(
        &self,
        files: &[(String, Vec<EntityId>)],
    ) -> Result<usize> {
        operations::reconcile_file_entities(&self.pool, files).await
    }

    /// Get entities by file path
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_entities_by_file(&self, file_path: &str) -> Result<Vec<Entity>> {
        operations::get_entities_by_file(&self.pool, file_path).await
    }

    /// Whether `file_path` is recorded in the `files` table, i.e. it was scanned
    /// into this graph.
    ///
    /// The file-reading MCP tools use this to decide whether a path outside the
    /// working directory is nonetheless safe to read: an indexed file's contents
    /// are already reachable through `search_entities` / `get_entity_details`, so
    /// reading it exposes nothing new.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn file_is_indexed(&self, file_path: &str) -> Result<bool> {
        operations::file_is_indexed(&self.pool, file_path).await
    }

    /// Get relationships for an entity
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_relationships(&self, entity_id: EntityId) -> Result<Vec<Relationship>> {
        operations::get_relationships(&self.pool, entity_id).await
    }

    /// Get relationships for multiple entities in a single batched query, so a
    /// graph traversal makes one query per level instead of one per node.
    pub async fn get_relationships_for_ids(
        &self,
        entity_ids: &[EntityId],
    ) -> Result<Vec<Relationship>> {
        let ids: Vec<String> = entity_ids.iter().map(|id| id.to_string()).collect();
        operations::get_relationships_for_ids(&self.pool, &ids).await
    }

    /// Persist vectors into the `entity_vectors` virtual table (batched).
    ///
    /// Thin wrapper around [`vector::upsert_embeddings_batch`] for callers
    /// that only have access to the `Database` handle.
    pub async fn upsert_embeddings_batch(&self, items: &[(EntityId, Vec<f32>)]) -> Result<usize> {
        vector::upsert_embeddings_batch(&self.pool, items).await
    }

    /// Store `(text_hash, vector)` pairs in the embedding cache.
    ///
    /// The hash must be `format!("{:016x}", xxh3_64(text.as_bytes()))`.
    pub async fn store_embedding_cache(&self, items: &[(String, Vec<f32>)]) -> Result<()> {
        vector::store_embedding_cache(&self.pool, items).await
    }

    /// The cached vectors for those of `text_hashes` the embedding cache holds,
    /// keyed by hash (hashed as for [`Self::store_embedding_cache`]).
    pub async fn lookup_embedding_cache(
        &self,
        text_hashes: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>> {
        vector::lookup_embedding_cache(&self.pool, text_hashes).await
    }

    /// Whether any entity has an embedding vector yet.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub async fn has_vectors(&self) -> Result<bool> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let any: Option<String> = conn
                .query_row("SELECT entity_id FROM entity_vectors LIMIT 1", [], |row| {
                    row.get(0)
                })
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            Ok(any.is_some())
        })
        .await
        .map_err(|e| Error::task_join("check for vectors", e))?
    }

    /// Fold the write-ahead log into the database file and empty it.
    ///
    /// SQLite folds the WAL back only when it grows past 1,000 pages or the
    /// last connection closes, and the pool's connections stay open until
    /// the process exits, so after a scan the index could sit almost entirely
    /// in `devscriptor.db-wal` beside a 4 KB `devscriptor.db`: copying or
    /// moving the `.db` file alone lost it. Call this when a command that
    /// wrote has finished. It waits for readers (`busy_timeout`), so a
    /// long-running server should not call it while serving requests.
    ///
    /// # Errors
    /// Returns an error if the checkpoint cannot run.
    pub async fn checkpoint(&self) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::task_join("checkpoint the database", e))?
    }

    /// Remove orphaned embedding vectors whose `entity_id` no longer exists
    /// in the `entities` table.
    ///
    /// Call this after pipeline runs to reclaim space and maintain data
    /// integrity between `entity_vectors` and `entities`.
    ///
    /// # Returns
    /// Number of orphaned vectors removed.
    pub async fn cleanup_orphan_vectors(&self) -> Result<usize> {
        vector::cleanup_orphan_vectors(&self.pool).await
    }

    /// Get the connection pool.
    #[must_use]
    pub fn conn_pool(&self) -> &connection::ConnectionPool {
        &self.pool
    }

    /// Return the IDs of the entities `selection` embeds that have no entry in
    /// `entity_vectors`, and how many entities there are in all.
    ///
    /// When `force = true`, returns every entity `selection` embeds (for a
    /// full re-embed).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_unembedded_entity_ids(
        &self,
        force: bool,
        selection: crate::embeddings::EmbeddingSelection,
    ) -> Result<(Vec<EntityId>, usize)> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let total: i64 = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;

            // Only the entities the embedding policy selects. The rest stay
            // reachable: the retriever fuses a BM25 channel over
            // `name`/`name_tokens`/`body` with the dense one, so an entity
            // without a vector is still returned by the keyword half.
            let selected = crate::embeddings::selection::sql_condition(selection);
            // By size, largest first. Callers embed the ids in batches, and a
            // batch is padded to its longest text, so neighbours of similar
            // size waste the least work on padding. Largest first because the
            // batches run several at a time: started last, the largest would run
            // on alone while the workers that are done sit idle.
            let sql = if force {
                format!(
                    "SELECT e.id FROM entities e WHERE {selected} \
                     ORDER BY length(e.data) DESC, e.rowid"
                )
            } else {
                format!(
                    "SELECT e.id FROM entities e \
                     LEFT JOIN entity_vectors v ON v.entity_id = e.id \
                     WHERE v.entity_id IS NULL AND {selected} \
                     ORDER BY length(e.data) DESC, e.rowid"
                )
            };

            let mut stmt = conn.prepare_cached(&sql)?;
            let ids: Vec<EntityId> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .filter_map(|s| uuid::Uuid::parse_str(&s).ok())
                .collect();

            Ok((ids, usize::try_from(total).unwrap_or(0)))
        })
        .await
        .map_err(|e| Error::task_join("get unembedded entity ids", e))?
    }

    /// Load entity text suitable for embedding for a slice of entity IDs.
    ///
    /// Returns `(entity_id, embedding_text)` pairs, skipping any IDs not found.
    /// The text is produced by the canonical
    /// [`entity_to_text`](crate::database::embeddings::entity_to_text) function
    /// so that the resulting vectors are compatible with those produced by the
    /// pipeline.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_entity_texts_for_ids(
        &self,
        ids: &[EntityId],
    ) -> Result<Vec<(EntityId, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let pool = self.pool.clone();
        let id_strings: Vec<String> = ids.iter().map(|id| id.to_string()).collect();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut result = Vec::with_capacity(id_strings.len());

            for chunk in id_strings.chunks(900) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql =
                    format!("SELECT id, type, data FROM entities WHERE id IN ({placeholders})");
                let mut stmt = conn.prepare_cached(&sql)?;
                let params: Vec<&dyn rusqlite::ToSql> =
                    chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

                let rows = stmt
                    .query_map(&params[..], |row| {
                        let id_str: String = row.get(0)?;
                        let entity_type: String = row.get(1)?;
                        let data_json: String = row.get(2)?;
                        Ok((id_str, entity_type, data_json))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                for (id_str, entity_type, data_json) in rows {
                    let entity_id = match uuid::Uuid::parse_str(&id_str) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };

                    // Deserialise then convert to text via the canonical function.
                    match vector::data_to_entity(&entity_type, &data_json) {
                        Ok(entity) => {
                            let text = embeddings::entity_to_text(&entity);
                            result.push((entity_id, text));
                        }
                        // Silence here loses the entity from semantic search
                        // with no trace: it is selected for embedding, produces
                        // no text, and the run reports fewer vectors stored than
                        // entities queued. A type missing from the mapping in
                        // `vector::data_to_entity` is the usual cause.
                        Err(e) => tracing::warn!(
                            "entity {entity_id} ({entity_type}) will not be embedded: {e}"
                        ),
                    }
                }
            }

            Ok(result)
        })
        .await
        .map_err(|e| Error::task_join("get entity texts for ids", e))?
    }

    /// Resolve entity ids to their name, type and file.
    ///
    /// Reads those columns directly (no `data` deserialisation), so it is cheap
    /// enough to name-resolve relationship endpoints in bulk — e.g. the `target`
    /// of an edge whose entity is not in the caller's own match set. An id
    /// that is not stored is left out.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_entity_names_by_ids(&self, ids: &[EntityId]) -> Result<Vec<EntityHeader>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let pool = self.pool.clone();
        let id_strings: Vec<String> = ids.iter().map(std::string::ToString::to_string).collect();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut result = Vec::with_capacity(id_strings.len());

            for chunk in id_strings.chunks(900) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT id, name, type, file_path, start_line FROM entities \
                     WHERE id IN ({placeholders})"
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                let params: Vec<&dyn rusqlite::ToSql> =
                    chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

                let rows = stmt
                    .query_map(&params[..], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, u32>(4)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                for (id_str, name, entity_type, file_path, start_line) in rows {
                    if let Ok(id) = uuid::Uuid::parse_str(&id_str) {
                        result.push(EntityHeader {
                            id,
                            name,
                            entity_type,
                            file_path,
                            start_line,
                        });
                    }
                }
            }

            Ok(result)
        })
        .await
        .map_err(|e| Error::task_join("get entity names for ids", e))?
    }

    /// The first line of each of `ids` that exists, from the `start_line`
    /// column (no `data` deserialisation) — for search results that carry an
    /// entity id but not its location.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn get_entity_start_lines(
        &self,
        ids: &[EntityId],
    ) -> Result<std::collections::HashMap<EntityId, u32>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let pool = self.pool.clone();
        let id_strings: Vec<String> = ids.iter().map(std::string::ToString::to_string).collect();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut result = std::collections::HashMap::with_capacity(id_strings.len());

            for chunk in id_strings.chunks(900) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql =
                    format!("SELECT id, start_line FROM entities WHERE id IN ({placeholders})");
                let mut stmt = conn.prepare_cached(&sql)?;
                let params: Vec<&dyn rusqlite::ToSql> =
                    chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

                let rows = stmt
                    .query_map(&params[..], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                for (id_str, start_line) in rows {
                    if let Ok(id) = uuid::Uuid::parse_str(&id_str) {
                        result.insert(id, start_line);
                    }
                }
            }

            Ok(result)
        })
        .await
        .map_err(|e| Error::task_join("get entity start lines for ids", e))?
    }

    /// Drop the FTS5 sync triggers to speed up bulk inserts.
    ///
    /// Call this before a fresh (non-incremental) pipeline run, then call
    /// [`end_bulk_load`](Self::end_bulk_load) afterwards to rebuild the index.
    ///
    /// **Do not use on incremental scans** — dropping triggers discards
    /// intermediate index updates and `rebuild` replaces the whole index.
    pub async fn begin_bulk_load(&self) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS entities_fts_insert;
                 DROP TRIGGER IF EXISTS entities_fts_delete;
                 DROP TRIGGER IF EXISTS entities_fts_update;",
            )?;
            Ok(())
        })
        .await
        .map_err(|e| Error::task_join("begin bulk load", e))?
    }

    /// Rebuild the FTS5 index from the canonical `entities` table and
    /// re-create the sync triggers.
    ///
    /// Must be called after [`begin_bulk_load`](Self::begin_bulk_load).
    pub async fn end_bulk_load(&self) -> Result<()> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            // Rebuild FTS index from scratch.
            conn.execute_batch("INSERT INTO entities_fts(entities_fts) VALUES('rebuild');")?;
            // Re-create sync triggers (use pre-computed name_tokens column).
            conn.execute_batch(
                "CREATE TRIGGER IF NOT EXISTS entities_fts_insert
                 AFTER INSERT ON entities
                 BEGIN
                   INSERT INTO entities_fts(rowid, name, name_tokens, body)
                   VALUES (new.rowid, new.name, new.name_tokens, new.body);
                 END;
                 CREATE TRIGGER IF NOT EXISTS entities_fts_delete
                 AFTER DELETE ON entities
                 BEGIN
                   INSERT INTO entities_fts(entities_fts, rowid, name, name_tokens, body)
                   VALUES('delete', old.rowid, old.name, old.name_tokens, old.body);
                 END;
                 CREATE TRIGGER IF NOT EXISTS entities_fts_update
                 AFTER UPDATE ON entities
                 BEGIN
                   INSERT INTO entities_fts(entities_fts, rowid, name, name_tokens, body)
                   VALUES('delete', old.rowid, old.name, old.name_tokens, old.body);
                   INSERT INTO entities_fts(rowid, name, name_tokens, body)
                   VALUES (new.rowid, new.name, new.name_tokens, new.body);
                 END;",
            )?;
            Ok(())
        })
        .await
        .map_err(|e| Error::task_join("end bulk load", e))?
    }

    /// Get all functions from the database
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_all_functions(&self) -> Result<Vec<crate::entities::Function>> {
        operations::get_all_functions(&self.pool).await
    }

    /// Get all classes from the database
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_all_classes(&self) -> Result<Vec<crate::entities::Class>> {
        operations::get_all_classes(&self.pool).await
    }

    /// Get all relationships from the database
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_all_relationships(&self) -> Result<Vec<Relationship>> {
        operations::get_all_relationships(&self.pool).await
    }

    /// Get database statistics
    ///
    /// # Returns
    /// Tuple of (file_count, entity_count, relationship_count)
    ///
    /// # Errors
    /// Returns an error if the database operation fails or times out
    pub async fn get_stats(&self) -> Result<(usize, usize, usize)> {
        let pool = self.pool.clone();
        let timeout_seconds = self.config.query_timeout_seconds;

        let task = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let file_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;

            let entity_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;

            let relationship_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM relationships", [], |row| row.get(0))?;

            Ok((
                usize::try_from(file_count).unwrap_or(0),
                usize::try_from(entity_count).unwrap_or(0),
                usize::try_from(relationship_count).unwrap_or(0),
            ))
        });

        // Apply timeout if configured
        if timeout_seconds > 0 {
            match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_seconds), task)
                .await
            {
                Ok(result) => result.map_err(|e| Error::task_join("get stats", e))?,
                Err(_) => Err(Error::timeout(timeout_seconds, "get stats")),
            }
        } else {
            task.await.map_err(|e| Error::task_join("get stats", e))?
        }
    }

    /// Get statistics scoped to a path.
    ///
    /// `path` is matched as a substring of `entities.file_path` (the same
    /// semantics as `SearchOptions::file_pattern`), so both absolute and
    /// relative fragments work. Returns `(files, entities, relationships)`
    /// where `relationships` counts edges whose **source** entity lives under
    /// the path.
    ///
    /// # Errors
    /// Returns an error if the database operation fails or times out
    pub async fn get_stats_for_path(&self, path: &str) -> Result<(usize, usize, usize)> {
        let pool = self.pool.clone();
        let timeout_seconds = self.config.query_timeout_seconds;
        let pattern = format!("%{}%", crate::utils::validation::escape_like(path));

        let task = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let file_count: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT file_path) FROM entities
                 WHERE file_path LIKE ?1 ESCAPE '\\'",
                [&pattern],
                |row| row.get(0),
            )?;

            let entity_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE file_path LIKE ?1 ESCAPE '\\'",
                [&pattern],
                |row| row.get(0),
            )?;

            let relationship_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM relationships r
                 JOIN entities e ON r.source_id = e.id
                 WHERE e.file_path LIKE ?1 ESCAPE '\\'",
                [&pattern],
                |row| row.get(0),
            )?;

            Ok((
                usize::try_from(file_count).unwrap_or(0),
                usize::try_from(entity_count).unwrap_or(0),
                usize::try_from(relationship_count).unwrap_or(0),
            ))
        });

        if timeout_seconds > 0 {
            match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_seconds), task)
                .await
            {
                Ok(result) => result.map_err(|e| Error::task_join("get stats for path", e))?,
                Err(_) => Err(Error::timeout(timeout_seconds, "get stats for path")),
            }
        } else {
            task.await
                .map_err(|e| Error::task_join("get stats for path", e))?
        }
    }

    /// Get the total number of entities in the database.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_entity_count(&self) -> Result<usize> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;
            Ok(usize::try_from(count).unwrap_or(0))
        })
        .await
        .map_err(|e| Error::task_join("get entity count", e))?
    }

    /// Create a new row in `embedding_jobs` with status = 'running'.
    ///
    /// Returns the job `id` (UUID string). The caller writes progress via
    /// [`update_embedding_job`](Self::update_embedding_job) and finalises
    /// with [`finish_embedding_job`](Self::finish_embedding_job).
    ///
    /// # Errors
    /// Returns an error if the database write fails.
    pub async fn create_embedding_job(&self, total: usize) -> Result<String> {
        let pool = self.pool.clone();
        let job_id = uuid::Uuid::new_v4().to_string();
        let id = job_id.clone();
        let now = time::OffsetDateTime::now_utc().to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            conn.execute(
                "INSERT INTO embedding_jobs (id, status, total, done, started_at, updated_at)
                 VALUES (?1, 'running', ?2, 0, ?3, ?3)",
                rusqlite::params![id, total as i64, now],
            )?;
            Ok::<(), Error>(())
        })
        .await
        .map_err(|e| Error::task_join("create embedding job", e))??;
        Ok(job_id)
    }

    /// Update the `done` counter and `updated_at` timestamp for a running job.
    ///
    /// # Errors
    /// Returns an error if the database write fails.
    pub async fn update_embedding_job(&self, job_id: &str, done: usize) -> Result<()> {
        let pool = self.pool.clone();
        let id = job_id.to_string();
        let now = time::OffsetDateTime::now_utc().to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE embedding_jobs SET done = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, done as i64, now],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| Error::task_join("update embedding job", e))?
    }

    /// Mark a job as 'done' or 'failed'.
    ///
    /// # Errors
    /// Returns an error if the database write fails.
    pub async fn finish_embedding_job(&self, job_id: &str, success: bool) -> Result<()> {
        let pool = self.pool.clone();
        let id = job_id.to_string();
        let status = if success { "done" } else { "failed" };
        let now = time::OffsetDateTime::now_utc().to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            conn.execute(
                "UPDATE embedding_jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, status, now],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| Error::task_join("finish embedding job", e))?
    }

    /// Read the latest embedding job row (by `started_at` descending).
    ///
    /// Returns `None` when no jobs have been recorded yet.
    ///
    /// # Errors
    /// Returns an error if the database read fails.
    pub async fn latest_embedding_job(&self) -> Result<Option<EmbeddingJobStatus>> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut stmt = conn.prepare_cached(
                "SELECT id, status, total, done, started_at, updated_at
                 FROM embedding_jobs
                 ORDER BY started_at DESC
                 LIMIT 1",
            )?;
            match stmt.query_row([], |row| {
                Ok(EmbeddingJobStatus {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    total: row.get::<_, i64>(2)? as usize,
                    done: row.get::<_, i64>(3)? as usize,
                    started_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            }) {
                Ok(job) => Ok(Some(job)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
        .map_err(|e| Error::task_join("latest embedding job", e))?
    }
}

/// Snapshot of an `embedding_jobs` row returned by [`Database::latest_embedding_job`].
#[derive(Debug, Clone)]
pub struct EmbeddingJobStatus {
    /// Job UUID string.
    pub id: String,
    /// 'running' | 'done' | 'failed'
    pub status: String,
    /// Total entities to embed.
    pub total: usize,
    /// Entities embedded so far.
    pub done: usize,
    /// ISO-8601 timestamp when the job started.
    pub started_at: String,
    /// ISO-8601 timestamp of the last progress update.
    pub updated_at: String,
}
