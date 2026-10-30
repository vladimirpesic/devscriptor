//! LSP server lifecycle management.
//!
//! This module handles spawning language server processes and managing
//! their lifecycle from initialization to shutdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, Command};
use tracing::{info, warn};

#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;

use crate::bridge::path_to_uri;
use crate::config::LspServerConfig;
use crate::error::{LspError, Result, ServerSpawnFailure};
use crate::lsp::client::LspClient;
use crate::lsp::compile_commands::COMPILE_COMMANDS_DIR_PLACEHOLDER;
use crate::lsp::transport::LspTransport;

/// How many stderr lines to retain for diagnosing a server that dies at startup.
/// Enough to carry a real error message, small enough that a chatty server
/// cannot grow it without bound.
const STDERR_CAPTURE_LINES: usize = 20;

/// Describe a server that exited before completing the `initialize` handshake,
/// quoting its stderr so the cause is visible in the error itself.
async fn exited_during_init_message(
    language_id: &str,
    status: std::io::Result<std::process::ExitStatus>,
    stderr_head: &std::sync::Mutex<Vec<String>>,
) -> String {
    // The process has exited, closing its stderr, so the reader task is draining
    // its last buffered lines. Give it a brief moment so the reason it died —
    // the whole point of this message — is captured rather than raced.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let how = match status {
        Ok(status) => match status.code() {
            Some(code) => format!("exited with status {code}"),
            None => "was terminated by a signal".to_string(),
        },
        Err(e) => format!("could not be waited on: {e}"),
    };
    let detail = stderr_head
        .lock()
        .ok()
        .map(|buf| buf.join("; "))
        .unwrap_or_default();
    if detail.trim().is_empty() {
        format!("server {how} during initialization (no stderr output)")
    } else {
        format!(
            "server {how} during initialization: {}{}",
            detail.trim(),
            expired_build_hint(language_id, &detail)
        )
    }
}

/// What to do about a server whose build has expired, as its `detail` (what
/// it printed) says: JetBrains' Kotlin LSP builds stop starting some weeks
/// after their release, with "This build of kotlin-server has expired".
/// Installing the server again brings it up to its latest build.
fn expired_build_hint(language_id: &str, detail: &str) -> String {
    if !(detail.contains("build of") && detail.contains("has expired")) {
        return String::new();
    }
    let stop = if detail.trim_end().ends_with('.') {
        ""
    } else {
        "."
    };
    format!("{stop} `devscriptor lsp setup {language_id}` installs its current build.")
}

/// Information about a spawned LSP server.
pub struct LspServer {
    /// Client for communication.
    client: LspClient,
    /// Child process handle. `Option` so that `Drop` can take ownership.
    _child: Option<Child>,
    /// Server configuration.
    config: LspServerConfig,
    /// Server capabilities returned by the `initialize` handshake, stored
    /// verbatim as a JSON value so the full `ServerCapabilities` object can be
    /// surfaced to callers without depending on exact `lsp_types` shape.
    capabilities: serde_json::Value,
    /// When the process was started.
    started: std::time::Instant,
    /// The server's own temporary directory (its `TMPDIR`), removed with it:
    /// what a server leaves there goes too. rust-analyzer leaves its empty
    /// `proc-macro-srv*` directories behind even when it exits cleanly.
    temp_dir: Option<tempfile::TempDir>,
}

/// `args` with what only the workspace decides: the directory of the
/// compilation database clangd reads for `folders`
/// ([`COMPILE_COMMANDS_DIR_PLACEHOLDER`]), written off the executor, as it
/// walks the project; the argument is left out where the project configures
/// clangd itself.
async fn workspace_args(args: &[String], folders: &[PathBuf]) -> Vec<String> {
    if !args
        .iter()
        .any(|arg| arg.contains(COMPILE_COMMANDS_DIR_PLACEHOLDER))
    {
        return args.to_vec();
    }
    let owned = folders.to_vec();
    let database = tokio::task::spawn_blocking(move || {
        crate::lsp::compile_commands::database_dir(&owned, &crate::manager::cache_root())
    })
    .await
    .ok()
    .flatten();
    args.iter()
        .filter_map(|arg| {
            if !arg.contains(COMPILE_COMMANDS_DIR_PLACEHOLDER) {
                return Some(arg.clone());
            }
            let dir = database.as_ref()?;
            Some(arg.replace(COMPILE_COMMANDS_DIR_PLACEHOLDER, &dir.to_string_lossy()))
        })
        .collect()
}

impl LspServer {
    /// The server process's id, while it runs.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self._child.as_ref().and_then(Child::id)
    }

    /// Seconds since the server process was started.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Get a reference to the LSP client.
    #[must_use]
    pub fn client(&self) -> &LspClient {
        &self.client
    }

    /// Get the real server capabilities returned during initialization.
    #[must_use]
    pub fn capabilities(&self) -> &serde_json::Value {
        &self.capabilities
    }

    /// Spawn a new LSP server process.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The process cannot be spawned
    /// - Stdin/stdout cannot be captured
    /// - The initialization handshake fails
    pub async fn spawn(config: &LspServerConfig, workspace_root: &Path) -> Result<Self> {
        Self::spawn_in(config, &[workspace_root.to_path_buf()]).await
    }

    /// Spawn a new LSP server process for several workspace folders at once —
    /// the projects of a monorepo whose root is not one, say. It runs in, and
    /// takes as its root, the first.
    ///
    /// # Errors
    ///
    /// As [`Self::spawn`]; also when `folders` is empty.
    pub async fn spawn_in(config: &LspServerConfig, folders: &[PathBuf]) -> Result<Self> {
        let workspace_root = folders.first().ok_or_else(|| LspError::ServerSpawnFailed {
            language_id: config.language_id.clone(),
            command: config.command.clone(),
            error: "no workspace folder to run in".into(),
        })?;
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!(
                "{TEMP_DIR_PREFIX}{}-{}-",
                std::process::id(),
                config.language_id
            ))
            .tempdir()
            .map_err(|e| {
                warn!(
                    "No temporary directory of its own for the {} server: {e}",
                    config.language_id
                );
            })
            .ok();
        #[cfg(unix)]
        if let Some(dir) = &temp_dir {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = std::fs::metadata(dir.path()) {
                sweep_orphaned_temp_dirs(meta.uid());
            }
        }
        let args = workspace_args(&config.args, folders).await;
        let mut cmd = Command::new(&config.command);
        if let Some(dir) = &temp_dir {
            for var in ["TMPDIR", "TEMP", "TMP"] {
                cmd.env(var, dir.path());
            }
        }
        // A lock file the user's environment names is theirs to keep.
        if config.language_id == "rust" && std::env::var_os(CARGO_LOCKFILE_VAR).is_none() {
            copy_cargo_locks(folders);
            cmd.env(CARGO_LOCKFILE_VAR, RUST_ANALYZER_LOCKFILE);
        }
        ignore_state_dirs(&config.language_id, folders);
        cmd.args(&args)
            // Some servers only find themselves through the environment: `ruby-lsp`
            // is a gem binstub that needs `GEM_HOME` pointing at its install dir.
            .envs(&config.env)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // `kill_on_drop(false)` prevents tokio from SIGKILLing
            // the child when `LspServer` is dropped. Graceful shutdown is
            // handled by `shutdown()`; unexpected drops are handled by the
            // `Drop` impl which spawns a background kill-and-reap task.
            .kill_on_drop(false);

        // Ask the kernel to kill this server when its parent goes away.
        //
        // `Drop` and `shutdown()` cover the orderly cases, but neither runs when
        // the parent is itself SIGKILLed — a `timeout` firing, a supervisor
        // reaping, an operator with `kill -9`. The children are then re-parented
        // to init and survive, and language servers accumulate across runs: a
        // day's work leaves a row of stray `phpactor`, `jdtls` and `gopls`
        // processes holding memory. `PR_SET_PDEATHSIG` is the only mechanism
        // that still fires in that case, because the kernel delivers it.
        //
        // The signal is tied to the parent *thread*, not the process. That is
        // sound here: the runtime's worker threads live as long as the runtime,
        // and tokio never retires one early.
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                    .map_err(std::io::Error::from)
            });
        }

        let mut child = cmd.spawn().map_err(|e| LspError::ServerSpawnFailed {
            language_id: config.language_id.clone(),
            command: config.command.clone(),
            error: e.to_string(),
        })?;
        let started = std::time::Instant::now();

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::ServerSpawnFailed {
                language_id: config.language_id.clone(),
                command: config.command.clone(),
                error: "Failed to capture stdin".into(),
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::ServerSpawnFailed {
                language_id: config.language_id.clone(),
                command: config.command.clone(),
                error: "Failed to capture stdout".into(),
            })?;

        // Drain stderr so the language server doesn't block on a full pipe.
        //
        // The first few lines are also retained: when a server dies during the
        // handshake, its stderr is the only thing that says *why* (e.g. a rustup
        // proxy reporting `Unknown binary 'rust-analyzer'`), and that belongs in
        // the error rather than in a debug log the caller never sees.
        let stderr_head = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        if let Some(stderr) = child.stderr.take() {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let lang = config.language_id.clone();
            let sink = std::sync::Arc::clone(&stderr_head);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::debug!(target: "lsp.stderr", language = %lang, "{line}");
                    if let Ok(mut buf) = sink.lock()
                        && buf.len() < STDERR_CAPTURE_LINES
                    {
                        buf.push(line);
                    }
                }
            });
        }

        let transport = LspTransport::new(stdin, stdout);
        let client = LspClient::from_transport(config.clone(), transport);

        // Perform initialization handshake and capture real server capabilities.
        //
        // Racing the handshake against the process exit matters: a server that
        // dies immediately — not installed, wrong architecture, a rustup shim for
        // an uninstalled component — will never answer `initialize`, and waiting
        // the full request timeout turns a startup error into a 30-second stall
        // reported as an unhelpful "timeout". Losing that race is diagnosable.
        let capabilities = tokio::select! {
            result = initialize_client(&client, config, folders) => match result {
                Ok(caps) => caps,
                // The handshake errored. If the process dies, its exit and
                // stderr are the real cause (a raw write/transport error hides
                // the `Unknown binary …` the server printed); otherwise the init
                // error stands. A server that closes its output exits a moment
                // later, so it is given a second: asked at once, sourcekit-lsp missing a
                // library would be "Language server closed the connection", where a
                // second later it says `libxml2.so.2` is not found.
                Err(init_err) => {
                    match tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await {
                        Ok(status) => {
                            return Err(LspError::ServerSpawnFailed {
                                language_id: config.language_id.clone(),
                                command: config.command.clone(),
                                error: exited_during_init_message(&config.language_id, status, &stderr_head)
                                    .await,
                            });
                        }
                        // Still running, but no server: stopped, not left behind.
                        Err(_) => {
                            let _ = child.kill().await;
                            return Err(init_err);
                        }
                    }
                }
            },
            // The process exited before answering the handshake.
            status = child.wait() => {
                return Err(LspError::ServerSpawnFailed {
                    language_id: config.language_id.clone(),
                    command: config.command.clone(),
                    error: exited_during_init_message(&config.language_id, status, &stderr_head)
                                    .await,
                });
            }
        };

        info!(
            "LSP server '{}' initialized successfully",
            config.language_id
        );

        Ok(Self {
            client,
            _child: Some(child),
            config: config.clone(),
            capabilities,
            started,
            temp_dir,
        })
    }

    /// Shutdown the LSP server gracefully.
    ///
    /// Sends the LSP shutdown request and waits for the process to exit.
    /// If the process does not exit within the timeout, it is killed when
    /// `self` is dropped (see the `Drop` impl).
    pub async fn shutdown(mut self) -> Result<()> {
        // Send LSP shutdown request (best effort - don't fail if already dead)
        let _ = self
            .client
            .request::<_, serde_json::Value>("shutdown", serde_json::Value::Null)
            .await;
        let _ = self.client.notify("exit", serde_json::Value::Null).await;

        // Wait for process to exit with timeout
        let timeout = std::time::Duration::from_secs(5);
        let Some(child) = self._child.as_mut() else {
            warn!("LSP server '{}' already shut down", self.config.language_id);
            return Ok(());
        };
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => {
                if status.success() {
                    info!("LSP server '{}' exited cleanly", self.config.language_id);
                } else {
                    warn!(
                        "LSP server '{}' exited with code {:?}",
                        self.config.language_id,
                        status.code()
                    );
                }
            }
            Ok(Err(e)) => {
                warn!(
                    "Failed to wait for LSP server '{}': {}",
                    self.config.language_id, e
                );
            }
            Err(_) => {
                warn!(
                    "LSP server '{}' did not exit within timeout, killing on drop",
                    self.config.language_id
                );
            }
        }

        Ok(())
    }
}

/// The start of a server's temporary directory's name: then the id of the
/// Devscriptor process it belongs to, and the language.
const TEMP_DIR_PREFIX: &str = "devscriptor-lsp-";

/// Remove, once per process, the servers' temporary directories that `owner`
/// left behind in Devscriptor processes that are gone: a process killed with
/// SIGKILL, or one that crashed, removed none of its own.
#[cfg(unix)]
fn sweep_orphaned_temp_dirs(owner: u32) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| sweep_orphaned_temp_dirs_in(&std::env::temp_dir(), owner));
}

/// [`sweep_orphaned_temp_dirs`] in `root`: the directories of `owner` named
/// for a process that is not running.
#[cfg(unix)]
fn sweep_orphaned_temp_dirs_in(root: &Path, owner: u32) {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(TEMP_DIR_PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        // `DirEntry::metadata` does not follow a symbolic link.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let running = !matches!(
            signal::kill(Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
        if meta.is_dir() && meta.uid() == owner && !running {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

impl Drop for LspServer {
    /// If `shutdown()` was not called (or the future was cancelled), ensure
    /// the child process is killed and reaped so it does not become a zombie.
    fn drop(&mut self) {
        let Some(mut child) = self._child.take() else {
            return;
        };

        #[cfg(unix)]
        {
            // Synchronous SIGKILL via nix — safe inside `Drop`, no runtime needed.
            if let Some(pid) = child.id() {
                let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
            // If a runtime is still alive, spawn a background task to `wait()`
            // and reap the child so it does not become a zombie, and remove its
            // temporary directory once nothing writes there any more.
            let temp_dir = self.temp_dir.take();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                #[allow(clippy::let_underscore_future)]
                let _ = handle.spawn(async move {
                    let _ = child.wait().await;
                    drop(temp_dir);
                });
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix platforms we rely on the runtime for async kill.
            // `Handle::try_current()` avoids the panic from `tokio::spawn`
            // when no runtime is present. The temporary directory goes once
            // the process is gone: Windows deletes no file a process holds.
            let temp_dir = self.temp_dir.take();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                #[allow(clippy::let_underscore_future)]
                let _ = handle.spawn(async move {
                    let _ = child.kill().await;
                    drop(temp_dir);
                });
            }
        }
    }
}

/// The capabilities Devscriptor declares to every language server.
fn client_capabilities() -> lsp_types::ClientCapabilities {
    lsp_types::ClientCapabilities {
        general: Some(lsp_types::GeneralClientCapabilities {
            // UTF-16 only. Columns cross this crate unconverted: an agent's
            // `character` goes to the server as-is, and the server's ranges
            // come back as-is and are applied to files by
            // `position_to_byte_offset`, which counts UTF-16 code units. A
            // server that negotiated UTF-8 instead (rust-analyzer and clangd
            // pick it when offered) answered in bytes, so a rename on a line
            // with non-ASCII text cut the file at the wrong offsets. UTF-16 is
            // the LSP default every server must support, and it counts each
            // character of the Basic Multilingual Plane as one.
            position_encodings: Some(vec![lsp_types::PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        // Declare server-initiated work-done progress. Without this, servers
        // like rust-analyzer never send `window/workDoneProgress/create` +
        // `$/progress`, so `wait_until_ready` never observes indexing and
        // short-circuits after its no-progress grace — the agent's first
        // semantic request then races the cold index and comes back empty.
        window: Some(lsp_types::WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        // Declare both ways of getting diagnostics. A server offers pull
        // diagnostics only to a client that says it can pull them, and
        // typescript-language-server pushes none to a client that does not
        // say it takes pushes — so, with neither declared, it never
        // reported a single diagnostic.
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities::default()),
            diagnostic: Some(lsp_types::DiagnosticClientCapabilities {
                dynamic_registration: Some(false),
                related_document_support: Some(false),
            }),
            // Take a file's symbols as a tree. To a client that does not say
            // it can, servers list them flat, each at its whole declaration:
            // `def`, `func` or a return type rather than the name, so a hover
            // or references asked where `get_document_symbols` put a symbol
            // found nothing.
            document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The position encoding a server chose, if it is not the UTF-16 Devscriptor
/// offered. A server that names none uses UTF-16.
fn foreign_position_encoding(
    capabilities: &lsp_types::ServerCapabilities,
) -> Option<&lsp_types::PositionEncodingKind> {
    capabilities
        .position_encoding
        .as_ref()
        .filter(|encoding| **encoding != lsp_types::PositionEncodingKind::UTF16)
}

/// The initialization options `config`'s server starts with for `folders`:
/// its own, and for jdtls the builds it is not to import
/// ([`jdtls_import_exclusions`]) and no build of its own: by default it
/// compiles each project it imports into the project's output folder —
/// Maven's `target/classes`, `bin/` beside a Gradle build, which Gradle
/// projects do not ignore — writing the user's build output with its own
/// compiler. Asked about the code, it answers from its sources.
fn initialization_options(
    config: &LspServerConfig,
    folders: &[PathBuf],
) -> Option<serde_json::Value> {
    let mut options = config.initialization_options.clone();
    if config.language_id == "java" {
        let exclusions = jdtls_import_exclusions(folders);
        let options = options.get_or_insert_with(|| serde_json::json!({}));
        if let Some(object) = options.as_object_mut() {
            object.insert(
                "settings".to_string(),
                serde_json::json!({ "java": {
                    "import": { "exclusions": exclusions },
                    "autobuild": { "enabled": false }
                } }),
            );
        }
    }
    options
}

/// The variable cargo takes a lock file's path from (Rust 1.97 and later):
/// what it resolves a workspace's dependencies into, in place of the
/// `Cargo.lock` beside its manifest.
const CARGO_LOCKFILE_VAR: &str = "CARGO_RESOLVER_LOCKFILE_PATH";

/// Where rust-analyzer's cargo keeps each workspace's lock file: in its own
/// build directory (`cargo.targetDir`), relative to the workspace root it
/// runs cargo in. Otherwise a workspace with no `Cargo.lock` would get one in
/// the project, and `cargo check` would rewrite one that is out of date.
const RUST_ANALYZER_LOCKFILE: &str = "target/rust-analyzer/Cargo.lock";

/// Copy the `Cargo.lock` of each of `folders` that has one to where
/// rust-analyzer's cargo reads it ([`RUST_ANALYZER_LOCKFILE`]), so that its
/// `cargo check` builds the versions the project pins, and leaves the
/// project's own lock file as it is. rust-analyzer reads a copy of its own
/// for `cargo metadata`.
fn copy_cargo_locks(folders: &[PathBuf]) {
    for folder in folders {
        let lock = folder.join("Cargo.lock");
        if !lock.is_file() {
            continue;
        }
        let copy = folder.join(RUST_ANALYZER_LOCKFILE);
        let copied = copy
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::copy(&lock, &copy));
        if let Err(e) = copied {
            warn!("rust-analyzer will resolve {} anew: {e}", folder.display());
        }
    }
}

/// The directories `language_id`'s server keeps its state in, inside the
/// projects among `folders`, which no build of the project makes and no
/// option moves elsewhere: Metals' `.metals/` (its database and log), the
/// `.bloop/` and `.bsp/` of the build it imports (Bloop's project files, and
/// the build server's connection files) and, of an sbt build, the
/// `project/.bloop/` of its build definition, and the
/// `.gradle/` Gradle keeps its project cache in at the root of each build
/// kotlin-lsp or jdtls imports. Each is made before the server starts, when
/// missing, with a `.gitignore` that ignores it, as ruby-lsp makes its
/// `.ruby-lsp/`: a project that does not ignore it would have them in `git status`.
/// One that exists — the user's own Metals' — is left as it is.
fn ignore_state_dirs(language_id: &str, folders: &[PathBuf]) {
    let dirs: Vec<PathBuf> = match language_id {
        "scala" => folders
            .iter()
            .flat_map(|folder| {
                let sbt = folder.join("build.sbt").is_file();
                [".metals", ".bloop", ".bsp", "project/.bloop"]
                    .into_iter()
                    .filter(move |dir| sbt || !dir.starts_with("project/"))
                    .map(|dir| folder.join(dir))
            })
            .collect(),
        "kotlin" => gradle_build_roots(folders, &["kt", "kts"])
            .into_iter()
            .map(|root| root.join(".gradle"))
            .collect(),
        "java" => gradle_build_roots(folders, &["java"])
            .into_iter()
            .map(|root| root.join(".gradle"))
            .collect(),
        _ => Vec::new(),
    };
    for dir in dirs {
        if dir.exists() {
            continue;
        }
        let made = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join(".gitignore"), "*\n"));
        if let Err(e) = made {
            warn!("{} may show in git status: {e}", dir.display());
        }
    }
}

/// The Gradle build files: a build's, and a multi-project build's settings.
const GRADLE_FILES: [&str; 4] = [
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// The roots of the Gradle builds under `folders` that hold a source file of
/// one of `extensions` (a build script aside): the outermost directory with a
/// Gradle file of each, where Gradle keeps the build's project cache.
fn gradle_build_roots(folders: &[PathBuf], extensions: &[&str]) -> Vec<PathBuf> {
    let mut builds: Vec<PathBuf> = Vec::new();
    let mut sources: Vec<PathBuf> = Vec::new();
    for folder in folders {
        for entry in ignore::WalkBuilder::new(folder)
            .max_depth(Some(12))
            .build()
            .flatten()
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if GRADLE_FILES.contains(&name) {
                if let Some(dir) = path.parent() {
                    builds.push(dir.to_path_buf());
                }
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
            {
                sources.push(path.to_path_buf());
            }
        }
    }
    builds.sort();
    builds.dedup();
    let holding: Vec<&PathBuf> = builds
        .iter()
        .filter(|build| sources.iter().any(|source| source.starts_with(build)))
        .collect();
    holding
        .iter()
        .filter(|build| {
            !holding
                .iter()
                .any(|outer| outer != *build && build.starts_with(outer))
        })
        .map(|build| (*build).clone())
        .collect()
}

/// jdtls's default import exclusions, which a setting replaces.
const JDTLS_DEFAULT_EXCLUSIONS: [&str; 4] = [
    "**/node_modules/**",
    "**/.metadata/**",
    "**/archetype-resources/**",
    "**/META-INF/maven/**",
];

/// The import exclusions jdtls starts with for `folders`: its defaults, and
/// every Maven or Gradle build under them that holds no Java source. jdtls
/// imports and builds every build it finds under its folders, and is given
/// the workspace: asked about a Java service of a monorepo, it imported the
/// Kotlin service's Gradle build, ran Gradle, and wrote `.gradle/` and a
/// compiled copy of the Kotlin sources under `bin/` into it.
fn jdtls_import_exclusions(folders: &[PathBuf]) -> Vec<String> {
    const BUILD_FILES: [&str; 5] = [
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ];
    let mut exclusions: Vec<String> = JDTLS_DEFAULT_EXCLUSIONS
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect();
    // (folder, build directory) pairs, and the Java sources found.
    let mut builds: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut with_java: Vec<PathBuf> = Vec::new();
    for folder in folders {
        for entry in ignore::WalkBuilder::new(folder)
            .max_depth(Some(12))
            .build()
            .flatten()
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if BUILD_FILES.contains(&name) {
                if let Some(dir) = path.parent() {
                    builds.push((folder.clone(), dir.to_path_buf()));
                }
            } else if path.extension().is_some_and(|ext| ext == "java") {
                with_java.push(path.to_path_buf());
            }
        }
    }
    builds.sort();
    builds.dedup();
    for (folder, build) in builds {
        // A build that holds Java sources, or a nested one that does — a
        // parent Maven or Gradle build of Java modules — stays imported.
        if with_java.iter().any(|file| file.starts_with(&build)) {
            continue;
        }
        // Written as jdtls matches them, relative to anywhere (`**/`): the
        // directory itself, which its detector tests as it walks — a pattern
        // for its contents, or an absolute one, matches nothing, and the build
        // would be imported anyway — and what is under it. A folder that is
        // itself such a build holds no Java at all, and is excluded whole.
        let patterns = match build.strip_prefix(&folder) {
            Ok(within) if within.as_os_str().is_empty() => vec!["**".to_string()],
            Ok(within) => {
                // A glob separates folders with `/`, on Windows too.
                let within = within
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                vec![format!("**/{within}"), format!("**/{within}/**")]
            }
            Err(_) => continue,
        };
        for pattern in patterns {
            if !exclusions.contains(&pattern) {
                exclusions.push(pattern);
            }
        }
    }
    exclusions
}

/// Initialize the LSP client with the server.
///
/// Returns the raw server capabilities from the `InitializeResult` so callers
/// can store and later surface them.
async fn initialize_client(
    client: &LspClient,
    config: &LspServerConfig,
    folders: &[PathBuf],
) -> Result<serde_json::Value> {
    let workspace_folders = folders
        .iter()
        .map(|folder| {
            Ok(lsp_types::WorkspaceFolder {
                uri: path_to_uri(folder)?,
                name: folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let root_uri = workspace_folders
        .first()
        .map(|folder| folder.uri.clone())
        .ok_or_else(|| LspError::Io("no workspace folder to initialize".into()))?;

    let init_params = lsp_types::InitializeParams {
        process_id: Some(std::process::id()),
        #[allow(deprecated)]
        root_path: None, // deprecated by the LSP spec in favour of the workspace folders
        #[allow(deprecated)]
        root_uri: Some(root_uri),
        initialization_options: initialization_options(config, folders),
        capabilities: client_capabilities(),
        trace: None,
        workspace_folders: Some(workspace_folders),
        client_info: Some(lsp_types::ClientInfo {
            name: "devscriptor-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        locale: None,
        work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
    };

    let init_result: lsp_types::InitializeResult =
        client.request("initialize", init_params).await?;
    if let Some(encoding) = foreign_position_encoding(&init_result.capabilities) {
        tracing::warn!(
            "{} chose the {} position encoding although only utf-16 was offered; \
             columns from and to it will be wrong on lines with non-ASCII text",
            config.language_id,
            encoding.as_str()
        );
    }

    // Send initialized notification
    client.notify("initialized", serde_json::json!({})).await?;
    if let Some(settings) = &config.configuration {
        client
            .notify(
                "workspace/didChangeConfiguration",
                serde_json::json!({ "settings": settings }),
            )
            .await?;
    }

    client
        .set_state(crate::lsp::client::ServerState::Ready)
        .await;

    // Serialize capabilities to a plain JSON value so the Translator can
    // surface them without needing to carry the `lsp_types` dependency.
    let capabilities =
        serde_json::to_value(init_result.capabilities).unwrap_or(serde_json::Value::Null);

    Ok(capabilities)
}

/// Result of spawning multiple LSP servers.
pub struct SpawnResult {
    /// Successfully spawned servers.
    pub servers: HashMap<String, LspServer>,
    /// Failures during spawn.
    pub failures: Vec<ServerSpawnFailure>,
}

impl SpawnResult {
    /// Check if all servers failed to spawn.
    #[must_use]
    pub fn all_failed(&self) -> bool {
        self.servers.is_empty() && !self.failures.is_empty()
    }

    /// Check if there was partial success.
    #[must_use]
    pub fn partial_success(&self) -> bool {
        !self.servers.is_empty() && !self.failures.is_empty()
    }

    /// Get the number of successfully spawned servers.
    #[must_use]
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Get the number of failures.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// Check if at least one server is available.
    #[must_use]
    pub fn has_servers(&self) -> bool {
        !self.servers.is_empty()
    }
}

/// Maximum number of LSP servers to spawn concurrently.
///
/// Each language server process can consume 100–500 MB of RAM. Spawning too
/// many concurrently on consumer hardware (8 GB) causes memory exhaustion.
/// This cap limits concurrent spawns to avoid that.
const MAX_CONCURRENT_SPAWNS: usize = 4;

/// Spawn multiple LSP servers with bounded concurrency.
///
/// Each configuration whose project markers match a workspace root is spawned
/// once, with every root it matches as a workspace folder.
///
/// # Arguments
///
/// * `configs` - Server configurations to spawn
/// * `workspace_roots` - Workspace root directories
///
/// # Returns
///
/// A `SpawnResult` containing successful servers and failures.
pub async fn spawn_servers(
    configs: &[LspServerConfig],
    workspace_roots: &[std::path::PathBuf],
) -> SpawnResult {
    let mut pairs: Vec<(LspServerConfig, Vec<PathBuf>)> = Vec::new();
    for config in configs {
        let folders: Vec<PathBuf> = workspace_roots
            .iter()
            .filter(|root| config.should_spawn(root, Some(3)))
            .cloned()
            .collect();
        if folders.is_empty() {
            info!(
                "Skipping LSP server '{}' ({}): no project markers found",
                config.language_id, config.command
            );
            continue;
        }
        pairs.push((config.clone(), folders));
    }

    let mut result = spawn_servers_in(&pairs).await;

    // Add fallback failures for configs that had no successful spawn
    for config in configs {
        let has_success = result.servers.contains_key(&config.language_id);
        let has_failure = result
            .failures
            .iter()
            .any(|f| f.language_id == config.language_id);
        if !has_success && !has_failure {
            result.failures.push(ServerSpawnFailure {
                language_id: config.language_id.clone(),
                command: config.command.clone(),
                error: "Failed to spawn for any workspace root".into(),
            });
        }
    }

    result
}

/// Spawn one server per `(config, workspace folders)` pair, at most
/// `MAX_CONCURRENT_SPAWNS` at a time.
///
/// Unlike `join_all` (which spawns everything at once), this bounds the number
/// of servers starting concurrently.
pub async fn spawn_servers_in(pairs: &[(LspServerConfig, Vec<PathBuf>)]) -> SpawnResult {
    use futures::stream::{self, StreamExt};

    // `buffer_unordered(0)` never polls its inner stream and therefore hangs
    // forever, so floor the concurrency at 1.
    let concurrency = MAX_CONCURRENT_SPAWNS.min(pairs.len()).max(1);
    let results: Vec<_> = stream::iter(pairs.iter().cloned())
        .map(|(config, folders)| async move {
            let shown = folders
                .iter()
                .map(|folder| folder.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            match LspServer::spawn_in(&config, &folders).await {
                Ok(server) => {
                    info!("Spawned LSP server '{}' for {shown}", config.language_id);
                    Ok((config.language_id.clone(), server))
                }
                Err(e) => {
                    warn!("Failed to spawn {} for {shown}: {e}", config.language_id);
                    Err(ServerSpawnFailure {
                        language_id: config.language_id.clone(),
                        command: config.command.clone(),
                        error: format!("Failed to spawn for {shown}: {e}"),
                    })
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let mut servers = HashMap::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            // One server per language: the first that started.
            Ok((lang_id, server)) => {
                servers.entry(lang_id).or_insert(server);
            }
            Err(failure) => failures.push(failure),
        }
    }

    SpawnResult { servers, failures }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rust-analyzer's cargo resolves a workspace into a lock file in its own
    /// build directory, copied from the project's where there is one: a
    /// workspace without `Cargo.lock` gets none in the project, and one out of
    /// date is not rewritten.
    #[test]
    fn rust_analyzer_keeps_its_lock_files_in_its_build_directory() {
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("rating");
        let unlocked = root.path().join("tools");
        for crate_dir in [&locked, &unlocked] {
            std::fs::create_dir_all(crate_dir).unwrap();
            std::fs::write(crate_dir.join("Cargo.toml"), "[package]\n").unwrap();
        }
        std::fs::write(locked.join("Cargo.lock"), "version = 4\n").unwrap();

        copy_cargo_locks(&[locked.clone(), unlocked.clone()]);
        assert_eq!(
            std::fs::read_to_string(locked.join(RUST_ANALYZER_LOCKFILE)).unwrap(),
            "version = 4\n"
        );
        assert!(!unlocked.join(RUST_ANALYZER_LOCKFILE).exists());
        assert!(!unlocked.join("Cargo.lock").exists());
        assert_eq!(RUST_ANALYZER_LOCKFILE, "target/rust-analyzer/Cargo.lock");
    }

    /// Metals' `.metals/`, `.bloop/` and `.bsp/`, the `project/.bloop/` of an
    /// sbt build's definition, and the `.gradle/` of each Gradle build
    /// kotlin-lsp or jdtls imports are made ignored before the server starts,
    /// where missing:
    /// of a Kotlin build for kotlin-lsp, of a Java one for jdtls, at the root
    /// of a multi-project build, and none for a Maven build. One the user's
    /// own server made is left as it is.
    #[test]
    fn a_servers_state_directories_are_ignored_by_git() {
        let root = tempfile::tempdir().unwrap();
        let write = |rel: &str| {
            let path = root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        write("analytics/build.sbt");
        write("billing/build.sbt");
        write("billing/.metals/metals.mv.db");
        write("mill/build.mill");
        write("reefer/build.gradle.kts");
        write("reefer/src/main/kotlin/SetPoint.kt");
        write("platform/settings.gradle");
        write("platform/core/build.gradle");
        write("platform/core/src/main/java/Core.java");
        write("berths/pom.xml");
        write("berths/src/main/java/Berth.java");
        let at = |rel: &str| root.path().join(rel);

        ignore_state_dirs("scala", &[at("analytics"), at("billing"), at("mill")]);
        for dir in [".metals", ".bloop", ".bsp", "project/.bloop"] {
            assert_eq!(
                std::fs::read_to_string(at(&format!("analytics/{dir}/.gitignore"))).unwrap(),
                "*\n"
            );
        }
        assert!(!at("billing/.metals/.gitignore").exists(), "the user's own");
        assert!(at("mill/.bloop/.gitignore").is_file());
        assert!(!at("mill/project").exists(), "not an sbt build");

        ignore_state_dirs("kotlin", &[root.path().to_path_buf()]);
        assert!(at("reefer/.gradle/.gitignore").is_file());
        assert!(!at("platform/.gradle").exists(), "no Kotlin in it");

        ignore_state_dirs("java", &[root.path().to_path_buf()]);
        assert!(at("platform/.gradle/.gitignore").is_file());
        assert!(!at("platform/core/.gradle").exists(), "its root's");
        assert!(!at("berths/.gradle").exists(), "a Maven build");
    }

    /// jdtls compiles nothing into the project: it answers from the sources,
    /// and wrote Maven's `target/classes` and a Gradle build's `bin/`.
    #[test]
    fn jdtls_starts_without_a_build_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let mut jdtls = crate::config::LspServerConfig::pyright();
        jdtls.language_id = "java".into();
        let options = initialization_options(&jdtls, &[root.path().to_path_buf()]).unwrap();
        assert_eq!(
            options["settings"]["java"]["autobuild"]["enabled"],
            serde_json::json!(false)
        );
        assert!(options["settings"]["java"]["import"]["exclusions"].is_array());
    }

    /// jdtls is kept from builds without Java: a Kotlin-only Gradle build is
    /// excluded, while a Java Maven build, a Java Gradle build written in the
    /// Kotlin DSL and the parent of a Java module stay imported, as do jdtls's
    /// own default exclusions.
    #[test]
    fn jdtls_imports_only_the_builds_that_hold_java() {
        let root = tempfile::tempdir().unwrap();
        let write = |rel: &str| {
            let path = root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        write("services/berths-java/pom.xml");
        write("services/berths-java/src/main/java/Berth.java");
        write("services/reefer-kotlin/build.gradle.kts");
        write("services/reefer-kotlin/src/main/kotlin/SetPoint.kt");
        write("services/ledger/build.gradle.kts");
        write("services/ledger/src/main/java/Ledger.java");
        write("platform/settings.gradle");
        write("platform/core/build.gradle");
        write("platform/core/src/main/java/Core.java");
        let exclusions = jdtls_import_exclusions(&[root.path().to_path_buf()]);
        let kotlin = "**/services/reefer-kotlin/**".to_string();
        assert!(exclusions.contains(&kotlin), "{exclusions:?}");
        for kept in [
            "services/berths-java",
            "services/ledger",
            "platform",
            "platform/core",
        ] {
            let pattern = format!("**/{kept}/**");
            assert!(!exclusions.contains(&pattern), "{kept}: {exclusions:?}");
        }
        assert!(exclusions.contains(&"**/services/reefer-kotlin".to_string()));
        assert_eq!(
            exclusions.len(),
            JDTLS_DEFAULT_EXCLUSIONS.len() + 2,
            "{exclusions:?}"
        );
        assert!(exclusions.iter().any(|p| p == "**/node_modules/**"));

        let config = crate::config::LspServerConfig {
            language_id: "java".into(),
            command: "jdtls".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["java".into()],
            heuristics: None,
            initialization_options: Some(serde_json::json!({ "bundles": [] })),
            settings: crate::config::ServerSettings::default(),
            configuration: None,
        };
        let options = initialization_options(&config, &[root.path().to_path_buf()]).unwrap();
        assert_eq!(options["bundles"], serde_json::json!([]));
        assert!(
            options["settings"]["java"]["import"]["exclusions"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(kotlin))
        );
    }

    /// The temporary directories of a Devscriptor process that is gone are
    /// removed; those of a running one, and others, are not.
    #[cfg(unix)]
    #[test]
    fn the_temporary_directories_of_a_process_that_is_gone_are_removed() {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let mut done = std::process::Command::new("true").spawn().unwrap();
        let gone = done.id();
        done.wait().unwrap();
        let orphan = root
            .path()
            .join(format!("{TEMP_DIR_PREFIX}{gone}-python-abc"));
        let ours = root
            .path()
            .join(format!("{TEMP_DIR_PREFIX}{}-rust-def", std::process::id()));
        let other = root.path().join("devscriptor-something-else");
        for dir in [&orphan, &ours, &other] {
            std::fs::create_dir_all(dir.join("proc-macro-srv")).unwrap();
        }
        let owner = std::fs::metadata(root.path()).unwrap().uid();

        sweep_orphaned_temp_dirs_in(root.path(), owner);
        assert!(!orphan.exists());
        assert!(ours.exists());
        assert!(other.exists());
    }

    /// Positions and edits cross Devscriptor unconverted, so the client must
    /// offer the one encoding they are counted in, and no other.
    #[test]
    fn only_utf16_positions_are_offered() {
        let general = client_capabilities().general.expect("general capabilities");
        assert_eq!(
            general.position_encodings,
            Some(vec![lsp_types::PositionEncodingKind::UTF16])
        );
    }

    #[test]
    fn a_server_that_answers_with_another_encoding_is_detected() {
        let mut capabilities = lsp_types::ServerCapabilities::default();
        assert_eq!(
            foreign_position_encoding(&capabilities),
            None,
            "unset is UTF-16"
        );

        capabilities.position_encoding = Some(lsp_types::PositionEncodingKind::UTF16);
        assert_eq!(foreign_position_encoding(&capabilities), None);

        capabilities.position_encoding = Some(lsp_types::PositionEncodingKind::UTF8);
        assert_eq!(
            foreign_position_encoding(&capabilities),
            Some(&lsp_types::PositionEncodingKind::UTF8)
        );
    }

    /// A server that says its build has expired is to be installed again,
    /// which brings it up to its current build.
    #[test]
    fn an_expired_build_is_to_be_installed_again() {
        let expired = "Log file: /c/kotlin-lsp/system/log/intellij-server.log; \
                       This build of kotlin-server has expired.; The IDE will now close.";
        assert_eq!(
            expired_build_hint("kotlin", expired),
            " `devscriptor lsp setup kotlin` installs its current build."
        );
        let unstopped = format!(
            "{expired}; Please download a new build from https://github.com/kotlin/kotlin-lsp"
        );
        assert_eq!(
            expired_build_hint("kotlin", &unstopped),
            ". `devscriptor lsp setup kotlin` installs its current build."
        );
        assert_eq!(
            expired_build_hint("kotlin", "Error: JAVA_HOME is not set"),
            ""
        );
    }
}
