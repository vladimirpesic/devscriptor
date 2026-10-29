//! Integration tests for LSP manager operations.
//!
//! These tests cover `LspManager`, `ServerRegistry`, and project detection
//! functionality without requiring actual LSP server processes.
#![cfg(feature = "lsp")]

use devscriptor_lsp::manager::{InstallationResult, LspManager, ServerRegistry, SourceType};
use tempfile::TempDir;
use tracing::info;

// ============================================================================
// LspManager::new() and ::with_install_dir() tests
// ============================================================================

#[test]
fn test_lsp_manager_new() {
    let manager = LspManager::new();
    assert!(manager.is_ok(), "LspManager::new() should succeed");

    let manager = manager.unwrap();
    // list_available should return servers
    let available = manager.list_available();
    assert!(
        !available.is_empty(),
        "Should have available servers in registry"
    );
}

#[test]
fn test_lsp_manager_with_install_dir() {
    let temp_dir = TempDir::new().unwrap();
    let install_path = temp_dir.path().join("lsp-servers");

    let manager = LspManager::with_install_dir(install_path.clone());
    assert!(
        manager.is_ok(),
        "LspManager::with_install_dir() should succeed"
    );

    // Verify the install directory was created
    assert!(install_path.exists(), "Install directory should be created");
}

#[test]
fn test_lsp_manager_with_install_dir_nested() {
    let temp_dir = TempDir::new().unwrap();
    let nested = temp_dir.path().join("a/b/c/lsp-servers");

    let manager = LspManager::with_install_dir(nested.clone());
    assert!(manager.is_ok());
    assert!(nested.exists());
}

// ============================================================================
// LspManager::list_available() tests
// ============================================================================

#[test]
fn test_list_available_returns_all_servers() {
    let manager = LspManager::new().unwrap();
    let available = manager.list_available();

    info!("Available servers: {}", available.len());
    for server in &available {
        info!("  - {} ({})", server.id, server.name);
    }

    // The registry should have 21 servers
    assert!(
        available.len() >= 21,
        "Expected at least 21 servers, got {}",
        available.len()
    );
}

#[test]
fn test_list_available_server_info_structure() {
    let manager = LspManager::new().unwrap();
    let available = manager.list_available();

    for server in available {
        // Verify all required fields are populated
        assert!(!server.id.is_empty(), "Server ID should not be empty");
        assert!(!server.name.is_empty(), "Server name should not be empty");
        assert!(
            !server.languages.is_empty(),
            "Server should support at least one language"
        );
        assert!(!server.bin_path.is_empty(), "bin_path should not be empty");
        assert!(!server.license.is_empty(), "license should not be empty");
    }
}

#[test]
fn test_list_available_contains_expected_servers() {
    let manager = LspManager::new().unwrap();
    let available = manager.list_available();
    let ids: Vec<_> = available.iter().map(|s| s.id.as_str()).collect();

    // Check for key servers
    let expected = [
        "rust-analyzer",
        "clangd",
        "gopls",
        "typescript-language-server",
        "pyright",
        "jdtls",
        "kotlin-lsp",
        "metals",
        "csharp-ls",
        "fsautocomplete",
        "sourcekit-lsp",
        "dart-ls",
        "ruby-lsp",
        "phpactor",
        "bash-language-server",
        "sql-language-server",
        "volar",
        "svelte-language-server",
        "html-languageserver",
        "graphql-language-service",
        "nomicfoundation-solc",
    ];

    for server_id in &expected {
        assert!(
            ids.contains(server_id),
            "Expected server '{server_id}' not found in registry"
        );
    }
}

// ============================================================================
// LspManager::is_installed() and get_server_path() tests
// ============================================================================

#[test]
fn test_is_installed_false_for_new_manager() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // No servers should be installed in a fresh temp directory
    assert!(!manager.is_installed("rust-analyzer"));
    assert!(!manager.is_installed("pyright"));
    assert!(!manager.is_installed("nonexistent-server"));
}

#[test]
fn test_get_server_path_none_for_uninstalled() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    let path = manager.get_server_path("rust-analyzer");
    assert!(path.is_none(), "Should return None for uninstalled server");
}

#[test]
fn test_is_installed_and_get_path_with_fake_install() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // Simulate an installation by creating the marker files
    let server_dir = temp_dir.path().join("rust-analyzer");
    std::fs::create_dir_all(&server_dir).unwrap();
    std::fs::write(server_dir.join(".installed"), "").unwrap();

    // Place the binary where the registry says it lives, rather than hard-coding a
    // layout: `bin_path` is what both the installer and the resolver consult, and
    // it differs per server (`bin/rust-analyzer`, `node_modules/.bin/…`).
    let registry = devscriptor_lsp::manager::registry::ServerRegistry::load().unwrap();
    let bin_path = registry
        .get_server("rust-analyzer")
        .unwrap()
        .bin_path
        .clone();
    let binary = server_dir.join(&bin_path);
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "fake binary").unwrap();

    assert!(manager.is_installed("rust-analyzer"));
    let path = manager.get_server_path("rust-analyzer");
    assert!(path.is_some(), "binary at {} not found", binary.display());
    assert_eq!(path.unwrap(), binary);
}

#[test]
fn test_is_installed_without_installed_marker() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // Create directory but no .installed marker
    let server_dir = temp_dir.path().join("rust-analyzer");
    std::fs::create_dir_all(&server_dir).unwrap();

    assert!(
        !manager.is_installed("rust-analyzer"),
        "Should not be installed without .installed marker"
    );
}

// ============================================================================
// LspManager::detect_for_project() tests
// ============================================================================

#[test]
fn test_detect_for_project_rust() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"rust-analyzer"),
        "Should detect rust-analyzer for Cargo.toml project"
    );
}

#[test]
fn test_detect_for_project_python() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("pyproject.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"pyright"),
        "Should detect pyright for Python project"
    );
}

#[test]
fn test_detect_for_project_typescript() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"typescript-language-server"),
        "Should detect typescript-language-server for package.json project"
    );
}

#[test]
fn test_detect_for_project_go() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("go.mod"), "module test\n").unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"gopls"), "Should detect gopls for Go project");
}

#[test]
fn test_detect_for_project_java() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("pom.xml"), "<project></project>").unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"jdtls"),
        "Should detect jdtls for Java project"
    );
}

#[test]
fn test_detect_for_project_by_extensions() {
    let temp_dir = TempDir::new().unwrap();
    // No marker files, but create source files with extensions
    std::fs::write(temp_dir.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(temp_dir.path().join("app.py"), "print('hello')").unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"rust-analyzer"),
        "Should detect rust-analyzer from .rs files"
    );
    assert!(
        ids.contains(&"pyright"),
        "Should detect pyright from .py files"
    );
}

#[test]
fn test_detect_for_project_empty() {
    let temp_dir = TempDir::new().unwrap();

    let manager = LspManager::new().unwrap();
    let _detected = manager.detect_for_project(temp_dir.path());

    // Empty directory should return empty or minimal results
    // (depending on temp dir contents)
    // This should not panic
}

#[test]
fn test_detect_for_project_multiple_markers() {
    let temp_dir = TempDir::new().unwrap();
    // Create multiple project markers
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();

    let manager = LspManager::new().unwrap();
    let detected = manager.detect_for_project(temp_dir.path());

    let ids: Vec<_> = detected.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"rust-analyzer"));
    assert!(ids.contains(&"typescript-language-server"));
}

// ============================================================================
// ServerRegistry tests
// ============================================================================

#[test]
fn test_server_registry_load() {
    let registry = ServerRegistry::load();
    assert!(registry.is_ok(), "Registry should load successfully");

    let registry = registry.unwrap();
    assert!(
        !registry.servers.is_empty(),
        "Registry should contain servers"
    );
}

#[test]
fn test_server_registry_get_server() {
    let registry = ServerRegistry::load().unwrap();

    let rust = registry.get_server("rust-analyzer");
    assert!(rust.is_some(), "rust-analyzer should be in registry");
    let rust = rust.unwrap();
    assert_eq!(rust.name, "Rust Analyzer");
    assert!(rust.languages.contains(&"rust".to_string()));

    let ts = registry.get_server("typescript-language-server");
    assert!(ts.is_some());

    let nonexistent = registry.get_server("definitely-not-real");
    assert!(nonexistent.is_none());
}

#[test]
fn test_server_registry_list_servers() {
    let registry = ServerRegistry::load().unwrap();
    let servers = registry.list_servers();

    assert!(
        servers.len() >= 21,
        "Should have at least 21 servers, got {}",
        servers.len()
    );
}

#[test]
fn test_server_registry_find_by_language() {
    let registry = ServerRegistry::load().unwrap();

    let rust = registry.find_by_language("rust");
    assert!(rust.is_some());
    assert_eq!(rust.unwrap().id, "rust-analyzer");

    let python = registry.find_by_language("python");
    assert!(python.is_some());

    let nonexistent = registry.find_by_language("brainfuck");
    assert!(nonexistent.is_none());
}

#[test]
fn test_server_registry_search() {
    let registry = ServerRegistry::load().unwrap();

    let rust_results = registry.search("rust");
    assert!(!rust_results.is_empty(), "Should find rust-related servers");

    let ts_results = registry.search("typescript");
    assert!(!ts_results.is_empty());

    let empty_results = registry.search("xyznonexistent");
    assert!(empty_results.is_empty());
}

// ============================================================================
// ServerInfo structure tests
// ============================================================================

#[test]
fn test_server_info_source_type_variants() {
    // Verify all SourceType variants exist
    let variants = vec![
        SourceType::GithubRelease,
        SourceType::Npm,
        SourceType::Pip,
        SourceType::GoInstall,
        SourceType::DotnetTool,
        SourceType::Gem,
        SourceType::Coursier,
        SourceType::Download,
        SourceType::System,
    ];

    assert_eq!(variants.len(), 9, "Should have 9 source types");
}

#[test]
fn test_server_info_clone_and_eq() {
    let registry = ServerRegistry::load().unwrap();
    let rust = registry.get_server("rust-analyzer").unwrap();

    let cloned = rust.clone();
    assert_eq!(rust.id, cloned.id);
    assert_eq!(rust.name, cloned.name);
    assert_eq!(rust.languages, cloned.languages);
}

// ============================================================================
// LspManager::list_installed() tests
// ============================================================================

#[test]
fn test_list_installed_empty() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    let installed = manager.list_installed().unwrap();
    assert!(
        installed.is_empty(),
        "Fresh install dir should have no installed servers"
    );
}

#[test]
fn test_list_installed_with_fake_servers() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // Create fake installed servers
    for server_id in &["rust-analyzer", "pyright"] {
        let server_dir = temp_dir.path().join(server_id);
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(server_dir.join(".installed"), "").unwrap();
        std::fs::write(server_dir.join(".version"), "1.0.0").unwrap();
    }

    let installed = manager.list_installed().unwrap();
    assert_eq!(installed.len(), 2, "Should find 2 installed servers");

    let ids: Vec<_> = installed.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"rust-analyzer"));
    assert!(ids.contains(&"pyright"));
}

// ============================================================================
// LspManager::uninstall() tests
// ============================================================================

#[test]
fn test_uninstall_removes_server() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // Create a fake installation
    let server_dir = temp_dir.path().join("rust-analyzer");
    std::fs::create_dir_all(&server_dir).unwrap();
    std::fs::write(server_dir.join(".installed"), "").unwrap();

    assert!(manager.is_installed("rust-analyzer"));

    let result = manager.uninstall("rust-analyzer");
    assert!(result.is_ok());
    assert!(!manager.is_installed("rust-analyzer"));
}

#[test]
fn test_uninstall_nonexistent_is_ok() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspManager::with_install_dir(temp_dir.path().to_path_buf()).unwrap();

    // Uninstalling a non-installed server should not error
    let result = manager.uninstall("not-installed");
    assert!(result.is_ok());
}

// ============================================================================
// InstallationResult tests
// ============================================================================

#[test]
fn test_installation_result_variants() {
    let results = vec![
        InstallationResult::Success,
        InstallationResult::AlreadyInstalled,
        InstallationResult::NotInstalled,
        InstallationResult::MissingPrerequisite("node".into()),
        InstallationResult::Failed("network error".into()),
    ];

    for result in results {
        // Just verify they can be created
        let _ = format!("{result:?}");
    }
}
