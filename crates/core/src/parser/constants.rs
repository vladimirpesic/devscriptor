//! Shared parsing constants
//!
//! Centralizes limits and thresholds used by both entity cleaning and
//! validation to prevent drift between the two modules.

/// Maximum length for entity names (255 characters).
pub const MAX_NAME_LENGTH: usize = 255;

/// Maximum body length for cleaning — bodies larger than this are truncated.
pub const MAX_BODY_LENGTH: usize = 100_000; // 100KB

/// Hard body length limit for validation — entities exceeding this are rejected.
pub const MAX_BODY_HARD_LIMIT: usize = 1_000_000; // 1MB

/// Maximum length of a single parameter string.
pub const MAX_PARAMETER_STRING_LENGTH: usize = 10_000; // 10KB

/// Maximum number of parameters in a parameter list.
pub const MAX_PARAMETER_LIST_LENGTH: usize = 1_000;

/// Maximum number of fields in a dictionary.
pub const MAX_FIELDS_DICT_LENGTH: usize = 10_000;

/// Maximum combined length of all field strings.
pub const MAX_FIELDS_STRING_LENGTH: usize = 100_000; // 100KB

/// Maximum length of a return-type annotation.
pub const MAX_RETURN_TYPE_LENGTH: usize = 1_000;

/// Maximum length of an `extends` clause.
pub const MAX_EXTENDS_LENGTH: usize = 1_000;

/// Maximum length of an `implements` clause.
pub const MAX_IMPLEMENTS_LENGTH: usize = 10_000;

/// Minimum valid confidence score (inclusive).
pub const MIN_CONFIDENCE_SCORE: f32 = 0.0;

/// Maximum valid confidence score (inclusive).
pub const MAX_CONFIDENCE_SCORE: f32 = 1.0;

/// Marker appended to truncated strings.
pub const TRUNCATION_MARKER: &str = "... (truncated)";

/// Stack size for the dedicated tree-sitter parse thread (256 MB).
///
/// Tree-sitter's internal parsing of deeply-nested or machine-generated sources
/// (huge nested initializer arrays, long macro chains, generated headers) can
/// drive deep C-level recursion.  On the default ~2 MB thread stack this risks a
/// stack-overflow that manifests as an **uncatchable** `SIGSEGV`/`SIGBUS` and
/// aborts the entire process.  Running the parse on a thread with this much
/// headroom turns such files into successful parses instead of crashes.  The
/// stack is lazily committed by the OS, so reserving 256 MB costs only the
/// address space, not physical RAM (only the pages actually touched are backed).
pub const PARSE_THREAD_STACK_SIZE: usize = 256 * 1024 * 1024; // 256 MB

/// Maximum byte length of a single source line that will be handed to
/// tree-sitter (2 MB).
///
/// Minified bundles and some generated files pack enormous content onto a single
/// line.  Such files are not meaningfully analyzable and can drive tree-sitter
/// into pathological memory/time.  Files containing any line longer than this
/// are skipped with a clean, recoverable error (the pipeline logs and continues)
/// rather than risking a crash.
pub const MAX_LINE_LENGTH: usize = 2 * 1024 * 1024; // 2 MB

/// Valid macro types derived from the original Python `validation_utils.py`.
pub const VALID_MACRO_TYPES: &[&str] = &[
    "function_like",
    "object_like",
    "variadic",
    "chain_rule",
    "include_guard",
    "conditional",
    "include",
    "define",
    "undef",
    "ifdef",
    "ifndef",
    "if",
    "elif",
    "else",
    "endif",
    "pragma",
    "error",
    "warning",
    "line",
];
