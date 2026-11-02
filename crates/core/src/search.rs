//! Search functionality for Devscriptor
//!
//! This module provides comprehensive search capabilities for codebase entities,
//! including name-based search, fuzzy matching, pattern matching, semantic search,
//! and streaming/paginated results.

use std::collections::HashMap;

use crate::database::connection::ConnectionPool;
use crate::database::vector;
use crate::embeddings::EmbeddingService;
use crate::entities::{Entity, EntityId, Relationship};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::utils::escape_like;

/// Compute a fuzzy match score between 0.0 and 1.0 for a name against a query.
///
/// Returns higher scores for closer matches:
/// - Exact match (case-sensitive): 1.0
/// - Exact match (case-insensitive): 0.95
/// - Starts with query (case-insensitive): 0.80–0.90
/// - Contains query (case-insensitive): 0.50–0.70
/// - Otherwise: 0.30 (baseline for LIKE matches that don't contain the query)
fn compute_match_score(name: &str, query: &str) -> f64 {
    if query.is_empty() {
        return 1.0;
    }

    let name_lower = name.to_lowercase();
    let query_lower = query.to_lowercase();

    if name == query {
        return 1.0;
    }
    if name_lower == query_lower {
        return 0.95;
    }
    if name_lower.starts_with(&query_lower) {
        let ratio = query.len() as f64 / name.len().max(1) as f64;
        return 0.80 + ratio * 0.10;
    }
    if name_lower.contains(&query_lower) {
        let ratio = query.len() as f64 / name.len().max(1) as f64;
        return 0.50 + ratio * 0.20;
    }

    // Substring characters present but not contiguous (SQLite LIKE matched)
    0.30
}

/// Classify the match type based on the score.
fn classify_match_type(score: f64) -> &'static str {
    if score >= 0.95 {
        "exact"
    } else if score >= 0.80 {
        "prefix"
    } else if score >= 0.50 {
        "contains"
    } else {
        "fuzzy"
    }
}

/// Search options for entity queries
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Enable fuzzy matching
    pub fuzzy: bool,
    /// Filter by entity type
    pub entity_type: Option<String>,
    /// Filter by language
    pub language: Option<String>,
    /// Filter by file path pattern
    pub file_pattern: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
    /// Sort by field
    pub sort_by: Option<String>,
    /// Sort ascending
    pub sort_asc: bool,
}

/// Search result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult<T> {
    /// The matched entity
    pub entity: T,
    /// Relevance score (higher = better match)
    pub score: f64,
    /// Match type (exact, fuzzy, pattern, etc.)
    pub match_type: String,
}

/// Paginated search results.
///
/// **Note**: This is intentionally separate from a lightweight API pagination type; this variant
/// a lightweight version for streaming/API pagination. This richer variant includes derived fields
/// such as `total_pages`, `has_next`, and `has_previous` for the search layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResults<T> {
    /// Results for current page
    pub results: Vec<T>,
    /// Total number of matching items
    pub total_count: usize,
    /// Current page number (1-based)
    pub page_number: usize,
    /// Number of items per page
    pub page_size: usize,
    /// Total number of pages
    pub total_pages: usize,
    /// Whether there are more pages
    pub has_next: bool,
    /// Whether there are previous pages
    pub has_previous: bool,
}

/// Semantic search result with similarity score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticResult {
    /// Entity ID
    pub id: EntityId,
    /// Entity name
    pub name: String,
    /// Entity type
    pub entity_type: String,
    /// File path
    pub file_path: String,
    /// Programming language
    pub language: String,
    /// First line of the entity (1-indexed)
    #[serde(default)]
    pub start_line: u32,
    /// Last line of the entity (1-indexed)
    #[serde(default)]
    pub end_line: u32,
    /// Relevance score in `[0, 1]`, normalized so the top result is `1.0`.
    ///
    /// Produced by hybrid BM25 + vector ranking (Reciprocal Rank Fusion) and then
    /// scaled to `[0, 1]` within the result set. It is a *relative* relevance for
    /// ranking, not an absolute cosine similarity, and is not comparable across
    /// queries. (Field name kept as `similarity` for API/serialization stability.)
    pub similarity: f64,
    /// Entity body preview
    pub body_preview: Option<String>,
    /// `true` when results came from a keyword fallback because the embedding
    /// service was unavailable. These results have no semantic meaning.
    #[serde(default)]
    pub fallback: bool,
}

/// Hybrid retriever: BM25 (FTS5) + dense (cosine vector) with RRF fusion.
///
/// Always produces results — even with zero corpus vectors it returns BM25
/// results. When vectors are available the two channels are fused via
/// Reciprocal Rank Fusion (k = 60), giving semantically-ranked output with
/// graceful degradation.
///
/// ## On-demand embedding
///
/// When on-demand embedding is on and an embedding service is configured,
/// BM25 candidates that lack vectors, of those the embedding selection picks,
/// are embedded on the fly (checking `embedding_cache` first) and stored in
/// `entity_vectors` before the dense channel runs. This bounds embedding work
/// to exactly what is queried and makes the second identical query cache-hot
/// (no new ONNX calls).
///
/// The flag defaults to `false` so CI and air-gapped environments stay
/// pure-lexical. Enable via [`Retriever::with_on_demand_embedding`].
pub struct Retriever {
    pool: ConnectionPool,
    embedding_service: Option<EmbeddingService>,
    /// The entities on-demand embedding may embed, when it is on.
    on_demand_embedding: Option<crate::embeddings::EmbeddingSelection>,
}

impl Retriever {
    /// Create a retriever backed by the given connection pool.
    ///
    /// Pass `None` for `embedding_service` for pure BM25 mode.
    /// On-demand embedding is disabled; use [`Self::with_on_demand_embedding`]
    /// to enable it.
    pub fn new(pool: ConnectionPool, embedding_service: Option<EmbeddingService>) -> Self {
        Self {
            pool,
            embedding_service,
            on_demand_embedding: None,
        }
    }

    /// Enable on-demand top-K embedding: BM25 candidates without vectors that
    /// `selection` embeds are embedded (cache-first) and stored before the
    /// dense channel runs. The others stay lexical, as `devscriptor embed`
    /// leaves them.
    ///
    /// Has no effect when `embedding_service` is `None`.
    #[must_use]
    pub fn with_on_demand_embedding(
        mut self,
        selection: crate::embeddings::EmbeddingSelection,
    ) -> Self {
        self.on_demand_embedding = Some(selection);
        self
    }

    /// Retrieve the top `limit` results for `query` using hybrid BM25 + dense search.
    ///
    /// - BM25 channel: FTS5 `entities_fts MATCH ?` ranked by relevance.
    /// - Dense channel: cosine ANN from `entity_vectors` (only when
    ///   `embedding_service` is `Some`). If `on_demand_embedding` is `true`,
    ///   BM25 candidates without vectors are embedded first.
    /// - Fusion: Reciprocal Rank Fusion with k = 60.
    ///
    /// When no vectors are available the result set is the BM25 channel alone
    /// with `fallback = false` (BM25 is first-class, not a fallback).
    ///
    /// # Errors
    /// Returns an error if the database cannot be reached.
    pub async fn retrieve(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SemanticResult>> {
        let candidates = limit.saturating_mul(3).max(20);

        // BM25 channel — always runs
        let bm25 = crate::database::operations::fts_search_entities(
            &self.pool,
            query,
            candidates,
            entity_type,
        )
        .await
        .unwrap_or_default();

        // On-demand embedding: embed BM25 candidates that lack vectors so the
        // dense channel can rank them. Silently skips on any error so the BM25
        // channel always survives.
        if let Some(selection) = self.on_demand_embedding
            && let Some(svc) = self.embedding_service.as_ref()
        {
            let _ = self.embed_missing_candidates(&bm25, svc, selection).await;
        }

        // Dense channel (only when embedding service is wired up)
        let dense: Vec<(Entity, f32)> = if let Some(svc) = self.embedding_service.as_ref() {
            let svc = svc.clone();
            let q = query.to_string();
            match tokio::task::spawn_blocking(move || svc.generate_embedding(&q))
                .await
                .map_err(|e| Error::Embedding(format!("Task failed: {e}")))?
            {
                Ok(embedding) => {
                    vector::search_similar_filtered(&self.pool, &embedding, candidates, entity_type)
                        .await
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        Ok(Self::rrf_fuse(bm25, dense, limit))
    }

    /// Embed BM25 candidates that `selection` embeds and that do not yet have
    /// a vector in `entity_vectors`.
    ///
    /// Steps:
    /// 1. Identify which candidates lack a vector row.
    /// 2. Compute `entity_to_text` + XXH3 hash for each missing one.
    /// 3. Batch-check `embedding_cache`; skip ONNX for cache hits.
    /// 4. Embed cache misses in one batched ONNX call.
    /// 5. Persist new vectors to both `entity_vectors` and `embedding_cache`.
    ///
    /// Errors are silently suppressed so the BM25 channel is never blocked.
    async fn embed_missing_candidates(
        &self,
        candidates: &[Entity],
        svc: &EmbeddingService,
        selection: crate::embeddings::EmbeddingSelection,
    ) -> Result<()> {
        use crate::database::embeddings::entity_to_text;
        use crate::database::vector::{
            entity_ids_with_vectors, lookup_embedding_cache, store_embedding_cache,
            upsert_embeddings_batch,
        };

        if candidates.is_empty() {
            return Ok(());
        }

        // 1. Which candidates already have vectors?
        let candidate_ids: Vec<EntityId> = candidates.iter().map(|e| e.id()).collect();
        let with_vectors = entity_ids_with_vectors(&self.pool, &candidate_ids).await?;

        let missing: Vec<&Entity> = candidates
            .iter()
            .filter(|e| {
                !with_vectors.contains(&e.id()) && crate::embeddings::should_embed(e, selection)
            })
            .collect();

        if missing.is_empty() {
            return Ok(());
        }

        // 2. Compute text + hash for each missing candidate.
        let texts_and_ids: Vec<(EntityId, String, String)> = missing
            .iter()
            .map(|e| {
                let text = entity_to_text(e);
                let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()));
                (e.id(), text, hash)
            })
            .collect();

        // 3. Check embedding_cache for hash hits.
        let hashes: Vec<String> = texts_and_ids.iter().map(|(_, _, h)| h.clone()).collect();
        let cached = lookup_embedding_cache(&self.pool, &hashes).await?;

        let mut new_vectors: Vec<(EntityId, Vec<f32>)> = Vec::new();
        let mut cache_misses: Vec<(EntityId, String)> = Vec::new(); // (id, text)

        for (id, text, hash) in &texts_and_ids {
            if let Some(vec) = cached.get(hash) {
                new_vectors.push((*id, vec.clone()));
            } else {
                cache_misses.push((*id, text.clone()));
            }
        }

        // 4. Batch-embed cache misses.
        if !cache_misses.is_empty() {
            let miss_texts: Vec<String> = cache_misses.iter().map(|(_, t)| t.clone()).collect();
            let miss_ids: Vec<EntityId> = cache_misses.iter().map(|(id, _)| *id).collect();
            let svc = svc.clone();
            let embeddings =
                tokio::task::spawn_blocking(move || svc.generate_embeddings_batch(&miss_texts))
                    .await
                    .map_err(|e| Error::task_join("on-demand embed", e))??;

            // Collect for cache storage.
            let cache_pairs: Vec<(String, Vec<f32>)> = cache_misses
                .iter()
                .zip(embeddings.iter())
                .map(|((_, text), vec)| {
                    let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()));
                    (hash, vec.clone())
                })
                .collect();

            // 5a. Persist to embedding_cache.
            store_embedding_cache(&self.pool, &cache_pairs).await?;

            for (id, vec) in miss_ids.into_iter().zip(embeddings) {
                new_vectors.push((id, vec));
            }
        }

        // 5b. Persist new vectors to entity_vectors.
        if !new_vectors.is_empty() {
            upsert_embeddings_batch(&self.pool, &new_vectors).await?;
        }

        Ok(())
    }

    /// Fuse BM25 and dense result lists with Reciprocal Rank Fusion (k = 60).
    ///
    /// Each channel contributes `1 / (60 + rank + 1)` to the entity's score.
    /// Results are deduplicated by entity ID, sorted descending by fused score,
    /// and truncated to `limit`.
    fn rrf_fuse(bm25: Vec<Entity>, dense: Vec<(Entity, f32)>, limit: usize) -> Vec<SemanticResult> {
        const RRF_K: f64 = 60.0;

        let mut scores: HashMap<EntityId, f64> = HashMap::new();
        let mut entity_map: HashMap<EntityId, Entity> = HashMap::new();

        for (rank, entity) in bm25.into_iter().enumerate() {
            let id = entity.id();
            *scores.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
            entity_map.entry(id).or_insert(entity);
        }
        for (rank, (entity, _distance)) in dense.into_iter().enumerate() {
            let id = entity.id();
            *scores.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
            entity_map.entry(id).or_insert(entity);
        }

        let mut ranked: Vec<(EntityId, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(limit);

        // Normalize the raw RRF sums to a [0, 1] relevance where the top result is
        // 1.0. Each channel contributes ~1/(60+rank+1), so the raw fused score is
        // tiny (~0.016–0.033) and is a *ranking* artifact, not a cosine similarity;
        // surfacing it directly misled consumers (CLI/MCP) into reading strong
        // matches as ~2%. Dividing by the top score is a monotonic transform — the
        // ordering, and therefore search quality, is unchanged.
        let top_score = ranked.first().map_or(1.0, |(_, s)| *s);
        let norm = if top_score > 0.0 { top_score } else { 1.0 };

        ranked
            .into_iter()
            .filter_map(|(id, score)| {
                let entity = entity_map.remove(&id)?;
                let body_preview = CodebaseSearcher::extract_body_preview(&entity);
                Some(SemanticResult {
                    id: entity.id(),
                    name: entity.name().to_string(),
                    entity_type: entity.entity_type().to_string(),
                    file_path: entity.file_path().to_string(),
                    language: entity.language().to_string(),
                    start_line: entity.location().start_line,
                    end_line: entity.location().end_line,
                    similarity: (score / norm).clamp(0.0, 1.0),
                    body_preview,
                    fallback: false,
                })
            })
            .collect()
    }
}

/// Provides search functionality for codebase entities
pub struct CodebaseSearcher {
    pool: ConnectionPool,
    embedding_service: Option<EmbeddingService>,
    on_demand_embedding: bool,
    /// The entities on-demand embedding may embed; a long-lived server
    /// changes it with its project's configuration.
    selection: parking_lot::RwLock<crate::embeddings::EmbeddingSelection>,
}

impl CodebaseSearcher {
    /// Create a new searcher for keyword/pattern (BM25) searches only.
    ///
    /// No embedding service is attached, so [`find_similar`](Self::find_similar)
    /// degrades to FTS5 keyword search with no vector channel. For semantic
    /// search, construct the searcher with
    /// [`with_embedding_service`](Self::with_embedding_service).
    pub fn new(pool: ConnectionPool) -> Self {
        Self {
            pool,
            embedding_service: None,
            on_demand_embedding: false,
            selection: parking_lot::RwLock::default(),
        }
    }

    /// Create a new searcher with an explicit embedding service.
    pub fn with_embedding_service(
        pool: ConnectionPool,
        embedding_service: EmbeddingService,
    ) -> Self {
        Self {
            pool,
            embedding_service: Some(embedding_service),
            on_demand_embedding: false,
            selection: parking_lot::RwLock::default(),
        }
    }

    /// Enable on-demand top-K embedding for the searches this searcher performs.
    ///
    /// Propagated to every [`Retriever`] it builds, so BM25 candidates without
    /// vectors, of those `selection` embeds, are embedded (cache-first) before
    /// the dense channel runs. Without this, a caller that indexed a codebase
    /// but never ran a separate embed pass gets a purely lexical
    /// `semantic_search`. Has no effect when no embedding service is
    /// configured.
    #[must_use]
    pub fn with_on_demand_embedding(
        mut self,
        selection: crate::embeddings::EmbeddingSelection,
    ) -> Self {
        self.on_demand_embedding = true;
        *self.selection.get_mut() = selection;
        self
    }

    /// The entities on-demand embedding may embed from now on.
    pub fn set_embedding_selection(&self, selection: crate::embeddings::EmbeddingSelection) {
        *self.selection.write() = selection;
    }

    /// Build a retriever carrying this searcher's on-demand embedding setting.
    fn retriever(&self) -> Retriever {
        let retriever = Retriever::new(self.pool.clone(), self.embedding_service.clone());
        if self.on_demand_embedding {
            retriever.with_on_demand_embedding(*self.selection.read())
        } else {
            retriever
        }
    }

    /// Get a reference to the embedding service, if one is configured.
    pub fn embedding_service(&self) -> Option<&EmbeddingService> {
        self.embedding_service.as_ref()
    }

    /// Search for functions by name
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    /// Search for entities by type and name with optional limit and offset.
    async fn search_entities_by_type(
        &self,
        entity_type: &str,
        identifier: &str,
        fuzzy: bool,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Entity>> {
        let pool = self.pool.clone();
        let entity_type = entity_type.to_string();
        let identifier = identifier.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let query = format!("%{}%", escape_like(&identifier));

            let sql_limit = limit
                .and_then(|l| i64::try_from(l).ok())
                .map_or(String::new(), |l| format!(" LIMIT {l}"));
            let sql_offset = offset
                .and_then(|o| i64::try_from(o).ok())
                .map_or(String::new(), |o| format!(" OFFSET {o}"));
            let sql_pagination = format!("{}{}", sql_limit, sql_offset);

            let entities = if fuzzy && !identifier.is_empty() {
                let sql = format!(
                    "SELECT type, data \
                     FROM entities \
                     WHERE type = ?1 AND name LIKE ?2 ESCAPE '\\' \
                     ORDER BY \
                         CASE WHEN name = ?3 THEN 0 ELSE 1 END, \
                         name{}",
                    sql_pagination,
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                stmt.query_map([&entity_type, &query, &identifier], Self::row_to_entity)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                let sql = format!(
                    "SELECT type, data \
                     FROM entities \
                     WHERE type = ?1 AND (?2 = '' OR name LIKE ?2 ESCAPE '\\') \
                     ORDER BY name{}",
                    sql_pagination,
                );
                let mut stmt = conn.prepare_cached(&sql)?;
                stmt.query_map([&entity_type, &query], Self::row_to_entity)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };

            Ok(entities)
        })
        .await
        .map_err(|e| Error::task_join("search by type", e))?
    }

    /// Search for functions by name with optional pagination.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_functions(
        &self,
        identifier: &str,
        fuzzy: bool,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Entity>> {
        self.search_entities_by_type("function", identifier, fuzzy, limit, offset)
            .await
    }

    /// Search for classes by name with optional pagination.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_classes(
        &self,
        identifier: &str,
        fuzzy: bool,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Entity>> {
        self.search_entities_by_type("class", identifier, fuzzy, limit, offset)
            .await
    }

    /// Get relationships for an entity or all relationships
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn get_relationships(
        &self,
        entity_id: Option<EntityId>,
    ) -> Result<Vec<Relationship>> {
        let pool = self.pool.clone();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let sql = "SELECT id, source_id, target_id, relationship_type, metadata, created_at \
                       FROM relationships \
                       WHERE ?1 IS NULL OR source_id = ?1 OR target_id = ?1 \
                       ORDER BY created_at DESC";
            let mut stmt = conn.prepare(sql)?;
            let id_param = entity_id.map(|id| id.to_string());

            let relationships = stmt
                .query_map(
                    [&id_param],
                    crate::database::operations::row_to_relationship,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(relationships)
        })
        .await
        .map_err(|e| Error::task_join("get relationships", e))?
    }

    /// Find similar entities using hybrid BM25 + vector retrieval.
    ///
    /// Delegates to [`Retriever::retrieve`] with no entity-type filter.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn find_similar(
        &self,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<SemanticResult>> {
        self.retriever().retrieve(query_text, None, limit).await
    }

    /// Extract a preview of the entity body (first few lines)
    fn extract_body_preview(entity: &Entity) -> Option<String> {
        let body = entity.body();

        // Get first 3 lines as preview
        let preview: String = body.lines().take(3).collect::<Vec<_>>().join("\n");
        if preview.len() > 200 {
            Some(format!(
                "{}...",
                preview.chars().take(200).collect::<String>()
            ))
        } else if preview.is_empty() {
            None
        } else {
            Some(preview)
        }
    }

    /// Search entities by pattern
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_by_pattern(
        &self,
        pattern: &str,
        entity_type: &str,
        use_regex: bool,
    ) -> Result<Vec<Entity>> {
        if use_regex {
            // Lightweight client-side validation to catch obvious ReDoS patterns
            // before they reach SQLite's regex engine.
            if pattern.len() > 256 {
                return Err(crate::error::Error::validation(
                    "regex pattern exceeds maximum length of 256 characters",
                    "pattern",
                    pattern,
                ));
            }
            // Reject patterns with deeply nested repetition (common ReDoS vector).
            let nesting = pattern.chars().filter(|&c| c == '(' || c == ')').count();
            if nesting > 32 {
                return Err(crate::error::Error::validation(
                    "regex pattern has too many nested groups (max 32)",
                    "pattern",
                    pattern,
                ));
            }
            // Reject patterns with excessive quantifier nesting.
            let star_count = pattern.matches('*').count();
            let plus_count = pattern.matches('+').count();
            if star_count + plus_count > 16 {
                return Err(crate::error::Error::validation(
                    "regex pattern has too many quantifiers (max 16)",
                    "pattern",
                    pattern,
                ));
            }
            // Validate that the pattern is syntactically valid by compiling it
            // with the standard regex engine. This also rejects many ReDoS-prone
            // patterns that the regex crate refuses to compile.
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(crate::error::Error::validation(
                    format!("invalid regex pattern: {e}"),
                    "pattern",
                    pattern,
                ));
            }
        }

        let pool = self.pool.clone();
        let pattern = pattern.to_string();
        let entity_type = entity_type.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            if use_regex {
                let sql = r#"
                    SELECT type, data
                    FROM entities
                    WHERE (?1 = '' OR type = ?1) AND name REGEXP ?2
                    ORDER BY name
                "#;

                let mut stmt = conn.prepare(sql)?;
                let entities = stmt
                    .query_map([&entity_type, &pattern], Self::row_to_entity)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok(entities)
            } else {
                let sql = r#"
                    SELECT type, data
                    FROM entities
                    WHERE (?1 = '' OR type = ?1) AND name LIKE ?2 ESCAPE '\'
                    ORDER BY name
                "#;

                let mut stmt = conn.prepare(sql)?;
                let like_pattern = format!("%{}%", escape_like(&pattern));
                let entities = stmt
                    .query_map([&entity_type, &like_pattern], Self::row_to_entity)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok(entities)
            }
        })
        .await
        .map_err(|e| Error::task_join("fuzzy search", e))?
    }

    /// Advanced pattern matching using regex
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_pattern_match(
        &self,
        pattern: &str,
        entity_type: &str,
    ) -> Result<Vec<Entity>> {
        self.search_by_pattern(pattern, entity_type, true).await
    }

    /// Perform hybrid semantic search (BM25 + dense with RRF fusion).
    ///
    /// Delegates to [`Retriever::retrieve`]. Always returns results: when no
    /// embedding service is configured BM25 results are returned directly.
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn semantic_search(
        &self,
        query_text: &str,
        entity_type: &str,
        limit: usize,
    ) -> Result<Vec<SemanticResult>> {
        let et_filter = if entity_type.is_empty() {
            None
        } else {
            Some(entity_type)
        };
        self.retriever()
            .retrieve(query_text, et_filter, limit)
            .await
    }

    /// Get paginated search results.
    ///
    /// # Warning
    /// This function loads **all** results from `search_fn` before slicing.
    /// For very large result sets, consider pushing pagination to the SQL layer
    /// (e.g. `LIMIT`/`OFFSET`) in the underlying query method.
    ///
    /// # Type Parameters
    /// - `F`: The search function type
    ///
    /// # Errors
    /// Returns an error if the search or pagination fails
    pub async fn get_paginated_results<F, Fut, T>(
        &self,
        search_fn: F,
        page_number: usize,
        page_size: usize,
    ) -> Result<PaginatedResults<T>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<T>>>,
        T: Clone,
    {
        let all_results = search_fn().await?;
        let total_count = all_results.len();
        // Warn when loading very large result sets into memory for pagination.
        // This is a reminder to push pagination down to the SQL layer.
        const LARGE_RESULT_THRESHOLD: usize = 10_000;
        if total_count > LARGE_RESULT_THRESHOLD {
            tracing::warn!(
                "get_paginated_results loaded {} items into memory; consider SQL-level pagination",
                total_count
            );
        }
        let total_pages = total_count.div_ceil(page_size);

        let start = (page_number.saturating_sub(1)) * page_size;
        let end = (start + page_size).min(total_count);

        let results = if start < total_count {
            all_results[start..end].to_vec()
        } else {
            Vec::new()
        };

        Ok(PaginatedResults {
            results,
            total_count,
            page_number,
            page_size,
            total_pages,
            has_next: page_number < total_pages,
            has_previous: page_number > 1,
        })
    }

    /// Search all entity types with options
    ///
    /// # Errors
    /// Returns an error if the database operation fails
    pub async fn search_entities(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<SearchResult<Entity>>> {
        let pool = self.pool.clone();
        let query = query.to_string();
        let options = options.clone();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sql = String::from("SELECT type, data FROM entities WHERE 1=1");
            // Use type-safe parameter collection to avoid coercing integers (LIMIT/OFFSET)
            // into strings.
            let mut params: Vec<rusqlite::types::Value> = Vec::new();

            // Add name filter
            let query_for_score = query.clone();
            if !query.is_empty() {
                if options.fuzzy {
                    sql.push_str(" AND name LIKE ? ESCAPE '\\'");
                    params.push(rusqlite::types::Value::Text(format!(
                        "%{}%",
                        escape_like(&query)
                    )));
                } else {
                    sql.push_str(" AND name = ?");
                    params.push(rusqlite::types::Value::Text(query));
                }
            }

            // Add type filter
            if let Some(et) = options.entity_type {
                sql.push_str(" AND type = ?");
                params.push(rusqlite::types::Value::Text(et));
            }

            // Add language filter
            if let Some(lang) = options.language {
                sql.push_str(" AND language = ?");
                params.push(rusqlite::types::Value::Text(lang));
            }

            // Add file pattern filter
            if let Some(pattern) = options.file_pattern {
                sql.push_str(" AND file_path LIKE ? ESCAPE '\\'");
                params.push(rusqlite::types::Value::Text(format!(
                    "%{}%",
                    escape_like(&pattern)
                )));
            }

            // Add sorting
            match options.sort_by.as_deref() {
                Some("name") => {
                    sql.push_str(" ORDER BY name ");
                    sql.push_str(if options.sort_asc { "ASC" } else { "DESC" });
                }
                Some("created") => {
                    sql.push_str(" ORDER BY created_at ");
                    sql.push_str(if options.sort_asc { "ASC" } else { "DESC" });
                }
                _ => {
                    sql.push_str(" ORDER BY name ASC");
                }
            }

            // Add limit and offset using type-safe i64 parameters
            if let Some(limit) = options.limit {
                sql.push_str(" LIMIT ?");
                params.push(rusqlite::types::Value::Integer(
                    i64::try_from(limit).unwrap_or(-1),
                ));
            }
            if let Some(offset) = options.offset {
                sql.push_str(" OFFSET ?");
                params.push(rusqlite::types::Value::Integer(
                    i64::try_from(offset).unwrap_or(0),
                ));
            }

            let mut stmt = conn.prepare(&sql)?;

            // Build params array for rusqlite using params_from_iter for arbitrary counts
            let fuzzy = options.fuzzy;
            let entities: Vec<SearchResult<Entity>> = stmt
                .query_map(rusqlite::params_from_iter(&params), |row| {
                    let entity = Self::row_to_entity(row)?;
                    let score = if fuzzy {
                        compute_match_score(entity.name(), &query_for_score)
                    } else {
                        1.0
                    };
                    Ok(SearchResult {
                        entity,
                        score,
                        match_type: classify_match_type(score).to_string(),
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(entities)
        })
        .await
        .map_err(|e| Error::task_join("full-text search", e))?
    }

    /// Convert database row to entity.
    /// Delegates to the canonical [`crate::database::operations::row_to_entity`].
    fn row_to_entity(row: &rusqlite::Row) -> std::result::Result<Entity, rusqlite::Error> {
        crate::database::operations::row_to_entity(row)
    }
}

/// Convenience function to search for functions
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn search_functions(
    pool: ConnectionPool,
    identifier: &str,
    fuzzy: bool,
) -> Result<Vec<Entity>> {
    let searcher = CodebaseSearcher::new(pool);
    searcher
        .search_functions(identifier, fuzzy, None, None)
        .await
}

/// Convenience function to search for classes
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn search_classes(
    pool: ConnectionPool,
    identifier: &str,
    fuzzy: bool,
) -> Result<Vec<Entity>> {
    let searcher = CodebaseSearcher::new(pool);
    searcher.search_classes(identifier, fuzzy, None, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, DatabaseConfig};
    use crate::embeddings::{EmbeddingService, MockEmbeddingProvider};
    use crate::entities::{Entity, EntityId, Function, SourceLocation};
    use std::sync::Arc;
    use time::OffsetDateTime;

    /// A `Database` in a temporary directory, which the test holds for as
    /// long as the database: bound as `let (_dir, db)`, the database is
    /// dropped before its directory is removed.
    fn make_test_db() -> (tempfile::TempDir, Arc<Database>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Arc::new(
            Database::new(DatabaseConfig {
                path: path.to_string_lossy().to_string(),
                ..DatabaseConfig::default()
            })
            .expect("test db"),
        );
        (dir, db)
    }

    /// Insert a dummy file row (required by FK) and one Function entity.
    async fn seed_function(db: &Database, name: &str) {
        use crate::database::operations::upsert_file;
        use crate::entities::FileInfo;

        let file_path = format!("/test/{name}.rs");
        let file = FileInfo {
            id: EntityId::new_v4(),
            path: file_path.clone(),
            language: "rust".to_string(),
            size_bytes: 100,
            line_count: 10,
            checksum: "abc".to_string(),
            last_modified: OffsetDateTime::now_utc(),
            entities: vec![],
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        upsert_file(db.conn_pool(), &file)
            .await
            .expect("upsert file");

        let entity = Entity::Function(Function {
            id: EntityId::new_v4(),
            name: name.to_string(),
            file_path,
            body: format!("fn {name}() {{ /* implementation */ }}"),
            location: SourceLocation::new(1, 0, 3, 0),
            language: "rust".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: None,
            return_type: None,
            modifiers: None,
            access_modifiers: None,
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        });
        db.insert_entities(&[entity]).await.expect("insert entity");
    }

    /// 3.1-A: `EmbeddingService::new()` must not load the model.
    /// The real model is lazy; this asserts the invariant survives wiring into `Retriever`.
    #[test]
    fn retriever_service_not_loaded_at_construction() {
        let service = EmbeddingService::new(crate::embeddings::EmbeddingConfig::default());
        assert!(
            !service.is_model_loaded(),
            "model must not be loaded at EmbeddingService::new()"
        );
        // Wiring into Retriever must not trigger a load either.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.db");
        let db = Database::new(DatabaseConfig {
            path: path.to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        })
        .expect("db");
        let _retriever = Retriever::new(db.conn_pool().clone(), Some(service.clone()));
        assert!(
            !service.is_model_loaded(),
            "model must not be loaded after Retriever::new()"
        );
    }

    /// 3.1-B: With `embedding_service = None`, `Retriever::retrieve` returns non-empty
    /// results from the BM25 channel when entities are in the DB (`fallback = false`).
    #[tokio::test]
    async fn retriever_bm25_returns_results_with_no_service() {
        let (_dir, db) = make_test_db();
        seed_function(&db, "find_user_by_id").await;

        let retriever = Retriever::new(db.conn_pool().clone(), None);
        let results = retriever
            .retrieve("find_user_by_id", None, 10)
            .await
            .expect("retrieve");

        assert!(
            !results.is_empty(),
            "BM25 must return results when entities exist"
        );
        assert!(
            results.iter().all(|r| !r.fallback),
            "fallback must be false — BM25 is first-class"
        );
        assert!(
            results.iter().any(|r| r.name == "find_user_by_id"),
            "expected the seeded entity in results"
        );
    }

    /// On-demand embedding embeds only the candidates the embedding selection
    /// picks: under `standard` a constant stays lexical, as `devscriptor
    /// embed` leaves it, while a function gets its vector.
    #[tokio::test]
    async fn on_demand_embedding_follows_the_selection() {
        use crate::embeddings::EmbeddingSelection;

        let (_dir, db) = make_test_db();
        let parsed = crate::parser::parse_content_sync(
            "const PAYMENT_LIMIT: u32 = 10;\n\
             fn process_payment(amount: u32) -> u32 { amount.min(PAYMENT_LIMIT) }\n",
            "rust",
            std::path::Path::new("pay.rs"),
        )
        .expect("parse");
        db.batch_insert_parsed_files(&[parsed.to_record()], None)
            .await
            .expect("insert");
        let id = |name: &str| {
            parsed
                .entities
                .iter()
                .find(|entity| entity.name() == name)
                .map(Entity::id)
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let (constant, function) = (id("PAYMENT_LIMIT"), id("process_payment"));
        let embedded = || async {
            crate::database::vector::entity_ids_with_vectors(db.conn_pool(), &[constant, function])
                .await
                .expect("vectors")
        };

        let svc = EmbeddingService::with_provider(MockEmbeddingProvider::new());
        Retriever::new(db.conn_pool().clone(), Some(svc.clone()))
            .with_on_demand_embedding(EmbeddingSelection::Standard)
            .retrieve("payment", None, 10)
            .await
            .expect("retrieve");
        let vectors = embedded().await;
        assert!(vectors.contains(&function));
        assert!(
            !vectors.contains(&constant),
            "standard leaves constants lexical"
        );

        Retriever::new(db.conn_pool().clone(), Some(svc))
            .with_on_demand_embedding(EmbeddingSelection::All)
            .retrieve("payment", None, 10)
            .await
            .expect("retrieve");
        assert!(embedded().await.contains(&constant), "all embeds it");
    }

    /// 3.1-C: With a mock embedding service, `Retriever::retrieve` still returns results
    /// for a repo with zero corpus vectors (mock produces vectors but entity_vectors is empty).
    #[tokio::test]
    async fn retriever_with_service_returns_bm25_when_no_vectors() {
        let (_dir, db) = make_test_db();
        seed_function(&db, "process_payment").await;

        let svc = EmbeddingService::with_provider(MockEmbeddingProvider::new());
        let retriever = Retriever::new(db.conn_pool().clone(), Some(svc));
        // entity_vectors is empty, so dense channel contributes nothing.
        let results = retriever
            .retrieve("process_payment", None, 10)
            .await
            .expect("retrieve");

        assert!(
            !results.is_empty(),
            "must return BM25 results when entity_vectors is empty"
        );
        assert!(
            results.iter().any(|r| r.name == "process_payment"),
            "expected seeded entity"
        );
    }
}
