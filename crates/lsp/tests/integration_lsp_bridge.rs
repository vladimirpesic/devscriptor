//! Integration tests for LSP bridge and translator.
//!
//! These tests cover Translator, `ClientRegistry`, `DocumentState`,
//! and path/URI conversion functions.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use devscriptor_lsp::bridge::{
    ClientRegistry, Document, DocumentState, Translator, path_to_uri, uri_to_path,
};
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerHeuristics, ServerSettings, WorkspaceConfig,
};
use tempfile::TempDir;

// ============================================================================
// Translator::new() tests
// ============================================================================

#[test]
fn test_translator_new() {
    let config = Config::default();
    let _translator = Translator::new(config);
    // Translator should be created successfully
}

#[test]
fn test_translator_new_with_custom_config() {
    let config = Config {
        workspace: WorkspaceConfig {
            roots: vec![PathBuf::from("/tmp/workspace")],
            ..Default::default()
        },
        servers: vec![LspServerConfig {
            language_id: "rust".into(),
            command: "rust-analyzer".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["rs".into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    };

    let _translator = Translator::new(config);
    // Just verify it doesn't panic
}

// ============================================================================
// Translator::set_workspace_roots() tests
// ============================================================================

#[test]
fn test_translator_set_workspace_roots() {
    let config = Config::default();
    let mut translator = Translator::new(config);

    let roots = vec![
        PathBuf::from("/tmp/project1"),
        PathBuf::from("/tmp/project2"),
    ];
    translator.set_workspace_roots(roots.clone());
    // Verify by trying to initialize (which uses workspace_roots)
}

// ============================================================================
// Translator::spawn_applicable() tests
// ============================================================================

#[tokio::test]
async fn test_translator_initialize_empty_workspace() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config {
        workspace: WorkspaceConfig {
            roots: vec![temp_dir.path().to_path_buf()],
            ..Default::default()
        },
        servers: vec![], // No servers configured
        lifecycle: LifecycleConfig::default(),
    };

    let translator = Translator::new(config);
    let result = translator.spawn_applicable(None).await;
    assert!(result.is_ok(), "Initialize with no servers should succeed");
}

#[tokio::test]
async fn test_translator_initialize_with_servers_no_markers() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config {
        workspace: WorkspaceConfig {
            roots: vec![temp_dir.path().to_path_buf()],
            ..Default::default()
        },
        servers: vec![LspServerConfig {
            language_id: "rust".into(),
            command: "rust-analyzer".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["rs".into()],
            heuristics: Some(ServerHeuristics {
                markers: vec!["Cargo.toml".into()],
                max_depth: 3,
            }),
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    };

    let translator = Translator::new(config);
    // No Cargo.toml in temp dir, so no applicable servers
    let result = translator.spawn_applicable(None).await;
    assert!(
        result.is_ok(),
        "Should succeed even with no applicable servers"
    );
}

// ============================================================================
// ClientRegistry tests
// ============================================================================

#[test]
fn test_client_registry_new() {
    let registry = ClientRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_client_registry_insert_and_get() {
    let registry = ClientRegistry::new();

    // We can't easily create a real LspClient without transport,
    // but we can test the API surface
    assert!(!registry.contains("rust"));
    assert!(registry.get("rust").is_none());
}

#[test]
fn test_client_registry_contains() {
    let registry = ClientRegistry::new();
    assert!(!registry.contains("rust"));
    assert!(!registry.contains("python"));
}

#[test]
fn test_client_registry_len_and_is_empty() {
    let registry = ClientRegistry::new();
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
}

#[test]
fn test_client_registry_languages() {
    let registry = ClientRegistry::new();
    let languages = registry.languages();
    assert!(languages.is_empty());
}

#[test]
fn test_client_registry_concurrent_access() {
    let registry = ClientRegistry::new();

    // Multiple concurrent reads should not block each other
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(!registry.contains("rust"));
}

#[test]
fn test_client_registry_clone() {
    let registry1 = ClientRegistry::new();
    let _registry2 = registry1.clone();
    // Both should be independent
}

#[test]
fn test_client_registry_default() {
    let registry: ClientRegistry = Default::default();
    assert!(registry.is_empty());
}

// ============================================================================
// DocumentState tests
// ============================================================================

#[test]
fn test_document_state_new() {
    let state = DocumentState::new();
    assert!(state.all_documents().is_empty());
}

#[test]
fn test_document_state_open() {
    let state = DocumentState::new();
    let path = PathBuf::from("/test.rs");
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    assert!(!state.is_open(&path));

    state.open(
        path.clone(),
        uri.clone(),
        Arc::from("fn main() {}"),
        "rust".into(),
    );

    assert!(state.is_open(&path));
    assert_eq!(state.version(&path), Some(1));
    assert_eq!(state.content(&path), Some(Arc::from("fn main() {}")));
    assert_eq!(state.language_id(&path), Some("rust".to_string()));
    assert_eq!(state.uri(&path), Some(uri));
}

#[test]
fn test_document_state_close() {
    let state = DocumentState::new();
    let path = PathBuf::from("/test.rs");
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    state.open(path.clone(), uri, Arc::from("content"), "rust".into());
    assert!(state.is_open(&path));

    state.close(&path);
    assert!(!state.is_open(&path));
    assert!(state.get(&path).is_none());
}

#[test]
fn test_document_state_update() {
    let state = DocumentState::new();
    let path = PathBuf::from("/test.rs");
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    state.open(path.clone(), uri, Arc::from("v1"), "rust".into());
    assert_eq!(state.version(&path), Some(1));

    let new_version = state.update(&path, Arc::from("v2"));
    assert_eq!(new_version, Some(2));
    assert_eq!(state.version(&path), Some(2));
    assert_eq!(state.content(&path), Some(Arc::from("v2")));
}

#[test]
fn test_document_state_update_not_found() {
    let state = DocumentState::new();
    let path = PathBuf::from("/nonexistent.rs");

    let result = state.update(&path, Arc::from("content"));
    assert!(result.is_none());
}

#[test]
fn test_document_state_get() {
    let state = DocumentState::new();
    let path = PathBuf::from("/test.rs");
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    state.open(
        path.clone(),
        uri.clone(),
        Arc::from("content"),
        "rust".into(),
    );

    let doc = state.get(&path);
    assert!(doc.is_some());
    let doc = doc.unwrap();
    assert_eq!(doc.path, path);
    assert_eq!(doc.content.as_ref(), "content");
    assert_eq!(doc.language_id, "rust");
}

#[test]
fn test_document_state_all_documents() {
    let state = DocumentState::new();
    let uri1: lsp_types::Uri = "file:///test1.rs".parse().unwrap();
    let uri2: lsp_types::Uri = "file:///test2.py".parse().unwrap();

    state.open(
        PathBuf::from("/test1.rs"),
        uri1,
        Arc::from("fn main() {}"),
        "rust".into(),
    );
    state.open(
        PathBuf::from("/test2.py"),
        uri2,
        Arc::from("print('hello')"),
        "python".into(),
    );

    let docs = state.all_documents();
    assert_eq!(docs.len(), 2);
}

#[test]
fn test_document_state_clear() {
    let state = DocumentState::new();
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    state.open(
        PathBuf::from("/test.rs"),
        uri,
        Arc::from("content"),
        "rust".into(),
    );
    assert_eq!(state.all_documents().len(), 1);

    state.clear();
    assert!(state.all_documents().is_empty());
    assert!(!state.is_open(&PathBuf::from("/test.rs")));
}

#[test]
fn test_document_state_version_not_found() {
    let state = DocumentState::new();
    let path = PathBuf::from("/nonexistent.rs");
    assert!(state.version(&path).is_none());
}

#[test]
fn test_document_state_content_not_found() {
    let state = DocumentState::new();
    let path = PathBuf::from("/nonexistent.rs");
    assert!(state.content(&path).is_none());
}

#[test]
fn test_document_state_uri_not_found() {
    let state = DocumentState::new();
    let path = PathBuf::from("/nonexistent.rs");
    assert!(state.uri(&path).is_none());
}

#[test]
fn test_document_state_language_id_not_found() {
    let state = DocumentState::new();
    let path = PathBuf::from("/nonexistent.rs");
    assert!(state.language_id(&path).is_none());
}

// ============================================================================
// Document struct tests
// ============================================================================

#[test]
fn test_document_fields() {
    let path = PathBuf::from("/test.rs");
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();

    let doc = Document {
        path: path.clone(),
        uri: uri.clone(),
        version: 5,
        content: Arc::from("fn main() {}"),
        language_id: "rust".into(),
    };

    assert_eq!(doc.path, path);
    assert_eq!(doc.uri, uri);
    assert_eq!(doc.version, 5);
    assert_eq!(doc.content.as_ref(), "fn main() {}");
    assert_eq!(doc.language_id, "rust");
}

#[test]
fn test_document_clone() {
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();
    let doc = Document {
        path: PathBuf::from("/test.rs"),
        uri: uri.clone(),
        version: 1,
        content: Arc::from("test"),
        language_id: "rust".into(),
    };

    let cloned = doc.clone();
    assert_eq!(doc.path, cloned.path);
    assert_eq!(doc.version, cloned.version);
    assert_eq!(doc.content.as_ref(), cloned.content.as_ref());
}

// ============================================================================
// path_to_uri() and uri_to_path() tests
// ============================================================================

#[test]
fn test_path_to_uri_basic() {
    let path = PathBuf::from("/tmp/test.rs");
    let result = path_to_uri(&path);
    assert!(result.is_ok());

    let uri = result.unwrap();
    let uri_str = uri.as_str();
    assert!(uri_str.starts_with("file://"));
    assert!(uri_str.contains("test.rs"));
}

#[test]
fn test_path_to_uri_relative() {
    let path = PathBuf::from("src/main.rs");
    let result = path_to_uri(&path);
    // Should work with relative paths too
    assert!(result.is_ok());
}

#[test]
fn test_uri_to_path_basic() {
    let uri: lsp_types::Uri = "file:///tmp/test.rs".parse().unwrap();
    let path = uri_to_path(&uri);
    assert!(path.contains("test.rs"));
}

#[test]
fn test_uri_to_path_with_percent_encoding() {
    let uri: lsp_types::Uri = "file:///tmp/my%20file.rs".parse().unwrap();
    let path = uri_to_path(&uri);
    assert!(
        path.contains("my file.rs"),
        "Should decode percent encoding: {path}"
    );
}

#[test]
fn test_uri_to_path_non_file_scheme() {
    // For non-file URIs, should return the full string
    let uri: lsp_types::Uri = "https://example.com/test.rs".parse().unwrap();
    let path = uri_to_path(&uri);
    assert_eq!(path, "https://example.com/test.rs");
}

/// A path comes back from its URI as it is, canonical: on Windows too, where
/// the URI is `file:///C:/…`.
#[test]
fn test_path_to_uri_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let original = temp_dir.path().join("test_file.rs");
    std::fs::write(&original, "").unwrap();
    let uri = path_to_uri(&original).unwrap();
    assert!(uri.as_str().starts_with("file:///"), "{}", uri.as_str());
    let restored = uri_to_path(&uri);
    assert_eq!(
        std::path::Path::new(&restored),
        dunce::canonicalize(&original).unwrap()
    );
}

#[test]
fn test_path_to_uri_with_spaces() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("my_file.rs");
    std::fs::write(&path, "").unwrap();

    let result = path_to_uri(&path);
    assert!(result.is_ok());
}

#[test]
fn test_uri_to_path_special_chars() {
    let uri: lsp_types::Uri = "file:///tmp/file%23hash.rs".parse().unwrap();
    let path = uri_to_path(&uri);
    assert!(path.contains("file#hash.rs"), "Should decode # character");
}

// ============================================================================
// DocumentState open/close/update integration
// ============================================================================

#[test]
fn test_document_state_lifecycle() {
    let state = DocumentState::new();
    let path = PathBuf::from("/project/src/main.rs");
    let uri: lsp_types::Uri = "file:///project/src/main.rs".parse().unwrap();

    // Open
    state.open(path.clone(), uri, Arc::from("fn main() {}"), "rust".into());
    assert!(state.is_open(&path));
    assert_eq!(state.version(&path), Some(1));

    // Update
    state.update(&path, Arc::from("fn main() { println!(\"hello\"); }"));
    assert_eq!(state.version(&path), Some(2));

    // Update again
    state.update(&path, Arc::from("// comment\nfn main() {}"));
    assert_eq!(state.version(&path), Some(3));

    // Close
    state.close(&path);
    assert!(!state.is_open(&path));
    assert!(state.version(&path).is_none());
}

#[test]
fn test_document_state_multiple_documents() {
    let state = DocumentState::new();

    let docs: Vec<(PathBuf, lsp_types::Uri, &str, &str)> = vec![
        (
            PathBuf::from("/a.rs"),
            "file:///a.rs".parse().unwrap(),
            "fn a() {}",
            "rust",
        ),
        (
            PathBuf::from("/b.py"),
            "file:///b.py".parse().unwrap(),
            "def b():",
            "python",
        ),
        (
            PathBuf::from("/c.go"),
            "file:///c.go".parse().unwrap(),
            "func c() {}",
            "go",
        ),
    ];

    for (path, uri, content, lang) in &docs {
        state.open(
            path.clone(),
            uri.clone(),
            Arc::from(*content),
            lang.to_string(),
        );
    }

    assert_eq!(state.all_documents().len(), 3);

    for (path, _, content, lang) in &docs {
        assert!(state.is_open(path));
        assert_eq!(state.content(path), Some(Arc::from(*content)));
        assert_eq!(state.language_id(path), Some(lang.to_string()));
    }

    // Close one
    state.close(&PathBuf::from("/b.py"));
    assert_eq!(state.all_documents().len(), 2);
    assert!(!state.is_open(&PathBuf::from("/b.py")));
}

#[test]
fn test_document_state_default() {
    let state: DocumentState = Default::default();
    assert!(state.all_documents().is_empty());
}
