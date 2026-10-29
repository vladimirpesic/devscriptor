//! The `devscriptor mcp` command: run the unified MCP server.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use devscriptor_mcp::daemon;

/// Launch the embedded devscriptor MCP server.
///
/// The MCP server speaks JSON-RPC over **stdio** (there is no network port and no
/// `--port` flag). The implementation lives inside this universal binary, so AI
/// coding agents get the full **75-tool** unified server — 32 core (including 5
/// Context Graph and 3 watcher tools), 38 LSP, and 5 architecture tools — without
/// a separate executable.
///
/// Nothing may be written to stdout on this path: the banner is suppressed in
/// `main`, and the tracing subscriber writes to stderr. A single stray byte on
/// stdout corrupts the JSON-RPC stream.
///
/// This lives in its own module, gated on `mcp`, rather than in `commands::lsp`
/// (gated on `lsp`) so that `--features mcp` alone produces a working `mcp`
/// subcommand. `Commands::Mcp` is gated on the same feature, so a build without
/// the server has no `mcp` subcommand at all — instead of one that cannot work.
///
/// # Database
///
/// `db_path` is `Some` only when the user passed the global `--database` flag.
/// Left as `None`, the server opens its own shared default (typically
/// `~/.local/share/devscriptor/default.db`), which — unlike every other
/// subcommand's `./devscriptor.db` default — does not depend on the working
/// directory, so an agent can scan absolute paths from anywhere.
///
/// # Language servers
///
/// `lsp_startup` is `--lsp-startup`: when the language servers start. It
/// outranks `DEVSCRIPTOR_LSP_STARTUP` and the configuration file.
///
/// # Watching
///
/// The server watches the scanned project it runs in, so the index follows
/// file changes; `no_watch` (`--no-watch`) or `DEVSCRIPTOR_WATCH=0` turns
/// that off.
///
/// # One process per workspace
///
/// By default the server is the workspace's daemon (`devscriptor serve`),
/// shared by every client of the workspace and its database, and this
/// process only relays its stdio to it, starting the daemon when none
/// answers. `no_daemon` (`--no-daemon`) or `DEVSCRIPTOR_DAEMON=0` serves in
/// this process instead, as does a daemon that cannot be reached within
/// [`DAEMON_START_TIMEOUT`], with the reason on stderr. A daemon started
/// here takes this command's `--lsp-startup`, `--no-watch` and `--config`;
/// one already running keeps those it was started with.
///
/// # Errors
/// Returns an error if the server fails to start or the transport fails.
pub async fn execute_mcp(
    db_path: Option<PathBuf>,
    config: Option<PathBuf>,
    lsp_startup: Option<LspStartup>,
    no_watch: bool,
    no_daemon: bool,
) -> anyhow::Result<()> {
    let options = devscriptor_mcp::ServerOptions {
        lsp: devscriptor_mcp::LifecycleOverrides {
            startup: lsp_startup.map(Into::into),
        },
        no_watch,
        config: config.clone(),
    };
    if no_daemon || !daemon_enabled() {
        return devscriptor_mcp::run_unified_server(db_path, options).await;
    }
    let workspace = std::env::current_dir()?;
    let database = super::serve::resolve_database(db_path.clone());
    let started = DaemonOptions {
        lsp_startup,
        no_watch,
        config: config
            .as_deref()
            .map(|path| dunce::canonicalize(path).or_else(|_| std::path::absolute(path)))
            .transpose()?,
    };
    match daemon_connection(&workspace, &database, &started).await {
        Ok(connection) => Ok(devscriptor_mcp::daemon::relay(connection).await?),
        Err(e) => {
            tracing::warn!(
                "Serving in this process: the workspace's Devscriptor daemon could not be \
                 reached ({e:#})"
            );
            devscriptor_mcp::run_unified_server(db_path, options).await
        }
    }
}

/// Environment variable that, set to `0` or `false`, keeps `devscriptor mcp`
/// serving in its own process, as `--no-daemon` does.
pub const DAEMON_ENV_VAR: &str = "DEVSCRIPTOR_DAEMON";

/// How long `devscriptor mcp` waits for the daemon it started to answer.
pub const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `devscriptor mcp` relays to the workspace's daemon: unless
/// [`DAEMON_ENV_VAR`] is `0` or `false`.
fn daemon_enabled() -> bool {
    std::env::var(DAEMON_ENV_VAR).map_or(true, |value| {
        !matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false")
    })
}

/// What a daemon this command starts is started with.
struct DaemonOptions {
    lsp_startup: Option<LspStartup>,
    no_watch: bool,
    /// The project configuration file, made absolute: the daemon may be
    /// started by a client in another directory.
    config: Option<PathBuf>,
}

/// A connection to the daemon for `workspace` and `database`, waiting for it
/// at most [`DAEMON_START_TIMEOUT`]. A daemon of another version is stopped
/// first; one is started when none holds the endpoint's lock, while one that
/// holds it is still starting.
async fn daemon_connection(
    workspace: &Path,
    database: &Path,
    options: &DaemonOptions,
) -> anyhow::Result<daemon::Connection> {
    let endpoint = daemon::Endpoint::new(workspace, database)?;
    daemon::stop_outdated(&endpoint, DAEMON_START_TIMEOUT).await?;
    if let Ok(connection) = daemon::connect(&endpoint).await {
        return Ok(connection);
    }
    if !daemon::InstanceLock::held(&endpoint)? {
        start_daemon(&endpoint, workspace, database, options)?;
    }
    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    loop {
        match daemon::connect(&endpoint).await {
            Ok(connection) => return Ok(connection),
            Err(e) if Instant::now() >= deadline => anyhow::bail!(
                "none answered within {} s ({e}); its log is {}",
                DAEMON_START_TIMEOUT.as_secs(),
                endpoint.log().display()
            ),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Start `devscriptor serve` for `workspace` and `database`, detached from
/// this process: in a session of its own on Unix (`setsid`), a detached
/// process on Windows, with its output appended to its log.
fn start_daemon(
    endpoint: &daemon::Endpoint,
    workspace: &Path,
    database: &Path,
    options: &DaemonOptions,
) -> anyhow::Result<()> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(endpoint.log())?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("--database").arg(database);
    if let Some(config) = &options.config {
        command.arg("--config").arg(config);
    }
    command
        .args(["serve", "--workspace"])
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(startup) = options.lsp_startup {
        command.args([
            "--lsp-startup",
            match startup {
                LspStartup::Lazy => "lazy",
                LspStartup::Eager => "eager",
            },
        ]);
    }
    if options.no_watch {
        command.arg("--no-watch");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook runs in the forked child before `exec`, and calls
        // only `setsid`, which is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// No console, and its own process group: Ctrl-C in the client's
        /// console does not reach it.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut daemon = command.spawn()?;
    // Reap it once it exits. A daemon that finds another serving, or cannot
    // listen, leaves at once, and would stay a zombie of this process for as
    // long as the session lasts. One that outlives this process is reaped
    // by the system instead, and the thread ends with the process.
    std::thread::spawn(move || {
        let _ = daemon.wait();
    });
    Ok(())
}

/// `devscriptor mcp --lsp-startup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LspStartup {
    /// Start each language's server when a tool first needs it, or in the
    /// background once `lsp_arch_detect` finds the language.
    Lazy,
    /// Start every applicable server before answering `initialize`.
    Eager,
}

impl From<LspStartup> for devscriptor_mcp::StartupMode {
    fn from(startup: LspStartup) -> Self {
        match startup {
            LspStartup::Lazy => Self::Lazy,
            LspStartup::Eager => Self::Eager,
        }
    }
}
