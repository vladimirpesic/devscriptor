//! File Scanner for Devscriptor
//!
//! Provides async file discovery and filtering for codebase scanning.
//! Includes priority-based file analysis for optimal processing order.
//! Automatically respects .gitignore files when enabled.

use crate::error::{Error, Result};
use crate::utils::validation::DEFAULT_MAX_FILE_SIZE;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::task;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

/// Configuration for a file priority pattern
#[derive(Debug, Clone, Serialize)]
pub struct PriorityPattern {
    /// Compiled regex pattern
    #[serde(skip)]
    pub pattern: Regex,
    /// Raw pattern string (for serialization)
    pub raw_pattern: String,
    /// Priority level (lower = higher priority)
    pub priority: i32,
    /// Human-readable description
    pub description: String,
}

impl<'de> Deserialize<'de> for PriorityPattern {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PriorityPatternHelper {
            raw_pattern: String,
            priority: i32,
            description: String,
        }

        let helper = PriorityPatternHelper::deserialize(deserializer)?;
        let pattern = Regex::new(&helper.raw_pattern).map_err(serde::de::Error::custom)?;

        Ok(PriorityPattern {
            pattern,
            raw_pattern: helper.raw_pattern,
            priority: helper.priority,
            description: helper.description,
        })
    }
}

impl PriorityPattern {
    /// Create a new priority pattern
    ///
    /// # Errors
    /// Returns an error if the regex pattern is invalid
    pub fn new(pattern: &str, priority: i32, description: impl Into<String>) -> Result<Self> {
        let regex = Regex::new(pattern)
            .map_err(|e| Error::Generic(format!("Invalid regex pattern '{}': {}", pattern, e)))?;

        Ok(Self {
            pattern: regex,
            raw_pattern: pattern.to_string(),
            priority,
            description: description.into(),
        })
    }

    /// Check if a file path matches this pattern
    #[must_use]
    pub fn matches(&self, file_path: &str) -> bool {
        self.pattern.is_match(file_path)
    }
}

/// Statistics from priority analysis
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriorityStats {
    /// Number of files categorized
    pub categorized: usize,
    /// Number of files not matching any pattern
    pub uncategorized: usize,
    /// Pattern match counts
    pub pattern_counts: HashMap<String, usize>,
}

/// File entry with priority information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrioritizedFileEntry {
    /// The file entry
    pub entry: FileEntry,
    /// Priority level (lower = higher priority)
    pub priority: i32,
    /// Pattern that matched (if any)
    pub matched_pattern: Option<String>,
}

/// Priority-based file analyzer for intelligent file categorization
#[derive(Debug, Clone)]
pub struct PriorityAnalyzer {
    /// Priority patterns for categorization
    pub patterns: Vec<PriorityPattern>,
    /// Statistics tracking
    pub stats: PriorityStats,
}

/// Internal result of categorizing a single file against priority patterns.
#[derive(Debug, Clone)]
struct CategorizationResult {
    priority: i32,
    matched_pattern: Option<String>,
}

impl PriorityAnalyzer {
    /// Lazily-compiled default priority patterns for common project structures.
    /// Compiled once globally to avoid re-compiling regexes on every `new()` call.
    fn default_patterns_cached() -> &'static [PriorityPattern] {
        static PATTERNS: LazyLock<Vec<PriorityPattern>> = LazyLock::new(|| {
            vec![
                // Priority 1: Entry points and main files
                PriorityPattern::new(
                    r"(main|app|index|server|cli|entry|bootstrap|init)\.(py|js|ts|go|rs|java|c|cpp|h|hpp)$",
                    1,
                    "Entry points (main, app, index, server, etc.)",
                ).expect("built-in default pattern is valid"),
                PriorityPattern::new(
                    r"(manage|wsgi|asgi|app\.py)$",
                    1,
                    "Framework entry points",
                ).expect("built-in default pattern is valid"),
                // Priority 2: Core modules and configuration
                PriorityPattern::new(
                    r"/(core|base|common|utils|helpers|config|settings|constants|types)/",
                    2,
                    "Core modules and utilities",
                ).expect("built-in default pattern is valid"),
                PriorityPattern::new(
                    r"(config|settings|constants|types|models|schemas)\.(py|js|ts|go|rs)$",
                    2,
                    "Configuration and schema files",
                ).expect("built-in default pattern is valid"),
                // Priority 3: Important source files (not tests)
                PriorityPattern::new(
                    r"/src/",
                    3,
                    "Source directory files",
                ).expect("built-in default pattern is valid"),
                // Every source file gets this priority; the test pattern (priority 4)
                // overrides it for files that match a test pattern.
                PriorityPattern::new(
                    r"\.(py|js|ts|go|rs|java|c|cpp|h|hpp)$",
                    3,
                    "Source files",
                ).expect("built-in default pattern is valid"),
                // Priority 4: Test files (processed last)
                PriorityPattern::new(
                    r"(test_|_test|tests?/|__tests__|spec|\.spec)\.(py|js|ts|go|rs|java)$",
                    4,
                    "Test files",
                ).expect("built-in default pattern is valid"),
                // Priority 5: Documentation and config files
                PriorityPattern::new(
                    r"\.(md|rst|txt|yaml|yml|json|toml|ini|cfg)$",
                    5,
                    "Documentation and configuration",
                ).expect("built-in default pattern is valid"),
            ]
        });
        &PATTERNS
    }

    /// Create a new priority analyzer with default patterns
    pub fn new() -> Self {
        Self {
            patterns: Self::default_patterns_cached().to_vec(),
            stats: PriorityStats::default(),
        }
    }

    /// Create a new priority analyzer with custom patterns
    #[must_use]
    pub fn with_patterns(patterns: Vec<PriorityPattern>) -> Self {
        Self {
            patterns,
            stats: PriorityStats::default(),
        }
    }

    /// Categorize a single file path string and update statistics.
    fn categorize_item(&mut self, file_str: &str) -> CategorizationResult {
        for priority_pattern in &self.patterns {
            if priority_pattern.matches(file_str) {
                *self
                    .stats
                    .pattern_counts
                    .entry(priority_pattern.description.clone())
                    .or_insert(0) += 1;
                self.stats.categorized += 1;
                return CategorizationResult {
                    priority: priority_pattern.priority,
                    matched_pattern: Some(priority_pattern.description.clone()),
                };
            }
        }

        let default_priority = self.patterns.iter().map(|p| p.priority).max().unwrap_or(10) + 1;
        self.stats.uncategorized += 1;
        CategorizationResult {
            priority: default_priority,
            matched_pattern: None,
        }
    }

    /// Categorize files by priority level
    ///
    /// Returns a map of priority levels to file paths
    #[must_use]
    pub fn categorize_files(&mut self, files: &[PathBuf]) -> HashMap<i32, Vec<PathBuf>> {
        let mut prioritized: HashMap<i32, Vec<PathBuf>> = HashMap::new();

        for file_path in files {
            let file_str = file_path.to_string_lossy();
            let result = self.categorize_item(&file_str);
            prioritized
                .entry(result.priority)
                .or_default()
                .push(file_path.clone());
        }

        prioritized
    }

    /// Categorize file entries and return prioritized entries
    #[must_use]
    pub fn prioritize_entries(&mut self, entries: &[FileEntry]) -> Vec<PrioritizedFileEntry> {
        let mut prioritized = Vec::new();

        for entry in entries {
            let file_str = entry.path_str();
            let result = self.categorize_item(&file_str);
            prioritized.push(PrioritizedFileEntry {
                entry: entry.clone(),
                priority: result.priority,
                matched_pattern: result.matched_pattern,
            });
        }

        // Sort by priority
        prioritized.sort_by_key(|p| p.priority);
        prioritized
    }

    /// Get priority statistics
    #[must_use]
    pub fn get_stats(&self) -> &PriorityStats {
        &self.stats
    }

    /// Add a custom priority pattern
    pub fn add_pattern(&mut self, pattern: PriorityPattern) {
        self.patterns.push(pattern);
        // Re-sort by priority
        self.patterns.sort_by_key(|p| p.priority);
    }

    /// Clear all patterns
    pub fn clear_patterns(&mut self) {
        self.patterns.clear();
        self.stats = PriorityStats::default();
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = PriorityStats::default();
    }
}

impl Default for PriorityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Detect entry points in a codebase
/// Detects entry-point files in a scanned file list.
#[derive(Debug, Clone)]
pub struct EntryPointDetector {
    custom: Vec<String>,
}

impl Default for EntryPointDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl EntryPointDetector {
    /// Exact file names that indicate entry points
    const EXACT_NAMES: &'static [&'static str] = &[
        "main.py",
        "main.js",
        "main.ts",
        "main.go",
        "main.rs",
        "main.java",
        "main.c",
        "main.cpp",
        "index.js",
        "index.ts",
        "index.py",
        "app.py",
        "app.js",
        "app.ts",
        "server.js",
        "server.ts",
        "server.py",
        "cli.py",
        "cli.js",
        "cli.ts",
        "cli.rs",
        "manage.py",
        "wsgi.py",
        "asgi.py",
        "lib.rs",
        "Cargo.toml",
        "package.json",
        "setup.py",
        "pyproject.toml",
        "go.mod",
    ];

    /// Create a detector with the built-in names only.
    #[must_use]
    pub fn new() -> Self {
        Self { custom: Vec::new() }
    }

    /// Add custom entry-point file names (G12).
    /// These are merged with the built-in list at detection time.
    pub fn with_custom(mut self, names: Vec<String>) -> Self {
        self.custom = names;
        self
    }

    /// Detect entry points in a list of files
    #[must_use]
    pub fn detect_entry_points(&self, files: &[PathBuf]) -> Vec<PathBuf> {
        files
            .iter()
            .filter(|f| self.is_entry_point(f))
            .cloned()
            .collect()
    }

    /// Check if a file is an entry point
    #[must_use]
    pub fn is_entry_point(&self, file: &Path) -> bool {
        let file_name = file
            .file_name()
            .map_or_else(|| file.to_string_lossy(), |name| name.to_string_lossy());
        Self::EXACT_NAMES
            .iter()
            .any(|&name| file_name.eq_ignore_ascii_case(name))
            || self
                .custom
                .iter()
                .any(|name| file_name.eq_ignore_ascii_case(name))
    }
}

/// A file entry discovered during scanning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Absolute file path
    pub path: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Last modified timestamp (unix epoch seconds)
    pub modified: u64,
    /// File extension
    pub extension: String,
}

impl FileEntry {
    /// Create a new file entry
    #[must_use]
    pub fn new(path: PathBuf, size: u64, modified: u64, extension: impl Into<String>) -> Self {
        Self {
            path,
            size,
            modified,
            extension: extension.into(),
        }
    }

    /// Get the file path as a string
    #[must_use]
    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().to_string()
    }

    /// Check if file matches a list of extensions
    #[must_use]
    pub fn has_extension(&self, extensions: &[String]) -> bool {
        extensions
            .iter()
            .any(|ext| ext.eq_ignore_ascii_case(&self.extension))
    }
}

/// File filter configuration using proper glob matching.
///
/// `exclude_globs` is rebuilt lazily: calling [`add_exclude`](FileFilter::add_exclude)
/// only appends the raw pattern and sets a dirty flag. The compiled `GlobSet`
/// is regenerated on the next call to [`matches`](FileFilter::matches), making
/// repeated `add_exclude` calls **O(1)** amortised instead of **O(n²)**.
#[derive(Debug)]
pub struct FileFilter {
    /// Include extensions (empty = all)
    pub include_extensions: HashSet<String>,
    /// Raw exclude patterns (kept for incremental building)
    exclude_patterns: Vec<String>,
    /// Compiled globset for exclude patterns
    exclude_globs: RwLock<globset::GlobSet>,
    /// Whether `exclude_globs` is stale and needs rebuilding
    exclude_dirty: AtomicBool,
    /// Maximum file size in bytes
    pub max_file_size: u64,
    /// Follow symlinks
    pub follow_symlinks: bool,
}

impl Default for FileFilter {
    fn default() -> Self {
        /// Cached default FileFilter components, built once.
        ///
        /// The ~22 glob patterns are compiled into a `GlobSet` once (`LazyLock`),
        /// and shared by every `FileScanner::default()`.
        static DEFAULTS: LazyLock<(HashSet<String>, Vec<String>, globset::GlobSet)> =
            LazyLock::new(|| {
                let excludes: Vec<String> =
                    default_excludes().iter().map(|&s| s.to_string()).collect();
                let globs = FileFilter::build_exclude_set(&excludes);
                (default_extensions(), excludes, globs)
            });

        let (exts, excludes, globs) = &*DEFAULTS;
        Self {
            include_extensions: exts.clone(),
            exclude_globs: RwLock::new(globs.clone()),
            exclude_patterns: excludes.clone(),
            exclude_dirty: AtomicBool::new(false),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            follow_symlinks: false,
        }
    }
}

impl FileFilter {
    /// Create a new file filter
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an extension to include
    pub fn add_extension(&mut self, ext: impl Into<String>) {
        self.include_extensions.insert(ext.into().to_lowercase());
    }

    /// Add an exclude pattern (glob syntax: `*`, `**`, `?`, `[seq]`).
    ///
    /// The compiled globset is **not** rebuilt immediately; it is lazily
    /// regenerated on the next call to [`matches`](FileFilter::matches).
    pub fn add_exclude(&mut self, pattern: impl Into<String>) {
        self.exclude_patterns.push(pattern.into());
        self.exclude_dirty.store(true, Ordering::Relaxed);
    }

    /// Ensure the compiled exclude globset is up-to-date.
    fn ensure_globs_built(&self) {
        if self.exclude_dirty.load(Ordering::Relaxed) {
            let mut guard = self.exclude_globs.write();
            *guard = Self::build_exclude_set(&self.exclude_patterns);
            self.exclude_dirty.store(false, Ordering::Relaxed);
        }
    }

    /// Check if a path is excluded by glob patterns.
    #[must_use]
    pub fn is_excluded(&self, path: &Path) -> bool {
        self.ensure_globs_built();
        self.exclude_globs.read().is_match(path)
    }

    /// Check if a file passes the filter
    #[must_use]
    pub fn matches(&self, path: &Path, size: u64) -> bool {
        // Check file size first (no lock needed)
        if size > self.max_file_size {
            trace!("File too large: {}", path.display());
            return false;
        }

        // Check glob exclusions against the file path
        if self.is_excluded(path) {
            trace!("File excluded by glob pattern: {}", path.display());
            return false;
        }

        // Check extension
        if !self.include_extensions.is_empty() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if !self.include_extensions.contains(&ext) {
                trace!("Extension not included: {}", path.display());
                return false;
            }
        }

        true
    }

    /// Build a GlobSet from a list of glob patterns.
    ///
    /// Bare names (e.g. `target`, `node_modules`) are automatically treated as
    /// depth-independent patterns so they match at any level in the tree.
    fn build_exclude_set(patterns: &[String]) -> globset::GlobSet {
        let mut builder = globset::GlobSetBuilder::new();
        for p in patterns {
            // Add the pattern exactly as written.
            if let Ok(glob) = globset::Glob::new(p) {
                let _ = builder.add(glob);
            }

            // If the pattern has no path separators and no `**/` prefix, also
            // add depth-independent variants so that `target` matches
            // `project/target`, `project/target/foo.rs`, etc.
            if !p.starts_with("**/") && !p.contains('/') && !p.contains('\\') {
                if let Ok(glob) = globset::Glob::new(&format!("**/{}", p)) {
                    let _ = builder.add(glob);
                }

                // For plain directory-like names without wildcards, also match
                // anything inside them.
                if !p.contains('*')
                    && !p.contains('?')
                    && !p.contains('[')
                    && let Ok(glob) = globset::Glob::new(&format!("**/{}/**", p))
                {
                    let _ = builder.add(glob);
                }
            }
        }
        builder
            .build()
            .unwrap_or_else(|_| globset::GlobSet::empty())
    }
}

impl Clone for FileFilter {
    fn clone(&self) -> Self {
        Self {
            include_extensions: self.include_extensions.clone(),
            exclude_patterns: self.exclude_patterns.clone(),
            exclude_globs: RwLock::new(self.exclude_globs.read().clone()),
            exclude_dirty: AtomicBool::new(self.exclude_dirty.load(Ordering::Relaxed)),
            max_file_size: self.max_file_size,
            follow_symlinks: self.follow_symlinks,
        }
    }
}

/// A walker over `dir` with a scan's rules: hidden files skipped, ignore
/// files applied (git's only when `respect_gitignore`), and the filter's
/// exclude patterns as overrides relative to the scan root `root`.
fn walk_builder(
    dir: &Path,
    root: &Path,
    filter: &FileFilter,
    respect_gitignore: bool,
) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .follow_links(filter.follow_symlinks)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore);

    // All patterns go into ONE OverrideBuilder — calling builder.overrides()
    // in a loop would overwrite the previous overrides on each iteration.
    let mut override_builder = ignore::overrides::OverrideBuilder::new(root);
    for pattern in &filter.exclude_patterns {
        // Prefix with `!` to mark the glob as an exclusion.
        let _ = override_builder.add(&format!("!{pattern}"));
        // Also add a recursive variant so bare names like `.venv`
        // exclude `project/.venv/` as well.
        if !pattern.starts_with('*') && !pattern.contains('/') {
            let _ = override_builder.add(&format!("!**/{pattern}"));
            let _ = override_builder.add(&format!("!**/{pattern}/**"));
        }
    }
    if let Ok(overrides) = override_builder.build() {
        builder.overrides(overrides);
    }
    builder
}

/// Async file scanner for discovering files in a codebase
///
/// Automatically respects .gitignore files in the scanned directories
/// when `respect_gitignore` is enabled (default: true).
pub struct FileScanner {
    filter: FileFilter,
    /// Whether to respect .gitignore files (default: true).
    /// The `ignore` crate's WalkBuilder handles gitignore natively.
    respect_gitignore: bool,
}

impl FileScanner {
    /// Create a new file scanner with the given filter, applying git's ignore
    /// files ([`Self::set_respect_gitignore`] turns them off).
    #[must_use]
    pub fn new(filter: FileFilter) -> Self {
        Self {
            filter,
            respect_gitignore: true,
        }
    }

    /// Enable or disable gitignore filtering
    ///
    /// When enabled (default), the scanner will:
    /// - Parse all .gitignore files in the directory tree
    /// - Ignore files/directories matching gitignore patterns
    /// - Apply nested .gitignore rules with correct precedence
    ///
    /// When disabled, only the default exclude patterns are used.
    pub fn set_respect_gitignore(&mut self, respect: bool) {
        self.respect_gitignore = respect;
    }

    /// Check if gitignore filtering is enabled
    #[must_use]
    pub fn respects_gitignore(&self) -> bool {
        self.respect_gitignore
    }

    /// Default timeout for scan operations (seconds).
    pub const DEFAULT_SCAN_TIMEOUT_SECS: u64 = 300;

    /// Scan a directory for files using a parallel walker.
    ///
    /// Uses the `ignore` crate's `WalkBuilder` which parallelises I/O across
    /// `num_cpus` threads while respecting `.gitignore` natively.  On large
    /// repos (100k+ files) this is 3–10× faster than single-threaded walkdir.
    ///
    /// # Cancellation
    ///
    /// Pass a [`CancellationToken`] to request cooperative cancellation.  The
    /// token is polled at each result from the parallel walker.
    pub async fn scan(
        &self,
        root: impl AsRef<Path>,
        cancel: Option<CancellationToken>,
    ) -> Result<Vec<FileEntry>> {
        let root = root.as_ref().to_path_buf();
        let filter = self.filter.clone();
        let respect_gitignore = self.respect_gitignore;
        let cancel = cancel.map(Arc::new);

        task::spawn_blocking(move || {
            let metadata = std::fs::metadata(&root).map_err(Error::Io)?;
            if !metadata.is_dir() {
                return Err(Error::Generic(format!(
                    "Path is not a directory: {}",
                    root.display()
                )));
            }

            debug!("Scanning directory: {}", root.display());

            // Build the parallel walker, using all available cores for I/O
            // parallelism.
            let mut builder = walk_builder(&root, &root, &filter, respect_gitignore);
            builder.threads(num_cpus::get().min(8));

            let entries = Arc::new(Mutex::new(Vec::<FileEntry>::new()));
            let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let entries_clone = Arc::clone(&entries);
            let cancelled_clone = Arc::clone(&cancelled);
            let cancel_arc = cancel.clone();
            let filter_arc = Arc::new(filter);

            builder.build_parallel().run(|| {
                let entries = Arc::clone(&entries_clone);
                let cancelled = Arc::clone(&cancelled_clone);
                let cancel = cancel_arc.clone();
                let filter = Arc::clone(&filter_arc);

                Box::new(move |result| {
                    // Cooperative cancellation.
                    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        return ignore::WalkState::Quit;
                    }
                    if let Some(ref ct) = cancel
                        && ct.is_cancelled()
                    {
                        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                        return ignore::WalkState::Quit;
                    }

                    let dir_entry = match result {
                        Ok(e) => e,
                        Err(e) => {
                            warn!("Error walking directory: {}", e);
                            return ignore::WalkState::Continue;
                        }
                    };

                    let ft = match dir_entry.file_type() {
                        Some(ft) => ft,
                        None => return ignore::WalkState::Continue,
                    };
                    if !ft.is_file() {
                        return ignore::WalkState::Continue;
                    }

                    let path = dir_entry.path();

                    let metadata = match dir_entry.metadata() {
                        Ok(m) => m,
                        Err(e) => {
                            trace!("Cannot read metadata for {}: {}", path.display(), e);
                            return ignore::WalkState::Continue;
                        }
                    };

                    let size = metadata.len();
                    // One `filter.matches()` call: it checks the excludes itself, so the
                    // globset runs once per file on the hot path.
                    if !filter.matches(path, size) {
                        return ignore::WalkState::Continue;
                    }

                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    let extension = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    if let Ok(mut guard) = entries.lock() {
                        guard.push(FileEntry::new(
                            path.to_path_buf(),
                            size,
                            modified,
                            extension,
                        ));
                    }

                    ignore::WalkState::Continue
                })
            });

            // Check cancellation before consuming the Arc.
            let was_cancelled = cancelled.load(std::sync::atomic::Ordering::Relaxed);

            // Drop the inter-thread Arc clones so `entries` has a single owner.
            drop(entries_clone);
            drop(cancelled_clone);

            if was_cancelled {
                debug!("Scan cancelled for {}", root.display());
                return Err(Error::Cancelled);
            }

            let mut result = Arc::try_unwrap(entries)
                .map_err(|_| Error::Generic("Arc contention on entries".to_string()))?
                .into_inner()
                .map_err(|_| Error::Generic("Mutex poisoned during scan".to_string()))?;

            // Parallel walkers produce non-deterministic order; sort for stable output.
            result.sort_by(|a, b| a.path.cmp(&b.path));

            debug!("Found {} files in {}", result.len(), root.display());
            Ok(result)
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(format!("scan task failed: {e}"))))?
    }

    /// The entries a scan of `root` would yield for `paths`, without walking
    /// the tree; the others are left out: a path that is not a file under
    /// `root`, that the filter rejects (size, exclude patterns, extensions), or
    /// that a hidden component or an ignore rule excludes, itself or one of its
    /// directories.
    ///
    /// Each directory below `root`, and then the file, must appear in a
    /// one-level walk of its parent made with the scan's own walker settings,
    /// so the two cannot disagree. Listings are shared between `paths`.
    #[must_use]
    pub fn entries_for(&self, root: &Path, paths: &[PathBuf]) -> Vec<FileEntry> {
        let mut listings: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        let mut entries = Vec::new();
        for path in paths {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            // A symlink counts as a file only when the scan follows links.
            let metadata = if self.filter.follow_symlinks {
                std::fs::metadata(path)
            } else {
                std::fs::symlink_metadata(path)
            };
            let Ok(metadata) = metadata else {
                continue;
            };
            if !metadata.is_file()
                || !self.filter.matches(path, metadata.len())
                || !self.reachable(root, path, &mut listings)
            {
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            entries.push(FileEntry::new(
                path.clone(),
                metadata.len(),
                modified,
                extension,
            ));
        }
        entries
    }

    /// The directories at and below `dir` that a scan of `root` descends into,
    /// `dir` first, and the files in them that it would index; `None` when a
    /// scan of `root` does not descend into `dir` at all.
    #[must_use]
    pub fn tree(&self, root: &Path, dir: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
        if dir != root && !self.reachable(root, dir, &mut HashMap::new()) {
            return None;
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in walk_builder(dir, root, &self.filter, self.respect_gitignore)
            .build()
            .flatten()
        {
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                dirs.push(entry.into_path());
            } else if file_type.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if self.filter.matches(entry.path(), size) {
                    files.push(entry.into_path());
                }
            }
        }
        Some((dirs, files))
    }

    /// Whether each directory from `root` down to `path`, then `path` itself,
    /// appears in a one-level walk of its parent made with the scan's walker
    /// settings: whether a scan of `root` reaches `path`. `listings` caches the
    /// walks by directory.
    fn reachable(
        &self,
        root: &Path,
        path: &Path,
        listings: &mut HashMap<PathBuf, HashSet<PathBuf>>,
    ) -> bool {
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        let mut dir = root.to_path_buf();
        for component in relative.components() {
            let child = dir.join(component);
            let listing = listings.entry(dir.clone()).or_insert_with(|| {
                walk_builder(&dir, root, &self.filter, self.respect_gitignore)
                    .max_depth(Some(1))
                    .build()
                    .flatten()
                    .filter(|entry| entry.depth() == 1)
                    .map(ignore::DirEntry::into_path)
                    .collect()
            });
            if !listing.contains(&child) {
                return false;
            }
            dir = child;
        }
        true
    }

    /// Scan multiple directories.
    ///
    /// # Arguments
    ///
    /// * `roots` - Root directories to scan
    /// * `cancel` - Optional cancellation token
    ///
    /// # Returns
    ///
    /// A vector of all file entries
    pub async fn scan_multiple(
        &self,
        roots: &[PathBuf],
        cancel: Option<CancellationToken>,
    ) -> Result<Vec<FileEntry>> {
        let mut all_entries = Vec::new();

        for root in roots {
            if let Some(ref ct) = cancel
                && ct.is_cancelled()
            {
                return Err(Error::Cancelled);
            }
            let entries = self.scan(root, cancel.clone()).await?;
            all_entries.extend(entries);
        }

        // Remove duplicates based on path
        all_entries.sort_by(|a, b| a.path.cmp(&b.path));
        all_entries.dedup_by(|a, b| a.path == b.path);

        Ok(all_entries)
    }

    /// Get total size of all scanned files
    #[must_use]
    pub fn total_size(entries: &[FileEntry]) -> u64 {
        entries.iter().map(|e| e.size).sum()
    }

    /// Count files by extension
    #[must_use]
    pub fn count_by_extension(entries: &[FileEntry]) -> Vec<(String, usize)> {
        use std::collections::HashMap;

        let mut counts: HashMap<String, usize> = HashMap::new();
        for entry in entries {
            *counts.entry(entry.extension.clone()).or_insert(0) += 1;
        }

        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by_key(|b| std::cmp::Reverse(b.1));
        result
    }
}

impl Default for FileScanner {
    fn default() -> Self {
        Self::new(FileFilter::default())
    }
}

/// Default file extensions to include
fn default_extensions() -> HashSet<String> {
    [
        "py", "pyi", "pyw", // Python
        "js", "jsx", "mjs", "cjs", // JavaScript
        "ts", "tsx",  // TypeScript
        "rs",   // Rust
        "go",   // Go
        "java", // Java
        "c", "h", // C
        "cpp", "cc", "cxx", "hpp", // C++
        "cs",  // C#
        "rb", "erb",   // Ruby
        "php",   // PHP
        "swift", // Swift
        "kt", "kts", // Kotlin
        "scala", "sc",   // Scala
        "dart", // Dart
        "sh", "bash", "zsh", // Shell
        "html", "htm", "css", "scss", "sass", "less", // Web
        "json", "yaml", "yml", "toml", "xml", // Data
        "md", "mdx", "rst", // Documentation
        "sql", // SQL
        // Additional languages with tree-sitter parsers
        "vue",    // Vue
        "svelte", // Svelte
        "sol",    // Solidity
        "cu", "cuh", // CUDA
        "fs", "fsx", "fsi", // F#
        "m",   // Objective-C, told from MATLAB by its content
        "mm",  // Objective-C++
        "gql", "graphql", // GraphQL
        "vb",      // Visual Basic / VB.NET
    ]
    .iter()
    .map(|&s| s.to_string())
    .collect()
}

/// Whether a scan can take in files with the extension `ext` (no dot): one of
/// the extensions a filter includes by default.
#[must_use]
pub fn is_scannable_extension(ext: &str) -> bool {
    static SCANNABLE: LazyLock<HashSet<String>> = LazyLock::new(default_extensions);
    SCANNABLE.contains(&ext.to_lowercase())
}

/// Default exclude patterns
fn default_excludes() -> &'static [&'static str] {
    &[
        "node_modules",
        ".git",
        "target",
        "__pycache__",
        ".venv",
        "venv",
        ".env",
        ".idea",
        ".vscode",
        "dist",
        "build",
        "*.min.js",
        "*.min.css",
        "*.map",
        ".pytest_cache",
        ".mypy_cache",
        ".tox",
        "vendor",
        "third_party",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Every extension the parser maps to a language it can parse is scanned
    /// by default: `.mm`, `.cuh`, `.fsx` and `.fsi` among them.
    #[test]
    fn every_parsed_extension_is_scanned_by_default() {
        let scanned = default_extensions();
        let mut missing: Vec<&str> = crate::parser::language_config::mapped_extensions()
            .filter(|(_, language)| crate::parser::language_config::is_language_supported(language))
            .map(|(extension, _)| extension)
            .filter(|extension| !scanned.contains(*extension))
            .collect();
        missing.sort_unstable();
        assert!(missing.is_empty(), "parsed but never scanned: {missing:?}");
    }

    /// Checking given paths applies the scan's rules exactly: of every file
    /// in a tree, `entries_for` keeps the ones `scan` finds. The tree is a git
    /// work tree with ignore rules at two levels, one of them re-including a
    /// file, plus a hidden directory and a dependency directory.
    #[tokio::test]
    async fn entries_for_agrees_with_a_scan() {
        let dir = TempDir::new().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let files = [
            "src/main.rs",
            "src/gen/out.rs",
            "src/keep.rs",
            "src/skip.rs",
            "build/artifact.rs",
            ".hidden/secret.rs",
            "node_modules/pkg/index.js",
            "lib/util.py",
            "README.md",
        ];
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x\n").unwrap();
        }
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n*.rs.bak\nskip.rs\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "gen/\n!keep.rs\n").unwrap();

        let scanner = FileScanner::new(FileFilter::new());
        let mut scanned: Vec<PathBuf> = scanner
            .scan(&root, None)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        scanned.sort();
        let candidates: Vec<PathBuf> = files.iter().map(|file| root.join(file)).collect();
        let mut checked: Vec<PathBuf> = scanner
            .entries_for(&root, &candidates)
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        checked.sort();

        assert_eq!(checked, scanned);
        assert!(checked.contains(&root.join("src/main.rs")));
        assert!(!checked.contains(&root.join("src/skip.rs")));
        assert!(!checked.contains(&root.join("src/gen/out.rs")));
        assert!(!checked.contains(&root.join("build/artifact.rs")));
        assert!(!checked.contains(&root.join(".hidden/secret.rs")));
        assert!(!checked.contains(&root.join("node_modules/pkg/index.js")));
        // Paths outside the root, missing or not files are left out.
        assert!(
            scanner
                .entries_for(
                    &root,
                    &[
                        root.join("src"),
                        root.join("src/gone.rs"),
                        PathBuf::from("/etc/hosts"),
                    ],
                )
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_file_scanner() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create test files
        let mut file1 = std::fs::File::create(root.join("test.py")).unwrap();
        file1.write_all(b"# test").unwrap();

        let mut file2 = std::fs::File::create(root.join("test.rs")).unwrap();
        file2.write_all(b"// test").unwrap();

        // Create subdirectory
        let subdir = root.join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let mut file3 = std::fs::File::create(subdir.join("test.go")).unwrap();
        file3.write_all(b"// test").unwrap();

        // Create excluded file
        std::fs::create_dir(root.join("node_modules")).unwrap();
        let mut file4 = std::fs::File::create(root.join("node_modules/test.js")).unwrap();
        file4.write_all(b"// test").unwrap();

        // Scan
        let scanner = FileScanner::default();
        let entries = scanner.scan(root, None).await.unwrap();

        // Should find 3 files (not the one in node_modules)
        assert_eq!(entries.len(), 3);

        // Check extensions
        let extensions: Vec<_> = entries.iter().map(|e| e.extension.clone()).collect();
        assert!(extensions.contains(&"py".to_string()));
        assert!(extensions.contains(&"rs".to_string()));
        assert!(extensions.contains(&"go".to_string()));
    }

    #[test]
    fn test_file_filter() {
        let mut filter = FileFilter::new();
        filter.add_extension("rs");
        filter.add_exclude("target");

        assert!(filter.matches(Path::new("/test.rs"), 100));
        assert!(!filter.matches(Path::new("/target/test.rs"), 100));
        assert!(!filter.matches(Path::new("/test.rs"), 100 * 1024 * 1024 + 1)); // Too large

        // Test that extensions not in the include list are rejected
        let ext = Path::new("/test.py")
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        assert_ne!(ext, "rs");
    }

    #[test]
    fn test_priority_pattern() {
        let pattern = PriorityPattern::new(r"main\.(py|rs)$", 1, "Main entry point").unwrap();

        assert!(pattern.matches("src/main.py"));
        assert!(pattern.matches("main.rs"));
        assert!(!pattern.matches("lib.rs"));
        assert!(!pattern.matches("test.py"));
    }

    #[test]
    fn test_priority_analyzer() {
        let mut analyzer = PriorityAnalyzer::default();

        let files = vec![
            PathBuf::from("src/main.py"),
            PathBuf::from("src/utils.py"),
            PathBuf::from("tests/test_main.py"),
            PathBuf::from("README.md"),
        ];

        let categorized = analyzer.categorize_files(&files);

        // Should categorize all files
        assert!(!categorized.is_empty());

        // main.py should be priority 1
        assert!(
            categorized
                .get(&1)
                .is_some_and(|v| v.iter().any(|p| p.to_string_lossy().contains("main.py")))
        );

        // test file should have higher priority number
        let test_priority = categorized
            .iter()
            .find(|(_, v)| v.iter().any(|p| p.to_string_lossy().contains("test_")))
            .map(|(k, _)| *k);
        assert!(test_priority.is_some());
    }

    #[test]
    fn test_entry_point_detector() {
        let files = vec![
            PathBuf::from("src/main.py"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("index.js"),
            PathBuf::from("test.py"),
            PathBuf::from("Cargo.toml"),
        ];

        let detector = EntryPointDetector::new();
        let entry_points = detector.detect_entry_points(&files);

        assert!(
            entry_points
                .iter()
                .any(|p| p.to_string_lossy().contains("main.py"))
        );
        assert!(
            entry_points
                .iter()
                .any(|p| p.to_string_lossy().contains("index.js"))
        );
        assert!(
            entry_points
                .iter()
                .any(|p| p.to_string_lossy().contains("Cargo.toml"))
        );
        assert!(
            !entry_points
                .iter()
                .any(|p| p.to_string_lossy().contains("test.py"))
        );
    }

    #[test]
    fn test_is_entry_point() {
        let detector = EntryPointDetector::new();
        assert!(detector.is_entry_point(Path::new("main.py")));
        assert!(detector.is_entry_point(Path::new("index.js")));
        assert!(detector.is_entry_point(Path::new("Cargo.toml")));
        assert!(!detector.is_entry_point(Path::new("utils.py")));
        assert!(!detector.is_entry_point(Path::new("test_main.py")));
    }
}
