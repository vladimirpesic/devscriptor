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
use crate::bridge::disk_changes::{DiskChange, DiskChangeLog};
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

mod convert;
mod documents;
mod requests;
mod servers;
#[cfg(test)]
mod tests;

pub(crate) use convert::windows_path;
use convert::*;
pub use convert::{path_to_uri, uri_to_path};

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
    /// The changes on disk each running server has yet to hear of
    /// ([`Self::note_disk_changes`]).
    disk_changes: Arc<parking_lot::Mutex<DiskChangeLog>>,
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
    /// The file's modification time and size when it was last read for the
    /// server.
    stamp: Option<DiskStamp>,
    /// A hash of the text last sent ([`text_hash`]).
    hash: u64,
    /// When the file was last read for the server: a time taken before the
    /// read.
    read_at: std::time::SystemTime,
    /// When a request last used it, as a tick of [`Translator::ticks`].
    last_used: u64,
}

/// A file's modification time and size.
type DiskStamp = (std::time::SystemTime, u64);

/// How long after a file's modification time another write can still leave
/// it with the same modification time and size: file systems step
/// modification times by up to two seconds (FAT; most by far less). A document
/// read within this long of its file's modification time is read again to
/// tell whether it changed.
const RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

impl OpenDocument {
    /// Whether the file, by its modification time and size alone, is still
    /// the version last sent: the same stamp, read [`RACY_WINDOW`] or more
    /// after its modification time.
    fn current_by(&self, stamp: Option<DiskStamp>) -> bool {
        match (self.stamp, stamp) {
            (Some(sent), Some(on_disk)) if sent == on_disk => self
                .read_at
                .duration_since(sent.0)
                .is_ok_and(|after| after >= RACY_WINDOW),
            _ => false,
        }
    }
}

/// The modification time and size of the file at `path`; `None` when it is
/// not a file.
async fn disk_stamp(path: &Path) -> Option<DiskStamp> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some((metadata.modified().ok()?, metadata.len()))
}

/// A hash of `text`, to tell whether a file read again holds what was sent.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
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
            disk_changes: Arc::new(parking_lot::Mutex::new(DiskChangeLog::default())),
        }
    }

    /// Note files created, changed or deleted on disk, which the running
    /// servers whose folders hold them have yet to hear of: each is told
    /// (`workspace/didChangeWatchedFiles`) before its next request. Servers
    /// started later read the files as they are.
    pub fn note_disk_changes(&self, changes: impl IntoIterator<Item = (PathBuf, DiskChange)>) {
        let mut log = self.disk_changes.lock();
        for (path, change) in changes {
            log.note(path.to_string_lossy().into_owned(), change);
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
        self.disk_changes.lock().clear();
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
        // The server now running was sent none of the documents, and reads
        // the files as they are now.
        self.forget_documents(language_id).await;
        self.disk_changes.lock().started(language_id);
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
}
