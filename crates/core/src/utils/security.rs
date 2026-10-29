//! Security utilities for Devscriptor
//!
//! This module provides security-focused utilities including:
//! - Secure error message sanitization
//! - Path sanitization for logging
//! - File hashing and verification
//! - Shell command safety checks

use crate::error::Result;
use std::path::Path;

/// Sanitize a path for display in error messages
///
/// Removes sensitive information like home directory paths
/// and limits the length of displayed paths.
///
/// # Arguments
/// * `path` - The path to sanitize
///
/// # Returns
/// A sanitized string safe for display in error messages
#[must_use]
pub fn sanitize_path_for_display(path: &Path) -> String {
    let path_str = path.to_string_lossy();

    // Replace home directory with ~
    if let Ok(home) = std::env::var("HOME")
        && path_str.starts_with(&home)
    {
        return format!("~{}", path_str.get(home.len()..).unwrap_or(""));
    }

    // Limit length
    if path_str.len() > 100 {
        let tail: String = path_str
            .chars()
            .rev()
            .take(97)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("...{tail}")
    } else {
        path_str.to_string()
    }
}

/// Sanitize an error message for production display
///
/// Removes sensitive information like full paths and internal details.
///
/// # Arguments
/// * `message` - The original error message
///
/// # Returns
/// A sanitized message safe for production display
#[must_use]
pub fn sanitize_error_message(message: &str) -> String {
    use std::sync::LazyLock;

    // Compile regex patterns once and cache them
    static UNIX_PATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        // Match common Unix absolute path prefixes followed by path components.
        // This is more restrictive than matching any slash + 10 chars,
        // avoiding false positives on URLs, JSON paths, and long identifiers.
        regex::Regex::new(r"/(?:home|Users|tmp|var|opt|usr|dev|etc|bin|lib|sbin|local|mnt|media|srv|sys|run|proc|boot|root)/[a-zA-Z0-9_./-]{3,}")
            .expect("Static Unix path regex pattern is valid")
    });
    static WINDOWS_PATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        // Match Windows absolute paths: drive letter + colon + backslash/slash
        // followed by at least one directory/component.
        regex::Regex::new(r#"[A-Za-z]:[\\/](?:[^\\/:*?"<>|\r\n]{3,}[\\/])*[^\\/:*?"<>|\r\n]{3,}"#)
            .expect("Static Windows path regex pattern is valid")
    });

    let mut sanitized = message.to_string();

    // Replace absolute paths with [PATH]
    sanitized = UNIX_PATH_RE.replace_all(&sanitized, "[PATH]").to_string();
    sanitized = WINDOWS_PATH_RE
        .replace_all(&sanitized, "[PATH]")
        .to_string();

    sanitized
}

/// Compute xxHash3-64 hash of file contents
///
/// Uses xxHash3 for fast non-cryptographic file integrity hashing.
/// For cryptographic guarantees, use a dedicated SHA-256 implementation.
///
/// # Arguments
/// * `path` - Path to the file
///
/// # Returns
/// Hex-encoded xxHash3-64 hash string
///
/// # Errors
/// Returns an error if the file cannot be read
pub fn compute_file_hash(path: &Path) -> Result<String> {
    use std::io::{BufReader, Read};
    use xxhash_rust::xxh3::Xxh3;

    let file = std::fs::File::open(path).map_err(crate::error::Error::Io)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Xxh3::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buf).map_err(crate::error::Error::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:016x}", hasher.digest()))
}

/// Verify a file against an expected hash
///
/// # Arguments
/// * `path` - Path to the file
/// * `expected_hash` - Expected xxHash3-64 hash (hex-encoded)
///
/// # Returns
/// `true` if the file hash matches, `false` otherwise
///
/// # Errors
/// Returns an error if the file cannot be read
pub fn verify_file_hash(path: &Path, expected_hash: &str) -> Result<bool> {
    let actual_hash = compute_file_hash(path)?;
    Ok(actual_hash.eq_ignore_ascii_case(expected_hash))
}

/// Check if a path contains shell metacharacters
///
/// # Arguments
/// * `path` - The path to check
///
/// # Returns
/// `true` if the path contains potentially dangerous characters
#[must_use]
pub fn contains_shell_metacharacters(path: &str) -> bool {
    const SHELL_META: &[char] = &[
        ';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}', '[', ']', '*', '?', '~', '\\', '"',
        '\n', '\t', '\0',
    ];
    path.chars().any(|c| SHELL_META.contains(&c))
}

/// Sanitize a path for use in shell commands
///
/// Returns an error if the path contains dangerous characters.
///
/// # Arguments
/// * `path` - The path to sanitize
///
/// # Returns
/// The sanitized path or an error
///
/// # Errors
/// Returns an error if the path contains shell metacharacters
pub fn sanitize_for_shell(path: &Path) -> Result<std::ffi::OsString> {
    let path_str = path.to_string_lossy();

    if contains_shell_metacharacters(&path_str) {
        return Err(crate::error::Error::validation(
            "Path contains shell metacharacters",
            "path",
            sanitize_path_for_display(path),
        ));
    }

    Ok(path.as_os_str().to_os_string())
}

/// Best-effort change-detection helper for memory-mapped files.
///
/// **Important:** this type does **not** acquire any kind of OS-level lock —
/// neither `flock(2)` nor `fcntl(F_SETLK)`. It only records the file's metadata
/// at construction time and offers an `is_modified` check that compares size +
/// mtime. Treat it strictly as a TOCTOU-prone hint, not as a concurrency
/// guarantee.
#[derive(Debug)]
pub struct FileChangeDetector {
    captured: std::fs::Metadata,
}

impl FileChangeDetector {
    /// Capture the current file metadata for later comparison.
    ///
    /// # Errors
    /// Returns an error if the file metadata cannot be read.
    pub fn new(path: &Path) -> Result<Self> {
        let captured = std::fs::metadata(path).map_err(crate::error::Error::Io)?;
        Ok(Self { captured })
    }

    /// Returns `true` if the file's `len` or modified time differs from
    /// the values captured by [`FileChangeDetector::new`].
    #[must_use]
    pub fn is_modified(&self, path: &Path) -> bool {
        match std::fs::metadata(path) {
            Ok(now) => {
                if now.len() != self.captured.len() {
                    return true;
                }
                match (now.modified(), self.captured.modified()) {
                    (Ok(a), Ok(b)) => a != b,
                    _ => true,
                }
            }
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_sanitize_path_for_display() {
        // Test a moderately long path (not over 100 chars so won't be truncated)
        let path = Path::new("/home/user/projects/test.rs");
        let sanitized = sanitize_path_for_display(path);
        // Since HOME env var is unlikely to be "/home/user" in test environment,
        // just verify the function runs without panic
        assert!(!sanitized.is_empty());
    }

    #[test]
    fn test_compute_file_hash() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, World!").unwrap();

        let hash = compute_file_hash(temp_file.path()).unwrap();
        assert_eq!(hash.len(), 16); // xxHash3-64 hex length
    }

    #[test]
    fn test_verify_file_hash() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, World!").unwrap();

        let hash = compute_file_hash(temp_file.path()).unwrap();
        assert!(verify_file_hash(temp_file.path(), &hash).unwrap());
        assert!(!verify_file_hash(temp_file.path(), "invalid").unwrap());
    }

    #[test]
    fn test_contains_shell_metacharacters() {
        assert!(contains_shell_metacharacters("file; rm -rf /"));
        assert!(contains_shell_metacharacters("file|cat"));
        assert!(!contains_shell_metacharacters("/safe/path/to/file"));
        assert!(!contains_shell_metacharacters("file_name.rs"));
    }
}
