//! What an install of an LSP server came to.
//!
//! [`LspManager::install`](super::LspManager::install) downloads, verifies and
//! unpacks a server, and records it with `.installed` and `.version`.

/// Result of an installation operation
#[derive(Debug, Clone)]
pub enum InstallationResult {
    Success,
    AlreadyInstalled,
    NotInstalled,
    MissingPrerequisite(String),
    Failed(String),
}
