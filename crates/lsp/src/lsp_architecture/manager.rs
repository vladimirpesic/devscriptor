//! LSP Architecture Manager
//!
//! High-level orchestration for the LSP detection workflow:
//! 1. Project detection
//! 2. Server resolution
//! 3. Prerequisite checking
//! 4. User prompting
//! 5. Auto-installation

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::trace;

use crate::config::LspServerConfig;
use crate::generated_registry;

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

/// The most files [`LspArchitectureManager::detect_languages`] looks at; a
/// larger tree is judged by the first ones its walk meets.
const DETECTION_FILE_LIMIT: usize = 100_000;

/// The first file among `file_names` (by name) that `marker` names, a name
/// or a glob (`*.csproj`).
fn marker_in(file_names: &HashMap<String, PathBuf>, marker: &str) -> Option<PathBuf> {
    if !marker.contains('*') {
        return file_names.get(marker).cloned();
    }
    let pattern = glob::Pattern::new(marker).ok()?;
    file_names
        .iter()
        .filter(|(name, _)| pattern.matches(name))
        .map(|(_, path)| path.clone())
        .min()
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

    /// Detect languages in a project: the language of each registry server
    /// with a source file in it, at any depth. One walk of the tree, as git
    /// sees it, counts every extension and finds the servers' project markers
    /// wherever they are: a monorepo keeps its services' sources and manifests
    /// some levels down (`services/rating-rust/src/discount.rs`). Ignore files
    /// are honoured, also outside a repository, and hidden directories (`.git`,
    /// `.venv`, the servers' own `.metals/`) are left out, but not the hidden
    /// files that mark a project (`.clangd`). At most [`DETECTION_FILE_LIMIT`]
    /// files are looked at.
    ///
    /// A language with sources is detected with confidence 0.3, and with 0.8
    /// where a marker of its server's is in the tree too; a marker alone
    /// detects nothing, as `pom.xml` is both Java's and Kotlin's. The most
    /// confident come first, and of those the ones with the most sources.
    fn detect_languages(&self, project_path: &Path) -> Vec<DetectedLanguage> {
        let mut ext_counts: HashMap<String, usize> = HashMap::new();
        let mut file_names: HashMap<String, PathBuf> = HashMap::new();
        let walker = ignore::WalkBuilder::new(project_path)
            .hidden(false)
            .require_git(false)
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !(entry.file_type().is_some_and(|t| t.is_dir())
                        && entry.file_name().to_string_lossy().starts_with('.'))
            })
            .build();
        for entry in walker
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
            .take(DETECTION_FILE_LIMIT)
        {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                *ext_counts.entry(ext.to_lowercase()).or_insert(0) += 1;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                file_names
                    .entry(name.to_string())
                    .or_insert_with(|| path.to_path_buf());
            }
        }

        let mut detected: Vec<(DetectedLanguage, usize)> = Vec::new();
        for server_id in generated_registry::SERVER_IDS {
            let (Some(language_id), Some(extensions)) = (
                generated_registry::language_id(server_id),
                generated_registry::extensions(server_id),
            ) else {
                continue;
            };
            let sources: usize = extensions
                .iter()
                .map(|e| ext_counts.get(&e.to_lowercase()).copied().unwrap_or(0))
                .sum();
            if sources == 0 {
                continue;
            }
            if let Some((language, _)) = detected
                .iter_mut()
                .find(|(language, _)| language.language_id == language_id)
            {
                language.suggested_servers.push((*server_id).to_string());
                continue;
            }
            let indicator_files: Vec<PathBuf> = generated_registry::heuristics(server_id)
                .map(|h| h.markers)
                .unwrap_or_default()
                .iter()
                .filter_map(|marker| marker_in(&file_names, marker))
                .collect();
            let confidence = if indicator_files.is_empty() { 0.3 } else { 0.8 };
            detected.push((
                DetectedLanguage {
                    language_id: language_id.to_string(),
                    confidence,
                    indicator_files,
                    suggested_servers: vec![(*server_id).to_string()],
                },
                sources,
            ));
        }
        detected.sort_by(|(a, a_sources), (b, b_sources)| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b_sources.cmp(a_sources))
        });
        detected.into_iter().map(|(language, _)| language).collect()
    }

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
