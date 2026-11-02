//! LSP Tool Handlers (38 tools) — only compiled with `lsp` feature.

use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::json;

use devscriptor_lsp::bridge::Translator;
use devscriptor_lsp::mcp::tools::{
    CallHierarchyItem, CallHierarchyParams, CodeAction, CodeActionResolveParams, CodeActionsParams,
    Completion, CompletionResolveParams, CompletionsParams, DefinitionParams, DiagnosticsParams,
    DocumentColorsParams, DocumentHighlightsParams, DocumentLinksParams, DocumentSymbolsParams,
    FoldingRangesParams, FormatDocumentParams, FormatRangeParams, HoverParams,
    ImplementationParams, IncomingCallsParams, InlayHintsParams, LinkedEditingRangeParams,
    OutgoingCallsParams, Position2D, ProjectFilesParams, ProjectSymbolsParams, Range,
    ReferencesParams, RenameParams, RestartServerParams, SelectionRangeParams,
    SemanticTokensParams, ServerCapabilitiesParams, ServerProjectsParams, ServerStatusParams,
    StartServerParams, StopServerParams, SubtypesParams, SupertypesParams, SymbolsParams,
    TypeDefinitionParams, TypeHierarchyItem, TypeHierarchyParams, WorkspaceSymbolParams,
};

use super::UnifiedMcpServer;

pub(super) fn create_schema(
    obj: serde_json::Value,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    match obj {
        serde_json::Value::Object(map) => std::sync::Arc::new(map),
        _ => std::sync::Arc::new(serde_json::Map::new()),
    }
}

/// Input schema for a position-based LSP tool.
///
/// Every such tool accepts a symbol `name` (resolved from the code index) as an
/// alternative to raw `line`/`character` coordinates — the difference between an
/// agent naming `apply_discount` and one guessing its 0-indexed column. `extra`
/// properties (e.g. `include_declaration`) are merged in. Nothing is strictly
/// required: the handler resolves a `name` when no explicit position is given,
/// and errors clearly when it has neither.
pub(super) fn position_schema(
    extra: serde_json::Value,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let mut properties = json!({
        "name": {
            "type": "string",
            "description": "Symbol to resolve from the code index (function, class, method, …) — use instead of line/character. Requires the project to have been scanned."
        },
        "entity_type": {
            "type": "string",
            "description": "Optional disambiguator for `name`: function, class, method, struct, etc."
        },
        "file_path": {
            "type": "string",
            "description": "Source file. Required alongside line/character; optional with `name`, where it disambiguates a name defined in several files."
        },
        "line": {"type": "integer", "description": "0-indexed line. Provide with `character`, or omit and pass `name`."},
        "character": {"type": "integer", "description": "0-indexed column on `line`, in UTF-16 code units (the LSP default): one per character, two for a character outside the Basic Multilingual Plane such as an emoji."}
    });
    if let (Some(dst), Some(src)) = (properties.as_object_mut(), extra.as_object()) {
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    create_schema(json!({
        "type": "object",
        "properties": properties,
        // `name` OR `line`+`character`. That is a disjunction JSON-Schema
        // expresses only through `oneOf`, which many MCP clients validate
        // poorly, so both stay optional and the handler enforces the choice.
        "required": []
    }))
}

/// Input schema for a tool that continues a call or type hierarchy: the `item`
/// that `prepared_by` returned, or — as [`position_schema`] takes it — the
/// symbol itself, for which the hierarchy is then prepared.
pub(super) fn hierarchy_item_schema(
    prepared_by: &str,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let position = json!({
        "type": "object",
        "properties": {"line": {"type": "integer"}, "character": {"type": "integer"}},
        "required": ["line", "character"]
    });
    position_schema(json!({
        "item": {
            "type": "object",
            // `data` is left out of `properties`: the server's own payload has
            // no one type to declare, and every property here has one. An item
            // passed back unchanged carries it all the same.
            "description": format!(
                "One of `{prepared_by}`'s `items`, passed back unchanged, its `data` \
                 included. Or leave it out and name the symbol instead: `name`, or \
                 `file_path` with `line` and `character`."
            ),
            "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string"},
                "location": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "range": {
                            "type": "object",
                            "properties": {"start": position, "end": position},
                            "required": ["start", "end"]
                        }
                    },
                    "required": ["file_path", "range"]
                },
                "selection_range": {
                    "type": "object",
                    "description": "The range of the symbol's name, as received.",
                    "properties": {"start": position, "end": position}
                },
                "container_name": {"type": "string"}
            },
            "required": ["name", "kind", "location"]
        }
    }))
}

/// The error for an `item` that is not the one the preparing call returned.
#[cfg(feature = "lsp")]
fn invalid_item(error: &serde_json::Error, prepared_by: &str) -> McpError {
    McpError::invalid_params(
        format!(
            "invalid item: {error}. Pass back one of `{prepared_by}`'s `items` unchanged — \
             {{name, kind, location: {{file_path, range: {{start: {{line, character}}, end: \
             {{line, character}}}}}}}} — or leave `item` out and name the symbol: `name`, or \
             `file_path` with `line` and `character`."
        ),
        None,
    )
}

/// The error for a position the preparing call found no item at.
#[cfg(feature = "lsp")]
fn no_item_here() -> McpError {
    McpError::invalid_params(
        "no hierarchy item at that position: name the symbol with `name`, or give the \
         position of its name",
        None,
    )
}

/// A tool result as JSON, or the error the call ended in.
#[cfg(feature = "lsp")]
/// The identifier at the 0-based `line` and `character` of `file`, the
/// character counted in UTF-16 code units, as a language server counts it.
fn identifier_at(file: &std::path::Path, line: u32, character: u32) -> Option<String> {
    let content = std::fs::read_to_string(file).ok()?;
    let chars: Vec<char> = content.lines().nth(line as usize)?.chars().collect();
    let mut units = 0;
    let at = chars
        .iter()
        .position(|c| {
            let here = units >= character as usize;
            units += c.len_utf16();
            here
        })
        .unwrap_or(chars.len());
    let in_word = |c: &char| c.is_alphanumeric() || matches!(c, '_' | '$');
    let start = chars[..at]
        .iter()
        .rposition(|c| !in_word(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| !in_word(c))
        .map_or(chars.len(), |i| at + i);
    (start < end).then(|| chars[start..end].iter().collect())
}

fn json_result<T: serde::Serialize>(
    result: devscriptor_lsp::error::Result<T>,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => Ok(CallToolResult::success(vec![
            ContentBlock::json(value).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ])),
        Err(e) => Err(McpError::internal_error(e.to_string(), None)),
    }
}

/// Characters that are part of an identifier token.
#[cfg(feature = "lsp")]
fn is_identifier_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The byte offset of `name` as a standalone identifier in `line`, at or after
/// the byte offset `floor`. Rejects matches that are substrings of a larger
/// identifier (so `apply_discount` does not match inside `apply_discount_v2`).
#[cfg(feature = "lsp")]
fn identifier_column(line: &str, name: &str, floor: usize) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let mut from = floor.min(line.len());
    while let Some(rel) = line.get(from..).and_then(|rest| rest.find(name)) {
        let at = from + rel;
        let after = at + name.len();
        let before_ok = !line[..at]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char);
        let after_ok = !line[after..].chars().next().is_some_and(is_identifier_char);
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + line[at..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// The LSP column of the byte offset `byte` in `line`: LSP positions count
/// UTF-16 code units (the only encoding Devscriptor offers servers), while
/// the code index and [`identifier_column`] count bytes. They differ on any
/// line with non-ASCII text before the column.
#[cfg(feature = "lsp")]
fn utf16_column(line: &str, byte: usize) -> usize {
    line.char_indices()
        .take_while(|&(i, _)| i < byte)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

/// A source file's text, read as given or relative to the working directory.
#[cfg(feature = "lsp")]
fn read_source(file_path: &str) -> Option<String> {
    std::fs::read_to_string(file_path)
        .ok()
        .or_else(|| std::fs::read_to_string(std::env::current_dir().ok()?.join(file_path)).ok())
}

/// How many lines past a definition's start to scan for its identifier, covering
/// decorators and multi-line signatures.
#[cfg(feature = "lsp")]
const IDENTIFIER_SCAN_LINES: usize = 4;

/// Locate the identifier `name` near a definition's start, returning its
/// 0-indexed LSP `(line, character)`.
///
/// The Context Graph records a definition's *start* — the `def`/`fn`/`class`
/// keyword — not its identifier, so raw coordinates land off the symbol. Scan
/// the start line (from the keyword column, a byte offset, onward) and a few
/// following lines for the identifier itself, which is where LSP servers
/// answer.
#[cfg(feature = "lsp")]
fn locate_identifier(
    file_path: &str,
    start_line: u32,
    start_column: u32,
    name: &str,
) -> Option<(u32, u32)> {
    let content = read_source(file_path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = start_line as usize;
    for offset in 0..IDENTIFIER_SCAN_LINES {
        let Some(line) = lines.get(start + offset) else {
            break;
        };
        let floor = if offset == 0 {
            start_column as usize
        } else {
            0
        };
        if let Some(column) = identifier_column(line, name, floor) {
            return Some(((start + offset) as u32, utf16_column(line, column) as u32));
        }
    }
    None
}

/// The definition start itself, as an LSP position, for when its identifier
/// is not found nearby. The stored column is a byte offset, converted against
/// the line when the file can be read.
#[cfg(feature = "lsp")]
fn definition_start(file_path: &str, start_line: u32, start_column: u32) -> (u32, u32) {
    let column = read_source(file_path)
        .and_then(|content| {
            let line = content.lines().nth(start_line as usize)?;
            u32::try_from(utf16_column(line, start_column as usize)).ok()
        })
        .unwrap_or(start_column);
    (start_line, column)
}

/// Whether a stored file path satisfies a caller's file hint — exact, a path
/// suffix, or the same file name.
#[cfg(feature = "lsp")]
fn path_hint_matches(stored: &str, hint: &str) -> bool {
    fn base(p: &str) -> &str {
        p.rsplit(['/', '\\']).next().unwrap_or(p)
    }
    stored == hint || stored.ends_with(hint) || base(stored) == base(hint)
}

/// Choose the best indexed entity for a symbol name: a file-path hint wins, then
/// a type match, then the first candidate.
#[cfg(feature = "lsp")]
fn pick_symbol<'a>(
    candidates: &'a [devscriptor_core::entities::Entity],
    entity_type: Option<&str>,
    file_hint: Option<&str>,
) -> Option<&'a devscriptor_core::entities::Entity> {
    let type_ok = |e: &devscriptor_core::entities::Entity| {
        entity_type.is_none_or(|t| e.entity_type().eq_ignore_ascii_case(t))
    };
    if let Some(hint) = file_hint
        && let Some(entity) = candidates
            .iter()
            .find(|e| type_ok(e) && path_hint_matches(e.file_path(), hint))
    {
        return Some(entity);
    }
    candidates
        .iter()
        .find(|e| type_ok(e))
        .or_else(|| candidates.first())
}

/// The LSP position of the indexed symbol `name`: its file, and the 0-indexed
/// line and UTF-16 column of its identifier. `entity_type` and `file_hint`
/// choose between same-named symbols ([`pick_symbol`]).
///
/// The candidates are read from an index current for their files
/// ([`crate::freshness::fresh_answer`]): the language server reads the file
/// on disk, and a position the index took from an older version of it points
/// at the wrong line.
#[cfg(feature = "lsp")]
async fn symbol_position_by_name(
    state: &crate::server::DevscriptorServerState,
    name: &str,
    entity_type: Option<&str>,
    file_hint: Option<&str>,
) -> Result<(String, u32, u32), McpError> {
    let (candidates, _) = crate::freshness::fresh_answer(state, || async move {
        let candidates = state
            .database()
            .get_entities_by_exact_name(name, 50)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("symbol lookup failed for '{name}': {e}"), None)
            })?;
        let files = candidates
            .iter()
            .map(|candidate| candidate.file_path().to_string())
            .collect();
        Ok((candidates, files))
    })
    .await?;

    let chosen = pick_symbol(&candidates, entity_type, file_hint).ok_or_else(|| {
        let of_type = entity_type
            .map(|t| format!(" of type '{t}'"))
            .unwrap_or_default();
        McpError::internal_error(
            format!(
                "no indexed symbol named '{name}'{of_type}. Run scan_codebase first, or pass explicit line/character."
            ),
            None,
        )
    })?;

    let file_path = chosen.file_path().to_string();
    let location = chosen.location();
    // The Context Graph stores 1-indexed lines (`start.row + 1` in the
    // parser); LSP positions are 0-indexed. `start_column` is already
    // 0-indexed, but counts bytes; both helpers return UTF-16 columns.
    let zero_indexed_line = location.start_line.saturating_sub(1);
    let (line, character) =
        locate_identifier(&file_path, zero_indexed_line, location.start_column, name)
            .unwrap_or_else(|| {
                definition_start(&file_path, zero_indexed_line, location.start_column)
            });
    Ok((file_path, line, character))
}

impl UnifiedMcpServer {
    pub(super) async fn handle_lsp_tool(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let translator = match &self.translator {
            Some(t) => t.clone(),
            None => {
                return Err(McpError::internal_error("LSP not initialized", None));
            }
        };

        match name {
            // Navigation
            "get_hover" => self.get_hover_impl(&translator, args).await,
            "get_definition" => self.get_definition_impl(&translator, args).await,
            "get_references" => self.get_references_impl(&translator, args).await,
            "get_type_definitions" => self.get_type_definition_impl(&translator, args).await,
            "get_implementations" => self.get_implementations_impl(&translator, args).await,
            // Completion
            "get_completions" => self.get_completions_impl(&translator, args).await,
            "get_completion_resolve" => self.get_completion_resolve_impl(&translator, args).await,
            // Diagnostics & Actions
            "get_diagnostics" => self.get_diagnostics_impl(&translator, args).await,
            "get_code_actions" => self.get_code_actions_impl(&translator, args).await,
            "get_code_action_resolve" => self.get_code_action_resolve_impl(&translator, args).await,
            // Symbols
            "get_document_symbols" => self.get_document_symbols_impl(&translator, args).await,
            "get_workspace_symbols" => self.get_workspace_symbols_impl(&translator, args).await,
            "get_symbols" => self.get_symbols_impl(&translator, args).await,
            "get_project_symbols" => self.get_project_symbols_impl(&translator, args).await,
            // Call Hierarchy
            "get_call_hierarchy" => self.get_call_hierarchy_impl(&translator, args).await,
            "get_incoming_calls" => self.get_incoming_calls_impl(&translator, args).await,
            "get_outgoing_calls" => self.get_outgoing_calls_impl(&translator, args).await,
            // Type Hierarchy
            "get_type_hierarchy" => self.get_type_hierarchy_impl(&translator, args).await,
            "get_subtypes" => self.get_subtypes_impl(&translator, args).await,
            "get_supertypes" => self.get_supertypes_impl(&translator, args).await,
            // Document Features
            "get_highlights" => self.get_highlights_impl(&translator, args).await,
            "get_colors" => self.get_colors_impl(&translator, args).await,
            "get_folding_ranges" => self.get_folding_ranges_impl(&translator, args).await,
            "get_selection_range" => self.get_selection_range_impl(&translator, args).await,
            "get_linked_editing_range" => {
                self.get_linked_editing_range_impl(&translator, args).await
            }
            "get_inlay_hints" => self.get_inlay_hints_impl(&translator, args).await,
            "get_links" => self.get_links_impl(&translator, args).await,
            "get_semantic_tokens" => self.get_semantic_tokens_impl(&translator, args).await,
            // Formatting
            "format_document" => self.format_document_impl(&translator, args).await,
            "get_range_format" => self.get_range_format_impl(&translator, args).await,
            "rename_symbol" => self.rename_symbol_impl(&translator, args).await,
            // Server Management
            "get_server_status" => self.get_server_status_impl(&translator, args).await,
            "get_server_capabilities" => self.get_server_capabilities_impl(&translator, args).await,
            "get_server_projects" => self.get_server_projects_impl(&translator, args).await,
            "get_project_files" => self.get_project_files_impl(&translator, args).await,
            "start_server" => self.start_server_impl(&translator, args).await,
            "stop_server" => self.stop_server_impl(&translator, args).await,
            "restart_server" => self.restart_server_impl(&translator, args).await,
            _ => Err(McpError::internal_error(
                format!("Unknown LSP tool: {name}"),
                None,
            )),
        }
    }

    /// Resolve a symbol `name` in a position-based request into concrete
    /// `file_path`/`line`/`character`, injected back into `args`.
    ///
    /// A no-op when no `name` is given or an explicit position is already
    /// present, so coordinate-based calls behave as they do without it. This is
    /// the name-addressability the position-based LSP tools share: an agent asks
    /// for `apply_discount` instead of computing its 0-indexed column, which is
    /// easy to get wrong.
    #[cfg(feature = "lsp")]
    async fn resolve_symbol_args(
        &self,
        args: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), McpError> {
        let Some(name) = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            return Ok(());
        };
        // An explicit position always wins over name resolution.
        if args
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .is_some()
            && args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .is_some()
        {
            return Ok(());
        }

        let entity_type = args
            .get("entity_type")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let file_hint = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let (file_path, line, character) = symbol_position_by_name(
            self.core.state(),
            &name,
            entity_type.as_deref(),
            file_hint.as_deref(),
        )
        .await?;
        args.insert("file_path".into(), serde_json::Value::String(file_path));
        args.insert("line".into(), json!(line));
        args.insert("character".into(), json!(character));
        Ok(())
    }

    // --- Navigation (5 tools) ---

    lsp_handler!(
        get_hover_impl, handle_hover, HoverParams,
        text: contents,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let line = args.get("line").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
            let character = args.get("character").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
            HoverParams { file_path, line, character }
        }
    );

    lsp_handler!(
        get_definition_impl,
        handle_definition,
        DefinitionParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            DefinitionParams {
                file_path,
                line,
                character,
            }
        }
    );

    /// `get_references`, after opening in its server the files the index says
    /// use the symbol ([`Self::open_files_using`]).
    async fn get_references_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = args;
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        self.resolve_symbol_args(&mut args).await?;
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let line = args
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let character = args
            .get("character")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let include_declaration = args
            .get("include_declaration")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let name =
            name.or_else(|| identifier_at(std::path::Path::new(&file_path), line, character));
        if let Some(name) = &name {
            self.open_files_using(translator, name, &file_path).await;
        }
        let mut result = translator
            .handle_references(ReferencesParams {
                file_path: file_path.clone(),
                line,
                character,
                include_declaration,
            })
            .await;
        if let (Ok(references), Some(name)) = (&mut result, &name) {
            self.add_index_uses(translator, name, &file_path, references)
                .await;
        }
        json_result(result)
    }

    /// The folders serving `file`, and the reference family of its language:
    /// where the index's uses of a symbol of `file` are looked for, and in
    /// which languages they can be (`reference_family`).
    async fn index_scope(
        translator: &Arc<Translator>,
        file: &str,
    ) -> Option<(String, Vec<std::path::PathBuf>)> {
        use devscriptor_core::parser::language_config::{
            detect_language_from_filename, reference_family,
        };
        let language = detect_language_from_filename(file)?;
        let folders = translator
            .folders_serving(std::path::Path::new(file))
            .await
            .ok()?;
        Some((reference_family(&language).to_string(), folders))
    }

    /// Add to `references` the uses of `name` the index records under the
    /// folders serving `file`, in languages that can name `file`'s symbols
    /// (`reference_family`), that the server did not report. A server answers
    /// for what it reads: typescript-language-server leaves out the calls in
    /// `.vue` and `.svelte` files, and Metals without a build those in other
    /// files, so an answer can look complete when it is not.
    async fn add_index_uses(
        &self,
        translator: &Arc<Translator>,
        name: &str,
        file: &str,
        references: &mut devscriptor_lsp::mcp::tools::ReferencesResult,
    ) {
        use devscriptor_core::parser::language_config::reference_family;
        let Some((family, folders)) = Self::index_scope(translator, file).await else {
            return;
        };
        let database = self.core.state().database();
        let names = [name.to_string()];
        let mut sites = Vec::new();
        for folder in &folders {
            match database.get_reference_sites(folder, &names).await {
                Ok(found) => sites.extend(
                    found
                        .into_iter()
                        .filter(|(_, _, language)| reference_family(language) == family)
                        .map(|(site, line, _)| (site, line)),
                ),
                Err(e) => {
                    tracing::debug!("No index uses of {name} under {}: {e}", folder.display())
                }
            }
        }
        references.add_index_uses(name, sites);
    }

    /// Add to `calls` the calls of `name` the index records under the folders
    /// serving `file`, in languages that can name `file`'s symbols, that the
    /// server did not report, each with what it is made in: asked for the
    /// callers of a function a Vue component calls, typescript-language-server
    /// gives only those in the files it reads.
    async fn add_index_calls(
        &self,
        translator: &Arc<Translator>,
        name: &str,
        file: &str,
        calls: &mut devscriptor_lsp::mcp::tools::IncomingCallsResult,
    ) {
        use devscriptor_core::parser::language_config::reference_family;
        let Some((family, folders)) = Self::index_scope(translator, file).await else {
            return;
        };
        let database = self.core.state().database();
        let names = [name.to_string()];
        let mut sites = Vec::new();
        for folder in &folders {
            match database.get_call_sites(folder, &names).await {
                Ok(found) => sites.extend(
                    found
                        .into_iter()
                        .filter(|(_, _, language, _)| reference_family(language) == family)
                        .map(|(site, line, _, caller)| (site, line, caller)),
                ),
                Err(e) => {
                    tracing::debug!("No index calls of {name} under {}: {e}", folder.display())
                }
            }
        }
        calls.add_index_calls(name, sites);
    }

    /// Open, in the server answering for `file`, the files under its folders
    /// that the index says use `name` — call it, pass it, use it as a type or
    /// import it — before it is asked for the references or callers of
    /// `name`: a server that looks only in the projects of open files finds
    /// theirs then ([`Translator::open_beside`]): typescript-language-server, for
    /// one, gives a function's references without the calls a test file makes
    /// of it until the test file is open.
    async fn open_files_using(&self, translator: &Arc<Translator>, name: &str, file: &str) {
        let file = std::path::Path::new(file);
        let Ok(folders) = translator.folders_serving(file).await else {
            return;
        };
        let database = self.core.state().database();
        let names = [name.to_string()];
        let mut files = std::collections::BTreeSet::new();
        for folder in &folders {
            match database.get_sources_referring_to(folder, &names).await {
                Ok(sources) => files.extend(
                    sources
                        .into_iter()
                        .map(|(_, path)| std::path::PathBuf::from(path)),
                ),
                Err(e) => tracing::debug!("No files using {name} under {}: {e}", folder.display()),
            }
        }
        let files: Vec<std::path::PathBuf> = files.into_iter().collect();
        if let Err(e) = translator.open_beside(file, &files).await {
            tracing::debug!("Not opening the files using {name}: {e}");
        }
    }

    lsp_handler!(
        get_type_definition_impl,
        handle_type_definition,
        TypeDefinitionParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            TypeDefinitionParams {
                file_path,
                line,
                character,
            }
        }
    );

    lsp_handler!(
        get_implementations_impl,
        handle_implementation,
        ImplementationParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            ImplementationParams {
                file_path,
                line,
                character,
            }
        }
    );

    // --- Completion (2 tools, 1 special) ---

    lsp_handler!(
        get_completions_impl,
        handle_completions,
        CompletionsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            CompletionsParams {
                file_path,
                line,
                character,
            }
        }
    );

    lsp_handler!(
        get_completion_resolve_impl,
        handle_completion_resolve,
        Completion,
        item,
        |args: serde_json::Map<String, serde_json::Value>, item| CompletionResolveParams {
            file_path: args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            item
        }
    );

    lsp_handler!(
        get_diagnostics_impl,
        handle_diagnostics,
        DiagnosticsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            DiagnosticsParams { file_path }
        }
    );

    lsp_handler!(
        get_code_actions_impl,
        handle_code_actions,
        CodeActionsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_line = args
                .get("start_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let start_character = args
                .get("start_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_line = args
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_character = args
                .get("end_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            CodeActionsParams {
                file_path,
                range: Range {
                    start: Position2D {
                        line: start_line,
                        character: start_character,
                    },
                    end: Position2D {
                        line: end_line,
                        character: end_character,
                    },
                },
            }
        }
    );

    lsp_handler!(
        get_code_action_resolve_impl,
        handle_code_action_resolve,
        CodeAction,
        action,
        |args: serde_json::Map<String, serde_json::Value>, action| CodeActionResolveParams {
            file_path: args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            action
        }
    );

    lsp_handler!(
        get_document_symbols_impl,
        handle_document_symbols,
        DocumentSymbolsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            DocumentSymbolsParams { file_path }
        }
    );

    lsp_handler!(
        get_workspace_symbols_impl,
        handle_workspace_symbol,
        WorkspaceSymbolParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            WorkspaceSymbolParams { query }
        }
    );

    lsp_handler!(
        get_symbols_impl,
        handle_symbols,
        SymbolsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let limit = args
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(100) as u32;
            let offset = args
                .get("offset")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            SymbolsParams {
                file_path,
                limit,
                offset,
            }
        }
    );

    lsp_handler!(
        get_project_symbols_impl,
        handle_project_symbols,
        ProjectSymbolsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let limit = args
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(100) as u32;
            let offset = args
                .get("offset")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            ProjectSymbolsParams {
                language_id,
                project,
                query,
                limit,
                offset,
            }
        }
    );

    lsp_handler!(
        get_call_hierarchy_impl,
        handle_call_hierarchy,
        CallHierarchyParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            CallHierarchyParams {
                file_path,
                line,
                character,
            }
        }
    );

    async fn get_incoming_calls_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let item = self.call_hierarchy_item(translator, args).await?;
        self.open_files_using(translator, &item.name, &item.location.file_path)
            .await;
        let (name, file) = (item.name.clone(), item.location.file_path.clone());
        let mut result = translator
            .handle_incoming_calls(IncomingCallsParams { item })
            .await;
        if let Ok(calls) = &mut result {
            self.add_index_calls(translator, &name, &file, calls).await;
        }
        json_result(result)
    }

    async fn get_outgoing_calls_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let item = self.call_hierarchy_item(translator, args).await?;
        json_result(
            translator
                .handle_outgoing_calls(OutgoingCallsParams { item })
                .await,
        )
    }

    /// The call-hierarchy item a follow-up call is about: `item` as
    /// `get_call_hierarchy` returned it, or else the first one it returns for
    /// the symbol the arguments name.
    ///
    /// Asked to pass an item back unchanged, an agent copied it back field by
    /// field, dropped one, and then guessed at the missing field call after
    /// call. Naming the symbol spares it the copy.
    async fn call_hierarchy_item(
        &self,
        translator: &Arc<Translator>,
        mut args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallHierarchyItem, McpError> {
        if let Some(item) = args.get("item") {
            return serde_json::from_value(item.clone())
                .map_err(|e| invalid_item(&e, "get_call_hierarchy"));
        }
        let (file_path, line, character) = self.symbol_position(&mut args).await?;
        let prepared = translator
            .handle_call_hierarchy(CallHierarchyParams {
                file_path,
                line,
                character,
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        prepared.items.into_iter().next().ok_or_else(no_item_here)
    }

    /// The position `args` names — its `name` resolved from the code index —
    /// or an error saying what to pass.
    async fn symbol_position(
        &self,
        args: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(String, u32, u32), McpError> {
        self.resolve_symbol_args(args).await?;
        let file_path = args.get("file_path").and_then(|v| v.as_str());
        let line = args.get("line").and_then(serde_json::Value::as_u64);
        let character = args.get("character").and_then(serde_json::Value::as_u64);
        match (file_path, line, character) {
            (Some(file_path), Some(line), Some(character)) => {
                Ok((file_path.to_string(), line as u32, character as u32))
            }
            _ => Err(McpError::invalid_params(
                "pass `item` from the preparing call, or the symbol: `name`, or `file_path` \
                 with `line` and `character`",
                None,
            )),
        }
    }

    lsp_handler!(
        get_type_hierarchy_impl,
        handle_type_hierarchy,
        TypeHierarchyParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            TypeHierarchyParams {
                file_path,
                line,
                character,
            }
        }
    );

    async fn get_subtypes_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let item = self.type_hierarchy_item(translator, args).await?;
        json_result(translator.handle_subtypes(SubtypesParams { item }).await)
    }

    async fn get_supertypes_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let item = self.type_hierarchy_item(translator, args).await?;
        json_result(
            translator
                .handle_supertypes(SupertypesParams { item })
                .await,
        )
    }

    /// The type-hierarchy item a follow-up call is about; see
    /// [`Self::call_hierarchy_item`].
    async fn type_hierarchy_item(
        &self,
        translator: &Arc<Translator>,
        mut args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<TypeHierarchyItem, McpError> {
        if let Some(item) = args.get("item") {
            return serde_json::from_value(item.clone())
                .map_err(|e| invalid_item(&e, "get_type_hierarchy"));
        }
        let (file_path, line, character) = self.symbol_position(&mut args).await?;
        let prepared = translator
            .handle_type_hierarchy(TypeHierarchyParams {
                file_path,
                line,
                character,
            })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        prepared.items.into_iter().next().ok_or_else(no_item_here)
    }

    lsp_handler!(
        get_highlights_impl,
        handle_document_highlights,
        DocumentHighlightsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            DocumentHighlightsParams {
                file_path,
                line,
                character,
            }
        }
    );

    lsp_handler!(
        get_colors_impl,
        handle_document_colors,
        DocumentColorsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            DocumentColorsParams { file_path }
        }
    );

    lsp_handler!(
        get_folding_ranges_impl,
        handle_folding_ranges,
        FoldingRangesParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            FoldingRangesParams { file_path }
        }
    );

    lsp_handler!(
        get_selection_range_impl,
        handle_selection_range,
        SelectionRangeParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let positions: Vec<Position2D> = args
                .get("positions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            SelectionRangeParams {
                file_path,
                positions,
            }
        }
    );

    lsp_handler!(
        get_linked_editing_range_impl,
        handle_linked_editing_range,
        LinkedEditingRangeParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            LinkedEditingRangeParams {
                file_path,
                line,
                character,
            }
        }
    );

    lsp_handler!(
        get_inlay_hints_impl,
        handle_inlay_hints,
        InlayHintsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_line = args
                .get("start_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let start_character = args
                .get("start_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_line = args
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_character = args
                .get("end_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            InlayHintsParams {
                file_path,
                start_line,
                start_character,
                end_line,
                end_character,
            }
        }
    );

    lsp_handler!(
        get_links_impl,
        handle_document_links,
        DocumentLinksParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            DocumentLinksParams { file_path }
        }
    );

    lsp_handler!(
        get_semantic_tokens_impl,
        handle_semantic_tokens,
        SemanticTokensParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            SemanticTokensParams { file_path }
        }
    );

    lsp_handler!(
        format_document_impl,
        handle_format_document,
        FormatDocumentParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            FormatDocumentParams { file_path }
        }
    );

    lsp_handler!(
        get_range_format_impl,
        handle_format_range,
        FormatRangeParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_line = args
                .get("start_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let start_character = args
                .get("start_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_line = args
                .get("end_line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let end_character = args
                .get("end_character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            FormatRangeParams {
                file_path,
                range: Range {
                    start: Position2D {
                        line: start_line,
                        character: start_character,
                    },
                    end: Position2D {
                        line: end_line,
                        character: end_character,
                    },
                },
            }
        }
    );

    lsp_handler!(
        rename_symbol_impl,
        handle_rename,
        RenameParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = args
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let character = args
                .get("character")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            let new_name = args
                .get("new_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            RenameParams {
                file_path,
                line,
                character,
                new_name,
            }
        }
    );

    lsp_handler!(
        get_server_status_impl,
        handle_server_status,
        ServerStatusParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            ServerStatusParams { language_id }
        }
    );

    lsp_handler!(
        get_server_capabilities_impl,
        handle_server_capabilities,
        ServerCapabilitiesParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ServerCapabilitiesParams { language_id }
        }
    );

    lsp_handler!(
        get_server_projects_impl,
        handle_server_projects,
        ServerProjectsParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ServerProjectsParams { language_id }
        }
    );

    lsp_handler!(
        get_project_files_impl,
        handle_project_files,
        ProjectFilesParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let limit = args
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(100) as u32;
            let offset = args
                .get("offset")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            ProjectFilesParams {
                project,
                limit,
                offset,
            }
        }
    );

    lsp_handler!(
        start_server_impl,
        handle_start_server,
        StartServerParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .map(String::from);
            StartServerParams {
                language_id,
                project,
            }
        }
    );

    lsp_handler!(
        stop_server_impl,
        handle_stop_server,
        StopServerParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            StopServerParams { language_id }
        }
    );

    lsp_handler!(
        restart_server_impl,
        handle_restart_server,
        RestartServerParams,
        |args: serde_json::Map<String, serde_json::Value>| {
            let language_id = args
                .get("language_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            RestartServerParams { language_id }
        }
    );
}

/// Build the 38 LSP tool schema definitions for [`list_tools`].
/// What every tool whose results carry positions says about quoting them.
const CITE_NOTE: &str = "Each result carries `cite`, `path:line:column` counted from 1: quote \
                         it as is. `range` and `position` are LSP's, 0-based.";

/// `description`, then [`CITE_NOTE`].
fn cites(description: &str) -> String {
    format!("{description} {CITE_NOTE}")
}

pub fn tool_list() -> Vec<Tool> {
    vec![
        // Navigation
        Tool::new(
            "get_hover",
            "Get hover info (type signature/docs) for a symbol. Pass a symbol `name` (resolved from the code index) or explicit line/character.",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_definition",
            cites(
                "Find where a symbol is defined. Pass a symbol `name` (resolved from the code index) or explicit line/character.",
            ),
            position_schema(json!({})),
        ),
        Tool::new(
            "get_references",
            cites(
                "Find all references to a symbol. Pass a symbol `name` (resolved from the code index) or explicit line/character. Uses the code index records (calls, reads of constants and properties, functions passed, type uses and imports) that the language server did not report (in files it cannot read, such as `.vue` and `.svelte` files for the TypeScript server) come in `index_locations`, by line.",
            ),
            position_schema(json!({
                "include_declaration": {"type": "boolean"}
            })),
        ),
        Tool::new(
            "get_type_definitions",
            cites(
                "Find a symbol's type definition(s). Pass a symbol `name` (resolved from the code index) or explicit line/character.",
            ),
            position_schema(json!({})),
        ),
        Tool::new(
            "get_implementations",
            cites("Find implementations"),
            position_schema(json!({})),
        ),
        // Completion
        Tool::new(
            "get_completions",
            "Get code completions",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_completion_resolve",
            "Resolve completion item details",
            create_schema(json!({
                "type": "object",
                "properties": {"item": {"type": "object", "description": "An item returned by `get_completions`. Pass it back unchanged; it must contain `label`."}},
                "required": ["item"]
            })),
        ),
        // Diagnostics & Actions
        Tool::new(
            "get_diagnostics",
            cites("Get diagnostics (errors/warnings) for a file"),
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_code_actions",
            "Get available code actions",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "get_code_action_resolve",
            "Resolve code action details",
            create_schema(json!({
                "type": "object",
                "properties": {"action": {"type": "object", "description": "An item returned by `get_code_actions`. Pass it back unchanged; it must contain `title`."}},
                "required": ["action"]
            })),
        ),
        // Symbols
        Tool::new(
            "get_document_symbols",
            cites(
                "Get all symbols in a file, parent first: each at its name, with the symbol it is declared in as `container_name`",
            ),
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_workspace_symbols",
            cites(
                "Search symbols across the workspace: every running language server that offers it is asked, and their answers merged; one that could not answer is named in `note`",
            ),
            create_schema(json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            })),
        ),
        Tool::new(
            "get_symbols",
            cites("Get symbols for a file as get_document_symbols gives them, a page at a time"),
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                },
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_project_symbols",
            cites("Search a language server's symbols by name, a page at a time"),
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "Language server to query, e.g. 'rust'. Optional when exactly one server is running."
                    },
                    "project": {
                        "type": "string",
                        "description": "Only the symbols under this directory, whose files are opened in the server first: gopls and typescript-language-server search only the projects of open files. Relative paths are taken from the workspace root."
                    },
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                }
            })),
        ),
        // Call Hierarchy
        Tool::new(
            "get_call_hierarchy",
            cites(
                "Prepare the call hierarchy for a symbol (feed the result to get_incoming_calls/get_outgoing_calls). Pass a symbol `name` (resolved from the code index) or explicit line/character.",
            ),
            position_schema(json!({})),
        ),
        Tool::new(
            "get_incoming_calls",
            cites(
                "Get the calls into a function: pass an `item` from `get_call_hierarchy`, or name the function (`name`, or `file_path` with `line`/`character`) and the call hierarchy is prepared for you. Calls the code index records that the language server did not report (in files it cannot read, such as `.vue` and `.svelte` files for the TypeScript server) come in `index_calls`, each with its caller and line.",
            ),
            hierarchy_item_schema("get_call_hierarchy"),
        ),
        Tool::new(
            "get_outgoing_calls",
            cites(
                "Get the calls a function makes: pass an `item` from `get_call_hierarchy`, or name the function (`name`, or `file_path` with `line`/`character`) and the call hierarchy is prepared for you.",
            ),
            hierarchy_item_schema("get_call_hierarchy"),
        ),
        // Type Hierarchy
        Tool::new(
            "get_type_hierarchy",
            cites("Get type hierarchy"),
            position_schema(json!({})),
        ),
        Tool::new(
            "get_subtypes",
            cites(
                "Get a type's subtypes: pass an `item` from `get_type_hierarchy`, or name the type (`name`, or `file_path` with `line`/`character`) and the type hierarchy is prepared for you.",
            ),
            hierarchy_item_schema("get_type_hierarchy"),
        ),
        Tool::new(
            "get_supertypes",
            cites(
                "Get a type's supertypes: pass an `item` from `get_type_hierarchy`, or name the type (`name`, or `file_path` with `line`/`character`) and the type hierarchy is prepared for you.",
            ),
            hierarchy_item_schema("get_type_hierarchy"),
        ),
        // Document Features
        Tool::new(
            "get_highlights",
            cites("Get document highlights"),
            position_schema(json!({})),
        ),
        Tool::new(
            "get_colors",
            cites("Get document colors"),
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_folding_ranges",
            cites("Get folding ranges"),
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_selection_range",
            cites("Get selection ranges"),
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "positions": {"type": "array"}
                },
                "required": ["file_path", "positions"]
            })),
        ),
        Tool::new(
            "get_linked_editing_range",
            "Get linked editing ranges",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_inlay_hints",
            cites("Get inlay hints"),
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "get_links",
            cites("Get document links"),
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_semantic_tokens",
            "Get semantic tokens",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        // Formatting
        Tool::new(
            "format_document",
            "Format a document with the language server and write the formatted result to disk. Returns the edits that were applied.",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_range_format",
            "Format a line/character range with the language server and write the result to disk. Returns the edits that were applied.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "rename_symbol",
            "Rename a symbol across the whole workspace, applying the edits to every affected file on disk. Pass a symbol `name` (resolved from the code index) or an explicit line/character, plus `new_name`. Returns the per-file changes that were applied.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Symbol to rename, resolved from the code index — use instead of line/character."},
                    "entity_type": {"type": "string", "description": "Optional disambiguator for `name`: function, class, method, etc."},
                    "file_path": {"type": "string"},
                    "line": {"type": "integer"},
                    "character": {"type": "integer"},
                    "new_name": {"type": "string", "description": "The new identifier."}
                },
                "required": ["new_name"]
            })),
        ),
        // Server Management
        Tool::new(
            "get_server_status",
            "Get server status",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "One server's status, e.g. 'rust'. Omit for every configured server's."
                    }
                }
            })),
        ),
        Tool::new(
            "get_server_capabilities",
            "Get server capabilities",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "Language server to query, e.g. 'rust'. Optional when exactly one server is running."
                    }
                }
            })),
        ),
        Tool::new(
            "get_server_projects",
            "Get the folders a language server has open: `language_id`'s, or the workspace's when none is given",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {"type": "string", "description": "A running server, e.g. 'rust'."}
                }
            })),
        ),
        Tool::new(
            "get_project_files",
            "Get a project's files, sorted, without hidden files and what its ignore files leave out (`.git`, a `.gitignore`d `node_modules`), a page at a time",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "The project's directory. Relative paths are taken from the workspace root."
                    },
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                },
                "required": ["project"]
            })),
        ),
        Tool::new(
            "start_server",
            "Start a language server: for `project`, or for the workspace when none is given. A server already running keeps running, restarted only to take a project it does not cover yet.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {"type": "string", "description": "Language of the server, e.g. 'rust'."},
                    "project": {
                        "type": "string",
                        "description": "A path in the project to start it for — the project is the outermost directory above it with the server's manifest (e.g. Cargo.toml). Relative paths are taken from the workspace root."
                    }
                },
                "required": ["language_id"]
            })),
        ),
        Tool::new(
            "stop_server",
            "Stop a language server",
            create_schema(json!({
                "type": "object",
                "properties": {"language_id": {"type": "string"}},
                "required": ["language_id"]
            })),
        ),
        Tool::new(
            "restart_server",
            "Restart a language server",
            create_schema(json!({
                "type": "object",
                "properties": {"language_id": {"type": "string"}},
                "required": ["language_id"]
            })),
        ),
    ]
}

#[cfg(all(test, feature = "lsp"))]
mod name_resolution_tests {
    use super::*;

    #[test]
    fn identifier_column_finds_the_name_not_the_keyword() {
        // The stored column points at `def` (0); the identifier is at 4.
        let line = "def apply_discount(price, percent):";
        assert_eq!(identifier_column(line, "apply_discount", 0), Some(4));
    }

    #[test]
    fn identifier_column_rejects_substrings_of_larger_identifiers() {
        let line = "    total = apply_discount_v2(x)";
        // `apply_discount` occurs only inside `apply_discount_v2`, which is a
        // different identifier — no whole-word match.
        assert_eq!(identifier_column(line, "apply_discount", 0), None);
        assert_eq!(identifier_column(line, "apply_discount_v2", 0), Some(12));
    }

    /// The code index and `identifier_column` count bytes; LSP positions
    /// count UTF-16 units. `ñ`/`ú` are two bytes but one unit each.
    #[test]
    fn locate_identifier_returns_utf16_columns_on_non_ascii_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(
            &file,
            "// Déjà vu\nfn ñandú_total(a: i32) -> i32 { a + 1 }\nconst É: i32 = 1; fn after_é() {}\n",
        )
        .unwrap();
        let path = file.to_str().unwrap();
        assert_eq!(locate_identifier(path, 1, 0, "ñandú_total"), Some((1, 3)));
        // `after_é` starts at byte 22 (`É` is two bytes) but UTF-16 column 21.
        assert_eq!(locate_identifier(path, 2, 0, "after_é"), Some((2, 21)));
        // The fallback converts the stored byte column the same way.
        assert_eq!(definition_start(path, 2, 22), (2, 21));
    }

    #[test]
    fn utf16_column_counts_code_units() {
        assert_eq!(utf16_column("abc", 2), 2);
        assert_eq!(utf16_column("ñandú x", 7), 5);
        // An emoji is four bytes and two UTF-16 units.
        assert_eq!(utf16_column("😀 x", 5), 3);
        assert_eq!(
            utf16_column("ab", 99),
            2,
            "past the end counts the whole line"
        );
    }

    #[test]
    fn identifier_column_treats_unicode_letters_as_part_of_an_identifier() {
        // `total` inside `ñtotal` is not a standalone identifier.
        assert_eq!(
            identifier_column("let ñtotal = total;", "total", 0),
            Some(14)
        );
        assert_eq!(
            identifier_column("fn ñandú_total()", "ñandú_total", 0),
            Some(3)
        );
    }

    #[test]
    fn identifier_column_honours_the_floor() {
        // Two occurrences; the floor skips the first (a call) to the second.
        let line = "x = foo(); def foo():";
        let first = identifier_column(line, "foo", 0).unwrap();
        let second = identifier_column(line, "foo", first + 1).unwrap();
        assert!(second > first);
    }

    #[test]
    fn path_hint_matches_by_basename_suffix_and_exact() {
        assert!(path_hint_matches("/a/b/pricing.py", "pricing.py"));
        assert!(path_hint_matches("/a/b/pricing.py", "b/pricing.py"));
        assert!(path_hint_matches("/a/b/pricing.py", "/a/b/pricing.py"));
        assert!(!path_hint_matches("/a/b/pricing.py", "cart.py"));
    }

    #[test]
    fn locate_identifier_resolves_a_definition_to_its_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pricing.py");
        std::fs::write(
            &file,
            "def line_total(u, q):\n    return u * q\n\ndef apply_discount(price, percent):\n    return price * (1 - percent / 100)\n",
        )
        .unwrap();
        // The graph stores the definition start: line 3 (0-indexed), column 0 (`def`).
        let (line, character) =
            locate_identifier(file.to_str().unwrap(), 3, 0, "apply_discount").unwrap();
        assert_eq!(line, 3, "same line as the definition");
        assert_eq!(character, 4, "the identifier, past `def `");
    }

    /// The Context Graph stores 1-indexed lines (`start.row + 1`), while LSP is
    /// 0-indexed. The resolver must convert before scanning, or it reads one line
    /// too low, misses the identifier, and returns a dead position.
    #[test]
    fn graph_line_is_one_indexed_and_converts_before_locating() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pricing.py");
        std::fs::write(
            &file,
            "def apply_discount(price, percent):\n    return price\n",
        )
        .unwrap();

        // Stored for a def on 0-indexed line 0.
        let graph_start_line: u32 = 1;
        let zero_indexed = graph_start_line.saturating_sub(1);
        let position = locate_identifier(file.to_str().unwrap(), zero_indexed, 0, "apply_discount");
        assert_eq!(position, Some((0, 4)), "must land on the identifier at 0:4");

        // Without the conversion, scanning starts on the body line and misses it.
        let wrong = locate_identifier(
            file.to_str().unwrap(),
            graph_start_line,
            0,
            "apply_discount",
        );
        assert_eq!(wrong, None, "the un-converted line finds nothing");
    }

    #[test]
    fn position_schema_advertises_name_and_keeps_extras() {
        let schema = position_schema(json!({ "include_declaration": {"type": "boolean"} }));
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("name"));
        assert!(props.contains_key("entity_type"));
        assert!(props.contains_key("line"));
        assert!(props.contains_key("include_declaration"));
        // Nothing is strictly required — the handler enforces name-or-position.
        assert_eq!(schema["required"].as_array().unwrap().len(), 0);
    }

    /// A name resolves to where the symbol is in the file on disk, though the
    /// index last read an older version of it: the language server reads the
    /// disk, and the old line would point it at a blank one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_name_resolves_to_its_place_in_the_file_on_disk() {
        use devscriptor_core::database::DatabaseConfig;
        use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let root = dunce::canonicalize(root).unwrap();
        let file = root.join("app.py");
        std::fs::write(&file, "def helper():\n    return 1\n").unwrap();
        let db = dir.path().join("index.db");
        // A budget the re-index fits in on a slow machine too: what is tested
        // is where the name resolves once the index holds the file on disk.
        let state = crate::server::DevscriptorServerState::with_db_path(db.clone())
            .unwrap()
            .with_freshness_budget(std::time::Duration::from_secs(60));
        CodebasePipeline::with_database(
            PipelineConfig {
                database: DatabaseConfig {
                    path: db.to_string_lossy().into_owned(),
                    ..DatabaseConfig::default()
                },
                skip_embeddings: true,
                ..PipelineConfig::default()
            },
            state.database(),
        )
        .unwrap()
        .analyze_directory(&root)
        .await
        .unwrap();

        std::fs::write(
            &file,
            format!(
                "{}# helper moved down\ndef helper():\n    return 1\n",
                "\n".repeat(10)
            ),
        )
        .unwrap();
        let (path, line, character) = symbol_position_by_name(&state, "helper", None, None)
            .await
            .unwrap();
        assert_eq!(path, file.to_string_lossy());
        assert_eq!((line, character), (11, 4));
    }
}

#[cfg(all(test, feature = "lsp"))]
mod hierarchy_item_tests {
    use super::*;

    /// A tool continuing a hierarchy takes the prepared `item`, spelled out, or
    /// the symbol itself; neither is required on its own.
    #[test]
    fn a_hierarchy_tool_takes_the_item_or_the_symbol() {
        let schema = hierarchy_item_schema("get_call_hierarchy");
        let props = schema["properties"].as_object().unwrap();
        for key in ["item", "name", "file_path", "line", "character"] {
            assert!(props.contains_key(key), "{key}");
        }
        assert_eq!(schema["required"].as_array().unwrap().len(), 0);

        let item = &props["item"];
        assert_eq!(item["required"], json!(["name", "kind", "location"]));
        let location = &item["properties"]["location"];
        assert_eq!(location["required"], json!(["file_path", "range"]));
        assert_eq!(
            location["properties"]["range"]["required"],
            json!(["start", "end"])
        );
    }

    /// A malformed item is answered with the shape it should have had and the
    /// way around copying it.
    #[test]
    fn a_malformed_item_is_answered_with_its_shape() {
        let error = serde_json::from_value::<CallHierarchyItem>(json!({
            "name": "suggestions",
            "kind": "Function",
            "location": {"file_path": "recommender.py", "range": {"start": {"line": 24, "character": 4}}}
        }))
        .unwrap_err();
        let message = invalid_item(&error, "get_call_hierarchy")
            .message
            .to_string();
        assert!(message.contains("missing field `end`"), "{message}");
        assert!(
            message.contains(
                "location: {file_path, range: {start: {line, character}, end: {line, character}}}"
            ),
            "{message}"
        );
        assert!(message.contains("`name`"), "{message}");
    }

    /// Every property of every LSP tool's schema, nested ones included, has an
    /// explicit `type`: some function-calling APIs refuse a property without.
    #[test]
    fn every_property_has_a_type() {
        fn untyped(schema: &serde_json::Value, at: &str, out: &mut Vec<String>) {
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (key, prop) in props {
                    let path = format!("{at}.{key}");
                    if prop.get("type").is_none() {
                        out.push(path.clone());
                    }
                    untyped(prop, &path, out);
                }
            }
        }
        let mut missing = Vec::new();
        for tool in tool_list() {
            let schema = serde_json::Value::Object((*tool.input_schema).clone());
            untyped(&schema, &tool.name, &mut missing);
        }
        assert!(missing.is_empty(), "untyped properties: {missing:?}");
    }

    /// `start_server` declares the `project` its handler reads: undeclared, no
    /// agent passed it, and a server could only start for the workspace.
    #[test]
    fn start_server_declares_its_project() {
        let tools = tool_list();
        let start = tools.iter().find(|t| t.name == "start_server").unwrap();
        let props = start.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("project"));
        assert_eq!(start.input_schema["required"], json!(["language_id"]));
    }

    /// An item passed back without the server's optional `data` is accepted,
    /// for type hierarchies as for call hierarchies.
    #[test]
    fn a_type_hierarchy_item_may_omit_its_data() {
        let item: TypeHierarchyItem = serde_json::from_value(json!({
            "name": "Shape",
            "kind": "Class",
            "location": {
                "file_path": "shapes.ts",
                "range": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 11}}
            }
        }))
        .unwrap();
        assert!(item.data.is_null());
    }
}

#[cfg(all(test, feature = "lsp"))]
mod open_files_tests {
    use super::*;
    use devscriptor_lsp::config::{
        Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
    };

    /// A fake JavaScript language server that appends to the file its first
    /// argument names `open <uri>` for each document opened, and the method of
    /// each request for references or callers, which it answers with none. A
    /// call hierarchy it prepares is one item, of the file asked about.
    const FAKE_SERVER: &str = r#"
import json, sys

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def log(line):
    with open(sys.argv[1], "a") as out:
        out.write(line + "\n")

span = {"start": {"line": 0, "character": 16}, "end": {"line": 0, "character": 32}}
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "textDocumentSync": 1, "referencesProvider": True,
            "callHierarchyProvider": True}}})
    elif method == "textDocument/didOpen":
        log("open " + msg["params"]["textDocument"]["uri"])
    elif method == "textDocument/prepareCallHierarchy":
        uri = msg["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [{"name": "buildOrdersQuery",
            "kind": 12, "uri": uri, "range": span, "selectionRange": span}]})
    elif method in ("textDocument/references", "callHierarchy/incomingCalls"):
        log(method)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// `cite` with `/` between folders, as the expectations are written: a cite
    /// names the file as this platform spells it.
    fn slashed(cite: &str) -> String {
        cite.replace('\\', "/")
    }

    /// What the fake server logged when `tool` was asked about
    /// `buildOrdersQuery`, of a scanned project where a test file and a Python
    /// script call it and another file does not, and what the tool answered.
    async fn asked(tool: &str) -> (Vec<String>, serde_json::Value) {
        asked_about(tool, "buildOrdersQuery").await
    }

    /// [`asked`] about `name`: `buildOrdersQuery`, or the `ORDERS_LIMIT` the
    /// test file and the Python script read.
    async fn asked_about(tool: &str, name: &str) -> (Vec<String>, serde_json::Value) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("admin");
        std::fs::create_dir_all(&project).unwrap();
        let project = dunce::canonicalize(project).unwrap();
        write(
            &project.join("src/api/orders.js"),
            "export function buildOrdersQuery() {\n  return '/api/orders';\n}\n\
             export function fetchOrders() {\n  return fetch(buildOrdersQuery());\n}\n\
             export const ORDERS_LIMIT = 50;\n",
        );
        write(
            &project.join("test/orders.test.js"),
            "import { buildOrdersQuery, ORDERS_LIMIT } from '../src/api/orders.js';\n\
             test('the query', () => {\n  buildOrdersQuery();\n});\n\
             test('the limit', () => {\n  expect(rows).toBeLessThan(ORDERS_LIMIT);\n});\n",
        );
        write(
            &project.join("scripts/report.py"),
            "def report():\n    return buildOrdersQuery()\n\n\ndef limit():\n    return ORDERS_LIMIT\n",
        );
        write(
            &project.join("src/other.js"),
            "export function unrelated() {\n  return 1;\n}\n",
        );
        let state =
            crate::server::DevscriptorServerState::with_db_path(temp.path().join("index.db"))
                .unwrap();
        let core = Arc::new(crate::CoreModule::new(Arc::new(state)));
        core.handle_tool(
            "scan_codebase",
            Some(json!({"path": project.to_string_lossy()})),
        )
        .await
        .unwrap();

        let script = temp.path().join("fake_lsp.py");
        write(&script, FAKE_SERVER);
        let log = temp.path().join("lsp.log");
        let mut translator = Translator::new(Config {
            workspace: WorkspaceConfig::default(),
            servers: vec![LspServerConfig {
                language_id: "javascript".into(),
                command: "python3".into(),
                args: vec![script.display().to_string(), log.display().to_string()],
                env: std::collections::HashMap::new(),
                working_dir: None,
                extensions: vec!["js".into()],
                heuristics: None,
                initialization_options: None,
                settings: ServerSettings::default(),
                configuration: None,
            }],
            lifecycle: LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![project]);
        let translator = Arc::new(translator);
        let server = UnifiedMcpServer::with_lsp(core, Arc::clone(&translator)).unwrap();
        let mut args = serde_json::Map::new();
        args.insert("name".into(), json!(name));
        let answer = server.handle_lsp_tool(tool, args).await.unwrap();
        let answer = serde_json::from_str(&answer.content[0].as_text().unwrap().text).unwrap();
        translator.shutdown().await;
        let lines = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        (lines, answer)
    }

    /// A position names the identifier it is in, its column counted in UTF-16
    /// code units as a language server counts it: past `ö`, one unit, and
    /// `𝔸`, two.
    #[test]
    fn a_position_names_the_identifier_it_is_in() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("orders.js");
        write(&file, "// orders\nconst ö𝔸 = buildOrdersQuery(1);\n");
        let at = |character| identifier_at(&file, 1, character);
        assert_eq!(at(12).as_deref(), Some("buildOrdersQuery"));
        assert_eq!(at(27).as_deref(), Some("buildOrdersQuery"));
        assert_eq!(at(6).as_deref(), Some("ö𝔸"));
        assert_eq!(at(7).as_deref(), Some("ö𝔸"));
        assert_eq!(at(10), None, "`=`, between spaces");
        assert_eq!(identifier_at(&file, 7, 0), None, "no such line");
    }

    /// The files of its language the index says call a function are open in
    /// its server before the server is asked for the function's references or
    /// callers, and no other file is: typescript-language-server, without a
    /// `jsconfig.json`, gives a function's references without a test file's
    /// calls of it until the test file is open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_files_using_a_symbol_are_open_before_its_references_are_asked() {
        for (tool, request) in [
            ("get_references", "textDocument/references"),
            ("get_incoming_calls", "callHierarchy/incomingCalls"),
        ] {
            let (lines, _) = asked(tool).await;
            let test_opened = lines.iter().position(|line| {
                line.starts_with("open ") && line.ends_with("test/orders.test.js")
            });
            let asked = lines.iter().position(|line| line == request);
            assert!(
                test_opened.is_some() && test_opened < asked,
                "{tool}: the test file is open first: {lines:?}"
            );
            assert!(
                !lines
                    .iter()
                    .any(|line| line.ends_with("other.js") || line.ends_with(".py")),
                "{tool}: {lines:?}"
            );
        }
    }

    /// What the server does not see, the index fills in: the fake server
    /// reports no reference to `buildOrdersQuery`, and the answer lists the
    /// calls the index records in JavaScript — in the function's own file and
    /// in the test — and not the Python script's call of a function of the
    /// same name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_references_the_server_did_not_report() {
        let (_, answer) = asked("get_references").await;
        assert_eq!(answer["locations"], json!([]), "{answer}");
        let cites: Vec<String> = answer["index_locations"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_locations: {answer}"))
            .iter()
            .map(|l| slashed(l["cite"].as_str().unwrap()))
            .collect();
        assert!(
            cites.iter().any(|c| c.ends_with("src/api/orders.js:5")),
            "{cites:?}"
        );
        assert!(
            cites.iter().any(|c| c.ends_with("test/orders.test.js:3")),
            "{cites:?}"
        );
        assert!(!cites.iter().any(|c| c.contains("report.py")), "{cites:?}");
        assert!(
            answer["note"]
                .as_str()
                .is_some_and(|n| n.contains("`buildOrdersQuery`"))
        );
    }

    /// A constant's reads the server did not report, the index fills in too:
    /// the test file's read of `ORDERS_LIMIT`, which is open before the server
    /// is asked, and not the Python script's read of a constant of the same
    /// name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_reads_of_a_constant_the_server_did_not_report() {
        let (lines, answer) = asked_about("get_references", "ORDERS_LIMIT").await;
        assert_eq!(answer["locations"], json!([]), "{answer}");
        let cites: Vec<String> = answer["index_locations"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_locations: {answer}"))
            .iter()
            .map(|l| slashed(l["cite"].as_str().unwrap()))
            .collect();
        assert!(
            cites.iter().any(|c| c.ends_with("test/orders.test.js:6")),
            "{cites:?}"
        );
        assert!(!cites.iter().any(|c| c.contains("report.py")), "{cites:?}");
        let opened = lines
            .iter()
            .position(|line| line.starts_with("open ") && line.ends_with("test/orders.test.js"));
        let asked = lines
            .iter()
            .position(|line| line == "textDocument/references");
        assert!(
            opened.is_some() && opened < asked,
            "the test file is open first: {lines:?}"
        );
    }

    /// The callers too: the fake server reports no call of
    /// `buildOrdersQuery`, and the answer lists the JavaScript calls the index
    /// records, each with what it is made in, and not the Python script's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_callers_the_server_did_not_report() {
        let (_, answer) = asked("get_incoming_calls").await;
        assert_eq!(answer["calls"], json!([]), "{answer}");
        let calls: Vec<(&str, String)> = answer["index_calls"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_calls: {answer}"))
            .iter()
            .map(|c| {
                (
                    c["caller"].as_str().unwrap(),
                    slashed(c["cite"].as_str().unwrap()),
                )
            })
            .collect();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[0].0 == "fetchOrders" && calls[0].1.ends_with("src/api/orders.js:5"),
            "{calls:?}"
        );
        assert!(
            calls[1].0 == "orders.test" && calls[1].1.ends_with("test/orders.test.js:3"),
            "{calls:?}"
        );
        assert!(
            answer["note"]
                .as_str()
                .is_some_and(|n| n.contains("call(s) of `buildOrdersQuery`"))
        );
    }
}

#[cfg(all(test, feature = "lsp"))]
mod argument_tests {
    use super::*;
    use devscriptor_lsp::config::{
        Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
    };

    /// A fake language server that answers `workspace/symbol`, as gopls does,
    /// with the symbols of the files it has been sent: two in
    /// `services/a/lib.fake`, one each in `services/ab` and `services/b`, of
    /// the workspace its first argument names.
    const SYMBOL_SERVER: &str = r#"
import json, pathlib, sys

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def symbol(name, path):
    span = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}
    uri = pathlib.Path(sys.argv[1], path).as_uri()
    return {"name": name, "kind": 12, "location": {"uri": uri, "range": span}}

opened = set()
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "workspaceSymbolProvider": True, "textDocumentSync": 1}}})
    elif method == "textDocument/didOpen":
        opened.add(msg["params"]["textDocument"]["uri"])
    elif method == "workspace/symbol":
        every = [symbol("total_a", "services/a/lib.fake"),
                 symbol("total_a2", "services/a/lib.fake"),
                 symbol("total_ab", "services/ab/lib.fake"),
                 symbol("total_b", "services/b/lib.fake")]
        send({"jsonrpc": "2.0", "id": msg["id"],
              "result": [s for s in every if s["location"]["uri"] in opened]})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A server whose code index lives in `scratch`, for `workspace`, its
    /// language servers those of `servers`.
    fn serve(
        scratch: &std::path::Path,
        workspace: &std::path::Path,
        servers: Vec<LspServerConfig>,
    ) -> (UnifiedMcpServer, Arc<Translator>) {
        let state =
            crate::server::DevscriptorServerState::with_db_path(scratch.join("index.db")).unwrap();
        let core = Arc::new(crate::CoreModule::new(Arc::new(state)));
        let mut translator = Translator::new(Config {
            workspace: WorkspaceConfig::default(),
            servers,
            lifecycle: LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![workspace.to_path_buf()]);
        let translator = Arc::new(translator);
        let server = UnifiedMcpServer::with_lsp(core, Arc::clone(&translator)).unwrap();
        (server, translator)
    }

    fn server_config(language_id: &str, command: &str, args: Vec<String>) -> LspServerConfig {
        LspServerConfig {
            language_id: language_id.into(),
            command: command.into(),
            args,
            env: std::collections::HashMap::new(),
            working_dir: None,
            extensions: vec![language_id.into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }
    }

    /// `tool`'s answer to `args`, as the agent reads it, or its error.
    async fn call(
        server: &UnifiedMcpServer,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let args = args.as_object().unwrap().clone();
        match server.handle_lsp_tool(tool, args).await {
            Ok(result) => {
                Ok(serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap())
            }
            Err(error) => Err(error.message.to_string()),
        }
    }

    /// `get_project_files` lists the project its schema's `project` names,
    /// relative to the workspace root, sorted and at any depth: not `.git`'s
    /// files, hidden ones, or what `.gitignore` leaves out.
    #[tokio::test]
    async fn get_project_files_lists_the_project_it_is_named_by() {
        let scratch = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace = dunce::canonicalize(workspace.path()).unwrap();
        write(&workspace.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&workspace.join(".gitignore"), "build/\n");
        write(&workspace.join(".env"), "TOKEN=1\n");
        write(&workspace.join("build/out.o"), "");
        // Neither in order nor in reverse, as a directory may list them.
        write(&workspace.join("docs/notes.md"), "# Notes\n");
        write(&workspace.join("a/b/c/d/e/f/g/h/i/j/k/deep.txt"), "deep\n");
        write(&workspace.join("src/main.rs"), "fn main() {}\n");
        let (server, translator) = serve(scratch.path(), &workspace, vec![]);

        let all = call(&server, "get_project_files", json!({"project": "."}))
            .await
            .unwrap();
        let listed = |page: &serde_json::Value| -> Vec<String> {
            page["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f.as_str().unwrap().to_string())
                .collect()
        };
        // Joined part by part: the listed paths have this platform's separators.
        let inside = |path: &str| {
            path.split('/')
                .fold(workspace.clone(), |dir, part| dir.join(part))
                .display()
                .to_string()
        };
        assert_eq!(
            listed(&all),
            [
                inside("a/b/c/d/e/f/g/h/i/j/k/deep.txt"),
                inside("docs/notes.md"),
                inside("src/main.rs"),
            ]
        );

        let page = call(
            &server,
            "get_project_files",
            json!({"project": workspace.join("docs").display().to_string(), "limit": 1}),
        )
        .await
        .unwrap();
        assert_eq!(listed(&page), [inside("docs/notes.md")]);

        let missing = call(&server, "get_project_files", json!({"project": "gone"}))
            .await
            .unwrap_err();
        assert!(
            missing.contains(&format!(
                "no project directory {}",
                workspace.join("gone").display()
            )),
            "{missing}"
        );
        translator.shutdown().await;
    }

    /// The server-management tools read what their schemas offer:
    /// `get_project_symbols` opens the files of `project` — a server like
    /// gopls searches only the projects of open files — keeps to it, and
    /// pages by `offset`,
    /// `get_server_projects` gives the folders of `language_id`'s server, and
    /// `get_server_status` answers for `language_id`, and says so of one no
    /// server is configured for rather than calling it stopped.
    #[tokio::test]
    async fn the_server_tools_read_what_their_schemas_offer() {
        let scratch = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace = dunce::canonicalize(workspace.path()).unwrap();
        let script = scratch.path().join("symbols.py");
        write(&script, SYMBOL_SERVER);
        for service in ["a", "ab", "b"] {
            write(
                &workspace.join(format!("services/{service}/lib.fake")),
                "total\n",
            );
        }
        let fake = server_config(
            "fake",
            "python3",
            vec![
                script.display().to_string(),
                workspace.display().to_string(),
            ],
        );
        let idle = server_config("other", "devscriptor-test-no-such-server", vec![]);
        let (server, translator) = serve(scratch.path(), &workspace, vec![fake, idle]);
        let names = |answer: &serde_json::Value| -> Vec<String> {
            answer["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_string())
                .collect()
        };

        let unopened = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(unopened["pagination"]["total"], 0, "no file is open yet");
        let ask = json!({"language_id": "fake", "project": "services/ab"});
        let found = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&found), ["total_ab"]);
        let ask = json!({"language_id": "fake", "project": "services/a", "query": "total"});
        let found = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&found), ["total_a", "total_a2"]);
        let ask = json!({"language_id": "fake", "project": "services/a", "offset": 1});
        let page = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&page), ["total_a2"]);
        assert_eq!(page["pagination"]["total"], 2);
        let opened = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(opened["pagination"]["total"], 3);
        let missing = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake", "project": "services/c"}),
        )
        .await
        .unwrap_err();
        assert!(missing.contains("no project directory"), "{missing}");
        let stopped = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "other"}),
        )
        .await
        .unwrap_err();
        assert_eq!(stopped, "the other language server is not running");

        let folders = call(
            &server,
            "get_server_projects",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(
            folders["projects"],
            json!([workspace.display().to_string()])
        );
        let idle_folders = call(
            &server,
            "get_server_projects",
            json!({"language_id": "other"}),
        )
        .await
        .unwrap_err();
        assert!(idle_folders.contains("not running"), "{idle_folders}");

        let one = call(&server, "get_server_status", json!({"language_id": "fake"}))
            .await
            .unwrap();
        assert_eq!(one["servers"].as_array().unwrap().len(), 1);
        assert_eq!(one["servers"][0]["state"], "running");
        let all = call(&server, "get_server_status", json!({})).await.unwrap();
        assert_eq!(all["servers"].as_array().unwrap().len(), 2);
        let unknown = call(&server, "get_server_status", json!({"language_id": "dart"}))
            .await
            .unwrap_err();
        assert!(
            unknown.contains("No LSP server configured for language: dart"),
            "{unknown}"
        );
        translator.shutdown().await;
    }

    /// Every argument these tools read is in their schemas, under the name
    /// they read it by.
    #[test]
    fn the_schemas_offer_what_the_server_tools_read() {
        let tools = tool_list();
        let offered = |name: &str| -> Vec<String> {
            let tool = tools.iter().find(|t| t.name == name).unwrap();
            let mut properties: Vec<String> = tool.input_schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            properties.sort();
            properties
        };
        assert_eq!(offered("get_project_files"), ["limit", "offset", "project"]);
        assert_eq!(
            offered("get_project_symbols"),
            ["language_id", "limit", "offset", "project", "query"]
        );
        assert_eq!(offered("get_symbols"), ["file_path", "limit", "offset"]);
        assert_eq!(offered("get_server_status"), ["language_id"]);
        assert_eq!(offered("get_server_projects"), ["language_id"]);
        assert_eq!(offered("restart_server"), ["language_id"]);
    }
}
