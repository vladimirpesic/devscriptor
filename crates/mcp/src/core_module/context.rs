//! Context graph tool implementations: notes, search, prune, stats

use serde_json::{Value, json};
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::tools::{
    ContextNoteResult, PruneContextResponse, PruneStatistics, SearchContextResponse,
};

impl CoreModule {
    pub(super) async fn add_context_note(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let note_type = args
            .get("note_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'note_type' argument".into()))?;

        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'title' argument".into()))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'content' argument".into()))?;

        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .map(String::from);

        let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

        let priority = args
            .get("priority")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(5) as u32;
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("agent");

        info!(
            "Adding context note: {} (type: {}, priority: {})",
            title, note_type, priority
        );

        let manager = self.state.context_manager();
        use devscriptor_core::context_graph::{
            ConceptLinkSpec, ContextNote, EntityLinkSpec, NoteStatus, NoteType,
        };

        // Parse optional entity links: notes can be attached to concrete code
        // entities so `get_related_context` can later retrieve them.
        let related_entities: Option<Vec<EntityLinkSpec>> = args
            .get("related_entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let entity_type = item.get("entity_type").and_then(|v| v.as_str())?;
                        Some(EntityLinkSpec {
                            entity_type: entity_type.to_string(),
                            entity_id: item
                                .get("entity_id")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            entity_name: item
                                .get("entity_name")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            file_path: item
                                .get("file_path")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            link_type: item
                                .get("link_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("references")
                                .to_string(),
                        })
                    })
                    .collect()
            });

        // Parse optional concept links: notes can be attached to abstract
        // concepts/modules/patterns.
        let related_concepts: Option<Vec<ConceptLinkSpec>> = args
            .get("related_concepts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let concept_type = item.get("concept_type").and_then(|v| v.as_str())?;
                        let concept_name = item.get("concept_name").and_then(|v| v.as_str())?;
                        Some(ConceptLinkSpec {
                            concept_type: concept_type.to_string(),
                            concept_name: concept_name.to_string(),
                            concept_path: item
                                .get("concept_path")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            link_type: item
                                .get("link_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("references")
                                .to_string(),
                        })
                    })
                    .collect()
            });

        let note_type_enum = match note_type {
            "adr" => NoteType::Adr,
            "standard" => NoteType::Standard,
            "optimization" => NoteType::Optimization,
            "failure" => NoteType::Failure,
            "practice" => NoteType::Practice,
            "reflection" => NoteType::Reflection,
            "file_description" => NoteType::FileDescription,
            "modification" => NoteType::Modification,
            _ => NoteType::General,
        };

        let note = ContextNote {
            id: None,
            note_type: note_type_enum,
            title: title.to_string(),
            content: content.to_string(),
            summary,
            author: author.to_string(),
            tags: tags.unwrap_or_default(),
            priority: priority as i32,
            status: NoteStatus::Active,
            created_at: None,
            updated_at: None,
            effective_from: None,
            effective_until: None,
            structured_data: serde_json::Value::Object(serde_json::Map::new()),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        };

        match manager
            .add_note(note, related_entities, related_concepts)
            .await
        {
            Ok(result) => {
                let response = json!({
                    "success": true,
                    "note_id": result.note_id,
                    "title": result.title,
                    "type": result.note_type,
                    "priority": priority,
                    "entity_links": result.entity_links.len(),
                    "concept_links": result.concept_links.len(),
                });

                ok_json(&response)
            }
            Err(e) => {
                warn!("Add context note failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn search_context(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' argument".into()))?;

        let top_k = args
            .get("top_k")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;
        let max_content = args
            .get("max_content_length")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(500);

        // Search strategy: keyword (default), semantic (vector), or hybrid (RRF).
        // Semantic/hybrid use the shared embedding service and degrade to
        // keyword search when embeddings are unavailable.
        use devscriptor_core::context_graph::{NoteStatus, NoteType, SearchFilters, SearchType};
        let search_type = match args
            .get("search_type")
            .and_then(|v| v.as_str())
            .unwrap_or("keyword")
        {
            "semantic" => SearchType::Semantic,
            "hybrid" => SearchType::Hybrid,
            _ => SearchType::Keyword,
        };

        // Optional result filters (applied by keyword search and the FTS paths).
        let mut filters = SearchFilters::new();
        if let Some(nt) = args.get("note_type").and_then(|v| v.as_str())
            && let Ok(parsed) = NoteType::from_str(nt)
        {
            filters.note_type = Some(parsed);
        }
        if let Some(st) = args.get("status").and_then(|v| v.as_str())
            && let Ok(parsed) = NoteStatus::from_str(st)
        {
            filters.status = Some(parsed);
        }
        if let Some(min_p) = args.get("min_priority").and_then(serde_json::Value::as_i64) {
            filters.min_priority = Some(min_p as i32);
        }

        info!(
            "Searching context: {} (search_type: {:?})",
            query, search_type
        );

        let manager = self.state.context_manager();
        match manager
            .search_notes_with_type(query, top_k, search_type, &filters)
            .await
        {
            Ok(results) => {
                let formatted_results: Vec<ContextNoteResult> = results
                    .into_iter()
                    .map(|r| ContextNoteResult {
                        id: r.note.id.unwrap_or(0),
                        note_type: r.note.note_type.as_str().to_string(),
                        title: r.note.title,
                        content: Self::truncate_content(&r.note.content, max_content),
                        priority: r.note.priority as u32,
                        created_at: r.note.created_at.map(|d| d.to_string()).unwrap_or_default(),
                        relevance_score: Some(r.score),
                    })
                    .collect();

                let response = SearchContextResponse {
                    query: query.to_string(),
                    results_count: formatted_results.len(),
                    results: formatted_results,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Search context failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn get_related_context(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let entity_type = args
            .get("entity_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'entity_type' argument".into()))?;

        let max_content = args
            .get("max_content_length")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(300);

        // Optional narrowing of the entity link: exact name and/or file-path fragment.
        let entity_name = args.get("entity_name").and_then(|v| v.as_str());
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        info!(
            "Getting related context for: {} (name: {:?}, file: {:?})",
            entity_type, entity_name, file_path
        );

        let manager = self.state.context_manager();
        match manager
            .get_context_for_entity_filtered(entity_type, entity_name, file_path)
            .await
        {
            Ok(results) => {
                let formatted_results: Vec<ContextNoteResult> = results
                    .into_iter()
                    .map(|r| ContextNoteResult {
                        id: r.note.id.unwrap_or(0),
                        note_type: r.note.note_type.as_str().to_string(),
                        title: r.note.title,
                        content: Self::truncate_content(&r.note.content, max_content),
                        priority: r.note.priority as u32,
                        created_at: r.note.created_at.map(|d| d.to_string()).unwrap_or_default(),
                        relevance_score: Some(r.score),
                    })
                    .collect();

                let response = json!({
                    "entity_type": entity_type,
                    "entity_name": entity_name,
                    "file_path": file_path,
                    "results_count": formatted_results.len(),
                    "results": formatted_results,
                });

                ok_json(&response)
            }
            Err(e) => {
                warn!("Get related context failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn prune_context(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.unwrap_or_default();

        let max_age_days = args
            .get("max_age_days")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(90) as u32;
        let dry_run = args
            .get("dry_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        // Cap the number of retained active notes. Surplus notes (lowest priority,
        // least recently updated) are archived, never deleted.
        let max_notes = args
            .get("max_notes")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize);

        info!(
            "Pruning context (max_age: {} days, max_notes: {:?}, dry_run: {})",
            max_age_days, max_notes, dry_run
        );

        let manager = self.state.context_manager();

        let stats_result = manager.get_prune_statistics(max_age_days).await;
        let prune_result = manager
            .prune_notes_capped(max_age_days, max_notes, dry_run)
            .await;

        match prune_result {
            Ok((archived, deleted)) => {
                let stats =
                    stats_result.unwrap_or(devscriptor_core::context_graph::PruneStatistics {
                        total_notes: 0,
                        active_notes: 0,
                        archived_notes: 0,
                        superseded_notes: 0,
                        old_active_notes: archived,
                        old_archived_notes: deleted,
                        max_age_days,
                    });

                let response = PruneContextResponse {
                    archived_count: archived,
                    deleted_count: deleted,
                    would_archive: if dry_run { Some(archived) } else { None },
                    would_delete: if dry_run { Some(deleted) } else { None },
                    message: {
                        let cap = match max_notes {
                            Some(n) => format!(", keeping at most {n} active notes"),
                            None => String::new(),
                        };
                        if dry_run {
                            format!(
                                "DRY RUN: Would archive {archived} notes and delete {deleted} notes older than {max_age_days} days{cap}"
                            )
                        } else {
                            format!(
                                "Successfully archived {archived} notes and deleted {deleted} notes (max age {max_age_days} days{cap})"
                            )
                        }
                    },
                    statistics: Some(PruneStatistics {
                        total_notes: stats.total_notes,
                        active_notes: stats.active_notes,
                        archived_notes: stats.archived_notes,
                        superseded_notes: stats.superseded_notes,
                        old_active_notes: stats.old_active_notes,
                        old_archived_notes: stats.old_archived_notes,
                        max_age_days: stats.max_age_days,
                    }),
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Prune context failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn get_context_statistics(
        &self,
        _arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        info!("Getting context statistics");

        let manager = self.state.context_manager();
        match manager.list_notes(1000, 0).await {
            Ok(notes) => {
                use std::collections::HashMap;
                let mut by_type: HashMap<String, usize> = HashMap::new();
                let mut total_priority = 0u32;

                for note in &notes {
                    *by_type
                        .entry(note.note_type.as_str().to_string())
                        .or_insert(0) += 1;
                    total_priority += note.priority as u32;
                }

                let avg_priority = if notes.is_empty() {
                    5.0
                } else {
                    f64::from(total_priority) / notes.len() as f64
                };

                let response = json!({
                    "total_notes": notes.len(),
                    "notes_by_type": by_type,
                    "average_priority": avg_priority,
                });

                ok_json(&response)
            }
            Err(e) => {
                warn!("Get context statistics failed: {}", e);
                let response = json!({
                    "total_notes": 0,
                    "notes_by_type": {},
                    "average_priority": 5.0,
                    "error": format!("{}", e),
                });
                ok_json(&response)
            }
        }
    }
}
