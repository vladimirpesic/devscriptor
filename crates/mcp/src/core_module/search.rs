//! Search tool implementations: functions, classes, fuzzy, semantic

use devscriptor_core::utils::number_lines;
use futures::StreamExt as _;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::tools::{
    ClassInfo, FunctionInfo, FuzzySearchResponse, FuzzySearchResult, SearchClassesResponse,
    SearchFunctionsResponse, SemanticSearchResponse, SemanticSearchResult,
};

/// Compute a basic fuzzy-match score between a query and an entity name.
///
/// Returns a value in the range `[0.0, 1.0]` where:
/// - `1.0` = exact match
/// - `0.9` = case-insensitive exact match
/// - `0.8` = query is a prefix of the name (case-insensitive)
/// - `0.6` = name contains the query as a substring (case-insensitive)
/// - lower values = character overlap ratio (Jaccard-like)
fn compute_fuzzy_score(query: &str, name: &str) -> f64 {
    if query.is_empty() || name.is_empty() {
        return 0.0;
    }

    // Stack-allocated lowercased byte buffers — zero heap allocation for
    // typical code identifiers (covers 99 % of real-world entity names).
    let mut q_buf = [0u8; 64];
    let mut n_buf = [0u8; 128];

    let query_lower = lower_cow(query, &mut q_buf);
    let name_lower = lower_cow(name, &mut n_buf);

    // Exact match (case-sensitive)
    if query == name {
        return 1.0;
    }

    // Case-insensitive exact match
    if query_lower == name_lower {
        return 0.95;
    }

    // Prefix match
    if name_lower.starts_with(query_lower.as_ref()) {
        return 0.85;
    }

    // Contains match
    if name_lower.contains(query_lower.as_ref()) {
        return 0.65;
    }

    // Character-level overlap (Jaccard-like) — stack-only bitset for ASCII.
    // Non-ASCII identifiers fall back to a small Vec to avoid HashSet allocation.
    let jaccard = ascii_jaccard(query_lower.as_ref(), name_lower.as_ref());
    jaccard * 0.5 // cap non-substring matches at 0.5
}

/// Lowercase a string into a stack buffer, falling back to `String::to_lowercase()`
/// only for non-ASCII or oversized inputs (rare for code identifiers).
fn lower_cow<'a>(s: &str, buf: &'a mut [u8]) -> std::borrow::Cow<'a, str> {
    let bytes = s.as_bytes();
    if bytes.len() <= buf.len() && s.is_ascii() {
        for (i, &b) in bytes.iter().enumerate() {
            buf[i] = b.to_ascii_lowercase();
        }
        // SAFETY: ASCII lowercasing preserves UTF-8 validity.
        std::borrow::Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(&buf[..bytes.len()]) })
    } else {
        std::borrow::Cow::Owned(s.to_lowercase())
    }
}

/// Compute Jaccard similarity of character sets using a fixed-size bitset.
///
/// Uses two `[u64; 2]` arrays (128 bits) on the stack — zero heap allocation
/// for typical ASCII identifiers. Non-ASCII characters are collected into
/// small temporary vectors only when needed.
fn ascii_jaccard(a: &str, b: &str) -> f64 {
    let mut bits_a = [0u64; 2];
    let mut bits_b = [0u64; 2];
    let mut extra_a: Vec<char> = Vec::new();
    let mut extra_b: Vec<char> = Vec::new();

    for ch in a.chars() {
        let c = ch as usize;
        if c < 128 {
            bits_a[c / 64] |= 1u64 << (c % 64);
        } else {
            extra_a.push(ch);
        }
    }
    for ch in b.chars() {
        let c = ch as usize;
        if c < 128 {
            bits_b[c / 64] |= 1u64 << (c % 64);
        } else {
            extra_b.push(ch);
        }
    }

    let mut intersection =
        (bits_a[0] & bits_b[0]).count_ones() + (bits_a[1] & bits_b[1]).count_ones();
    let mut union = (bits_a[0] | bits_b[0]).count_ones() + (bits_a[1] | bits_b[1]).count_ones();

    // Handle non-ASCII fallback (rare for code identifiers)
    if !extra_a.is_empty() || !extra_b.is_empty() {
        extra_a.sort_unstable();
        extra_a.dedup();
        extra_b.sort_unstable();
        extra_b.dedup();
        for ch in &extra_a {
            if extra_b.binary_search(ch).is_ok() {
                intersection += 1;
            } else {
                union += 1;
            }
        }
        for ch in &extra_b {
            if extra_a.binary_search(ch).is_err() {
                union += 1;
            }
        }
    }

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

impl CoreModule {
    /// Search options for a file-scoped entity search, used when `search_functions`
    /// / `search_classes` receive a `file_path` filter so the filter and the
    /// pagination are both applied by SQL rather than after truncation.
    fn entity_search_options(
        entity_type: &str,
        file_pattern: &str,
        limit: usize,
        offset: usize,
    ) -> devscriptor_core::search::SearchOptions {
        devscriptor_core::search::SearchOptions {
            fuzzy: true,
            entity_type: Some(entity_type.to_string()),
            file_pattern: Some(file_pattern.to_string()),
            limit: Some(limit),
            offset: Some(offset),
            ..Default::default()
        }
    }

    pub(super) async fn search_functions(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.unwrap_or_default();

        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(20) as usize;
        let offset = args
            .get("offset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;

        // Optional file filter, matched as a substring of the entity's file path.
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        info!("Searching functions: {} (file: {:?})", query, file_path);

        let searcher = self.state.searcher();
        // Push pagination — and the file filter, when present — into SQL (MCP-6 fix).
        let entities = match file_path {
            Some(pattern) => searcher
                .search_entities(
                    query,
                    Self::entity_search_options("function", pattern, limit, offset),
                )
                .await
                .map(|results| results.into_iter().map(|r| r.entity).collect::<Vec<_>>()),
            None => {
                searcher
                    .search_functions(query, true, Some(limit), Some(offset))
                    .await
            }
        };
        match entities {
            Ok(entities) => {
                let functions: Vec<FunctionInfo> = entities
                    .into_iter()
                    .filter_map(|e| {
                        if let devscriptor_core::entities::Entity::Function(f) = e {
                            Some(FunctionInfo {
                                name: f.name,
                                file_path: f.file_path,
                                start_line: f.location.start_line,
                                end_line: f.location.end_line,
                                language: f.language,
                                docstring: f.docstring.map(|d| d.chars().take(200).collect()),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                let returned = functions.len();

                let response = SearchFunctionsResponse {
                    total: returned,
                    returned,
                    offset,
                    limit,
                    functions,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Search functions failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn search_classes(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.unwrap_or_default();

        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(20) as usize;
        let offset = args
            .get("offset")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;

        // Optional file filter, matched as a substring of the entity's file path.
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        info!("Searching classes: {} (file: {:?})", query, file_path);

        let searcher = self.state.searcher();
        // Push pagination — and the file filter, when present — into SQL (MCP-6 fix).
        let entities = match file_path {
            Some(pattern) => searcher
                .search_entities(
                    query,
                    Self::entity_search_options("class", pattern, limit, offset),
                )
                .await
                .map(|results| results.into_iter().map(|r| r.entity).collect::<Vec<_>>()),
            None => {
                searcher
                    .search_classes(query, true, Some(limit), Some(offset))
                    .await
            }
        };
        match entities {
            Ok(entities) => {
                let classes: Vec<ClassInfo> = entities
                    .into_iter()
                    .filter_map(|e| {
                        if let devscriptor_core::entities::Entity::Class(c) = e {
                            let methods: Vec<String> = c.methods.unwrap_or_default();
                            let method_count = methods.len();

                            Some(ClassInfo {
                                name: c.name,
                                file_path: c.file_path,
                                start_line: c.location.start_line,
                                end_line: c.location.end_line,
                                language: c.language,
                                method_count,
                                methods: methods.into_iter().take(10).collect(),
                                docstring: c.docstring.map(|d| d.chars().take(200).collect()),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                let returned = classes.len();

                let response = SearchClassesResponse {
                    total: returned,
                    returned,
                    offset,
                    limit,
                    classes,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Search classes failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn fuzzy_search(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' argument".into()))?;

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        info!("Fuzzy search: {}", query);

        // True fuzzy matching: a typo like "totl_area" does not substring-match
        // "total_area", so scoring only `LIKE` hits returns nothing. Instead score
        // a broad (capped) candidate set with `compute_fuzzy_score` and keep the
        // best matches. An empty pattern (`%%`) selects all entities up to the cap.
        const CANDIDATE_CAP: usize = 20_000;
        const MIN_SCORE: f64 = 0.3;

        match self
            .state
            .database()
            .search_entities("", CANDIDATE_CAP)
            .await
        {
            Ok(entities) => {
                let mut results: Vec<FuzzySearchResult> = entities
                    .into_iter()
                    .filter_map(|e| {
                        let match_score = compute_fuzzy_score(query, e.name());
                        (match_score >= MIN_SCORE).then(|| FuzzySearchResult {
                            name: e.name().to_string(),
                            entity_type: e.entity_type().to_string(),
                            file_path: e.file_path().to_string(),
                            start_line: e.location().start_line,
                            end_line: e.location().end_line,
                            match_score,
                        })
                    })
                    .collect();

                // Best matches first, then cap to the requested limit.
                results.sort_by(|a, b| {
                    b.match_score
                        .partial_cmp(&a.match_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results.truncate(limit);

                let total = results.len();
                let response = FuzzySearchResponse {
                    query: query.to_string(),
                    results,
                    total,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Fuzzy search failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn semantic_search(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' argument".into()))?;

        // No filter unless one is asked for. Defaulting to `function` would make
        // every unqualified semantic search silently function-only: classes,
        // modules and documentation sections would be absent from results that
        // look complete, and a caller could not tell a missing match from a
        // filtered-out one.
        let entity_type = args
            .get("entity_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        info!("Semantic search: {} (type: {})", query, entity_type);

        let searcher = self.state.searcher();
        match searcher.semantic_search(query, entity_type, limit).await {
            Ok(results) => {
                let formatted_results: Vec<SemanticSearchResult> = results
                    .into_iter()
                    .map(|r| SemanticSearchResult {
                        name: r.name,
                        file_path: r.file_path,
                        start_line: r.start_line,
                        end_line: r.end_line,
                        entity_type: r.entity_type,
                        language: Some(r.language),
                        similarity_score: r.similarity,
                        body_preview: r
                            .body_preview
                            .map(|preview| number_lines(&preview, r.start_line)),
                        fallback: r.fallback,
                    })
                    .collect();

                let any_fallback = formatted_results.iter().any(|r| r.fallback);
                // With no vectors yet only keywords can match, so an empty answer
                // says nothing about meaning; say so rather than look conclusive.
                let unembedded = formatted_results.is_empty()
                    && !self.state.database().has_vectors().await.unwrap_or(true);
                let response = SemanticSearchResponse {
                    query: query.to_string(),
                    entity_type: entity_type.to_string(),
                    results: formatted_results,
                    message: if any_fallback {
                        Some("Keyword fallback: results are keyword-matched, not semantically similar.".into())
                    } else if unembedded {
                        Some(
                            "No entity has an embedding vector yet, so only keyword matches \
                             could be found, and none matched. Run generate_embeddings once \
                             for the project, then search again."
                                .into(),
                        )
                    } else {
                        None
                    },
                    service_available: true,
                };

                ok_json(&response)
            }
            Err(e) => {
                let msg = format!("{e}");
                if matches!(e, devscriptor_core::Error::EmbeddingServiceNotConfigured)
                    || msg.contains("Embedding service not configured")
                {
                    warn!(
                        "Semantic search unavailable: embedding service not configured — falling back to keyword search"
                    );
                    // Graceful fallback: keyword search via search_entities
                    let keyword_results = self.state.database().search_entities(query, limit).await;
                    match keyword_results {
                        Ok(entities) => {
                            let formatted_results: Vec<SemanticSearchResult> = entities
                                .into_iter()
                                .map(|e| SemanticSearchResult {
                                    name: e.name().to_string(),
                                    file_path: e.file_path().to_string(),
                                    start_line: e.location().start_line,
                                    end_line: e.location().end_line,
                                    entity_type: e.entity_type().to_string(),
                                    language: Some(e.language().to_string()),
                                    similarity_score: 0.5, // neutral placeholder score for keyword matches
                                    body_preview: None,
                                    fallback: true,
                                })
                                .collect();
                            let response = SemanticSearchResponse {
                                query: query.to_string(),
                                entity_type: entity_type.to_string(),
                                results: formatted_results,
                                message: Some("Keyword fallback: embedding service not configured. Results are keyword-matched, not semantically similar.".into()),
                                service_available: false,
                            };
                            return ok_json(&response);
                        }
                        Err(ke) => {
                            warn!("Keyword fallback also failed: {}", ke);
                            let response = SemanticSearchResponse {
                                query: query.to_string(),
                                entity_type: entity_type.to_string(),
                                results: Vec::new(),
                                message: Some(
                                    "Semantic search unavailable and keyword fallback failed."
                                        .into(),
                                ),
                                service_available: false,
                            };
                            return ok_json(&response);
                        }
                    }
                }
                warn!("Semantic search failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    /// Generate and store semantic embedding vectors for entities missing them.
    ///
    /// The MCP counterpart of the `devscriptor embed` CLI command: an agent that
    /// has scanned a codebase can build the dense vector index in one call,
    /// rather than shelling out. Idempotent — only entities without a vector are
    /// processed (unless `force` re-embeds existing ones). `semantic_search` also
    /// embeds on demand for the entities a query touches; this pre-builds the
    /// whole index so subsequent semantic queries are immediate.
    ///
    /// Arguments: `force` (bool, re-embed existing vectors; default false) and
    /// `batch_size` (number, ONNX batch size; default 16).
    pub(super) async fn generate_embeddings(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        use devscriptor_core::entities::EntityId;

        /// Upper bound on the per-call ONNX batch. A caller-supplied batch size
        /// is otherwise unbounded, and one huge batch is a single allocation
        /// large enough to OOM the process.
        const MAX_BATCH_SIZE: u64 = 256;
        /// The fewest batches per concurrent worker a call is cut into, so
        /// that no worker sits idle while the last batches run.
        const BATCHES_PER_WORKER: usize = 3;

        let args = arguments.unwrap_or_default();
        let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
        let batch_size = args
            .get("batch_size")
            .and_then(Value::as_u64)
            .map_or(16, |n| n.clamp(1, MAX_BATCH_SIZE) as usize);

        info!("generate_embeddings (force: {force}, batch_size: {batch_size})");

        let db = self.state.database();
        let selection = self.state.project_config().embedding_selection();
        let (unembedded_ids, total_entities) = db
            .get_unembedded_entity_ids(force, selection)
            .await
            .map_err(ToolError::from)?;

        if unembedded_ids.is_empty() {
            return ok_json(&json!({
                "embedded": 0,
                "pending_before": 0,
                "remaining": 0,
                "total_entities": total_entities,
                "message": "All entities already have embedding vectors; semantic search is fully available."
            }));
        }

        let pending_before = unembedded_ids.len();

        // Reuse the process-wide embedding service rather than building another.
        // The server constructs exactly one (`DevscriptorServerState::assemble`), so
        // the model — ~1.3 GB resident — is loaded at most once per process; a second
        // `EmbeddingService::new` here would load a second copy beside it. The
        // service is cheap to clone (Arc-backed).
        let svc = self
            .state
            .searcher()
            .embedding_service()
            .cloned()
            .ok_or_else(|| {
                ToolError::from(devscriptor_core::Error::EmbeddingServiceNotConfigured)
            })?;

        // Several chunks at once, as `devscriptor embed` runs them. One after
        // another, the call used one core: the model runs each batch on a
        // single thread when it expects several to run together. The chunks
        // are small enough for every worker to get a few (the ids come
        // largest first, so the longest batches start first).
        let workers = svc.session_count().max(1);
        let chunk_size = batch_size
            .min(pending_before.div_ceil(workers * BATCHES_PER_WORKER))
            .max(1);
        let chunks: Vec<Vec<EntityId>> = unembedded_ids
            .chunks(chunk_size)
            .map(<[EntityId]>::to_vec)
            .collect();
        let stored: Vec<Result<usize, ToolError>> =
            futures::stream::iter(chunks.into_iter().map(|id_chunk| {
                let db = db.clone();
                let svc = svc.clone();
                async move {
                    let texts_and_ids = db
                        .get_entity_texts_for_ids(&id_chunk)
                        .await
                        .map_err(ToolError::from)?;
                    if texts_and_ids.is_empty() {
                        return Ok(0);
                    }
                    let texts: Vec<String> = texts_and_ids.iter().map(|(_, t)| t.clone()).collect();
                    let ids: Vec<EntityId> = texts_and_ids.iter().map(|(id, _)| *id).collect();
                    let vectors =
                        tokio::task::spawn_blocking(move || svc.generate_embeddings_batch(&texts))
                            .await
                            .map_err(|e| {
                                ToolError::from(devscriptor_core::Error::Embedding(format!(
                                    "embedding task panicked: {e}"
                                )))
                            })?
                            .map_err(ToolError::from)?;
                    let items: Vec<(EntityId, Vec<f32>)> = ids.into_iter().zip(vectors).collect();
                    db.upsert_embeddings_batch(&items)
                        .await
                        .map_err(ToolError::from)
                }
            }))
            .buffer_unordered(workers)
            .collect()
            .await;
        let mut embedded = 0usize;
        for result in stored {
            embedded += result?;
        }

        // `pending_before` is what lacked vectors when the call started and
        // `remaining` what still does, so a caller cannot read the pre-run count
        // as outstanding work ("pending: 14" when all 14 have just been embedded).
        ok_json(&json!({
            "embedded": embedded,
            "pending_before": pending_before,
            "remaining": pending_before.saturating_sub(embedded),
            "total_entities": total_entities,
            "message": "Embeddings generated; semantic search is now available."
        }))
    }
}
