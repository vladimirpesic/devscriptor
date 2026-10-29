//! Devscriptor LSP Module
//!
//! Provides LSP (Language Server Protocol) integration for code intelligence
//! capabilities across 36 programming languages.
//!
//! # Example
//!
//! ```rust,no_run
//! use devscriptor_lsp::{Config, initialize};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Load configuration
//!     let config = Config::load()?;
//!
//!     let integration = initialize(config).await?;
//!     // Use integration.translator() to get the Translator instance
//!     Ok(())
//! }
//! ```

#[cfg(feature = "lsp")]
pub mod bridge;
#[cfg(feature = "lsp")]
pub mod config;
#[cfg(feature = "lsp")]
pub mod error;
#[cfg(feature = "lsp")]
pub mod generated_registry;
#[cfg(feature = "lsp")]
pub mod languages;
#[cfg(feature = "lsp")]
pub mod lsp;
#[cfg(feature = "lsp")]
pub mod lsp_architecture;
#[cfg(feature = "lsp")]
pub mod manager;
#[cfg(feature = "lsp")]
pub mod mcp;
#[cfg(feature = "lsp")]
pub mod user_dirs;

#[cfg(feature = "lsp")]
pub use bridge::Translator;
#[cfg(feature = "lsp")]
pub use config::{Config, LspServerConfig, ServerHeuristics, WorkspaceConfig};
#[cfg(feature = "lsp")]
pub use error::{Error, LspError, Result, ServerSpawnFailure};
#[cfg(feature = "lsp")]
pub use manager::{InstallationResult, InstalledServer, LspManager};

#[cfg(feature = "lsp")]
use std::sync::Arc;

/// LSP module integration handle.
#[cfg(feature = "lsp")]
pub struct LspIntegration {
    /// The translator that bridges MCP and LSP protocols.
    ///
    /// Stored as `Arc<Translator>` (no outer `RwLock`): all `Translator::handle_*`
    /// methods, and `spawn_applicable`, take `&self`.
    translator: Arc<Translator>,
    /// The configuration.
    config: Config,
}

#[cfg(feature = "lsp")]
impl LspIntegration {
    /// Initialize from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No LSP servers can be initialized
    /// - Configuration is invalid
    pub async fn from_config(config: &Config) -> Result<Self> {
        let translator = Translator::new(config.clone());
        translator.spawn_applicable(None).await?;

        Ok(Self {
            translator: Arc::new(translator),
            config: config.clone(),
        })
    }

    /// Get the translator for MCP server.
    #[must_use]
    pub fn translator(&self) -> Arc<Translator> {
        self.translator.clone()
    }

    /// Check if a file is supported.
    ///
    /// Returns `true` if there's an LSP server configured for the file's extension.
    #[must_use]
    pub fn is_file_supported(&self, path: &std::path::Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| self.config.get_server_for_extension(ext).is_some())
    }

    /// Get the configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

/// Initialize the LSP module.
///
/// # Errors
///
/// Returns an error if initialization fails.
#[cfg(feature = "lsp")]
pub async fn initialize(config: Config) -> Result<LspIntegration> {
    LspIntegration::from_config(&config).await
}

/// Get module version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Get module name.
#[must_use]
pub const fn name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!version().is_empty());
    }

    #[test]
    fn test_name() {
        assert_eq!(name(), "devscriptor-lsp");
    }
}
