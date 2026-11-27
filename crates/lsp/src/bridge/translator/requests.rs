//! The LSP features an MCP tool asks for: each request translated to the
//! server and its answer back.

use super::*;

impl Translator {
    /// Handle hover request.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_hover(&self, params: HoverParams) -> Result<HoverResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::HoverParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<lsp_types::Hover> = self
            .request_feature(&client, file_path, "textDocument/hover", lsp_params)
            .await?;

        // lsp-types 0.97 HoverContents is not Option
        let contents = response
            .map(|h| match h.contents {
                lsp_types::HoverContents::Scalar(s) => marked_string_to_string(s),
                lsp_types::HoverContents::Array(arr) => arr
                    .into_iter()
                    .map(marked_string_to_string)
                    .collect::<Vec<_>>()
                    .join("\n---\n"),
                lsp_types::HoverContents::Markup(m) => m.value,
            })
            .unwrap_or_default();

        Ok(HoverResult { contents })
    }

    /// Handle go-to-definition.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_definition(&self, params: DefinitionParams) -> Result<DefinitionResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::GotoDefinitionResponse> = self
            .request_feature(&client, file_path, "textDocument/definition", lsp_params)
            .await?;

        let locations = match response {
            Some(lsp_types::GotoDefinitionResponse::Scalar(loc)) => vec![location_from_lsp(loc)],
            Some(lsp_types::GotoDefinitionResponse::Array(locs)) => {
                locs.into_iter().map(location_from_lsp).collect()
            }
            Some(lsp_types::GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|link| {
                    Location::new(
                        uri_to_path(&link.target_uri),
                        range_from_lsp(link.target_selection_range),
                    )
                })
                .collect(),
            None => vec![],
        };

        Ok(DefinitionResult { locations })
    }

    /// Handle find-references.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_references(&self, params: ReferencesParams) -> Result<ReferencesResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::ReferenceParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: lsp_types::ReferenceContext {
                include_declaration: params.include_declaration,
            },
        };

        let response: Option<Vec<lsp_types::Location>> = self
            .request_feature(&client, file_path, "textDocument/references", lsp_params)
            .await?;

        let locations = response
            .unwrap_or_default()
            .into_iter()
            .map(location_from_lsp)
            .collect();

        Ok(ReferencesResult {
            locations,
            ..ReferencesResult::default()
        })
    }

    /// Handle completions.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_completions(&self, params: CompletionsParams) -> Result<CompletionsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: None,
        };

        let response: Option<lsp_types::CompletionResponse> = self
            .request_feature(&client, file_path, "textDocument/completion", lsp_params)
            .await?;

        let items = match response {
            Some(lsp_types::CompletionResponse::Array(items)) => items,
            Some(lsp_types::CompletionResponse::List(list)) => list.items,
            None => vec![],
        };

        let completions = items
            .into_iter()
            .map(|item| Completion {
                label: item.label,
                kind: item.kind.map(|k| format!("{k:?}")),
                detail: item.detail,
                documentation: item.documentation.map(|doc| match doc {
                    lsp_types::Documentation::String(s) => s,
                    lsp_types::Documentation::MarkupContent(m) => m.value,
                }),
                insert_text: item.insert_text,
            })
            .collect();

        Ok(CompletionsResult { items: completions })
    }

    /// Handle diagnostics.
    ///
    /// A server that offers pull diagnostics (`diagnosticProvider`) is asked
    /// with `textDocument/diagnostic`. When it takes saves, it is told the
    /// file is saved first, and what it pushes is added to its answer: a check
    /// may run only on a save, and report only by push — rust-analyzer's
    /// answer holds its own analysis, while the errors `cargo check` finds,
    /// type errors among them, it pushes. The check is waited for, at most
    /// `wait_ms` (30 s by default); one still running then leaves its
    /// findings out but those it pushed since the save, and the answer says
    /// it is `checking`. One that does not offer pull diagnostics only pushes them —
    /// typescript-language-server, clangd, gopls, jdtls — so the answer is what
    /// it pushes for the file: after opening or refreshing it, the pushes that
    /// follow, waited for at most `wait_ms`; otherwise the last one. Asked with
    /// `textDocument/diagnostic`, those servers would fail every call with
    /// `-32601 Unhandled method`. When no push follows the file's refresh in
    /// time, the last push before it answers, and the answer says it is
    /// `stale` unless that push was empty. Diagnostics less severe than
    /// `severity` are left out.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_diagnostics(&self, params: DiagnosticsParams) -> Result<DiagnosticsResult> {
        let file_path = Path::new(&params.file_path);
        let language_id = self.get_language_id(file_path);
        // Pushes are this call's only if they follow the `didOpen`/`didChange`
        // that `open_file` sends, so mark where they stand before it.
        let mark = self
            .clients
            .get(&language_id)
            .map_or(0, |client| client.diagnostics_mark());
        let (client, refreshed) = self.open_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let cap = params.wait_ms.map(std::time::Duration::from_millis);
        let (diagnostics, stale, checking) = if self
            .server_supports(&language_id, "diagnosticProvider")
            .await
        {
            let saved = self.save_document(&client, &language_id, file_path).await?;
            let checked = match saved {
                Some(saved) => {
                    client
                        .await_check(saved.checks, cap.unwrap_or(CHECK_WAIT))
                        .await
                }
                None => true,
            };
            match Self::pull_diagnostics(&client, &uri).await {
                Err(LspError::LspServerError { code, .. }) if code == LSP_METHOD_NOT_FOUND => {
                    let (pushed, stale) =
                        Self::pushed_diagnostics(&client, &uri, mark, refreshed, cap).await;
                    (pushed, stale, false)
                }
                pulled => {
                    let mut diagnostics = pulled?;
                    let mut checking = false;
                    if let Some(saved) = saved {
                        // The check may have ended while the server answered.
                        let checked = checked
                            || client
                                .await_check(saved.checks, std::time::Duration::ZERO)
                                .await;
                        checking = !checked;
                        // While the check runs, a push before the save is of
                        // an older text.
                        let pushed = if checked {
                            client.pushed_diagnostics(uri.as_str())
                        } else {
                            client.pushed_since(uri.as_str(), saved.mark)
                        };
                        for diagnostic in pushed.unwrap_or_default() {
                            if !diagnostics.contains(&diagnostic) {
                                diagnostics.push(diagnostic);
                            }
                        }
                    }
                    (diagnostics, false, checking)
                }
            }
        } else {
            let (pushed, stale) =
                Self::pushed_diagnostics(&client, &uri, mark, refreshed, cap).await;
            (pushed, stale, false)
        };

        let least = params.severity.unwrap_or(DiagnosticSeverity::Hint);
        let diagnostics = diagnostics
            .into_iter()
            .map(|d| Diagnostic {
                cite: cite(
                    &params.file_path,
                    d.range.start.line,
                    d.range.start.character,
                ),
                range: range_from_lsp(d.range),
                severity: d.severity.map(|s| match s {
                    lsp_types::DiagnosticSeverity::ERROR => DiagnosticSeverity::Error,
                    lsp_types::DiagnosticSeverity::WARNING => DiagnosticSeverity::Warning,
                    lsp_types::DiagnosticSeverity::INFORMATION => DiagnosticSeverity::Information,
                    lsp_types::DiagnosticSeverity::HINT => DiagnosticSeverity::Hint,
                    _ => DiagnosticSeverity::Information,
                }),
                code: d.code.map(|c| match c {
                    lsp_types::NumberOrString::Number(n) => format!("{n}"),
                    lsp_types::NumberOrString::String(s) => s,
                }),
                source: d.source,
                message: d.message,
            })
            .filter(|d| {
                d.severity
                    .unwrap_or(DiagnosticSeverity::Error)
                    .at_least(least)
            })
            .collect();

        Ok(DiagnosticsResult {
            diagnostics,
            stale,
            checking,
        })
    }

    /// Ask `client` for `uri`'s diagnostics with `textDocument/diagnostic`.
    pub(super) async fn pull_diagnostics(
        client: &LspClient,
        uri: &lsp_types::Uri,
    ) -> Result<Vec<lsp_types::Diagnostic>> {
        let diag_params = lsp_types::DocumentDiagnosticParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            identifier: None,
            previous_result_id: None,
            partial_result_params: lsp_types::PartialResultParams::default(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: lsp_types::DocumentDiagnosticReportResult = client
            .request("textDocument/diagnostic", diag_params)
            .await?;
        Ok(match response {
            lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Full(full),
            ) => full.full_document_diagnostic_report.items,
            lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Unchanged(_),
            )
            | lsp_types::DocumentDiagnosticReportResult::Partial(_) => vec![],
        })
    }

    /// The diagnostics `client` pushes for `uri`: those it pushes after `mark`
    /// when the document was just sent to it (`refreshed`), else its last push;
    /// and whether they are stale: no push followed in time, and the server
    /// had pushed none, or a non-empty list, for the document before.
    ///
    /// A document sent for the first time is always pushed for once analysed,
    /// so that wait is long. A change to one already pushed for may push
    /// nothing — typescript-language-server skips a push that would repeat an
    /// empty list — so that wait is short and ends in the last push. `cap`
    /// bounds the whole wait, the pushes' settling included: past it, the
    /// latest push since `mark`, if any, answers.
    pub(super) async fn pushed_diagnostics(
        client: &LspClient,
        uri: &lsp_types::Uri,
        mark: u64,
        refreshed: bool,
        cap: Option<std::time::Duration>,
    ) -> (Vec<lsp_types::Diagnostic>, bool) {
        let last = client.pushed_diagnostics(uri.as_str());
        let wait = match (&last, refreshed) {
            (Some(_), false) => return (last.unwrap_or_default(), false),
            (Some(_), true) => PUSH_CHANGE_WAIT,
            (None, _) => PUSH_FIRST_WAIT,
        };
        let pushes = client.await_pushed_diagnostics(uri.as_str(), mark, wait, PUSH_SETTLE);
        let pushed = match cap {
            Some(cap) => tokio::time::timeout(cap, pushes)
                .await
                .unwrap_or_else(|_| client.pushed_since(uri.as_str(), mark)),
            None => pushes.await,
        };
        match pushed {
            Some(pushed) => (pushed, false),
            // An empty last push still holds: a server may skip a push that
            // would repeat an empty list.
            None => {
                let stale = last.as_ref().is_none_or(|last| !last.is_empty());
                (last.unwrap_or_default(), stale)
            }
        }
    }

    /// Handle document symbols.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_document_symbols(
        &self,
        params: DocumentSymbolsParams,
    ) -> Result<DocumentSymbolsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        self.ensure_capability(file_path, "documentSymbolProvider", "document symbols")
            .await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::DocumentSymbolParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::DocumentSymbolResponse> = client
            .request("textDocument/documentSymbol", lsp_params)
            .await?;
        let text = std::fs::read_to_string(file_path).ok();

        Ok(DocumentSymbolsResult {
            symbols: symbols_from_lsp(response, &params.file_path, text.as_deref()),
        })
    }

    /// Handle workspace symbol search: every running server that offers
    /// `workspace/symbol` is asked, and their answers are merged: in a polyglot
    /// workspace, typescript-language-server, with no project open, answers a
    /// query for a Rust function with "No Project", so asking one server alone
    /// can fail. A server that cannot answer is named in `note`, with its error.
    ///
    /// # Errors
    ///
    /// Returns an error if no server is running or offers workspace symbols,
    /// or none of those that do could answer.
    pub async fn handle_workspace_symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<WorkspaceSymbolResult> {
        self.start_for_request("").await?;
        let languages = self.clients.languages();
        if languages.is_empty() {
            return Err(LspError::ServerUnavailable(
                "no language server is running; call an LSP tool on a file first".into(),
            ));
        }
        let mut asked = Vec::new();
        for language in &languages {
            if self
                .server_supports(language, "workspaceSymbolProvider")
                .await
                && let Some(client) = self.clients.get(language)
            {
                asked.push((language.clone(), client));
            }
        }
        if asked.is_empty() {
            return Err(LspError::Unsupported {
                language_id: languages.join(", "),
                feature: "workspace symbols".to_string(),
            });
        }
        for (language, client) in &asked {
            self.sync_for_request(client, language).await?;
        }

        let answers = futures::future::join_all(asked.into_iter().map(|(language, client)| {
            let lsp_params = lsp_types::WorkspaceSymbolParams {
                query: params.query.clone(),
                work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            };
            async move {
                let response: Result<Option<lsp_types::WorkspaceSymbolResponse>> =
                    client.request("workspace/symbol", lsp_params).await;
                (language, response)
            }
        }))
        .await;
        let mut symbols = Vec::new();
        let mut answered = 0;
        let mut unanswered = Vec::new();
        for (language, response) in answers {
            match response {
                Ok(response) => {
                    answered += 1;
                    symbols.extend(workspace_symbols_from_lsp(response));
                }
                Err(e) => unanswered.push(format!("the {language} server: {e}")),
            }
        }
        if answered == 0 {
            return Err(LspError::ServerUnavailable(format!(
                "no language server could answer: {}",
                unanswered.join("; ")
            )));
        }
        let note = (!unanswered.is_empty()).then(|| {
            format!(
                "Not every running server answered, so symbols of their languages may be \
                 missing: {}",
                unanswered.join("; ")
            )
        });
        Ok(WorkspaceSymbolResult { symbols, note })
    }

    /// Handle document formatting.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_format_document(
        &self,
        params: FormatDocumentParams,
    ) -> Result<FormatDocumentResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::DocumentFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            options: lsp_types::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                trim_trailing_whitespace: Some(true),
                insert_final_newline: Some(true),
                trim_final_newlines: Some(true),
                ..Default::default()
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<Vec<lsp_types::TextEdit>> = self
            .request_feature(&client, file_path, "textDocument/formatting", lsp_params)
            .await
            .map_err(with_formatter_hint)?;

        let edits: Vec<TextEdit> = response
            .unwrap_or_default()
            .into_iter()
            .map(|e| TextEdit {
                range: range_from_lsp(e.range),
                new_text: e.new_text,
            })
            .collect();

        // Apply the formatting to disk — otherwise the tool is a silent no-op
        // that returns edits the caller must re-apply by hand.
        apply_edits_to_file(&params.file_path, &edits)?;

        Ok(FormatDocumentResult { edits })
    }

    /// Handle code actions.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_code_actions(
        &self,
        params: CodeActionsParams,
    ) -> Result<CodeActionsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            range: within_document(
                file_path,
                lsp_types::Range {
                    start: lsp_types::Position {
                        line: params.range.start.line,
                        character: params.range.start.character,
                    },
                    end: lsp_types::Position {
                        line: params.range.end.line,
                        character: params.range.end.character,
                    },
                },
            ),
            context: lsp_types::CodeActionContext {
                diagnostics: vec![],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::CodeActionResponse> = self
            .request_feature(&client, file_path, "textDocument/codeAction", lsp_params)
            .await?;

        let actions = response
            .unwrap_or_default()
            .into_iter()
            .map(|a| match a {
                lsp_types::CodeActionOrCommand::Command(cmd) => CodeAction {
                    title: cmd.title,
                    kind: None,
                    edit: None,
                    command: Some(cmd.command),
                },
                lsp_types::CodeActionOrCommand::CodeAction(action) => CodeAction {
                    title: action.title,
                    kind: action.kind.map(|k| k.as_str().to_string()),
                    edit: action.edit.map(|e| {
                        let changes: Vec<(String, Vec<TextEdit>)> = e
                            .changes
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(uri, edits)| {
                                (
                                    uri_to_path(&uri),
                                    edits
                                        .into_iter()
                                        .map(|e| TextEdit {
                                            range: range_from_lsp(e.range),
                                            new_text: e.new_text,
                                        })
                                        .collect(),
                                )
                            })
                            .collect();
                        WorkspaceEdit { changes }
                    }),
                    command: action.command.map(|c| c.command),
                },
            })
            .collect();

        Ok(CodeActionsResult { actions })
    }

    /// Handle rename.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_rename(&self, params: RenameParams) -> Result<RenameResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::RenameParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position {
                    line: params.line,
                    character: params.character,
                },
            },
            new_name: params.new_name,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<lsp_types::WorkspaceEdit> = self
            .request_feature(&client, file_path, "textDocument/rename", lsp_params)
            .await?;

        let mut changes: Vec<DocumentChanges> = Vec::new();
        if let Some(edit) = response {
            if let Some(map) = edit.changes {
                for (uri, edits) in map {
                    changes.push(DocumentChanges {
                        file_path: uri_to_path(&uri),
                        edits: edits.into_iter().map(text_edit_from_lsp).collect(),
                    });
                }
            }
            // Servers that advertise versioned edits (e.g. rust-analyzer, jdtls)
            // return `documentChanges` instead of the `changes` map; without this
            // branch a rename from those servers would silently return nothing.
            if changes.is_empty()
                && let Some(lsp_types::DocumentChanges::Edits(docs)) = edit.document_changes
            {
                for tde in docs {
                    let file_path = uri_to_path(&tde.text_document.uri);
                    let edits = tde
                        .edits
                        .into_iter()
                        .map(|oneof| match oneof {
                            lsp_types::OneOf::Left(te) => text_edit_from_lsp(te),
                            lsp_types::OneOf::Right(annotated) => {
                                text_edit_from_lsp(annotated.text_edit)
                            }
                        })
                        .collect();
                    changes.push(DocumentChanges { file_path, edits });
                }
            }
        }

        // Apply the workspace edit to disk so the rename actually happens rather
        // than returning edits the caller has to re-apply by hand.
        for change in &changes {
            apply_edits_to_file(&change.file_path, &change.edits)?;
        }

        Ok(RenameResult { changes })
    }

    /// Handle signature help.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> Result<SignatureHelpResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::SignatureHelpParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            context: None,
        };

        let response: Option<lsp_types::SignatureHelp> = self
            .request_feature(&client, file_path, "textDocument/signatureHelp", lsp_params)
            .await?;

        let signatures = response.map(|sh| {
            let sigs: Vec<Signature> = sh
                .signatures
                .into_iter()
                .map(|s| Signature {
                    label: s.label,
                    documentation: s.documentation.map(|d| match d {
                        lsp_types::Documentation::String(s) => s,
                        lsp_types::Documentation::MarkupContent(m) => m.value,
                    }),
                    parameters: s
                        .parameters
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| Parameter {
                            label: match p.label {
                                lsp_types::ParameterLabel::Simple(s) => s,
                                lsp_types::ParameterLabel::LabelOffsets(_) => String::new(),
                            },
                            documentation: p.documentation.map(|d| match d {
                                lsp_types::Documentation::String(s) => s,
                                lsp_types::Documentation::MarkupContent(m) => m.value,
                            }),
                        })
                        .collect(),
                })
                .collect();
            (sigs, sh.active_signature, sh.active_parameter)
        });

        Ok(SignatureHelpResult {
            signatures: signatures
                .as_ref()
                .map(|(s, _, _)| s.clone())
                .unwrap_or_default(),
            active_signature: signatures.as_ref().and_then(|(_, a, _)| *a),
            active_parameter: signatures.as_ref().and_then(|(_, _, a)| *a),
        })
    }

    /// Handle go-to-type-definition.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_type_definition(
        &self,
        params: TypeDefinitionParams,
    ) -> Result<TypeDefinitionResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::GotoDefinitionResponse> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/typeDefinition",
                lsp_params,
            )
            .await?;

        let locations = match response {
            Some(lsp_types::GotoDefinitionResponse::Scalar(loc)) => vec![location_from_lsp(loc)],
            Some(lsp_types::GotoDefinitionResponse::Array(locs)) => {
                locs.into_iter().map(location_from_lsp).collect()
            }
            Some(lsp_types::GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|link| {
                    Location::new(
                        uri_to_path(&link.target_uri),
                        range_from_lsp(link.target_selection_range),
                    )
                })
                .collect(),
            None => vec![],
        };

        Ok(TypeDefinitionResult { locations })
    }

    /// Handle go-to-implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_implementation(
        &self,
        params: ImplementationParams,
    ) -> Result<ImplementationResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::GotoDefinitionResponse> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/implementation",
                lsp_params,
            )
            .await?;

        let locations = match response {
            Some(lsp_types::GotoDefinitionResponse::Scalar(loc)) => vec![location_from_lsp(loc)],
            Some(lsp_types::GotoDefinitionResponse::Array(locs)) => {
                locs.into_iter().map(location_from_lsp).collect()
            }
            Some(lsp_types::GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|link| {
                    Location::new(
                        uri_to_path(&link.target_uri),
                        range_from_lsp(link.target_selection_range),
                    )
                })
                .collect(),
            None => vec![],
        };

        Ok(ImplementationResult { locations })
    }

    /// Handle call hierarchy preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_call_hierarchy(
        &self,
        params: CallHierarchyParams,
    ) -> Result<CallHierarchyResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::CallHierarchyPrepareParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<Vec<lsp_types::CallHierarchyItem>> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/prepareCallHierarchy",
                lsp_params,
            )
            .await?;

        let items = response
            .unwrap_or_default()
            .into_iter()
            .map(call_hierarchy_item_from_lsp)
            .collect();

        Ok(CallHierarchyResult { items })
    }

    /// Handle incoming calls.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_incoming_calls(
        &self,
        params: IncomingCallsParams,
    ) -> Result<IncomingCallsResult> {
        let item = &params.item;
        // Route to the server for the item's file, not an arbitrary one.
        let client = self
            .get_client_for_file(Path::new(&item.location.file_path))
            .await?;

        let lsp_item = call_hierarchy_item_to_lsp(item)?;

        let lsp_params = lsp_types::CallHierarchyIncomingCallsParams {
            item: lsp_item,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::CallHierarchyIncomingCall>> = self
            .request_feature(
                &client,
                Path::new(&item.location.file_path),
                "callHierarchy/incomingCalls",
                lsp_params,
            )
            .await?;

        let calls = response
            .unwrap_or_default()
            .into_iter()
            .map(|call| CallIncoming {
                from: call_hierarchy_item_from_lsp(call.from),
                from_ranges: call.from_ranges.into_iter().map(range_from_lsp).collect(),
            })
            .collect();

        Ok(IncomingCallsResult {
            calls,
            ..IncomingCallsResult::default()
        })
    }

    /// Handle outgoing calls.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_outgoing_calls(
        &self,
        params: OutgoingCallsParams,
    ) -> Result<OutgoingCallsResult> {
        let item = &params.item;
        // Route to the server for the item's file.
        let client = self
            .get_client_for_file(Path::new(&item.location.file_path))
            .await?;

        let lsp_item = call_hierarchy_item_to_lsp(item)?;

        let lsp_params = lsp_types::CallHierarchyOutgoingCallsParams {
            item: lsp_item,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::CallHierarchyOutgoingCall>> = self
            .request_feature(
                &client,
                Path::new(&item.location.file_path),
                "callHierarchy/outgoingCalls",
                lsp_params,
            )
            .await?;

        let calls = response
            .unwrap_or_default()
            .into_iter()
            .map(|call| CallOutgoing {
                to: call_hierarchy_item_from_lsp(call.to),
                from_ranges: call.from_ranges.into_iter().map(range_from_lsp).collect(),
            })
            .collect();

        Ok(OutgoingCallsResult { calls })
    }

    /// Handle type hierarchy preparation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_type_hierarchy(
        &self,
        params: TypeHierarchyParams,
    ) -> Result<TypeHierarchyResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::TypeHierarchyPrepareParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<Vec<lsp_types::TypeHierarchyItem>> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/prepareTypeHierarchy",
                lsp_params,
            )
            .await?;

        let items = response
            .unwrap_or_default()
            .into_iter()
            .map(type_hierarchy_item_from_lsp)
            .collect();

        Ok(TypeHierarchyResult { items })
    }

    /// Handle subtypes request.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_subtypes(&self, params: SubtypesParams) -> Result<SubtypesResult> {
        let item = &params.item;
        // Route to the server for the item's file.
        let client = self
            .get_client_for_file(Path::new(&item.location.file_path))
            .await?;

        let lsp_item = type_hierarchy_item_to_lsp(item)?;

        let lsp_params = lsp_types::TypeHierarchySubtypesParams {
            item: lsp_item,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::TypeHierarchyItem>> = self
            .request_feature(
                &client,
                Path::new(&item.location.file_path),
                "typeHierarchy/subtypes",
                lsp_params,
            )
            .await?;

        let subtypes = response
            .unwrap_or_default()
            .into_iter()
            .map(type_hierarchy_item_from_lsp)
            .collect();

        Ok(SubtypesResult { subtypes })
    }

    /// Handle supertypes request.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_supertypes(&self, params: SupertypesParams) -> Result<SupertypesResult> {
        let item = &params.item;
        // Route to the server for the item's file.
        let client = self
            .get_client_for_file(Path::new(&item.location.file_path))
            .await?;

        let lsp_item = type_hierarchy_item_to_lsp(item)?;

        let lsp_params = lsp_types::TypeHierarchySupertypesParams {
            item: lsp_item,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::TypeHierarchyItem>> = self
            .request_feature(
                &client,
                Path::new(&item.location.file_path),
                "typeHierarchy/supertypes",
                lsp_params,
            )
            .await?;

        let supertypes = response
            .unwrap_or_default()
            .into_iter()
            .map(type_hierarchy_item_from_lsp)
            .collect();

        Ok(SupertypesResult { supertypes })
    }

    /// Handle document highlights.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_document_highlights(
        &self,
        params: DocumentHighlightsParams,
    ) -> Result<DocumentHighlightsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::DocumentHighlightParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::DocumentHighlight>> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/documentHighlight",
                lsp_params,
            )
            .await?;

        let highlights = response
            .unwrap_or_default()
            .into_iter()
            .map(|h| DocumentHighlight {
                cite: cite(
                    &params.file_path,
                    h.range.start.line,
                    h.range.start.character,
                ),
                range: range_from_lsp(h.range),
                kind: h.kind.map(|k| format!("{k:?}")),
            })
            .collect();

        Ok(DocumentHighlightsResult { highlights })
    }

    /// Handle document colors.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_document_colors(
        &self,
        params: DocumentColorsParams,
    ) -> Result<DocumentColorsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::DocumentColorParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::ColorInformation>> = self
            .request_feature(&client, file_path, "textDocument/documentColor", lsp_params)
            .await?;

        let colors = response
            .unwrap_or_default()
            .into_iter()
            .map(|c| ColorInformation {
                cite: cite(
                    &params.file_path,
                    c.range.start.line,
                    c.range.start.character,
                ),
                range: range_from_lsp(c.range),
                color: Color {
                    red: f64::from(c.color.red),
                    green: f64::from(c.color.green),
                    blue: f64::from(c.color.blue),
                    alpha: f64::from(c.color.alpha),
                },
            })
            .collect();

        Ok(DocumentColorsResult { colors })
    }

    /// Handle folding ranges.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_folding_ranges(
        &self,
        params: FoldingRangesParams,
    ) -> Result<FoldingRangesResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::FoldingRangeParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::FoldingRange>> = self
            .request_feature(&client, file_path, "textDocument/foldingRange", lsp_params)
            .await?;

        let ranges = response
            .unwrap_or_default()
            .into_iter()
            .map(|r| FoldingRange {
                cite: cite(
                    &params.file_path,
                    r.start_line,
                    r.start_character.unwrap_or(0),
                ),
                start_line: r.start_line,
                start_character: r.start_character,
                end_line: r.end_line,
                end_character: r.end_character,
                kind: r.kind.map(|k| format!("{k:?}")),
            })
            .collect();

        Ok(FoldingRangesResult { ranges })
    }

    /// Handle selection range.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<SelectionRangeResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let positions: Vec<lsp_types::Position> = params
            .positions
            .into_iter()
            .map(|p| lsp_types::Position {
                line: p.line,
                character: p.character,
            })
            .collect();

        let lsp_params = lsp_types::SelectionRangeParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            positions,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::SelectionRange>> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/selectionRange",
                lsp_params,
            )
            .await?;

        fn convert_selection_range(path: &str, sr: &lsp_types::SelectionRange) -> SelectionRange {
            SelectionRange {
                cite: cite(path, sr.range.start.line, sr.range.start.character),
                range: range_from_lsp(sr.range),
                parent: sr
                    .parent
                    .as_ref()
                    .map(|p| Box::new(convert_selection_range(path, p))),
            }
        }

        let ranges = response
            .unwrap_or_default()
            .into_iter()
            .map(|r| convert_selection_range(&params.file_path, &r))
            .collect();

        Ok(SelectionRangeResult { ranges })
    }

    /// Handle linked editing range.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<LinkedEditingRangeResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let uri = path_to_uri(file_path)?;
        let position = lsp_types::Position {
            line: params.line,
            character: params.character,
        };

        let lsp_params = lsp_types::LinkedEditingRangeParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<lsp_types::LinkedEditingRanges> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/linkedEditingRange",
                lsp_params,
            )
            .await?;

        let ranges = response.map(|r| {
            let ranges: Vec<Range> = r.ranges.into_iter().map(range_from_lsp).collect();
            (ranges, r.word_pattern)
        });

        Ok(LinkedEditingRangeResult {
            ranges: ranges.as_ref().map(|(r, _)| r.clone()).unwrap_or_default(),
            word_pattern: ranges.and_then(|(_, p)| p),
        })
    }

    /// Handle inlay hints.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_inlay_hints(&self, params: InlayHintsParams) -> Result<InlayHintsResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::InlayHintParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            range: within_document(
                file_path,
                lsp_types::Range {
                    start: lsp_types::Position {
                        line: params.start_line,
                        character: params.start_character,
                    },
                    end: lsp_types::Position {
                        line: params.end_line,
                        character: params.end_character,
                    },
                },
            ),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<Vec<lsp_types::InlayHint>> = self
            .request_feature(&client, file_path, "textDocument/inlayHint", lsp_params)
            .await?;

        let hints = response
            .unwrap_or_default()
            .into_iter()
            .map(|h| InlayHint {
                cite: cite(&params.file_path, h.position.line, h.position.character),
                position: Position2D {
                    line: h.position.line,
                    character: h.position.character,
                },
                label: match h.label {
                    lsp_types::InlayHintLabel::String(s) => s,
                    lsp_types::InlayHintLabel::LabelParts(parts) => {
                        parts.into_iter().map(|p| p.value).collect::<String>()
                    }
                },
                kind: h.kind.map(|k| format!("{k:?}")),
                tooltip: h.tooltip.map(|t| match t {
                    lsp_types::InlayHintTooltip::String(s) => s,
                    lsp_types::InlayHintTooltip::MarkupContent(m) => m.value,
                }),
            })
            .collect();

        Ok(InlayHintsResult { hints })
    }

    /// Handle document links.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_document_links(
        &self,
        params: DocumentLinksParams,
    ) -> Result<DocumentLinksResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::DocumentLinkParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<Vec<lsp_types::DocumentLink>> = self
            .request_feature(&client, file_path, "textDocument/documentLink", lsp_params)
            .await?;

        let links = response
            .unwrap_or_default()
            .into_iter()
            .map(|l| DocumentLink {
                cite: cite(
                    &params.file_path,
                    l.range.start.line,
                    l.range.start.character,
                ),
                range: range_from_lsp(l.range),
                target: l.target.map(|t| t.to_string()),
                tooltip: l.tooltip,
                data: l.data,
            })
            .collect();

        Ok(DocumentLinksResult { links })
    }

    /// Handle semantic tokens.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_semantic_tokens(
        &self,
        params: SemanticTokensParams,
    ) -> Result<SemanticTokensResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::SemanticTokensParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::SemanticTokens> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/semanticTokens/full",
                lsp_params,
            )
            .await?;

        let tokens = match response {
            Some(t) => SemanticTokensResult {
                result_id: t.result_id,
                data: t
                    .data
                    .into_iter()
                    .flat_map(|token| {
                        [
                            token.delta_line,
                            token.delta_start,
                            token.length,
                            token.token_type,
                            token.token_modifiers_bitset,
                        ]
                    })
                    .collect(),
            },
            None => SemanticTokensResult {
                result_id: None,
                data: vec![],
            },
        };

        Ok(tokens)
    }

    /// Handle range formatting.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_format_range(
        &self,
        params: FormatRangeParams,
    ) -> Result<FormatRangeResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;
        let uri = path_to_uri(file_path)?;

        let lsp_params = lsp_types::DocumentRangeFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            range: within_document(
                file_path,
                lsp_types::Range {
                    start: lsp_types::Position {
                        line: params.range.start.line,
                        character: params.range.start.character,
                    },
                    end: lsp_types::Position {
                        line: params.range.end.line,
                        character: params.range.end.character,
                    },
                },
            ),
            options: lsp_types::FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                trim_trailing_whitespace: Some(true),
                insert_final_newline: Some(true),
                trim_final_newlines: Some(true),
                ..Default::default()
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };

        let response: Option<Vec<lsp_types::TextEdit>> = self
            .request_feature(
                &client,
                file_path,
                "textDocument/rangeFormatting",
                lsp_params,
            )
            .await
            .map_err(with_formatter_hint)?;

        let edits: Vec<TextEdit> = response
            .unwrap_or_default()
            .into_iter()
            .map(|e| TextEdit {
                range: range_from_lsp(e.range),
                new_text: e.new_text,
            })
            .collect();

        // Apply the formatting to disk (see handle_format_document).
        apply_edits_to_file(&params.file_path, &edits)?;

        Ok(FormatRangeResult { edits })
    }

    /// Handle completion resolve.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_completion_resolve(
        &self,
        params: CompletionResolveParams,
    ) -> Result<CompletionResolveResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let item = lsp_types::CompletionItem {
            label: params.item.label,
            kind: params
                .item
                .kind
                .as_ref()
                .and_then(|k| parse_completion_kind(k)),
            detail: params.item.detail,
            documentation: params.item.documentation.map(|d| {
                lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: d,
                })
            }),
            insert_text: params.item.insert_text.clone(),
            ..Default::default()
        };

        let response: lsp_types::CompletionItem = self
            .request_feature(&client, file_path, "completionItem/resolve", item)
            .await?;

        Ok(CompletionResolveResult {
            item: Completion {
                label: response.label,
                kind: response.kind.map(|k| format!("{k:?}")),
                detail: response.detail,
                documentation: response.documentation.map(|d| match d {
                    lsp_types::Documentation::String(s) => s,
                    lsp_types::Documentation::MarkupContent(m) => m.value,
                }),
                insert_text: response.insert_text,
            },
        })
    }

    /// Handle code action resolve.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_code_action_resolve(
        &self,
        params: CodeActionResolveParams,
    ) -> Result<CodeActionResolveResult> {
        let file_path = Path::new(&params.file_path);
        let client = self.get_client_for_file(file_path).await?;

        let action = lsp_types::CodeAction {
            title: params.action.title,
            kind: params.action.kind.map(lsp_types::CodeActionKind::from),
            edit: None,
            command: params.action.command.map(|c| lsp_types::Command {
                title: c.clone(),
                command: c,
                arguments: None,
            }),
            is_preferred: None,
            disabled: None,
            data: None,
            diagnostics: None,
        };

        let response: lsp_types::CodeAction = self
            .request_feature(&client, file_path, "codeAction/resolve", action)
            .await?;

        Ok(CodeActionResolveResult {
            action: CodeAction {
                title: response.title,
                kind: response.kind.map(|k| k.as_str().to_string()),
                edit: response.edit.map(|e| {
                    let changes: Vec<(String, Vec<TextEdit>)> = e
                        .changes
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(uri, edits)| {
                            (
                                uri_to_path(&uri),
                                edits
                                    .into_iter()
                                    .map(|e| TextEdit {
                                        range: range_from_lsp(e.range),
                                        new_text: e.new_text,
                                    })
                                    .collect(),
                            )
                        })
                        .collect();
                    WorkspaceEdit { changes }
                }),
                command: response.command.map(|c| c.command),
            },
        })
    }
}
