//! The handlers of the LSP tools that ask the translator one request each,
//! and those of the call and type hierarchies.

use super::*;

impl UnifiedMcpServer {
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

    pub(super) async fn get_incoming_calls_impl(
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

    pub(super) async fn get_outgoing_calls_impl(
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
    pub(super) async fn call_hierarchy_item(
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
    pub(super) async fn symbol_position(
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

    pub(super) async fn get_subtypes_impl(
        &self,
        translator: &Arc<Translator>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        let item = self.type_hierarchy_item(translator, args).await?;
        json_result(translator.handle_subtypes(SubtypesParams { item }).await)
    }

    pub(super) async fn get_supertypes_impl(
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
    pub(super) async fn type_hierarchy_item(
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
                .map(String::from);
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
