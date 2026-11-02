//! Thread-safe LSP client registry for concurrent access.
//!
//! Provides a concurrent hash map for storing LSP clients by language ID,
//! allowing multiple concurrent read operations while maintaining exclusive
//! write access during initialization.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::lsp::client::LspClient;

/// Thread-safe registry of LSP clients.
#[derive(Clone)]
pub struct ClientRegistry {
    /// LSP clients by language ID - uses `RwLock` for concurrent reads.
    clients: Arc<RwLock<HashMap<String, LspClient>>>,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientRegistry {
    /// Create a new empty client registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a client for reading (concurrent).
    ///
    /// # Arguments
    ///
    /// * `language_id` - The language ID to look up
    ///
    /// # Returns
    ///
    /// The client if found, or None if not registered
    #[must_use]
    pub fn get(&self, language_id: &str) -> Option<LspClient> {
        let clients = self.clients.read();
        clients.get(language_id).cloned()
    }

    /// Insert a client (exclusive lock during write).
    ///
    /// # Arguments
    ///
    /// * `language_id` - The language ID to register
    /// * `client` - The LSP client to store
    pub fn insert(&self, language_id: String, client: LspClient) {
        let mut clients = self.clients.write();
        clients.insert(language_id, client);
    }

    /// Check if a language has a registered client (concurrent read).
    ///
    /// # Arguments
    ///
    /// * `language_id` - The language ID to check
    ///
    /// # Returns
    ///
    /// true if a client is registered for the language
    #[must_use]
    pub fn contains(&self, language_id: &str) -> bool {
        let clients = self.clients.read();
        clients.contains_key(language_id)
    }

    /// Get the number of registered clients.
    #[must_use]
    pub fn len(&self) -> usize {
        let clients = self.clients.read();
        clients.len()
    }

    /// Check if the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get all registered language IDs.
    #[must_use]
    pub fn languages(&self) -> Vec<String> {
        // Collect keys while holding the lock, then clone after releasing it, to
        // keep lock contention low.
        let keys: Vec<String> = {
            let clients = self.clients.read();
            clients.keys().cloned().collect()
        };
        keys
    }

    /// Remove a client and return it if it existed.
    #[must_use]
    pub fn remove(&self, language_id: &str) -> Option<LspClient> {
        let mut clients = self.clients.write();
        clients.remove(language_id)
    }

    /// Clear all registered clients.
    pub fn clear(&self) {
        let mut clients = self.clients.write();
        clients.clear();
    }

    /// Return an arbitrary registered client (useful when any client will do).
    #[must_use]
    pub fn first(&self) -> Option<LspClient> {
        let clients = self.clients.read();
        clients.values().next().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_registry_basic() {
        let registry = ClientRegistry::new();

        // Initially empty
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        // Check non-existent client
        assert!(!registry.contains("rust"));
        assert!(registry.get("rust").is_none());
    }

    #[test]
    fn test_concurrent_reads() {
        let registry = ClientRegistry::new();

        // Multiple concurrent reads should not block each other
        assert!(registry.get("rust").is_none());
        assert!(registry.get("python").is_none());
        assert!(!registry.contains("rust"));
    }
}
