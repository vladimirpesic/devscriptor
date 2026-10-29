//! Core business logic — protocol agnostic
//!
//! This module contains all 28 tool implementations with no protocol dependencies.
//! The MCP transport (`crate::unified_server`) adapts it to the protocol.
//!
//! Split into domain-specific submodules under [`core_module/`][self].

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::server::DevscriptorServerState;

// ============================================================================
// Domain-specific tool implementations
// ============================================================================

mod analysis;
mod context;
mod files;
mod jobs;
mod scan;
mod search;
mod utility;
mod watch;

// ============================================================================
// Shared types
// ============================================================================

/// Result of a tool execution.
///
/// `Success` carries the tool's **payload**, not an MCP `CallToolResult`. Framing
/// the payload into `{"content": [{"type": "text", ...}]}` is the transport's
/// job: [`crate::unified_server`] hands it to `ContentBlock::json`. A handler
/// that returned the envelope would have it wrapped a second time, a JSON string
/// holding another `{"content": ...}` object; build payloads with `ok_json` and
/// the framing stays where it belongs.
#[derive(Debug, Clone)]
pub enum ToolResult {
    Success(Value),
    Error(String),
}

/// Wrap `payload` as a successful tool result.
///
/// Serialization cannot fail for the response types used here, but a fallible
/// conversion is cheaper than a panic if one ever grows a custom `Serialize`.
pub(crate) fn ok_json<T: serde::Serialize>(payload: &T) -> Result<ToolResult, ToolError> {
    serde_json::to_value(payload)
        .map(ToolResult::Success)
        .map_err(|e| ToolError::Internal(format!("failed to serialize tool response: {e}")))
}

/// Errors that can occur during tool execution
#[derive(Debug)]
pub enum ToolError {
    /// Tool not found.
    NotFound(String),
    /// Invalid parameters were provided.
    InvalidParams(String),
    /// Internal error.
    Internal(String),
    /// I/O error.
    Io(String),
    /// Database error.
    Database(String),
    /// Embedding generation error.
    Embedding(String),
    /// Configuration error.
    Config(String),
    /// Validation error.
    Validation { field_name: String, message: String },
    /// Resource exhausted error.
    ResourceExhausted {
        resource_type: String,
        limit: u64,
        current: u64,
    },
    /// Unsupported language error.
    UnsupportedLanguage(String),
    /// Rate limit exceeded.
    RateLimited {
        tool_name: String,
        retry_after_secs: u64,
    },
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::NotFound(name) => write!(f, "Tool not found: {name}"),
            ToolError::InvalidParams(msg) => write!(f, "Invalid parameters: {msg}"),
            ToolError::Internal(msg) => write!(f, "Internal error: {msg}"),
            ToolError::Io(msg) => write!(f, "I/O error: {msg}"),
            ToolError::Database(msg) => write!(f, "Database error: {msg}"),
            ToolError::Embedding(msg) => write!(f, "Embedding error: {msg}"),
            ToolError::Config(msg) => write!(f, "Configuration error: {msg}"),
            ToolError::Validation {
                field_name,
                message,
            } => {
                write!(f, "Validation error for '{field_name}': {message}")
            }
            ToolError::ResourceExhausted {
                resource_type,
                limit,
                current,
            } => {
                write!(
                    f,
                    "Resource exhausted: {resource_type} exceeds limit ({current} > {limit})"
                )
            }
            ToolError::UnsupportedLanguage(lang) => {
                write!(f, "Language not supported: {lang}")
            }
            ToolError::RateLimited {
                tool_name,
                retry_after_secs,
            } => {
                write!(
                    f,
                    "Rate limit exceeded for '{tool_name}': retry after {retry_after_secs}s"
                )
            }
        }
    }
}

impl std::error::Error for ToolError {}

// A5 FIX: Structured From conversion that preserves error category context.
impl From<devscriptor_core::Error> for ToolError {
    fn from(err: devscriptor_core::Error) -> Self {
        use devscriptor_core::Error as CoreErr;
        match err {
            CoreErr::Database(e) => ToolError::Database(e.to_string()),
            CoreErr::ConnectionPool(msg) => ToolError::Database(msg),
            CoreErr::VectorDb(msg) => ToolError::Database(msg),
            CoreErr::Parser(msg) => ToolError::Internal(msg),
            CoreErr::UnsupportedLanguage(lang) => ToolError::UnsupportedLanguage(lang),
            CoreErr::Config(msg) => ToolError::Config(msg),
            CoreErr::Io(e) => ToolError::Io(e.to_string()),
            CoreErr::Serialization(e) => ToolError::Internal(e.to_string()),
            CoreErr::EntityExtraction(msg) => ToolError::Internal(msg),
            CoreErr::Analysis(msg) => ToolError::Internal(msg),
            CoreErr::Embedding(msg) => ToolError::Embedding(msg),
            CoreErr::EmbeddingServiceNotConfigured => {
                ToolError::Embedding("Embedding service not configured".into())
            }
            CoreErr::Watcher(msg) => ToolError::Io(msg),
            CoreErr::Validation {
                message,
                field_name,
                field_value: _,
            } => ToolError::Validation {
                field_name,
                message,
            },
            CoreErr::PathTraversal { path, base_path } => {
                ToolError::InvalidParams(format!("Path '{path}' is outside of base '{base_path}'"))
            }
            CoreErr::ResourceExhausted {
                resource_type,
                limit,
                current,
            } => ToolError::ResourceExhausted {
                resource_type,
                limit,
                current,
            },
            CoreErr::Generic(msg) => ToolError::Internal(msg),
            CoreErr::Cancelled => ToolError::Internal("Operation was cancelled".into()),
            CoreErr::TaskJoin(ctx, detail) => {
                ToolError::Internal(format!("{ctx} failed: {detail}"))
            }
            CoreErr::Timeout(secs, ctx) => {
                ToolError::Internal(format!("{ctx} timed out after {secs}s"))
            }
        }
    }
}

// ============================================================================
// Rate Limiter (MCP-2)
// ============================================================================

/// Token-bucket rate limiter per tool.
///
/// Allows 10 requests per second per tool with a burst capacity of 20.
/// Non-blocking: returns immediately with `true` if the request is allowed.
pub struct RateLimiter {
    inner: std::sync::Mutex<std::collections::HashMap<String, TokenBucket>>,
}

struct TokenBucket {
    tokens: f64,
    last_update: std::time::Instant,
    rate: f64,  // tokens per second
    burst: f64, // max tokens
}

impl RateLimiter {
    /// Default: 10 req/sec, burst of 20.
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Attempt to consume one token for `tool_name`.
    /// Returns `true` if the request is allowed.
    pub fn check(&self, tool_name: &str) -> bool {
        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = std::time::Instant::now();
        let bucket = map
            .entry(tool_name.to_string())
            .or_insert_with(|| TokenBucket {
                tokens: 20.0,
                last_update: now,
                rate: 10.0,
                burst: 20.0,
            });

        let elapsed = now.duration_since(bucket.last_update).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.rate).min(bucket.burst);
        bucket.last_update = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// O(1) lookup table: tool name → small integer id, so a call costs one hash
/// lookup instead of a string comparison per tool.
pub(crate) static TOOL_IDS: std::sync::LazyLock<HashMap<&'static str, u8>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("scan_codebase", 0);
        m.insert("search_entities", 1);
        m.insert("analyze_code", 2);
        m.insert("get_statistics", 3);
        m.insert("start_analysis", 4);
        m.insert("get_analysis_progress", 5);
        m.insert("list_analysis_jobs", 6);
        m.insert("cleanup_old_jobs", 7);
        m.insert("search_functions", 8);
        m.insert("search_classes", 9);
        m.insert("fuzzy_search", 10);
        m.insert("semantic_search", 11);
        m.insert("get_entity_details", 12);
        m.insert("get_relationships", 13);
        m.insert("find_dead_code", 14);
        m.insert("analyze_complexity", 15);
        m.insert("add_context_note", 16);
        m.insert("search_context", 17);
        m.insert("get_related_context", 18);
        m.insert("prune_context", 19);
        m.insert("get_context_statistics", 20);
        m.insert("describe_file", 21);
        m.insert("describe_modification", 22);
        m.insert("get_file_context", 23);
        m.insert("health_check", 24);
        m.insert("validate_file_edit", 25);
        m.insert("query_with_context", 26);
        m.insert("cancel_analysis", 27);
        m.insert("start_watching", 28);
        m.insert("stop_watching", 29);
        m.insert("get_watch_status", 30);
        m.insert("generate_embeddings", 31);
        m
    });

// ============================================================================
// CoreModule
// ============================================================================

/// Protocol-agnostic core module containing all business logic
pub struct CoreModule {
    state: Arc<DevscriptorServerState>,
    rate_limiter: RateLimiter,
}

impl CoreModule {
    /// Create a new [`CoreModule`] with the given state
    #[must_use]
    pub fn new(state: Arc<DevscriptorServerState>) -> Self {
        Self {
            state,
            rate_limiter: RateLimiter::new(),
        }
    }

    /// Get a reference to the internal state
    #[must_use]
    pub fn state(&self) -> &Arc<DevscriptorServerState> {
        &self.state
    }

    /// Truncate content to a maximum length, appending ellipsis if truncated.
    fn truncate_content(content: &str, max_len: usize) -> String {
        if content.len() <= max_len {
            content.to_string()
        } else {
            let mut s: String = content.chars().take(max_len).collect();
            s.push_str("...[truncated]");
            s
        }
    }

    /// Validate a path parameter for tool inputs.
    ///
    /// Rejects `..`, `~`, environment variable expansion, null
    /// bytes, and shell metacharacters. When `project_root` is `Some`, also
    /// rejects paths that escape the project directory, preventing an agent
    /// from reading arbitrary files (e.g. `/etc/passwd`).
    fn validate_tool_path(path: &str) -> Result<std::path::PathBuf, ToolError> {
        let opts = devscriptor_core::utils::validation::PathValidationOptions::default();
        devscriptor_core::utils::validation::validate_file_path(path, None, &opts)
            .map_err(ToolError::from)
    }

    /// The directory a scoping `path` argument names — `find_dead_code`'s,
    /// `analyze_complexity`'s, `analyze_code`'s — resolved as every tool path
    /// is ([`Self::validate_tool_path`]); `None` for `.`, the whole index.
    ///
    /// # Errors
    /// The path is invalid, does not exist, or is no directory: passed on, it
    /// found nothing, which read as a clean result.
    fn scope_directory(path: &str) -> Result<Option<std::path::PathBuf>, ToolError> {
        if path == "." {
            return Ok(None);
        }
        let resolved = Self::validate_tool_path(path)?;
        if !resolved.exists() {
            return Err(ToolError::InvalidParams(format!(
                "Path does not exist: {path}"
            )));
        }
        if !resolved.is_dir() {
            return Err(ToolError::InvalidParams(format!(
                "Path is not a directory: {path}"
            )));
        }
        Ok(Some(resolved))
    }

    /// Validate a tool path confined to a project root.
    ///
    /// Rejects paths outside `project_root`.  Use this for all file-reading
    /// tools where the agent supplies a file path.
    fn validate_tool_path_within(
        path: &str,
        project_root: &std::path::Path,
    ) -> Result<std::path::PathBuf, ToolError> {
        let opts = devscriptor_core::utils::validation::PathValidationOptions::default();
        devscriptor_core::utils::validation::validate_file_path(path, Some(project_root), &opts)
            .map_err(ToolError::from)
    }

    /// Resolve a path for the file-reading tools, confining it to the project, so
    /// a prompt-injected agent cannot exfiltrate arbitrary host files.
    ///
    /// Two roots are accepted, because either can be the project:
    ///
    /// * the **working directory** — where an agent launches the server, and
    ///   where `devscriptor mcp` with the default database is pointed: its
    ///   default database, `~/.local/share/devscriptor/default.db`, lies outside
    ///   every project;
    /// * the **database's directory** — the project when the caller passes
    ///   `--database /path/to/project/graph.db`.
    ///
    /// A file outside both is still readable if it is already **indexed** in this
    /// graph: its contents are reachable through `search_entities` and
    /// `get_entity_details` regardless, so reading it exposes nothing new.
    /// Anything else is rejected.
    async fn resolve_readable_path(&self, path: &str) -> Result<std::path::PathBuf, ToolError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let db_path = std::path::PathBuf::from(self.state.database().path());
        let db_root = db_path.parent().unwrap_or(std::path::Path::new("."));

        let mut first_err = None;
        for root in [cwd.as_path(), db_root] {
            match Self::validate_tool_path_within(path, root) {
                Ok(resolved) => return Ok(resolved),
                Err(e) => first_err.get_or_insert(e),
            };
        }

        if self
            .state
            .database()
            .file_is_indexed(path)
            .await
            .unwrap_or(false)
        {
            return Ok(std::path::PathBuf::from(path));
        }

        Err(first_err.expect("at least one root was tried"))
    }

    /// The first `max_bytes` of a file as UTF-8 text, a character the limit
    /// cuts in two left out.
    ///
    /// # Errors
    ///
    /// The file cannot be read, or is not UTF-8 text.
    async fn read_limited(path: &str, max_bytes: usize) -> Result<String, std::io::Error> {
        use tokio::io::AsyncReadExt;
        let file = tokio::fs::File::open(path).await?;
        let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
        file.take(max_bytes as u64).read_to_end(&mut buf).await?;
        match String::from_utf8(buf) {
            Ok(text) => Ok(text),
            // Cut short inside a character, by the limit, at the very end.
            Err(e) if e.utf8_error().error_len().is_none() => {
                let valid = e.utf8_error().valid_up_to();
                let mut bytes = e.into_bytes();
                bytes.truncate(valid);
                String::from_utf8(bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            }
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        }
    }

    /// Lines in a file, counted as [`str::lines`] counts them.
    ///
    /// # Errors
    ///
    /// The file cannot be read.
    async fn count_lines(path: &str) -> Result<usize, std::io::Error> {
        use tokio::io::AsyncReadExt;
        let mut file = tokio::fs::File::open(path).await?;
        let mut buf = vec![0u8; 64 * 1024];
        let (mut newlines, mut last) = (0usize, None);
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            newlines += buf[..n].iter().filter(|&&b| b == b'\n').count();
            last = Some(buf[n - 1]);
        }
        Ok(newlines + usize::from(last.is_some_and(|b| b != b'\n')))
    }
}

// ============================================================================
// Shared scan helper
// ============================================================================

/// Run the full `CodebasePipeline` on `path` using the existing `db`: discover
/// the files, parse them with tree-sitter, extract entities and references,
/// derive the edges, and record checksums, languages and line counts, as
/// `devscriptor scan` does. Embeddings are skipped; `semantic_search` embeds
/// on demand, and `generate_embeddings` embeds the whole index.
///
/// The scan takes the project's configuration (`project`), then the
/// `scan_codebase` / `start_analysis` tool arguments, which outrank it:
/// `include_extensions` is a whitelist (empty = the project's, else every
/// supported language) and `exclude_patterns` are globs matched against file
/// paths (empty = the project's).
async fn run_pipeline(
    db: Arc<devscriptor_core::database::Database>,
    project: &devscriptor_core::config::ProjectConfig,
    path: &std::path::Path,
    include_extensions: Vec<String>,
    exclude_patterns: Vec<String>,
) -> Result<devscriptor_core::pipeline::PipelineResult, devscriptor_core::Error> {
    use devscriptor_core::database::DatabaseConfig;
    use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};

    let mut pipeline_config = PipelineConfig {
        database: DatabaseConfig {
            path: db.path().to_string(),
            ..DatabaseConfig::default()
        },
        skip_embeddings: true,
        enable_cross_file: true,
        ..PipelineConfig::default()
    };
    project.apply_to(&mut pipeline_config);
    if !include_extensions.is_empty() {
        pipeline_config.include_extensions = include_extensions;
    }
    if !exclude_patterns.is_empty() {
        pipeline_config.exclude_patterns = exclude_patterns;
    }

    let pipeline = CodebasePipeline::with_database(pipeline_config, Arc::clone(&db))
        .map_err(|e| devscriptor_core::Error::Analysis(format!("Pipeline init failed: {e}")))?;

    pipeline
        .analyze_directory(path)
        .await
        .map_err(|e| devscriptor_core::Error::Analysis(format!("Pipeline scan failed: {e}")))
}

/// Read an optional array-of-strings tool argument (e.g. `extensions`).
///
/// Missing, null, or non-array values yield an empty vector, which the pipeline
/// treats as "no filter".
pub(crate) fn string_array_arg(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Run the full pipeline with a progress callback for job-based tracking.
///
/// Returns the final `PipelineResult`. The progress callback receives
/// `(files_processed, files_discovered, last_file_path)` updates.
async fn run_pipeline_with_progress<F>(
    db: Arc<devscriptor_core::database::Database>,
    project: &devscriptor_core::config::ProjectConfig,
    path: &std::path::Path,
    include_extensions: Vec<String>,
    exclude_patterns: Vec<String>,
    on_progress: F,
) -> Result<devscriptor_core::pipeline::PipelineResult, devscriptor_core::Error>
where
    F: Fn(usize, usize, String) + Send + Sync + 'static,
{
    // The pipeline does not yet expose per-file progress callbacks; we report
    // at the start and end. For fine-grained progress, a future refactor could
    // add a progress channel to PipelineConfig.
    on_progress(0, 0, path.to_string_lossy().to_string());
    let result = run_pipeline(db, project, path, include_extensions, exclude_patterns).await?;
    on_progress(
        result.files_parsed,
        result.files_discovered,
        path.to_string_lossy().to_string(),
    );
    Ok(result)
}

/// Whether the tool `name` answers from the index with results that name
/// their files, so that the answer is first checked against those files on
/// disk ([`crate::freshness`]).
fn reads_the_index(name: &str) -> bool {
    matches!(
        name,
        "semantic_search"
            | "search_entities"
            | "search_functions"
            | "search_classes"
            | "fuzzy_search"
            | "query_with_context"
            | "get_entity_details"
            | "get_relationships"
            | "get_file_context"
            | "describe_file"
    )
}

impl CoreModule {
    /// Universal tool handler — routes to the appropriate implementation.
    ///
    /// MCP-2: Rate limiting is checked before dispatch (10 req/sec, burst 20).
    /// A tool that answers from the index answers from an index that is
    /// current for the files its answer names, as far as the freshness budget
    /// allows ([`crate::freshness::ensure_fresh`]).
    pub async fn handle_tool(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        if !self.rate_limiter.check(name) {
            return Err(ToolError::RateLimited {
                tool_name: name.to_string(),
                retry_after_secs: 1,
            });
        }
        if reads_the_index(name) {
            return self.answer_from_current_index(name, arguments).await;
        }
        self.dispatch(name, arguments).await
    }

    /// Answer `name`, a tool that reads the index, from an index that is
    /// current for the files its answer names ([`crate::freshness::fresh_answer`]):
    /// those that changed on disk are re-indexed, and the query runs again
    /// when a file it read was re-indexed meanwhile. The answer says which
    /// files were re-indexed for it (`refreshed_files`) and which may be newer
    /// than it (`stale_files`).
    async fn answer_from_current_index(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let (mut answer, report) = crate::freshness::fresh_answer(&self.state, || {
            self.answer_and_files(name, arguments.clone())
        })
        .await?;
        if let ToolResult::Success(payload) = &mut answer {
            report.annotate(payload);
        }
        Ok(answer)
    }

    /// The answer of the tool `name` and the files it was read from: the
    /// `file_path` fields of the answer, or, for `get_relationships`, whose
    /// answer names entities only, the files of the entities and edges it
    /// reports.
    async fn answer_and_files(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<(ToolResult, Vec<String>), ToolError> {
        if name == "get_relationships" {
            let (response, files) = self.relationships(arguments).await?;
            return Ok((ok_json(&response)?, files));
        }
        let answer = self.dispatch(name, arguments).await?;
        let files = match &answer {
            ToolResult::Success(payload) => crate::freshness::named_files(payload),
            ToolResult::Error(_) => Vec::new(),
        };
        Ok((answer, files))
    }

    /// Run the tool `name`.
    ///
    /// A `HashMap` gives the tool's id in O(1), and a `u8` match on it dispatches
    /// the call.
    async fn dispatch(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        match TOOL_IDS.get(name).copied() {
            Some(0) => self.scan_codebase(arguments).await,
            Some(1) => self.search_entities(arguments).await,
            Some(2) => self.analyze_code(arguments).await,
            Some(3) => self.get_statistics(arguments).await,
            Some(4) => self.start_analysis(arguments).await,
            Some(5) => self.get_analysis_progress(arguments).await,
            Some(6) => self.list_analysis_jobs(arguments).await,
            Some(7) => self.cleanup_old_jobs(arguments).await,
            Some(8) => self.search_functions(arguments).await,
            Some(9) => self.search_classes(arguments).await,
            Some(10) => self.fuzzy_search(arguments).await,
            Some(11) => self.semantic_search(arguments).await,
            Some(12) => self.get_entity_details(arguments).await,
            Some(13) => self.get_relationships(arguments).await,
            Some(14) => self.find_dead_code(arguments).await,
            Some(15) => self.analyze_complexity(arguments).await,
            Some(16) => self.add_context_note(arguments).await,
            Some(17) => self.search_context(arguments).await,
            Some(18) => self.get_related_context(arguments).await,
            Some(19) => self.prune_context(arguments).await,
            Some(20) => self.get_context_statistics(arguments).await,
            Some(21) => self.describe_file(arguments).await,
            Some(22) => self.describe_modification(arguments).await,
            Some(23) => self.get_file_context(arguments).await,
            Some(24) => self.health_check(arguments).await,
            Some(25) => self.validate_file_edit(arguments).await,
            Some(26) => self.query_with_context(arguments).await,
            Some(27) => self.cancel_analysis(arguments).await,
            Some(28) => self.start_watching(arguments).await,
            Some(29) => self.stop_watching(arguments).await,
            Some(30) => self.get_watch_status(arguments).await,
            Some(31) => self.generate_embeddings(arguments).await,
            _ => Err(ToolError::NotFound(name.to_string())),
        }
    }
}
