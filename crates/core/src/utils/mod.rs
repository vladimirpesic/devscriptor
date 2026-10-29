//! Utilities Module for Devscriptor-Core
//!
//! Provides common utilities: performance monitoring, input validation, and
//! numbering the source excerpts Devscriptor answers with.

pub mod gitignore_filter;
pub mod memory_guard;
pub mod numbered;
pub mod performance;
pub mod security;
pub mod type_helpers;
pub mod validation;

pub use gitignore_filter::GitignoreFilter;
pub use numbered::number_lines;
pub use security::{
    FileChangeDetector, compute_file_hash, contains_shell_metacharacters, sanitize_error_message,
    sanitize_for_shell, sanitize_path_for_display, verify_file_hash,
};

// Internal-only re-exports (pub(crate) via the module visibility above).
// Not exposed publicly because no downstream crate consumes them.
pub use memory_guard::{MemoryGuard, free_ram_mb, total_ram_mb};
pub use performance::{MemoryMonitor, PerformanceMonitor, PerformanceProfiler, PerformanceTimer};
pub use validation::{
    PathValidationOptions, escape_like, sanitize_filename, validate_database_url,
    validate_file_path, validate_file_size, validate_file_size_default, validate_language,
    validate_positive_integer, validate_positive_integer_str,
};
