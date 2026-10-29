//! Error types for devscriptor-core
//!
//! # Security
//!
//! Error messages in this module may contain file paths in development builds.
//! For production builds, use the `sanitize_for_production` method to remove
//! sensitive information from error messages.

use thiserror::Error;

/// Result type alias for devscriptor operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error enum for the library
#[derive(Error, Debug)]
pub enum Error {
    /// Database-related errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Database pool errors
    #[error("Connection pool error: {0}")]
    ConnectionPool(String),

    /// Vector database errors
    #[error("Vector database error: {0}")]
    VectorDb(String),

    /// Parser errors
    #[error("Parser error: {0}")]
    Parser(String),

    /// Language not supported
    #[error("Language not supported: {0}")]
    UnsupportedLanguage(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization errors
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Entity extraction errors
    #[error("Entity extraction error: {0}")]
    EntityExtraction(String),

    /// Analysis errors
    #[error("Analysis error: {0}")]
    Analysis(String),

    /// Embedding generation errors
    #[error("Embedding error: {0}")]
    Embedding(String),

    /// Embedding service is not configured or unavailable
    #[error("Embedding service not configured")]
    EmbeddingServiceNotConfigured,

    /// File watching errors
    #[error("File watcher error: {0}")]
    Watcher(String),

    /// Validation errors
    #[error("Validation error: {message}")]
    Validation {
        /// Error message
        message: String,
        /// Name of the field that failed validation
        field_name: String,
        /// Value of the field that failed validation (sanitized in production)
        field_value: String,
    },

    /// Path traversal error
    #[error("Path traversal attempt: path '{path}' is outside of base '{base_path}'")]
    PathTraversal {
        /// The path that was attempted
        path: String,
        /// The base path that was expected
        base_path: String,
    },

    /// Resource exhausted error
    #[error("Resource exhausted: {resource_type} exceeds limit ({current} > {limit})")]
    ResourceExhausted {
        /// Type of resource that was exhausted
        resource_type: String,
        /// Maximum allowed value
        limit: u64,
        /// Current value that exceeded limit
        current: u64,
    },

    /// Generic error with message
    #[error("{0}")]
    Generic(String),

    /// Operation was cancelled
    #[error("Operation cancelled")]
    Cancelled,

    /// Background task failed to complete
    #[error("Task failed: {0}: {1}")]
    TaskJoin(&'static str, String),

    /// Database query timed out
    #[error("Database query timed out after {0} seconds: {1}")]
    Timeout(u64, &'static str),
}

impl Error {
    /// Create a [`TaskJoin`](Error::TaskJoin) error with context.
    ///
    /// Use this instead of `Error::task_join("blocking task", e)`
    /// to preserve the original error context.
    pub fn task_join(context: &'static str, source: impl std::fmt::Display) -> Self {
        Error::TaskJoin(context, source.to_string())
    }

    /// Create a [`Timeout`](Error::Timeout) error with context.
    pub fn timeout(seconds: u64, context: &'static str) -> Self {
        Error::Timeout(seconds, context)
    }
}

impl Error {
    /// Create a validation error
    pub fn validation(
        message: impl Into<String>,
        field_name: impl Into<String>,
        field_value: impl Into<String>,
    ) -> Self {
        Error::Validation {
            message: message.into(),
            field_name: field_name.into(),
            field_value: field_value.into(),
        }
    }

    /// Create a path traversal error
    pub fn path_traversal(path: impl Into<String>, base_path: impl Into<String>) -> Self {
        Error::PathTraversal {
            path: path.into(),
            base_path: base_path.into(),
        }
    }

    /// Create a resource exhausted error
    pub fn resource_exhausted(resource_type: impl Into<String>, limit: u64, current: u64) -> Self {
        Error::ResourceExhausted {
            resource_type: resource_type.into(),
            limit,
            current,
        }
    }

    /// Sanitize the error message for production display
    ///
    /// This method removes sensitive information like file paths
    /// from error messages before displaying them in production.
    ///
    /// # Security note
    ///
    /// The path-sanitization regexes are intentionally broad and may
    /// over-match legitimate non-path text (e.g. `foo/bar/baz` inside a
    /// string literal or a portion of a UUID).  This is an acceptable
    /// trade-off to guarantee that no absolute or system-sensitive paths
    /// leak in release builds.  If you need the raw message for debugging,
    /// rely on the un-sanitized output returned in `debug_assertions`
    /// builds instead.
    #[must_use]
    pub fn sanitize_for_production(&self) -> String {
        sanitize_message(&self.to_string())
    }
}

/// Redact filesystem paths from an arbitrary message string for production display.
///
/// In release builds this replaces home-directory references, known system
/// directories, nested file paths with an extension, and Windows absolute paths
/// with `[PATH]`. In debug builds the message is returned unchanged so developers
/// keep full diagnostics. This is the boundary helper used by the CLI top-level
/// handler and the MCP server so error text shown to users or agents never leaks
/// absolute paths (privacy-first).
///
/// The regex is intentionally broad and may over-match non-path text; that is an
/// accepted trade-off to guarantee no path leaks in release builds.
#[must_use]
pub fn sanitize_message(msg: &str) -> String {
    #[cfg(not(debug_assertions))]
    {
        use regex::Regex;
        use std::sync::LazyLock;

        static PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?x)
                (?:
                    ~(?:/[^\s]*)?                                 # home directory
                    |/(?:home|Users|root|var|tmp|etc|usr|opt|lib|bin|sbin|dev|proc|sys|mnt|media|srv|run|snap)(?:/[^\s]*)?
                    |/[^\s]+/[^\s]*\.[a-zA-Z0-9]{1,10}             # nested file paths with extension
                    |[A-Za-z]:[\\/][^\s]*                          # Windows paths
                )",
            )
            .expect("Static path regex pattern is valid")
        });

        PATH_RE.replace_all(msg, "[PATH]").to_string()
    }

    #[cfg(debug_assertions)]
    {
        msg.to_string()
    }
}

impl From<r2d2::Error> for Error {
    fn from(e: r2d2::Error) -> Self {
        Error::ConnectionPool(e.to_string())
    }
}
