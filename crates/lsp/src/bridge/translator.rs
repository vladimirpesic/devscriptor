//! Bridge layer for translating between MCP and LSP protocols.
//!
//! This module provides the translation layer that converts MCP tool requests
//! into LSP requests and back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::bridge::ClientRegistry;
use crate::config::{Config, LspServerConfig, StartupMode};
use crate::error::{LspError, Result};
use crate::lsp::client::LspClient;
use crate::lsp::lifecycle::{LspServer, spawn_servers_in};
use crate::mcp::tools::{
    CallHierarchyItem, CallHierarchyParams, CallHierarchyResult, CallIncoming, CallOutgoing,
    CodeAction, CodeActionResolveParams, CodeActionResolveResult, CodeActionsParams,
    CodeActionsResult, Color, ColorInformation, Completion, CompletionResolveParams,
    CompletionResolveResult, CompletionsParams, CompletionsResult, DefinitionParams,
    DefinitionResult, Diagnostic, DiagnosticSeverity, DiagnosticsParams, DiagnosticsResult,
    DocumentChanges, DocumentColorsParams, DocumentColorsResult, DocumentHighlight,
    DocumentHighlightsParams, DocumentHighlightsResult, DocumentLink, DocumentLinksParams,
    DocumentLinksResult, DocumentSymbolsParams, DocumentSymbolsResult, FoldingRange,
    FoldingRangesParams, FoldingRangesResult, FormatDocumentParams, FormatDocumentResult,
    FormatRangeParams, FormatRangeResult, HoverParams, HoverResult, ImplementationParams,
    ImplementationResult, IncomingCallsParams, IncomingCallsResult, InlayHint, InlayHintsParams,
    InlayHintsResult, LinkedEditingRangeParams, LinkedEditingRangeResult, Location,
    OutgoingCallsParams, OutgoingCallsResult, PaginationInfo, Parameter, Position2D,
    ProjectFilesParams, ProjectFilesResult, ProjectSymbolsParams, ProjectSymbolsResult, Range,
    ReferencesParams, ReferencesResult, RenameParams, RenameResult, RestartServerParams,
    RestartServerResult, SelectionRange, SelectionRangeParams, SelectionRangeResult,
    SemanticTokensParams, SemanticTokensResult, ServerCapabilitiesParams, ServerCapabilitiesResult,
    ServerProjectsParams, ServerProjectsResult, ServerStatus, ServerStatusParams,
    ServerStatusResult, Signature, SignatureHelpParams, SignatureHelpResult, StartServerParams,
    StartServerResult, StopServerParams, StopServerResult, SubtypesParams, SubtypesResult,
    SupertypesParams, SupertypesResult, Symbol, SymbolsParams, SymbolsResult, TextEdit,
    TypeDefinitionParams, TypeDefinitionResult, TypeHierarchyItem, TypeHierarchyParams,
    TypeHierarchyResult, WorkspaceEdit, WorkspaceSymbol, WorkspaceSymbolParams,
    WorkspaceSymbolResult, cite,
};

/// Translator that bridges MCP and LSP protocols.
pub struct Translator {
    /// Workspace root directories.
    workspace_roots: Vec<PathBuf>,
    /// LSP clients by language ID (thread-safe registry).
    clients: ClientRegistry,
    /// LSP server processes by language ID.
    ///
    /// The `LspServer` owns the spawned `tokio::process::Child` (with
    /// `kill_on_drop(false)`), so the process lives as long as its entry here: a
    /// dropped `LspServer` kills its process.
    ///
    /// Wrapped in `Arc<RwLock<…>>` so `&self` methods (e.g. `handle_start_server`,
    /// `handle_stop_server`, `handle_restart_server`) can change the registry.
    servers: Arc<RwLock<HashMap<String, LspServer>>>,
    /// Extension to language ID mapping.
    extension_map: HashMap<String, String>,
    /// Server configuration.
    config: Config,
    /// Serializes lazy, on-demand server startup so a burst of concurrent
    /// requests for the same language spawns exactly one process.
    lazy_spawn_lock: Arc<tokio::sync::Mutex<()>>,
    /// Documents this translator has announced to a server, by URI.
    ///
    /// A language server only answers position requests for files it has been
    /// told about, so every request announces its file first (`didOpen`): the
    /// *first* `get_hover` / `get_definition` / … against a newly started server
    /// would otherwise race its indexing and fail with `file not found`.
    open_documents: Arc<RwLock<HashMap<String, OpenDocument>>>,
    /// The workspace folders each running server was started with, by
    /// language ID.
    server_folders: Arc<RwLock<HashMap<String, Vec<PathBuf>>>>,
    /// Monotonic counter ordering document use, for [`MAX_OPEN_DOCUMENTS`].
    ticks: Arc<std::sync::atomic::AtomicU64>,
    /// Why each language's server last failed to start, until one starts.
    /// `get_server_status` reports it as `last_error`.
    start_errors: Arc<RwLock<HashMap<String, String>>>,
    /// When each language's server was last restarted automatically after
    /// exiting, within [`Self::RESTART_WINDOW`].
    restarts: Arc<parking_lot::Mutex<HashMap<String, Vec<std::time::Instant>>>>,
    /// Why each stopped language's server is down, until it starts again.
    /// A stopped language without an entry was never started.
    stop_reasons: Arc<parking_lot::Mutex<HashMap<String, StopReason>>>,
}

/// Why a language's server stopped, as `get_server_status` reports it
/// (`stop_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    /// Unused for longer than the idle timeout.
    Idle,
    /// `stop_server` stopped it.
    Requested,
    /// Stopped, as the least recently used, to start another server within
    /// `max_running_servers`.
    Cap,
}

impl StopReason {
    /// The `stop_reason` of a server never started.
    const NOT_STARTED: &'static str = "not started";

    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Requested => "requested",
            Self::Cap => "cap",
        }
    }
}

/// A document announced to a language server, and the on-disk state it reflects.
#[derive(Debug, Clone)]
struct OpenDocument {
    /// LSP document version, incremented on every `didChange`.
    version: i32,
    /// Modification time of the file when it was last sent to the server.
    mtime: Option<std::time::SystemTime>,
    /// When a request last used it, as a tick of [`Translator::ticks`].
    last_used: u64,
}

/// How many documents each language server is kept holding open. A long agent
/// session would otherwise keep hundreds of files resident (and re-analysed) in
/// its servers; past this many the least recently used are closed. A closed
/// file is reopened, from disk, by the next request for it.
const MAX_OPEN_DOCUMENTS: usize = 128;

/// The documents to close so that at most `cap` of `language_id`'s stay open:
/// its least recently used, never `keep`. `language_of` maps a document's path
/// to its language.
fn documents_to_close(
    open: &HashMap<String, OpenDocument>,
    language_of: impl Fn(&str) -> String,
    language_id: &str,
    keep: &str,
    cap: usize,
) -> Vec<String> {
    let mut same: Vec<(&String, u64)> = open
        .iter()
        .filter(|(path, _)| language_of(path) == language_id)
        .map(|(path, doc)| (path, doc.last_used))
        .collect();
    if same.len() <= cap {
        return Vec::new();
    }
    let excess = same.len() - cap;
    same.sort_by_key(|&(path, used)| (used, path.clone()));
    same.into_iter()
        .filter(|(path, _)| path.as_str() != keep)
        .take(excess)
        .map(|(path, _)| path.clone())
        .collect()
}

/// JSON-RPC `MethodNotFound`: what a server that has no handler for a request
/// answers it with.
const LSP_METHOD_NOT_FOUND: i32 = -32601;

/// How long a server that pushes diagnostics gets to push them for a document
/// it has just been sent for the first time, once it reports itself ready.
const PUSH_FIRST_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long it gets to push after a change to a document it has pushed for.
const PUSH_CHANGE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// How long a document goes without a push before its diagnostics count as
/// complete.
const PUSH_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

impl Translator {
    /// Create a new translator.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let mut extension_map = HashMap::new();

        // Build extension to language mapping from config
        for server in &config.servers {
            for ext in &server.extensions {
                extension_map.insert(ext.clone(), server.language_id.clone());
            }
        }

        Self {
            workspace_roots: config.workspace.roots.clone(),
            clients: ClientRegistry::new(),
            servers: Arc::new(RwLock::new(HashMap::new())),
            extension_map,
            config,
            lazy_spawn_lock: Arc::new(tokio::sync::Mutex::new(())),
            open_documents: Arc::new(RwLock::new(HashMap::new())),
            server_folders: Arc::new(RwLock::new(HashMap::new())),
            ticks: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            start_errors: Arc::new(RwLock::new(HashMap::new())),
            restarts: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            stop_reasons: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    /// How long `language_id`'s server has gone unused ([`LspClient::idle_for`]);
    /// `None` while it is in use or not running.
    fn idle_for(&self, language_id: &str) -> Option<std::time::Duration> {
        self.clients
            .get(language_id)
            .and_then(|client| client.idle_for())
    }

    /// Start the task that stops servers left unused for longer than
    /// `idle_timeout_secs`, checking every `maintenance_tick_ms`. It holds the
    /// translator weakly and ends once the translator is dropped. Does nothing
    /// when the idle timeout is 0. Call it once, from within a Tokio runtime.
    pub fn spawn_maintenance(self: &Arc<Self>) {
        let lifecycle = &self.config.lifecycle;
        if lifecycle.idle_timeout_secs == 0 {
            return;
        }
        let tick = std::time::Duration::from_millis(lifecycle.maintenance_tick_ms.max(1));
        let translator = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(tick);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick completes at once.
            ticks.tick().await;
            loop {
                ticks.tick().await;
                let Some(translator) = translator.upgrade() else {
                    return;
                };
                translator.stop_idle_servers().await;
            }
        });
    }

    /// Stop every running server unused for longer than `idle_timeout_secs`:
    /// no request, notification or readiness wait in progress, and none for
    /// that long ([`LspClient::idle_for`]). The next request for its language
    /// starts it again; an idle stop is not a crash and does not count against
    /// [`Self::MAX_RESTARTS`].
    async fn stop_idle_servers(&self) {
        let timeout = std::time::Duration::from_secs(self.config.lifecycle.idle_timeout_secs);
        if timeout.is_zero() {
            return;
        }
        for language in self.clients.languages() {
            if self.idle_for(&language).is_some_and(|idle| idle >= timeout) {
                info!(
                    "Stopping the {language} language server: unused for {}s",
                    timeout.as_secs()
                );
                self.stop_language(&language, StopReason::Idle).await;
            }
        }
    }

    /// Stop `language_id`'s server, if one runs: shut it down gracefully in the
    /// background, unregister its client, forget its folders and documents,
    /// and record `reason`. Returns whether a server was running.
    async fn stop_language(&self, language_id: &str, reason: StopReason) -> bool {
        let removed = self.servers.write().await.remove(language_id);
        let _ = self.clients.remove(language_id);
        self.server_folders.write().await.remove(language_id);
        self.forget_documents(language_id).await;
        let Some(server) = removed else {
            return false;
        };
        self.stop_reasons
            .lock()
            .insert(language_id.to_string(), reason);
        let language_id = language_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = server.shutdown().await {
                warn!("LSP server '{language_id}' shutdown error: {e}");
            }
        });
        true
    }

    /// Before `starting` (languages whose servers are about to start) start,
    /// stop the least recently used other servers so that no more than
    /// `max_running_servers` run (0: no cap).
    async fn make_room(&self, starting: &[String]) {
        let cap = self.config.lifecycle.max_running_servers;
        if cap == 0 {
            return;
        }
        let running = self.clients.languages();
        let new = starting
            .iter()
            .filter(|language| !running.contains(language))
            .count();
        let excess = (running.len() + new).saturating_sub(cap);
        if excess == 0 {
            return;
        }
        // The longest unused first; servers in use last.
        let mut others: Vec<(std::cmp::Reverse<Option<std::time::Duration>>, String)> = running
            .into_iter()
            .filter(|language| !starting.contains(language))
            .map(|language| (std::cmp::Reverse(self.idle_for(&language)), language))
            .collect();
        others.sort();
        for (_, language) in others.into_iter().take(excess) {
            info!("Stopping the {language} language server: at most {cap} may run");
            self.stop_language(&language, StopReason::Cap).await;
        }
    }

    /// When the applicable servers start ([`StartupMode`]), from the
    /// configuration's `[lifecycle]` table.
    #[must_use]
    pub fn startup_mode(&self) -> StartupMode {
        self.config.lifecycle.startup
    }

    /// Those of `languages` that [`Self::spawn_applicable`] would start: each
    /// has a configured (installed) server, none of them running, that
    /// applies to a workspace root. Sorted, without duplicates.
    #[must_use]
    pub fn startable_languages(&self, languages: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut startable: Vec<String> = languages
            .into_iter()
            .filter(|language| self.clients.get(language).is_none())
            .filter(|language| {
                self.server_config(language).is_some_and(|config| {
                    self.workspace_roots
                        .iter()
                        .any(|root| self.applies_to(&config, root))
                })
            })
            .collect();
        startable.sort();
        startable.dedup();
        startable
    }

    /// How many automatic restarts a server gets within [`Self::RESTART_WINDOW`]
    /// before it is left down until `restart_server` is called.
    const MAX_RESTARTS: usize = 3;
    /// The window [`Self::MAX_RESTARTS`] counts within.
    const RESTART_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

    /// Restart `language_id`'s server if its process has exited.
    ///
    /// The next request after a crash restarts the server, with the folders it
    /// ran with, rather than failing with "Command channel closed" until someone
    /// calls `restart_server`: up to [`Self::MAX_RESTARTS`] times in
    /// [`Self::RESTART_WINDOW`]; past that it stays down, so a server that dies
    /// on every start is not restarted in a loop.
    async fn restart_if_exited(&self, language_id: &str) -> Result<()> {
        if !self.clients.get(language_id).is_some_and(|c| c.is_closed()) {
            return Ok(());
        }
        let _guard = self.lazy_spawn_lock.lock().await;
        // Another caller may have restarted it while we waited.
        if !self.clients.get(language_id).is_some_and(|c| c.is_closed()) {
            return Ok(());
        }
        let exited = format!("the {language_id} language server exited");
        {
            let mut restarts = self.restarts.lock();
            let recent = restarts.entry(language_id.to_string()).or_default();
            recent.retain(|at| at.elapsed() < Self::RESTART_WINDOW);
            if recent.len() >= Self::MAX_RESTARTS {
                return Err(LspError::ServerUnavailable(format!(
                    "{exited} {} times in {} minutes and is left down; \
                     call restart_server to start it again",
                    recent.len() + 1,
                    Self::RESTART_WINDOW.as_secs() / 60
                )));
            }
            recent.push(std::time::Instant::now());
        }
        self.start_errors
            .write()
            .await
            .insert(language_id.to_string(), exited.clone());
        let config = self
            .config
            .servers
            .iter()
            .find(|s| s.language_id == language_id)
            .cloned()
            .ok_or_else(|| LspError::NoServerForLanguage(language_id.to_string()))?;
        let folders = self
            .server_folders
            .read()
            .await
            .get(language_id)
            .cloned()
            .unwrap_or_default();
        if folders.is_empty() {
            return Ok(());
        }
        warn!("{exited}; restarting it for {}", show_folders(&folders));
        self.register_server(&config, folders).await.map(|_| ())
    }

    /// Set workspace roots.
    pub fn set_workspace_roots(&mut self, roots: Vec<PathBuf>) {
        self.workspace_roots = roots;
    }

    /// Start the servers that apply to the workspace roots (their project
    /// markers are found within `heuristics_max_depth`), only those of
    /// `languages` when given, skipping any whose server is running. A
    /// failure to start is kept for `get_server_status` (`last_error`).
    ///
    /// Eager startup calls this once, for every language, before the MCP
    /// server answers; lazy startup calls it when a workspace-wide request
    /// finds no server, and in the background after `lsp_arch_detect`.
    ///
    /// # Errors
    ///
    /// Returns an error if servers applied and none of them could be started.
    pub async fn spawn_applicable(&self, languages: Option<&[String]>) -> Result<()> {
        let _guard = self.lazy_spawn_lock.lock().await;

        // Filter configs that should be spawned
        let applicable_configs: Vec<_> = self
            .config
            .servers
            .iter()
            .filter(|server_config| {
                languages.is_none_or(|languages| languages.contains(&server_config.language_id))
            })
            .filter(|server_config| self.clients.get(&server_config.language_id).is_none())
            .filter(|server_config| {
                self.workspace_roots
                    .iter()
                    .any(|root| self.applies_to(server_config, root))
            })
            .cloned()
            .collect();

        if applicable_configs.is_empty() {
            info!("No applicable LSP servers to start for this workspace");
            return Ok(());
        }

        let mut pairs: Vec<(LspServerConfig, Vec<PathBuf>)> = applicable_configs
            .into_iter()
            .filter_map(|config| {
                let folders = self.startup_folders(&config);
                (!folders.is_empty()).then_some((config, folders))
            })
            .collect();
        let cap = self.config.lifecycle.max_running_servers;
        if cap > 0 && pairs.len() > cap {
            warn!(
                "{} servers apply, but at most {cap} may run; starting the first {cap}",
                pairs.len()
            );
            pairs.truncate(cap);
        }
        let starting: Vec<String> = pairs
            .iter()
            .map(|(config, _)| config.language_id.clone())
            .collect();
        self.make_room(&starting).await;

        info!(
            "Attempting to spawn {} applicable LSP server(s)...",
            pairs.len()
        );

        // Spawn all servers
        let result = spawn_servers_in(&pairs).await;
        {
            let mut start_errors = self.start_errors.write().await;
            for failure in &result.failures {
                start_errors.insert(failure.language_id.clone(), failure.error.clone());
            }
        }

        // Handle results
        if result.all_failed() {
            return Err(LspError::AllServersFailedToInit {
                count: result.failure_count(),
                failures: result.failures,
            });
        }

        if result.partial_success() {
            warn!(
                "Partial server initialization: {} succeeded, {} failed",
                result.server_count(),
                result.failure_count()
            );
            for failure in &result.failures {
                error!("Server initialization failed: {}", failure);
            }
        }

        // Store both the client handle and the LspServer that owns the child
        // process: dropped at the end of this loop iteration, the LspServer would
        // kill the process it just started.
        let mut servers = self.servers.write().await;
        let mut server_folders = self.server_folders.write().await;
        for (language_id, server) in result.servers {
            self.stop_reasons.lock().remove(&language_id);
            self.clients
                .insert(language_id.clone(), server.client().clone());
            if let Some((_, folders)) = pairs
                .iter()
                .find(|(config, _)| config.language_id == language_id)
            {
                server_folders.insert(language_id.clone(), folders.clone());
            }
            servers.insert(language_id, server);
        }
        drop(server_folders);
        drop(servers);

        info!("Proceeding with {} LSP server(s)", self.clients.len());

        Ok(())
    }

    /// In lazy mode, start the applicable servers a request that is not about
    /// one file needs: the server of `language_id` if it is not running, or,
    /// when `language_id` is empty, every applicable server if none is
    /// running. Eager mode started them before the first request.
    async fn start_for_request(&self, language_id: &str) -> Result<()> {
        if self.startup_mode() != StartupMode::Lazy {
            return Ok(());
        }
        if language_id.is_empty() {
            if self.clients.is_empty() {
                self.spawn_applicable(None).await?;
            }
        } else if self.clients.get(language_id).is_none() {
            self.spawn_applicable(Some(&[language_id.to_string()]))
                .await?;
        }
        Ok(())
    }

    /// Gracefully shut down every spawned LSP server.
    ///
    /// Sends the LSP `shutdown`/`exit` handshake and waits up to 5 seconds for
    /// each server to exit; a server still running then is killed when its
    /// `LspServer` is dropped (see its `Drop`).
    pub async fn shutdown(&self) {
        let servers: Vec<(String, LspServer)> = {
            let mut guard = self.servers.write().await;
            std::mem::take(&mut *guard).into_iter().collect()
        };
        self.clients.clear();
        self.server_folders.write().await.clear();
        self.open_documents.write().await.clear();
        for (language_id, server) in servers {
            if let Err(e) = server.shutdown().await {
                warn!("LSP server '{language_id}' shutdown error: {e}");
            }
        }
    }

    /// Resolve the language a server-scoped tool should act on.
    ///
    /// Tools like `get_server_capabilities` and `get_project_symbols` are not tied
    /// to a file, so they cannot infer a language the way `get_hover` does. Their
    /// schemas advertise `language_id` as optional; when it is omitted and exactly
    /// one server is running, that server is the only sensible target. With several
    /// running the choice is ambiguous, and the error names them so the agent can
    /// retry with an explicit value instead of guessing.
    fn resolve_language(&self, requested: &str) -> Result<String> {
        if !requested.is_empty() {
            return Ok(requested.to_string());
        }

        let mut running = self.clients.languages();
        match running.len() {
            0 => Err(LspError::ServerUnavailable(
                "no language server is running; call an LSP tool on a file first, \
                 or pass `language_id`"
                    .to_string(),
            )),
            1 => Ok(running.remove(0)),
            _ => {
                running.sort();
                Err(LspError::ServerUnavailable(format!(
                    "several servers are running ({}); pass `language_id` to choose one",
                    running.join(", ")
                )))
            }
        }
    }

    /// Register a client for a language.
    pub fn register_client(&mut self, language_id: String, client: LspClient) {
        self.clients.insert(language_id, client);
    }

    /// Maximum number of ancestor directories searched for a project marker when
    /// resolving the workspace root of a file. Deep enough for any real layout,
    /// bounded so a pathological path cannot walk to `/`.
    const MAX_ROOT_SEARCH_DEPTH: usize = 64;

    /// How long a cold-start `documentSymbol` probe keeps retrying before it
    /// gives up and lets the request proceed anyway. Bounded well under the
    /// per-request timeout so a symbol-less file cannot stall a call.
    const COLD_START_LOAD_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

    /// Delay between cold-start load probes.
    const COLD_START_LOAD_INTERVAL: std::time::Duration = std::time::Duration::from_millis(600);

    /// The workspace root that governs `file_path` for `config`.
    ///
    /// Walks upward from the file looking for the server's project markers
    /// (`Cargo.toml`, `go.mod`, `package.json`, …) and returns the **outermost**
    /// match, never ascending past the enclosing VCS repository.
    ///
    /// Outermost, not nearest: `crates/lsp/Cargo.toml` is a Cargo *workspace
    /// member* that inherits `version.workspace = true` from the repository root.
    /// Rooting `rust-analyzer` there makes it reject the very file that selected
    /// it (`file not found`). The same holds for Yarn/pnpm workspaces and Go
    /// modules with nested `go.mod` files. Stopping at the `.git` directory keeps
    /// a stray marker in `$HOME` from swallowing the whole tree.
    ///
    /// A server with no heuristics matches anywhere, and is rooted at the file's
    /// own directory.
    ///
    /// When no project marker is found, most servers are still rooted at the
    /// file's own directory: pyright, gopls, clangd, typescript, … analyse a
    /// loose script or scratch file fine, and refusing to serve it would make the
    /// whole language silently unavailable for marker-less trees (a bare `.py`
    /// directory would report "no project root"). The exception is
    /// [`Self::STRICT_ROOT_LANGUAGES`].
    fn workspace_root_for(config: &LspServerConfig, file_path: &Path) -> Option<PathBuf> {
        let start = if file_path.is_dir() {
            file_path
        } else {
            file_path.parent()?
        };

        let Some(heuristics) = &config.heuristics else {
            return Some(start.to_path_buf());
        };

        // Ancestors, nearest first, truncated *after* the VCS root.
        let mut candidates: Vec<&Path> = Vec::new();
        for dir in start.ancestors().take(Self::MAX_ROOT_SEARCH_DEPTH) {
            candidates.push(dir);
            if dir.join(".git").exists() {
                break;
            }
        }

        // Outermost applicable ancestor wins.
        candidates
            .into_iter()
            .rev()
            .find(|dir| heuristics.is_applicable(dir))
            .map(Path::to_path_buf)
            .or_else(|| {
                // No marker anywhere. Build-graph servers stay unavailable so
                // the agent gets a clear "missing manifest" instead of a spawned
                // but useless server; everyone else falls back to the file's dir.
                (!Self::STRICT_ROOT_LANGUAGES.contains(&config.language_id.as_str()))
                    .then(|| start.to_path_buf())
            })
    }

    /// Languages whose servers cannot resolve anything without a project
    /// manifest. rust-analyzer (Cargo), csharp-ls (`.csproj`/`.sln`),
    /// fsautocomplete and metals all drive their analysis off the build graph
    /// and either error or return empty when rooted at an arbitrary directory,
    /// so a missing marker means "no server" rather than a single-file fallback.
    /// graphql-language-service loads its schema from a GraphQL config; started
    /// without one, it took 26 s to return no symbols.
    const STRICT_ROOT_LANGUAGES: &'static [&'static str] =
        &["rust", "csharp", "fsharp", "scala", "graphql"];

    /// Languages whose server imports a project only at the root of one of
    /// its workspace folders, and analyses any other file on its own:
    /// kotlin-lsp, given a monorepo's root, finds no Gradle build there and
    /// returns no reference outside the file asked about; sourcekit-lsp
    /// answers every request for a file of a package below the root with
    /// `No language service`. They get the projects inside a workspace root
    /// for their folders, as the [`Self::STRICT_ROOT_LANGUAGES`] do, but still
    /// serve a file of no project. Only a project that holds a source file of
    /// theirs is one: kotlin-lsp, given every Gradle and Maven build, would
    /// import the Java ones too.
    const PROJECT_FOLDER_LANGUAGES: &'static [&'static str] = &["kotlin", "swift"];

    /// Whether `language_id`'s server knows only the projects among its
    /// folders: one that needs its project manifest at its root
    /// ([`Self::STRICT_ROOT_LANGUAGES`]), and clangd, which reads the
    /// compilation database Devscriptor writes for its folders
    /// ([`crate::lsp::compile_commands`]) and, for a file outside them, a
    /// command guessed from the nearest file it lists.
    fn knows_only_its_folders(language_id: &str) -> bool {
        Self::STRICT_ROOT_LANGUAGES.contains(&language_id)
            || Self::PROJECT_FOLDER_LANGUAGES.contains(&language_id)
            || language_id == "cpp"
    }

    /// Start a server for `language_id` rooted at `file_path`'s own workspace.
    ///
    /// Returns the running client. Concurrent callers for the same language are
    /// serialized, and the registry is re-checked after acquiring the lock, so a
    /// burst of requests spawns exactly one server.
    async fn start_server_for_file(
        &self,
        language_id: &str,
        file_path: &Path,
    ) -> Result<LspClient> {
        let _guard = self.lazy_spawn_lock.lock().await;

        // Another caller may have started it while we waited.
        if let Some(client) = self.clients.get(language_id) {
            return Ok(client);
        }

        let config = self
            .config
            .servers
            .iter()
            .find(|s| s.language_id == language_id)
            .cloned()
            .ok_or_else(|| LspError::NoServerForLanguage(language_id.to_string()))?;

        let root = Self::workspace_root_for(&config, file_path).ok_or_else(|| {
            let markers = config
                .heuristics
                .as_ref()
                .map(|h| h.markers.join(", "))
                .unwrap_or_default();
            let why = if language_id == "graphql" {
                ": graphql-language-service needs a GraphQL config at the project root"
            } else {
                ""
            };
            LspError::ServerUnavailable(format!(
                "the {language_id} language server has no project root above {} \
                 (looked for: {markers}){why}",
                file_path.display()
            ))
        })?;

        let folders = self.folders_for_file(&config, canonical(&root));
        info!(
            "Lazily starting {language_id} server for {} (workspace folders: {})",
            file_path.display(),
            show_folders(&folders)
        );

        // The root is already resolved by `workspace_root_for` (which applies
        // the single-file fallback and filters strict build-graph languages),
        // so spawn this one server directly. Going through `spawn_servers` here
        // would re-apply its project-marker gate and reject a deliberately
        // marker-less root — e.g. a loose Python script — with "Failed to spawn
        // for any workspace root", defeating the fallback.
        let client = self.register_server(&config, folders.clone()).await?;
        info!("{language_id} server ready for {}", show_folders(&folders));

        Ok(client)
    }

    /// The folders `config`'s server starts with for a request about a file of
    /// the project at `root`. A server that needs its project manifest at its
    /// root ([`Self::STRICT_ROOT_LANGUAGES`]) gets that project, and takes in
    /// others as they are asked about ([`Self::cover_project`]). Another gets,
    /// for a file in the workspace, the folders it starts with for the
    /// workspace ([`Self::startup_folders`]), so that how a server was started
    /// does not change what it knows: clangd, started again after its idle
    /// timeout for a file under `src/`, compiled it without its project's
    /// `include/`.
    fn folders_for_file(&self, config: &LspServerConfig, root: PathBuf) -> Vec<PathBuf> {
        let in_workspace = self
            .workspace_roots
            .iter()
            .any(|workspace| root.starts_with(canonical(workspace)));
        if in_workspace && !Self::STRICT_ROOT_LANGUAGES.contains(&config.language_id.as_str()) {
            return self.startup_folders(config);
        }
        vec![root]
    }

    /// Whether `config`'s server applies to the workspace root `root`: its
    /// project markers are there (for a server without markers, files of its
    /// language), or files of its language are — as a request for one of them
    /// starts it ([`Self::start_server_for_file`]) — unless it needs its project
    /// manifest at its root ([`Self::STRICT_ROOT_LANGUAGES`]). A Python
    /// project with no `pyproject.toml` was detected by `lsp_arch_detect`, and
    /// its server not started until the first request waited for it.
    fn applies_to(&self, config: &LspServerConfig, root: &Path) -> bool {
        let max_depth = Some(self.config.workspace.heuristics_max_depth);
        config.should_spawn(root, max_depth)
            || (!Self::STRICT_ROOT_LANGUAGES.contains(&config.language_id.as_str())
                && config.has_files_within(root, max_depth))
    }

    /// The path a tool names a project by, taken from the workspace root
    /// when relative, made absolute ([`canonical`]).
    fn in_workspace(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        canonical(&match self.workspace_roots.first() {
            Some(root) if path.is_relative() => canonical(root).join(path),
            _ => path.to_path_buf(),
        })
    }

    /// The configuration of `language_id`'s server.
    fn server_config(&self, language_id: &str) -> Option<LspServerConfig> {
        self.config
            .servers
            .iter()
            .find(|s| s.language_id == language_id)
            .cloned()
    }

    /// The workspace folders `config`'s server starts with when no file asks
    /// for it: the workspace roots — except that a server needing a project
    /// manifest at its root ([`Self::STRICT_ROOT_LANGUAGES`]) gets, for a root
    /// that is no such project, the projects inside it. Rooted at the root of
    /// a monorepo instead, rust-analyzer finds no `Cargo.toml` there and
    /// answers every request with `file not found`. A server that imports a
    /// project only at a folder's root ([`Self::PROJECT_FOLDER_LANGUAGES`])
    /// gets the projects inside it too, or the root where it holds none.
    fn startup_folders(&self, config: &LspServerConfig) -> Vec<PathBuf> {
        let language_id = config.language_id.as_str();
        let strict = Self::STRICT_ROOT_LANGUAGES.contains(&language_id);
        let projects = Self::PROJECT_FOLDER_LANGUAGES.contains(&language_id);
        let depth = self.config.workspace.heuristics_max_depth;
        let mut folders: Vec<PathBuf> = Vec::new();
        for root in &self.workspace_roots {
            let root = canonical(root);
            let found = match &config.heuristics {
                Some(heuristics) if strict => heuristics.project_dirs(&root, depth),
                Some(heuristics) if projects => {
                    let dirs: Vec<PathBuf> = heuristics
                        .project_dirs(&root, depth)
                        .into_iter()
                        .filter(|dir| holds_source_of(dir, config))
                        .collect();
                    if dirs.is_empty() { vec![root] } else { dirs }
                }
                _ => vec![root],
            };
            for folder in found {
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
            }
        }
        folders
    }

    /// Start `config`'s server for `folders` and register it, in place of any
    /// server running for its language, which is shut down.
    async fn register_server(
        &self,
        config: &LspServerConfig,
        folders: Vec<PathBuf>,
    ) -> Result<LspClient> {
        let language_id = config.language_id.as_str();
        self.make_room(&[language_id.to_string()]).await;
        let server = match LspServer::spawn_in(config, &folders).await {
            Ok(server) => {
                self.start_errors.write().await.remove(language_id);
                server
            }
            Err(e) => {
                // In the log too, not only in the caller's error: a server started on
                // demand would otherwise fail out of sight.
                warn!(
                    "The {language_id} language server failed to start for {}: {e}",
                    show_folders(&folders)
                );
                self.start_errors
                    .write()
                    .await
                    .insert(language_id.to_string(), e.to_string());
                return Err(LspError::ServerUnavailable(format!(
                    "the {language_id} language server failed to start for {}: {e}",
                    show_folders(&folders)
                )));
            }
        };
        let client = server.client().clone();
        self.stop_reasons.lock().remove(language_id);
        let replaced = self
            .servers
            .write()
            .await
            .insert(language_id.to_string(), server);
        self.clients.insert(language_id.to_string(), client.clone());
        self.server_folders
            .write()
            .await
            .insert(language_id.to_string(), folders);
        // The server now running was sent none of the documents.
        self.forget_documents(language_id).await;
        if let Some(replaced) = replaced {
            let language_id = language_id.to_string();
            tokio::spawn(async move {
                if let Err(e) = replaced.shutdown().await {
                    warn!("LSP server '{language_id}' shutdown error: {e}");
                }
            });
        }

        // A freshly spawned server answers semantic requests with *empty results*
        // until it has indexed the workspace. Wait it out, or the agent's first
        // `get_hover` silently returns nothing.
        client.wait_until_ready().await;
        Ok(client)
    }

    /// Forget which documents were sent to `language_id`'s server, so that the
    /// next request for each sends it to the server running then. Remembered
    /// across a restart, they would never reach the new server, which would fail
    /// every request for them.
    async fn forget_documents(&self, language_id: &str) {
        self.open_documents
            .write()
            .await
            .retain(|path, _| self.get_language_id(Path::new(path)) != language_id);
    }

    /// Make the running `language_id` server cover `file_path`'s project.
    ///
    /// A server that knows only the projects among its workspace folders
    /// ([`Self::knows_only_its_folders`]) is restarted with the file's project
    /// added to them: rust-analyzer started for one crate of a monorepo
    /// answered `file not found` for every file of another. Returns the new
    /// client, or `None` when the running server covers the file already or is
    /// of another kind.
    async fn cover_project(
        &self,
        language_id: &str,
        file_path: &Path,
    ) -> Result<Option<LspClient>> {
        if !Self::knows_only_its_folders(language_id) {
            return Ok(None);
        }
        let Some(config) = self.server_config(language_id) else {
            return Ok(None);
        };
        let Some(project) = Self::workspace_root_for(&config, file_path).map(|p| canonical(&p))
        else {
            return Ok(None);
        };
        let covers = |folders: &[PathBuf]| folders.iter().any(|f| project.starts_with(f));
        let folders = self.server_folders.read().await.get(language_id).cloned();
        if folders.as_deref().is_none_or(covers) {
            return Ok(None);
        }

        let _guard = self.lazy_spawn_lock.lock().await;
        // Another caller may have added it while we waited.
        let mut folders = self
            .server_folders
            .read()
            .await
            .get(language_id)
            .cloned()
            .unwrap_or_default();
        if covers(&folders) {
            return Ok(self.clients.get(language_id));
        }
        info!(
            "Restarting {language_id} server to add {} to {}",
            project.display(),
            show_folders(&folders)
        );
        folders.push(project);
        self.register_server(&config, folders).await.map(Some)
    }

    /// How many files [`Self::open_beside`] opens at most.
    const MAX_OPENED_BESIDE: usize = 64;

    /// The folders of the server that answers for `file`, started for it if
    /// none runs: where the files it can be asked about are.
    ///
    /// # Errors
    ///
    /// Returns an error if no server can answer for `file`.
    pub async fn folders_serving(&self, file: &Path) -> Result<Vec<PathBuf>> {
        self.get_client_for_file(file).await?;
        let language = self.get_language_id(file);
        Ok(self
            .server_folders
            .read()
            .await
            .get(&language)
            .cloned()
            .unwrap_or_default())
    }

    /// Open `files` in the server that answers for `file`, as a request about
    /// each would — those of its language, at most [`Self::MAX_OPENED_BESIDE`]
    /// — so that a server that looks for references only in the projects of
    /// open files finds theirs: typescript-language-server, with no
    /// `jsconfig.json`, gives each open file a project of its own and of what
    /// it imports, and found none of a test file's calls until the test file
    /// was open.
    ///
    /// # Errors
    ///
    /// Returns an error if no server can answer for `file`; a file that
    /// cannot be opened is passed over.
    pub async fn open_beside(&self, file: &Path, files: &[PathBuf]) -> Result<()> {
        let client = self.get_client_for_file(file).await?;
        let language = self.get_language_id(file);
        let mut sent = false;
        for other in files
            .iter()
            .filter(|other| other.as_path() != file && self.get_language_id(other) == language)
            .take(Self::MAX_OPENED_BESIDE)
        {
            match self.ensure_document_open(&client, other).await {
                Ok(opened) => sent |= opened,
                Err(e) => debug!("Not opening {}: {e}", other.display()),
            }
        }
        if sent {
            client.wait_until_ready().await;
        }
        Ok(())
    }

    /// Open `project`'s files of `language_id` in its server, as
    /// [`Self::open_beside`] opens a symbol's users: gopls loads a module,
    /// and typescript-language-server a project, only once a file of it is
    /// open, and before that searched none of its symbols.
    async fn open_project(&self, language_id: &str, project: &Path) -> Result<()> {
        let files: Vec<PathBuf> = ignore::WalkBuilder::new(project)
            .build()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(ignore::DirEntry::into_path)
            .filter(|path| self.get_language_id(path) == language_id)
            .take(Self::MAX_OPENED_BESIDE + 1)
            .collect();
        match files.split_first() {
            Some((first, rest)) => self.open_beside(first, rest).await,
            None => Ok(()),
        }
    }

    /// Get the LSP client that should handle `file_path`.
    ///
    /// The server set is *not* fixed at startup. [`Self::spawn_applicable`]
    /// only starts servers for the workspace roots the process was launched
    /// in (at startup when eager, else on demand); a request for a file
    /// anywhere lazily starts the right server, rooted at that file's own
    /// project. So the LSP tools work on any file, whatever directory the MCP
    /// server was started in.
    async fn get_client_for_file(&self, file_path: &Path) -> Result<LspClient> {
        Ok(self.open_file(file_path).await?.0)
    }

    /// [`Self::get_client_for_file`], and whether `file_path` was just sent to
    /// the server — opened, or refreshed after changing on disk — so that the
    /// server is analysing it anew.
    async fn open_file(&self, file_path: &Path) -> Result<(LspClient, bool)> {
        let extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| {
                LspError::InvalidPath(format!(
                    "Cannot determine file extension: {}",
                    file_path.display()
                ))
            })?;

        // `.m` is MATLAB's as well as Objective-C's: clangd gets only the
        // Objective-C ones.
        if extension == "m" && !crate::languages::is_objective_c(file_path) {
            return Err(LspError::NoServerForFile(format!(
                "{} is not Objective-C (MATLAB uses .m too), and no language server here reads \
                 MATLAB",
                file_path.display()
            )));
        }
        let language_id = self
            .extension_map
            .get(extension)
            .ok_or_else(|| no_server_for(extension))?;
        self.restart_if_exited(language_id).await?;

        let (client, freshly_spawned) = match self.clients.get(language_id) {
            Some(client) => match self.cover_project(language_id, file_path).await? {
                Some(client) => (client, true),
                None => (client, false),
            },
            None => (
                self.start_server_for_file(language_id, file_path).await?,
                true,
            ),
        };

        // In use until the document is open and loaded; the request that
        // follows holds the client in use itself.
        let _in_use = client.in_use();

        // Opening or refreshing a document makes the server (re)analyse it. Let
        // that settle, otherwise a semantic request races the analysis and comes
        // back empty — indistinguishable, to an agent, from "no such symbol".
        let refreshed = self.ensure_document_open(&client, file_path).await?;
        if refreshed {
            client.wait_until_ready().await;
        }

        // `$/progress` alone is not a reliable "the file is loaded" signal: a
        // server reports progress in phases (rust-analyzer: metadata → cache
        // priming → indexing) with brief idle gaps between them, and
        // `wait_until_ready` returns on the first gap — before the file is mapped
        // to its crate. The first `documentSymbol` then comes back empty even
        // though the code is fine. Only on a cold spawn, probe until the server
        // actually knows the file (or a bounded budget elapses).
        if freshly_spawned {
            self.await_file_loaded(&client, file_path).await;
        }
        Ok((client, refreshed))
    }

    /// Poll `documentSymbol` for a freshly spawned server until it returns
    /// symbols (the file is loaded) or [`Self::COLD_START_LOAD_BUDGET`] elapses.
    ///
    /// The common case — a non-empty first probe — returns immediately and adds
    /// no latency. A genuinely symbol-less file (empty/config) simply waits out
    /// the budget once, which is bounded well under the per-request timeout.
    async fn await_file_loaded(&self, client: &LspClient, file_path: &Path) {
        let Ok(uri) = path_to_uri(file_path) else {
            return;
        };
        // The probe uses `documentSymbol` as a "file is loaded" proxy. A server
        // that doesn't implement it would never answer, burning the whole budget,
        // so skip the probe for those (`wait_until_ready` already ran).
        if !self
            .server_supports(&self.get_language_id(file_path), "documentSymbolProvider")
            .await
        {
            return;
        }
        let deadline = std::time::Instant::now() + Self::COLD_START_LOAD_BUDGET;
        loop {
            let params = lsp_types::DocumentSymbolParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                partial_result_params: lsp_types::PartialResultParams::default(),
            };
            let resp: Result<Option<lsp_types::DocumentSymbolResponse>> =
                client.request("textDocument/documentSymbol", params).await;
            let loaded = matches!(
                &resp,
                Ok(Some(lsp_types::DocumentSymbolResponse::Flat(s))) if !s.is_empty()
            ) || matches!(
                &resp,
                Ok(Some(lsp_types::DocumentSymbolResponse::Nested(s))) if !s.is_empty()
            );
            if loaded || std::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Self::COLD_START_LOAD_INTERVAL).await;
            client.wait_until_ready().await;
        }
    }

    /// Announce `file_path` to `client`, or refresh it if the file changed on disk.
    ///
    /// Servers answer position requests only for documents they know about, so
    /// this runs before every request rather than only before diagnostics. The
    /// on-disk mtime is tracked so an agent that edits a file and immediately asks
    /// for hover or diagnostics sees its own change, not the version cached when
    /// the document was first opened.
    /// Returns `true` if a `didOpen` or `didChange` was sent (i.e. the server
    /// now has fresh work to do), `false` if the document was already current.
    async fn ensure_document_open(&self, client: &LspClient, file_path: &Path) -> Result<bool> {
        let uri = path_to_uri(file_path)?;
        let key = uri_to_path(&uri);

        let mtime = tokio::fs::metadata(file_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());

        let tick = self
            .ticks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Fast path: already open and unchanged.
        if let Some(doc) = self.open_documents.write().await.get_mut(&key)
            && doc.mtime == mtime
        {
            doc.last_used = tick;
            return Ok(false);
        }

        let text = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| LspError::Io(format!("Failed to read {}: {e}", file_path.display())))?;
        let language_id = document_language_id(file_path, &self.get_language_id(file_path));

        let mut open = self.open_documents.write().await;
        // Re-check: a concurrent caller may have opened it while we read the file.
        match open.get(&key) {
            Some(doc) if doc.mtime == mtime => return Ok(false),
            Some(doc) => {
                let version = doc.version + 1;
                let change = lsp_types::DidChangeTextDocumentParams {
                    text_document: lsp_types::VersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version,
                    },
                    content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text,
                    }],
                };
                client
                    .notify("textDocument/didChange", change)
                    .await
                    .map_err(|e| LspError::Io(format!("didChange failed: {e}")))?;
                open.insert(
                    key,
                    OpenDocument {
                        version,
                        mtime,
                        last_used: tick,
                    },
                );
            }
            None => {
                let params = lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem {
                        uri: uri.clone(),
                        language_id,
                        version: 1,
                        text,
                    },
                };
                client
                    .notify("textDocument/didOpen", params)
                    .await
                    .map_err(|e| LspError::Io(format!("didOpen failed: {e}")))?;
                let language_id = self.get_language_id(file_path);
                open.insert(
                    key.clone(),
                    OpenDocument {
                        version: 1,
                        mtime,
                        last_used: tick,
                    },
                );
                let stale = documents_to_close(
                    &open,
                    |path| self.get_language_id(Path::new(path)),
                    &language_id,
                    &key,
                    MAX_OPEN_DOCUMENTS,
                );
                for path in stale {
                    open.remove(&path);
                    if let Ok(uri) = path_to_uri(Path::new(&path)) {
                        let params = lsp_types::DidCloseTextDocumentParams {
                            text_document: lsp_types::TextDocumentIdentifier { uri },
                        };
                        if let Err(e) = client.notify("textDocument/didClose", params).await {
                            warn!("didClose for {path} failed: {e}");
                        }
                    }
                }
            }
        }

        Ok(true)
    }

    /// Get language ID for a file path.
    fn get_language_id(&self, path: &Path) -> String {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.extension_map.get(ext).cloned())
            .unwrap_or_else(|| "plaintext".to_string())
    }

    /// Whether the running server for `language_id` advertises an LSP capability
    /// (e.g. `documentSymbolProvider`). A capability that is absent, `null`, or
    /// `false` counts as unsupported; per the LSP spec a supported one is `true`
    /// or an options object. Returns `false` if no such server is running.
    async fn server_supports(&self, language_id: &str, provider: &str) -> bool {
        let servers = self.servers.read().await;
        servers
            .get(language_id)
            .is_some_and(|s| capability_supported(s.capabilities(), provider))
    }

    /// `client.request(method, params)` for a request about `file`. Where the
    /// server declares no capability for `method` ([`declaring_capability`])
    /// and the request fails, it fails as [`LspError::Unsupported`]: a server
    /// answers what it does not offer with an error of its own, an opaque
    /// `-32601 Unhandled method` or worse (jdtls: `-32603 Internal error`). A
    /// server that answers all the same is taken at its word: clangd answers
    /// type hierarchies it does not declare.
    async fn request_feature<P, R>(
        &self,
        client: &LspClient,
        file: &Path,
        method: &str,
        params: P,
    ) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        match client.request(method, params).await {
            Ok(response) => Ok(response),
            Err(error) => Err(self.unsupported_unless_declared(file, method, error).await),
        }
    }

    /// `error`, a failure of `method` on the server of `file`, or
    /// [`LspError::Unsupported`] where the server declares no capability for
    /// `method`. A broken connection is not a missing feature, and stays.
    async fn unsupported_unless_declared(
        &self,
        file: &Path,
        method: &str,
        error: LspError,
    ) -> LspError {
        let Some((provider, feature)) = declaring_capability(method) else {
            return error;
        };
        if matches!(
            error,
            LspError::ServerClosed | LspError::Transport(_) | LspError::Shutdown
        ) {
            return error;
        }
        let language_id = self.get_language_id(file);
        let servers = self.servers.read().await;
        match servers.get(&language_id) {
            Some(server) if !capability_supported(server.capabilities(), provider) => {
                LspError::Unsupported {
                    language_id,
                    feature: feature.to_string(),
                }
            }
            _ => error,
        }
    }

    /// Turn a request the server cannot handle into a clear up-front error
    /// instead of an opaque `-32601 Unhandled method` from the wire. Best-effort:
    /// if the server is not (yet) known, the request is allowed to proceed.
    async fn ensure_capability(&self, path: &Path, provider: &str, feature: &str) -> Result<()> {
        let language_id = self.get_language_id(path);
        let servers = self.servers.read().await;
        if let Some(server) = servers.get(&language_id)
            && !capability_supported(server.capabilities(), provider)
        {
            return Err(LspError::Unsupported {
                language_id,
                feature: feature.to_string(),
            });
        }
        Ok(())
    }

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
    /// with `textDocument/diagnostic`. One that does not only pushes them —
    /// typescript-language-server, clangd, gopls, jdtls — so the answer is what
    /// it pushes for the file: after opening or refreshing it, the pushes that
    /// follow; otherwise the last one. Asked with `textDocument/diagnostic`,
    /// those servers would fail every call with `-32601 Unhandled method`.
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

        let diagnostics = if self
            .server_supports(&language_id, "diagnosticProvider")
            .await
        {
            match Self::pull_diagnostics(&client, &uri).await {
                Err(LspError::LspServerError { code, .. }) if code == LSP_METHOD_NOT_FOUND => {
                    Self::pushed_diagnostics(&client, &uri, mark, refreshed).await
                }
                pulled => pulled?,
            }
        } else {
            Self::pushed_diagnostics(&client, &uri, mark, refreshed).await
        };

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
            .collect();

        Ok(DiagnosticsResult { diagnostics })
    }

    /// Ask `client` for `uri`'s diagnostics with `textDocument/diagnostic`.
    async fn pull_diagnostics(
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
    /// when the document was just sent to it (`refreshed`), else its last push.
    ///
    /// A document sent for the first time is always pushed for once analysed,
    /// so that wait is long. A change to one already pushed for may push
    /// nothing — typescript-language-server skips a push that would repeat an
    /// empty list — so that wait is short and ends in the last push.
    async fn pushed_diagnostics(
        client: &LspClient,
        uri: &lsp_types::Uri,
        mark: u64,
        refreshed: bool,
    ) -> Vec<lsp_types::Diagnostic> {
        let last = client.pushed_diagnostics(uri.as_str());
        let wait = match (&last, refreshed) {
            (Some(_), false) => return last.unwrap_or_default(),
            (Some(_), true) => PUSH_CHANGE_WAIT,
            (None, _) => PUSH_FIRST_WAIT,
        };
        match client
            .await_pushed_diagnostics(uri.as_str(), mark, wait, PUSH_SETTLE)
            .await
        {
            Some(pushed) => pushed,
            None => last.unwrap_or_default(),
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
            .await?;

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
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: params.range.start.line,
                    character: params.range.start.character,
                },
                end: lsp_types::Position {
                    line: params.range.end.line,
                    character: params.range.end.character,
                },
            },
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
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: params.start_line,
                    character: params.start_character,
                },
                end: lsp_types::Position {
                    line: params.end_line,
                    character: params.end_character,
                },
            },
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
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: params.range.start.line,
                    character: params.range.start.character,
                },
                end: lsp_types::Position {
                    line: params.range.end.line,
                    character: params.range.end.character,
                },
            },
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
            .await?;

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
        let project = self.in_workspace(&params.project);
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

// Helper functions for type conversions

/// The LSP language identifier of the document at `path`, as `didOpen` gives
/// it: the language of the server that handles it, except where one server
/// takes several. typescript-language-server parses a document as the language
/// it is opened as, so a `.tsx` file opened as `typescript` would have its JSX
/// read as syntax errors — "'>' expected" at every tag.
fn document_language_id(path: &Path, server_language: &str) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("ts" | "mts" | "cts") => "typescript",
        _ => server_language,
    }
    .to_string()
}

/// Whether `dir` holds a source file `config`'s server reads, other than a
/// project marker of its own: a Gradle build script is a `.kts` file, a
/// Swift package's manifest a `.swift` one.
fn holds_source_of(dir: &Path, config: &LspServerConfig) -> bool {
    /// Deep enough for `src/main/kotlin/com/example/app/Main.kt`.
    const MAX_DEPTH: usize = 12;
    let markers: &[String] = config
        .heuristics
        .as_ref()
        .map_or(&[], |heuristics| heuristics.markers.as_slice());
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_DEPTH))
        .build()
        .flatten()
        .any(|entry| {
            entry.file_type().is_some_and(|kind| kind.is_file())
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| config.extensions.iter().any(|e| e == extension))
                && !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| markers.iter().any(|marker| marker == name))
        })
}

/// `path` made absolute, symlinks resolved, so that workspace folders and
/// projects compare by where they are rather than by how they are spelled:
/// the workspace root is `.` by default, a file an agent asks about absolute.
/// Compared as spelled, a server started for `./services/api` would not cover
/// `/repo/services/api`, and would be restarted to add it a second time.
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Why no server answers for files of `extension`: the registry has none for
/// it, or has one that does not run here ([`crate::manager::Unavailable`]).
fn no_server_for(extension: &str) -> LspError {
    let registered = crate::generated_registry::SERVER_IDS.iter().find(|id| {
        crate::generated_registry::extensions(id).is_some_and(|exts| exts.contains(&extension))
    });
    match registered {
        Some(id) => LspError::ServerUnavailable(format!(
            "the {} language server ({id}) for .{extension} files {}",
            crate::generated_registry::language_id(id).unwrap_or(id),
            crate::manager::why_unavailable(id).map_or_else(
                || "does not run here".to_string(),
                |unavailable| unavailable.reason
            )
        )),
        None => LspError::NoServerForFile(format!("No LSP server for extension: .{extension}")),
    }
}

/// Workspace folders as a message shows them.
fn show_folders(folders: &[PathBuf]) -> String {
    folders
        .iter()
        .map(|folder| folder.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A `workspace/symbol` answer's symbols, flat or nested; a nested one's
/// symbols without a range (a `WorkspaceLocation`) are left out.
fn workspace_symbols_from_lsp(
    response: Option<lsp_types::WorkspaceSymbolResponse>,
) -> Vec<WorkspaceSymbol> {
    match response {
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
    }
}

/// The capability a server declares for the LSP `method`, and the feature it
/// is named as in an [`LspError::Unsupported`].
fn declaring_capability(method: &str) -> Option<(&'static str, &'static str)> {
    Some(match method {
        "textDocument/hover" => ("hoverProvider", "hover"),
        "textDocument/definition" => ("definitionProvider", "go to definition"),
        "textDocument/references" => ("referencesProvider", "references"),
        "textDocument/completion" | "completionItem/resolve" => {
            ("completionProvider", "completion")
        }
        "textDocument/formatting" => ("documentFormattingProvider", "formatting"),
        "textDocument/rangeFormatting" => ("documentRangeFormattingProvider", "range formatting"),
        "textDocument/codeAction" | "codeAction/resolve" => ("codeActionProvider", "code actions"),
        "textDocument/rename" => ("renameProvider", "rename"),
        "textDocument/signatureHelp" => ("signatureHelpProvider", "signature help"),
        "textDocument/typeDefinition" => ("typeDefinitionProvider", "type definitions"),
        "textDocument/implementation" => ("implementationProvider", "implementations"),
        "textDocument/prepareCallHierarchy"
        | "callHierarchy/incomingCalls"
        | "callHierarchy/outgoingCalls" => ("callHierarchyProvider", "call hierarchy"),
        "textDocument/prepareTypeHierarchy"
        | "typeHierarchy/subtypes"
        | "typeHierarchy/supertypes" => ("typeHierarchyProvider", "type hierarchy"),
        "textDocument/documentHighlight" => ("documentHighlightProvider", "document highlights"),
        "textDocument/documentColor" => ("colorProvider", "document colors"),
        "textDocument/foldingRange" => ("foldingRangeProvider", "folding ranges"),
        "textDocument/selectionRange" => ("selectionRangeProvider", "selection ranges"),
        "textDocument/linkedEditingRange" => {
            ("linkedEditingRangeProvider", "linked editing ranges")
        }
        "textDocument/inlayHint" => ("inlayHintProvider", "inlay hints"),
        "textDocument/documentLink" => ("documentLinkProvider", "document links"),
        "textDocument/semanticTokens/full" => ("semanticTokensProvider", "semantic tokens"),
        _ => return None,
    })
}

/// Whether an `initialize` capabilities object advertises `provider`.
///
/// Per the LSP spec a supported provider is `true` or an options object; it is
/// unsupported when the field is `false`, `null`, or absent. A request is
/// rejected up-front by it, rather than left to fail with an opaque `-32601`.
fn capability_supported(capabilities: &serde_json::Value, provider: &str) -> bool {
    !matches!(
        capabilities.get(provider),
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false))
    )
}

/// Convert a file path to a `file://` URI, percent-encoding each component
/// (RFC 3986) so paths with spaces, `#`, `?` or non-ASCII characters give a
/// valid URI. On Windows the path is written with `/` and after a `/`, as a
/// `file:` URI has it: `C:\a` is `file:///C:/a`, and the UNC path
/// `\\srv\share\a` is `file://srv/share/a`.
///
/// # Errors
///
/// Returns an error if the path cannot be converted to a URI.
pub fn path_to_uri(path: &Path) -> Result<lsp_types::Uri> {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // percent-encode each path component separately so separators stay intact.
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    // Chars that must be encoded in a URI path segment (RFC 3986 §3.3).
    const PATH_SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'[')
        .add(b']')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');

    let path = canonical.to_string_lossy();
    let path = if cfg!(windows) {
        windows_uri_path(&path)
    } else {
        path.into_owned()
    };
    let encoded = utf8_percent_encode(&path, PATH_SEGMENT).to_string();
    let uri_str = format!("file://{encoded}");

    uri_str
        .parse::<lsp_types::Uri>()
        .map_err(|e| LspError::InvalidPath(format!("Failed to create URI: {e}")))
}

/// Convert a URI to a file path string, correctly decoding percent-encoded bytes.
///
/// Each `%XX` is decoded to a byte, and the bytes then as UTF-8, so multi-byte
/// characters survive.
#[must_use]
pub fn uri_to_path(uri: &lsp_types::Uri) -> String {
    let s = uri.as_str();
    match s.strip_prefix("file://") {
        Some(path) if cfg!(windows) => windows_path(&percent_decode_utf8(path)),
        Some(path) => percent_decode_utf8(path),
        // Another scheme names no file: it is passed on as it is.
        None => percent_decode_utf8(s),
    }
}

/// The part of a `file:` URI after `file://` that names the Windows path
/// `path`: `/C:/a` for `C:\a`, and `srv/share/a`, with the server as its host,
/// for the UNC path `\\srv\share\a`.
fn windows_uri_path(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    match slashed.strip_prefix("//") {
        Some(unc) => unc.to_string(),
        None => format!("/{slashed}"),
    }
}

/// The Windows path a decoded `file:` URI path names: `/C:/a`, or `/c:/a` as
/// some servers write it, is `C:\a`, with the drive's letter in capitals so
/// the two spellings compare equal; `srv/share/a`, a URI with a host, is the
/// UNC path `\\srv\share\a`.
pub(crate) fn windows_path(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        let drive = char::from(bytes[1]).to_ascii_uppercase();
        let rest = path[3..].replace('/', "\\");
        if rest.is_empty() {
            format!("{drive}:\\")
        } else {
            format!("{drive}:{rest}")
        }
    } else if path.starts_with('/') {
        path.replace('/', "\\")
    } else {
        format!("\\\\{}", path.replace('/', "\\"))
    }
}

/// Convert a 0-indexed LSP position (line, UTF-16 character offset) into a byte
/// offset into `content`. Clamps to the end of the target line / of `content`.
fn position_to_byte_offset(content: &str, line: u32, character: u32) -> usize {
    let bytes = content.as_bytes();
    let mut idx = 0usize;
    let mut cur_line = 0u32;
    while cur_line < line && idx < bytes.len() {
        if bytes[idx] == b'\n' {
            cur_line += 1;
        }
        idx += 1;
    }
    // `idx` is now at the first byte of the target line (or end of content).
    let mut utf16 = 0u32;
    for ch in content[idx..].chars() {
        if utf16 >= character || ch == '\n' {
            break;
        }
        utf16 += ch.len_utf16() as u32;
        idx += ch.len_utf8();
    }
    idx
}

/// Apply a set of LSP text edits to `content`, returning the new text. Edits are
/// applied last-position-first so that byte offsets of earlier edits remain
/// valid (the LSP spec guarantees edits do not overlap).
fn apply_text_edits(content: &str, edits: &[TextEdit]) -> String {
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by(|a, b| {
        (b.range.start.line, b.range.start.character)
            .cmp(&(a.range.start.line, a.range.start.character))
    });
    let mut result = content.to_string();
    for edit in ordered {
        let start =
            position_to_byte_offset(&result, edit.range.start.line, edit.range.start.character);
        let end = position_to_byte_offset(&result, edit.range.end.line, edit.range.end.character);
        if start <= end && end <= result.len() {
            result.replace_range(start..end, &edit.new_text);
        }
    }
    result
}

/// Convert an LSP text edit into the crate's `TextEdit`.
fn text_edit_from_lsp(e: lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        range: range_from_lsp(e.range),
        new_text: e.new_text,
    }
}

/// Apply text edits to a file on disk. Returns the number of edits written (0
/// when the edit list is empty or the content is unchanged).
fn apply_edits_to_file(file_path: &str, edits: &[TextEdit]) -> Result<usize> {
    if edits.is_empty() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(file_path)?;
    let updated = apply_text_edits(&content, edits);
    if updated == content {
        return Ok(0);
    }
    std::fs::write(file_path, updated)?;
    Ok(edits.len())
}

/// Decode a percent-encoded string by collecting bytes then UTF-8 decoding.
fn percent_decode_utf8(s: &str) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let high = chars.next().and_then(|c| c.to_digit(16));
            let low = chars.next().and_then(|c| c.to_digit(16));
            if let (Some(h), Some(l)) = (high, low) {
                bytes.push(((h << 4) | l) as u8);
            } else {
                bytes.push(b'%');
            }
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

/// The symbols of `file` as `response` gives them, a tree flattened parent
/// first. Each symbol is located at its name in `text`, the file's (see
/// [`at_name`]), and keeps its own name, its parent's in `container_name`, as
/// a flat answer gives them.
fn symbols_from_lsp(
    response: Option<lsp_types::DocumentSymbolResponse>,
    file: &str,
    text: Option<&str>,
) -> Vec<Symbol> {
    fn flatten(
        symbols: Vec<lsp_types::DocumentSymbol>,
        parent: Option<&str>,
        file: &str,
        text: Option<&str>,
        out: &mut Vec<Symbol>,
    ) {
        for s in symbols {
            let range = text.map_or(s.selection_range, |t| {
                at_name(t, &s.name, s.selection_range)
            });
            out.push(Symbol {
                name: s.name.clone(),
                kind: format!("{:?}", s.kind),
                location: Location::new(file.to_string(), range_from_lsp(range)),
                container_name: parent.map(str::to_string),
            });
            if let Some(children) = s.children {
                flatten(children, Some(&s.name), file, text, out);
            }
        }
    }

    match response {
        Some(lsp_types::DocumentSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|s| {
                let mut location = s.location;
                if let Some(t) = text {
                    location.range = at_name(t, &s.name, location.range);
                }
                Symbol {
                    name: s.name,
                    kind: format!("{:?}", s.kind),
                    location: location_from_lsp(location),
                    container_name: s.container_name,
                }
            })
            .collect(),
        Some(lsp_types::DocumentSymbolResponse::Nested(symbols)) => {
            let mut out = Vec::new();
            flatten(symbols, None, file, text, &mut out);
            out
        }
        None => Vec::new(),
    }
}

/// The range of `name` in `text`, for a symbol the server put at `range`:
/// `range` where the text there starts with the name, else the name's first
/// whole-word occurrence inside `range` on a line that is no comment, and
/// `range` itself where the name is not in it. FSAutoComplete starts a
/// symbol's selection range at its doc comment, and a flat answer gives a
/// declaration's whole range (`package com.tidewater.berths;` for the
/// package): a position there is not on the symbol, and asked about it, a
/// server finds nothing. The name is also looked for without its signature
/// (`overlaps(BerthWindow)`) and its qualifier (`Revenue.total`). Columns
/// count UTF-16 code units, as the server's do.
fn at_name(text: &str, name: &str, range: lsp_types::Range) -> lsp_types::Range {
    fn utf16(s: &str) -> u32 {
        u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
    }
    /// The byte offset of the UTF-16 column `character` in `line`.
    fn byte_at(line: &str, character: u32) -> Option<usize> {
        let mut units = 0;
        for (byte, c) in line.char_indices() {
            if units >= character {
                return Some(byte);
            }
            units += u32::try_from(c.len_utf16()).unwrap_or(2);
        }
        (units >= character).then_some(line.len())
    }
    fn identifier(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    const COMMENTS: &[&str] = &["//", "#", "--", "/*", "*", "(*", ";"];

    let lines: Vec<&str> = text.lines().collect();
    let mut names = vec![name];
    for part in [
        name.split(['(', '<', ' ', ':']).next(),
        name.rsplit(['.', ':']).next(),
    ]
    .into_iter()
    .flatten()
    {
        if !part.is_empty() && !names.contains(&part) {
            names.push(part);
        }
    }
    let at = |line: u32, character: u32, n: &str| {
        lines
            .get(line as usize)
            .and_then(|l| byte_at(l, character).map(|b| l[b..].starts_with(n)))
            .unwrap_or(false)
    };
    if names
        .iter()
        .any(|n| at(range.start.line, range.start.character, n))
    {
        return range;
    }
    let last = range.end.line.min(
        u32::try_from(lines.len())
            .unwrap_or(u32::MAX)
            .saturating_sub(1),
    );
    for n in &names {
        for line_no in range.start.line..=last {
            let Some(line) = lines.get(line_no as usize) else {
                break;
            };
            if COMMENTS.iter().any(|c| line.trim_start().starts_with(c)) {
                continue;
            }
            for (byte, _) in line.match_indices(n) {
                let column = utf16(&line[..byte]);
                let end = column + utf16(n);
                let whole_word = !line[..byte].chars().next_back().is_some_and(identifier)
                    && !line[byte + n.len()..]
                        .chars()
                        .next()
                        .is_some_and(identifier);
                let inside = (line_no > range.start.line || column >= range.start.character)
                    && (line_no < range.end.line || end <= range.end.character);
                if whole_word && inside {
                    return lsp_types::Range {
                        start: lsp_types::Position {
                            line: line_no,
                            character: column,
                        },
                        end: lsp_types::Position {
                            line: line_no,
                            character: end,
                        },
                    };
                }
            }
        }
    }
    range
}

/// Convert an LSP location to our Location type.
fn location_from_lsp(loc: lsp_types::Location) -> Location {
    Location::new(uri_to_path(&loc.uri), range_from_lsp(loc.range))
}

/// Convert an LSP range to our Range type.
///
/// Both LSP positions and our `Position2D`/`Range` types are 0-indexed,
/// so this is a direct passthrough.
fn range_from_lsp(range: lsp_types::Range) -> Range {
    Range {
        start: Position2D {
            line: range.start.line,
            character: range.start.character,
        },
        end: Position2D {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

/// Convert a `MarkedString` to a String.
///
/// lsp-types 0.97 `MarkedString` doesn't implement Display.
fn marked_string_to_string(ms: lsp_types::MarkedString) -> String {
    match ms {
        lsp_types::MarkedString::String(s) => s,
        lsp_types::MarkedString::LanguageString(ls) => {
            format!(
                "```{language}\n{value}\n```",
                language = ls.language,
                value = ls.value
            )
        }
    }
}

/// Convert our Range type to LSP Range.
fn range_to_lsp(range: Range) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: range.start.line,
            character: range.start.character,
        },
        end: lsp_types::Position {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

/// Parse a symbol kind string to `lsp_types::SymbolKind`.
fn parse_symbol_kind(kind: &str) -> lsp_types::SymbolKind {
    match kind {
        "file" => lsp_types::SymbolKind::FILE,
        "module" => lsp_types::SymbolKind::MODULE,
        "namespace" => lsp_types::SymbolKind::NAMESPACE,
        "package" => lsp_types::SymbolKind::PACKAGE,
        "class" => lsp_types::SymbolKind::CLASS,
        "method" => lsp_types::SymbolKind::METHOD,
        "property" => lsp_types::SymbolKind::PROPERTY,
        "field" => lsp_types::SymbolKind::FIELD,
        "constructor" => lsp_types::SymbolKind::CONSTRUCTOR,
        "enum" => lsp_types::SymbolKind::ENUM,
        "interface" => lsp_types::SymbolKind::INTERFACE,
        "function" => lsp_types::SymbolKind::FUNCTION,
        "variable" => lsp_types::SymbolKind::VARIABLE,
        "constant" => lsp_types::SymbolKind::CONSTANT,
        "string" => lsp_types::SymbolKind::STRING,
        "number" => lsp_types::SymbolKind::NUMBER,
        "boolean" => lsp_types::SymbolKind::BOOLEAN,
        "array" => lsp_types::SymbolKind::ARRAY,
        "object" => lsp_types::SymbolKind::OBJECT,
        "key" => lsp_types::SymbolKind::KEY,
        "null" => lsp_types::SymbolKind::NULL,
        "enumMember" => lsp_types::SymbolKind::ENUM_MEMBER,
        "struct" => lsp_types::SymbolKind::STRUCT,
        "event" => lsp_types::SymbolKind::EVENT,
        "operator" => lsp_types::SymbolKind::OPERATOR,
        "typeParameter" => lsp_types::SymbolKind::TYPE_PARAMETER,
        _ => lsp_types::SymbolKind::VARIABLE,
    }
}

/// Convert LSP `CallHierarchyItem` to our type.
fn call_hierarchy_item_from_lsp(item: lsp_types::CallHierarchyItem) -> CallHierarchyItem {
    let file_path = uri_to_path(&item.uri);
    let name = range_from_lsp(item.selection_range);
    CallHierarchyItem {
        name: item.name,
        kind: format!("{:?}", item.kind),
        cite: cite(&file_path, name.start.line, name.start.character),
        location: Location::new(file_path, range_from_lsp(item.range)),
        selection_range: Some(name),
        container_name: None,
        data: item.data.unwrap_or_default(),
    }
}

/// Our `CallHierarchyItem` as the LSP item it came from, for a follow-up
/// request. An item without a `selection_range` — built by a caller rather
/// than returned by `get_call_hierarchy` — is selected by its whole range.
fn call_hierarchy_item_to_lsp(item: &CallHierarchyItem) -> Result<lsp_types::CallHierarchyItem> {
    let selection = item
        .selection_range
        .clone()
        .unwrap_or_else(|| item.location.range.clone());
    Ok(lsp_types::CallHierarchyItem {
        name: item.name.clone(),
        kind: parse_symbol_kind(&item.kind),
        tags: None,
        detail: None,
        uri: path_to_uri(Path::new(&item.location.file_path))?,
        range: range_to_lsp(item.location.range.clone()),
        selection_range: range_to_lsp(selection),
        data: Some(item.data.clone()),
    })
}

/// Convert LSP `TypeHierarchyItem` to our type.
fn type_hierarchy_item_from_lsp(item: lsp_types::TypeHierarchyItem) -> TypeHierarchyItem {
    let file_path = uri_to_path(&item.uri);
    let name = range_from_lsp(item.selection_range);
    TypeHierarchyItem {
        name: item.name,
        kind: format!("{:?}", item.kind),
        cite: cite(&file_path, name.start.line, name.start.character),
        location: Location::new(file_path, range_from_lsp(item.range)),
        selection_range: Some(name),
        data: item.data.unwrap_or_default(),
    }
}

/// Our `TypeHierarchyItem` as the LSP item it came from; see
/// [`call_hierarchy_item_to_lsp`].
fn type_hierarchy_item_to_lsp(item: &TypeHierarchyItem) -> Result<lsp_types::TypeHierarchyItem> {
    let selection = item
        .selection_range
        .clone()
        .unwrap_or_else(|| item.location.range.clone());
    Ok(lsp_types::TypeHierarchyItem {
        name: item.name.clone(),
        kind: parse_symbol_kind(&item.kind),
        tags: None,
        detail: None,
        uri: path_to_uri(Path::new(&item.location.file_path))?,
        range: range_to_lsp(item.location.range.clone()),
        selection_range: range_to_lsp(selection),
        data: Some(item.data.clone()),
    })
}

/// Parse completion kind string.
fn parse_completion_kind(kind: &str) -> Option<lsp_types::CompletionItemKind> {
    match kind {
        "text" => Some(lsp_types::CompletionItemKind::TEXT),
        "method" => Some(lsp_types::CompletionItemKind::METHOD),
        "function" => Some(lsp_types::CompletionItemKind::FUNCTION),
        "constructor" => Some(lsp_types::CompletionItemKind::CONSTRUCTOR),
        "field" => Some(lsp_types::CompletionItemKind::FIELD),
        "variable" => Some(lsp_types::CompletionItemKind::VARIABLE),
        "class" => Some(lsp_types::CompletionItemKind::CLASS),
        "interface" => Some(lsp_types::CompletionItemKind::INTERFACE),
        "module" => Some(lsp_types::CompletionItemKind::MODULE),
        "property" => Some(lsp_types::CompletionItemKind::PROPERTY),
        "unit" => Some(lsp_types::CompletionItemKind::UNIT),
        "value" => Some(lsp_types::CompletionItemKind::VALUE),
        "enum" => Some(lsp_types::CompletionItemKind::ENUM),
        "keyword" => Some(lsp_types::CompletionItemKind::KEYWORD),
        "snippet" => Some(lsp_types::CompletionItemKind::SNIPPET),
        "color" => Some(lsp_types::CompletionItemKind::COLOR),
        "file" => Some(lsp_types::CompletionItemKind::FILE),
        "reference" => Some(lsp_types::CompletionItemKind::REFERENCE),
        "folder" => Some(lsp_types::CompletionItemKind::FOLDER),
        "enumMember" => Some(lsp_types::CompletionItemKind::ENUM_MEMBER),
        "constant" => Some(lsp_types::CompletionItemKind::CONSTANT),
        "struct" => Some(lsp_types::CompletionItemKind::STRUCT),
        "event" => Some(lsp_types::CompletionItemKind::EVENT),
        "operator" => Some(lsp_types::CompletionItemKind::OPERATOR),
        "typeParameter" => Some(lsp_types::CompletionItemKind::TYPE_PARAMETER),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Component;

    /// `path` spelled relative to the current directory: `..` up to what the
    /// two have in common, then the rest of `path`. A path on another Windows
    /// drive than the current directory has no such spelling.
    fn relative_to_cwd(path: &Path) -> Option<PathBuf> {
        let cwd = std::env::current_dir().ok()?;
        let (cwd, path): (Vec<_>, Vec<_>) =
            (cwd.components().collect(), path.components().collect());
        let root = |parts: &[Component]| {
            parts
                .iter()
                .take_while(|part| !matches!(part, Component::Normal(_)))
                .count()
        };
        if root(&cwd) != root(&path) || cwd[..root(&cwd)] != path[..root(&path)] {
            return None;
        }
        let common = cwd.iter().zip(&path).take_while(|(a, b)| a == b).count();
        let mut relative: PathBuf =
            std::iter::repeat_n(Component::ParentDir, cwd.len() - common).collect();
        relative.extend(&path[common..]);
        Some(relative)
    }

    /// A file whose registry server does not run here is told which server
    /// and why; one no server is registered for is told there is none.
    #[test]
    fn a_file_is_told_why_no_server_answers() {
        let unusable = no_server_for("rb").to_string();
        assert!(unusable.contains("(ruby-lsp) for .rb files"), "{unusable}");
        let why = crate::manager::why_unavailable("ruby-lsp").unwrap().reason;
        assert!(unusable.ends_with(&format!("files {why}")), "{unusable}");
        assert!(matches!(no_server_for("xyz"), LspError::NoServerForFile(_)));
    }

    // ---- LSP capability detection (turns opaque -32601 into a clear error) ----

    fn range(a: (u32, u32), b: (u32, u32)) -> lsp_types::Range {
        lsp_types::Range {
            start: lsp_types::Position {
                line: a.0,
                character: a.1,
            },
            end: lsp_types::Position {
                line: b.0,
                character: b.1,
            },
        }
    }

    /// A symbol is put at its name: FSAutoComplete's selection range starts
    /// at the doc comment, and a flat answer gives the whole declaration; one
    /// already at its name stays, as does one whose name is not in its range.
    #[test]
    fn a_symbol_is_put_at_its_name() {
        let fsharp =
            "/// Issued sequence numbers, by counter key.\ntype Counters = Map<string, int>\n";
        assert_eq!(
            at_name(fsharp, "Counters", range((0, 0), (1, 32))),
            range((1, 5), (1, 13))
        );
        assert_eq!(
            at_name(fsharp, "Counters", range((1, 5), (1, 13))),
            range((1, 5), (1, 13))
        );
        let java = "package com.tidewater.berths;\n";
        assert_eq!(
            at_name(java, "com.tidewater.berths", range((0, 0), (0, 29))),
            range((0, 8), (0, 28))
        );
        // A comment that names it, a longer word that holds it, a signature.
        let noisy = "// Counters by key\nlet Countersx = 1\nlet Counters(key) = 2\n";
        assert_eq!(
            at_name(noisy, "Counters(string)", range((0, 0), (2, 21))),
            range((2, 4), (2, 12))
        );
        assert_eq!(
            at_name(noisy, "Missing", range((0, 0), (2, 21))),
            range((0, 0), (2, 21))
        );
    }

    /// Columns count UTF-16 code units, as the server's do: `ü` is one, and
    /// two bytes.
    #[test]
    fn a_symbol_is_put_at_its_name_in_utf16_columns() {
        let text = "fn ü(); fn target() {}\n";
        assert_eq!(
            at_name(text, "target", range((0, 0), (0, 22))),
            range((0, 11), (0, 17))
        );
    }

    /// The symbols of a tree answer and of a flat one are both put at their
    /// names in the file's text.
    #[test]
    fn document_symbols_are_located_at_their_names() {
        let text = "/// The counters.\ntype Counters = int\n";
        let nested = lsp_types::DocumentSymbolResponse::Nested(vec![
            #[allow(deprecated)]
            lsp_types::DocumentSymbol {
                name: "Counters".into(),
                detail: None,
                kind: lsp_types::SymbolKind::STRUCT,
                tags: None,
                deprecated: None,
                range: range((0, 0), (1, 19)),
                selection_range: range((0, 0), (1, 19)),
                children: None,
            },
        ]);
        let symbols = symbols_from_lsp(Some(nested), "/p/a.fs", Some(text));
        assert_eq!(symbols[0].location.range.start.line, 1);
        assert_eq!(symbols[0].location.range.start.character, 5);
        let uri: lsp_types::Uri = "file:///p/a.fs".parse().unwrap();
        #[allow(deprecated)]
        let flat = lsp_types::DocumentSymbolResponse::Flat(vec![lsp_types::SymbolInformation {
            name: "Counters".into(),
            kind: lsp_types::SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            location: lsp_types::Location {
                uri,
                range: range((0, 0), (1, 19)),
            },
            container_name: None,
        }]);
        let symbols = symbols_from_lsp(Some(flat), "/p/a.fs", Some(text));
        assert_eq!(symbols[0].location.range.start.line, 1);
        assert_eq!(symbols[0].location.range.start.character, 5);
    }

    #[test]
    fn capability_supported_reads_lsp_provider_shapes() {
        use serde_json::json;
        // `true` or an options object → supported.
        let caps = json!({
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": { "resolveProvider": false },
        });
        assert!(capability_supported(&caps, "documentSymbolProvider"));
        assert!(capability_supported(&caps, "workspaceSymbolProvider"));

        // `false`, `null`, or absent → unsupported.
        let caps = json!({
            "documentSymbolProvider": false,
            "hoverProvider": serde_json::Value::Null,
        });
        assert!(!capability_supported(&caps, "documentSymbolProvider"));
        assert!(!capability_supported(&caps, "hoverProvider"));
        assert!(!capability_supported(&caps, "workspaceSymbolProvider")); // absent
        assert!(!capability_supported(&json!({}), "documentSymbolProvider"));
    }

    // ---- text-edit application (rename / format write-back) ----

    fn te(sl: u32, sc: u32, el: u32, ec: u32, new: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position2D {
                    line: sl,
                    character: sc,
                },
                end: Position2D {
                    line: el,
                    character: ec,
                },
            },
            new_text: new.to_string(),
        }
    }

    #[test]
    fn position_to_byte_offset_walks_lines_and_clamps() {
        let src = "ab\ncd\n";
        assert_eq!(position_to_byte_offset(src, 0, 0), 0);
        assert_eq!(position_to_byte_offset(src, 0, 2), 2); // end of "ab"
        assert_eq!(position_to_byte_offset(src, 1, 0), 3); // start of "cd"
        assert_eq!(position_to_byte_offset(src, 1, 1), 4);
        assert_eq!(position_to_byte_offset(src, 0, 99), 2); // past EOL clamps
    }

    #[test]
    fn apply_text_edits_single_rename() {
        let out = apply_text_edits("const oldName = 1;\n", &[te(0, 6, 0, 13, "renamed")]);
        assert_eq!(out, "const renamed = 1;\n");
    }

    #[test]
    fn apply_text_edits_multiple_edits_out_of_order() {
        // Two edits on different lines, supplied later-first and earlier-second;
        // both must land correctly (offsets stay valid because we apply tail-first).
        let edits = vec![te(2, 0, 2, 3, "baz"), te(0, 0, 0, 3, "baz")];
        let out = apply_text_edits("foo();\nbar();\nfoo();\n", &edits);
        assert_eq!(out, "baz();\nbar();\nbaz();\n");
    }

    #[test]
    fn apply_text_edits_insertion() {
        let out = apply_text_edits("a\nb\n", &[te(1, 0, 1, 0, "X")]);
        assert_eq!(out, "a\nXb\n");
    }

    fn open_doc(last_used: u64) -> OpenDocument {
        OpenDocument {
            version: 1,
            mtime: None,
            last_used,
        }
    }

    /// Past the cap, a language's least recently used documents are closed —
    /// never the one just opened, and never another language's.
    #[test]
    fn the_least_recently_used_documents_of_the_language_are_closed() {
        let mut open = HashMap::new();
        open.insert("/p/a.rs".to_string(), open_doc(5));
        open.insert("/p/b.rs".to_string(), open_doc(1));
        open.insert("/p/c.rs".to_string(), open_doc(3));
        open.insert("/p/new.rs".to_string(), open_doc(0));
        open.insert("/p/old.py".to_string(), open_doc(0));
        let language = |path: &str| {
            if path.ends_with(".rs") {
                "rust"
            } else {
                "python"
            }
            .to_string()
        };

        let closed = documents_to_close(&open, language, "rust", "/p/new.rs", 2);
        assert_eq!(closed, ["/p/b.rs", "/p/c.rs"]);
        assert!(documents_to_close(&open, language, "rust", "/p/new.rs", 4).is_empty());
    }

    /// Edit columns count UTF-16 code units, the only encoding offered to
    /// servers: `ñ` and `ú` are one unit (two UTF-8 bytes) each. Read as byte
    /// offsets, this rename cut `(a` from the line.
    #[test]
    fn apply_text_edits_counts_utf16_units_on_non_ascii_lines() {
        let src = "// Déjà vu\nfn ñandú_total(a: i32) -> i32 { a + 1 }\n";
        let out = apply_text_edits(src, &[te(1, 3, 1, 14, "total")]);
        assert_eq!(out, "// Déjà vu\nfn total(a: i32) -> i32 { a + 1 }\n");
    }

    /// A character outside the Basic Multilingual Plane is two UTF-16 units.
    #[test]
    fn apply_text_edits_counts_a_surrogate_pair_as_two_units() {
        // `let s = "` is 9 units, the emoji 2, `"; let ` 7: `x` starts at 18.
        let out = apply_text_edits("let s = \"😀\"; let x = 1;\n", &[te(0, 18, 0, 19, "y")]);
        assert_eq!(out, "let s = \"😀\"; let y = 1;\n");
    }

    // ---- workspace root resolution (lazy, file-rooted LSP startup) ----

    /// The real shipped preset, so these tests track the actual markers.
    fn rust_config() -> LspServerConfig {
        let c = LspServerConfig::rust_analyzer();
        assert!(
            c.heuristics
                .as_ref()
                .is_some_and(|h| h.markers.iter().any(|m| m == "Cargo.toml")),
            "rust preset must key on Cargo.toml"
        );
        c
    }

    /// A workspace member must resolve to the *workspace* root, not to itself:
    /// `crates/lsp/Cargo.toml` inherits `version.workspace = true` and cannot be
    /// loaded standalone — rust-analyzer rooted there rejects the file.
    #[test]
    fn workspace_member_resolves_to_the_repository_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();

        let member = root.join("crates/lsp/src");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(root.join("crates/lsp/Cargo.toml"), "[package]\n").unwrap();

        let file = member.join("lib.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
        assert_eq!(got, root, "must pick the outermost Cargo.toml");
    }

    /// The search must not ascend past the enclosing repository, or a stray
    /// `Cargo.toml` in a parent directory would swallow unrelated projects.
    #[test]
    fn search_stops_at_the_vcs_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        // A decoy marker *above* the repo.
        std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").unwrap();

        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]\n").unwrap();
        let src = repo.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
        assert_eq!(got, repo, "must not escape the .git boundary");
    }

    /// A standalone crate with no VCS still resolves to its own root.
    #[test]
    fn standalone_crate_resolves_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("crate");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        let file = root.join("src/main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let got = Translator::workspace_root_for(&rust_config(), &file).unwrap();
        assert_eq!(got, root);
    }

    /// A loose source file with no project marker anywhere yields `None`, so the
    /// caller can tell the agent exactly what is missing instead of reporting
    /// "no LSP server configured".
    #[test]
    fn no_marker_anywhere_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let file = tmp.path().join("loose.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        assert!(Translator::workspace_root_for(&rust_config(), &file).is_none());
    }

    /// A C file belongs to the project its build file or clangd configuration
    /// is in: a Makefile project's, for a file under its `src/`, not `src/`
    /// alone.
    #[test]
    fn a_c_file_belongs_to_its_makefile_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let project = tmp.path().join("services/geo-cpp");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("Makefile"), "all:\n").unwrap();
        let file = project.join("src/luhn.c");
        std::fs::write(&file, "int luhn_check_digit(const char *s);\n").unwrap();

        let mut clangd = rust_config();
        clangd.language_id = "cpp".into();
        clangd.heuristics = crate::generated_registry::heuristics("clangd");
        assert_eq!(
            Translator::workspace_root_for(&clangd, &file),
            Some(project)
        );
    }

    /// A loose file for a non-build-graph language (e.g. Python) with no marker
    /// falls back to the file's own directory instead of yielding `None`, so a
    /// bare `.py` script is still analysable. Strict languages (Rust) do not.
    #[test]
    fn marker_less_non_strict_language_falls_back_to_file_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let file = tmp.path().join("scratch.py");
        std::fs::write(&file, "def f():\n    pass\n").unwrap();

        // A Python-shaped config: has heuristics, but none of its markers exist.
        let mut c = rust_config();
        c.language_id = "python".into();
        c.heuristics = Some(crate::config::ServerHeuristics {
            markers: vec!["pyproject.toml".into(), "requirements.txt".into()],
            max_depth: 3,
        });

        let got = Translator::workspace_root_for(&c, &file).unwrap();
        assert_eq!(
            got,
            tmp.path(),
            "marker-less Python roots at the file's dir"
        );

        // A strict, build-graph language stays unavailable.
        let mut strict = c.clone();
        strict.language_id = "csharp".into();
        assert!(
            Translator::workspace_root_for(&strict, &file).is_none(),
            "build-graph languages must not fall back to a bare directory"
        );
    }

    /// A server without heuristics matches anywhere and is rooted at the file.
    #[test]
    fn config_without_heuristics_roots_at_the_file_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("x.rs");
        std::fs::write(&file, "").unwrap();

        let mut c = rust_config();
        c.heuristics = None;
        let got = Translator::workspace_root_for(&c, &file).unwrap();
        assert_eq!(got, tmp.path());
    }

    /// A strict-root server starting for a monorepo whose root is no project
    /// of its kind gets the projects inside the root; another server gets the
    /// root. Rooted at such a root, rust-analyzer answered `file not found`.
    #[test]
    fn startup_folders_root_a_strict_server_at_the_projects_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("services/pricing-rust/src")).unwrap();
        std::fs::write(root.join("services/pricing-rust/Cargo.toml"), "[package]\n").unwrap();

        let mut python = rust_config();
        python.language_id = "python".into();
        let mut translator = Translator::new(Config {
            workspace: crate::config::WorkspaceConfig::default(),
            servers: vec![rust_config(), python.clone()],
            lifecycle: crate::config::LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![root.clone()]);

        assert_eq!(
            translator.startup_folders(&rust_config()),
            vec![root.join("services/pricing-rust")]
        );
        assert_eq!(translator.startup_folders(&python), vec![root]);
    }

    /// kotlin-lsp imports a Gradle build only at the root of a workspace
    /// folder: starting for a monorepo it gets the projects inside, and for a
    /// tree that holds none, the root, where it still serves each file.
    #[test]
    fn startup_folders_give_kotlin_the_projects_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("depot");
        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        std::fs::create_dir_all(root.join(".git")).unwrap();
        write("services/reefer-kotlin/build.gradle.kts");
        write("services/reefer-kotlin/src/main/kotlin/SetPoint.kt");
        // A Groovy build, a Maven build and a multi-project build hold
        // Kotlin as much as a build written in the Kotlin DSL.
        write("services/rates-groovy/build.gradle");
        write("services/rates-groovy/src/main/kotlin/Rate.kt");
        write("services/ledger-maven/pom.xml");
        write("services/ledger-maven/src/main/kotlin/Ledger.kt");
        write("platform/settings.gradle.kts");
        write("platform/core/build.gradle.kts");
        write("platform/core/src/main/kotlin/Core.kt");
        // Builds without Kotlin are no project of kotlin-lsp's: its build
        // script is no Kotlin source.
        write("services/berths-java/pom.xml");
        write("services/berths-java/src/main/java/Berth.java");
        write("services/billing-java/build.gradle.kts");
        write("services/billing-java/src/main/java/Bill.java");
        let loose = dunce::canonicalize(tmp.path()).unwrap().join("scripts");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("tool.kt"), "fun main() {}\n").unwrap();

        let mut kotlin = rust_config();
        kotlin.language_id = "kotlin".into();
        kotlin.extensions = vec!["kt".into(), "kts".into()];
        kotlin.heuristics = crate::generated_registry::heuristics("kotlin-lsp");
        let mut translator = Translator::new(Config {
            workspace: crate::config::WorkspaceConfig::default(),
            servers: vec![kotlin.clone()],
            lifecycle: crate::config::LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![root.clone()]);
        assert_eq!(
            translator.startup_folders(&kotlin),
            [
                "platform",
                "services/ledger-maven",
                "services/rates-groovy",
                "services/reefer-kotlin",
            ]
            .map(|project| root.join(project))
        );
        assert_eq!(
            translator.folders_for_file(&kotlin, root.join("services/reefer-kotlin")),
            translator.startup_folders(&kotlin)
        );
        assert!(Translator::knows_only_its_folders("kotlin"));

        translator.set_workspace_roots(vec![loose.clone()]);
        assert_eq!(translator.startup_folders(&kotlin), vec![loose]);
    }

    /// sourcekit-lsp is given the Swift packages inside a monorepo's root: given
    /// the root, it answers every request for a file of a package below it
    /// with `No language service`. Where there is no package, the root.
    #[test]
    fn startup_folders_give_swift_the_packages_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("depot");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("apps/reefer-ios/Sources/ReeferKit")).unwrap();
        std::fs::write(root.join("apps/reefer-ios/Package.swift"), "").unwrap();
        std::fs::write(
            root.join("apps/reefer-ios/Sources/ReeferKit/SetPointRange.swift"),
            "",
        )
        .unwrap();
        let loose = dunce::canonicalize(tmp.path()).unwrap().join("scripts");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("tool.swift"), "let limit = 3\n").unwrap();

        let mut swift = rust_config();
        swift.language_id = "swift".into();
        swift.extensions = vec!["swift".into()];
        swift.heuristics = Some(crate::config::ServerHeuristics {
            markers: vec!["Package.swift".into()],
            max_depth: 3,
        });
        let mut translator = Translator::new(Config {
            workspace: crate::config::WorkspaceConfig::default(),
            servers: vec![swift.clone()],
            lifecycle: crate::config::LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![root.clone()]);
        assert_eq!(
            translator.startup_folders(&swift),
            vec![root.join("apps/reefer-ios")]
        );
        assert!(Translator::knows_only_its_folders("swift"));

        translator.set_workspace_roots(vec![loose.clone()]);
        assert_eq!(translator.startup_folders(&swift), vec![loose]);
    }

    /// A server whose project markers are absent still starts where files of
    /// its language are, as a request for one of them would start it; a server
    /// that needs its manifest does not.
    #[test]
    fn a_server_starts_where_its_files_are_unless_it_needs_its_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
        std::fs::create_dir_all(root.join("services/recommender-python")).unwrap();
        std::fs::write(
            root.join("services/recommender-python/recommender.py"),
            "x = 1\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("services/loose-rust")).unwrap();
        std::fs::write(root.join("services/loose-rust/main.rs"), "fn main() {}\n").unwrap();

        let mut translator = Translator::new(Config {
            workspace: crate::config::WorkspaceConfig::default(),
            servers: vec![LspServerConfig::pyright(), rust_config()],
            lifecycle: crate::config::LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![root]);
        assert_eq!(
            translator.startable_languages(["python".to_string(), "rust".to_string()]),
            ["python"],
            "pyright needs no marker; rust-analyzer needs a Cargo.toml"
        );
    }

    /// Folders are absolute whatever the spelling of the workspace root, so
    /// that a file's project, resolved from its absolute path, is found among
    /// them.
    #[test]
    fn startup_folders_are_absolute_for_a_relative_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("shop");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("services/pricing-rust/src")).unwrap();
        std::fs::write(root.join("services/pricing-rust/Cargo.toml"), "[package]\n").unwrap();
        let file = root.join("services/pricing-rust/src/lib.rs");
        std::fs::write(&file, "").unwrap();

        let mut translator = Translator::new(Config {
            workspace: crate::config::WorkspaceConfig::default(),
            servers: vec![rust_config()],
            lifecycle: crate::config::LifecycleConfig::default(),
        });
        // `root` spelled relative to the current directory, as `.` is; a root
        // on another Windows drive has no such spelling to try.
        let Some(relative) = relative_to_cwd(&root) else {
            return;
        };
        translator.set_workspace_roots(vec![relative]);

        let folders = translator.startup_folders(&rust_config());
        let project = Translator::workspace_root_for(&rust_config(), &file).unwrap();
        assert_eq!(folders, vec![project]);
    }

    /// Documents of the TypeScript server are opened as their own languages;
    /// others as their server's.
    #[test]
    fn documents_are_opened_as_their_own_language() {
        for (file, id) in [
            ("CartBanner.tsx", "typescriptreact"),
            ("pricing.ts", "typescript"),
            ("app.jsx", "javascriptreact"),
            ("app.mjs", "javascript"),
        ] {
            assert_eq!(
                document_language_id(Path::new(file), "typescript"),
                id,
                "{file}"
            );
        }
        assert_eq!(document_language_id(Path::new("lib.rs"), "rust"), "rust");
    }

    /// A hierarchy item keeps the range of its name through the round trip
    /// to the agent and back: servers resolve a passed-back item by it.
    #[test]
    fn hierarchy_items_keep_their_selection_range() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("lib.rs");
        std::fs::write(&file, "pub fn apply_bulk_discount() {}\n").unwrap();
        let span = |l0, c0, l1, c1| lsp_types::Range {
            start: lsp_types::Position {
                line: l0,
                character: c0,
            },
            end: lsp_types::Position {
                line: l1,
                character: c1,
            },
        };
        let lsp_item = lsp_types::CallHierarchyItem {
            name: "apply_bulk_discount".into(),
            kind: lsp_types::SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri: path_to_uri(&file).unwrap(),
            range: span(40, 0, 47, 1),
            selection_range: span(41, 7, 41, 26),
            data: None,
        };

        let ours = call_hierarchy_item_from_lsp(lsp_item.clone());
        let back = call_hierarchy_item_to_lsp(&ours).unwrap();
        assert_eq!(back.selection_range, lsp_item.selection_range);
        assert_eq!(back.range, lsp_item.range);

        // One an agent built without it is selected by its whole range.
        let built = CallHierarchyItem {
            selection_range: None,
            ..ours
        };
        assert_eq!(
            call_hierarchy_item_to_lsp(&built).unwrap().selection_range,
            lsp_item.range
        );
    }

    /// A Windows path is written in a `file:` URI with `/`, after a `/`, and a
    /// UNC path with its server as the host; and read back from either
    /// spelling of the drive.
    #[test]
    fn windows_paths_and_their_uris() {
        assert_eq!(windows_uri_path(r"C:\Users\a b\x.rs"), "/C:/Users/a b/x.rs");
        assert_eq!(windows_uri_path(r"\\srv\share\x.rs"), "srv/share/x.rs");
        assert_eq!(windows_path("/C:/Users/a b/x.rs"), r"C:\Users\a b\x.rs");
        assert_eq!(windows_path("/c:/Users/x.rs"), r"C:\Users\x.rs");
        assert_eq!(windows_path("/D:"), r"D:\");
        assert_eq!(windows_path("srv/share/x.rs"), r"\\srv\share\x.rs");
    }

    #[cfg(unix)]
    #[test]
    fn test_uri_to_path_unix() {
        let uri: lsp_types::Uri = "file:///home/user/test.rs".parse().unwrap();
        assert_eq!(uri_to_path(&uri), "/home/user/test.rs");
    }

    #[test]
    fn test_range_from_lsp() {
        let lsp_range = lsp_types::Range {
            start: lsp_types::Position {
                line: 0,
                character: 0,
            },
            end: lsp_types::Position {
                line: 5,
                character: 10,
            },
        };

        let range = range_from_lsp(lsp_range);
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 5);
        assert_eq!(range.end.character, 10);
    }

    #[test]
    fn test_marked_string_to_string_plain() {
        let ms = lsp_types::MarkedString::String("hello world".into());
        assert_eq!(marked_string_to_string(ms), "hello world");
    }

    #[test]
    fn test_marked_string_to_string_language() {
        let ms = lsp_types::MarkedString::LanguageString(lsp_types::LanguageString {
            language: "rust".into(),
            value: "fn main() {}".into(),
        });
        assert_eq!(marked_string_to_string(ms), "```rust\nfn main() {}\n```");
    }

    /// path_to_uri percent-encodes spaces and special characters.
    #[test]
    fn path_to_uri_percent_encodes_spaces() {
        let path = std::path::Path::new("/home/user/my project/file.rs");
        let uri = path_to_uri(path).expect("path_to_uri should succeed");
        let uri_str = uri.to_string();
        assert!(
            !uri_str.contains(' '),
            "URI must not contain raw spaces; got: {uri_str}"
        );
        assert!(
            uri_str.contains("%20"),
            "space must be percent-encoded as %20; got: {uri_str}"
        );
    }

    /// uri_to_path decodes percent-encoded paths back correctly.
    #[test]
    fn uri_to_path_decodes_percent_encoding() {
        let uri: lsp_types::Uri = "file:///home/user/my%20project/file.rs".parse().unwrap();
        let path = uri_to_path(&uri);
        assert!(
            path.contains("my project"),
            "decoded path must contain 'my project'; got: {path}"
        );
    }
}
