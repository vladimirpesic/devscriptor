//! One Devscriptor process per workspace.
//!
//! `devscriptor serve` holds what is costly to have twice — the index and its
//! connections, the watcher, the embedding model, the language servers — and
//! serves every MCP client of one workspace over a local socket, one session
//! per connection. Each [`Endpoint`] names the socket, lock, sidecar and log
//! of the daemon for one workspace and database.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::unified_server::{ServerOptions, UnifiedMcpServer};

/// How many hex digits of the SHA-256 the key keeps: 128 bits. The full 64
/// would put the socket past the 104 bytes macOS allows a socket path, in a
/// home directory of an ordinary length.
const KEY_DIGITS: usize = 32;

/// The size past which [`RotatingLog`] starts its file afresh.
const LOG_LIMIT: u64 = 10 * 1024 * 1024;

/// Environment variable: how many seconds the daemon waits with no session
/// open before it stops ([`DEFAULT_IDLE`] when unset).
pub const IDLE_ENV_VAR: &str = "DEVSCRIPTOR_DAEMON_IDLE_SECS";

/// How long the daemon waits with no session open before it stops.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(600);

/// Where the daemon of one workspace and database listens, and keeps its
/// lock, sidecar and log: files named after its key in [`runtime_dir`].
#[derive(Debug, Clone)]
pub struct Endpoint {
    key: String,
    dir: PathBuf,
}

impl Endpoint {
    /// The endpoint of the daemon for `workspace` and `database`, in
    /// [`runtime_dir`], which it creates.
    ///
    /// # Errors
    /// Returns an error if the runtime directory cannot be created.
    pub fn new(workspace: &Path, database: &Path) -> io::Result<Self> {
        Ok(Self {
            key: key(workspace, database),
            dir: runtime_dir()?,
        })
    }

    /// The key: the first 32 hex digits of the SHA-256 of the canonical
    /// workspace path, a NUL byte and the canonical database path.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The daemon's Unix socket, `<key>.sock`.
    #[must_use]
    pub fn socket(&self) -> PathBuf {
        self.dir.join(format!("{}.sock", self.key))
    }

    /// The daemon's named pipe on Windows, `\\.\pipe\devscriptor-<key>`.
    #[must_use]
    pub fn pipe(&self) -> String {
        format!(r"\\.\pipe\devscriptor-{}", self.key)
    }

    /// The lock the daemon holds for its whole life, `<key>.lock`.
    #[must_use]
    pub fn lock(&self) -> PathBuf {
        self.dir.join(format!("{}.lock", self.key))
    }

    /// The daemon's sidecar, `<key>.json`: its pid, version and start time.
    #[must_use]
    pub fn sidecar(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.key))
    }

    /// The daemon's log, `<key>.log`.
    #[must_use]
    pub fn log(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.key))
    }
}

/// The key of the daemon for `workspace` and `database` ([`Endpoint::key`]).
#[must_use]
pub fn key(workspace: &Path, database: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical(workspace).as_os_str().as_encoded_bytes());
    hasher.update([0]);
    hasher.update(canonical(database).as_os_str().as_encoded_bytes());
    let mut key = hex::encode(hasher.finalize());
    key.truncate(KEY_DIGITS);
    key
}

/// `path` as one spelling for every caller: canonical when it exists; when
/// only its directory does (a database not created yet), that directory
/// canonical with the file's name; else absolute.
#[must_use]
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = dunce::canonicalize(path) {
        return path;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = dunce::canonicalize(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
    {
        return parent.join(name);
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The directory of the daemons' sockets, locks, sidecars and logs, created
/// if missing, private to the user on Unix (mode 0700):
/// `$XDG_RUNTIME_DIR/devscriptor`, else `~/.cache/devscriptor/run` (Linux and
/// macOS), or `devscriptor\run` in the local application data directory on
/// Windows.
///
/// # Errors
/// Returns an error if no home directory is known, or the directory cannot be
/// created or made private.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("devscriptor"),
        None if cfg!(windows) => dirs::data_local_dir()
            .ok_or_else(|| io::Error::other("no local application data directory"))?
            .join("devscriptor")
            .join("run"),
        None => dirs::home_dir()
            .ok_or_else(|| io::Error::other("no home directory"))?
            .join(".cache")
            .join("devscriptor")
            .join("run"),
    };
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// A log file renamed to `<name>.1`, replacing the one before, and started
/// afresh once it would pass 10 MB. Clones write to the same file.
#[derive(Clone)]
pub struct RotatingLog(Arc<LogFile>);

/// The file behind a [`RotatingLog`].
struct LogFile {
    path: PathBuf,
    /// The open file and how many bytes it holds.
    file: Mutex<(std::fs::File, u64)>,
}

impl RotatingLog {
    /// Open `path` for appending, creating it if needed.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self(Arc::new(LogFile {
            path,
            file: Mutex::new((file, size)),
        })))
    }
}

impl LogFile {
    /// Write `buf` as one piece, after starting the file afresh if it would
    /// pass the limit.
    fn append(&self, buf: &[u8]) -> io::Result<()> {
        let mut file = self.file.lock();
        if file.1 > 0 && file.1 + buf.len() as u64 > LOG_LIMIT {
            let mut rotated = self.path.clone().into_os_string();
            rotated.push(".1");
            std::fs::rename(&self.path, rotated)?;
            *file = (
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
                0,
            );
        }
        file.0.write_all(buf)?;
        file.1 += buf.len() as u64;
        Ok(())
    }
}

/// One event's writer into a [`RotatingLog`].
pub struct LogWriter(Arc<LogFile>);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.append(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.file.lock().0.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingLog {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

/// Run the daemon for `workspace` and `database` until a signal stops it, or
/// no session has been open for [`IDLE_ENV_VAR`] seconds: what
/// `devscriptor serve` runs. The server — its index, watcher, embedding
/// model and language servers — is made once, and every accepted connection
/// is an MCP session of its own on it.
///
/// One daemon per endpoint: one that cannot take the endpoint's lock, held
/// by a daemon already serving or starting, leaves at once. The endpoint is
/// listened on only once the server is made: a client that connects is
/// served, and one that finds no endpoint can fall back to a server of its
/// own. Stopping, the daemon stops its language servers and its watcher,
/// checkpoints the index, and removes its socket, sidecar and lock.
///
/// # Errors
/// Returns an error if the server cannot be made or the endpoint cannot be
/// listened on.
pub async fn serve(
    workspace: PathBuf,
    database: PathBuf,
    options: ServerOptions,
) -> anyhow::Result<()> {
    let workspace = canonical(&workspace);
    let database = canonical(&database);
    let endpoint = Endpoint::new(&workspace, &database)?;
    let Some(lock) = InstanceLock::acquire(&endpoint)? else {
        tracing::info!(
            "Another daemon serves {} already; leaving",
            workspace.display()
        );
        return Ok(());
    };
    // Removes the sidecar and the lock however the daemon ends.
    let _instance = Instance {
        endpoint: endpoint.clone(),
        lock: Some(lock),
    };
    Sidecar::current().write(&endpoint)?;
    crate::server::serve_as_daemon();
    let idle = idle_limit();

    let watch = options.watch();
    let server =
        UnifiedMcpServer::new_with_lsp(Some(database.clone()), &workspace, options).await?;
    let mut listener = Listener::bind(&endpoint).await?;
    if watch {
        let state = Arc::clone(server.state());
        let root = workspace.clone();
        tokio::spawn(async move {
            state.watch_workspace(&root).await;
        });
    }
    tracing::info!(
        "Serving {} (index {}) at {}; stopping after {} s with no session",
        workspace.display(),
        database.display(),
        listener.address(),
        idle.as_secs()
    );

    // How many sessions are open: the daemon stops once none has been for
    // `idle`.
    let (open, mut opened) = tokio::sync::watch::channel(0_usize);
    let open = Arc::new(open);
    let stop = stop_signal();
    tokio::pin!(stop);
    loop {
        let unused = *opened.borrow_and_update() == 0;
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let server = server.clone();
                    let open = Arc::clone(&open);
                    open.send_modify(|sessions| *sessions += 1);
                    tokio::spawn(async move {
                        use rmcp::service::ServiceExt;
                        match server.serve(stream).await {
                            Ok(session) => {
                                let _ = session.waiting().await;
                            }
                            Err(e) => tracing::warn!("A session failed to start: {e}"),
                        }
                        open.send_modify(|sessions| *sessions -= 1);
                    });
                }
                Err(e) => tracing::warn!("Accepting a connection failed: {e}"),
            },
            _ = opened.changed() => {}
            () = tokio::time::sleep(idle), if unused => {
                tracing::info!("No session for {} s", idle.as_secs());
                break;
            }
            () = &mut stop => break,
        }
    }

    tracing::info!("Stopping the daemon for {}", workspace.display());
    server.shutdown().await;
    listener.close();
    Ok(())
}

/// How long the daemon waits with no session open: [`IDLE_ENV_VAR`]
/// seconds, else [`DEFAULT_IDLE`].
fn idle_limit() -> Duration {
    match std::env::var(IDLE_ENV_VAR) {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(secs) => Duration::from_secs(secs),
            Err(_) => {
                tracing::warn!(
                    "{IDLE_ENV_VAR}={value:?} is not a number of seconds; using {}",
                    DEFAULT_IDLE.as_secs()
                );
                DEFAULT_IDLE
            }
        },
        Err(_) => DEFAULT_IDLE,
    }
}

/// A running daemon's claim on its endpoint: dropped, however the daemon
/// ends, it removes the sidecar and then releases and removes the lock.
struct Instance {
    endpoint: Endpoint,
    lock: Option<InstanceLock>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let sidecar = self.endpoint.sidecar();
        if let Err(e) = std::fs::remove_file(&sidecar)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("Could not remove {}: {e}", sidecar.display());
        }
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
    }
}

/// The lock that makes a daemon the only one of its endpoint, held for its
/// whole life (`<key>.lock`).
pub struct InstanceLock {
    file: std::fs::File,
    path: PathBuf,
}

impl InstanceLock {
    /// Take the endpoint's lock; `None` when another process holds it.
    ///
    /// A daemon stopping removes the lock file, then releases it, so a lock
    /// taken just then may be on the file it removed, which no one else
    /// looks at: the lock is taken again until it is on the file the path
    /// names.
    ///
    /// # Errors
    /// Returns an error if the lock file cannot be opened or locked.
    pub fn acquire(endpoint: &Endpoint) -> io::Result<Option<Self>> {
        let path = endpoint.lock();
        loop {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
            if is_file_at(&file, &path) {
                return Ok(Some(Self { file, path }));
            }
        }
    }

    /// Whether a daemon holds the endpoint's lock: one serves, or starts.
    ///
    /// # Errors
    /// Returns an error if the lock file cannot be opened or locked.
    pub fn held(endpoint: &Endpoint) -> io::Result<bool> {
        // A lock taken here is let go at once, its file left in place.
        Ok(Self::acquire(endpoint)?.is_none())
    }

    /// Remove the lock file, then release the lock.
    fn release(self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!("Could not remove {}: {e}", self.path.display());
        }
        drop(self.file);
    }
}

/// Whether `file` is the file `path` names now.
fn is_file_at(file: &std::fs::File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), std::fs::metadata(path)) {
            (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        // Windows deletes an open file as Unix does: std opens files with
        // `FILE_SHARE_DELETE`, and NTFS removes the name at once.
        let named = std::fs::File::open(path).ok();
        match (file_id(file), named.as_ref().and_then(file_id)) {
            (Some(open), Some(named)) => open == named,
            _ => false,
        }
    }
}

/// Which file `file` is: the serial number of its volume and its index there.
#[cfg(windows)]
fn file_id(file: &std::fs::File) -> Option<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is that of an open file, and `info` outlives the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return None;
    }
    Some((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

/// What a daemon records about itself in its endpoint's sidecar
/// (`<key>.json`), written once it holds the endpoint's lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The daemon's process id.
    pub pid: u32,
    /// The Devscriptor version it runs.
    pub version: String,
    /// When it started, in seconds since the Unix epoch.
    pub started: u64,
}

impl Sidecar {
    /// This process's record.
    fn current() -> Self {
        Self {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
        }
    }

    /// Write the record to the endpoint's sidecar, replacing it at once.
    fn write(&self, endpoint: &Endpoint) -> io::Result<()> {
        let path = endpoint.sidecar();
        let mut temporary = path.clone().into_os_string();
        temporary.push(".tmp");
        std::fs::write(&temporary, serde_json::to_vec(self)?)?;
        std::fs::rename(&temporary, &path)
    }

    /// The record in the endpoint's sidecar, if there is a readable one.
    #[must_use]
    pub fn read(endpoint: &Endpoint) -> Option<Self> {
        let content = std::fs::read(endpoint.sidecar()).ok()?;
        serde_json::from_slice(&content).ok()
    }
}

/// Stop the daemon at `endpoint` if it runs another version than this
/// process, and wait at most `limit` for it to release its lock: clients are
/// served by the version they run. Nothing is done when no daemon holds the
/// lock: a sidecar left by one that ended abruptly names no live process.
/// Only a process of the same user can be stopped: SIGTERM on Unix,
/// `taskkill` (`TerminateProcess`) on Windows.
///
/// # Errors
/// Returns an error if the daemon cannot be stopped, or still holds its lock
/// after `limit`.
pub async fn stop_outdated(endpoint: &Endpoint, limit: Duration) -> anyhow::Result<()> {
    let Some(sidecar) = Sidecar::read(endpoint) else {
        return Ok(());
    };
    if sidecar.version == env!("CARGO_PKG_VERSION") || !InstanceLock::held(endpoint)? {
        return Ok(());
    }
    tracing::info!(
        "Stopping the daemon of Devscriptor {} (pid {}) to start {}",
        sidecar.version,
        sidecar.pid,
        env!("CARGO_PKG_VERSION")
    );
    terminate(sidecar.pid)?;
    let deadline = tokio::time::Instant::now() + limit;
    while InstanceLock::held(endpoint)? {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "the daemon of Devscriptor {} (pid {}) still runs {} s after it was asked to stop",
                sidecar.version,
                sidecar.pid,
                limit.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Ask process `pid` to stop.
fn terminate(pid: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(pid)?);
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        anyhow::ensure!(status.success(), "taskkill /PID {pid} failed: {status}");
        Ok(())
    }
}

/// A client's connection to a daemon.
#[cfg(unix)]
pub type Connection = tokio::net::UnixStream;

/// A client's connection to a daemon.
#[cfg(windows)]
pub type Connection = tokio::net::windows::named_pipe::NamedPipeClient;

/// Connect to the daemon at `endpoint`, if it is a daemon of this process's
/// user. On Unix, the process serving the socket must run as this user. On
/// Windows, where any user may create a pipe of any free name, the process
/// serving the pipe must be the daemon the endpoint's sidecar names, a file
/// only this user can write.
///
/// # Errors
/// Returns an error if no daemon listens there, none can take another
/// connection right now, or another process serves the endpoint.
pub async fn connect(endpoint: &Endpoint) -> io::Result<Connection> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(endpoint.socket()).await?;
        let user = stream.peer_cred()?.uid();
        if user != nix::unistd::getuid().as_raw() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is served by user {user}", endpoint.socket().display()),
            ));
        }
        Ok(stream)
    }
    #[cfg(windows)]
    {
        let pipe = tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint.pipe())?;
        let server = server_process(&pipe)?;
        if Sidecar::read(endpoint).is_some_and(|daemon| daemon.pid == server) {
            Ok(pipe)
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is served by process {server}, not by the workspace's daemon",
                    endpoint.pipe()
                ),
            ))
        }
    }
}

/// The id of the process at the server end of `pipe`.
#[cfg(windows)]
fn server_process(pipe: &tokio::net::windows::named_pipe::NamedPipeClient) -> io::Result<u32> {
    use std::os::windows::io::AsRawHandle;
    let mut pid = 0;
    // SAFETY: the handle is that of an open pipe, and `pid` outlives the call.
    let found = unsafe {
        windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
            pipe.as_raw_handle(),
            &mut pid,
        )
    };
    if found == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

/// Copy this process's stdin to `connection` and what comes back to its
/// stdout, until either end closes: a proxy's whole work. The bytes are not
/// read as JSON, and nothing else is written to stdout.
///
/// # Errors
/// Returns an error if reading or writing either end fails.
pub async fn relay(mut connection: Connection) -> io::Result<()> {
    let mut stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    tokio::io::copy_bidirectional(&mut stdio, &mut connection).await?;
    Ok(())
}

/// Completes on Ctrl-C, or on SIGTERM on Unix.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!("SIGTERM cannot be handled ({e}); stopping on Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The daemon's end of the endpoint: its Unix socket, which only the user
/// the daemon runs as may use.
#[cfg(unix)]
struct Listener {
    inner: tokio::net::UnixListener,
    path: PathBuf,
    /// The user the daemon runs as: the only one whose connections it serves.
    uid: u32,
}

#[cfg(unix)]
impl Listener {
    /// Listen on the endpoint's socket, readable and writable by its owner
    /// alone (mode 0600, in a directory of mode 0700). The daemon holds the
    /// endpoint's lock, so a socket file there is left from one that stopped
    /// without removing it, and is replaced.
    async fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let path = endpoint.socket();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let inner = tokio::net::UnixListener::bind(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "cannot listen on {}: {e} (a socket path has room for about 100 \
                     bytes; set XDG_RUNTIME_DIR to a shorter directory)",
                    path.display()
                ),
            )
        })?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            inner,
            path,
            uid: nix::unistd::getuid().as_raw(),
        })
    }

    /// The next connection of the daemon's own user. One from another user
    /// (`SO_PEERCRED` on Linux, `getpeereid` on macOS) is closed.
    async fn accept(&mut self) -> io::Result<tokio::net::UnixStream> {
        loop {
            let (stream, _) = self.inner.accept().await?;
            match stream.peer_cred() {
                Ok(peer) if peer.uid() == self.uid => return Ok(stream),
                Ok(peer) => tracing::warn!("Refused a connection of user {}", peer.uid()),
                Err(e) => tracing::warn!("Refused a connection whose user is unknown: {e}"),
            }
        }
    }

    fn address(&self) -> String {
        self.path.display().to_string()
    }

    /// Stop listening and remove the socket.
    fn close(self) {
        drop(self.inner);
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!("Could not remove {}: {e}", self.path.display());
        }
    }
}

/// The daemon's end of the endpoint: its named pipe, one instance per
/// connection. Remote clients are refused. The pipe keeps the default
/// security of a named pipe, under which other users may open it only for
/// reading, so they can send no request and add no instance. Its first
/// instance is exclusive: the daemon does not serve a pipe another process
/// made first, and clients refuse such a pipe (see [`connect`]).
#[cfg(windows)]
struct Listener {
    name: String,
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl Listener {
    /// Create the endpoint's pipe, whose first instance is exclusive.
    async fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = endpoint.pipe();
        let next = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("cannot create {name}, which another process may have made: {e}"),
                )
            })?;
        Ok(Self { name, next })
    }

    async fn accept(&mut self) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        use tokio::net::windows::named_pipe::ServerOptions;
        self.next.connect().await?;
        let waiting = ServerOptions::new().create(&self.name)?;
        Ok(std::mem::replace(&mut self.next, waiting))
    }

    fn address(&self) -> String {
        self.name.clone()
    }

    /// Stop listening: the pipe goes with its last instance.
    fn close(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_names_the_workspace_and_database() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        let database = workspace.join("devscriptor.db");
        let key = key(&workspace, &database);
        assert_eq!(key.len(), KEY_DIGITS);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        // The same files, spelled otherwise, give the same key.
        let relative = workspace.join("..").join("project");
        assert_eq!(key, super::key(&relative, &relative.join("devscriptor.db")));
        // Another database of the same workspace is another daemon.
        assert_ne!(key, super::key(&workspace, &dir.path().join("other.db")));
        assert_ne!(key, super::key(dir.path(), &database));
    }

    #[test]
    fn a_missing_database_is_named_by_its_canonical_directory() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join(".").join("index.db");
        assert_eq!(
            canonical(&named),
            dunce::canonicalize(dir.path()).unwrap().join("index.db")
        );
    }

    /// An endpoint in `dir`, for tests that must not touch the user's
    /// runtime directory.
    fn endpoint_in(dir: &Path) -> Endpoint {
        Endpoint {
            key: "0123456789abcdef0123456789abcdef".to_string(),
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn the_lock_admits_one_daemon_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_in(dir.path());
        assert!(!InstanceLock::held(&endpoint).unwrap());
        let lock = InstanceLock::acquire(&endpoint)
            .unwrap()
            .expect("a free lock");
        assert!(InstanceLock::acquire(&endpoint).unwrap().is_none());
        assert!(InstanceLock::held(&endpoint).unwrap());
        lock.release();
        assert!(!endpoint.lock().exists(), "a released lock is removed");
        assert!(!InstanceLock::held(&endpoint).unwrap());
        assert!(InstanceLock::acquire(&endpoint).unwrap().is_some());
    }

    /// A lock taken on a file that was removed meanwhile is no lock: the next
    /// daemon makes a new file at the path.
    #[test]
    fn a_removed_lock_file_is_not_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let file = std::fs::File::create(&path).unwrap();
        assert!(is_file_at(&file, &path));
        std::fs::remove_file(&path).unwrap();
        assert!(!is_file_at(&file, &path));
        std::fs::File::create(&path).unwrap();
        assert!(!is_file_at(&file, &path), "another file now has the name");
    }

    #[test]
    fn the_sidecar_names_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint_in(dir.path());
        assert_eq!(Sidecar::read(&endpoint), None);
        let current = Sidecar::current();
        current.write(&endpoint).unwrap();
        assert_eq!(Sidecar::read(&endpoint), Some(current.clone()));
        assert_eq!(current.pid, std::process::id());
        assert_eq!(current.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_log_starts_afresh_past_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        let log = RotatingLog::open(path.clone()).unwrap();
        let line = vec![b'x'; 1024 * 1024];
        for _ in 0..10 {
            log.0.append(&line).unwrap();
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 10 * 1024 * 1024);
        log.0.append(b"next\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"next\n");
        assert_eq!(
            std::fs::metadata(dir.path().join("daemon.log.1"))
                .unwrap()
                .len(),
            10 * 1024 * 1024
        );
    }
}
