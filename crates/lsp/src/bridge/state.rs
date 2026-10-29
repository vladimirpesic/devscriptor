//! Document state tracking for the bridge layer.
//!
//! This module manages open documents and their versions for synchronization
//! with LSP servers.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Document information.
///
/// `content` is stored as [`Arc<str>`] rather than [`String`], so that cloning a
/// `Document` (or returning it from `get`) only bumps a reference count instead
/// of heap-allocating a full copy of potentially large file contents.
#[derive(Debug, Clone)]
pub struct Document {
    /// File path.
    pub path: PathBuf,
    /// Document URI.
    pub uri: lsp_types::Uri,
    /// Current version.
    pub version: i32,
    /// Document content — shared reference to avoid expensive clones.
    pub content: Arc<str>,
    /// Language ID.
    pub language_id: String,
}

/// Thread-safe document state manager.
///
/// All operations are synchronized via an internal [`RwLock`], allowing
/// concurrent reads (e.g. `get`, `is_open`) and exclusive writes
/// (e.g. `open`, `close`, `update`).
#[derive(Debug, Clone, Default)]
pub struct DocumentState {
    /// Open documents by path.
    documents: Arc<RwLock<HashMap<PathBuf, Document>>>,
}

impl DocumentState {
    /// Create a new document state manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Open a document.
    ///
    /// # Arguments
    ///
    /// * `path` - File path
    /// * `uri` - Document URI
    /// * `content` - Document content
    /// * `language_id` - Language ID
    pub fn open(
        &self,
        path: PathBuf,
        uri: lsp_types::Uri,
        content: impl Into<Arc<str>>,
        language_id: String,
    ) {
        let document = Document {
            path: path.clone(),
            uri,
            version: 1,
            content: content.into(),
            language_id,
        };
        self.documents.write().insert(path, document);
    }

    /// Close a document.
    ///
    /// # Arguments
    ///
    /// * `path` - File path
    pub fn close(&self, path: &PathBuf) {
        self.documents.write().remove(path);
    }

    /// Get a document by path.
    #[must_use]
    pub fn get(&self, path: &PathBuf) -> Option<Document> {
        self.documents.read().get(path).cloned()
    }

    /// Check if a document is open.
    #[must_use]
    pub fn is_open(&self, path: &PathBuf) -> bool {
        self.documents.read().contains_key(path)
    }

    /// Update document content.
    ///
    /// # Arguments
    ///
    /// * `path` - File path
    /// * `content` - New content
    ///
    /// # Returns
    ///
    /// The new version number, or `None` if the document is not open.
    pub fn update(&self, path: &PathBuf, content: impl Into<Arc<str>>) -> Option<i32> {
        let mut docs = self.documents.write();
        if let Some(doc) = docs.get_mut(path) {
            doc.version += 1;
            doc.content = content.into();
            Some(doc.version)
        } else {
            None
        }
    }

    /// Get the version of a document.
    #[must_use]
    pub fn version(&self, path: &PathBuf) -> Option<i32> {
        self.documents.read().get(path).map(|d| d.version)
    }

    /// Get the URI of a document.
    #[must_use]
    pub fn uri(&self, path: &PathBuf) -> Option<lsp_types::Uri> {
        self.documents.read().get(path).map(|d| d.uri.clone())
    }

    /// Get the content of a document.
    #[must_use]
    pub fn content(&self, path: &PathBuf) -> Option<Arc<str>> {
        self.documents
            .read()
            .get(path)
            .map(|d| Arc::clone(&d.content))
    }

    /// Get the language ID of a document.
    #[must_use]
    pub fn language_id(&self, path: &PathBuf) -> Option<String> {
        self.documents
            .read()
            .get(path)
            .map(|d| d.language_id.clone())
    }

    /// Get all open documents.
    #[must_use]
    pub fn all_documents(&self) -> Vec<Document> {
        self.documents.read().values().cloned().collect()
    }

    /// Clear all documents.
    pub fn clear(&self) {
        self.documents.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_uri() -> lsp_types::Uri {
        "file:///test.rs".parse().unwrap()
    }

    #[test]
    fn test_document_state_open() {
        let state = DocumentState::new();
        let path = PathBuf::from("/test.rs");
        let uri = create_test_uri();
        state.open(
            path.clone(),
            uri.clone(),
            "content".to_string(),
            "rust".to_string(),
        );

        assert!(state.is_open(&path));
        assert_eq!(state.version(&path), Some(1));
        assert_eq!(state.uri(&path), Some(uri));
        assert_eq!(state.content(&path), Some(Arc::from("content")));
        assert_eq!(state.language_id(&path), Some("rust".to_string()));
    }

    #[test]
    fn test_document_state_close() {
        let state = DocumentState::new();
        let path = PathBuf::from("/test.rs");
        state.open(
            path.clone(),
            create_test_uri(),
            "content".to_string(),
            "rust".to_string(),
        );
        state.close(&path);

        assert!(!state.is_open(&path));
        assert_eq!(state.version(&path), None);
    }

    #[test]
    fn test_document_state_update() {
        let state = DocumentState::new();
        let path = PathBuf::from("/test.rs");
        state.open(
            path.clone(),
            create_test_uri(),
            "old".to_string(),
            "rust".to_string(),
        );
        let new_version = state.update(&path, "new".to_string());

        assert_eq!(new_version, Some(2));
        assert_eq!(state.content(&path), Some(Arc::from("new")));
    }

    #[test]
    fn test_document_state_update_not_found() {
        let state = DocumentState::new();
        let path = PathBuf::from("/nonexistent.rs");
        let result = state.update(&path, "new".to_string());

        assert_eq!(result, None);
    }

    #[test]
    fn test_document_state_clear() {
        let state = DocumentState::new();
        let path1 = PathBuf::from("/test1.rs");
        let path2 = PathBuf::from("/test2.rs");
        state.open(
            path1.clone(),
            create_test_uri(),
            "content1".to_string(),
            "rust".to_string(),
        );
        state.open(
            path2.clone(),
            "file:///test2.rs".parse().unwrap(),
            "content2".to_string(),
            "python".to_string(),
        );
        state.clear();

        assert!(state.all_documents().is_empty());
        assert!(!state.is_open(&path1));
        assert!(!state.is_open(&path2));
    }
}
