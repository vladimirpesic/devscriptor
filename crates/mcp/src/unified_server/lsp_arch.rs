//! LSP Architecture Tool Handlers (5 tools) — only compiled with `lsp` feature.

use rmcp::ErrorData as McpError;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock;
use rmcp::model::Tool;
use serde_json::json;

use devscriptor_lsp::config::StartupMode;

use super::UnifiedMcpServer;
use super::lsp_tools::create_schema;

/// Build the 5 LSP architecture tool schema definitions for `list_tools`.
pub(super) fn arch_tool_list() -> Vec<Tool> {
    vec![
        Tool::new(
            "lsp_arch_detect",
            "Detect languages in a project and resolve/plan the LSP servers it needs; \
             starts the installed ones in the background (`prewarming`), so call it \
             before the first LSP tool",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "project_path": {"type": "string"}
                }
            })),
        ),
        Tool::new(
            "lsp_arch_check",
            "Check whether a single LSP server is available and where it resolves, or the `reason` it does not run here. Name it by its id (`dart-ls`) or by its language (`dart`)",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "server_id": {
                        "type": "string",
                        "description": "A server id, such as `rust-analyzer`, or a language, such as `rust`"
                    }
                },
                "required": ["server_id"]
            })),
        ),
        Tool::new(
            "lsp_arch_batch_check",
            "Check availability of several LSP servers at once, each named by its id (`dart-ls`) or its language (`dart`); one that is not available comes with the `reason`",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "server_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Server ids, such as `rust-analyzer`, or languages, such as `rust`"
                    }
                },
                "required": ["server_ids"]
            })),
        ),
        Tool::new(
            "lsp_arch_status",
            "Report resolution status for all known LSP servers, grouped by source; one installed here that cannot run is under `cannot_run`, and each not available has its `reason`",
            create_schema(json!({
                "type": "object",
                "properties": {}
            })),
        ),
        Tool::new(
            "lsp_arch_setup",
            "Detect a project's languages and (optionally) install the servers it needs",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "project_path": {"type": "string"},
                    "auto_install": {"type": "boolean"},
                    "servers_to_install": {"type": "array", "items": {"type": "string"}}
                }
            })),
        ),
    ]
}

impl UnifiedMcpServer {
    pub(super) async fn handle_lsp_arch_tool(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        match name {
            "lsp_arch_detect" => self.lsp_arch_detect_impl(args).await,
            "lsp_arch_check" => self.lsp_arch_check_impl(args).await,
            "lsp_arch_batch_check" => self.lsp_arch_batch_check_impl(args).await,
            "lsp_arch_status" => self.lsp_arch_status_impl(args).await,
            "lsp_arch_setup" => self.lsp_arch_setup_impl(args).await,
            _ => Err(McpError::internal_error(
                format!("Unknown LSP Architecture tool: {name}"),
                None,
            )),
        }
    }

    /// With lazy startup, start the servers of `languages` that are installed,
    /// not running and applicable to the workspace, in the background: an
    /// agent calls `lsp_arch_detect` before its first LSP request, which then
    /// finds them running instead of waiting for a cold start. Returns the
    /// languages being started. Failures are kept for `get_server_status`
    /// (`last_error`), never reported here.
    fn prewarm(&self, languages: impl IntoIterator<Item = String>) -> Vec<String> {
        let Some(translator) = &self.translator else {
            return Vec::new();
        };
        if translator.startup_mode() != StartupMode::Lazy {
            return Vec::new();
        }
        let starting = translator.startable_languages(languages);
        if !starting.is_empty() {
            let translator = std::sync::Arc::clone(translator);
            let languages = starting.clone();
            tokio::spawn(async move {
                if let Err(e) = translator.spawn_applicable(Some(&languages)).await {
                    tracing::warn!(
                        "Background start of the {} server(s) failed: {e}",
                        languages.join(", ")
                    );
                }
            });
        }
        starting
    }

    async fn lsp_arch_detect_impl(
        &self,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use devscriptor_lsp::mcp::tools::{
            DetectedLanguageInfo, LspArchDetectResult, LspSourceInfo, PrerequisiteInfo,
            ResolvedServer, ServerMissingPrerequisites, ServerNeedsInstall,
        };

        let project_path = args
            .get("project_path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        let path = std::path::Path::new(&project_path);

        let manager = self.lsp_arch_manager.lock().await;
        let result = manager.analyze_project(path);

        let languages: Vec<DetectedLanguageInfo> = result
            .languages
            .iter()
            .map(|lang| DetectedLanguageInfo {
                language_id: lang.language_id.clone(),
                confidence: lang.confidence,
                indicator_files: lang
                    .indicator_files
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                suggested_servers: lang.suggested_servers.clone(),
            })
            .collect();

        let resolved_servers: Vec<ResolvedServer> = result
            .resolved_servers
            .iter()
            .map(|(server_id, source, _config)| ResolvedServer {
                server_id: server_id.clone(),
                source: LspSourceInfo::from(source),
            })
            .collect();

        let needs_install: Vec<ServerNeedsInstall> = result
            .needs_install
            .iter()
            .map(|(server_id, prereq_result)| ServerNeedsInstall {
                server_id: server_id.clone(),
                reason: reason_unavailable(server_id),
                satisfied_prerequisites: prereq_result
                    .satisfied
                    .iter()
                    .map(|p| PrerequisiteInfo {
                        name: p.name().to_string(),
                        install_instructions: p.install_instructions(),
                    })
                    .collect(),
                missing_prerequisites: prereq_result
                    .missing
                    .iter()
                    .map(|p| PrerequisiteInfo {
                        name: p.name().to_string(),
                        install_instructions: p.install_instructions(),
                    })
                    .collect(),
            })
            .collect();

        let missing_prerequisites: Vec<ServerMissingPrerequisites> = result
            .missing_prerequisites
            .iter()
            .map(|(server_id, prereq_result)| ServerMissingPrerequisites {
                server_id: server_id.clone(),
                reason: reason_unavailable(server_id),
                missing_prerequisites: prereq_result
                    .missing
                    .iter()
                    .map(|p| PrerequisiteInfo {
                        name: p.name().to_string(),
                        install_instructions: p.install_instructions(),
                    })
                    .collect(),
            })
            .collect();

        let prewarming = self.prewarm(
            languages
                .iter()
                .map(|language| language.language_id.clone())
                .chain(
                    result
                        .resolved_servers
                        .iter()
                        .map(|(_, _, config)| config.language_id.clone()),
                ),
        );

        let detect_result = LspArchDetectResult {
            languages,
            resolved_servers,
            needs_install,
            missing_prerequisites,
            prewarming,
        };

        Ok(CallToolResult::success(vec![
            ContentBlock::json(detect_result).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ]))
    }

    async fn lsp_arch_check_impl(
        &self,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use devscriptor_lsp::lsp_architecture::{quick_check, server_id_for};
        use devscriptor_lsp::mcp::tools::{LspArchCheckResult, LspSourceInfo};

        let name = args
            .get("server_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() {
            return Err(McpError::internal_error("server_id is required", None));
        }

        let check_result = match server_id_for(&name) {
            Some(server_id) => {
                let source = quick_check(server_id);
                LspArchCheckResult {
                    server_id: server_id.to_string(),
                    requested: (server_id != name).then_some(name),
                    available: source.is_some(),
                    reason: source
                        .is_none()
                        .then(|| reason_unavailable(server_id))
                        .flatten(),
                    source: source.as_ref().map(LspSourceInfo::from),
                    unknown: None,
                }
            }
            None => LspArchCheckResult {
                unknown: Some(format!(
                    "{name} is no server and no language a server serves; the server ids are: {}",
                    devscriptor_lsp::generated_registry::SERVER_IDS.join(", ")
                )),
                server_id: name,
                requested: None,
                available: false,
                source: None,
                reason: None,
            },
        };

        Ok(CallToolResult::success(vec![
            ContentBlock::json(check_result).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ]))
    }

    async fn lsp_arch_batch_check_impl(
        &self,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use devscriptor_lsp::lsp_architecture::{batch_check, server_id_for};
        use devscriptor_lsp::mcp::tools::{
            BatchCheckResultItem, LspArchBatchCheckResult, LspSourceInfo, UnknownServers,
        };

        let server_ids: Vec<String> = args
            .get("server_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if server_ids.is_empty() {
            return Err(McpError::internal_error(
                "server_ids array is required",
                None,
            ));
        }

        // A language stands for its server; a name that is neither is set
        // apart rather than reported as a server that is not installed.
        let (known, unknown): (Vec<(String, &str)>, Vec<String>) = server_ids.into_iter().fold(
            (Vec::new(), Vec::new()),
            |(mut known, mut unknown), name| {
                match server_id_for(&name) {
                    Some(id) => known.push((name, id)),
                    None => unknown.push(name),
                }
                (known, unknown)
            },
        );
        let server_refs: Vec<&str> = known.iter().map(|(_, id)| *id).collect();
        let batch_results = batch_check(&server_refs);

        let mut available_count = 0;
        let mut missing_count = 0;

        let results: Vec<BatchCheckResultItem> = batch_results
            .into_iter()
            .zip(known)
            .map(|((server_id, source), (name, _))| {
                let (available, source_info, reason) = if let Some(src) = source {
                    available_count += 1;
                    (true, Some(LspSourceInfo::from(&src)), None)
                } else {
                    missing_count += 1;
                    (false, None, reason_unavailable(&server_id))
                };
                BatchCheckResultItem {
                    requested: (server_id != name).then_some(name),
                    server_id,
                    available,
                    source: source_info,
                    reason,
                }
            })
            .collect();

        let batch_result = LspArchBatchCheckResult {
            results,
            available_count,
            missing_count,
            unknown: (!unknown.is_empty()).then(|| UnknownServers {
                names: unknown,
                server_ids: devscriptor_lsp::generated_registry::SERVER_IDS
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect(),
            }),
        };

        Ok(CallToolResult::success(vec![
            ContentBlock::json(batch_result).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ]))
    }

    async fn lsp_arch_status_impl(
        &self,
        _args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use devscriptor_lsp::lsp_architecture::{
            LspResolver, LspSource, check_prerequisites_async,
        };
        use devscriptor_lsp::mcp::tools::{
            LspArchStatusResult, LspServerStatus, LspSourceInfo, PrerequisiteInfo,
            ServerStatusInfo, ServersBySource,
        };

        let resolver = match LspResolver::new() {
            Ok(r) => r,
            Err(e) => {
                return Err(McpError::internal_error(
                    format!("Failed to create resolver: {e}"),
                    None,
                ));
            }
        };

        let mut bundled = Vec::new();
        let mut system = Vec::new();
        let mut installed = Vec::new();
        let mut vscode = Vec::new();
        let mut user_config = Vec::new();
        let mut needs_install = Vec::new();
        let mut missing_prereqs = Vec::new();
        let mut cannot_run = Vec::new();

        let common_servers = vec![
            "rust-analyzer",
            "typescript-language-server",
            "pyright",
            "gopls",
            "jdtls",
            "kotlin-lsp",
            "csharp-ls",
            "clangd",
            "bash-language-server",
            "html-languageserver",
            "graphql-language-service",
            "volar",
            "svelte-language-server",
            "ruby-lsp",
            "phpactor",
            "sql-language-server",
            "fsautocomplete",
            "sourcekit-lsp",
            "dart-ls",
            "metals",
            "nomicfoundation-solc",
        ];

        for server_id in &common_servers {
            let resolution = resolver.resolve(server_id);
            let prereq_result = check_prerequisites_async(server_id).await;

            if let Some(result) = &resolution {
                let info = LspSourceInfo::from(&result.source);
                let status = ServerStatusInfo {
                    server_id: server_id.to_string(),
                    status: LspServerStatus::Available,
                    source: Some(info),
                    missing_prerequisites: None,
                    reason: None,
                };
                match &result.source {
                    LspSource::UserConfig { .. } => user_config.push(status),
                    LspSource::Installed(_) => installed.push(status),
                    LspSource::Bundled(_) => bundled.push(status),
                    LspSource::SystemWide(_) => system.push(status),
                    LspSource::VsCodeExtension(_) => vscode.push(status),
                }
                continue;
            }

            // An install that is there but cannot run needs no install: it
            // is told apart, with why.
            let unavailable = devscriptor_lsp::manager::why_unavailable(server_id);
            if let Some(unavailable) = unavailable.as_ref().filter(|u| u.installed) {
                cannot_run.push(ServerStatusInfo {
                    server_id: server_id.to_string(),
                    status: LspServerStatus::CannotRun,
                    source: None,
                    missing_prerequisites: None,
                    reason: Some(unavailable.reason.clone()),
                });
                continue;
            }
            let reason = unavailable.map(|u| u.reason);
            if prereq_result.is_satisfied() {
                needs_install.push(ServerStatusInfo {
                    server_id: server_id.to_string(),
                    status: LspServerStatus::NeedsInstall,
                    source: None,
                    missing_prerequisites: None,
                    reason,
                });
            } else {
                missing_prereqs.push(ServerStatusInfo {
                    server_id: server_id.to_string(),
                    status: LspServerStatus::MissingPrerequisites,
                    reason,
                    source: None,
                    missing_prerequisites: Some(
                        prereq_result
                            .missing
                            .iter()
                            .map(|p| PrerequisiteInfo {
                                name: p.name().to_string(),
                                install_instructions: p.install_instructions(),
                            })
                            .collect(),
                    ),
                });
            }
        }

        let total_checked = bundled.len()
            + system.len()
            + installed.len()
            + vscode.len()
            + user_config.len()
            + needs_install.len()
            + missing_prereqs.len()
            + cannot_run.len();
        let available_count =
            bundled.len() + system.len() + installed.len() + vscode.len() + user_config.len();

        let status_result = LspArchStatusResult {
            servers: ServersBySource {
                bundled,
                system,
                installed,
                vscode,
                user_config,
                needs_install,
                missing_prerequisites: missing_prereqs,
                cannot_run,
            },
            total_checked,
            available_count,
        };

        Ok(CallToolResult::success(vec![
            ContentBlock::json(status_result).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ]))
    }

    async fn lsp_arch_setup_impl(
        &self,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        use devscriptor_lsp::mcp::tools::{
            DetectedLanguageInfo, LspArchSetupResult, LspSourceInfo, PrerequisiteInfo,
            ResolvedServer, ServerInstallResult, ServerMissingPrerequisites, ServerNeedsInstall,
        };

        let project_path = args
            .get("project_path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        let auto_install = args
            .get("auto_install")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let servers_to_install: Vec<String> = args
            .get("servers_to_install")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let path = std::path::Path::new(&project_path);

        // Extract all analysis data while holding the lock, then drop it
        // before any async operations to avoid blocking concurrent calls.
        let (languages, already_available, needs_install, missing_prerequisites) = {
            let manager = self.lsp_arch_manager.lock().await;
            let analysis_result = manager.analyze_project(path);

            let languages: Vec<DetectedLanguageInfo> = analysis_result
                .languages
                .iter()
                .map(|lang| DetectedLanguageInfo {
                    language_id: lang.language_id.clone(),
                    confidence: lang.confidence,
                    indicator_files: lang
                        .indicator_files
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect(),
                    suggested_servers: lang.suggested_servers.clone(),
                })
                .collect();

            let already_available: Vec<ResolvedServer> = analysis_result
                .resolved_servers
                .iter()
                .map(|(server_id, source, _config)| {
                    let source_info = LspSourceInfo::from(source);
                    ResolvedServer {
                        server_id: server_id.clone(),
                        source: source_info,
                    }
                })
                .collect();

            let needs_install: Vec<(
                String,
                devscriptor_lsp::lsp_architecture::prerequisites::PrerequisitesResult,
            )> = analysis_result.needs_install.into_iter().collect();

            let missing_prerequisites: Vec<ServerMissingPrerequisites> = analysis_result
                .missing_prerequisites
                .iter()
                .map(|(server_id, prereq_result)| ServerMissingPrerequisites {
                    server_id: server_id.clone(),
                    reason: reason_unavailable(server_id),
                    missing_prerequisites: prereq_result
                        .missing
                        .iter()
                        .map(|p| PrerequisiteInfo {
                            name: p.name().to_string(),
                            install_instructions: p.install_instructions(),
                        })
                        .collect(),
                })
                .collect();

            (
                languages,
                already_available,
                needs_install,
                missing_prerequisites,
            )
        }; // lock dropped here

        let mut installed: Vec<ServerInstallResult> = Vec::new();
        let mut still_needed: Vec<ServerNeedsInstall> = Vec::new();

        for (server_id, prereq_result) in &needs_install {
            let should_install = auto_install
                && (servers_to_install.is_empty() || servers_to_install.contains(server_id));

            if should_install {
                let install_result = self.install_server(server_id).await;
                installed.push(ServerInstallResult {
                    server_id: server_id.clone(),
                    success: install_result.is_ok(),
                    error: install_result.err().map(|e| e.to_string()),
                });
            } else {
                still_needed.push(ServerNeedsInstall {
                    server_id: server_id.clone(),
                    reason: reason_unavailable(server_id),
                    satisfied_prerequisites: prereq_result
                        .satisfied
                        .iter()
                        .map(|p| PrerequisiteInfo {
                            name: p.name().to_string(),
                            install_instructions: p.install_instructions(),
                        })
                        .collect(),
                    missing_prerequisites: prereq_result
                        .missing
                        .iter()
                        .map(|p| PrerequisiteInfo {
                            name: p.name().to_string(),
                            install_instructions: p.install_instructions(),
                        })
                        .collect(),
                });
            }
        }

        let setup_complete = still_needed.is_empty() && missing_prerequisites.is_empty();

        let summary = if setup_complete {
            format!(
                "Setup complete! {} languages detected, {} servers already available, {} servers installed.",
                languages.len(),
                already_available.len(),
                installed.len()
            )
        } else {
            format!(
                "Setup partially complete. {} languages detected, {} servers available, {} servers still need installation, {} servers have missing prerequisites.",
                languages.len(),
                already_available.len(),
                still_needed.len(),
                missing_prerequisites.len()
            )
        };

        let setup_result = LspArchSetupResult {
            languages,
            already_available,
            installed,
            still_needed,
            missing_prerequisites,
            setup_complete,
            summary,
        };

        Ok(CallToolResult::success(vec![
            ContentBlock::json(setup_result).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ]))
    }

    async fn install_server(&self, server_id: &str) -> anyhow::Result<()> {
        use devscriptor_lsp::manager::InstallationResult;

        let result = self.lsp_manager.install(server_id, None).await?;
        match result {
            InstallationResult::Success => Ok(()),
            InstallationResult::AlreadyInstalled => Ok(()),
            InstallationResult::Failed(e) => Err(anyhow::anyhow!("Installation failed: {e}")),
            InstallationResult::NotInstalled => Err(anyhow::anyhow!("Server not installed")),
            InstallationResult::MissingPrerequisite(p) => {
                Err(anyhow::anyhow!("Missing prerequisite: {p}"))
            }
        }
    }
}

/// Why the registry's server `server_id` does not run here, and what makes it
/// run, as its install shows: an agent told only that `ruby-lsp` is not
/// available would report it not installed, when a link to a build for
/// another Ruby is.
fn reason_unavailable(server_id: &str) -> Option<String> {
    devscriptor_lsp::manager::why_unavailable(server_id).map(|unavailable| unavailable.reason)
}
