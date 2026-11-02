//! The `devscriptor serve` command: one Devscriptor process per workspace.

use std::path::{Path, PathBuf};

use devscriptor_mcp::daemon::{Endpoint, RotatingLog};

use super::mcp::LspStartup;

/// Run the daemon for `workspace`: the index, its watcher, the embedding model
/// and the language servers, made once and shared by every MCP session that
/// connects to the workspace's socket, until a signal stops it.
///
/// `database` is the global `--database`; without it, the shared default
/// `devscriptor mcp` opens. Nothing is written to stdout: the log goes to
/// [`log`] (see [`devscriptor_mcp::daemon::serve`]).
///
/// # Errors
/// Returns an error if the server cannot start or its socket cannot be
/// listened on.
pub async fn execute_serve(
    workspace: PathBuf,
    database: Option<PathBuf>,
    config: Option<PathBuf>,
    lsp_startup: Option<LspStartup>,
    no_watch: bool,
) -> anyhow::Result<()> {
    let options = devscriptor_mcp::ServerOptions {
        lsp: devscriptor_mcp::LifecycleOverrides {
            startup: lsp_startup.map(Into::into),
        },
        no_watch,
        config,
    };
    devscriptor_mcp::daemon::serve(workspace, resolve_database(database), options).await
}

/// The database the daemon opens: `database` when given, else the shared
/// default of `devscriptor mcp`.
#[must_use]
pub fn resolve_database(database: Option<PathBuf>) -> PathBuf {
    database.unwrap_or_else(devscriptor_mcp::DevscriptorServerState::default_db_path)
}

/// The log of the daemon for `workspace` and `database` (the global
/// `--database`): `<key>.log` in the runtime directory, rotated at 10 MB.
///
/// # Errors
/// Returns an error if the runtime directory or the log cannot be created.
pub fn log(workspace: &Path, database: Option<PathBuf>) -> std::io::Result<RotatingLog> {
    let endpoint = Endpoint::new(workspace, &resolve_database(database))?;
    RotatingLog::open(endpoint.log())
}
