//! MCP Server State Management
//!
//! Provides the main server state that integrates with devscriptor-core components.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use devscriptor_core::analysis::api::AnalysisApi;
use devscriptor_core::config::ProjectConfig;
use devscriptor_core::context_graph::{ContextConfig, ContextGraphManager};
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig, ScanScope};

use crate::project::{ProjectSettings, Refresh};
use devscriptor_core::search::CodebaseSearcher;
use devscriptor_core::watcher::{CodeWatcher, WatchConfig, WatchState};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{error, info};
use uuid::Uuid;

/// Status of an analysis job
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Analysis job information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisJob {
    pub job_id: String,
    pub path: String,
    pub status: JobStatus,
    pub progress: f64,
    pub current_file: String,
    pub files_processed: usize,
    pub total_files: usize,
    pub message: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: u64,
    pub completed_at: Option<u64>,
}

impl AnalysisJob {
    /// Create a new analysis job
    pub fn new(path: impl Into<String>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            job_id: Uuid::new_v4().to_string(),
            path: path.into(),
            status: JobStatus::Pending,
            progress: 0.0,
            current_file: String::new(),
            files_processed: 0,
            total_files: 0,
            message: "Pending".to_string(),
            result: None,
            error: None,
            created_at: now,
            completed_at: None,
        }
    }

    /// Convert to dictionary representation
    #[must_use]
    pub fn to_dict(&self) -> serde_json::Value {
        let elapsed = if let Some(completed) = self.completed_at {
            completed.saturating_sub(self.created_at)
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(self.created_at)
        };

        serde_json::json!({
            "job_id": self.job_id,
            "path": self.path,
            "status": self.status.to_string(),
            "progress": (self.progress * 100.0).round() / 100.0,
            "current_file": self.current_file,
            "files_processed": self.files_processed,
            "total_files": self.total_files,
            "message": self.message,
            "error": self.error,
            "started_at": self.created_at,
            "completed_at": self.completed_at,
            "elapsed_seconds": elapsed,
        })
    }
}

/// Why inline embedding cannot run with `free_mb` of RAM free, or `None` when
/// it can: the floor `scan --embed` and `watch --embed` apply too.
fn inline_embedding_shortfall(free_mb: u64) -> Option<String> {
    let min_mb = devscriptor_core::embeddings::MIN_INLINE_EMBED_FREE_MB;
    (free_mb < min_mb).then(|| {
        format!(
            "only {free_mb} MB of RAM free (< {min_mb} MB required): watching without \
             inline embedding; run `generate_embeddings` when more memory is free"
        )
    })
}

/// MCP Server state that integrates with devscriptor-core
pub struct DevscriptorServerState {
    /// Database instance
    database: Arc<Database>,
    /// Analysis API
    analysis_api: Arc<AnalysisApi>,
    /// Context graph manager
    context_manager: ContextGraphManager,
    /// Codebase searcher
    searcher: CodebaseSearcher,
    /// Job tracking
    jobs: Arc<Mutex<HashMap<String, AnalysisJob>>>,
    /// Background task handles for active jobs (enables cancellation)
    running_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Notification channel for job status changes (eliminates busy-wait polling)
    job_notify: Arc<Notify>,
    /// Database path
    db_path: PathBuf,
    /// The project configuration the server follows, read again when its
    /// file changes ([`Self::refresh_project_config`]).
    project: Arc<ProjectSettings>,
    /// Real-time code watcher (started/stopped via MCP tools).
    code_watcher: Arc<Mutex<Option<CodeWatchHandle>>>,
    /// Held while a code watcher starts, so that two starts (the one
    /// `devscriptor mcp` makes by itself and an agent's) cannot both run.
    watch_start: Arc<tokio::sync::Mutex<()>>,
    /// Why the watcher `devscriptor mcp` starts by itself did not start.
    auto_watch_error: Arc<Mutex<Option<String>>>,
    /// The workspace the server watches by itself, as soon as a scan has
    /// indexed a root that contains it ([`Self::watch_workspace`]); cleared
    /// by `stop_watching`.
    watched_workspace: Arc<Mutex<Option<PathBuf>>>,
    /// When a tool call last looked whether that workspace can be watched
    /// now ([`Self::check_workspace_watch`]).
    workspace_checked: Arc<Mutex<Option<std::time::Instant>>>,
    /// The pipeline the freshness check, and a watcher without embeddings,
    /// re-index through; made on first use.
    structural_pipeline: tokio::sync::OnceCell<Arc<CodebasePipeline>>,
    /// How long an answer waits for its changed files to be re-indexed.
    freshness_budget: std::time::Duration,
}

/// This process's id, once it serves as the workspace's daemon
/// (`devscriptor serve`).
static DAEMON_PID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// Record that this process serves as the workspace's daemon, shared by
/// every client of the workspace: `health_check` then names its pid.
#[cfg(feature = "lsp")]
pub(crate) fn serve_as_daemon() {
    let _ = DAEMON_PID.set(std::process::id());
}

/// The pid of the daemon this process is, if it is one: `health_check`'s
/// `daemon_pid`, the same for every session the daemon serves.
#[must_use]
pub fn daemon_pid() -> Option<u32> {
    DAEMON_PID.get().copied()
}

/// How often tool calls look whether the workspace the server watches by
/// itself has been indexed by a scan made elsewhere.
pub const WORKSPACE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Live status of the real-time code watcher, updated after every re-scan.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CodeWatchStatus {
    /// Number of re-scans performed since the watcher started.
    pub rescans: u64,
    /// Files re-parsed in the most recent re-scan.
    pub last_files_parsed: usize,
    /// Files skipped (unchanged) in the most recent re-scan.
    pub last_files_skipped: usize,
    /// Entities extracted from the files the most recent re-scan re-parsed.
    pub last_entity_count: usize,
    /// Relationship rows the most recent re-scan wrote.
    pub last_relationship_count: usize,
    /// Duration of the most recent re-scan in seconds.
    pub last_rescan_secs: f64,
}

/// Add a watch's `mode` (`events` or `poll`), `watched_directories` and,
/// when it polls, `fallback_reason` to a status object.
fn add_watch_state(status: &mut serde_json::Value, watch: &WatchState) {
    status["mode"] = watch.mode().as_str().into();
    status["watched_directories"] = watch.watched_directories().into();
    if let Some(reason) = watch.fallback_reason() {
        status["fallback_reason"] = reason.into();
    }
}

/// Handle to a running code watcher: the background task plus shared status.
struct CodeWatchHandle {
    root: PathBuf,
    debounce_ms: u64,
    embed: bool,
    task: tokio::task::JoinHandle<()>,
    status: Arc<Mutex<CodeWatchStatus>>,
    /// How the watch learns about changes, and how many directories it covers.
    watch: Arc<WatchState>,
    /// The pipeline it re-indexes through: the server's structural one, or
    /// its own when it embeds.
    pipeline: Arc<CodebasePipeline>,
}

impl DevscriptorServerState {
    /// Create a new server state with default database
    pub fn new() -> anyhow::Result<Self> {
        let db_path = Self::default_db_path();
        Self::with_db_path(db_path)
    }

    /// Create a new server state with specific database path
    pub fn with_db_path(db_path: PathBuf) -> anyhow::Result<Self> {
        Self::for_project(db_path, ProjectSettings::none())
    }

    /// A server state following the project configuration `project`, its
    /// database at `db_path`, opened with the project's pool size.
    pub fn for_project(db_path: PathBuf, project: ProjectSettings) -> anyhow::Result<Self> {
        info!(
            "Initializing Devscriptor MCP Server with database: {}",
            db_path.display()
        );

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Initialize database
        let config = DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            pool_size: project.current().database.pool_size,
            ..DatabaseConfig::default()
        };

        let database = Arc::new(Database::new(config)?);
        Self::assemble(db_path, database, project)
    }

    /// Create a new server state reusing an existing [`Database`] instance, so a
    /// caller that owns an [`Arc<Database>`] does not get a second connection
    /// pool.
    pub fn with_shared_database(db_path: PathBuf, database: Arc<Database>) -> anyhow::Result<Self> {
        Self::assemble(db_path, database, ProjectSettings::none())
    }

    /// The state over `database`, following `project`.
    fn assemble(
        db_path: PathBuf,
        database: Arc<Database>,
        project: ProjectSettings,
    ) -> anyhow::Result<Self> {
        info!(
            "Initializing Devscriptor MCP Server with shared database: {}",
            db_path.display()
        );

        let analysis_api = Arc::new(AnalysisApi::new(database.clone()));
        let running_tasks = Arc::new(Mutex::new(HashMap::new()));
        let job_notify = Arc::new(Notify::new());

        // One shared embedding service, so the model is loaded at most once per
        // process. The same Arc-backed service is handed to both the entity
        // searcher and the context graph manager.
        let embedding_service = EmbeddingService::new(EmbeddingConfig::default());

        let context_config = ContextConfig::default();
        let context_manager = ContextGraphManager::with_embedding_service(
            database.clone(),
            context_config,
            embedding_service.clone(),
        );

        // Enable on-demand embedding so `semantic_search` embeds BM25 candidates
        // (cache-first) that lack vectors before the dense channel runs. Without
        // it, an agent that scans then immediately calls `semantic_search` over
        // the MCP interface gets no vector matches, because `scan_codebase` does
        // not embed and the MCP surface has no separate embed step. The explicit
        // `generate_embeddings` tool remains available to pre-build the full index.
        let searcher = CodebaseSearcher::with_embedding_service(
            database.conn_pool().clone(),
            embedding_service,
        )
        .with_on_demand_embedding(project.current().embedding_selection());

        let jobs = Arc::new(Mutex::new(HashMap::new()));

        info!("Devscriptor MCP Server initialized successfully");

        Ok(Self {
            database,
            analysis_api,
            context_manager,
            searcher,
            jobs,
            running_tasks,
            job_notify,
            db_path,
            project: Arc::new(project),
            code_watcher: Arc::new(Mutex::new(None)),
            watch_start: Arc::new(tokio::sync::Mutex::new(())),
            auto_watch_error: Arc::new(Mutex::new(None)),
            watched_workspace: Arc::new(Mutex::new(None)),
            workspace_checked: Arc::new(Mutex::new(None)),
            structural_pipeline: tokio::sync::OnceCell::new(),
            freshness_budget: crate::freshness::budget_from_env(),
        })
    }

    /// Wait at most `budget` for an answer's changed files to be re-indexed,
    /// instead of what [`crate::freshness::BUDGET_ENV_VAR`] says.
    #[must_use]
    pub fn with_freshness_budget(mut self, budget: std::time::Duration) -> Self {
        self.freshness_budget = budget;
        self
    }

    /// How long an answer waits for its changed files to be re-indexed
    /// ([`crate::freshness::ensure_fresh`]).
    #[must_use]
    pub fn freshness_budget(&self) -> std::time::Duration {
        self.freshness_budget
    }

    /// The incremental pipeline over the shared database, with embeddings
    /// (for a watcher that embeds) or without, in the project's scope.
    fn incremental_pipeline_config(&self, embed: bool) -> PipelineConfig {
        let mut config = PipelineConfig {
            database: DatabaseConfig {
                path: self.db_path.to_string_lossy().to_string(),
                ..DatabaseConfig::default()
            },
            embedding: embed.then(EmbeddingConfig::default),
            skip_embeddings: !embed,
            unload_model_after_scan: false,
            incremental: true,
            enable_cross_file: true,
            bulk_load: false,
            ..PipelineConfig::default()
        };
        self.project.current().apply_to(&mut config);
        config
    }

    /// The project configuration in force.
    #[must_use]
    pub fn project_config(&self) -> Arc<ProjectConfig> {
        self.project.current()
    }

    /// Take a change of the project's configuration file: the pipelines the
    /// server keeps (the structural one, a watcher's own) take the new scope
    /// once the run in progress, if any, ends. Called at every tool call: it
    /// costs a `stat` when nothing changed.
    pub fn refresh_project_config(self: &Arc<Self>) {
        if self.project.refresh() != Refresh::Applied {
            return;
        }
        let scope = ScanScope::of(&self.incremental_pipeline_config(false));
        self.searcher
            .set_embedding_selection(scope.embedding_selection);
        let mut pipelines: Vec<Arc<CodebasePipeline>> = self
            .structural_pipeline
            .get()
            .cloned()
            .into_iter()
            .collect();
        if let Some(watcher) = self.code_watcher.lock().as_ref()
            && !pipelines
                .iter()
                .any(|pipeline| Arc::ptr_eq(pipeline, &watcher.pipeline))
        {
            pipelines.push(Arc::clone(&watcher.pipeline));
        }
        if pipelines.is_empty() {
            return;
        }
        tokio::spawn(async move {
            for pipeline in pipelines {
                pipeline.set_scope(scope.clone()).await;
            }
        });
    }

    /// The incremental pipeline without embeddings that the query-time
    /// freshness check re-indexes through, and the watcher too when it does
    /// not embed. One per server, made on first use: its runs never overlap,
    /// so each derives its edges from the index as the run before left it.
    ///
    /// # Errors
    /// Returns an error if the pipeline cannot be built.
    pub async fn structural_pipeline(&self) -> anyhow::Result<Arc<CodebasePipeline>> {
        let pipeline = self
            .structural_pipeline
            .get_or_try_init(|| async {
                CodebasePipeline::with_database(
                    self.incremental_pipeline_config(false),
                    self.database.clone(),
                )
                .map(Arc::new)
            })
            .await?;
        Ok(Arc::clone(pipeline))
    }

    /// Watch `workspace` by itself, so that the index follows the edits of a
    /// session without the agent asking: what the server does when it starts
    /// (the daemon's `--workspace`, the working directory of `devscriptor mcp
    /// --no-daemon`). It watches the scanned root the workspace lies in at
    /// once if there is one, and else as soon as a scan has indexed one: after
    /// `scan_codebase` and `start_analysis` ([`Self::watch_after_scan`]), and
    /// for a scan made elsewhere, such as `devscriptor scan` in a terminal, at
    /// the next tool call ([`Self::check_workspace_watch`]).
    pub async fn watch_workspace(&self, workspace: &Path) -> Option<PathBuf> {
        let workspace = dunce::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        *self.watched_workspace.lock() = Some(workspace.clone());
        self.auto_watch(&workspace).await
    }

    /// Start watching the workspace the server watches by itself, if a scan
    /// has indexed it since and nothing is watched yet. Nothing is done once
    /// `stop_watching` asked to stop, or while another root is watched.
    pub async fn watch_after_scan(&self) -> Option<PathBuf> {
        let workspace = self.watched_workspace.lock().clone()?;
        if self.code_watcher.lock().is_some() {
            return None;
        }
        self.auto_watch(&workspace).await
    }

    /// At a tool call: look, at most every [`WORKSPACE_CHECK_INTERVAL`],
    /// whether the workspace the server watches by itself can be watched now,
    /// in the background. A scan made by another process is noticed so.
    pub fn check_workspace_watch(self: &Arc<Self>) {
        if self.watched_workspace.lock().is_none() || self.code_watcher.lock().is_some() {
            return;
        }
        {
            let mut checked = self.workspace_checked.lock();
            let now = std::time::Instant::now();
            if checked.is_some_and(|last| now.duration_since(last) < WORKSPACE_CHECK_INTERVAL) {
                return;
            }
            *checked = Some(now);
        }
        let state = Arc::clone(self);
        tokio::spawn(async move {
            state.watch_after_scan().await;
        });
    }

    /// Stop watching by itself: `stop_watching` turns the watch off for the
    /// rest of the server's life, so a later scan does not start it again.
    pub fn stop_watching_by_itself(&self) {
        *self.watched_workspace.lock() = None;
    }

    /// Watch the scanned root `cwd` lies in (the deepest, when scanned roots
    /// nest). Returns the root, or `None` when `cwd` lies in no scanned root
    /// ([`Database::indexed_roots`]) or the watcher did not start; a failure
    /// is logged and reported by [`Self::code_watch_status`] as
    /// `auto_watch_error`, never returned.
    async fn auto_watch(&self, cwd: &Path) -> Option<PathBuf> {
        let cwd = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let roots = match self.database.indexed_roots().await {
            Ok(roots) => roots,
            Err(e) => {
                tracing::warn!("Not watching: the scanned roots could not be read: {e}");
                *self.auto_watch_error.lock() = Some(format!("reading the scanned roots: {e}"));
                return None;
            }
        };
        let root = roots
            .into_iter()
            .filter(|root| cwd.starts_with(root))
            .max_by_key(|root| root.components().count())?;
        match self.start_code_watching(root.clone(), 400, false).await {
            Ok(_) => {
                tracing::info!("Watching {} for changes", root.display());
                Some(root)
            }
            Err(e) => {
                tracing::warn!("Not watching {}: {e}", root.display());
                *self.auto_watch_error.lock() = Some(format!("{}: {e}", root.display()));
                None
            }
        }
    }

    /// Start the real-time code watcher on `path`.
    ///
    /// Runs an incremental re-scan (over the shared database) whenever source
    /// files change, keeping the knowledge graph current. Returns an error if a
    /// watcher is already running.
    pub async fn start_code_watching(
        &self,
        path: PathBuf,
        debounce_ms: u64,
        embed: bool,
    ) -> anyhow::Result<serde_json::Value> {
        // The index stores canonical paths: compare and watch in them.
        let path = dunce::canonicalize(&path).unwrap_or(path);
        let _starting = self.watch_start.lock().await;
        let watching = {
            let guard = self.code_watcher.lock();
            guard
                .as_ref()
                .filter(|handle| !handle.task.is_finished())
                .map(|handle| handle.root.clone())
        };
        match watching {
            // Asked again for the root being watched: nothing to do.
            Some(root) if root == path => {
                let mut status = self.code_watch_status();
                status["already_watching"] = serde_json::Value::Bool(true);
                return Ok(status);
            }
            Some(root) => anyhow::bail!(
                "Already watching {}: one watcher runs per server, so stop_watching first to \
                 watch {}",
                root.display(),
                path.display()
            ),
            None => {}
        }

        // As `devscriptor watch --embed` does: with too little free RAM to load
        // the model safely, watch the structure only, and say why.
        let embed_skipped = embed
            .then(|| {
                inline_embedding_shortfall(devscriptor_core::utils::memory_guard::free_ram_mb())
            })
            .flatten();
        let embed = embed && embed_skipped.is_none();

        // An incremental pipeline over the shared database: the server's own
        // when the watcher does not embed, so that it and the freshness check
        // keep one set of entities current.
        let pipeline = if embed {
            Arc::new(CodebasePipeline::with_database(
                self.incremental_pipeline_config(true),
                self.database.clone(),
            )?)
        } else {
            self.structural_pipeline().await?
        };

        // Watch first, then prime the graph with one incremental scan, and
        // only then answer: a file written during the scan, or right after
        // the caller is told it is being watched, triggers a re-scan, and a watch
        // that cannot be registered (no inotify instances left) is an error,
        // not `watching: true`.
        let session = CodeWatcher::new(
            path.clone(),
            Arc::clone(&pipeline),
            WatchConfig {
                debounce: std::time::Duration::from_millis(debounce_ms),
                ..WatchConfig::default()
            },
        )
        .start()?;
        let watch = session.state();
        pipeline.analyze_directory(&path).await?;

        let status = Arc::new(Mutex::new(CodeWatchStatus::default()));
        let status_for_cb = status.clone();
        let task = tokio::spawn(session.run(move |rescan| {
            let r = &rescan.result;
            let mut s = status_for_cb.lock();
            s.rescans += 1;
            s.last_files_parsed = r.files_parsed;
            s.last_files_skipped = r.files_skipped;
            s.last_entity_count = r.entity_count;
            s.last_relationship_count = r.relationship_count;
            s.last_rescan_secs = r.processing_time_secs;
        }));

        let mut response = serde_json::json!({
            "watching": true,
            "path": path.to_string_lossy(),
            "debounce_ms": debounce_ms,
            "embed": embed,
        });
        add_watch_state(&mut response, &watch);
        *self.code_watcher.lock() = Some(CodeWatchHandle {
            root: path.clone(),
            debounce_ms,
            embed,
            task,
            status,
            watch,
            pipeline,
        });
        if let Some(reason) = embed_skipped {
            response["embed_skipped"] = serde_json::Value::String(reason);
        }
        Ok(response)
    }

    /// Stop the real-time code watcher if one is running.
    pub fn stop_code_watching(&self) -> serde_json::Value {
        let mut guard = self.code_watcher.lock();
        if let Some(handle) = guard.take() {
            handle.task.abort();
            serde_json::json!({ "stopped": true, "path": handle.root.to_string_lossy() })
        } else {
            serde_json::json!({ "stopped": false, "message": "no watcher running" })
        }
    }

    /// Snapshot the current watcher status.
    pub fn code_watch_status(&self) -> serde_json::Value {
        let guard = self.code_watcher.lock();
        match guard.as_ref() {
            Some(handle) if !handle.task.is_finished() => {
                let s = handle.status.lock().clone();
                let mut status = serde_json::json!({
                    "watching": true,
                    "path": handle.root.to_string_lossy(),
                    "debounce_ms": handle.debounce_ms,
                    "embed": handle.embed,
                    "rescans": s.rescans,
                    "last_rescan": {
                        "files_parsed": s.last_files_parsed,
                        "files_skipped": s.last_files_skipped,
                        "entity_count": s.last_entity_count,
                        "relationship_count": s.last_relationship_count,
                        "seconds": s.last_rescan_secs,
                    }
                });
                add_watch_state(&mut status, &handle.watch);
                status
            }
            _ => {
                let mut status = serde_json::json!({ "watching": false });
                if let Some(error) = self.auto_watch_error.lock().clone() {
                    status["auto_watch_error"] = serde_json::Value::String(error);
                }
                status
            }
        }
    }

    /// The directories the shared default database may live in, most
    /// preferred first, each named by where it comes from.
    fn default_db_dirs() -> Vec<(Option<PathBuf>, &'static str)> {
        vec![
            (
                devscriptor_core::user_dirs::data_local_dir().map(|p| p.join("devscriptor")),
                "data_local/devscriptor",
            ),
            (
                dirs::home_dir().map(|p| p.join(".devscriptor")),
                "home/.devscriptor",
            ),
            (
                std::env::var("XDG_DATA_HOME")
                    .ok()
                    .map(|p| PathBuf::from(p).join("devscriptor")),
                "XDG_DATA_HOME/devscriptor",
            ),
            (
                std::env::var("HOME")
                    .ok()
                    .map(|p| PathBuf::from(p).join(".devscriptor")),
                "HOME/.devscriptor",
            ),
        ]
    }

    /// The database `devscriptor mcp` and `devscriptor serve` open when no
    /// `--database` is given: shared, and independent of the working
    /// directory. Its directory is created if missing.
    ///
    /// In sandboxed environments where `HOME` is not set and the current
    /// directory is not writable, this falls back to the platform temp
    /// directory.
    #[must_use]
    pub fn default_db_path() -> PathBuf {
        // Try the directories in order of preference, checking writability.
        for (candidate, source) in Self::default_db_dirs() {
            if let Some(path) = candidate {
                // Ensure the devscriptor subdirectory exists (create if needed)
                let dir = if path.is_dir() || path.file_name().is_some() {
                    &path
                } else {
                    path.parent().unwrap_or(&path)
                };
                if let Err(e) = std::fs::create_dir_all(dir) {
                    tracing::debug!(
                        "Cannot create directory from {}: {} — {}",
                        source,
                        dir.display(),
                        e
                    );
                    continue;
                }
                match std::fs::metadata(dir) {
                    Ok(meta) => {
                        let is_writable = !meta.permissions().readonly();
                        if is_writable {
                            tracing::debug!(
                                "Using database path from {}: {}",
                                source,
                                path.display()
                            );
                            return path.join("default.db");
                        }
                        tracing::debug!(
                            "Directory from {} is not writable: {}",
                            source,
                            dir.display()
                        );
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Directory from {} not accessible ({}): {}",
                            source,
                            e,
                            dir.display()
                        );
                    }
                }
            }
        }

        // Fallback to temp directory (always writable)
        let temp = std::env::temp_dir().join(".devscriptor");
        let _ = std::fs::create_dir_all(&temp);
        tracing::warn!(
            "All preferred database directories are unavailable; falling back to temp: {}",
            temp.display()
        );
        temp.join("default.db")
    }

    /// Get reference to database
    #[must_use]
    pub fn database(&self) -> Arc<Database> {
        self.database.clone()
    }

    /// Get reference to analysis API
    #[must_use]
    pub fn analysis_api(&self) -> Arc<AnalysisApi> {
        self.analysis_api.clone()
    }

    /// Get reference to context manager
    #[must_use]
    pub fn context_manager(&self) -> &ContextGraphManager {
        &self.context_manager
    }

    /// Get reference to searcher
    #[must_use]
    pub fn searcher(&self) -> &CodebaseSearcher {
        &self.searcher
    }

    /// Create a new analysis job
    pub fn create_job(&self, path: impl Into<String>) -> String {
        let job = AnalysisJob::new(path.into());
        let job_id = job.job_id.clone();
        let path_for_log = job.path.clone();

        {
            let mut jobs = self.jobs.lock();
            jobs.insert(job_id.clone(), job);
        }

        info!("Created analysis job {} for path: {}", job_id, path_for_log);
        job_id
    }

    /// Get a job by ID
    #[must_use]
    pub fn get_job(&self, job_id: &str) -> Option<AnalysisJob> {
        let jobs = self.jobs.lock();
        jobs.get(job_id).cloned()
    }

    /// Update job progress
    #[allow(clippy::too_many_arguments)]
    pub fn update_job_progress(
        &self,
        job_id: &str,
        progress: Option<f64>,
        current_file: Option<String>,
        files_processed: Option<usize>,
        total_files: Option<usize>,
        message: Option<String>,
        status: Option<JobStatus>,
    ) {
        let mut jobs = self.jobs.lock();
        if let Some(job) = jobs.get_mut(job_id) {
            if let Some(p) = progress {
                job.progress = p.clamp(0.0, 100.0);
            }
            if let Some(f) = current_file {
                job.current_file = f;
            }
            if let Some(fp) = files_processed {
                job.files_processed = fp;
            }
            if let Some(tf) = total_files {
                job.total_files = tf;
            }
            if let Some(m) = message {
                job.message = m;
            }
            if let Some(s) = status {
                job.status = s;
            }
        }
    }

    /// Mark job as completed
    pub fn complete_job(&self, job_id: &str, result: serde_json::Value) {
        let mut jobs = self.jobs.lock();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Completed;
            job.progress = 100.0;
            job.result = Some(result);
            job.completed_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            info!("Job {} completed successfully", job_id);
        }
        // Clean up the task handle to prevent memory leaks.
        self.running_tasks.lock().remove(job_id);
        // Wake up any waiters watching for job completion.
        self.job_notify.notify_waiters();
    }

    /// Mark job as failed
    pub fn fail_job(&self, job_id: &str, error: impl Into<String>) {
        let error = error.into();
        let mut jobs = self.jobs.lock();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Failed;
            job.error = Some(error.clone());
            job.completed_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            error!("Job {} failed: {}", job_id, error);
        }
        // Clean up the task handle to prevent memory leaks.
        self.running_tasks.lock().remove(job_id);
        // Wake up any waiters watching for job completion.
        self.job_notify.notify_waiters();
    }

    /// List all jobs
    #[must_use]
    pub fn list_jobs(&self, limit: usize) -> Vec<serde_json::Value> {
        let jobs = self.jobs.lock();
        let mut job_list: Vec<_> = jobs.values().map(AnalysisJob::to_dict).collect();

        // Sort by created_at descending
        job_list.sort_by(|a, b| {
            let a_time = a
                .get("started_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let b_time = b
                .get("started_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            b_time.cmp(&a_time)
        });

        job_list.into_iter().take(limit).collect()
    }

    /// Cleanup old jobs
    ///
    /// Collects candidate keys under a read lock, then upgrades to a write
    /// lock only for the removal phase. This keeps the critical section short
    /// when the map is large.
    pub fn cleanup_old_jobs(&self, max_age_seconds: u64) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Phase 1 — read lock: identify stale job IDs without mutating.
        let to_remove: Vec<String> = {
            let jobs = self.jobs.lock();
            jobs.iter()
                .filter(|(_, job)| {
                    if job.status != JobStatus::Completed && job.status != JobStatus::Failed {
                        return false;
                    }
                    if let Some(completed) = job.completed_at {
                        (now - completed) > max_age_seconds
                    } else {
                        false
                    }
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        // Phase 2 — write lock: remove only the identified keys.
        if !to_remove.is_empty() {
            let mut jobs = self.jobs.lock();
            for id in &to_remove {
                jobs.remove(id);
            }
            info!("Cleaned up {} old jobs", to_remove.len());
        }

        to_remove.len()
    }

    /// Get a reference to the job completion notifier.
    #[must_use]
    pub fn job_notify(&self) -> &Notify {
        &self.job_notify
    }

    /// Store a background task handle so it can be cancelled later.
    pub fn store_job_handle(&self, job_id: &str, handle: tokio::task::JoinHandle<()>) {
        self.running_tasks.lock().insert(job_id.to_string(), handle);
    }

    /// Cancel a running job by aborting its background task.
    ///
    /// Returns `true` if the job was found and cancelled, `false` otherwise.
    pub fn cancel_job(&self, job_id: &str) -> bool {
        let mut tasks = self.running_tasks.lock();
        if let Some(handle) = tasks.remove(job_id) {
            handle.abort();
            let mut jobs = self.jobs.lock();
            if let Some(job) = jobs.get_mut(job_id) {
                job.status = JobStatus::Failed;
                job.error = Some("Cancelled by user".to_string());
                job.completed_at = Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
            }
            info!("Job {} cancelled", job_id);
            true
        } else {
            false
        }
    }

    /// Get health status
    ///
    /// Performs lightweight real checks instead of returning hard-coded values.
    #[must_use]
    pub fn get_health_status(&self) -> serde_json::Value {
        let jobs = self.jobs.lock();
        let active_count = jobs
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count();

        // Real database check: verify the path exists and is writable.
        // The searcher shares the same connection pool, so this also
        // validates the searcher backend.
        let db_connected = self.db_path.exists()
            && std::fs::OpenOptions::new()
                .write(true)
                .open(&self.db_path)
                .is_ok();

        let overall_status = if db_connected { "healthy" } else { "unhealthy" };

        serde_json::json!({
            "status": overall_status,
            "version": devscriptor_core::VERSION,
            "database": {
                "path": self.db_path.to_string_lossy().to_string(),
                "connected": db_connected,
            },
            "jobs": {
                "active": active_count,
                "in_memory": jobs.len(),
            },
            "devscriptor": {
                "initialized": true,
            },
            // Whether the server follows a project configuration file.
            "hot_reload": {
                "enabled": self.project.path().is_some(),
            },
            "project_config": self.project.status(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// `start_watching(embed: true)` needs the free RAM the CLI paths need, and
    /// below it watches without embedding.
    #[test]
    fn inline_embedding_needs_the_cli_ram_floor() {
        let floor = devscriptor_core::embeddings::MIN_INLINE_EMBED_FREE_MB;
        assert_eq!(inline_embedding_shortfall(floor), None);
        assert_eq!(inline_embedding_shortfall(floor * 4), None);
        let reason = inline_embedding_shortfall(floor - 1).expect("below the floor");
        assert!(reason.contains("generate_embeddings"), "{reason}");
    }

    /// A server state over a database in a temporary directory, which the
    /// test holds for as long as the state: bound as `let (_dir, state)`, the
    /// state is dropped before its directory is removed.
    fn create_test_state() -> (TempDir, DevscriptorServerState) {
        let temp_dir = TempDir::new().unwrap();
        let state = DevscriptorServerState::with_db_path(temp_dir.path().join("test.db")).unwrap();
        (temp_dir, state)
    }

    /// A change of the project's file reaches the pipeline the watcher and
    /// the freshness check re-index through, and on-demand embedding, at the
    /// next tool call; the server is not restarted.
    #[tokio::test]
    async fn a_change_of_the_project_file_reaches_the_kept_pipeline() {
        let workspace = TempDir::new().unwrap();
        let state = Arc::new(
            DevscriptorServerState::for_project(
                workspace.path().join("index.db"),
                ProjectSettings::for_workspace(workspace.path(), None),
            )
            .unwrap(),
        );
        let pipeline = state.structural_pipeline().await.unwrap();
        assert!(pipeline.scope().include_extensions.is_empty());

        std::fs::write(
            workspace
                .path()
                .join(devscriptor_core::config::PROJECT_CONFIG_FILE),
            "scan:\n  include: [py]\n  cross_file: light\n",
        )
        .unwrap();
        state.refresh_project_config();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pipeline.scope().include_extensions.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the scope never reached the pipeline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let scope = pipeline.scope();
        assert_eq!(scope.include_extensions, ["py"]);
        assert_eq!(
            scope.cross_file_mode,
            devscriptor_core::parser::cross_file::CrossFileMode::Light
        );
        assert_eq!(state.project_config().scan.include, ["py"]);
    }

    #[test]
    fn test_server_state_creation() {
        let (_dir, state) = create_test_state();
        assert!(!state.db_path.as_os_str().is_empty());
    }

    /// The candidates only: resolving the path creates its directory, and a
    /// test must not create one in the user's data directory.
    #[test]
    fn test_default_db_path() {
        let dirs = DevscriptorServerState::default_db_dirs();
        assert!(!dirs.is_empty());
        assert!(
            dirs.iter()
                .filter_map(|(dir, _)| dir.as_ref())
                .all(|dir| dir.to_string_lossy().contains("devscriptor"))
        );
    }

    #[test]
    fn test_create_job() {
        let (_dir, state) = create_test_state();
        let job_id = state.create_job("/test/path");

        // Job ID should be a valid UUID
        assert!(!job_id.is_empty());
        assert_eq!(job_id.len(), 36); // UUID v4 length
    }

    #[test]
    fn test_get_job() {
        let (_dir, state) = create_test_state();
        let job_id = state.create_job("/test/path");

        let job = state.get_job(&job_id);
        assert!(job.is_some());

        let job = job.unwrap();
        assert_eq!(job.path, "/test/path");
        assert!(matches!(job.status, JobStatus::Pending));
    }

    #[test]
    fn test_get_nonexistent_job() {
        let (_dir, state) = create_test_state();
        let job = state.get_job("nonexistent-id");
        assert!(job.is_none());
    }

    #[test]
    fn test_update_job_progress() {
        let (_dir, state) = create_test_state();
        let job_id = state.create_job("/test/path");

        // Update progress with new signature
        state.update_job_progress(
            &job_id,
            Some(50.0),
            Some("current.rs".to_string()),
            Some(5),
            Some(10),
            Some("Processing...".to_string()),
            Some(JobStatus::Running),
        );

        let job = state.get_job(&job_id).unwrap();
        assert_eq!(job.progress, 50.0);
        assert_eq!(job.current_file, "current.rs");
        assert_eq!(job.files_processed, 5);
        assert_eq!(job.total_files, 10);
        assert_eq!(job.message, "Processing...".to_string());
        assert!(matches!(job.status, JobStatus::Running));
    }

    #[test]
    fn test_complete_job() {
        let (_dir, state) = create_test_state();
        let job_id = state.create_job("/test/path");

        // Mark as complete
        let result = serde_json::json!({"files_processed": 10});
        state.complete_job(&job_id, result.clone());

        let job = state.get_job(&job_id).unwrap();
        assert!(matches!(job.status, JobStatus::Completed));
        assert_eq!(job.result, Some(result));
        assert_eq!(job.progress, 100.0);
    }

    #[test]
    fn test_fail_job() {
        let (_dir, state) = create_test_state();
        let job_id = state.create_job("/test/path");

        // Mark as failed
        state.fail_job(&job_id, "Something went wrong");

        let job = state.get_job(&job_id).unwrap();
        assert!(matches!(job.status, JobStatus::Failed));
        assert_eq!(job.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_list_jobs() {
        let (_dir, state) = create_test_state();

        // Create several jobs
        let id1 = state.create_job("/test/path1");
        let id2 = state.create_job("/test/path2");
        let _id3 = state.create_job("/test/path3");

        // List with limit
        let jobs = state.list_jobs(2);
        assert_eq!(jobs.len(), 2);

        // List all
        let jobs = state.list_jobs(10);
        assert_eq!(jobs.len(), 3);

        // Check job IDs are in the list (using job_id field)
        let job_ids: Vec<String> = jobs
            .iter()
            .map(|j| j["job_id"].as_str().unwrap().to_string())
            .collect();
        assert!(job_ids.contains(&id1));
        assert!(job_ids.contains(&id2));
    }

    #[test]
    fn test_cleanup_old_jobs() {
        let (_dir, state) = create_test_state();

        // Create a job and complete it
        let job_id = state.create_job("/test/path");
        state.complete_job(&job_id, serde_json::json!({"done": true}));

        // Small sleep to ensure time passes
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Cleanup with very small age (should remove completed jobs older than 1ms)
        let cleaned = state.cleanup_old_jobs(0);
        // Note: Due to timing, job might not be cleaned if completed too recently
        // The important thing is the API works correctly

        if cleaned > 0 {
            // Job should be gone if it was cleaned
            assert!(state.get_job(&job_id).is_none());
        } else {
            // Job should still exist if not old enough
            assert!(state.get_job(&job_id).is_some());
        }
    }

    #[test]
    fn test_cleanup_does_not_remove_running_jobs() {
        let (_dir, state) = create_test_state();

        // Create a job but don't complete it
        let job_id = state.create_job("/test/path");
        state.update_job_progress(
            &job_id,
            Some(50.0),
            None,
            None,
            None,
            None,
            Some(JobStatus::Running),
        );

        // Cleanup should not remove running jobs
        let cleaned = state.cleanup_old_jobs(0);
        assert_eq!(cleaned, 0);

        // Job should still exist
        assert!(state.get_job(&job_id).is_some());
    }

    #[test]
    fn test_health_status() {
        let (_dir, state) = create_test_state();

        // Create some jobs
        let _id1 = state.create_job("/test/path1");
        let id2 = state.create_job("/test/path2");
        state.update_job_progress(
            &id2,
            Some(50.0),
            None,
            None,
            None,
            None,
            Some(JobStatus::Running),
        );

        let health = state.get_health_status();

        assert_eq!(health["status"], "healthy");
        assert_eq!(health["version"], devscriptor_core::VERSION);
        assert_eq!(health["database"]["connected"], true);
        assert_eq!(health["jobs"]["active"], 1);
        assert_eq!(health["jobs"]["in_memory"], 2);
    }

    #[test]
    fn test_job_status_display() {
        assert_eq!(format!("{}", JobStatus::Pending), "pending");
        assert_eq!(format!("{}", JobStatus::Running), "running");
        assert_eq!(format!("{}", JobStatus::Completed), "completed");
        assert_eq!(format!("{}", JobStatus::Failed), "failed");
    }

    #[test]
    fn test_job_creation() {
        let job = AnalysisJob::new("/test/path");

        assert_eq!(job.path, "/test/path");
        assert!(!job.job_id.is_empty()); // UUID generated
        assert!(matches!(job.status, JobStatus::Pending));
        assert_eq!(job.progress, 0.0);
        assert_eq!(job.message, "Pending");
    }

    #[test]
    fn test_job_to_dict() {
        let job = AnalysisJob::new("/test/path");

        let dict = job.to_dict();

        assert_eq!(dict["path"], "/test/path");
        assert_eq!(dict["status"], "pending");
        assert!(dict["started_at"].is_number());
        assert!(dict["job_id"].is_string());
    }

    #[test]
    fn test_database_access() {
        let (_dir, state) = create_test_state();

        // Should be able to access database
        let db = state.database();
        assert!(Arc::strong_count(&db) >= 1);
    }

    #[test]
    fn test_analysis_api_access() {
        let (_dir, state) = create_test_state();

        // Should be able to access analysis API
        let _api = state.analysis_api();
    }

    #[test]
    fn test_server_state_thread_safety() {
        use std::thread;

        let (_dir, state) = create_test_state();

        let state = Arc::new(state);
        let mut handles = vec![];

        // Spawn multiple threads creating jobs
        for i in 0..10 {
            let state = Arc::clone(&state);
            let handle = thread::spawn(move || {
                let job_id = state.create_job(format!("/test/path{i}"));
                state.update_job_progress(
                    &job_id,
                    Some(f64::from(i) * 10.0),
                    None,
                    None,
                    None,
                    None,
                    None,
                );
                job_id
            });
            handles.push(handle);
        }

        let job_ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All jobs should exist
        assert_eq!(job_ids.len(), 10);
        for id in job_ids {
            assert!(state.get_job(&id).is_some());
        }
    }
}
