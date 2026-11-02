//! Workspace configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Workspace configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace root directories.
    #[serde(default = "default_roots")]
    pub roots: Vec<PathBuf>,
    /// Maximum depth for heuristics search.
    #[serde(default = "default_heuristics_max_depth")]
    pub heuristics_max_depth: usize,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            roots: default_roots(),
            heuristics_max_depth: default_heuristics_max_depth(),
        }
    }
}

impl WorkspaceConfig {
    /// Build a mapping from file extensions to language IDs.
    #[must_use]
    pub fn build_extension_map(
        &self,
        servers: &[super::LspServerConfig],
    ) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for server in servers {
            for ext in &server.extensions {
                map.insert(ext.clone(), server.language_id.clone());
            }
        }
        map
    }
}

fn default_roots() -> Vec<PathBuf> {
    vec![PathBuf::from(".")]
}

fn default_heuristics_max_depth() -> usize {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_config_default() {
        let config = WorkspaceConfig::default();
        assert_eq!(config.roots.len(), 1);
        assert_eq!(config.heuristics_max_depth, 3);
    }
}
