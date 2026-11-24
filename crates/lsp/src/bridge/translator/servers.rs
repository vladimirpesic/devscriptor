//! Starting, stopping, restarting and describing the language servers, and
//! the projects and symbols they serve.

use super::*;

impl Translator {
    /// Handle start server.
    ///
    /// The server starts for `project` — the project of that path — or, with
    /// none given, for the workspace (see `Self::startup_folders`). A server
    /// already running for the language keeps running, restarted only if it
    /// has to take a project it does not cover yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot be started.
    pub async fn handle_start_server(
        &self,
        params: StartServerParams,
    ) -> Result<StartServerResult> {
        let language_id = params.language_id.as_str();
        let Some(config) = self.server_config(language_id) else {
            return Ok(StartServerResult {
                success: false,
                message: format!("No server configuration found for language: {language_id}"),
            });
        };
        let markers = || {
            config
                .heuristics
                .as_ref()
                .map(|h| h.markers.join(", "))
                .unwrap_or_default()
        };

        let _guard = self.lazy_spawn_lock.lock().await;
        let requested = match params.project.as_deref() {
            Some(project) => {
                let path = self.in_workspace(project);
                match Self::workspace_root_for(&config, &path) {
                    Some(root) => vec![canonical(&root)],
                    None => {
                        return Ok(StartServerResult {
                            success: false,
                            message: format!(
                                "{language_id}: no project at or above {} (looked for: {})",
                                path.display(),
                                markers()
                            ),
                        });
                    }
                }
            }
            None => self.startup_folders(&config),
        };
        if requested.is_empty() {
            return Ok(StartServerResult {
                success: false,
                message: format!(
                    "{language_id}: no project in {} (looked for: {}); pass `project`",
                    show_folders(&self.workspace_roots),
                    markers()
                ),
            });
        }

        let running = self.server_folders.read().await.get(language_id).cloned();
        let folders = match running {
            Some(mut folders) if self.clients.contains(language_id) => {
                let new: Vec<PathBuf> = requested
                    .into_iter()
                    .filter(|f| !folders.iter().any(|running| f.starts_with(running)))
                    .collect();
                if new.is_empty() {
                    return Ok(StartServerResult {
                        success: true,
                        message: format!(
                            "Server for {language_id} is already running for {}",
                            show_folders(&folders)
                        ),
                    });
                }
                folders.extend(new);
                folders
            }
            _ => requested,
        };

        match self.register_server(&config, folders.clone()).await {
            Ok(_) => Ok(StartServerResult {
                success: true,
                message: format!(
                    "Server for {language_id} started for {}",
                    show_folders(&folders)
                ),
            }),
            Err(e) => Ok(StartServerResult {
                success: false,
                message: format!("Failed to start server: {e}"),
            }),
        }
    }

    /// Handle stop server.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot be stopped.
    pub async fn handle_stop_server(&self, params: StopServerParams) -> Result<StopServerResult> {
        if self
            .stop_language(&params.language_id, StopReason::Requested)
            .await
        {
            Ok(StopServerResult {
                success: true,
                message: format!("Server for {} stopped successfully", params.language_id),
            })
        } else {
            Ok(StopServerResult {
                success: false,
                message: format!(
                    "No running server found for language: {}",
                    params.language_id
                ),
            })
        }
    }

    /// Handle restart server.
    ///
    /// The server restarts for the workspace folders it ran with, or — if none
    /// was running — for the workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot be restarted.
    pub async fn handle_restart_server(
        &self,
        params: RestartServerParams,
    ) -> Result<RestartServerResult> {
        let language_id = params.language_id.as_str();
        let Some(config) = self.server_config(language_id) else {
            return Ok(RestartServerResult {
                success: false,
                message: format!("No server configuration found for language: {language_id}"),
            });
        };
        // An explicit restart gives the server a fresh allowance of automatic ones.
        self.restarts.lock().remove(language_id);

        let _guard = self.lazy_spawn_lock.lock().await;
        let folders = self
            .server_folders
            .write()
            .await
            .remove(language_id)
            .filter(|folders| !folders.is_empty())
            .unwrap_or_else(|| self.startup_folders(&config));

        // Stop the existing server first (if any).
        if let Some(server) = self.servers.write().await.remove(language_id) {
            let lang = language_id.to_string();
            tokio::spawn(async move {
                if let Err(e) = server.shutdown().await {
                    warn!("LSP server '{lang}' shutdown error: {e}");
                }
            });
        }
        let _ = self.clients.remove(language_id);
        self.forget_documents(language_id).await;

        if folders.is_empty() {
            return Ok(RestartServerResult {
                success: false,
                message: format!(
                    "Restart failed: no {language_id} project in {}",
                    show_folders(&self.workspace_roots)
                ),
            });
        }
        match self.register_server(&config, folders.clone()).await {
            Ok(_) => Ok(RestartServerResult {
                success: true,
                message: format!(
                    "Server for {language_id} restarted for {}",
                    show_folders(&folders)
                ),
            }),
            Err(e) => Ok(RestartServerResult {
                success: false,
                message: format!("Restart failed: {e}"),
            }),
        }
    }

    /// Handle server status.
    ///
    /// # Errors
    ///
    /// Returns an error if status cannot be retrieved.
    pub async fn handle_server_status(
        &self,
        params: ServerStatusParams,
    ) -> Result<ServerStatusResult> {
        let running = self.servers.read().await;
        let start_errors = self.start_errors.read().await;
        let stop_reasons = self.stop_reasons.lock().clone();
        let status = |language_id: &str| {
            let server = running.get(language_id);
            let client = self.clients.get(language_id);
            ServerStatus {
                language_id: language_id.to_string(),
                state: match &client {
                    Some(client) if client.is_closed() => "exited".to_string(),
                    Some(_) => "running".to_string(),
                    None => "stopped".to_string(),
                },
                pid: server.and_then(LspServer::pid),
                uptime: server.map(LspServer::uptime_secs),
                last_error: start_errors.get(language_id).cloned(),
                stop_reason: client.is_none().then(|| {
                    stop_reasons
                        .get(language_id)
                        .map_or(StopReason::NOT_STARTED, |reason| reason.as_str())
                        .to_string()
                }),
            }
        };
        let servers: Vec<ServerStatus> = match params.language_id {
            Some(lang_id) if self.server_config(&lang_id).is_none() => {
                return Err(LspError::NoServerForLanguage(lang_id));
            }
            Some(lang_id) => vec![status(&lang_id)],
            None => self
                .config
                .servers
                .iter()
                .map(|s| status(&s.language_id))
                .collect(),
        };

        Ok(ServerStatusResult { servers })
    }

    /// Handle server capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error if capabilities cannot be retrieved.
    pub async fn handle_server_capabilities(
        &self,
        params: ServerCapabilitiesParams,
    ) -> Result<ServerCapabilitiesResult> {
        // The capabilities the server reported at `initialize`, stored on its
        // LspServer and reached through the servers registry (not the client
        // registry).
        self.start_for_request(&params.language_id).await?;
        let language_id = self.resolve_language(&params.language_id)?;
        let capabilities = {
            let servers = self.servers.read().await;
            if let Some(server) = servers.get(&language_id) {
                server.capabilities().clone()
            } else {
                return Err(LspError::ServerUnavailable(format!(
                    "the {language_id} language server is not running"
                )));
            }
        };
        Ok(ServerCapabilitiesResult { capabilities })
    }

    /// The folders `params.language_id`'s server has open, or the
    /// workspace's when no language is given.
    ///
    /// # Errors
    ///
    /// Returns an error if that server is not running.
    pub async fn handle_server_projects(
        &self,
        params: ServerProjectsParams,
    ) -> Result<ServerProjectsResult> {
        let folders = if params.language_id.is_empty() {
            self.workspace_roots.clone()
        } else {
            self.server_folders
                .read()
                .await
                .get(&params.language_id)
                .cloned()
                .ok_or_else(|| {
                    LspError::ServerUnavailable(format!(
                        "the {} language server is not running",
                        params.language_id
                    ))
                })?
        };
        let projects = folders
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        Ok(ServerProjectsResult { projects })
    }

    /// The files of the project `params.project` names, sorted: hidden
    /// files and what its ignore files leave out are not among them — not
    /// `.git`'s objects, nor a `.gitignore`d `node_modules` or `target`.
    ///
    /// # Errors
    ///
    /// Returns an error if the project is not a directory.
    pub async fn handle_project_files(
        &self,
        params: ProjectFilesParams,
    ) -> Result<ProjectFilesResult> {
        let project = self.in_workspace(params.project.as_deref().unwrap_or("."));
        if !project.is_dir() {
            return Err(LspError::InvalidPath(format!(
                "no project directory {}",
                project.display()
            )));
        }
        let mut all_files: Vec<String> = ignore::WalkBuilder::new(&project)
            .build()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(|entry| entry.path().to_string_lossy().into_owned())
            .collect();
        all_files.sort();

        let total = all_files.len() as u32;
        let files = all_files
            .into_iter()
            .skip(params.offset as usize)
            .take(params.limit as usize)
            .collect();

        Ok(ProjectFilesResult {
            files,
            pagination: PaginationInfo {
                total,
                offset: params.offset,
                has_more: params.offset + params.limit < total,
            },
        })
    }

    /// Handle symbols (paginated document symbols).
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file is not supported.
    pub async fn handle_symbols(&self, params: SymbolsParams) -> Result<SymbolsResult> {
        let mut symbols = self
            .handle_document_symbols(DocumentSymbolsParams {
                file_path: params.file_path,
            })
            .await?
            .symbols;

        let total = symbols.len() as u32;
        symbols = symbols
            .into_iter()
            .skip(params.offset as usize)
            .take(params.limit as usize)
            .collect();

        Ok(SymbolsResult {
            symbols,
            pagination: PaginationInfo {
                total,
                offset: params.offset,
                has_more: params.offset + params.limit < total,
            },
        })
    }

    /// The symbols of `params.language_id`'s server matching `params.query`,
    /// those under `params.project` when it names a directory, a page at a
    /// time.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or no servers are available.
    pub async fn handle_project_symbols(
        &self,
        params: ProjectSymbolsParams,
    ) -> Result<ProjectSymbolsResult> {
        self.start_for_request(&params.language_id).await?;
        let language_id = self.resolve_language(&params.language_id)?;
        let project = (!params.project.is_empty()).then(|| self.in_workspace(&params.project));
        if let Some(project) = &project {
            if !project.is_dir() {
                return Err(LspError::InvalidPath(format!(
                    "no project directory {}",
                    project.display()
                )));
            }
            self.open_project(&language_id, project).await?;
        }
        let client = self.clients.get(&language_id).ok_or_else(|| {
            LspError::ServerUnavailable(format!("the {language_id} language server is not running"))
        })?;
        if !self
            .server_supports(&language_id, "workspaceSymbolProvider")
            .await
        {
            return Err(LspError::Unsupported {
                language_id,
                feature: "workspace symbols".to_string(),
            });
        }

        self.sync_for_request(&client, &language_id).await?;
        let lsp_params = lsp_types::WorkspaceSymbolParams {
            query: params.query,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };

        let response: Option<lsp_types::WorkspaceSymbolResponse> =
            client.request("workspace/symbol", lsp_params).await?;

        let mut symbols: Vec<WorkspaceSymbol> = match response {
            Some(lsp_types::WorkspaceSymbolResponse::Flat(symbols)) => symbols
                .into_iter()
                .map(|s| WorkspaceSymbol {
                    name: s.name,
                    kind: format!("{:?}", s.kind),
                    location: location_from_lsp(s.location),
                    container_name: s.container_name,
                })
                .collect(),
            Some(lsp_types::WorkspaceSymbolResponse::Nested(symbols)) => symbols
                .into_iter()
                .filter_map(|s| match &s.location {
                    lsp_types::OneOf::Left(loc) => Some(WorkspaceSymbol {
                        name: s.name,
                        kind: format!("{:?}", s.kind),
                        location: Location::new(uri_to_path(&loc.uri), range_from_lsp(loc.range)),
                        container_name: s.container_name,
                    }),
                    lsp_types::OneOf::Right(_) => None,
                })
                .collect(),
            None => vec![],
        };
        if let Some(project) = &project {
            symbols.retain(|symbol| Path::new(&symbol.location.file_path).starts_with(project));
        }

        let total = symbols.len() as u32;
        symbols = symbols
            .into_iter()
            .skip(params.offset as usize)
            .take(params.limit as usize)
            .collect();

        Ok(ProjectSymbolsResult {
            symbols,
            pagination: PaginationInfo {
                total,
                offset: params.offset,
                has_more: params.offset + params.limit < total,
            },
        })
    }
}
