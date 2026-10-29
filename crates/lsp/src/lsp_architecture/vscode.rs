//! VS Code Extension LSP Detection
//!
//! Maps Devscriptor server IDs to VS Code extension paths.
//! Supports 21 programming languages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use tracing::{debug, warn};

/// Cache of VS Code extension directory scan results.
///
/// Built once on first access, so the extensions directory is not scanned again
/// for every server lookup.
static VSCODE_EXTENSION_INDEX: LazyLock<HashMap<&'static str, PathBuf>> =
    LazyLock::new(build_extension_index);

/// Build a single index of all VS Code extensions → LSP binary paths.
fn build_extension_index() -> HashMap<&'static str, PathBuf> {
    let mut index = HashMap::new();
    let extensions_dir = match get_vscode_extensions_dir() {
        Some(d) => d,
        None => return index,
    };

    let entries = match std::fs::read_dir(&extensions_dir) {
        Ok(e) => e,
        Err(_) => return index,
    };

    // Collect all directory entries once
    let dir_entries: Vec<_> = entries.flatten().collect();

    for ext_info in VSCODE_EXTENSIONS {
        let pattern_prefix = format!("{}.{}", ext_info.publisher, ext_info.extension_name);

        for entry in &dir_entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if !name_str.starts_with(&pattern_prefix) {
                continue;
            }

            let extension_path = entry.path();

            // Try primary binary path
            let primary_path = extension_path.join(ext_info.lsp_binary_path);
            if primary_path.exists() {
                index.insert(ext_info.server_id, primary_path);
                break;
            }

            // Try alternative paths
            let mut found = false;
            for alt_path in ext_info.alt_binary_paths {
                let full_path = extension_path.join(alt_path);
                if full_path.exists() {
                    index.insert(ext_info.server_id, full_path);
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
    }

    index
}

/// Information about a VS Code extension that provides an LSP server
#[derive(Debug, Clone)]
pub struct VsCodeExtension {
    /// Server ID used by Devscriptor
    pub server_id: &'static str,
    /// Human-readable name
    pub name: &'static str,
    /// Extension directory pattern (e.g., "rust-lang.rust-analyzer-*")
    pub extension_pattern: &'static str,
    /// Publisher name for exact matching
    pub publisher: &'static str,
    /// Extension name (without publisher)
    pub extension_name: &'static str,
    /// Relative path to LSP binary within extension
    pub lsp_binary_path: &'static str,
    /// Alternative binary paths to try
    pub alt_binary_paths: &'static [&'static str],
    /// Supported languages
    pub languages: &'static [&'static str],
}

impl VsCodeExtension {
    /// Get the extension pattern for directory matching
    #[must_use]
    pub fn pattern(&self) -> String {
        format!("{}.{}", self.publisher, self.extension_name)
    }

    /// Check if this extension supports a given language
    #[must_use]
    pub fn supports_language(&self, language: &str) -> bool {
        self.languages
            .iter()
            .any(|&l| l.eq_ignore_ascii_case(language))
    }
}

/// Complete mapping of 21 LSP servers to VS Code extensions
pub const VSCODE_EXTENSIONS: &[VsCodeExtension] = &[
    // ========== Systems Languages ==========
    VsCodeExtension {
        server_id: "rust-analyzer",
        name: "Rust Analyzer",
        extension_pattern: "rust-lang.rust-analyzer-*",
        publisher: "rust-lang",
        extension_name: "rust-analyzer",
        lsp_binary_path: "server/rust-analyzer",
        alt_binary_paths: &["rust-analyzer"],
        languages: &["rust"],
    },
    VsCodeExtension {
        server_id: "clangd",
        name: "Clangd",
        extension_pattern: "llvm-vs-code-extensions.vscode-clangd-*",
        publisher: "llvm-vs-code-extensions",
        extension_name: "vscode-clangd",
        lsp_binary_path: "bin/clangd",
        alt_binary_paths: &[],
        languages: &["c", "cpp", "cuda", "objc"],
    },
    VsCodeExtension {
        server_id: "gopls",
        name: "Go Language Server",
        extension_pattern: "golang.go-*",
        publisher: "golang",
        extension_name: "go",
        lsp_binary_path: "bin/gopls",
        alt_binary_paths: &["gopls"],
        languages: &["go"],
    },
    // ========== Web Languages ==========
    VsCodeExtension {
        server_id: "typescript-language-server",
        name: "TypeScript/JavaScript",
        extension_pattern: "vscode.typescript-language-features-*",
        publisher: "vscode",
        extension_name: "typescript-language-features",
        lsp_binary_path: "server/typescript-language-server",
        alt_binary_paths: &[
            "server/tsserver",
            "node_modules/.bin/typescript-language-server",
        ],
        languages: &["typescript", "javascript", "tsx", "jsx"],
    },
    VsCodeExtension {
        server_id: "volar",
        name: "Vue Language Server",
        extension_pattern: "vue.volar-*",
        publisher: "vue",
        extension_name: "volar",
        lsp_binary_path: "server/vue-language-server",
        alt_binary_paths: &["node_modules/.bin/vue-language-server"],
        languages: &["vue"],
    },
    VsCodeExtension {
        server_id: "svelte-language-server",
        name: "Svelte Language Server",
        extension_pattern: "svelte.svelte-vscode-*",
        publisher: "svelte",
        extension_name: "svelte-vscode",
        lsp_binary_path: "server/svelteserver",
        alt_binary_paths: &["node_modules/.bin/svelteserver"],
        languages: &["svelte"],
    },
    VsCodeExtension {
        server_id: "html-languageserver",
        name: "HTML Language Server",
        extension_pattern: "vscode.html-language-features-*",
        publisher: "vscode",
        extension_name: "html-language-features",
        lsp_binary_path: "server/html-language-server",
        alt_binary_paths: &["node_modules/.bin/vscode-html-language-server"],
        languages: &["html", "htm"],
    },
    // ========== JVM Languages ==========
    VsCodeExtension {
        server_id: "jdtls",
        name: "Eclipse JDT Language Server",
        extension_pattern: "redhat.java-*",
        publisher: "redhat",
        extension_name: "java",
        lsp_binary_path: "server/jdtls",
        alt_binary_paths: &["server/bin/jdtls"],
        languages: &["java"],
    },
    // "Kotlin by JetBrains" bundles the Kotlin LSP's standalone server.
    VsCodeExtension {
        server_id: "kotlin-lsp",
        name: "Kotlin LSP",
        extension_pattern: "jetbrains.kotlin-server-*",
        publisher: "jetbrains",
        extension_name: "kotlin-server",
        lsp_binary_path: "server/bin/intellij-server",
        alt_binary_paths: &["server/bin/intellij-server.exe"],
        languages: &["kotlin"],
    },
    VsCodeExtension {
        server_id: "metals",
        name: "Scala Metals",
        extension_pattern: "scala-lang.scala-*",
        publisher: "scala-lang",
        extension_name: "scala",
        lsp_binary_path: "server/metals",
        alt_binary_paths: &[],
        languages: &["scala"],
    },
    // ========== Microsoft Ecosystem ==========
    VsCodeExtension {
        server_id: "csharp-ls",
        name: "csharp-ls (C#)",
        extension_pattern: "csharp-ls.csharp-ls-*",
        publisher: "csharp-ls",
        extension_name: "csharp-ls",
        lsp_binary_path: "bin/csharp-ls",
        alt_binary_paths: &[],
        languages: &["csharp", "cs"],
    },
    VsCodeExtension {
        server_id: "fsautocomplete",
        name: "F# Language Server",
        extension_pattern: "ionide.ionide-fsharp-*",
        publisher: "ionide",
        extension_name: "ionide-fsharp",
        lsp_binary_path: "server/fsautocomplete",
        alt_binary_paths: &[],
        languages: &["fsharp", "fs"],
    },
    // ========== Mobile Development ==========
    VsCodeExtension {
        server_id: "dart-ls",
        name: "Dart",
        extension_pattern: "dart-code.dart-code-*",
        publisher: "dart-code",
        extension_name: "dart-code",
        lsp_binary_path: "server/dart",
        alt_binary_paths: &["bin/dart"],
        languages: &["dart"],
    },
    // ========== Functional Languages ==========

    // ========== Other Languages ==========
    VsCodeExtension {
        server_id: "ruby-lsp",
        name: "Ruby LSP",
        extension_pattern: "shopify.ruby-lsp-*",
        publisher: "shopify",
        extension_name: "ruby-lsp",
        lsp_binary_path: "server/ruby-lsp",
        alt_binary_paths: &["bin/ruby-lsp"],
        languages: &["ruby", "rb"],
    },
    VsCodeExtension {
        server_id: "phpactor",
        name: "Phpactor",
        extension_pattern: "phpactor.vscode-phpactor-*",
        publisher: "phpactor",
        extension_name: "vscode-phpactor",
        lsp_binary_path: "phpactor",
        alt_binary_paths: &["out/phpactor"],
        languages: &["php"],
    },
    VsCodeExtension {
        server_id: "bash-language-server",
        name: "Bash IDE",
        extension_pattern: "mads-hartmann.bash-ide-vscode-*",
        publisher: "mads-hartmann",
        extension_name: "bash-ide-vscode",
        lsp_binary_path: "server/bash-language-server",
        alt_binary_paths: &["node_modules/.bin/bash-language-server"],
        languages: &["bash", "sh", "zsh"],
    },
    VsCodeExtension {
        server_id: "graphql-language-service",
        name: "GraphQL",
        extension_pattern: "graphql.vscode-graphql-*",
        publisher: "graphql",
        extension_name: "vscode-graphql",
        lsp_binary_path: "server/graphql-lsp",
        alt_binary_paths: &["node_modules/.bin/graphql-lsp"],
        languages: &["graphql", "gql"],
    },
    VsCodeExtension {
        server_id: "nomicfoundation-solc",
        name: "Solidity",
        extension_pattern: "nomicfoundation.hardhat-solidity-*",
        publisher: "nomicfoundation",
        extension_name: "hardhat-solidity",
        lsp_binary_path: "server/nomicfoundation-solidity-language-server",
        alt_binary_paths: &[],
        languages: &["solidity", "sol"],
    },
];

/// Find VS Code extension mapping by server ID
#[must_use]
pub fn find_by_server_id(server_id: &str) -> Option<&'static VsCodeExtension> {
    VSCODE_EXTENSIONS
        .iter()
        .find(|ext| ext.server_id == server_id)
}

/// Find VS Code extensions by language
#[must_use]
pub fn find_by_language(language: &str) -> Vec<&'static VsCodeExtension> {
    VSCODE_EXTENSIONS
        .iter()
        .filter(|ext| ext.supports_language(language))
        .collect()
}

/// Get the VS Code extensions directory
pub fn get_vscode_extensions_dir() -> Option<PathBuf> {
    // Check common locations
    let home = dirs::home_dir()?;

    let candidates = [
        home.join(".vscode/extensions"),
        home.join(".vscode-server/extensions"),
        home.join(".cursor/extensions"),   // Cursor editor
        home.join(".windsurf/extensions"), // Windsurf editor
        home.join(".code-oss/extensions"),
    ];

    for path in &candidates {
        if path.exists() {
            debug!("Found VS Code extensions directory: {}", path.display());
            return Some(path.clone());
        }
    }

    // Default to standard location even if not found
    Some(home.join(".vscode/extensions"))
}

/// Detect LSP server in VS Code extensions.
///
/// Uses a pre-built index, so the extensions directory is not scanned again on
/// every call.
pub fn detect_lsp_server(server_id: &str) -> Option<PathBuf> {
    let ext_info = find_by_server_id(server_id)?;

    // Fast-path: check the pre-built index first
    if let Some(path) = VSCODE_EXTENSION_INDEX.get(server_id) {
        debug!(
            "Found {} in VS Code extension index: {}",
            server_id,
            path.display()
        );
        return Some(path.clone());
    }

    // Fallback: do a direct scan if the index missed it (e.g. newly installed)
    debug!(
        "{} not in index; scanning VS Code extensions directly",
        server_id
    );
    let extensions_dir = get_vscode_extensions_dir()?;
    let entries = std::fs::read_dir(&extensions_dir).ok()?;
    let pattern_prefix = format!("{}.{}", ext_info.publisher, ext_info.extension_name);

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with(&pattern_prefix) {
            let extension_path = entry.path();

            let primary_path = extension_path.join(ext_info.lsp_binary_path);
            if primary_path.exists() {
                debug!("Found {} at: {}", server_id, primary_path.display());
                return Some(primary_path);
            }

            for alt_path in ext_info.alt_binary_paths {
                let full_path = extension_path.join(alt_path);
                if full_path.exists() {
                    debug!(
                        "Found {} at alternative path: {}",
                        server_id,
                        full_path.display()
                    );
                    return Some(full_path);
                }
            }
        }
    }

    warn!("Could not find {} in VS Code extensions", server_id);
    None
}

/// Scan all VS Code extensions for available LSP servers
#[must_use]
pub fn scan_all_servers() -> Vec<(&'static VsCodeExtension, PathBuf)> {
    let mut found = Vec::new();

    for ext_info in VSCODE_EXTENSIONS {
        if let Some(path) = detect_lsp_server(ext_info.server_id) {
            found.push((ext_info, path));
        }
    }

    found
}

/// Check if a specific LSP server is available in VS Code extensions
#[must_use]
pub fn is_server_available(server_id: &str) -> bool {
    detect_lsp_server(server_id).is_some()
}

/// Check if VS Code extensions directory is available
#[must_use]
pub fn is_vscode_available() -> bool {
    get_vscode_extensions_dir().is_some()
}

/// Get all available servers for a language
#[must_use]
pub fn get_available_servers_for_language(
    language: &str,
) -> Vec<(&'static VsCodeExtension, PathBuf)> {
    let mut results = Vec::new();

    for ext in find_by_language(language) {
        if let Some(path) = detect_lsp_server(ext.server_id) {
            results.push((ext, path));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_by_server_id() {
        let ext = find_by_server_id("rust-analyzer");
        assert!(ext.is_some());
        assert_eq!(ext.unwrap().publisher, "rust-lang");
    }

    #[test]
    fn test_find_by_language() {
        let exts = find_by_language("rust");
        assert!(!exts.is_empty());
        assert!(exts.iter().any(|e| e.server_id == "rust-analyzer"));
    }

    #[test]
    fn test_supports_language() {
        let ext = find_by_server_id("typescript-language-server").unwrap();
        assert!(ext.supports_language("typescript"));
        assert!(ext.supports_language("javascript"));
        assert!(!ext.supports_language("rust"));
    }

    #[test]
    fn test_all_servers_have_valid_patterns() {
        for ext in VSCODE_EXTENSIONS {
            assert!(!ext.server_id.is_empty());
            assert!(!ext.publisher.is_empty());
            assert!(!ext.extension_name.is_empty());
            assert!(!ext.lsp_binary_path.is_empty());
            assert!(!ext.languages.is_empty());
        }
    }
}
