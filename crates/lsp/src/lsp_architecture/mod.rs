//! LSP Architecture - LSP Detection System
//!
//! Provides an optimal 4-tier resolution strategy for discovering LSP servers:
//!
//! 1. **User Configuration** (lsp.toml overrides)
//! 2. **Devscriptor-managed installs** — on-demand installs in
//!    `~/.cache/devscriptor/lsp-servers/` (via `lsp install`), then servers
//!    bundled alongside the executable (`bundle/lsp-servers/`)
//! 3. **System-Wide Detection** (PATH, SDK paths)
//! 4. **VS Code Extensions** (~/.vscode/extensions/)
//!
//! ## Usage
//!
//! ```rust,no_run
//! use devscriptor_lsp::lsp_architecture::LspArchitectureManager;
//!
//! # fn main() -> anyhow::Result<()> {
//! let mut manager = LspArchitectureManager::new()?;
//! let result = manager.analyze_project(std::path::Path::new("."));
//!
//! for lang in &result.languages {
//!     println!("Detected: {} (confidence: {:.0}%)", lang.language_id, lang.confidence * 100.0);
//! }
//! # Ok(())
//! # }
//! ```

pub mod manager;
pub mod prerequisites;
pub mod resolver;
pub mod system;
pub mod vscode;

// Re-export main types
pub use manager::{DetectedLanguage, LspAction, LspArchitectureManager, LspArchitectureResult};
pub use prerequisites::{
    Prerequisite, PrerequisiteTiers, PrerequisitesResult, check_prerequisites,
    check_prerequisites_async, get_prerequisite_tiers,
};
pub use resolver::{LspResolver, LspSource, ResolutionResult, ServerOverride, UserLspConfig};
pub use system::{SystemLspInfo, detect_system_lsp};
pub use vscode::{VsCodeExtension, detect_lsp_server};

/// Quick check if an LSP server is available
///
/// This is a convenience function that checks all tiers without
/// requiring you to create a manager instance.
///
/// # Arguments
///
/// * `server_id` - The server identifier (e.g., "rust-analyzer", "typescript-language-server")
///
/// # Returns
///
/// `Some(LspSource)` if found, `None` otherwise
#[must_use]
pub fn quick_check(server_id: &str) -> Option<LspSource> {
    let resolver = LspResolver::new().ok()?;
    resolver.resolve(server_id).map(|r| r.source)
}

/// The registry's server id that `name` stands for: a server id as it is, or
/// a language one of the registry's servers serves (`dart` → `dart-ls`).
/// `None` for a name that is neither.
#[must_use]
pub fn server_id_for(name: &str) -> Option<&'static str> {
    let ids = crate::generated_registry::SERVER_IDS;
    ids.iter().copied().find(|id| *id == name).or_else(|| {
        ids.iter()
            .copied()
            .find(|id| crate::generated_registry::language_id(id) == Some(name))
    })
}

/// Batch check multiple servers
///
/// Efficiently checks multiple LSP servers at once, returning
/// a map of server IDs to their resolved sources.
#[must_use]
pub fn batch_check<S: AsRef<str>>(server_ids: &[S]) -> Vec<(String, Option<LspSource>)> {
    let resolver = match LspResolver::new() {
        Ok(r) => r,
        Err(_) => {
            return server_ids
                .iter()
                .map(|id| (id.as_ref().to_string(), None))
                .collect();
        }
    };

    server_ids
        .iter()
        .map(|id| {
            let id_ref = id.as_ref();
            let source = resolver.resolve(id_ref).map(|r| r.source);
            (id_ref.to_string(), source)
        })
        .collect()
}

/// Check if VS Code extensions can be detected
///
/// Returns true if VS Code is installed and extensions directory exists
#[must_use]
pub fn is_vscode_detection_available() -> bool {
    vscode::is_vscode_available()
}

/// Get the LSP detection statistics
///
/// Returns information about what was found in each tier
#[must_use]
pub fn get_detection_stats() -> DetectionStats {
    let vscode_count = vscode::VSCODE_EXTENSIONS
        .iter()
        .filter(|ext| vscode::detect_lsp_server(ext.server_id).is_some())
        .count();

    let system_count = system::SYSTEM_LSP_REGISTRY
        .iter()
        .filter(|info| system::detect_system_lsp(info.server_id).is_some())
        .count();

    DetectionStats {
        vscode_extensions_found: vscode_count,
        system_servers_found: system_count,
        total_vscode_extensions: vscode::VSCODE_EXTENSIONS.len(),
        total_system_servers: system::SYSTEM_LSP_REGISTRY.len(),
    }
}

/// Statistics about LSP detection
#[derive(Debug, Clone)]
pub struct DetectionStats {
    /// Number of VS Code extensions with LSP servers found
    pub vscode_extensions_found: usize,
    /// Number of system-wide LSP servers found
    pub system_servers_found: usize,
    /// Total VS Code extensions known
    pub total_vscode_extensions: usize,
    /// Total system servers known
    pub total_system_servers: usize,
}

impl DetectionStats {
    /// Check if any LSP servers were found
    #[must_use]
    pub fn has_any_servers(&self) -> bool {
        self.vscode_extensions_found > 0 || self.system_servers_found > 0
    }

    /// Get total servers available
    #[must_use]
    pub fn total_available(&self) -> usize {
        self.vscode_extensions_found + self.system_servers_found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server goes by its id or its language; anything else by nothing.
    #[test]
    fn a_server_goes_by_its_id_or_its_language() {
        assert_eq!(server_id_for("dart-ls"), Some("dart-ls"));
        assert_eq!(server_id_for("dart"), Some("dart-ls"));
        assert_eq!(server_id_for("rust"), Some("rust-analyzer"));
        assert_eq!(server_id_for("swift"), Some("sourcekit-lsp"));
        assert_eq!(server_id_for("cobol"), None);
        assert_eq!(server_id_for(""), None);
    }

    #[test]
    fn test_quick_check_rust_analyzer() {
        // This test will pass if rust-analyzer is found in any tier
        let result = quick_check("rust-analyzer");
        println!("rust-analyzer found: {:?}", result.is_some());
    }

    #[test]
    fn test_batch_check() {
        let servers = vec!["rust-analyzer", "typescript-language-server", "gopls"];
        let results = batch_check(&servers);

        assert_eq!(results.len(), 3);
        for (id, source) in results {
            println!("{}: {:?}", id, source.is_some());
        }
    }

    #[test]
    fn test_detection_stats() {
        let stats = get_detection_stats();
        println!(
            "VS Code: {}/{}",
            stats.vscode_extensions_found, stats.total_vscode_extensions
        );
        println!(
            "System: {}/{}",
            stats.system_servers_found, stats.total_system_servers
        );
    }
}
