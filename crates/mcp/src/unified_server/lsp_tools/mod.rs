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

/// A handler of an LSP tool: its arguments read, the symbol a name gives
/// resolved to a place, the translator asked, and the answer returned as JSON
/// or text.
macro_rules! lsp_handler {
    // Standard handler — serialises result as JSON
    ($fn_name:ident, $handle_method:ident, $params_ty:ty, $build:expr) => {
        pub(super) async fn $fn_name(
            &self,
            translator: &Arc<Translator>,
            args: serde_json::Map<String, serde_json::Value>,
        ) -> Result<CallToolResult, McpError> {
            let mut args = args;
            self.resolve_symbol_args(&mut args).await?;
            let params: $params_ty = ($build)(args);
            match translator.$handle_method(params).await {
                Ok(result) => Ok(CallToolResult::success(vec![
                    ContentBlock::json(result).map_err(|e| {
                        McpError::internal_error(format!("Failed to serialize result: {e}"), None)
                    })?,
                ])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
    };
    // Text result handler (e.g. hover)
    ($fn_name:ident, $handle_method:ident, $params_ty:ty, text: $field:ident, $build:expr) => {
        pub(super) async fn $fn_name(
            &self,
            translator: &Arc<Translator>,
            args: serde_json::Map<String, serde_json::Value>,
        ) -> Result<CallToolResult, McpError> {
            let mut args = args;
            self.resolve_symbol_args(&mut args).await?;
            let params: $params_ty = ($build)(args);
            match translator.$handle_method(params).await {
                Ok(result) => Ok(CallToolResult::success(vec![ContentBlock::text(
                    result.$field,
                )])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
    };
    // Resolve handler — deserializes a JSON field then builds params
    // $build is a closure |args, item| -> Params
    ($fn_name:ident, $handle_method:ident, $item_ty:ty, $item_field:ident, $build:expr) => {
        pub(super) async fn $fn_name(
            &self,
            translator: &Arc<Translator>,
            args: serde_json::Map<String, serde_json::Value>,
        ) -> Result<CallToolResult, McpError> {
            let $item_field: $item_ty = match serde_json::from_value(
                args.get(stringify!($item_field))
                    .cloned()
                    .unwrap_or_default(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    return Err(McpError::internal_error(
                        format!("Invalid {}: {e}", stringify!($item_field)),
                        None,
                    ))
                }
            };
            let params = ($build)(args, $item_field);
            match translator.$handle_method(params).await {
                Ok(result) => Ok(CallToolResult::success(vec![
                    ContentBlock::json(result).map_err(|e| {
                        McpError::internal_error(format!("Failed to serialize result: {e}"), None)
                    })?,
                ])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
    };
}

mod handlers;
mod positions;
mod references;
mod schemas;
#[cfg(test)]
mod tests;

use positions::*;
pub(super) use schemas::{create_schema, tool_list};

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
        // The changes the watcher has seen reach the language servers with
        // its re-index (`Translator::note_disk_changes`): wait for them, so
        // that a server answers from the files as they are.
        self.core.state().settle_watcher().await;

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
}
