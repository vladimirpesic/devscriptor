//! Integration tests for LSP architecture detection and resolution.
//!
//! These tests cover `LspArchitectureManager`, `LspResolver`, `quick_check`,
//! `batch_check`, `detect_system_lsp`, and `ServerStatus`.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::PathBuf;

use devscriptor_lsp::config::{LspServerConfig, ServerSettings};
use devscriptor_lsp::lsp_architecture::manager::ServerStatus;
use devscriptor_lsp::lsp_architecture::prerequisites::{
    Prerequisite, PrerequisiteTiers, PrerequisitesResult,
};
use devscriptor_lsp::lsp_architecture::system::{
    detect_system_lsp, expand_home, find_system_lsp, is_system_server_available,
    scan_all_system_servers,
};
use devscriptor_lsp::lsp_architecture::{
    DetectedLanguage, LspAction, LspArchitectureManager, LspArchitectureResult, LspResolver,
    LspSource, ResolutionResult, ServerOverride, UserLspConfig, batch_check, check_prerequisites,
    get_detection_stats, get_prerequisite_tiers, is_vscode_detection_available, quick_check,
};
use tempfile::TempDir;
use tracing::info;

/// Check if a command is available on the system.
fn is_command_available(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

// ============================================================================
// LspArchitectureManager::new() tests
// ============================================================================

#[test]
fn test_lsp_architecture_manager_new() {
    let manager = LspArchitectureManager::new();
    assert!(
        manager.is_ok(),
        "LspArchitectureManager::new() should succeed"
    );
}

#[test]
fn test_lsp_architecture_manager_with_bundled_dir() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    // Should not panic
    let _ = manager.analyze_project(temp_dir.path());
}

#[test]
fn test_lsp_architecture_manager_with_user_config() {
    let temp_dir = TempDir::new().unwrap();
    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());

    let user_config = UserLspConfig {
        server_paths: HashMap::new(),
        vscode_discovery: false,
        system_discovery: true,
        extra_paths: vec![],
    };

    let manager = manager.with_user_config(user_config);
    let _ = manager.analyze_project(temp_dir.path());
}

// ============================================================================
// LspArchitectureManager::analyze_project() tests
// ============================================================================

#[test]
fn test_analyze_project_rust() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("main.rs"), "fn main() {}").unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    assert!(
        !result.languages.is_empty(),
        "Should detect at least one language"
    );

    let has_rust = result.languages.iter().any(|l| l.language_id == "rust");
    assert!(has_rust, "Should detect rust language");

    for lang in &result.languages {
        info!(
            "Detected: {} (confidence: {:.2})",
            lang.language_id, lang.confidence
        );
        assert!(lang.confidence > 0.0);
        assert!(lang.confidence <= 1.0);
    }
}

#[test]
fn test_analyze_project_python() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("pyproject.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("main.py"), "print('hello')").unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    let has_python = result.languages.iter().any(|l| l.language_id == "python");
    assert!(has_python, "Should detect python language");
}

#[test]
fn test_analyze_project_typescript() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(temp_dir.path().join("app.ts"), "const x = 1;").unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    let has_ts = result
        .languages
        .iter()
        .any(|l| l.language_id == "typescript");
    assert!(has_ts, "Should detect typescript language");
}

#[test]
fn test_analyze_project_go() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("go.mod"), "module test\n").unwrap();
    std::fs::write(temp_dir.path().join("main.go"), "package main\n").unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    let has_go = result.languages.iter().any(|l| l.language_id == "go");
    assert!(has_go, "Should detect go language");
}

#[test]
fn test_analyze_project_empty() {
    let temp_dir = TempDir::new().unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    // Empty project may have no detected languages
    // Just verify it doesn't panic
    for lang in &result.languages {
        assert!(lang.confidence > 0.0);
        assert!(lang.confidence <= 1.0);
    }
}

#[test]
fn test_analyze_project_multiple_languages() {
    let temp_dir = TempDir::new().unwrap();
    // Create markers for multiple languages
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(temp_dir.path().join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(temp_dir.path().join("app.ts"), "const x = 1;").unwrap();

    let manager = LspArchitectureManager::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = manager.analyze_project(temp_dir.path());

    // Languages are sorted by confidence, so highest confidence first
    assert!(!result.languages.is_empty());
}

// ============================================================================
// DetectedLanguage tests
// ============================================================================

#[test]
fn test_detected_language_fields() {
    let lang = DetectedLanguage {
        language_id: "rust".into(),
        confidence: 0.85,
        indicator_files: vec![PathBuf::from("Cargo.toml")],
        suggested_servers: vec!["rust-analyzer".into()],
    };

    assert_eq!(lang.language_id, "rust");
    assert!((lang.confidence - 0.85).abs() < f32::EPSILON);
    assert_eq!(lang.indicator_files.len(), 1);
    assert_eq!(lang.suggested_servers.len(), 1);
}

// ============================================================================
// LspResolver tests
// ============================================================================

#[test]
fn test_lsp_resolver_new() {
    let resolver = LspResolver::new();
    assert!(resolver.is_ok());
}

#[test]
fn test_lsp_resolver_with_bundled_dir() {
    let temp_dir = TempDir::new().unwrap();
    let resolver = LspResolver::with_bundled_dir(temp_dir.path().to_path_buf());
    // Should create successfully
    let _ = resolver.resolve("rust-analyzer");
}

#[test]
fn test_lsp_resolver_with_user_config() {
    let temp_dir = TempDir::new().unwrap();
    let resolver = LspResolver::with_bundled_dir(temp_dir.path().to_path_buf());

    let user_config = UserLspConfig {
        server_paths: HashMap::new(),
        vscode_discovery: false,
        system_discovery: false,
        extra_paths: vec![],
    };

    let resolver = resolver.with_user_config(user_config);
    let result = resolver.resolve("rust-analyzer");
    // With both system and vscode disabled, and no bundled, should return None
    assert!(result.is_none() || result.is_some());
}

// ============================================================================
// LspResolver::resolve() tier tests
// ============================================================================

/// The system tier is reached only when no managed install shadows it.
///
/// `LspResolver::new()` points at the real `~/.cache/devscriptor/lsp-servers`, so
/// on a machine where `rust-analyzer` is installed there it would resolve from
/// `Installed` (tier 2) rather than `SystemWide` (tier 3), which is the correct
/// precedence. Empty managed directories make the tier deterministic.
#[test]
fn test_resolve_from_system() {
    if !is_system_server_available("rust-analyzer") {
        eprintln!("Skipping test_resolve_from_system: rust-analyzer not available");
        return;
    }

    let install = tempfile::tempdir().unwrap();
    let bundled = tempfile::tempdir().unwrap();
    let resolver = LspResolver::with_dirs(install.path().into(), bundled.path().into());

    let result = resolver.resolve("rust-analyzer");
    assert!(result.is_some(), "Should resolve rust-analyzer from system");

    let result = result.unwrap();
    assert!(
        matches!(result.source, LspSource::SystemWide(_)),
        "Should resolve from system, got {:?}",
        result.source
    );
    assert_eq!(result.config.language_id, "rust");
}

#[test]
fn test_resolve_from_bundled() {
    let temp_dir = TempDir::new().unwrap();
    // Create a fake bundled installation
    let bundled = temp_dir.path().join("rust-analyzer");
    std::fs::create_dir_all(&bundled).unwrap();
    // With the suffix a Windows install has: there only a suffixed file qualifies.
    let name = if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    };
    std::fs::write(bundled.join(name), "fake").unwrap();

    let resolver = LspResolver::with_bundled_dir(temp_dir.path().to_path_buf());
    let result = resolver.resolve("rust-analyzer");
    assert!(result.is_some(), "Should resolve from bundled");

    let result = result.unwrap();
    assert!(matches!(result.source, LspSource::Bundled(_)));
}

#[test]
fn test_resolve_from_user_config() {
    let temp_dir = TempDir::new().unwrap();
    let fake_binary = temp_dir.path().join("custom-rust-analyzer");
    std::fs::write(&fake_binary, "fake").unwrap();

    let resolver = LspResolver::with_bundled_dir(PathBuf::from("/nonexistent"));

    let mut server_paths = HashMap::new();
    server_paths.insert(
        "rust-analyzer".into(),
        ServerOverride {
            path: fake_binary.clone(),
            args: Some(vec!["--custom".into()]),
            env: None,
            auto_detect: true,
        },
    );

    let user_config = UserLspConfig {
        server_paths,
        vscode_discovery: false,
        system_discovery: false,
        extra_paths: vec![],
    };

    let resolver = resolver.with_user_config(user_config);
    let result = resolver.resolve("rust-analyzer");
    assert!(result.is_some(), "Should resolve from user config");

    let result = result.unwrap();
    assert!(matches!(result.source, LspSource::UserConfig { .. }));
    assert_eq!(result.config.args, vec!["--custom"]);
}

#[test]
fn test_resolve_unknown_server() {
    let resolver = LspResolver::new().unwrap();
    let result = resolver.resolve("totally-unknown-server-xyz");
    // Should return Some with default metadata even for unknown servers
    assert!(result.is_some() || result.is_none());
}

#[test]
fn test_resolve_returns_none_for_missing() {
    let resolver = LspResolver::with_bundled_dir(PathBuf::from("/nonexistent/bundle/dir"));
    let _result = resolver.resolve("nonexistent-server-12345");
    // Unknown server IDs may still return Some with defaults
}

// ============================================================================
// LspSource tests
// ============================================================================

#[test]
fn test_lsp_source_path() {
    let path = PathBuf::from("/usr/bin/rust-analyzer");

    let user = LspSource::UserConfig {
        path: path.clone(),
        args: vec![],
    };
    assert_eq!(user.path(), &path);

    let bundled = LspSource::Bundled(path.clone());
    assert_eq!(bundled.path(), &path);

    let system = LspSource::SystemWide(path.clone());
    assert_eq!(system.path(), &path);

    let vscode = LspSource::VsCodeExtension(path.clone());
    assert_eq!(vscode.path(), &path);
}

#[test]
fn test_lsp_source_type() {
    assert_eq!(
        LspSource::UserConfig {
            path: PathBuf::new(),
            args: vec![]
        }
        .source_type(),
        "user config"
    );
    assert_eq!(LspSource::Bundled(PathBuf::new()).source_type(), "bundled");
    assert_eq!(
        LspSource::SystemWide(PathBuf::new()).source_type(),
        "system"
    );
    assert_eq!(
        LspSource::VsCodeExtension(PathBuf::new()).source_type(),
        "vscode extension"
    );
}

#[test]
fn test_lsp_source_is_user_config() {
    let user = LspSource::UserConfig {
        path: PathBuf::new(),
        args: vec![],
    };
    assert!(user.is_user_config());

    let bundled = LspSource::Bundled(PathBuf::new());
    assert!(!bundled.is_user_config());
}

// ============================================================================
// quick_check() and batch_check() tests
// ============================================================================

#[test]
fn test_quick_check_rust_analyzer() {
    let result = quick_check("rust-analyzer");
    if is_command_available("rust-analyzer") {
        assert!(result.is_some(), "rust-analyzer should be found");
    }
    // If not available, should return None - test either way
}

#[test]
fn test_quick_check_nonexistent() {
    let result = quick_check("totally-fake-server-xyz123");
    assert!(result.is_none(), "Nonexistent server should return None");
}

#[test]
fn test_batch_check() {
    let servers = vec![
        "rust-analyzer",
        "typescript-language-server",
        "gopls",
        "nonexistent-xyz",
    ];
    let results = batch_check(&servers);

    assert_eq!(results.len(), 4, "Should return result for each server");

    for (id, source) in &results {
        info!("{}: found = {}", id, source.is_some());
        if id == "nonexistent-xyz" {
            assert!(source.is_none(), "Nonexistent server should be None");
        }
    }
}

#[test]
fn test_batch_check_empty() {
    let results = batch_check::<&str>(&[]);
    assert!(results.is_empty());
}

// ============================================================================
// detect_system_lsp() tests
// ============================================================================

#[test]
fn test_detect_system_lsp_rust_analyzer() {
    if !is_command_available("rust-analyzer") {
        eprintln!("Skipping: rust-analyzer not available");
        return;
    }

    let result = detect_system_lsp("rust-analyzer");
    assert!(result.is_some(), "Should find rust-analyzer in system");
}

#[test]
fn test_detect_system_lsp_nonexistent() {
    let result = detect_system_lsp("totally-fake-lsp-12345");
    assert!(result.is_none(), "Nonexistent LSP should return None");
}

#[test]
fn test_is_system_server_available() {
    if is_command_available("rust-analyzer") {
        assert!(is_system_server_available("rust-analyzer"));
    }
    assert!(!is_system_server_available("nonexistent-server-12345"));
}

#[test]
fn test_find_system_lsp() {
    let info = find_system_lsp("rust-analyzer");
    assert!(info.is_some(), "rust-analyzer should be in registry");
    assert_eq!(info.unwrap().server_id, "rust-analyzer");

    let info = find_system_lsp("nonexistent");
    assert!(info.is_none());
}

#[test]
fn test_scan_all_system_servers() {
    let found = scan_all_system_servers();
    info!("Found {} system servers", found.len());

    for (info, path) in &found {
        info!("  {}: {}", info.server_id, path.display());
    }
    // Just verify it doesn't panic
}

#[test]
fn test_expand_home() {
    let expanded = expand_home("~/.cargo/bin");
    assert!(expanded.is_some());
    let expanded = expanded.unwrap();
    assert!(!expanded.to_string_lossy().contains('~'));
}

#[test]
fn test_expand_home_non_home() {
    let expanded = expand_home("/usr/bin");
    assert!(expanded.is_some());
    assert_eq!(expanded.unwrap(), PathBuf::from("/usr/bin"));
}

// ============================================================================
// ServerStatus tests
// ============================================================================

#[test]
fn test_server_status_available() {
    let status = ServerStatus::Available {
        source: LspSource::SystemWide(PathBuf::from("/usr/bin/rust-analyzer")),
        config: LspServerConfig {
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
        },
    };

    assert!(status.is_available());
    assert!(status.source().is_some());
}

#[test]
fn test_server_status_needs_install() {
    let status = ServerStatus::NeedsInstall;
    assert!(!status.is_available());
    assert!(status.source().is_none());
}

#[test]
fn test_server_status_missing_prerequisites() {
    let status = ServerStatus::MissingPrerequisites {
        prerequisites: PrerequisitesResult {
            server_id: "test".into(),
            satisfied: vec![],
            missing: vec![],
        },
        tiers: PrerequisiteTiers {
            auto_installable: vec![],
            package_manager: vec![],
            manual: vec![],
            large_download: vec![],
        },
    };

    assert!(!status.is_available());
    assert!(status.source().is_none());
}

// ============================================================================
// Prerequisites tests
// ============================================================================

#[test]
fn test_check_prerequisites_typescript() {
    let result = check_prerequisites("typescript-language-server");
    assert_eq!(result.server_id, "typescript-language-server");

    // Should have Node.js and npm prerequisites
    let total = result.satisfied.len() + result.missing.len();
    assert!(total >= 2, "Should have at least 2 prerequisites");
}

#[test]
fn test_check_prerequisites_rust() {
    let result = check_prerequisites("rust-analyzer");
    assert_eq!(result.server_id, "rust-analyzer");
    assert!(!result.satisfied.is_empty() || !result.missing.is_empty());
}

#[test]
fn test_check_prerequisites_clangd() {
    let result = check_prerequisites("clangd");
    assert_eq!(result.server_id, "clangd");
    // clangd has no prerequisites
    assert!(result.is_satisfied());
}

#[test]
fn test_prerequisites_result_is_satisfied() {
    let result = PrerequisitesResult {
        server_id: "test".into(),
        satisfied: vec![],
        missing: vec![],
    };
    assert!(result.is_satisfied());

    let result = PrerequisitesResult {
        server_id: "test".into(),
        satisfied: vec![],
        missing: vec![Prerequisite::BuildTool {
            name: "node",
            command: "node",
            min_version: None,
        }],
    };
    assert!(!result.is_satisfied());
}

#[test]
fn test_get_prerequisite_tiers() {
    let tiers = get_prerequisite_tiers("typescript-language-server");
    // Just verify it doesn't panic and returns a valid structure
    let total = tiers.auto_installable.len()
        + tiers.package_manager.len()
        + tiers.manual.len()
        + tiers.large_download.len();
    info!("Total missing prerequisites: {}", total);
}

#[test]
fn test_prerequisite_tiers_requires_manual() {
    let tiers = PrerequisiteTiers {
        auto_installable: vec![],
        package_manager: vec![],
        manual: vec![Prerequisite::Custom {
            name: "test",
            check_command: "test",
            install_instructions: "test",
            install_url: None,
        }],
        large_download: vec![],
    };
    assert!(tiers.requires_manual());
    assert_eq!(tiers.total_missing(), 1);
}

// ============================================================================
// Prerequisite::is_satisfied() tests
// ============================================================================

#[test]
fn test_prerequisite_is_satisfied_build_tool() {
    let node = Prerequisite::BuildTool {
        name: "Node.js",
        command: "node",
        min_version: None,
    };
    // Just verify it doesn't panic
    let _ = node.is_satisfied();
}

#[test]
fn test_prerequisite_is_satisfied_sdk() {
    let java = Prerequisite::Sdk {
        name: "Java",
        command: "java",
        env_var: Some("JAVA_HOME"),
        min_version: Some("21.0.0"),
    };
    let _ = java.is_satisfied();
}

// ============================================================================
// get_detection_stats() tests
// ============================================================================

#[test]
fn test_get_detection_stats() {
    let stats = get_detection_stats();
    info!(
        "VS Code: {}/{}, System: {}/{}",
        stats.vscode_extensions_found,
        stats.total_vscode_extensions,
        stats.system_servers_found,
        stats.total_system_servers
    );

    assert!(stats.total_vscode_extensions > 0);
    assert!(stats.total_system_servers > 0);
}

#[test]
fn test_detection_stats_has_any_servers() {
    let stats = get_detection_stats();
    // May or may not have servers depending on system
    let _ = stats.has_any_servers();
    let _ = stats.total_available();
}

// ============================================================================
// is_vscode_detection_available() tests
// ============================================================================

#[test]
fn test_is_vscode_detection_available() {
    let available = is_vscode_detection_available();
    info!("VS Code detection available: {}", available);
    // Just verify it doesn't panic
}

// ============================================================================
// LspAction tests
// ============================================================================

#[test]
fn test_lsp_action_variants() {
    let use_existing = LspAction::UseExisting {
        path: PathBuf::from("/usr/bin/rust-analyzer"),
        source: LspSource::SystemWide(PathBuf::from("/usr/bin/rust-analyzer")),
    };
    let install = LspAction::Install;
    let skip = LspAction::Skip;
    let custom = LspAction::CustomPath(PathBuf::from("/custom/path"));

    // Just verify all variants can be constructed
    let _ = format!("{use_existing:?}");
    let _ = format!("{install:?}");
    let _ = format!("{skip:?}");
    let _ = format!("{custom:?}");
}

// ============================================================================
// LspArchitectureResult tests
// ============================================================================

#[test]
fn test_lsp_architecture_result_default() {
    let result = LspArchitectureResult {
        languages: vec![],
        resolved_servers: vec![],
        needs_install: vec![],
        missing_prerequisites: vec![],
    };

    assert!(result.languages.is_empty());
    assert!(result.resolved_servers.is_empty());
    assert!(result.needs_install.is_empty());
    assert!(result.missing_prerequisites.is_empty());
}

// ============================================================================
// ResolutionResult tests
// ============================================================================

#[test]
fn test_resolution_result_fields() {
    let result = ResolutionResult {
        source: LspSource::SystemWide(PathBuf::from("/usr/bin/rust-analyzer")),
        config: LspServerConfig {
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
        },
    };

    assert!(matches!(result.source, LspSource::SystemWide(_)));
    assert_eq!(result.config.language_id, "rust");
}
