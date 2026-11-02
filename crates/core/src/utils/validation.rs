//! Input validation utilities for Devscriptor-Core
//!
//! This module provides secure validation functions for file paths, user inputs,
//! and other potentially dangerous data.

use crate::error::Error;
use crate::error::Result;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// Maximum allowed file size (50MB default)
/// Default maximum file size in bytes (50 MiB).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// Escape LIKE metacharacters so user input is treated as a literal substring.
/// SQLite LIKE uses `\` as the escape character (declared via `ESCAPE '\'`).
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// Maximum path length
const MAX_PATH_LENGTH: usize = 4096;

// Maximum filename length
const MAX_FILENAME_LENGTH: usize = 255;

/// Path validation options
#[derive(Debug, Clone)]
pub struct PathValidationOptions {
    /// Whether the path must exist
    pub must_exist: bool,
    /// Whether the path must be a file
    pub must_be_file: bool,
    /// Whether the path must be a directory
    pub must_be_directory: bool,
    /// Maximum allowed path length
    pub max_length: usize,
}

impl Default for PathValidationOptions {
    fn default() -> Self {
        Self {
            must_exist: false,
            must_be_file: false,
            must_be_directory: false,
            max_length: MAX_PATH_LENGTH,
        }
    }
}

impl PathValidationOptions {
    /// Create with must_exist set to true
    pub fn must_exist() -> Self {
        Self {
            must_exist: true,
            ..Default::default()
        }
    }

    /// Create with must_be_file set to true
    pub fn must_be_file() -> Self {
        Self {
            must_exist: true,
            must_be_file: true,
            ..Default::default()
        }
    }

    /// Create with must_be_directory set to true
    pub fn must_be_directory() -> Self {
        Self {
            must_exist: true,
            must_be_directory: true,
            ..Default::default()
        }
    }
}

/// Validate a file path for security and correctness.
///
/// # Arguments
/// * `path` - The path to validate
/// * `base_path` - Optional base path that the file must be within
/// * `options` - Validation options
///
/// # Returns
/// Validated PathBuf object
///
/// # Errors
/// Returns an error if the path is invalid or doesn't meet criteria
pub fn validate_file_path<P: AsRef<Path>>(
    path: P,
    base_path: Option<&Path>,
    options: &PathValidationOptions,
) -> Result<PathBuf> {
    let path = path.as_ref();
    let path_str = path.to_string_lossy();

    // Check if path is empty
    if path_str.is_empty() {
        return Err(Error::validation(
            "Path cannot be empty",
            "path",
            path_str.to_string(),
        ));
    }

    // Check path length (character count to avoid multi-byte panics)
    if path_str.chars().count() > options.max_length {
        return Err(Error::validation(
            format!(
                "Path exceeds maximum length of {} characters",
                options.max_length
            ),
            "path",
            truncate_for_display(&path_str, 50),
        ));
    }

    // Check for dangerous patterns
    check_dangerous_patterns(&path_str)?;

    // Resolve the path
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(Error::Io)?.join(path)
    };

    // Canonicalize the path, or its deepest existing parent
    let canonical = canonicalize_existing_or_parent(&absolute)?;

    // Check for path traversal if base_path is specified
    if let Some(base) = base_path {
        let base_canon = dunce::canonicalize(base).map_err(Error::Io)?;
        if !canonical.starts_with(&base_canon) {
            return Err(Error::path_traversal(
                canonical.display().to_string(),
                base_canon.display().to_string(),
            ));
        }
    }

    // Check existence if required
    if options.must_exist && !canonical.exists() {
        return Err(Error::validation(
            format!("Path does not exist: {}", canonical.display()),
            "path",
            canonical.display().to_string(),
        ));
    }

    // Check if file
    if options.must_be_file && canonical.exists() && !canonical.is_file() {
        return Err(Error::validation(
            format!("Path is not a file: {}", canonical.display()),
            "path",
            canonical.display().to_string(),
        ));
    }

    // Check if directory
    if options.must_be_directory && canonical.exists() && !canonical.is_dir() {
        return Err(Error::validation(
            format!("Path is not a directory: {}", canonical.display()),
            "path",
            canonical.display().to_string(),
        ));
    }

    Ok(canonical)
}

/// Truncate a string to at most `n` characters for safe display.
fn truncate_for_display(s: &str, n: usize) -> String {
    let mut out = String::with_capacity(n);
    for (i, c) in s.chars().enumerate() {
        if i >= n {
            break;
        }
        out.push(c);
    }
    out
}

/// Canonicalize a path, or if it does not yet exist, canonicalize its deepest
/// existing parent and re-apply the suffix.
fn canonicalize_existing_or_parent(p: &Path) -> Result<PathBuf> {
    if let Ok(c) = dunce::canonicalize(p) {
        return Ok(c);
    }
    let mut cur = p.to_path_buf();
    while !cur.exists() {
        cur = match cur.parent() {
            Some(par) => par.to_path_buf(),
            None => {
                return Err(Error::Io(std::io::Error::other(
                    "no canonical ancestor for path",
                )));
            }
        };
    }
    let canonical_parent = dunce::canonicalize(&cur).map_err(Error::Io)?;
    let suffix = p
        .strip_prefix(&cur)
        .map_err(|_| Error::Io(std::io::Error::other("prefix mismatch during canonicalize")))?;
    Ok(canonical_parent.join(suffix))
}

/// Check for dangerous patterns in a path
fn check_dangerous_patterns(path: &str) -> Result<()> {
    // Check for parent directory references (path traversal)
    if path.contains("..") {
        return Err(Error::validation(
            "Path contains dangerous pattern: parent directory reference (..)",
            "path",
            path.to_string(),
        ));
    }

    // Check for home directory reference at start
    if path.starts_with('~') {
        return Err(Error::validation(
            "Path contains dangerous pattern: home directory reference (~)",
            "path",
            path.to_string(),
        ));
    }

    // Check for environment variable expansion ($VAR or %VAR%)
    static ENV_REGEX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"\$\w+|%\w+%").expect("Static env var regex pattern is valid")
    });

    let env_regex = &*ENV_REGEX;

    if env_regex.is_match(path) {
        return Err(Error::validation(
            "Path contains dangerous pattern: environment variable expansion",
            "path",
            path.to_string(),
        ));
    }

    Ok(())
}

/// Validate file size is within limits.
///
/// # Arguments
/// * `file_path` - Path to the file
/// * `max_size` - Maximum allowed size in bytes
///
/// # Returns
/// File size in bytes
///
/// # Errors
/// Returns an error if file is too large or cannot be accessed
pub fn validate_file_size<P: AsRef<Path>>(file_path: P, max_size: u64) -> Result<u64> {
    let path = validate_file_path(file_path, None, &PathValidationOptions::must_be_file())?;

    let metadata = std::fs::metadata(&path).map_err(Error::Io)?;
    let size = metadata.len();

    if size > max_size {
        let size_mb = size as f64 / (1024.0 * 1024.0);
        let max_mb = max_size as f64 / (1024.0 * 1024.0);
        return Err(Error::ResourceExhausted {
            resource_type: format!("file_size ({}MB > {:.2}MB)", size_mb, max_mb),
            limit: max_size,
            current: size,
        });
    }

    Ok(size)
}

/// Validate file size with default limit (50MB)
pub fn validate_file_size_default<P: AsRef<Path>>(file_path: P) -> Result<u64> {
    validate_file_size(file_path, DEFAULT_MAX_FILE_SIZE)
}

/// Sanitize a filename to prevent security issues.
///
/// # Arguments
/// * `filename` - The filename to sanitize
///
/// # Returns
/// Sanitized filename
///
/// # Errors
/// Returns an error if filename is empty or invalid after sanitization
pub fn sanitize_filename(filename: &str) -> Result<String> {
    if filename.is_empty() {
        return Err(Error::validation(
            "Filename cannot be empty",
            "filename",
            filename,
        ));
    }

    let mut sanitized = filename.to_string();

    // Remove null bytes
    sanitized = sanitized.replace('\0', "");

    // Remove path separators
    sanitized = sanitized.replace(['/', '\\'], "_");

    // Remove control characters
    sanitized = sanitized.chars().filter(|&c| c as u32 >= 32).collect();

    // Remove leading dots (hidden files)
    sanitized = sanitized.trim_start_matches('.').to_string();

    // Limit length (character count, not bytes)
    if sanitized.chars().count() > MAX_FILENAME_LENGTH {
        if let Some(pos) = sanitized.rfind('.') {
            let ext = &sanitized[pos..];
            let name_len = MAX_FILENAME_LENGTH - ext.chars().count();
            if name_len > 0 {
                sanitized = format!(
                    "{}{}",
                    truncate_for_display(&sanitized[..pos], name_len),
                    ext
                );
            } else {
                sanitized = truncate_for_display(&sanitized, MAX_FILENAME_LENGTH);
            }
        } else {
            sanitized = truncate_for_display(&sanitized, MAX_FILENAME_LENGTH);
        }
    }

    // Ensure filename is not empty after sanitization
    if sanitized.is_empty() || sanitized.trim_matches('.').is_empty() {
        return Err(Error::validation(
            "Filename is invalid after sanitization",
            "filename",
            filename,
        ));
    }

    Ok(sanitized)
}

/// Validate a database URL.
///
/// # Arguments
/// * `url` - Database URL to validate
///
/// # Returns
/// Validated URL
///
/// # Errors
/// Returns an error if URL is invalid
pub fn validate_database_url(url: &str) -> Result<String> {
    if url.is_empty() {
        return Err(Error::validation(
            "Database URL cannot be empty",
            "database_url",
            url,
        ));
    }

    // Only allow SQLite URLs for security
    let allowed_prefixes = ["sqlite://", "file://"];

    let has_valid_prefix = allowed_prefixes
        .iter()
        .any(|prefix| url.starts_with(prefix));

    if !has_valid_prefix {
        return Err(Error::validation(
            format!(
                "Only SQLite databases are supported. URL must start with one of: {:?}",
                allowed_prefixes
            ),
            "database_url",
            truncate_for_display(url, 50),
        ));
    }

    // Check for path traversal in file path
    if let Some(path_part) = url.split("://").nth(1)
        && !path_part.is_empty()
        && !path_part.starts_with(':')
    {
        // Check for path traversal
        if path_part.contains("../") || path_part.contains("..\\") {
            return Err(Error::validation(
                "Database path contains path traversal pattern",
                "database_url",
                truncate_for_display(url, 50),
            ));
        }
    }

    Ok(url.to_string())
}

/// Validate a language identifier.
///
/// # Arguments
/// * `language` - Language identifier to validate
///
/// # Returns
/// Validated language (lowercase)
///
/// # Errors
/// Returns an error if language is invalid
pub fn validate_language(language: &str) -> Result<String> {
    if language.is_empty() {
        return Err(Error::validation(
            "Language cannot be empty",
            "language",
            language,
        ));
    }

    let language_lower = language.to_lowercase();

    // Language should be lowercase letters, numbers, and underscores
    static VALID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[a-z][a-z0-9_]*$").expect("Static language validation regex pattern is valid")
    });
    if !VALID_REGEX.is_match(&language_lower) {
        return Err(Error::validation(
            format!("Invalid language identifier: '{}'", language),
            "language",
            language,
        ));
    }

    Ok(language_lower)
}

/// Validate a positive integer value.
///
/// # Arguments
/// * `value` - Value to validate
/// * `field_name` - Name of the field for error messages
///
/// # Returns
/// Validated integer
///
/// # Errors
/// Returns an error if value is not a positive integer
pub fn validate_positive_integer(value: i64, field_name: &str) -> Result<i64> {
    if value <= 0 {
        return Err(Error::validation(
            format!("{} must be positive, got: {}", field_name, value),
            field_name,
            value.to_string(),
        ));
    }

    Ok(value)
}

/// Validate a string as a positive integer.
///
/// # Arguments
/// * `value` - String value to validate
/// * `field_name` - Name of the field for error messages
///
/// # Returns
/// Validated integer
///
/// # Errors
/// Returns an error if value cannot be parsed or is not positive
pub fn validate_positive_integer_str(value: &str, field_name: &str) -> Result<i64> {
    let int_value: i64 = value.parse().map_err(|_| {
        Error::validation(
            format!("{} must be an integer, got: {}", field_name, value),
            field_name,
            value,
        )
    })?;

    validate_positive_integer(int_value, field_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_validate_file_path_empty() {
        let result = validate_file_path("", None, &PathValidationOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Path cannot be empty"));
    }

    #[test]
    fn test_validate_file_path_traversal() {
        let result = validate_file_path("../etc/passwd", None, &PathValidationOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("dangerous pattern"));
    }

    #[test]
    fn test_validate_file_path_home() {
        let result = validate_file_path("~/.bashrc", None, &PathValidationOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("home directory"));
    }

    #[test]
    fn test_validate_file_path_env_var() {
        let result = validate_file_path("$HOME/file", None, &PathValidationOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("environment variable"));
    }

    #[test]
    fn test_validate_file_path_with_base() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();
        let file_path = base_path.join("test.txt");
        File::create(&file_path).unwrap();

        // Valid path within base
        let result = validate_file_path(
            &file_path,
            Some(base_path),
            &PathValidationOptions::must_be_file(),
        );
        assert!(result.is_ok());

        // Path outside base (should fail)
        let outside_path = PathBuf::from("/etc/passwd");
        let result = validate_file_path(
            outside_path,
            Some(base_path),
            &PathValidationOptions::default(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_file_size() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"Hello, World!").unwrap();

        let size = validate_file_size(&file_path, 1024);
        assert!(size.is_ok());
        assert_eq!(size.unwrap(), 13);

        let size = validate_file_size(&file_path, 5);
        assert!(size.is_err());
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("test.txt").unwrap(), "test.txt");
        assert_eq!(sanitize_filename("../test.txt").unwrap(), "_test.txt");
        assert_eq!(sanitize_filename("file/name.txt").unwrap(), "file_name.txt");
        assert_eq!(sanitize_filename(".hidden").unwrap(), "hidden");

        // Empty filename should error
        assert!(sanitize_filename("").is_err());

        // Only dots should error
        assert!(sanitize_filename("...").is_err());
    }

    #[test]
    fn test_sanitize_filename_long() {
        let long_name = "a".repeat(300) + ".txt";
        let sanitized = sanitize_filename(&long_name).unwrap();
        assert!(sanitized.chars().count() <= MAX_FILENAME_LENGTH);
        assert!(sanitized.ends_with(".txt"));
    }

    #[test]
    fn test_validate_database_url() {
        assert!(validate_database_url("sqlite:///path/to/db.sqlite").is_ok());
        assert!(validate_database_url("file:///path/to/db.sqlite").is_ok());

        // Invalid protocols
        assert!(validate_database_url("postgres://localhost/db").is_err());
        assert!(validate_database_url("mysql://localhost/db").is_err());
        assert!(validate_database_url("http://example.com/db").is_err());

        // Empty URL
        assert!(validate_database_url("").is_err());

        // Path traversal
        assert!(validate_database_url("sqlite:///../etc/passwd").is_err());
    }

    #[test]
    fn test_validate_language() {
        assert_eq!(validate_language("python").unwrap(), "python");
        assert_eq!(validate_language("Rust").unwrap(), "rust");
        assert_eq!(validate_language("c_plus_plus").unwrap(), "c_plus_plus");

        // Invalid languages
        assert!(validate_language("").is_err());
        assert!(validate_language("123invalid").is_err());
        assert!(validate_language("test-file").is_err());
    }

    #[test]
    fn test_validate_positive_integer() {
        assert_eq!(validate_positive_integer(1, "count").unwrap(), 1);
        assert_eq!(validate_positive_integer(100, "limit").unwrap(), 100);

        assert!(validate_positive_integer(0, "count").is_err());
        assert!(validate_positive_integer(-5, "count").is_err());
    }

    #[test]
    fn test_validate_positive_integer_str() {
        assert_eq!(validate_positive_integer_str("42", "count").unwrap(), 42);
        assert_eq!(validate_positive_integer_str("1", "limit").unwrap(), 1);

        assert!(validate_positive_integer_str("0", "count").is_err());
        assert!(validate_positive_integer_str("-5", "count").is_err());
        assert!(validate_positive_integer_str("abc", "count").is_err());
        assert!(validate_positive_integer_str("", "count").is_err());
    }
}
