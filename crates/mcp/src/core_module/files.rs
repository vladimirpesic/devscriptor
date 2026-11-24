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
/// The most imports `get_file_context` lists.
const FILE_CONTEXT_IMPORTS: usize = 100;
/// The most files `get_file_context` lists of each kind.
const FILE_CONTEXT_FILES: usize = 50;

/// An import as `get_file_context` lists it: what it binds and where from.
fn describe_import(import: &devscriptor_core::parser::ImportInfo) -> String {
    let module = &import.source_module;
    let local = &import.local_name;
    if import.is_wildcard {
        return format!("* from {module}");
    }
    match &import.original_name {
        Some(name) if name != local => format!("{name} as {local} from {module}"),
        Some(name) => format!("{name} from {module}"),
        None if local == module => module.clone(),
        None => format!("{module} as {local}"),
    }
}

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

        // --- Entity changes: parsed where the language is one Devscriptor
        // reads, by definition lines otherwise ---
        let (entities_added, entities_removed, entities_modified) =
            entity_changes(original, modified, file_path)
                .unwrap_or_else(|| definition_line_changes(original, modified));

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

        let database = self.state.database();
        match database.get_entities_by_file(file_path).await {
            Ok(entities) => {
                // The file's imports as the parser read them, and the files the
                // graph connects it to — not its own definitions.
                let imports: Vec<String> = database
                    .get_file_imports(&[file_path.to_string()])
                    .await
                    .map_err(|e| ToolError::Internal(e.to_string()))?
                    .remove(file_path)
                    .unwrap_or_default()
                    .iter()
                    .take(FILE_CONTEXT_IMPORTS)
                    .map(describe_import)
                    .collect();
                let connected = database
                    .get_connected_files(file_path, FILE_CONTEXT_FILES)
                    .await
                    .map_err(|e| ToolError::Internal(e.to_string()))?;

                let entity_contexts =
                    Self::build_entity_contexts(file_path, &entities, context_lines).await;

                let response = GetFileContextResponse {
                    file_path: file_path.to_string(),
                    imports,
                    dependencies: connected.depends_on,
                    related_files: connected.depended_on_by,
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

/// Changes `describe_modification` reports: entities added, removed and
/// modified, each named once.
type EntityChanges = (Vec<String>, Vec<String>, Vec<String>);

/// The entity changes between two versions of `file_path`, parsed: entities
/// are matched by identity — kind, enclosing scope and name, and parameters
/// for overloads — and one is modified when its own text changed. An entity
/// whose text is the same in both is unchanged, whatever else changed, and
/// a renamed one is removed under its old name and added under its new.
/// `None` when the file's language is not one Devscriptor parses, or either
/// version does not parse.
fn entity_changes(original: &str, modified: &str, file_path: &str) -> Option<EntityChanges> {
    use devscriptor_core::parser::{language_config, parse_content_sync};
    let path = std::path::Path::new(file_path);
    let name = path.file_name()?.to_str()?;
    let language = language_config::detect_language_from_filename(name)
        .or_else(|| language_config::filename_to_language(&name.to_lowercase()))?;
    let entities = |content: &str| {
        parse_content_sync(content, &language, path)
            .ok()
            .map(|parsed| entity_texts(&parsed.entities, content))
    };
    let (before, after) = (entities(original)?, entities(modified)?);
    let lookup = |entries: &[(EntityKey, String, Vec<String>)], key: &EntityKey| {
        entries
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, _, texts)| texts.clone())
    };
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (key, label, texts) in &after {
        match lookup(&before, key) {
            None => added.push(label.clone()),
            Some(old) if old != *texts => changed.push(label.clone()),
            Some(_) => {}
        }
    }
    let removed = before
        .iter()
        .filter(|(key, _, _)| lookup(&after, key).is_none())
        .map(|(_, label, _)| label.clone())
        .collect();
    Some((added, removed, changed))
}

/// What identifies an entity across two versions of its file.
type EntityKey = (&'static str, String, String);

/// Each distinct entity of `entities`, parsed from `content`, in order of
/// appearance: its key, its label (`Class.method` for a member) and its
/// text, sorted, so overloads compare as a set. The file's own module, whose
/// text is the whole file, is left out: it changes with any change.
fn entity_texts(
    entities: &[devscriptor_core::entities::Entity],
    content: &str,
) -> Vec<(EntityKey, String, Vec<String>)> {
    use devscriptor_core::entities::Entity;
    let mut out: Vec<(EntityKey, String, Vec<String>)> = Vec::new();
    for entity in entities {
        if matches!(entity, Entity::Module(_)) && entity.body().trim() == content.trim() {
            continue;
        }
        let key = (
            entity.entity_type(),
            entity.identity_disambiguator(),
            entity.name().to_string(),
        );
        let label = match entity {
            Entity::Function(function) => match function
                .parent_class
                .as_deref()
                .or(function.parent_function.as_deref())
            {
                Some(parent) => format!("{parent}.{}", function.name),
                None => function.name.clone(),
            },
            _ => entity.name().to_string(),
        };
        match out.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, texts)) => {
                texts.push(entity.body().to_string());
                texts.sort();
            }
            None => out.push((key, label, vec![entity.body().to_string()])),
        }
    }
    out
}

/// For a language Devscriptor does not parse: the names defined on
/// definition-like lines (`fn`, `def`, `function`, `class`, `struct`, `enum`)
/// that one version has and the other lacks. Whether a definition present in
/// both changed cannot be told this way, so none is reported modified.
fn definition_line_changes(original: &str, modified: &str) -> EntityChanges {
    fn definition_name(line: &str) -> Option<&str> {
        const PREFIXES: &[&str] = &["fn ", "def ", "function ", "class ", "struct ", "enum "];
        let line = line.trim();
        let rest = PREFIXES
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))?;
        let name = rest
            .split_whitespace()
            .next()?
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        (!name.is_empty()).then_some(name)
    }
    let names = |content: &str| {
        let mut names: Vec<String> = Vec::new();
        for name in content.lines().filter_map(definition_name) {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
        names
    };
    let (before, after) = (names(original), names(modified));
    let added = after
        .iter()
        .filter(|n| !before.contains(n))
        .cloned()
        .collect();
    let removed = before
        .iter()
        .filter(|n| !after.contains(n))
        .cloned()
        .collect();
    (added, removed, Vec::new())
}
