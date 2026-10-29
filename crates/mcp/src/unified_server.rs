//! Unified MCP Server - Core + LSP Integration
//!
//! Combines 32 Core tools with 38 LSP tools and 5 LSP Architecture tools
//! for a total of 75 MCP tools, served with the rmcp SDK.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use crate::core_module::{CoreModule, ToolResult};
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
    Tool,
};
use rmcp::service::{RequestContext, RoleServer};

// LSP module types (only available with lsp feature)
#[cfg(feature = "lsp")]
use devscriptor_lsp::bridge::Translator;
#[cfg(feature = "lsp")]
use devscriptor_lsp::config::{Config, LifecycleOverrides, StartupMode};
#[cfg(feature = "lsp")]
use devscriptor_lsp::lsp_architecture::LspArchitectureManager;
#[cfg(feature = "lsp")]
use devscriptor_lsp::manager::LspManager;

/// Static set of core tool names for O(1) dispatch in [`call_tool`].
static CORE_TOOLS: std::sync::LazyLock<HashSet<&'static str>> = std::sync::LazyLock::new(|| {
    HashSet::from([
        "scan_codebase",
        "search_entities",
        "analyze_code",
        "get_statistics",
        "start_analysis",
        "get_analysis_progress",
        "list_analysis_jobs",
        "cleanup_old_jobs",
        "search_functions",
        "search_classes",
        "fuzzy_search",
        "semantic_search",
        "generate_embeddings",
        "get_entity_details",
        "get_relationships",
        "find_dead_code",
        "analyze_complexity",
        "add_context_note",
        "search_context",
        "get_related_context",
        "prune_context",
        "get_context_statistics",
        "describe_file",
        "describe_modification",
        "get_file_context",
        "health_check",
        "validate_file_edit",
        "query_with_context",
        "cancel_analysis",
        "start_watching",
        "stop_watching",
        "get_watch_status",
    ])
});

#[cfg(feature = "lsp")]
/// Static set of LSP tool names for O(1) dispatch in [`call_tool`].
static LSP_TOOLS: std::sync::LazyLock<HashSet<&'static str>> = std::sync::LazyLock::new(|| {
    HashSet::from([
        "get_hover",
        "get_definition",
        "get_references",
        "get_type_definitions",
        "get_implementations",
        "get_completions",
        "get_completion_resolve",
        "get_diagnostics",
        "get_code_actions",
        "get_code_action_resolve",
        "get_document_symbols",
        "get_workspace_symbols",
        "get_symbols",
        "get_project_symbols",
        "get_call_hierarchy",
        "get_incoming_calls",
        "get_outgoing_calls",
        "get_type_hierarchy",
        "get_subtypes",
        "get_supertypes",
        "get_highlights",
        "get_colors",
        "get_folding_ranges",
        "get_selection_range",
        "get_linked_editing_range",
        "get_inlay_hints",
        "get_links",
        "get_semantic_tokens",
        "format_document",
        "get_range_format",
        "rename_symbol",
        "get_server_status",
        "get_server_capabilities",
        "get_server_projects",
        "get_project_files",
        "start_server",
        "stop_server",
        "restart_server",
    ])
});

#[cfg(feature = "lsp")]
/// Static set of LSP architecture tool names for O(1) dispatch.
static LSP_ARCH_TOOLS: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        HashSet::from([
            "lsp_arch_detect",
            "lsp_arch_check",
            "lsp_arch_batch_check",
            "lsp_arch_status",
            "lsp_arch_setup",
        ])
    });

// --------------------------------------------------------------------------
// Declarative macro: eliminates ~1 000 lines of identical boilerplate.
// --------------------------------------------------------------------------
#[cfg(feature = "lsp")]
macro_rules! lsp_handler {
    // Standard handler — serialises result as JSON
    ($fn_name:ident, $handle_method:ident, $params_ty:ty, $build:expr) => {
        #[cfg(feature = "lsp")]
        async fn $fn_name(
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
        #[cfg(feature = "lsp")]
        async fn $fn_name(
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
        #[cfg(feature = "lsp")]
        async fn $fn_name(
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

// --------------------------------------------------------------------------
// Domain submodules (must follow macro so `lsp_handler!` is in scope).
// --------------------------------------------------------------------------
#[cfg(feature = "lsp")]
mod lsp_arch;
#[cfg(feature = "lsp")]
mod lsp_tools;

/// How `devscriptor mcp` was asked to run, from its command line.
#[cfg(feature = "lsp")]
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// Language server lifecycle settings given on the command line; they
    /// outrank the environment and the configuration file.
    pub lsp: LifecycleOverrides,
    /// `--no-watch`: do not watch the scanned root the server runs in.
    pub no_watch: bool,
    /// `--config`: the project configuration file to follow instead of the
    /// workspace's `devscriptor.yaml`.
    pub config: Option<std::path::PathBuf>,
}

/// Environment variable that, set to `0` or `false`, keeps `devscriptor mcp`
/// from watching the scanned root it runs in, as `--no-watch` does.
#[cfg(feature = "lsp")]
pub const WATCH_ENV_VAR: &str = "DEVSCRIPTOR_WATCH";

#[cfg(feature = "lsp")]
impl ServerOptions {
    /// Whether to watch the scanned root the server runs in: unless
    /// `--no-watch` was given or [`WATCH_ENV_VAR`] is `0` or `false`.
    #[must_use]
    pub fn watch(&self) -> bool {
        !self.no_watch
            && std::env::var(WATCH_ENV_VAR).map_or(true, |value| {
                !matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false")
            })
    }
}

/// Unified MCP server combining Core and LSP capabilities.
#[derive(Clone)]
pub struct UnifiedMcpServer {
    /// Core module for code analysis tools.
    core: Arc<CoreModule>,
    /// LSP translator for language server tools (optional).
    #[cfg(feature = "lsp")]
    translator: Option<Arc<Translator>>,
    /// LSP server manager for installation.
    #[cfg(feature = "lsp")]
    lsp_manager: Arc<LspManager>,
    /// LSP architecture manager for detection.
    #[cfg(feature = "lsp")]
    lsp_arch_manager: Arc<tokio::sync::Mutex<LspArchitectureManager>>,
}

impl UnifiedMcpServer {
    /// Create a new unified server with core module only.
    pub fn new(core: Arc<CoreModule>) -> anyhow::Result<Self> {
        #[cfg(feature = "lsp")]
        let lsp_manager = Arc::new(LspManager::new()?);
        #[cfg(feature = "lsp")]
        let lsp_arch_manager = Arc::new(tokio::sync::Mutex::new(LspArchitectureManager::new()?));

        Ok(Self {
            core,
            #[cfg(feature = "lsp")]
            translator: None,
            #[cfg(feature = "lsp")]
            lsp_manager,
            #[cfg(feature = "lsp")]
            lsp_arch_manager,
        })
    }

    /// Create a new unified server with both core and LSP.
    #[cfg(feature = "lsp")]
    pub fn with_lsp(core: Arc<CoreModule>, translator: Arc<Translator>) -> anyhow::Result<Self> {
        let lsp_manager = Arc::new(LspManager::new()?);
        let lsp_arch_manager = Arc::new(tokio::sync::Mutex::new(LspArchitectureManager::new()?));

        Ok(Self {
            core,
            translator: Some(translator),
            lsp_manager,
            lsp_arch_manager,
        })
    }

    /// Create a new unified server with default core module.
    pub fn new_default() -> anyhow::Result<Self> {
        let state = Arc::new(crate::server::DevscriptorServerState::new()?);
        let core = Arc::new(CoreModule::new(state));
        Self::new(core)
    }

    /// Build the LSP translator, tolerating a workspace whose language servers
    /// are not installed.
    ///
    /// With lazy startup (the default) nothing is started here: each server
    /// starts when a tool first needs it. With eager startup every applicable
    /// server starts before the MCP server answers. `Translator::spawn_applicable`
    /// returns `Ok` when *no* server applies to the workspace, but
    /// `Err(AllServersFailedToInit)` when every applicable server fails to spawn
    /// — e.g. a Rust project on a machine without `rust-analyzer`. Propagating
    /// that error killed the whole MCP server at startup, taking the 32 core
    /// tools and the 5 architecture tools down with it — including
    /// `lsp_arch_setup`, the very tool that installs the missing server.
    ///
    /// So a failed eager start is downgraded to a warning: the translator is
    /// returned with zero clients, every tool stays advertised, individual LSP
    /// calls fail with a clear per-call error, and `lsp_arch_setup` /
    /// `start_server` can bring servers up afterwards.
    ///
    /// The lifecycle settings in effect come from `options`, the environment
    /// and the configuration file, in that order of precedence
    /// ([`LifecycleConfig::resolve`](devscriptor_lsp::config::LifecycleConfig::resolve));
    /// an invalid environment value is an error.
    #[cfg(feature = "lsp")]
    async fn build_translator(lsp: LifecycleOverrides) -> anyhow::Result<Translator> {
        // An explicit `devscriptor-lsp.toml` (or `$DEVSCRIPTOR_LSP_CONFIG`) is
        // authoritative. Otherwise use every registry server installed on this
        // machine rather than the three hard-coded in `Config::default`, which
        // silently limited all 38 LSP tools to Rust, Python and TypeScript.
        let mut config = match Config::explicit()? {
            Some(config) => config,
            None => Config::from_installed_servers(),
        };
        config.lifecycle = config.lifecycle.resolve_from_env(lsp)?;
        let translator = Translator::new(config);
        if translator.startup_mode() == StartupMode::Eager
            && let Err(e) = translator.spawn_applicable(None).await
        {
            tracing::warn!(
                "No language server could be started ({e}). Continuing without LSP: \
                 core and architecture tools remain available. Run `lsp_arch_setup` \
                 (or `devscriptor lsp setup`) to install the servers this project needs."
            );
        }
        Ok(translator)
    }

    /// Create a new unified server with LSP configuration, for the project in
    /// `workspace`: it follows the project's configuration file
    /// (`options.config`, else `devscriptor.yaml` there).
    ///
    /// `db_path` overrides the default database location; pass `None` to use the
    /// shared, cwd-independent default.
    #[cfg(feature = "lsp")]
    pub async fn new_with_lsp(
        db_path: Option<std::path::PathBuf>,
        workspace: &std::path::Path,
        options: ServerOptions,
    ) -> anyhow::Result<Self> {
        let project =
            crate::project::ProjectSettings::for_workspace(workspace, options.config.as_deref());
        let db_path =
            db_path.unwrap_or_else(crate::server::DevscriptorServerState::default_db_path);
        let state = Arc::new(crate::server::DevscriptorServerState::for_project(
            db_path, project,
        )?);
        let core = Arc::new(CoreModule::new(state));

        let translator = Arc::new(Self::build_translator(options.lsp).await?);
        translator.spawn_maintenance();

        Self::with_lsp(core, translator)
    }

    /// The server's shared state.
    #[cfg(feature = "lsp")]
    #[must_use]
    pub(crate) fn state(&self) -> &Arc<crate::server::DevscriptorServerState> {
        self.core.state()
    }

    /// Stop what the server runs before it exits: its language servers and
    /// its watcher; then checkpoint its index.
    #[cfg(feature = "lsp")]
    pub(crate) async fn shutdown(&self) {
        if let Some(translator) = &self.translator {
            translator.shutdown().await;
        }
        self.state().stop_code_watching();
        if let Err(e) = self.state().database().checkpoint().await {
            tracing::warn!("Checkpointing the index failed: {e}");
        }
    }

    // ======================================================================
    // Core tool dispatch
    // ======================================================================
    async fn handle_core_tool(
        &self,
        name: &str,
        args: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        match self.core.handle_tool(name, args).await {
            Ok(ToolResult::Success(result)) => Ok(CallToolResult::success(vec![
                ContentBlock::json(result).map_err(|e| {
                    McpError::internal_error(format!("Failed to serialize: {e}"), None)
                })?,
            ])),
            // Release builds redact filesystem paths from the error text an agent
            // sees (see docs/privacy.md); debug builds pass it through.
            Ok(ToolResult::Error(e)) => Ok(failed(devscriptor_core::sanitize_message(&e))),
            Err(e) => Ok(failed(devscriptor_core::sanitize_message(&e.to_string()))),
        }
    }
}

/// A call that failed, as MCP has a tool report it: a result with `isError`,
/// which the model reads and can correct its call by. Protocol errors are
/// for a tool this server does not have, and for a malformed request, which
/// the SDK rejects before any handler runs; a call that runs and fails — a
/// missing or invalid argument, a name the index does not know, a language
/// server that cannot start, a throttled tool — gets this answer.
fn failed(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// What a tool handler's error becomes: its message, as the failed call's
/// answer ([`failed`]).
#[cfg(feature = "lsp")]
fn answered(outcome: Result<CallToolResult, McpError>) -> CallToolResult {
    outcome.unwrap_or_else(|error| failed(error.message))
}

/// Every tool this build serves, in `tools/list` order, each annotated with
/// what it does to its environment ([`crate::tool_annotations`]).
fn all_tools() -> Vec<Tool> {
    #[allow(unused_mut)]
    let mut tools = crate::tool_schemas::CORE_TOOLS.clone();
    #[cfg(feature = "lsp")]
    {
        tools.extend(lsp_tools::tool_list());
        tools.extend(lsp_arch::arch_tool_list());
    }
    tools
        .into_iter()
        .map(
            |tool| match crate::tool_annotations::effect_of(&tool.name) {
                Some(effect) => tool.annotate(effect.annotations()),
                None => tool,
            },
        )
        .collect()
}

/// The `instructions` sent at initialization, counted from the tool lists
/// themselves so they cannot drift from what `tools/list` serves.
fn instructions() -> String {
    let core = crate::tool_schemas::CORE_TOOLS.len();
    #[allow(unused_mut)]
    let mut total = core;
    #[allow(unused_mut)]
    let mut groups = format!(
        "\u{2022} {core} Core tools: Code analysis, search, semantic embeddings, context management, real-time watcher"
    );
    #[cfg(feature = "lsp")]
    {
        let lsp = lsp_tools::tool_list().len();
        let arch = lsp_arch::arch_tool_list().len();
        total += lsp + arch;
        groups.push_str(&format!(
            "\n\u{2022} {lsp} LSP tools: Language server features\
             \n\u{2022} {arch} LSP Architecture tools: Detection, setup, and management"
        ));
    }
    format!(
        "Devscriptor Unified MCP Server - Complete {total}-tool implementation:\n{groups}\n\
         Supports 26 languages and frameworks with advanced code intelligence."
    )
}

// ==========================================================================
// ServerHandler trait implementation
// ==========================================================================
impl ServerHandler for UnifiedMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            // The answer to a client that asks for a revision this server does
            // not support; one it does support is answered in kind.
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
            // `Implementation::from_build_env()` reads *rmcp's* `CARGO_*` vars,
            // because `env!` is expanded inside the rmcp crate, and would name
            // this server after the SDK. Name ourselves.
            .with_server_info(Implementation::new(
                "devscriptor",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(instructions())
    }

    /// Every revision that still opens with an `initialize` handshake. The
    /// server only lists and calls tools, which all of them define alike;
    /// 2026-07-28, which drops the handshake, is not offered, so its clients
    /// fall back to 2025-11-25.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(
            &ProtocolVersion::LATEST_WITH_INITIALIZE,
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.as_ref();
        let args = request.arguments;
        // A workspace indexed since by a scan made elsewhere is watched now,
        // and a change of the project's configuration file is taken.
        self.core.state().check_workspace_watch();
        self.core.state().refresh_project_config();

        // O(1) HashSet dispatch.
        if CORE_TOOLS.contains(name) {
            let args = args.map(serde_json::Value::Object);
            return self.handle_core_tool(name, args).await.map(Into::into);
        }
        #[cfg(feature = "lsp")]
        if LSP_TOOLS.contains(name) {
            let args = args.unwrap_or_default();
            return Ok(answered(self.handle_lsp_tool(name, args).await).into());
        }
        #[cfg(feature = "lsp")]
        if LSP_ARCH_TOOLS.contains(name) {
            let args = args.unwrap_or_default();
            return Ok(answered(self.handle_lsp_arch_tool(name, args).await).into());
        }

        // The spec's answer to a call naming no tool the server has.
        Err(McpError::invalid_params(
            format!("Unknown tool: {name}"),
            None,
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(all_tools()))
    }
}

// ==========================================================================
// Entry-point helpers
// ==========================================================================

/// Serve the given MCP server over stdio and wait for completion.
async fn serve_stdio(server: UnifiedMcpServer) -> anyhow::Result<()> {
    use rmcp::service::ServiceExt;

    let service = server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {e}"))?;

    let _ = service.waiting().await;
    Ok(())
}

/// Run the unified MCP server with stdio transport (core + LSP tools).
///
/// `db_path` is the database to open. `None` selects the shared default that
/// `crate::server::DevscriptorServerState::default_db_path` resolves, which is
/// independent of the working directory — agents scan absolute paths from any cwd.
/// `options` are the command line's settings.
#[cfg(feature = "lsp")]
pub async fn run_unified_server(
    db_path: Option<std::path::PathBuf>,
    options: ServerOptions,
) -> anyhow::Result<()> {
    // Count from the same source `list_tools` serves, so the banner can never
    // drift from the real tool count (core + LSP + LSP-architecture).
    let tool_count = all_tools().len();
    tracing::info!("Starting Unified MCP Server ({tool_count} tools)");
    let workspace = std::env::current_dir()?;
    let watch = options.watch();
    let server = UnifiedMcpServer::new_with_lsp(db_path, &workspace, options).await?;
    if watch {
        // In the background: the priming scan of a large tree takes a while,
        // and the server answers meanwhile.
        let state = Arc::clone(server.core.state());
        tokio::spawn(async move {
            state.watch_workspace(&workspace).await;
        });
    }
    serve_stdio(server).await
}

/// Run the unified MCP server without LSP (core only).
pub async fn run_core_only_server() -> anyhow::Result<()> {
    let tool_count = crate::tool_schemas::CORE_TOOLS.len();
    tracing::info!("Starting Core-only MCP Server ({tool_count} tools)");
    let server = UnifiedMcpServer::new_default()?;
    serve_stdio(server).await
}
