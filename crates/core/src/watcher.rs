//! Real-time code watcher.
//!
//! [`CodeWatcher`] watches a project directory and keeps the code knowledge
//! graph current as source files change. It watches each directory a scan of
//! the project descends into, one non-recursive watch apiece, so ignored trees
//! such as `target/` and `node_modules/` cost no watches. The source files that
//! change during a debounce window (default 400 ms) are re-indexed together
//! with [`CodebasePipeline::reindex_paths`]:
//!
//! - changed files are re-parsed with re-index semantics,
//! - deleted files, and the files of deleted directories, are removed, and
//! - new files are added; a new directory is watched too, and the files
//!   already in it are indexed.
//!
//! When the operating system drops events (an overflow) or the watcher reports
//! an error, the session runs a full incremental
//! [`CodebasePipeline::analyze_directory`] instead. Where native events cannot
//! be had (the watcher cannot be created, or the system's limits on watches or
//! watcher instances are reached) it polls ([`WatchMode::Poll`]).
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::path::PathBuf;
//! use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
//! use devscriptor_core::watcher::{CodeWatcher, WatchConfig};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let mut config = PipelineConfig::default();
//! config.incremental = true;
//! config.skip_embeddings = true;
//! let pipeline = Arc::new(CodebasePipeline::new(config)?);
//!
//! let watcher = CodeWatcher::new(PathBuf::from("."), pipeline, WatchConfig::default());
//! watcher.run(|rescan| {
//!     println!("re-scanned: {} entities", rescan.result.entity_count);
//! }).await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::error::{Error, Result};
use crate::parser::language_config::{extension_to_language, filename_to_language};
use crate::pipeline::{CodebasePipeline, PipelineResult};

/// Directory names whose contents are never watched and never trigger a
/// re-index (VCS, build output, virtualenvs, editor metadata), wherever they
/// occur below the watched root.
const IGNORED_DIR_COMPONENTS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".idea",
    ".cache",
];

/// Configuration for a [`CodeWatcher`].
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Quiet period after the last filesystem event before a re-index runs.
    /// Coalesces bursts of edits into a single one.
    pub debounce: Duration,
    /// Poll for changes even where native events are available: for file
    /// systems whose events cannot be trusted, and for tests.
    pub force_poll: bool,
    /// How often a polling watch looks for changes.
    pub poll_interval: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(400),
            force_poll: false,
            poll_interval: Duration::from_secs(2),
        }
    }
}

/// Result of a single re-index triggered by the watcher.
#[derive(Debug, Clone)]
pub struct WatchRescan {
    /// The pipeline result of the re-index.
    pub result: PipelineResult,
}

/// How a watch learns about changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    /// Native file-system events (inotify, FSEvents, ReadDirectoryChangesW).
    Events,
    /// Polling the watched directories every [`WatchConfig::poll_interval`].
    Poll,
}

impl WatchMode {
    /// `events` or `poll`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Poll => "poll",
        }
    }
}

/// What a running watch covers, readable while it runs.
#[derive(Debug)]
pub struct WatchState {
    /// The mode, and why it is [`WatchMode::Poll`] when it is.
    mode: parking_lot::Mutex<(WatchMode, Option<String>)>,
    watched_directories: AtomicUsize,
}

impl WatchState {
    fn new() -> Self {
        Self {
            mode: parking_lot::Mutex::new((WatchMode::Events, None)),
            watched_directories: AtomicUsize::new(0),
        }
    }

    fn poll(&self, reason: String) {
        *self.mode.lock() = (WatchMode::Poll, Some(reason));
    }

    /// How the watch learns about changes.
    #[must_use]
    pub fn mode(&self) -> WatchMode {
        self.mode.lock().0
    }

    /// Why the watch polls, when it does.
    #[must_use]
    pub fn fallback_reason(&self) -> Option<String> {
        self.mode.lock().1.clone()
    }

    /// How many directories are watched.
    #[must_use]
    pub fn watched_directories(&self) -> usize {
        self.watched_directories.load(Ordering::Relaxed)
    }
}

/// Watches a project directory and keeps the code graph current in real time.
pub struct CodeWatcher {
    root: PathBuf,
    pipeline: Arc<CodebasePipeline>,
    config: WatchConfig,
}

impl CodeWatcher {
    /// Create a new watcher for `root`, driving the given (incremental)
    /// pipeline. The root is canonicalized when it can be, as the pipeline
    /// stores its paths.
    #[must_use]
    pub fn new(root: PathBuf, pipeline: Arc<CodebasePipeline>, config: WatchConfig) -> Self {
        Self {
            root: dunce::canonicalize(&root).unwrap_or(root),
            pipeline,
            config,
        }
    }

    /// The directory being watched.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Start watching and run the re-index loop until the returned future is
    /// dropped: [`start`](Self::start), then [`WatchSession::run`].
    ///
    /// # Errors
    /// See [`Self::start`].
    pub async fn run<F>(&self, on_rescan: F) -> Result<()>
    where
        F: FnMut(WatchRescan) + Send,
    {
        self.start()?.run(on_rescan).await;
        Ok(())
    }

    /// Register the watches and return the session that re-indexes on change.
    ///
    /// Every change made after this returns reaches the session, even before
    /// [`WatchSession::run`] is polled. So start the watch *before* priming
    /// the graph with a scan: a file written while that scan runs, or just
    /// after the caller reports that it is watching, then triggers a re-index
    /// instead of staying out of the graph until an unrelated change.
    ///
    /// Each directory a scan of the root descends into gets a non-recursive
    /// watch. When native events are unavailable or a watch hits the system's
    /// limits, the session polls instead ([`WatchState::mode`]).
    ///
    /// # Errors
    /// Returns an error when the root cannot be scanned or watched, by events
    /// or by polling.
    pub fn start(&self) -> Result<WatchSession> {
        // Unbounded so the notify callback (a synchronous closure on the notify
        // thread) never blocks; signals are coalesced on the consumer side.
        let (tx, rx) = mpsc::unbounded_channel::<Signal>();
        let state = Arc::new(WatchState::new());

        let backend = if self.config.force_poll {
            state.poll("polling was requested".to_string());
            poll_backend(&self.root, &tx, &self.config)?
        } else {
            match notify::recommended_watcher(handler(self.root.clone(), tx.clone())) {
                Ok(watcher) => Backend::Events(watcher),
                Err(e) => {
                    state.poll(format!("native file events are unavailable: {e}"));
                    poll_backend(&self.root, &tx, &self.config)?
                }
            }
        };

        let mut session = WatchSession {
            backend,
            tx,
            rx,
            root: self.root.clone(),
            pipeline: Arc::clone(&self.pipeline),
            config: self.config.clone(),
            watched: HashSet::new(),
            state,
        };
        let directories = session.scanned_directories(&self.root).ok_or_else(|| {
            Error::Watcher(format!(
                "{} is not a directory a scan can read",
                self.root.display()
            ))
        })?;
        session.register(&directories.0)?;

        if let Some(reason) = session.state.fallback_reason() {
            tracing::warn!(
                "Watching {} by polling every {} ms: {reason}",
                self.root.display(),
                self.config.poll_interval.as_millis()
            );
        }
        tracing::info!(
            "👁  Watching {} for changes ({}, {} directories, debounce {} ms)",
            self.root.display(),
            session.state.mode().as_str(),
            session.state.watched_directories(),
            self.config.debounce.as_millis()
        );
        Ok(session)
    }
}

/// What the event handler tells the session.
#[derive(Debug)]
enum Signal {
    /// Paths an event touched that may need re-indexing, and directories it
    /// created (or moved into place).
    Changed {
        paths: Vec<PathBuf>,
        created_dirs: Vec<PathBuf>,
    },
    /// Events were lost or the watcher failed: only a full re-scan is safe.
    Rescan(String),
}

/// The watcher behind a session.
enum Backend {
    Events(notify::RecommendedWatcher),
    Poll(notify::PollWatcher),
}

impl Backend {
    fn watch(&mut self, dir: &Path) -> notify::Result<()> {
        match self {
            Self::Events(watcher) => watcher.watch(dir, RecursiveMode::NonRecursive),
            Self::Poll(watcher) => watcher.watch(dir, RecursiveMode::NonRecursive),
        }
    }
}

/// A polling watcher reporting to `tx`.
fn poll_backend(
    root: &Path,
    tx: &mpsc::UnboundedSender<Signal>,
    config: &WatchConfig,
) -> Result<Backend> {
    notify::PollWatcher::new(
        handler(root.to_path_buf(), tx.clone()),
        notify::Config::default().with_poll_interval(config.poll_interval),
    )
    .map(Backend::Poll)
    .map_err(|e| Error::Watcher(format!("failed to create a polling watcher: {e}")))
}

/// The event handler: turns each event under `root` into a [`Signal`].
fn handler(
    root: PathBuf,
    tx: mpsc::UnboundedSender<Signal>,
) -> impl FnMut(notify::Result<notify::Event>) + Send + 'static {
    move |event: notify::Result<notify::Event>| {
        let signal = match event {
            Err(e) => Signal::Rescan(format!("the file watcher reported an error: {e}")),
            Ok(event) if event.need_rescan() => {
                Signal::Rescan("the system dropped file events".to_string())
            }
            Ok(event) => match changes(&root, &event) {
                Some(signal) => signal,
                None => return,
            },
        };
        // Ignore send errors: they only happen once the receiver (the run
        // loop) has been dropped, i.e. we're shutting down.
        let _ = tx.send(signal);
    }
}

/// What `event` changed under `root` that the index may care about: source
/// files created, modified or removed; any path removed or moved away (a
/// directory's files go with it); and directories created or moved into
/// place, which need watches of their own.
fn changes(root: &Path, event: &notify::Event) -> Option<Signal> {
    use notify::EventKind;
    use notify::event::ModifyKind;

    // Access (read/open) events never change content.
    if matches!(event.kind, EventKind::Access(_)) {
        return None;
    }
    let created = matches!(event.kind, EventKind::Create(_));
    let moved_or_gone = matches!(
        event.kind,
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
    );
    let mut paths = Vec::new();
    let mut created_dirs = Vec::new();
    for path in &event.paths {
        if has_ignored_component(root, path) {
            continue;
        }
        if path.is_dir() {
            if created || moved_or_gone {
                created_dirs.push(path.clone());
            }
        } else if path_is_relevant(root, path) || (moved_or_gone && !path.exists()) {
            paths.push(path.clone());
        }
    }
    (!paths.is_empty() || !created_dirs.is_empty()).then_some(Signal::Changed {
        paths,
        created_dirs,
    })
}

/// Whether a failure to add a watch means the system's limits on watches or
/// watcher instances are reached: what polling avoids.
fn is_limit(error: &notify::Error) -> bool {
    match &error.kind {
        notify::ErrorKind::MaxFilesWatch => true,
        notify::ErrorKind::Io(e) => is_limit_io(e),
        _ => false,
    }
}

#[cfg(unix)]
fn is_limit_io(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENOSPC))
}

#[cfg(not(unix))]
fn is_limit_io(_error: &std::io::Error) -> bool {
    false
}

/// Signals gathered during one debounce window.
#[derive(Default)]
struct Batch {
    paths: HashSet<PathBuf>,
    created_dirs: Vec<PathBuf>,
    rescan: Option<String>,
}

impl Batch {
    fn add(&mut self, signal: Signal) {
        match signal {
            Signal::Changed {
                paths,
                created_dirs,
            } => {
                self.paths.extend(paths);
                self.created_dirs.extend(created_dirs);
            }
            Signal::Rescan(reason) => {
                self.rescan.get_or_insert(reason);
            }
        }
    }
}

/// A registered watch, ready to re-index on change; see [`CodeWatcher::start`].
/// Dropping it ends the watch.
pub struct WatchSession {
    /// Owns the registrations: the watches end when this is dropped.
    backend: Backend,
    /// For the handler of a polling watcher created after a fallback.
    tx: mpsc::UnboundedSender<Signal>,
    rx: mpsc::UnboundedReceiver<Signal>,
    root: PathBuf,
    pipeline: Arc<CodebasePipeline>,
    config: WatchConfig,
    watched: HashSet<PathBuf>,
    state: Arc<WatchState>,
}

impl WatchSession {
    /// What the watch covers, readable while it runs.
    #[must_use]
    pub fn state(&self) -> Arc<WatchState> {
        Arc::clone(&self.state)
    }

    /// The watched directories, sorted.
    #[must_use]
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.watched.iter().cloned().collect();
        dirs.sort();
        dirs
    }

    /// The directories at and below `dir` that a scan of the root descends
    /// into, outside [`IGNORED_DIR_COMPONENTS`], and the files in them it
    /// would index; `None` when the scan does not reach `dir`.
    fn scanned_directories(&self, dir: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
        let (dirs, files) = self.pipeline.scanner().tree(&self.root, dir)?;
        let dirs = dirs
            .into_iter()
            .filter(|dir| !has_ignored_component(&self.root, dir))
            .collect();
        Some((dirs, files))
    }

    /// Watch each of `dirs` not yet watched. At the system's limits, switch
    /// to polling and watch everything again.
    ///
    /// # Errors
    /// Returns an error when the root cannot be watched, or a polling watcher
    /// cannot be created.
    fn register(&mut self, dirs: &[PathBuf]) -> Result<()> {
        for dir in dirs {
            if self.watched.contains(dir) {
                continue;
            }
            match self.backend.watch(dir) {
                Ok(()) => {
                    self.watched.insert(dir.clone());
                }
                Err(e) if is_limit(&e) && matches!(self.backend, Backend::Events(_)) => {
                    self.fall_back_to_polling(format!(
                        "the system's limit on file watches was reached: {e}"
                    ))?;
                    return self.register(dirs);
                }
                // Gone since it was listed.
                Err(e) if matches!(e.kind, notify::ErrorKind::PathNotFound) => {}
                Err(e) if *dir == self.root => {
                    return Err(Error::Watcher(format!(
                        "failed to watch {}: {e}",
                        dir.display()
                    )));
                }
                Err(e) => tracing::warn!("Not watching {}: {e}", dir.display()),
            }
        }
        self.state
            .watched_directories
            .store(self.watched.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Replace the native watcher with a polling one, watching what it
    /// watched.
    fn fall_back_to_polling(&mut self, reason: String) -> Result<()> {
        tracing::warn!("Watching {} by polling: {reason}", self.root.display());
        self.backend = poll_backend(&self.root, &self.tx, &self.config)?;
        self.state.poll(reason);
        let watched: Vec<PathBuf> = self.watched.drain().collect();
        self.register(&watched)
    }

    /// Run the re-index loop until the returned future is dropped (e.g. via
    /// `tokio::select!` on a shutdown signal).
    ///
    /// `on_rescan` is invoked after every successful re-index with the
    /// pipeline result, so callers can log progress or update UI. Changes made
    /// since [`CodeWatcher::start`] returned trigger the first one. A failed
    /// re-index is logged, and the next change triggers another.
    pub async fn run<F>(mut self, mut on_rescan: F)
    where
        F: FnMut(WatchRescan) + Send,
    {
        loop {
            // Park until the first change signal.
            let Some(first) = self.rx.recv().await else {
                return;
            };
            let mut batch = Batch::default();
            batch.add(first);

            // Debounce: keep collecting until the stream is quiet for `debounce`.
            loop {
                match tokio::time::timeout(self.config.debounce, self.rx.recv()).await {
                    Ok(Some(signal)) => batch.add(signal),
                    Ok(None) => return,
                    Err(_elapsed) => break,
                }
            }

            // New directories: watch them, and index the files already in
            // them, which may predate their watches.
            for dir in std::mem::take(&mut batch.created_dirs) {
                if let Some((dirs, files)) = self.scanned_directories(&dir) {
                    if let Err(e) = self.register(&dirs) {
                        batch.rescan.get_or_insert(e.to_string());
                    }
                    batch.paths.extend(files);
                }
            }

            let outcome = match batch.rescan {
                Some(reason) => {
                    tracing::info!("Re-scanning {}: {reason}", self.root.display());
                    self.pipeline.analyze_directory(&self.root).await
                }
                None if batch.paths.is_empty() => continue,
                None => {
                    let paths: Vec<PathBuf> = batch.paths.into_iter().collect();
                    self.pipeline.reindex_paths(&self.root, &paths).await
                }
            };
            match outcome {
                Ok(result) => {
                    tracing::info!(
                        "🔄 Re-index: {} parsed, {} skipped, {} entities, {} relationships written ({:.2}s)",
                        result.files_parsed,
                        result.files_skipped,
                        result.entity_count,
                        result.relationship_count,
                        result.processing_time_secs,
                    );
                    on_rescan(WatchRescan { result });
                }
                Err(e) => {
                    // A single failed re-index should not tear down the watcher;
                    // the next change will trigger another attempt.
                    tracing::error!("watch re-index failed: {e}");
                }
            }
            // Do NOT drain queued signals here: any change that landed during
            // the re-index must trigger the next iteration so nothing is
            // missed. Our own writes go only to the DB, which `changes`
            // filters out, so there is no self-retrigger.
        }
    }
}

/// Whether `path` names a parseable source file below `root`, outside the
/// ignored directories and editor scratch files.
///
/// Restricting to source files (by extension or well-known filename) excludes
/// the knowledge-graph database and its WAL/SHM artifacts, logs, and other
/// noise, so the watcher never retriggers on its own database writes.
fn path_is_relevant(root: &Path, path: &Path) -> bool {
    if has_ignored_component(root, path) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // Ignore editor swap/backup/temp files (the real save on the target file
    // still triggers).
    if name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".tmp") {
        return false;
    }
    // Relevant only if it is a language we can parse — by extension (e.g. `.rs`,
    // `.py`) or by well-known filename (e.g. `Makefile`, `Dockerfile`). This
    // also excludes `.db`/`.db-wal`/`.db-shm`, logs, and binary/data files.
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && extension_to_language(ext).is_some()
    {
        return true;
    }
    filename_to_language(name).is_some()
}

/// Whether a directory of `path` below `root` is one of
/// [`IGNORED_DIR_COMPONENTS`]. Only components below the root count: a project
/// that itself lives under a directory named `build` is still watched.
fn has_ignored_component(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| IGNORED_DIR_COMPONENTS.contains(&s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_vcs_and_build_dirs() {
        let root = Path::new("/proj");
        assert!(has_ignored_component(root, Path::new("/proj/.git/index")));
        assert!(has_ignored_component(
            root,
            Path::new("/proj/target/debug/foo")
        ));
        assert!(has_ignored_component(
            root,
            Path::new("/proj/node_modules/x/y.js")
        ));
        assert!(!has_ignored_component(root, Path::new("/proj/src/main.rs")));
    }

    /// A project under a directory with an ignored name is still watched.
    #[test]
    fn only_components_below_the_root_count() {
        let root = Path::new("/home/me/build/project");
        assert!(!has_ignored_component(
            root,
            Path::new("/home/me/build/project/src/main.rs")
        ));
        assert!(has_ignored_component(
            root,
            Path::new("/home/me/build/project/target/x.rs")
        ));
    }

    #[test]
    fn only_source_files_are_relevant() {
        let root = Path::new("/proj");
        // Parseable source files trigger.
        assert!(path_is_relevant(root, Path::new("/proj/src/main.rs")));
        assert!(path_is_relevant(root, Path::new("/proj/app/service.py")));
        // Database artifacts, logs, and data files do NOT trigger.
        assert!(!path_is_relevant(root, Path::new("/proj/devscriptor.db")));
        assert!(!path_is_relevant(
            root,
            Path::new("/proj/devscriptor.db-wal")
        ));
        assert!(!path_is_relevant(
            root,
            Path::new("/proj/devscriptor.db-shm")
        ));
        assert!(!path_is_relevant(root, Path::new("/proj/watch.log")));
        assert!(!path_is_relevant(root, Path::new("/proj/data.bin")));
        // Ignored directories and editor temp files do NOT trigger.
        assert!(!path_is_relevant(root, Path::new("/proj/.git/HEAD")));
        assert!(!path_is_relevant(root, Path::new("/proj/src/main.rs.swp")));
        assert!(!path_is_relevant(root, Path::new("/proj/src/main.rs~")));
    }

    /// A removed path counts whatever its name: it may have been a directory
    /// whose files are gone with it.
    #[test]
    fn a_removed_path_counts_and_a_created_directory_is_reported() {
        use notify::event::{CreateKind, RemoveKind};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let gone = notify::Event::new(notify::EventKind::Remove(RemoveKind::Any))
            .add_path(root.join("old_package"));
        match changes(root, &gone) {
            Some(Signal::Changed { paths, .. }) => {
                assert_eq!(paths, vec![root.join("old_package")]);
            }
            other => panic!("{other:?}"),
        }

        std::fs::create_dir(root.join("pkg")).unwrap();
        let created = notify::Event::new(notify::EventKind::Create(CreateKind::Folder))
            .add_path(root.join("pkg"));
        match changes(root, &created) {
            Some(Signal::Changed { created_dirs, .. }) => {
                assert_eq!(created_dirs, vec![root.join("pkg")]);
            }
            other => panic!("{other:?}"),
        }
    }
}
