//! Semantic and hybrid search for context notes

use std::sync::Arc;

use crate::context_graph::{ContextConfig, ContextNote, ContextSearchResult, SearchFilters};
use crate::database::Database;
use crate::embeddings::EmbeddingService;
use crate::error::Result;

/// The most neighbours one sqlite-vec KNN query may ask for
/// (`SQLITE_VEC_VEC0_K_MAX` in sqlite-vec 0.1).
const KNN_K_MAX: i64 = 4096;

/// How many notes one embedding call takes.
const NOTE_EMBED_BATCH: usize = 16;

/// The text a note is embedded from: its title, summary and content.
pub(crate) fn note_text(title: &str, summary: Option<&str>, content: &str) -> String {
    match summary.map(str::trim).filter(|s| !s.is_empty()) {
        Some(summary) => format!("{title}\n\n{summary}\n\n{content}"),
        None => format!("{title}\n\n{content}"),
    }
}

/// The hash a note's vector row keeps of the text it was embedded from.
fn text_hash(text: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()))
}

/// Append the search filters to a query over `context_notes n`.
fn push_filters(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    filters: &SearchFilters,
) {
    if let Some(note_type) = &filters.note_type {
        sql.push_str(" AND n.note_type = ?");
        params.push(Box::new(note_type.as_str().to_string()));
    }
    if let Some(status) = &filters.status {
        sql.push_str(" AND n.status = ?");
        params.push(Box::new(status.as_str().to_string()));
    }
    if let Some(min_p) = filters.min_priority {
        sql.push_str(" AND n.priority >= ?");
        params.push(Box::new(min_p));
    }
}

/// Semantic and hybrid search for context notes
#[derive(Clone)]
pub struct ContextSearch {
    /// Database reference
    database: Arc<Database>,
    /// Configuration
    config: ContextConfig,
    /// Shared embedding service (shared with the entity searcher so the model
    /// is only loaded once per process).  `None` → semantic search degrades to
    /// pure FTS5 keyword search.
    embedding_service: Option<EmbeddingService>,
}

impl ContextSearch {
    /// Create new context search without an embedding service.
    ///
    /// Semantic queries will degrade to FTS5 keyword search.
    /// Use [`Self::with_embedding_service`] to enable vector search.
    pub fn new(database: Arc<Database>, config: ContextConfig) -> Self {
        Self {
            database,
            config,
            embedding_service: None,
        }
    }

    /// Create a new context search with a shared embedding service.
    pub fn with_embedding_service(
        database: Arc<Database>,
        config: ContextConfig,
        embedding_service: EmbeddingService,
    ) -> Self {
        Self {
            database,
            config,
            embedding_service: Some(embedding_service),
        }
    }

    /// Semantic search: the notes nearest the query by meaning.
    ///
    /// Each note is embedded from its title, summary and content into
    /// `note_vectors` ([`Self::sync_note_vectors`]), so a note is ranked by
    /// what it says. Without an embedding service or the vector table (no
    /// sqlite-vec), or when the query cannot be embedded, the search falls
    /// back to FTS5 and marks its results `fallback`.
    ///
    /// # Errors
    /// Returns an error if the full-text fallback fails.
    pub async fn semantic_search(
        &self,
        query: &str,
        top_k: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<ContextSearchResult>> {
        if self.config.enable_embeddings
            && let Some(service) = &self.embedding_service
        {
            match self.vector_search(service, query, top_k, filters).await {
                Ok(Some(results)) => return Ok(results),
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    "Context-note vector search failed: {e}; falling back to full-text"
                ),
            }
        }
        self.full_text_search(query, top_k, filters).await
    }

    /// The notes nearest `query` in `note_vectors`, after bringing the
    /// vectors up to date; `None` when the database has no vector table.
    async fn vector_search(
        &self,
        service: &EmbeddingService,
        query: &str,
        top_k: usize,
        filters: &SearchFilters,
    ) -> Result<Option<Vec<ContextSearchResult>>> {
        if !self.sync_note_vectors(service).await? {
            return Ok(None);
        }
        let embedding = self.generate_query_embedding(query).await?;
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        let pool = self.database.conn_pool().clone();
        let filters = filters.clone();

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let notes: i64 =
                conn.query_row("SELECT COUNT(*) FROM note_vectors", [], |row| row.get(0))?;
            if notes == 0 {
                return Ok(Vec::new());
            }
            // Every note is a candidate (up to sqlite-vec's KNN limit), so the
            // filters, which apply after the KNN, never leave the answer short.
            let mut sql = "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata, v.distance
                 FROM note_vectors v
                 JOIN context_notes n ON n.id = v.rowid
                 WHERE v.embedding MATCH ?1 AND k = ?2".to_string();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(bytes), Box::new(notes.min(KNN_K_MAX))];
            push_filters(&mut sql, &mut params, &filters);
            sql.push_str(" ORDER BY v.distance LIMIT ?");
            params.push(Box::new(top_k as i64));

            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let results = stmt
                .query_map(param_refs.as_slice(), |row| {
                    let note = ContextNote::from_row(row)?;
                    let distance: f64 = row.get("distance")?;
                    // note_vectors uses distance_metric=cosine, so distance ∈ [0, 2]
                    // and the cosine similarity is 1 − distance.
                    Ok(ContextSearchResult {
                        note,
                        score: (1.0 - distance).clamp(0.0, 1.0),
                        metadata: serde_json::Value::Object(serde_json::Map::new()),
                        fallback: false,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok::<Vec<ContextSearchResult>, crate::error::Error>(results)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(Some(results))
    }

    /// Bring `note_vectors` up to date with the notes: embed every note that
    /// has no vector or whose text changed since it was embedded, and drop
    /// the vectors of notes that are gone. A note's vector row carries a hash
    /// of the text it was embedded from, so an edit is noticed however it was
    /// made. Returns `false` when the database has no vector table.
    async fn sync_note_vectors(&self, service: &EmbeddingService) -> Result<bool> {
        let pool = self.database.conn_pool().clone();
        let plan = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let has_table: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'note_vectors')",
                [],
                |row| row.get(0),
            )?;
            if !has_table {
                return Ok(None);
            }
            let mut stored = std::collections::HashMap::new();
            let mut stmt = conn.prepare("SELECT rowid, text_hash FROM note_vectors")?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })? {
                let (id, hash) = row?;
                stored.insert(id, hash);
            }
            let mut stmt = conn.prepare("SELECT id, title, summary, content FROM context_notes")?;
            let mut wanted = Vec::new();
            let mut alive = std::collections::HashSet::new();
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })? {
                let (id, title, summary, content) = row?;
                alive.insert(id);
                let text = note_text(&title, summary.as_deref(), &content);
                let hash = text_hash(&text);
                if stored.get(&id).and_then(Option::as_deref) != Some(hash.as_str()) {
                    wanted.push((id, text, hash));
                }
            }
            let gone: Vec<i64> = stored
                .into_keys()
                .filter(|id| !alive.contains(id))
                .collect();
            Ok::<_, crate::error::Error>(Some((wanted, gone)))
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        let Some((wanted, gone)) = plan else {
            return Ok(false);
        };
        if wanted.is_empty() && gone.is_empty() {
            return Ok(true);
        }
        let mut vectors = Vec::with_capacity(wanted.len());
        for chunk in wanted.chunks(NOTE_EMBED_BATCH) {
            let texts: Vec<String> = chunk.iter().map(|(_, text, _)| text.clone()).collect();
            let service = service.clone();
            vectors.extend(
                tokio::task::spawn_blocking(move || service.generate_embeddings_batch(&texts))
                    .await
                    .map_err(|e| crate::error::Error::task_join("embedding task", e))??,
            );
        }

        let pool = self.database.conn_pool().clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let tx = crate::database::operations::begin_write_transaction(&conn)?;
            for id in gone {
                tx.execute("DELETE FROM note_vectors WHERE rowid = ?1", [id])?;
            }
            // sqlite-vec has no UPSERT: a changed note's row is replaced.
            for ((id, _, hash), vector) in wanted.into_iter().zip(vectors) {
                let bytes: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
                tx.execute("DELETE FROM note_vectors WHERE rowid = ?1", [id])?;
                tx.execute(
                    "INSERT INTO note_vectors (rowid, embedding, text_hash) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, bytes, hash],
                )?;
            }
            tx.commit()?;
            Ok::<(), crate::error::Error>(())
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;
        Ok(true)
    }

    /// Hybrid search combining vector similarity with keyword matching.
    ///
    /// Runs [`Self::semantic_search`] and [`Self::keyword_search`]
    /// concurrently and fuses their rankings with Reciprocal Rank Fusion
    /// (k = 60): each list adds `1 / (60 + rank + 1)` to a note's score. The fused
    /// score is divided by the top one, so the best match scores 1.0, as
    /// entity search reports it. Without vectors the semantic list is the
    /// full-text one, and the ranking is the keyword ranking.
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn hybrid_search(
        &self,
        query: &str,
        filters: &SearchFilters,
        top_k: usize,
    ) -> Result<Vec<ContextSearchResult>> {
        let semantic_future = self.semantic_search(query, top_k * 2, filters);
        let keyword_future = self.keyword_search(query, top_k * 2, filters);
        let (semantic_results, keyword_results) = tokio::join!(semantic_future, keyword_future);
        let semantic_results = semantic_results.unwrap_or_default();
        let keyword_results = keyword_results.unwrap_or_default();

        const RRF_K: f64 = 60.0;
        let mut scores: std::collections::HashMap<Option<i64>, f64> =
            std::collections::HashMap::new();
        let mut note_map: std::collections::HashMap<Option<i64>, ContextSearchResult> =
            std::collections::HashMap::new();
        // Each list holds a note at most once: one vector, one FTS row per note.
        for list in [semantic_results, keyword_results] {
            for (rank, result) in list.into_iter().enumerate() {
                *scores.entry(result.note.id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
                note_map.entry(result.note.id).or_insert(result);
            }
        }

        let mut ranked: Vec<(Option<i64>, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(top_k);
        let top = ranked.first().map_or(1.0, |(_, score)| *score);
        let norm = if top > 0.0 { top } else { 1.0 };
        Ok(ranked
            .into_iter()
            .filter_map(|(id, score)| {
                let mut result = note_map.remove(&id)?;
                result.score = (score / norm).clamp(0.0, 1.0);
                Some(result)
            })
            .collect())
    }

    /// Keyword search using LIKE patterns
    ///
    /// # Arguments
    /// * `query` - The search query
    /// * `limit` - Maximum results
    /// * `filters` - Optional filters
    ///
    /// # Returns
    /// List of matching notes
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn keyword_search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<ContextSearchResult>> {
        // The query's words first, ranked (BM25 over title, content and
        // summary); then the query as one substring, which still finds part
        // of a word. Matching only the whole query as a substring found a
        // note for a sentence only if the note contained that sentence.
        let ranked = self
            .full_text_search(query, limit, filters)
            .await
            .unwrap_or_default();
        if !ranked.is_empty() {
            return Ok(ranked);
        }
        self.substring_search(query, limit, filters).await
    }

    /// Notes whose title or content contains `query` as written.
    async fn substring_search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<ContextSearchResult>> {
        let pool = self.database.conn_pool().clone();
        let query = query.to_string();
        let filters = filters.clone();

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sql = "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata
                 FROM context_notes n
                 WHERE (n.title LIKE ?1 ESCAPE '\\' OR n.content LIKE ?1 ESCAPE '\\')".to_string();
            let search_pattern = format!("%{}%", crate::utils::escape_like(&query));
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(search_pattern)];
            push_filters(&mut sql, &mut params, &filters);

            sql.push_str(" ORDER BY n.priority DESC, n.created_at DESC LIMIT ?");
            params.push(Box::new(limit as i64));

            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;

            let query_lower = query.to_lowercase();
            let notes = stmt
                .query_map(param_refs.as_slice(), |row| {
                    let note = ContextNote::from_row(row)?;

                    // Compute a simple relevance score based on match location.
                    // Use case-insensitive comparison without allocating new strings.
                    let title = &note.title;
                    let content = &note.content;
                    let title_matches = title.to_lowercase().contains(&query_lower);
                    let content_matches = content.to_lowercase().contains(&query_lower);
                    let score = if title_matches {
                        1.0
                    } else if content_matches {
                        0.7
                    } else {
                        0.5
                    };

                    Ok(ContextSearchResult {
                        note,
                        score,
                        metadata: serde_json::Value::Object(serde_json::Map::new()),
                        fallback: false,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ContextSearchResult>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(results)
    }

    /// Search notes linked to a code entity type
    ///
    /// # Arguments
    /// * `entity_type` - Type of entity (function, class, etc.)
    ///
    /// # Returns
    /// List of related notes
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn search_by_entity(&self, entity_type: &str) -> Result<Vec<ContextSearchResult>> {
        self.search_by_entity_filtered(entity_type, None, None)
            .await
    }

    /// Search notes linked to a code entity, optionally narrowing by the linked
    /// entity's name and/or file path.
    ///
    /// `entity_name` is matched exactly against `context_entity_links.entity_name`.
    /// `file_path` is matched as a substring of `context_entity_links.file_path`,
    /// so a relative fragment matches an absolute stored path.
    ///
    /// # Arguments
    /// * `entity_type` - Type of entity (function, class, etc.)
    /// * `entity_name` - Optional exact entity name to narrow the link
    /// * `file_path` - Optional file-path fragment to narrow the link
    ///
    /// # Returns
    /// List of related notes
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn search_by_entity_filtered(
        &self,
        entity_type: &str,
        entity_name: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<ContextSearchResult>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let entity_type = entity_type.to_string();
        let entity_name = entity_name.map(ToString::to_string);
        let file_path =
            file_path.map(|p| format!("%{}%", crate::utils::validation::escape_like(p)));

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sql = String::from(
                "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata, l.link_type as link_type
                 FROM context_notes n
                 JOIN context_entity_links l ON n.id = l.context_note_id
                 WHERE l.entity_type = ?1",
            );

            let mut params: Vec<rusqlite::types::Value> =
                vec![rusqlite::types::Value::Text(entity_type)];

            if let Some(name) = entity_name {
                params.push(rusqlite::types::Value::Text(name));
                sql.push_str(&format!(" AND l.entity_name = ?{}", params.len()));
            }
            if let Some(pattern) = file_path {
                params.push(rusqlite::types::Value::Text(pattern));
                sql.push_str(&format!(
                    " AND l.file_path LIKE ?{} ESCAPE '\\'",
                    params.len()
                ));
            }

            sql.push_str(" ORDER BY n.priority DESC, n.created_at DESC");

            let mut stmt = conn.prepare(&sql)?;

            let notes = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    let note = ContextNote::from_row(row)?;
                    let link_type: String = row.get("link_type")?;

                    let mut metadata = serde_json::Map::new();
                    metadata.insert("link_type".to_string(), serde_json::Value::String(link_type));

                    Ok(ContextSearchResult {
                        note,
                        score: 0.0,
                        metadata: serde_json::Value::Object(metadata),
                        fallback: false,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ContextSearchResult>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(results)
    }

    /// Search notes linked to an abstract concept
    ///
    /// # Arguments
    /// * `concept_name` - Name of the concept
    ///
    /// # Returns
    /// List of related notes
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn search_by_concept(&self, concept_name: &str) -> Result<Vec<ContextSearchResult>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let concept_name = concept_name.to_string();

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let sql = "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata, c.link_type
                 FROM context_notes n
                 JOIN context_concept_links c ON n.id = c.context_note_id
                 WHERE c.concept_name = ?1
                 ORDER BY n.priority DESC, n.created_at DESC";

            let mut stmt = conn.prepare(sql)?;

            let notes = stmt
                .query_map([&concept_name], |row| {
                    let note = ContextNote::from_row(row)?;
                    let link_type: String = row.get("link_type")?;

                    let mut metadata = serde_json::Map::new();
                    metadata.insert("link_type".to_string(), serde_json::Value::String(link_type));

                    Ok(ContextSearchResult {
                        note,
                        score: 0.0,
                        metadata: serde_json::Value::Object(metadata),
                        fallback: false,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ContextSearchResult>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(results)
    }

    /// Get all notes with pagination
    ///
    /// # Arguments
    /// * `limit` - Maximum results
    /// * `offset` - Pagination offset
    ///
    /// # Returns
    /// List of notes
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn get_all_notes(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ContextSearchResult>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let limit_str = limit.to_string();
        let offset_str = offset.to_string();

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let sql = "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata
                 FROM context_notes n
                 ORDER BY n.priority DESC, n.created_at DESC
                 LIMIT ?1 OFFSET ?2";

            let mut stmt = conn.prepare(sql)?;

            let notes = stmt
                .query_map([&limit_str, &offset_str], |row| {
                    let note = ContextNote::from_row(row)?;

                    Ok(ContextSearchResult {
                        note,
                        score: 1.0,
                        metadata: serde_json::Value::Object(serde_json::Map::new()),
                        fallback: false,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ContextSearchResult>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(results)
    }

    /// Get the configuration
    #[must_use]
    pub const fn config(&self) -> &ContextConfig {
        &self.config
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Generate an embedding vector for a plain-text query using the injected service.
    ///
    /// Returns an error if no embedding service was configured.
    async fn generate_query_embedding(&self, query: &str) -> Result<Vec<f32>> {
        let service = self
            .embedding_service
            .clone()
            .ok_or(crate::error::Error::EmbeddingServiceNotConfigured)?;
        let query = query.to_string();

        tokio::task::spawn_blocking(move || service.generate_embedding(&query))
            .await
            .map_err(|e| crate::error::Error::task_join("embedding task", e))?
    }

    /// Full-text search over context notes using the FTS5 virtual table.
    async fn full_text_search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<ContextSearchResult>> {
        // The query as FTS5 must see it: see `fts_query`.
        let Some(query) = crate::database::fts_query::match_query(query) else {
            return Ok(Vec::new());
        };
        let pool = self.database.conn_pool().clone();
        let filters = filters.clone();

        let results = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sql = "SELECT n.id, n.note_type, n.title, n.content, n.summary, n.author, n.priority, n.status,
                        n.created_at, n.updated_at, n.effective_from, n.effective_until,
                        n.tags, n.structured_data, n.metadata, fts.rank AS rank
                 FROM context_notes n
                 JOIN context_notes_fts fts ON fts.rowid = n.id
                 WHERE context_notes_fts MATCH ?1".to_string();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query.clone())];
            push_filters(&mut sql, &mut params, &filters);

            // Best match first; among equal matches, the more important note.
            sql.push_str(" ORDER BY fts.rank, n.priority DESC, n.created_at DESC LIMIT ?");
            params.push(Box::new(limit as i64));

            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;

            let ranked = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok((ContextNote::from_row(row)?, row.get::<_, f64>("rank")?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // BM25 ranks are negative, the best the most negative: each match
            // scores its rank's share of the best one's, so the best is 1.0.
            let best = ranked.first().map_or(-1.0, |(_, rank)| *rank);
            let notes = ranked
                .into_iter()
                .map(|(note, rank)| ContextSearchResult {
                    note,
                    score: if best < 0.0 { (rank / best).clamp(0.0, 1.0) } else { 1.0 },
                    metadata: serde_json::Value::Object(serde_json::Map::new()),
                    fallback: true,
                })
                .collect();

            Ok::<Vec<ContextSearchResult>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_graph::NoteType;

    #[test]
    fn test_search_filters_creation() {
        let filters = SearchFilters::new().with_note_type(NoteType::Adr);

        assert_eq!(filters.note_type, Some(NoteType::Adr));
    }
}
