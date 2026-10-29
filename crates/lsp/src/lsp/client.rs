//! LSP client implementation with async request/response handling.
//!
//! This module provides a high-level client for communicating with LSP servers,
//! handling request correlation, timeouts, and graceful shutdown.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration as StdDuration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, interval, timeout};
use tracing::{error, warn};

use crate::config::LspServerConfig;
use crate::error::{LspError, Result};
use crate::lsp::protocol::{InboundMessage, NotificationMessage, RequestId, RequestMessage};
use crate::lsp::transport::{LspReader, LspTransport, LspWriter};

/// JSON-RPC protocol version.
const JSONRPC_VERSION: &str = "2.0";

/// LSP `ContentModified`: the server declined because the document changed
/// while it was computing. The spec treats it as retryable, not fatal.
const LSP_CONTENT_MODIFIED: i32 = -32801;
/// LSP `ServerCancelled` (3.17): the server cancelled a request it may be
/// asked again, e.g. because newer input made the answer obsolete.
const LSP_SERVER_CANCELLED: i32 = -32802;
/// How many times to retry a `ContentModified` or `ServerCancelled` before
/// surfacing it.
const CONTENT_MODIFIED_RETRIES: u32 = 3;
/// Linear backoff between those retries.
const CONTENT_MODIFIED_BACKOFF: std::time::Duration = std::time::Duration::from_millis(150);

/// Type for response callbacks.
pub type ResponseCallback = oneshot::Sender<Result<Value>>;

/// LSP server state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Uninitialized state.
    Uninitialized,
    /// Initializing state.
    Initializing,
    /// Ready state.
    Ready,
    /// Shutting down.
    ShuttingDown,
    /// Shutdown complete.
    Shutdown,
}

/// Commands for client control.
#[derive(Debug)]
enum ClientCommand {
    /// Send a request and wait for response.
    SendRequest {
        /// Request to send.
        request: RequestMessage,
        /// Channel for receiving response.
        response_tx: ResponseCallback,
    },
    /// Send a notification (no response expected).
    SendNotification {
        /// Method name.
        method: String,
        /// Method parameters.
        params: Option<Value>,
    },
    /// Reply to a request the *server* sent us.
    SendResponse {
        /// The server's request id, echoed back.
        id: RequestId,
        /// Result payload.
        result: Value,
    },
    /// Shutdown the client.
    Shutdown,
}

/// Inner state of the LSP client.
struct LspClientInner {
    /// Pending requests awaiting responses, with insertion timestamps.
    pending_requests: HashMap<RequestId, (ResponseCallback, Instant)>,
    /// Whether the client is shutting down.
    shutdown: bool,
}

/// End a client whose server has stopped answering: refuse new commands, then
/// fail every request still waiting with `error`.
///
/// The command channel is closed first, so that by the time a caller learns
/// why its request failed the client reports itself closed and the bridge
/// restarts the server for the next one. The waiting requests fail at once,
/// rather than each sitting out its whole request timeout (30 s, 180 s for some
/// servers) for an answer that can no longer come.
async fn end_client(
    command_rx: &mut mpsc::Receiver<ClientCommand>,
    inner: &Mutex<LspClientInner>,
    error: impl Fn() -> LspError,
) {
    command_rx.close();
    let mut inner = inner.lock().await;
    inner.shutdown = true;
    for (_, (tx, _)) in inner.pending_requests.drain() {
        let _ = tx.send(Err(error()));
    }
    while let Ok(command) = command_rx.try_recv() {
        if let ClientCommand::SendRequest { response_tx, .. } = command {
            let _ = response_tx.send(Err(error()));
        }
    }
}

/// Maximum age of a pending request before it is evicted as stale.
const PENDING_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(300); // 5 minutes

/// How long [`LspClient::wait_until_ready`] will wait for background indexing.
///
/// Cold `rust-analyzer` on a large workspace can take tens of seconds. Callers
/// sit inside a single MCP tool call, whose timeout is larger than this.
const READY_TIMEOUT: StdDuration = StdDuration::from_secs(60);

/// Quiet period after the last `$/progress` before a server counts as idle.
const READY_SETTLE: StdDuration = StdDuration::from_millis(500);

/// How often [`LspClient::await_pushed_diagnostics`] looks for a new push.
const PUSH_POLL: StdDuration = StdDuration::from_millis(25);

/// If a server has reported no progress at all within this window, assume it does
/// not implement `$/progress` and treat it as ready.
const READY_NO_PROGRESS_GRACE: StdDuration = StdDuration::from_secs(2);

/// Tracks a server's background work, reported via `$/progress`.
///
/// Semantic requests (`hover`, `definition`, …) answer with *empty results* —
/// not errors — while a server is still indexing. An agent cannot tell that
/// apart from "no such symbol", so requests wait for the work to end.
#[derive(Debug)]
struct ProgressState {
    /// Number of `$/progress` tokens currently in `begin`..`end`.
    active: std::sync::atomic::AtomicUsize,
    /// Whether any `$/progress` has ever been observed.
    seen: std::sync::atomic::AtomicBool,
    /// When the last `$/progress` arrived.
    last: Mutex<Option<Instant>>,
}

impl ProgressState {
    fn new() -> Self {
        Self {
            active: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::atomic::AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }

    /// Fold a `$/progress` notification into the state.
    async fn observe(&self, params: Option<&Value>) {
        let Some(kind) = params
            .and_then(|p| p.get("value"))
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
        else {
            return;
        };

        self.seen.store(true, Ordering::SeqCst);
        *self.last.lock().await = Some(Instant::now());

        match kind {
            "begin" => {
                self.active.fetch_add(1, Ordering::SeqCst);
            }
            "end" => {
                // `try_update` floors at zero: a stray `end` must not underflow.
                let _ = self
                    .active
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        Some(n.saturating_sub(1))
                    });
            }
            _ => {}
        }
    }
}

/// Diagnostics the server pushed with `textDocument/publishDiagnostics`: the
/// latest for each document.
///
/// A server without pull diagnostics — typescript-language-server, clangd,
/// gopls and jdtls among them — only ever pushes them, so for those servers
/// these are what `get_diagnostics` answers with.
#[derive(Debug, Default)]
struct PushedDiagnostics {
    /// The latest push for each document, keyed by [`document_key`].
    latest: parking_lot::Mutex<HashMap<String, Push>>,
    /// How many pushes have arrived, for any document.
    count: std::sync::atomic::AtomicU64,
}

/// One push of diagnostics for a document.
#[derive(Debug, Clone)]
struct Push {
    /// Its place, from 1, in the order pushes arrived.
    number: u64,
    /// When it arrived.
    at: Instant,
    diagnostics: Vec<lsp_types::Diagnostic>,
}

impl PushedDiagnostics {
    /// Keep the diagnostics of a `textDocument/publishDiagnostics`.
    fn record(&self, params: Option<Value>) {
        let Some(params) = params.and_then(|params| {
            serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(params).ok()
        }) else {
            return;
        };
        let mut latest = self.latest.lock();
        let number = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        latest.insert(
            document_key(params.uri.as_str()),
            Push {
                number,
                at: Instant::now(),
                diagnostics: params.diagnostics,
            },
        );
    }

    fn latest(&self, key: &str) -> Option<Push> {
        self.latest.lock().get(key).cloned()
    }
}

/// A document's identity among pushed diagnostics: its decoded path. A server
/// may spell the URI of a document differently from the client that sent it —
/// typescript-language-server re-encodes it from the file's path.
fn document_key(uri: &str) -> String {
    let decode = |text: &str| {
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    };
    match uri.strip_prefix("file://") {
        // `file:///c%3A/a` and `file:///C:/a` are one document.
        Some(path) if cfg!(windows) => crate::bridge::translator::windows_path(&decode(path)),
        Some(path) => decode(path),
        None => decode(uri),
    }
}

/// How busy a client is: the operations in progress, and when it was last
/// used. Shared by every clone of the client.
#[derive(Debug)]
struct Activity {
    /// Operations in progress ([`InUse`] guards alive).
    in_use: AtomicUsize,
    /// When the client was created or last stopped being in use.
    last_used: parking_lot::Mutex<Instant>,
}

/// Marks a client in use while alive. An idle check never stops a server
/// whose client is in use; dropping the guard records the time of use.
#[derive(Debug)]
#[must_use = "the client counts as in use only while the guard is alive"]
pub struct InUse {
    activity: Arc<Activity>,
}

impl Drop for InUse {
    fn drop(&mut self) {
        // Stamp first, so whoever sees nothing in use sees this use.
        *self.activity.last_used.lock() = Instant::now();
        self.activity.in_use.fetch_sub(1, Ordering::SeqCst);
    }
}

/// LSP client with async request/response handling.
#[derive(Clone)]
pub struct LspClient {
    /// Channel for sending commands to the message loop.
    command_tx: mpsc::Sender<ClientCommand>,
    /// Server state.
    state: Arc<Mutex<ServerState>>,
    /// Request ID counter.
    request_counter: Arc<AtomicI64>,
    /// Background-work tracker, fed by the notification drain task.
    progress: Arc<ProgressState>,
    /// Diagnostics the server pushed, kept by the notification drain task.
    pushed: Arc<PushedDiagnostics>,
    /// Per-request timeout, from this server's configuration (`timeout_seconds`):
    /// a cold `csharp-ls` takes longer than 30 s to load a .NET project, so a
    /// fixed 30 s would fail its first `documentSymbol`.
    request_timeout: StdDuration,
    /// Operations in progress and the time of last use, for idle shutdown.
    activity: Arc<Activity>,
}

impl LspClient {
    /// Create a new LSP client from transport.
    ///
    /// # Panics
    ///
    /// Panics if the client cannot be created (should not happen in normal operation).
    #[must_use]
    pub fn from_transport(config: LspServerConfig, transport: LspTransport) -> Self {
        let request_timeout = StdDuration::from_secs(config.settings.timeout_seconds);
        let (notification_tx, notification_rx) = mpsc::channel(256);

        // Split the transport before spawning any tasks. `transport.receive()`
        // accumulates bytes into local state, so dropped mid-read in a
        // `tokio::select!` arm (whenever another arm wins) it would desynchronise the
        // `BufReader` cursor: the next call would start mid-message. The reader half
        // runs in its own task that loops unconditionally and forwards decoded
        // messages to `inbound_tx`. The message-dispatch task only ever selects
        // between *two channels* (`command_rx` and `inbound_rx`), both of which are
        // cancellation-safe.
        let (reader, writer) = transport.split();
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Result<InboundMessage>>(256);

        let inner_clone = Arc::new(Mutex::new(LspClientInner {
            pending_requests: HashMap::new(),
            shutdown: false,
        }));
        let state = Arc::new(Mutex::new(ServerState::Initializing));
        let request_counter = Arc::new(AtomicI64::new(1));

        let (command_tx, mut command_rx) = mpsc::channel(1024);

        let state_clone = state.clone();

        // Spawn dedicated reader task — never cancelled mid-message.
        let _reader_handle: JoinHandle<()> = tokio::spawn(async move {
            let mut reader: LspReader = reader;
            loop {
                match reader.receive().await {
                    Ok(msg) => {
                        if inbound_tx.send(Ok(msg)).await.is_err() {
                            break; // Dispatch task dropped — shutting down.
                        }
                    }
                    // A server that closed its stdout between messages has
                    // simply exited. Ending the stream lets the dispatcher take
                    // its normal end-of-stream path instead of reporting a
                    // transport failure that never happened.
                    Err(LspError::ServerClosed) => break,
                    Err(e) => {
                        // Forward the error then stop reading.
                        let _ = inbound_tx.send(Err(e)).await;
                        break;
                    }
                }
            }
        });

        // Spawn stale-request cleanup task.
        let cleanup_inner = inner_clone.clone();
        let _cleanup_handle: JoinHandle<()> = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let mut inner = cleanup_inner.lock().await;
                if inner.shutdown {
                    break;
                }
                let now = Instant::now();
                let stale: Vec<RequestId> = inner
                    .pending_requests
                    .iter()
                    .filter(|(_, (_, ts))| now.duration_since(*ts) > PENDING_REQUEST_TIMEOUT)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in stale {
                    if let Some((tx, _)) = inner.pending_requests.remove(&id) {
                        let _ = tx.send(Err(LspError::Timeout {
                            operation: "pending request".into(),
                            timeout_ms: PENDING_REQUEST_TIMEOUT.as_millis() as u64,
                        }));
                        warn!("Evicted stale pending request {:?}", id);
                    }
                }
            }
        });

        // Command-dispatch task — selects only on channels (both cancellation-safe).
        let _handle: JoinHandle<Result<()>> = tokio::spawn(async move {
            let mut writer: LspWriter = writer;
            loop {
                tokio::select! {
                    cmd = command_rx.recv() => {
                        match cmd {
                            Some(ClientCommand::SendRequest { request, response_tx }) => {
                                let mut inner = inner_clone.lock().await;
                                if inner.shutdown {
                                    let _ = response_tx.send(Err(LspError::Shutdown));
                                    continue;
                                }
                                inner.pending_requests.insert(
                                    request.id.clone(),
                                    (response_tx, Instant::now()),
                                );

                                let message = match serde_json::to_value(&request) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        let _ = inner
                                            .pending_requests
                                            .remove(&request.id)
                                            .map(|(tx, _)| tx.send(Err(LspError::Json(e.to_string()))));
                                        error!("Failed to serialize request: {}", e);
                                        continue;
                                    }
                                };

                                if let Err(e) = writer.send(&message).await {
                                    let _ = inner
                                        .pending_requests
                                        .remove(&request.id)
                                        .map(|(tx, _)| tx.send(Err(LspError::Transport(e.to_string()))));
                                    error!("Failed to send request: {}", e);
                                }
                            }
                            Some(ClientCommand::SendNotification { method, params }) => {
                                let notification = NotificationMessage {
                                    jsonrpc: JSONRPC_VERSION.into(),
                                    method,
                                    params,
                                };

                                let message = match serde_json::to_value(&notification) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        error!("Failed to serialize notification: {}", e);
                                        continue;
                                    }
                                };

                                let _inner = inner_clone.lock().await;
                                if let Err(e) = writer.send(&message).await {
                                    error!("Failed to send notification: {}", e);
                                }
                            }
                            Some(ClientCommand::SendResponse { id, result }) => {
                                let message = serde_json::json!({
                                    "jsonrpc": JSONRPC_VERSION,
                                    "id": id,
                                    "result": result,
                                });
                                let _inner = inner_clone.lock().await;
                                if let Err(e) = writer.send(&message).await {
                                    error!("Failed to answer server request: {}", e);
                                }
                            }
                            Some(ClientCommand::Shutdown) => {
                                let mut inner = inner_clone.lock().await;
                                inner.shutdown = true;
                                for (id, (tx, _)) in inner.pending_requests.drain() {
                                    let _ = tx.send(Err(LspError::Shutdown));
                                    warn!("Request {:?} cancelled due to shutdown", id);
                                }
                                let mut state = state_clone.lock().await;
                                *state = ServerState::Shutdown;
                                break Ok(());
                            }
                            None => break Ok(()),
                        }
                    }

                    // Channel receive — cancellation-safe (no raw I/O here).
                    result = inbound_rx.recv() => {
                        match result {
                            Some(Ok(InboundMessage::Response(response))) => {
                                let mut inner = inner_clone.lock().await;
                                if let Some((callback, _)) = inner.pending_requests.remove(&response.id) {
                                    let res = if let Some(error) = response.error {
                                        Err(LspError::LspServerError {
                                            code: error.code,
                                            message: without_stack(&error.message),
                                        })
                                    } else {
                                        Ok(response.result.unwrap_or(Value::Null))
                                    };
                                    let _ = callback.send(res);
                                } else {
                                    warn!("Received response for unknown request ID: {:?}", response.id);
                                }
                            }
                            Some(Ok(InboundMessage::Notification(notification))) => {
                                if let Err(e) = notification_tx.try_send(InboundMessage::Notification(notification)) {
                                    warn!("Failed to forward notification: {}", e);
                                }
                            }
                            Some(Ok(InboundMessage::Request(request))) => {
                                if let Err(e) = notification_tx.try_send(InboundMessage::Request(request)) {
                                    warn!("Failed to forward server request: {}", e);
                                }
                            }
                            Some(Err(e)) => {
                                error!("Transport error: {}", e);
                                let reason = e.to_string();
                                end_client(&mut command_rx, &inner_clone, || {
                                    LspError::Transport(reason.clone())
                                })
                                .await;
                                let mut state = state_clone.lock().await;
                                *state = ServerState::Shutdown;
                                break Err(e);
                            }
                            None => {
                                // Reader task exited (server closed stdout):
                                // no answer can come now.
                                end_client(&mut command_rx, &inner_clone, || LspError::ServerClosed)
                                    .await;
                                break Ok(());
                            }
                        }
                    }
                }
            }
        });

        // Drain server-initiated messages, so a full channel never blocks the
        // reader: `$/progress` goes into `progress`, pushed diagnostics into
        // `pushed`, and the rest is discarded.
        let progress = Arc::new(ProgressState::new());
        let progress_task = Arc::clone(&progress);
        let pushed = Arc::new(PushedDiagnostics::default());
        let pushed_task = Arc::clone(&pushed);
        let mut notification_rx = notification_rx;
        let responder = command_tx.clone();
        tokio::spawn(async move {
            while let Some(message) = notification_rx.recv().await {
                match message {
                    InboundMessage::Notification(n) if n.method == "$/progress" => {
                        progress_task.observe(n.params.as_ref()).await;
                    }
                    InboundMessage::Notification(n)
                        if n.method == "textDocument/publishDiagnostics" =>
                    {
                        pushed_task.record(n.params);
                    }
                    InboundMessage::Notification(_) => {}
                    InboundMessage::Request(request) => {
                        // A server request *blocks the server* until answered.
                        // `csharp-ls` sends `client/registerCapability` right after
                        // initialize and waits: with no reply it never loads the
                        // project, so every subsequent request timed out.
                        let result = Self::answer_server_request(&request);
                        let _ = responder
                            .send(ClientCommand::SendResponse {
                                id: request.id.clone(),
                                result,
                            })
                            .await;
                    }
                    InboundMessage::Response(_) => {}
                }
            }
        });

        Self {
            command_tx,
            state,
            request_counter,
            progress,
            pushed,
            request_timeout,
            activity: Arc::new(Activity {
                in_use: AtomicUsize::new(0),
                last_used: parking_lot::Mutex::new(Instant::now()),
            }),
        }
    }

    /// Mark the client in use until the returned guard is dropped. Requests,
    /// notifications and [`Self::wait_until_ready`] hold one; a caller holds
    /// one across a sequence of them.
    pub fn in_use(&self) -> InUse {
        self.activity.in_use.fetch_add(1, Ordering::SeqCst);
        *self.activity.last_used.lock() = Instant::now();
        InUse {
            activity: Arc::clone(&self.activity),
        }
    }

    /// How long the client has gone unused since it was created or last in
    /// use; `None` while it is in use.
    #[must_use]
    pub fn idle_for(&self) -> Option<StdDuration> {
        if self.activity.in_use.load(Ordering::SeqCst) > 0 {
            return None;
        }
        Some(self.activity.last_used.lock().elapsed())
    }

    /// Build a reply for a request the server sent us.
    ///
    /// The LSP spec has servers ask the client to register capabilities, create
    /// progress tokens, or fetch configuration. Devscriptor is a headless client
    /// with nothing to configure, so an empty-but-valid answer is correct — and
    /// far better than silence, which leaves the server waiting forever.
    fn answer_server_request(request: &RequestMessage) -> Value {
        match request.method.as_str() {
            // The client is asked for one settings object per requested item.
            "workspace/configuration" => {
                let n = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("items"))
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Value::Array(vec![Value::Null; n])
            }
            // Everything else — registerCapability, unregisterCapability,
            // workDoneProgress/create, semanticTokens/refresh, … — takes a null
            // result to mean "acknowledged".
            _ => Value::Null,
        }
    }

    /// Wait until the server has finished its background work.
    ///
    /// Returns as soon as no `$/progress` token is active and the server has been
    /// quiet for `READY_SETTLE`. A server that never reports progress is treated
    /// as ready after `READY_NO_PROGRESS_GRACE`. Gives up after
    /// `READY_TIMEOUT` and returns anyway — a slow answer beats no answer.
    pub async fn wait_until_ready(&self) {
        let _in_use = self.in_use();
        let started = Instant::now();
        let mut ticker = interval(StdDuration::from_millis(100));

        loop {
            ticker.tick().await;

            if started.elapsed() >= READY_TIMEOUT {
                warn!(
                    "LSP server still busy after {:?}; proceeding anyway",
                    READY_TIMEOUT
                );
                return;
            }

            let seen = self.progress.seen.load(Ordering::SeqCst);
            if !seen {
                if started.elapsed() >= READY_NO_PROGRESS_GRACE {
                    return; // Server does not report progress.
                }
                continue;
            }

            if self.progress.active.load(Ordering::SeqCst) > 0 {
                continue;
            }

            let quiet_for = self
                .progress
                .last
                .lock()
                .await
                .map(|t| t.elapsed())
                .unwrap_or(StdDuration::ZERO);
            if quiet_for >= READY_SETTLE {
                return;
            }
        }
    }

    /// How many diagnostics pushes have arrived: a mark that
    /// [`Self::await_pushed_diagnostics`] waits for a later push than.
    #[must_use]
    pub fn diagnostics_mark(&self) -> u64 {
        self.pushed.count.load(Ordering::SeqCst)
    }

    /// The diagnostics the server last pushed for `uri`, if it pushed any.
    #[must_use]
    pub fn pushed_diagnostics(&self, uri: &str) -> Option<Vec<lsp_types::Diagnostic>> {
        self.pushed
            .latest(&document_key(uri))
            .map(|push| push.diagnostics)
    }

    /// The diagnostics the server pushes for `uri` after `mark`.
    ///
    /// Waits up to `wait` for a push for `uri` later than `mark`, then until
    /// `uri` has had no push for `settle`, and returns the last one: servers
    /// push in waves — typescript-language-server sends syntax errors, then
    /// type errors, then suggestions — and only the last wave is complete. The
    /// settling ends after the request timeout even if pushes keep coming.
    /// `None` when no push came within `wait`.
    pub async fn await_pushed_diagnostics(
        &self,
        uri: &str,
        mark: u64,
        wait: StdDuration,
        settle: StdDuration,
    ) -> Option<Vec<lsp_types::Diagnostic>> {
        let key = document_key(uri);
        let deadline = Instant::now() + wait;
        let mut push = loop {
            match self.pushed.latest(&key) {
                Some(push) if push.number > mark => break push,
                _ if Instant::now() >= deadline => return None,
                _ => tokio::time::sleep(PUSH_POLL).await,
            }
        };
        let settle_deadline = Instant::now() + self.request_timeout;
        loop {
            let quiet = push.at.elapsed();
            if quiet >= settle || Instant::now() >= settle_deadline {
                return Some(push.diagnostics);
            }
            tokio::time::sleep(settle - quiet).await;
            if let Some(latest) = self.pushed.latest(&key) {
                push = latest;
            }
        }
    }

    /// Send a request and wait for response.
    ///
    /// # Type Parameters
    ///
    /// * `P` - The type of the request parameters
    /// * `R` - The type of the response result
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The server has shut down
    /// - The request times out
    /// - The response cannot be deserialized
    /// - The LSP server returns an error
    pub async fn request<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let _in_use = self.in_use();
        let params = serde_json::to_value(params).ok();

        // A server that is still digesting a `didOpen`/`didChange` answers
        // `ContentModified` rather than computing against a stale tree. It is
        // explicitly a "try again" signal, not a failure: without this retry the
        // first request against a freshly opened document fails every time.
        let mut attempt = 0;
        loop {
            match self.request_once::<R>(method, params.clone()).await {
                Err(LspError::LspServerError { code, .. })
                    if matches!(code, LSP_CONTENT_MODIFIED | LSP_SERVER_CANCELLED)
                        && attempt < CONTENT_MODIFIED_RETRIES =>
                {
                    attempt += 1;
                    tracing::debug!(
                        method,
                        attempt,
                        code,
                        "LSP server asked for the request again; retrying"
                    );
                    tokio::time::sleep(CONTENT_MODIFIED_BACKOFF * attempt).await;
                }
                other => return other,
            }
        }
    }

    /// One round trip, with no retry.
    async fn request_once<R>(&self, method: &str, params: Option<serde_json::Value>) -> Result<R>
    where
        R: DeserializeOwned,
    {
        let id = RequestId::Number(self.request_counter.fetch_add(1, Ordering::SeqCst));
        let request = RequestMessage {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.into(),
            params,
        };

        let (tx, rx) = oneshot::channel();

        self.command_tx
            .send(ClientCommand::SendRequest {
                request,
                response_tx: tx,
            })
            .await
            .map_err(|_| LspError::Transport("Command channel closed".into()))?;

        // Wait for response with timeout
        let result = timeout(self.request_timeout, rx)
            .await
            .map_err(|_| LspError::Timeout {
                operation: format!("LSP request: {method}"),
                timeout_ms: self.request_timeout.as_millis() as u64,
            })?;

        match result {
            Ok(Ok(value)) => serde_json::from_value(value)
                .map_err(|e| LspError::Json(format!("Failed to deserialize response: {e}"))),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(LspError::Transport("Response channel closed".into())),
        }
    }

    /// Send a notification (no response expected).
    ///
    /// # Errors
    ///
    /// Returns an error if the command channel is closed.
    pub async fn notify<P>(&self, method: &str, params: P) -> Result<()>
    where
        P: Serialize,
    {
        let _in_use = self.in_use();
        self.command_tx
            .send(ClientCommand::SendNotification {
                method: method.into(),
                params: serde_json::to_value(params).ok(),
            })
            .await
            .map_err(|_| LspError::Transport("Command channel closed".into()))
    }

    /// Whether the connection to the server is gone: its process exited (it
    /// closed stdout) or the transport failed. Requests can no longer reach it.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.command_tx.is_closed()
    }

    /// Get the current server state.
    pub async fn state(&self) -> ServerState {
        *self.state.lock().await
    }

    /// Set the server state.
    pub async fn set_state(&self, new_state: ServerState) {
        let mut state = self.state.lock().await;
        *state = new_state;
    }

    /// Shutdown the client.
    ///
    /// # Errors
    ///
    /// Returns an error if the command channel is closed.
    pub async fn shutdown(&self) -> Result<()> {
        self.command_tx
            .send(ClientCommand::Shutdown)
            .await
            .map_err(|_| LspError::Transport("Command channel closed".into()))
    }
}

/// A server's error message without the stack trace a server may append:
/// typescript-language-server sends tsserver's exception with its whole
/// JavaScript stack, which reached the agent as the answer. The lines from
/// the first indented `at …` frame on go (JavaScript's and Java's stacks).
fn without_stack(message: &str) -> String {
    message
        .lines()
        .take_while(|line| {
            !(line.starts_with(char::is_whitespace) && line.trim_start().starts_with("at "))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JavaScript or Java stack goes from a server's error message; the
    /// message, and a line that only reads "at", stay.
    #[test]
    fn a_servers_error_loses_its_stack() {
        let tsserver = "<syntax> TypeScript Server Error (5.9.3)\nNo Project.\nError: No Project.\n    \
                        at Object.ThrowNoProject (typescript.js:186170:11)\n    at next (x.js:1:1)";
        assert_eq!(
            without_stack(tsserver),
            "<syntax> TypeScript Server Error (5.9.3)\nNo Project.\nError: No Project."
        );
        let jdtls =
            "Internal error.\njava.lang.NullPointerException\n\tat org.eclipse.jdt.X.y(X.java:3)";
        assert_eq!(
            without_stack(jdtls),
            "Internal error.\njava.lang.NullPointerException"
        );
        assert_eq!(
            without_stack("at line 4: unexpected token"),
            "at line 4: unexpected token"
        );
    }

    #[test]
    fn test_server_state_enum() {
        assert_ne!(ServerState::Uninitialized, ServerState::Ready);
        assert_ne!(ServerState::Uninitialized, ServerState::Shutdown);
        assert_eq!(ServerState::Ready, ServerState::Ready);
        assert_eq!(ServerState::Shutdown, ServerState::Shutdown);
    }

    #[test]
    fn test_server_state_all_variants() {
        // Verify all variants exist and are distinct
        let states = vec![
            ServerState::Uninitialized,
            ServerState::Initializing,
            ServerState::Ready,
            ServerState::ShuttingDown,
            ServerState::Shutdown,
        ];

        // Each state should equal itself
        for state in &states {
            assert_eq!(*state, *state);
        }

        // States should be different from each other
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[tokio::test]
    async fn test_client_clone() {
        // This test just verifies the client can be cloned.
        // Actual client testing requires a mock transport.
        fn _assert_clone<T: Clone>() {}
        _assert_clone::<LspClient>();
    }

    #[test]
    fn test_request_id_equality() {
        let id1 = RequestId::Number(1);
        let id2 = RequestId::Number(1);
        let id3 = RequestId::Number(2);
        let id4 = RequestId::String("1".to_string());

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_ne!(id1, id4); // Number 1 != String "1"

        let id5 = RequestId::String("test".to_string());
        let id6 = RequestId::String("test".to_string());
        assert_eq!(id5, id6);
    }

    #[test]
    fn test_request_id_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(RequestId::Number(1));
        set.insert(RequestId::Number(1));
        set.insert(RequestId::Number(2));
        set.insert(RequestId::String("1".to_string()));

        assert_eq!(set.len(), 3); // 1, 2, and "1" are all different
    }

    #[test]
    fn test_request_id_debug() {
        let id_num = RequestId::Number(42);
        let debug_str = format!("{id_num:?}");
        assert!(debug_str.contains("42"));

        let id_str = RequestId::String("abc".to_string());
        let debug_str = format!("{id_str:?}");
        assert!(debug_str.contains("abc"));
    }

    #[test]
    fn test_client_command_variants() {
        // Verify command variants can be created
        let shutdown = ClientCommand::Shutdown;
        assert!(matches!(shutdown, ClientCommand::Shutdown));

        // Verify SendRequest variant structure
        let (tx, _rx) = oneshot::channel();
        let request = RequestMessage {
            jsonrpc: "2.0".to_string(),
            id: RequestId::Number(1),
            method: "test".to_string(),
            params: None,
        };
        let send_req = ClientCommand::SendRequest {
            request,
            response_tx: tx,
        };
        assert!(matches!(send_req, ClientCommand::SendRequest { .. }));

        // Verify SendNotification variant
        let notify = ClientCommand::SendNotification {
            method: "test".to_string(),
            params: None,
        };
        assert!(matches!(notify, ClientCommand::SendNotification { .. }));
    }

    #[test]
    fn test_request_id_serialization() {
        // Test numeric ID serialization
        let id_num = RequestId::Number(42);
        let json = serde_json::to_string(&id_num).unwrap();
        assert_eq!(json, "42");

        // Test string ID serialization
        let id_str = RequestId::String("abc".to_string());
        let json = serde_json::to_string(&id_str).unwrap();
        assert_eq!(json, "\"abc\"");
    }

    #[test]
    fn test_request_id_deserialization() {
        // Test numeric ID deserialization
        let id: RequestId = serde_json::from_str("42").unwrap();
        assert!(matches!(id, RequestId::Number(42)));

        // Test string ID deserialization
        let id: RequestId = serde_json::from_str("\"abc\"").unwrap();
        assert!(matches!(id, RequestId::String(s) if s == "abc"));
    }

    /// A server request blocks the server until answered. `csharp-ls` sends
    /// `client/registerCapability` immediately after `initialize` and waits for a
    /// reply; with none, it never loads the project and every later request timed
    /// out. Devscriptor is a headless client, so an empty-but-valid answer is right.
    #[test]
    fn server_requests_get_an_acknowledging_reply() {
        let req = RequestMessage {
            jsonrpc: JSONRPC_VERSION.into(),
            id: RequestId::Number(2),
            method: "client/registerCapability".into(),
            params: Some(serde_json::json!({"registrations": []})),
        };
        assert_eq!(LspClient::answer_server_request(&req), Value::Null);
    }

    /// `workspace/configuration` must return one settings entry per requested item,
    /// or the server rejects the response and stalls.
    #[test]
    fn workspace_configuration_returns_one_entry_per_item() {
        let req = RequestMessage {
            jsonrpc: JSONRPC_VERSION.into(),
            id: RequestId::Number(3),
            method: "workspace/configuration".into(),
            params: Some(serde_json::json!({"items": [{"section": "a"}, {"section": "b"}]})),
        };
        assert_eq!(
            LspClient::answer_server_request(&req),
            Value::Array(vec![Value::Null, Value::Null])
        );
    }

    #[test]
    fn workspace_configuration_with_no_items_returns_an_empty_array() {
        let req = RequestMessage {
            jsonrpc: JSONRPC_VERSION.into(),
            id: RequestId::String("x".into()),
            method: "workspace/configuration".into(),
            params: None,
        };
        assert_eq!(LspClient::answer_server_request(&req), Value::Array(vec![]));
    }

    #[test]
    fn test_jsonrpc_version_constant() {
        assert_eq!(JSONRPC_VERSION, "2.0");
    }

    /// The per-request timeout comes from the server's configuration, not from a
    /// constant: a hard-coded 30 s would fail the first request to a cold
    /// `csharp-ls` or `fsautocomplete` while the .NET project is still loading.
    #[test]
    fn request_timeout_follows_server_settings() {
        let mut config = LspServerConfig::rust_analyzer();
        assert_eq!(
            config.settings.timeout_seconds, 30,
            "default should stay at 30s"
        );

        config.settings.timeout_seconds = 180;
        assert_eq!(
            StdDuration::from_secs(config.settings.timeout_seconds),
            StdDuration::from_secs(180),
            "a server may raise its own request timeout"
        );
    }

    #[test]
    fn test_notification_message_creation() {
        let notification = NotificationMessage {
            jsonrpc: "2.0".to_string(),
            method: "textDocument/didOpen".to_string(),
            params: Some(serde_json::json!({
                "textDocument": {
                    "uri": "file:///test.rs"
                }
            })),
        };

        assert_eq!(notification.jsonrpc, "2.0");
        assert_eq!(notification.method, "textDocument/didOpen");
        assert!(notification.params.is_some());
    }

    #[test]
    fn test_request_message_creation() {
        let request = RequestMessage {
            jsonrpc: "2.0".to_string(),
            id: RequestId::Number(1),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "processId": 12345
            })),
        };

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "initialize");
        assert!(matches!(request.id, RequestId::Number(1)));
    }

    #[tokio::test]
    async fn test_client_state_management() {
        // This test verifies the client state management logic.
        // Full integration testing requires a spawned LSP server process.

        // Verify state enum values
        let state = ServerState::Uninitialized;
        assert!(matches!(state, ServerState::Uninitialized));

        // State transitions are tested through the type system
        // The actual async state management is integration-tested
    }

    #[test]
    fn test_lsp_error_variants() {
        use crate::error::LspError;

        let timeout_err = LspError::Timeout {
            operation: "test".to_string(),
            timeout_ms: 1000,
        };
        assert!(matches!(timeout_err, LspError::Timeout { .. }));

        let transport_err = LspError::Transport("test".to_string());
        assert!(matches!(transport_err, LspError::Transport(_)));

        let protocol_err = LspError::Protocol("test".to_string());
        assert!(matches!(protocol_err, LspError::Protocol(_)));

        let json_err = LspError::Json("test".to_string());
        assert!(matches!(json_err, LspError::Json(_)));
    }
}
