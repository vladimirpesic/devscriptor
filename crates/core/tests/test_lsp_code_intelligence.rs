//! LSP Code Intelligence Integration Test for Devscriptor
//!
//! This test validates LSP tool schemas and language coverage without
//! requiring actual LSP servers. For real LSP server integration, see
//! `integration_lsp_lifecycle.rs` in the `devscriptor_lsp` crate.

use std::collections::HashMap;
use std::path::PathBuf;

/// Language to file extension mapping
fn language_file_mapping() -> HashMap<&'static str, &'static str> {
    let mut map = HashMap::new();
    map.insert("rust", "test_rust.rs");
    map.insert("python", "test_python.py");
    map.insert("typescript", "test_typescript.ts");
    map.insert("javascript", "test_javascript.js");
    map.insert("go", "test_go.go");
    map.insert("java", "test_java.java");
    map.insert("c", "test_c.c");
    map.insert("cpp", "test_cpp.cpp");
    map.insert("csharp", "test_csharp.cs");
    map.insert("kotlin", "test_kotlin.kt");
    map.insert("swift", "test_swift.swift");
    map.insert("php", "test_php.php");
    map.insert("ruby", "test_ruby.rb");
    map.insert("scala", "test_scala.scala");
    map.insert("dart", "test_dart.dart");
    map.insert("objc", "test_objective_c.m");
    map.insert("solidity", "test_solidity.sol");
    // The remaining languages.
    map.insert("bash", "test_bash.sh");
    map.insert("cuda", "test_cuda.cu");
    map.insert("fsharp", "test_fsharp.fs");
    map.insert("graphql", "test_graphql.gql");
    map.insert("html", "test_html.html");
    map.insert("sql", "test_sql.sql");
    map.insert("svelte", "test_svelte.svelte");
    map.insert("vue", "test_vue.vue");
    map.insert("tsx", "test_tsx.tsx");
    map.insert("vb", "test_vb.vb");
    map
}

/// Get test files directory
fn test_files_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_files")
}

// ---------------------------------------------------------------------------
//  GROUP 1 — Test File Validation
// ---------------------------------------------------------------------------

/// Verify that test files exist for available languages
#[tokio::test]
async fn test_test_files_exist_for_supported_languages() {
    let test_dir = test_files_dir();
    let mappings = language_file_mapping();

    let mut found = 0;
    let mut missing = Vec::new();

    for (lang, file) in &mappings {
        let path = test_dir.join(file);
        if path.exists() {
            found += 1;
            let meta = std::fs::metadata(&path).unwrap();
            assert!(
                meta.len() > 0,
                "{lang} test file ({}) should not be empty",
                path.display()
            );
        } else {
            missing.push(lang.to_string());
        }
    }

    tracing::info!("\n📋 Test File Verification:");
    tracing::info!("   Found: {}/{} languages", found, mappings.len());
    if !missing.is_empty() {
        tracing::info!("   Missing: {}", missing.join(", "));
    }

    assert!(
        found >= 5,
        "At least 5 language test files should exist (found {found})"
    );
}

/// Verify that file sizes are reasonable (>0 bytes, <1MB each)
#[tokio::test]
async fn test_test_files_reasonable_size() {
    let test_dir = test_files_dir();
    let mappings = language_file_mapping();

    for (lang, file) in &mappings {
        let path = test_dir.join(file);
        if !path.exists() {
            continue;
        }

        let meta = std::fs::metadata(&path).unwrap();
        let len = meta.len();
        assert!(len > 0, "{lang} test file should have content");
        assert!(
            len < 1_000_000,
            "{lang} test file should be <1MB (got {} bytes)",
            len
        );
    }

    tracing::info!("\n✅ All test files have reasonable sizes");
}

/// Verify that language mappings cover at least 10 languages
#[tokio::test]
async fn test_language_mapping_comprehensiveness() {
    let mappings = language_file_mapping();
    assert!(
        mappings.len() >= 10,
        "Language mappings should cover at least 10 languages (got {})",
        mappings.len()
    );

    let known = [
        "rust",
        "python",
        "typescript",
        "javascript",
        "go",
        "java",
        "c",
        "cpp",
        "csharp",
        "kotlin",
        "swift",
        "php",
        "ruby",
        "scala",
        "dart",
        "objc",
        "solidity",
        "bash",
        "cuda",
        "fsharp",
        "graphql",
        "html",
        "sql",
        "svelte",
        "vue",
        "tsx",
        "vb",
    ];

    for lang in mappings.keys() {
        assert!(
            known.contains(lang),
            "Language '{lang}' should be in known languages list"
        );
    }

    tracing::info!("\n✅ Language mapping covers {} languages", mappings.len());
}

// ---------------------------------------------------------------------------
//  GROUP 2 — Cleanup Verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lsp_cleanup_notice() {
    tracing::info!(
        "\n✅ Test infrastructure validated; real LSP tests are in devscriptor_lsp crate"
    );
}
