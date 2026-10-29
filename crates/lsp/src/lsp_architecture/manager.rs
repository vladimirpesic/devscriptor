//! LSP Architecture Manager
//!
//! High-level orchestration for the LSP detection workflow:
//! 1. Project detection
//! 2. Server resolution
//! 3. Prerequisite checking
//! 4. User prompting
//! 5. Auto-installation

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::trace;

use crate::config::LspServerConfig;

use super::prerequisites::{PrerequisitesResult, check_prerequisites, get_prerequisite_tiers};
use super::resolver::{LspResolver, LspSource, ResolutionResult, UserLspConfig};

/// LSP Architecture Manager - Main entry point for LSP discovery
pub struct LspArchitectureManager {
    /// LSP resolver for 4-tier resolution
    resolver: LspResolver,
    /// Cache of resolved servers (a `RefCell`, for a `&self` API)
    resolved_cache: RefCell<HashMap<String, ResolutionResult>>,
}

/// Information about a detected language in a project
#[derive(Debug, Clone)]
pub struct DetectedLanguage {
    /// Language ID (rust, typescript, etc.)
    pub language_id: String,
    /// Confidence level (0.0 - 1.0)
    pub confidence: f32,
    /// Files found that indicate this language
    pub indicator_files: Vec<PathBuf>,
    /// Suggested LSP server IDs
    pub suggested_servers: Vec<String>,
}

/// Type alias for language marker tuples
pub type LanguageMarkers = Vec<(
    String,
    Vec<&'static str>,
    Vec<&'static str>,
    Vec<&'static str>,
)>;

/// Action to take for an LSP server
#[derive(Debug, Clone)]
pub enum LspAction {
    /// Use the existing server
    UseExisting { path: PathBuf, source: LspSource },
    /// Download and install
    Install,
    /// Skip this server
    Skip,
    /// Use custom path
    CustomPath(PathBuf),
}

/// Result of LSP architecture resolution for a project
#[derive(Debug, Clone)]
pub struct LspArchitectureResult {
    /// Detected languages
    pub languages: Vec<DetectedLanguage>,
    /// Resolved servers with their sources
    pub resolved_servers: Vec<(String, LspSource, LspServerConfig)>,
    /// Servers that need installation
    pub needs_install: Vec<(String, PrerequisitesResult)>,
    /// Servers with missing prerequisites
    pub missing_prerequisites: Vec<(String, PrerequisitesResult)>,
}

impl LspArchitectureManager {
    /// Create a new LSP architecture manager
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            resolver: LspResolver::new()?,
            resolved_cache: RefCell::new(HashMap::new()),
        })
    }

    /// Create with custom bundled directory
    #[must_use]
    pub fn with_bundled_dir(bundled_dir: PathBuf) -> Self {
        Self {
            resolver: LspResolver::with_bundled_dir(bundled_dir),
            resolved_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Set user configuration
    #[must_use]
    pub fn with_user_config(mut self, config: UserLspConfig) -> Self {
        self.resolver = self.resolver.with_user_config(config);
        self
    }

    /// Analyze a project and resolve all LSP servers
    pub fn analyze_project(&self, project_path: &Path) -> LspArchitectureResult {
        trace!("Analyzing project: {}", project_path.display());

        // Detect languages
        let languages = self.detect_languages(project_path);

        let mut resolved_servers = Vec::new();
        let mut needs_install = Vec::new();
        let mut missing_prerequisites = Vec::new();

        // Resolve servers for each detected language
        for language in &languages {
            for server_id in &language.suggested_servers {
                if let Some(result) = self.resolve_server(server_id) {
                    resolved_servers.push((server_id.clone(), result.source, result.config));
                } else {
                    // Check prerequisites
                    let prereq_result = check_prerequisites(server_id);

                    if prereq_result.is_satisfied() {
                        needs_install.push((server_id.clone(), prereq_result));
                    } else {
                        missing_prerequisites.push((server_id.clone(), prereq_result));
                    }
                }
            }
        }

        LspArchitectureResult {
            languages,
            resolved_servers,
            needs_install,
            missing_prerequisites,
        }
    }

    /// Detect languages in a project.
    ///
    /// Walks the filesystem **once** and builds an extension→count map,
    /// then looks up each language's extensions against that map
    /// rather than walking the tree once per language.
    fn detect_languages(&self, project_path: &Path) -> Vec<DetectedLanguage> {
        let mut detected = Vec::new();
        let mut found_languages = HashSet::new();

        // Single filesystem walk: collect all file extensions and their counts.
        let mut ext_counts: HashMap<String, usize> = HashMap::new();
        for entry in walkdir::WalkDir::new(project_path)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                *ext_counts.entry(ext.to_lowercase()).or_insert(0) += 1;
            }
        }

        // Check for marker files
        let markers = self.get_language_markers();

        for (language_id, marker_patterns, extensions, servers) in markers {
            let mut indicator_files = Vec::new();
            let mut confidence = 0.0f32;

            // Check marker files
            for pattern in marker_patterns {
                if let Some(path) = self.find_marker_file(project_path, pattern) {
                    indicator_files.push(path);
                    confidence += 0.5;
                }
            }

            // Check for source files using the pre-built extension map
            let source_count: usize = extensions
                .iter()
                .map(|e| ext_counts.get(&e.to_lowercase()).copied().unwrap_or(0))
                .sum();
            if source_count > 0 {
                confidence += 0.3;
            }

            if confidence > 0.0 && !found_languages.contains(&language_id) {
                found_languages.insert(language_id.clone());
                detected.push(DetectedLanguage {
                    language_id,
                    confidence: confidence.min(1.0),
                    indicator_files,
                    suggested_servers: servers.iter().map(|&s| s.to_string()).collect(),
                });
            }
        }

        // Sort by confidence (handle NaN safely)
        detected.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        detected
    }

    /// Get language markers for detection
    fn get_language_markers(&self) -> LanguageMarkers {
        vec![
            (
                "rust".to_string(),
                vec!["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"],
                vec!["rs"],
                vec!["rust-analyzer"],
            ),
            (
                "typescript".to_string(),
                vec!["package.json", "tsconfig.json", "tsconfig.*.json"],
                vec!["ts", "tsx", "js", "jsx", "mjs", "cjs"],
                vec!["typescript-language-server"],
            ),
            (
                "python".to_string(),
                vec![
                    "pyproject.toml",
                    "requirements.txt",
                    "setup.py",
                    "Pipfile",
                    "poetry.lock",
                ],
                vec!["py", "pyw", "pyi"],
                vec!["pyright"],
            ),
            (
                "go".to_string(),
                vec!["go.mod", "go.sum"],
                vec!["go"],
                vec!["gopls"],
            ),
            (
                "java".to_string(),
                vec!["pom.xml", "build.gradle", ".classpath", "gradlew"],
                vec!["java"],
                vec!["jdtls"],
            ),
            (
                "kotlin".to_string(),
                vec!["build.gradle.kts", "*.kt"],
                vec!["kt", "kts"],
                vec!["kotlin-lsp"],
            ),
            (
                "scala".to_string(),
                vec!["build.sbt", "*.scala"],
                vec!["scala", "sc"],
                vec!["metals"],
            ),
            (
                "csharp".to_string(),
                vec!["*.csproj", "*.sln", ".csproj"],
                vec!["cs"],
                vec!["csharp-ls"],
            ),
            (
                "fsharp".to_string(),
                vec!["*.fsproj", "*.sln"],
                vec!["fs", "fsx"],
                vec!["fsautocomplete"],
            ),
            // The canonical server id, as the system and VS Code registries name it.
            (
                "dart".to_string(),
                vec!["pubspec.yaml", "pubspec.lock"],
                vec!["dart"],
                vec!["dart-ls"],
            ),
            (
                "swift".to_string(),
                vec!["Package.swift", "*.xcodeproj"],
                vec!["swift"],
                vec!["sourcekit-lsp"],
            ),
            (
                "cpp".to_string(),
                vec![
                    "CMakeLists.txt",
                    "Makefile",
                    "compile_commands.json",
                    "compile_flags.txt",
                    ".clangd",
                ],
                vec!["c", "cpp", "cc", "cxx", "h", "hpp", "cu", "cuh", "mm", "m"],
                vec!["clangd"],
            ),
            (
                "ruby".to_string(),
                vec!["Gemfile", "*.gemspec", "Rakefile"],
                vec!["rb", "erb"],
                vec!["ruby-lsp"],
            ),
            (
                "php".to_string(),
                vec!["composer.json", "composer.lock"],
                vec!["php"],
                vec!["phpactor"],
            ),
            (
                "vue".to_string(),
                vec!["vite.config.ts", "nuxt.config.ts", "vue.config.js"],
                vec!["vue"],
                vec!["volar"],
            ),
            (
                "svelte".to_string(),
                vec!["svelte.config.js"],
                vec!["svelte"],
                vec!["svelte-language-server"],
            ),
            (
                "graphql".to_string(),
                vec!["*.graphql", "*.gql", ".graphqlrc"],
                vec!["graphql", "gql"],
                vec!["graphql-language-service"],
            ),
            // The canonical server id.
            (
                "solidity".to_string(),
                vec!["hardhat.config.js", "foundry.toml", "remappings.txt"],
                vec!["sol"],
                vec!["nomicfoundation-solc"],
            ),
        ]
    }

    /// Find a marker file in project
    fn find_marker_file(&self, project_path: &Path, pattern: &str) -> Option<PathBuf> {
        // Direct file
        let direct = project_path.join(pattern);
        if direct.exists() {
            return Some(direct);
        }

        // Glob pattern
        if pattern.contains('*')
            && let Ok(entries) = glob::glob(&format!("{}/{}", project_path.display(), pattern))
        {
            for entry in entries.flatten() {
                if entry.exists() {
                    return Some(entry);
                }
            }
        }

        None
    }

    /// Find source files with given extensions
    /// Resolve a single server (with caching)
    fn resolve_server(&self, server_id: &str) -> Option<ResolutionResult> {
        // Check cache first
        if let Some(result) = self.resolved_cache.borrow().get(server_id) {
            trace!("Using cached resolution for {}", server_id);
            return Some(result.clone());
        }

        // Resolve using the 4-tier resolver
        let result = self.resolver.resolve(server_id)?;

        // Cache the result
        self.resolved_cache
            .borrow_mut()
            .insert(server_id.to_string(), result.clone());

        Some(result)
    }

    /// Get prerequisite information for a server
    #[must_use]
    pub fn get_prerequisites(&self, server_id: &str) -> PrerequisitesResult {
        check_prerequisites(server_id)
    }

    /// Check if a server can be auto-installed
    #[must_use]
    pub fn can_auto_install(&self, server_id: &str) -> bool {
        let prereq_result = check_prerequisites(server_id);

        if !prereq_result.is_satisfied() {
            return false;
        }

        // Check if it's a downloadable server
        matches!(
            server_id,
            "rust-analyzer"
                | "clangd"
                | "gopls"
                | "typescript-language-server"
                | "volar"
                | "svelte-language-server"
                | "html-languageserver"
                | "graphql-language-service"
                | "pyright"
                | "sql-language-server"
                | "jdtls"
                | "kotlin-lsp"
                | "metals"
                | "csharp-ls"
                | "fsautocomplete"
                | "sourcekit-lsp"
                | "dart-ls"
                | "ruby-lsp"
                | "phpactor"
                | "bash-language-server"
                | "nomicfoundation-solc"
        )
    }

    /// Get human-readable status for a server.
    ///
    /// This is now `&self` thanks to interior mutability in `resolve_server`.
    pub fn get_server_status(&self, server_id: &str) -> ServerStatus {
        if let Some(result) = self.resolve_server(server_id) {
            ServerStatus::Available {
                source: result.source,
                config: result.config,
            }
        } else {
            let prereqs = check_prerequisites(server_id);

            if prereqs.is_satisfied() {
                ServerStatus::NeedsInstall
            } else {
                let tiers = get_prerequisite_tiers(server_id);
                ServerStatus::MissingPrerequisites {
                    prerequisites: prereqs,
                    tiers,
                }
            }
        }
    }

    /// Get all available servers
    #[must_use]
    pub fn get_all_available_servers(&self) -> Vec<(String, LspSource)> {
        self.resolver.scan_all_available()
    }

    /// Clear the resolution cache
    pub fn clear_cache(&mut self) {
        self.resolved_cache.borrow_mut().clear();
    }
}

/// Status of an LSP server
#[derive(Debug, Clone)]
pub enum ServerStatus {
    /// Server is available
    Available {
        source: LspSource,
        config: LspServerConfig,
    },
    /// Needs to be installed
    NeedsInstall,
    /// Missing prerequisites
    MissingPrerequisites {
        prerequisites: PrerequisitesResult,
        tiers: super::prerequisites::PrerequisiteTiers,
    },
}

impl ServerStatus {
    /// Check if the server is available
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, ServerStatus::Available { .. })
    }

    /// Get the source if available
    #[must_use]
    pub fn source(&self) -> Option<&LspSource> {
        match self {
            ServerStatus::Available { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsp_architecture_manager_new() {
        let manager = LspArchitectureManager::new();
        assert!(manager.is_ok());
    }

    #[test]
    fn test_detect_languages_empty_project() {
        let manager = LspArchitectureManager::with_bundled_dir(PathBuf::from("/tmp"));
        let temp_dir = std::env::temp_dir();
        let _languages = manager.detect_languages(&temp_dir);
        // Should not panic, may or may not detect languages depending on temp dir contents
    }

    #[test]
    fn test_server_status_is_available() {
        use crate::languages::presets;
        let status = ServerStatus::Available {
            source: LspSource::Bundled(PathBuf::from("/usr/bin/test")),
            config: presets::rust_analyzer(),
        };
        assert!(status.is_available());
    }
}
