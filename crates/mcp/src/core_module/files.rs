//! File description tool implementations

use devscriptor_core::utils::number_lines;
use serde_json::Value;
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::tools::{
    DescribeFileResponse, DescribeModificationResponse, EntityContext, EntitySummary,
    GetFileContextResponse,
};

/// Maximum number of entities for which a source snippet is produced, so a very
/// large file cannot blow up the tool response.
const MAX_ENTITY_CONTEXTS: usize = 50;

impl CoreModule {
    /// Build a source excerpt around each entity's definition line.
    ///
    /// Returns an empty vector when the file cannot be read (the entity graph is
    /// still useful without snippets). Lines are 1-indexed, matching the entity
    /// locations stored in the graph.
    async fn build_entity_contexts(
        file_path: &str,
        entities: &[devscriptor_core::entities::Entity],
        context_lines: usize,
    ) -> Vec<EntityContext> {
        let Ok(content) = Self::read_limited(file_path, 1024 * 1024).await else {
            return Vec::new();
        };
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return Vec::new();
        }

        entities
            .iter()
            .take(MAX_ENTITY_CONTEXTS)
            .filter_map(|entity| {
                let start_line = entity.location().start_line;
                // Entity lines are 1-indexed; a 0 means "unknown".
                let index = usize::try_from(start_line).ok()?.checked_sub(1)?;
                if index >= lines.len() {
                    return None;
                }
                let first = index.saturating_sub(context_lines);
                let last = (index + context_lines).min(lines.len() - 1);

                let snippet_start_line = u32::try_from(first + 1).unwrap_or(1);
                Some(EntityContext {
                    name: entity.name().to_string(),
                    entity_type: entity.entity_type().to_string(),
                    line: start_line,
                    snippet_start_line,
                    snippet: number_lines(&lines[first..=last].join("\n"), snippet_start_line),
                })
            })
            .collect()
    }

    pub(super) async fn describe_file(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'file_path' argument".into()))?;

        let include_content = args
            .get("include_content")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);

        // Confine file reads to the project. See `resolve_readable_path`.
        let path_buf = self.resolve_readable_path(file_path).await?;
        let file_path = path_buf.to_str().unwrap_or(file_path);
        info!("Describing file: {}", file_path);

        match self.state.database().get_entities_by_file(file_path).await {
            Ok(entities) => {
                let entity_summaries: Vec<EntitySummary> = entities
                    .iter()
                    .map(|e| EntitySummary {
                        name: e.name().to_string(),
                        entity_type: e.entity_type().to_string(),
                        line: e.location().start_line,
                    })
                    .collect();

                let content_preview = if include_content {
                    Self::read_limited(file_path, 64 * 1024).await.ok()
                } else {
                    None
                };
                let total_lines = Self::count_lines(file_path).await.ok();

                let response = DescribeFileResponse {
                    file_path: file_path.to_string(),
                    language: std::path::Path::new(file_path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    total_lines,
                    entity_count: entity_summaries.len(),
                    entities: entity_summaries,
                    content_preview: content_preview
                        .map(|c| number_lines(&Self::truncate_content(&c, 1000), 1)),
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Describe file failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn describe_modification(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'file_path' argument".into()))?;

        let original = args
            .get("original_content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidParams("Missing 'original_content' argument".into())
            })?;

        let modified = args
            .get("modified_content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidParams("Missing 'modified_content' argument".into())
            })?;

        // Confine file reads to the project. See `resolve_readable_path`.
        let path_buf = self.resolve_readable_path(file_path).await?;
        let file_path = path_buf.to_str().unwrap_or(file_path);
        info!("Describing modification: {}", file_path);

        // --- Line-level diff using `similar` (MCP-3) ---
        use similar::{ChangeTag, TextDiff};

        let diff = TextDiff::from_lines(original, modified);
        let mut lines_added = 0usize;
        let mut lines_removed = 0usize;
        let mut unified_diff = String::new();

        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => {
                    lines_added += 1;
                    unified_diff.push('+');
                    unified_diff.push_str(change.value());
                }
                ChangeTag::Delete => {
                    lines_removed += 1;
                    unified_diff.push('-');
                    unified_diff.push_str(change.value());
                }
                ChangeTag::Equal => {
                    unified_diff.push(' ');
                    unified_diff.push_str(change.value());
                }
            }
        }

        // --- Entity detection (line heuristic + AST second pass) ---
        let mut entities_added = Vec::new();
        let mut entities_removed = Vec::new();
        let mut entities_modified = Vec::new();

        // 1. Line heuristic: scan for definition-like lines
        fn extract_line_entities(content: &str) -> Vec<String> {
            let mut out = Vec::new();
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(name) = try_extract_definition_name(trimmed) {
                    out.push(name.to_string());
                }
            }
            out
        }

        fn try_extract_definition_name(line: &str) -> Option<&str> {
            const PREFIXES: &[&str] = &["fn ", "def ", "function ", "class ", "struct ", "enum "];
            for prefix in PREFIXES {
                if let Some(rest) = line.strip_prefix(prefix)
                    && let Some(name) = rest.split_whitespace().next()
                {
                    let cleaned = name.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                    if !cleaned.is_empty() {
                        return Some(cleaned);
                    }
                }
            }
            if line.contains(" class ")
                && let Some(name) = line.split_whitespace().nth(2)
            {
                let cleaned = name.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
            None
        }

        let original_entities = extract_line_entities(original);
        let modified_entities = extract_line_entities(modified);

        for e in &modified_entities {
            if !original_entities.contains(e) {
                entities_added.push(e.clone());
            }
        }
        for e in &original_entities {
            if !modified_entities.contains(e) {
                entities_removed.push(e.clone());
            }
        }
        for e in &original_entities {
            if modified_entities.contains(e)
                && original.contains(e)
                && modified.contains(e)
                && original != modified
            {
                // Only flag as "modified" if the entity appears in both but overall file changed
                entities_modified.push(e.clone());
            }
        }
        entities_modified.dedup();
        entities_modified.retain(|e| !entities_added.contains(e) && !entities_removed.contains(e));

        // 2. AST-aware second pass (best-effort)
        Self::ast_aware_entity_diff(
            original,
            modified,
            file_path,
            &mut entities_added,
            &mut entities_removed,
        );

        let response = DescribeModificationResponse {
            file_path: file_path.to_string(),
            changes_summary: format!(
                "Modified {file_path}: +{lines_added} lines, -{lines_removed} lines"
            ),
            entities_added,
            entities_removed,
            entities_modified,
            lines_added,
            lines_removed,
        };

        ok_json(&response)
    }

    /// AST-aware entity diff as a second pass (MCP-3).
    ///
    /// Parses both versions with `TreeSitterParser` and compares entity
    /// lists for higher accuracy than the line heuristic alone.
    fn ast_aware_entity_diff(
        original: &str,
        modified: &str,
        file_path: &str,
        entities_added: &mut Vec<String>,
        entities_removed: &mut Vec<String>,
    ) {
        use devscriptor_core::parser::parse_content_sync;
        use std::collections::HashSet;
        use std::path::Path;

        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let language = match ext {
            "rs" => "rust",
            "py" => "python",
            "js" => "javascript",
            "ts" => "typescript",
            "go" => "go",
            "java" => "java",
            "cpp" | "cc" | "cxx" | "hpp" => "cpp",
            "c" | "h" => "c",
            _ => return, // unsupported — skip AST pass
        };

        let original_parsed = parse_content_sync(original, language, Path::new(file_path));
        let modified_parsed = parse_content_sync(modified, language, Path::new(file_path));

        let old_names: HashSet<String> = match &original_parsed {
            Ok(pf) => pf.entities.iter().map(|e| e.name().to_string()).collect(),
            Err(_) => return,
        };
        let new_names: HashSet<String> = match &modified_parsed {
            Ok(pf) => pf.entities.iter().map(|e| e.name().to_string()).collect(),
            Err(_) => return,
        };

        for name in &new_names {
            if !old_names.contains(name) && !entities_added.contains(name) {
                entities_added.push(name.clone());
            }
        }
        for name in &old_names {
            if !new_names.contains(name) && !entities_removed.contains(name) {
                entities_removed.push(name.clone());
            }
        }
    }

    pub(super) async fn get_file_context(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'file_path' argument".into()))?;

        // Number of source lines to include on either side of each entity's
        // definition line. `0` returns just the definition line itself.
        let context_lines = args
            .get("context_lines")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(10);

        // Resolved as `describe_file` resolves it: a relative path names a file
        // in the project, and the index stores absolute paths. Queried as given,
        // a relative path found nothing and answered "0 entities".
        let path_buf = self.resolve_readable_path(file_path).await?;
        let file_path = path_buf.to_str().unwrap_or(file_path);
        info!(
            "Getting file context: {} (context_lines: {})",
            file_path, context_lines
        );

        match self.state.database().get_entities_by_file(file_path).await {
            Ok(entities) => {
                let imports: Vec<String> = entities
                    .iter()
                    .filter_map(|e| match e {
                        devscriptor_core::entities::Entity::Function(f) => Some(f.name.clone()),
                        devscriptor_core::entities::Entity::Class(c) => Some(c.name.clone()),
                        _ => None,
                    })
                    .collect();

                let entity_contexts =
                    Self::build_entity_contexts(file_path, &entities, context_lines).await;

                let response = GetFileContextResponse {
                    file_path: file_path.to_string(),
                    imports: imports.into_iter().take(20).collect(),
                    dependencies: vec![],
                    related_files: vec![],
                    context_summary: format!("File contains {} entities", entities.len()),
                    entity_contexts,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Get file context failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }
}
