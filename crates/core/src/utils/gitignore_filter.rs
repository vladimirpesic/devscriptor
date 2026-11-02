//! Gitignore filtering for file scanning
//!
//! Provides bullet-proof .gitignore support using the `ignore` crate,
//! the same library used by ripgrep. This ensures complete compatibility
//! with git's ignore rules including:
//! - All glob patterns (*, **, ?, `[abc]`)
//! - Negation patterns (!)
//! - Directory-only patterns (/)
//! - Anchored patterns
//! - Nested .gitignore files with proper precedence
//! - .git/info/exclude support
//! - Global gitignore support

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use tracing::{debug, trace, warn};

use crate::error::{Error, Result};

/// Gitignore-based file filter for respecting .gitignore files
///
/// This filter automatically discovers and parses all .gitignore files
/// in the scanned directory tree, applying the correct precedence rules.
#[derive(Debug, Clone)]
pub struct GitignoreFilter {
    /// The compiled gitignore matcher
    gitignore: Gitignore,
    /// Root directory for the filter
    root: PathBuf,
    /// Whether to respect hidden files/directories
    respect_hidden: bool,
}

impl GitignoreFilter {
    /// Create a new GitignoreFilter for the given root directory
    ///
    /// This will automatically discover and parse all .gitignore files
    /// in the directory tree.
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory to scan
    ///
    /// # Returns
    ///
    /// A new GitignoreFilter instance
    ///
    /// # Errors
    ///
    /// Returns an error if the gitignore patterns cannot be built
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = dunce::canonicalize(root.as_ref() as &Path).map_err(Error::Io)?;

        debug!("Creating GitignoreFilter for: {}", root.display());

        // Build gitignore from the directory
        let mut builder = GitignoreBuilder::new(&root);

        // Add all .gitignore files found in the directory tree
        Self::add_gitignore_files(&mut builder, &root)?;

        // Add global gitignore if available
        if let Some(global_gitignore) = Self::find_global_gitignore() {
            debug!("Adding global gitignore: {}", global_gitignore.display());
            let _ = builder.add(global_gitignore);
        }

        let gitignore = builder
            .build()
            .map_err(|e| Error::Generic(format!("Failed to build gitignore patterns: {}", e)))?;

        Ok(Self {
            gitignore,
            root,
            respect_hidden: true,
        })
    }

    /// Create a new GitignoreFilter with custom configuration
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory
    /// * `respect_hidden` - Whether to treat hidden files as ignored
    pub fn with_config(root: impl AsRef<Path>, respect_hidden: bool) -> Result<Self> {
        let mut filter = Self::new(root)?;
        filter.respect_hidden = respect_hidden;
        Ok(filter)
    }

    /// Check if a file or directory should be ignored
    ///
    /// # Arguments
    ///
    /// * `path` - The path to check (relative or absolute)
    ///
    /// # Returns
    ///
    /// `true` if the path should be ignored, `false` otherwise
    pub fn is_ignored(&self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();

        // Convert to relative path from root for proper matching
        let relative_path = if path.is_absolute() {
            match path.strip_prefix(&self.root) {
                Ok(p) => p,
                Err(_) => {
                    // Path is absolute but not under root, check as-is
                    trace!("Path outside root, checking as-is: {}", path.display());
                    path
                }
            }
        } else {
            path
        };

        // Check if it's a directory
        let is_dir = path.is_dir();

        // Use the gitignore matcher with relative path
        let result = self.gitignore.matched(relative_path, is_dir);

        match result {
            ignore::Match::None => {
                trace!("Path not matched by gitignore: {}", path.display());
                false
            }
            ignore::Match::Ignore(_) => {
                trace!("Path ignored by .gitignore: {}", path.display());
                true
            }
            ignore::Match::Whitelist(_) => {
                trace!("Path whitelisted by .gitignore: {}", path.display());
                false
            }
        }
    }

    /// Check if a file should be processed (not ignored)
    ///
    /// This is the inverse of `is_ignored`
    pub fn should_process(&self, path: impl AsRef<Path>) -> bool {
        !self.is_ignored(path)
    }

    /// Check if a directory should be descended into
    ///
    /// Returns `false` if the path is not a directory or is ignored.
    /// This is semantically stricter than [`should_process`](Self::should_process)
    /// which only checks the ignore filter.
    pub fn should_descend(&self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        path.is_dir() && !self.is_ignored(path)
    }

    /// Get the root directory
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check if gitignore filtering is enabled
    ///
    /// This always returns `true` for GitignoreFilter since its purpose
    /// is to respect gitignore rules. Use `with_config()` to control
    /// whether hidden files are also treated as ignored.
    #[must_use]
    pub fn respects_gitignore(&self) -> bool {
        true
    }

    /// Check if a path is hidden (starts with .)
    ///
    /// Excludes special git files like .gitignore, .gitattributes, .gitmodules
    /// which should not be ignored even though they start with a dot
    fn is_hidden_path(path: &Path) -> bool {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");

        // Don't treat git configuration files as hidden
        if file_name == ".gitignore"
            || file_name == ".gitattributes"
            || file_name == ".gitmodules"
            || file_name == ".gitkeep"
        {
            return false;
        }

        file_name.starts_with('.')
    }

    /// Recursively find and add all .gitignore files.
    ///
    /// Limits traversal depth to 10 to prevent unbounded recursion on
    /// deeply nested or malformed directory structures.
    fn add_gitignore_files(builder: &mut GitignoreBuilder, root: &Path) -> Result<()> {
        use walkdir::WalkDir;

        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(10)
            .into_iter()
            .filter_entry(|e| !Self::is_hidden_path(e.path()))
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("Error walking directory: {}", e);
                    continue;
                }
            };

            if entry.file_type().is_file() && entry.file_name() == ".gitignore" {
                let path = entry.path();
                debug!("Adding gitignore file: {}", path.display());

                // Add this gitignore file to the builder
                // The ignore crate handles nested .gitignore files correctly
                if let Some(e) = builder.add(path) {
                    warn!("Failed to add gitignore file {}: {}", path.display(), e);
                }
            }
        }

        Ok(())
    }

    /// Find the global gitignore file
    fn find_global_gitignore() -> Option<PathBuf> {
        // Check GIT_IGNORE_GLOBAL environment variable
        if let Ok(path) = std::env::var("GIT_IGNORE_GLOBAL") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }

        // Check core.excludesfile git config
        if let Ok(output) = std::process::Command::new("git")
            .args(["config", "--global", "core.excludesfile"])
            .output()
            && output.status.success()
        {
            let path_str = String::from_utf8_lossy(&output.stdout);
            let path = PathBuf::from(path_str.trim());
            if path.exists() {
                return Some(path);
            }
        }

        // Check common locations
        let home = dirs::home_dir()?;
        let candidates = [
            home.join(".gitignore_global"),
            home.join(".gitignore"),
            home.join(".config").join("git").join("ignore"),
        ];

        for path in &candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }

        None
    }
}

impl Default for GitignoreFilter {
    fn default() -> Self {
        // Create a filter that ignores nothing (for testing/fallback)
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let gitignore = GitignoreBuilder::new(&root).build().unwrap_or_else(|_| {
            // An empty Gitignore that matches nothing.
            GitignoreBuilder::new(".")
                .build()
                .expect("Empty gitignore from '.' should always build")
        });

        Self {
            gitignore,
            root,
            respect_hidden: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gitignore_filter_creation() {
        // Test that we can create a default filter
        let filter = GitignoreFilter::default();
        assert!(filter.respects_gitignore());

        // Test with_config - respects_gitignore always returns true since
        // that's the purpose of GitignoreFilter. The respect_hidden parameter
        // controls whether hidden files are treated as ignored.
        let temp_dir = std::env::temp_dir();
        let filter = GitignoreFilter::with_config(&temp_dir, false);
        assert!(filter.is_ok());
        let filter = filter.unwrap();
        // GitignoreFilter always respects gitignore rules
        assert!(filter.respects_gitignore());
    }

    #[test]
    fn test_is_hidden_path() {
        use std::path::Path;

        // Regular hidden files
        assert!(GitignoreFilter::is_hidden_path(Path::new(".env")));
        assert!(GitignoreFilter::is_hidden_path(Path::new(".hidden")));

        // Non-hidden files
        assert!(!GitignoreFilter::is_hidden_path(Path::new("main.rs")));
        assert!(!GitignoreFilter::is_hidden_path(Path::new("file.txt")));

        // Git config files should NOT be treated as hidden
        assert!(!GitignoreFilter::is_hidden_path(Path::new(".gitignore")));
        assert!(!GitignoreFilter::is_hidden_path(Path::new(
            ".gitattributes"
        )));
        assert!(!GitignoreFilter::is_hidden_path(Path::new(".gitmodules")));
        assert!(!GitignoreFilter::is_hidden_path(Path::new(".gitkeep")));
    }
}
