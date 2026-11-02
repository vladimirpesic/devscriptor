//! Error types for the LSP module.
//!
//! This module defines all error variants used throughout the LSP module.
//! All error variants implement Clone for use in async contexts.

use std::fmt;
use std::io;

/// Result type alias for LSP operations.
pub type Result<T> = std::result::Result<T, LspError>;

/// Error type alias for LSP operations.
pub type Error = LspError;

/// Details of a single server spawn failure.
#[derive(Debug, Clone)]
pub struct ServerSpawnFailure {
    /// Language ID of the failed server.
    pub language_id: String,
    /// Command that was attempted.
    pub command: String,
    /// Error message describing the failure.
    pub error: String,
}

impl fmt::Display for ServerSpawnFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.language_id, self.command, self.error)
    }
}

/// Main error type for LSP module operations.
#[derive(Debug, Clone)]
pub enum LspError {
    /// Server spawn failed with details.
    ServerSpawnFailed {
        language_id: String,
        command: String,
        error: String,
    },
    /// IO error.
    Io(String),
    /// JSON serialization/deserialization error.
    Json(String),
    /// Protocol error.
    Protocol(String),
    /// Transport error.
    Transport(String),
    /// The server closed its stdout at a message boundary.
    ///
    /// Distinct from [`LspError::Transport`] because it is how a language server
    /// *normally* ends: the process exits, its stdout reaches EOF, and the
    /// reader sees that between messages rather than inside one. Reported as a
    /// reader sees that between messages rather than inside one. Reported as a
    /// transport failure it would log an `ERROR` line at every clean shutdown,
    /// burying the real transport failures it would look identical to. EOF in
    /// the middle of a message is still [`LspError::Transport`]: there, bytes
    /// really are lost.
    ServerClosed,
    /// Request timeout.
    Timeout { operation: String, timeout_ms: u64 },
    /// LSP server returned an error.
    LspServerError { code: i32, message: String },
    /// No server configured for language.
    NoServerForLanguage(String),
    /// A configured server that cannot answer — it failed to start, exited
    /// too often, finds no project root, or is not running — or a request
    /// that names no server among several. The message says which.
    ServerUnavailable(String),
    /// No server available for file.
    NoServerForFile(String),
    /// The running server does not advertise the requested LSP capability, so
    /// the request would fail with an opaque `-32601`. Reported up-front instead.
    Unsupported {
        language_id: String,
        feature: String,
    },
    /// Configuration error.
    Config(String),
    /// Invalid path.
    InvalidPath(String),
    /// All servers failed to initialize.
    AllServersFailedToInit {
        count: usize,
        failures: Vec<ServerSpawnFailure>,
    },
    /// Shutdown requested.
    Shutdown,
}

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LspError::ServerSpawnFailed {
                language_id,
                command,
                error,
            } => {
                write!(
                    f,
                    "Failed to spawn {language_id} server ({command}): {error}"
                )
            }
            LspError::Io(s) => write!(f, "IO error: {s}"),
            LspError::Json(s) => write!(f, "JSON error: {s}"),
            LspError::Protocol(s) => write!(f, "Protocol error: {s}"),
            LspError::Transport(s) => write!(f, "Transport error: {s}"),
            LspError::ServerClosed => write!(f, "Language server closed the connection"),
            LspError::Timeout {
                operation,
                timeout_ms,
            } => {
                write!(f, "Operation '{operation}' timed out after {timeout_ms}ms")
            }
            LspError::LspServerError { code, message } => {
                write!(f, "LSP server error {code}: {message}")
            }
            LspError::NoServerForLanguage(lang) => {
                write!(f, "No LSP server configured for language: {lang}")
            }
            LspError::ServerUnavailable(why) => write!(f, "{why}"),
            LspError::NoServerForFile(path) => {
                write!(f, "No LSP server available for file: {path}")
            }
            LspError::Unsupported {
                language_id,
                feature,
            } => {
                write!(
                    f,
                    "the {language_id} language server does not support {feature}"
                )
            }
            LspError::Config(s) => write!(f, "Configuration error: {s}"),
            LspError::InvalidPath(s) => write!(f, "Invalid path: {s}"),
            LspError::AllServersFailedToInit { count, failures } => {
                write!(
                    f,
                    "All {count} LSP servers failed to initialize: {}",
                    failures
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
            LspError::Shutdown => write!(f, "Server shutdown requested"),
        }
    }
}

impl std::error::Error for LspError {}

impl From<io::Error> for LspError {
    fn from(e: io::Error) -> Self {
        LspError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for LspError {
    fn from(e: serde_json::Error) -> Self {
        LspError::Json(e.to_string())
    }
}

impl From<toml::de::Error> for LspError {
    fn from(e: toml::de::Error) -> Self {
        LspError::Config(format!("TOML parse error: {e}"))
    }
}
