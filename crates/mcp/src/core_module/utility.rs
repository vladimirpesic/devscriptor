//! Utility tool implementations: health check, validate edit, query with context

use serde_json::{Value, json};
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};

impl CoreModule {
    pub(super) async fn health_check(
        &self,
        _arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        info!("Performing health check");

        let mut health = self.state.get_health_status();
        if let Some(pid) = crate::server::daemon_pid() {
            health["daemon_pid"] = json!(pid);
        }

        ok_json(&health)
    }

    pub(super) async fn validate_file_edit(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'file_path' argument".into()))?;

        info!("Validating file edit: {}", file_path);

        // Stale-content detection, from the optional `original_content`.
        if let Some(original_content) = args.get("original_content").and_then(|v| v.as_str()) {
            // Confine the read to the project. See `resolve_readable_path`.
            let path_buf = self.resolve_readable_path(file_path).await?;
            let current_content = match tokio::fs::read_to_string(&path_buf).await {
                Ok(content) => content,
                Err(e) => {
                    return Ok(ToolResult::Error(format!(
                        "Cannot read file '{file_path}' for validation: {e}"
                    )));
                }
            };

            // Direct comparison — exact, no collision risk, no extra deps.
            if original_content != current_content {
                let response = json!({
                    "allowed": false,
                    "stale": true,
                    "message": format!(
                        "File '{}' has been modified externally since you read it. \
                         Please re-read the file and retry.",
                        file_path
                    )
                });
                return Ok(ToolResult::Success(response));
            }
        }

        let manager = self.state.context_manager();
        let results = manager.search_notes(file_path, 10).await.map_err(|e| {
            ToolError::Internal(format!("Context search failed for '{file_path}': {e}"))
        })?;

        let file_notes: Vec<_> = results
            .into_iter()
            .filter(|r| {
                matches!(
                    r.note.note_type,
                    devscriptor_core::context_graph::NoteType::FileDescription
                )
            })
            .collect();

        if !file_notes.is_empty() {
            let note = &file_notes[0];
            let response = json!({
                "allowed": true,
                "is_first_time": false,
                "existing_context": {
                    "file_path": note.note.structured_data.get("file_path").and_then(|v| v.as_str()).unwrap_or(file_path),
                    "description": note.note.summary,
                    "last_updated": note.note.updated_at.map(|d| d.to_string()).unwrap_or_default(),
                }
            });

            return Ok(ToolResult::Success(response));
        }

        let response = json!({
            "allowed": false,
            "is_first_time": true,
            "required_action": "describe_file",
            "message": format!("File '{}' needs to be described first. Call describe_file tool to add context.", file_path)
        });

        Ok(ToolResult::Success(response))
    }

    pub(super) async fn query_with_context(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' argument".into()))?;

        let entity_type = args.get("entity_type").and_then(|v| v.as_str());
        let file_path = args.get("file_path").and_then(|v| v.as_str());
        let force_broad_search = args
            .get("force_broad_search")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        info!("Querying with context: {}", query);

        if !force_broad_search {
            let manager = self.state.context_manager();
            match manager.search_notes(query, 10).await {
                Ok(context_results) if !context_results.is_empty() => {
                    let code_pointers: Vec<serde_json::Value> = context_results.iter().filter_map(|r| {
                        let fp = r.note.structured_data.get("file_path").and_then(|v| v.as_str())?;
                        Some(json!({
                            "file_path": fp,
                            "entity_name": r.note.title,
                            "entity_type": r.note.structured_data.get("entity_type").and_then(|v| v.as_str()).unwrap_or("unknown"),
                            "relevance_score": r.score,
                            "note_content": r.note.content.chars().take(200).collect::<String>() + "...",
                        }))
                    }).collect();

                    if !code_pointers.is_empty() {
                        let response = json!({
                            "source": "context_guided",
                            "entities": [],
                            "code_pointers": code_pointers,
                            "context_notes_found": context_results.len(),
                        });

                        return ok_json(&response);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("Context search failed for '{}': {}", query, e);
                    return Err(ToolError::Internal(format!(
                        "Context search failed for '{query}': {e}"
                    )));
                }
            }
        }

        // No note points at code for the query: search the code itself. The
        // query is a question or a phrase as often as a name, so it goes to the
        // hybrid retriever — keyword (BM25) and meaning together, keyword only
        // without embeddings — rather than being matched as one name: a
        // several-word query matched no entity name, and this path answered
        // nothing for exactly the queries it is for.
        let limit = 10;
        // A path filter narrows the results after ranking; ask for more so a
        // narrow filter still has some to keep.
        let candidates = if file_path.is_some() {
            limit * 5
        } else {
            limit
        };
        let results = self
            .state
            .searcher()
            .semantic_search(query, entity_type.unwrap_or(""), candidates)
            .await
            .map_err(|e| ToolError::Internal(format!("Search failed for '{query}': {e}")))?;
        let results: Vec<_> = results
            .into_iter()
            .filter(|r| file_path.is_none_or(|fp| r.file_path.contains(fp)))
            .take(limit)
            .collect();
        let ids: Vec<_> = results.iter().map(|r| r.id).collect();
        let lines = self
            .state
            .database()
            .get_entity_start_lines(&ids)
            .await
            .map_err(|e| ToolError::Internal(format!("Search failed for '{query}': {e}")))?;
        let filtered_entities: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "entity_type": r.entity_type,
                    "file_path": r.file_path,
                    "line": lines.get(&r.id),
                    "relevance": r.similarity,
                })
            })
            .collect();
        let keyword_only = results.iter().any(|r| r.fallback);

        let mut response = json!({
            "source": "broad_search",
            "entities": filtered_entities,
            "code_pointers": [],
            "total_results": filtered_entities.len(),
        });
        if keyword_only {
            response["message"] = json!(
                "Keyword ranking only: embeddings are unavailable, so results match words, not meaning."
            );
        }

        ok_json(&response)
    }
}
