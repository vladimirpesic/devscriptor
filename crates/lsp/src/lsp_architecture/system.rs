//! System-wide LSP Detection
//!
//! Detects LSP servers installed on the system through:
//! - PATH resolution (which command)
//! - Known SDK paths (rustup, Dart SDK, etc.)
//! - Environment variables

use std::path::PathBuf;
use tracing::{debug, trace};

/// Information about system LSP detection for a server
#[derive(Debug, Clone)]
pub struct SystemLspInfo {
    /// Server ID
    pub server_id: &'static str,
    /// Binary names to try (in order of preference)
    pub binary_names: &'static [&'static str],
    /// Known SDK paths to check (platform-specific)
    pub sdk_paths: &'static [SdkPath],
    /// Environment variable that might contain the path
    pub env_var: Option<&'static str>,
    /// Special detection logic required
    pub special_detector: Option<fn() -> Option<PathBuf>>,
}

/// Known SDK installation path
#[derive(Debug, Clone)]
pub struct SdkPath {
    /// Base directory (can use ~ for home)
    pub base: &'static str,
    /// Relative path from base to binary
    pub relative: &'static str,
    /// Description for logging
    pub description: &'static str,
}

/// Platform-specific SDK paths for Rust
const RUST_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.cargo/bin",
        relative: "rust-analyzer",
        description: "Cargo bin directory",
    },
    SdkPath {
        base: "~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin",
        relative: "rust-analyzer",
        description: "Rustup toolchain (Linux x86_64)",
    },
    SdkPath {
        base: "~/.rustup/toolchains/stable-aarch64-unknown-linux-gnu/bin",
        relative: "rust-analyzer",
        description: "Rustup toolchain (Linux ARM64)",
    },
    SdkPath {
        base: "~/.rustup/toolchains/stable-x86_64-apple-darwin/bin",
        relative: "rust-analyzer",
        description: "Rustup toolchain (macOS x86_64)",
    },
    SdkPath {
        base: "~/.rustup/toolchains/stable-aarch64-apple-darwin/bin",
        relative: "rust-analyzer",
        description: "Rustup toolchain (macOS ARM64)",
    },
];

/// Platform-specific SDK paths for Clangd
const CLANGD_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "/usr/lib/llvm-18/bin",
        relative: "clangd",
        description: "LLVM 18 (Linux)",
    },
    SdkPath {
        base: "/usr/lib/llvm-17/bin",
        relative: "clangd",
        description: "LLVM 17 (Linux)",
    },
    SdkPath {
        base: "/usr/lib/llvm-16/bin",
        relative: "clangd",
        description: "LLVM 16 (Linux)",
    },
    SdkPath {
        base: "/usr/lib/llvm-15/bin",
        relative: "clangd",
        description: "LLVM 15 (Linux)",
    },
    SdkPath {
        base: "/usr/lib/llvm-14/bin",
        relative: "clangd",
        description: "LLVM 14 (Linux)",
    },
    SdkPath {
        base: "/usr/local/opt/llvm/bin",
        relative: "clangd",
        description: "Homebrew LLVM (macOS Intel)",
    },
    SdkPath {
        base: "/opt/homebrew/opt/llvm/bin",
        relative: "clangd",
        description: "Homebrew LLVM (macOS ARM64)",
    },
    SdkPath {
        base: "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin",
        relative: "clangd",
        description: "Xcode toolchain (macOS)",
    },
    SdkPath {
        base: "/usr/bin",
        relative: "clangd",
        description: "System PATH",
    },
];

// ========== Platform-specific SDK paths for Go ==========
const GO_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/go/bin",
        relative: "gopls",
        description: "Go bin directory",
    },
    SdkPath {
        base: "/usr/local/go/bin",
        relative: "gopls",
        description: "System Go installation",
    },
    SdkPath {
        base: "/usr/bin",
        relative: "gopls",
        description: "System bin",
    },
];

// ========== Platform-specific SDK paths for Python (Pyright) ==========
const PYRIGHT_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "pyright-langserver",
        description: "npm global",
    },
    SdkPath {
        base: "~/.local/bin",
        relative: "pyright-langserver",
        description: "User local bin",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "pyright-langserver",
        description: "System local bin",
    },
    SdkPath {
        base: "/usr/bin",
        relative: "pyright-langserver",
        description: "System bin",
    },
];

/// Platform-specific SDK paths for TypeScript language server
const TYPESCRIPT_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "typescript-language-server",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "typescript-language-server",
        description: "System local",
    },
    SdkPath {
        base: "~/.volta/bin",
        relative: "typescript-language-server",
        description: "Volta",
    },
];

/// Get Vue/Volar SDK paths
const VOLAR_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "vue-language-server",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "vue-language-server",
        description: "System local",
    },
];

/// Get Svelte SDK paths
const SVELTE_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "svelteserver",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "svelteserver",
        description: "System local",
    },
];

/// Get HTML SDK paths
const HTML_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "vscode-html-language-server",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "vscode-html-language-server",
        description: "System local",
    },
];

/// Get SQL SDK paths
const SQL_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "sql-language-server",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "sql-language-server",
        description: "System local",
    },
];

/// Get GraphQL SDK paths
const GRAPHQL_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "graphql-lsp",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "graphql-lsp",
        description: "System local",
    },
];

/// Get Solidity SDK paths
const SOLIDITY_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.npm-global/bin",
        relative: "nomicfoundation-solidity-language-server",
        description: "npm global",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "nomicfoundation-solidity-language-server",
        description: "System local",
    },
];

// ========== Platform-specific SDK paths for JVM Languages ==========
const JAVA_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "/usr/share/java/jdtls/bin",
        relative: "jdtls",
        description: "System jdtls",
    },
    SdkPath {
        base: "~/jdtls/bin",
        relative: "jdtls",
        description: "User jdtls",
    },
    SdkPath {
        base: "~/.local/bin",
        relative: "jdtls",
        description: "User local bin",
    },
];

// JetBrains' Kotlin LSP: Homebrew links `kotlin-lsp` onto PATH, and its
// README has a manual install link it from `~/.local/bin`.
const KOTLIN_SDK_PATHS: &[SdkPath] = &[SdkPath {
    base: "~/.local/bin",
    relative: "kotlin-lsp",
    description: "User local bin",
}];

const SCALA_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.local/bin",
        relative: "metals",
        description: "Coursier installed",
    },
    SdkPath {
        base: "~/.cache/coursier/bin",
        relative: "metals",
        description: "Coursier cache",
    },
];

// ========== Platform-specific SDK paths for .NET Languages ==========
// One constant per server, with the binary's own name: a `relative: ""` would
// resolve to the tools *directory*, not the binary.
const CSHARP_LS_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.dotnet/tools",
        relative: "csharp-ls",
        description: "dotnet tools (csharp-ls)",
    },
    SdkPath {
        base: "/usr/share/dotnet/tools",
        relative: "csharp-ls",
        description: "System dotnet (csharp-ls)",
    },
];

const FSAUTOCOMPLETE_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.dotnet/tools",
        relative: "fsautocomplete",
        description: "dotnet tools (fsautocomplete)",
    },
    SdkPath {
        base: "/usr/share/dotnet/tools",
        relative: "fsautocomplete",
        description: "System dotnet (fsautocomplete)",
    },
];

// ========== Platform-specific SDK paths for Dart/Flutter ==========
const DART_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "/opt/dart-sdk/bin",
        relative: "dart",
        description: "System Dart SDK",
    },
    SdkPath {
        base: "~/flutter/bin/cache/dart-sdk/bin",
        relative: "dart",
        description: "Flutter embedded Dart SDK",
    },
    SdkPath {
        base: "~/development/flutter/bin/cache/dart-sdk/bin",
        relative: "dart",
        description: "Flutter dev directory",
    },
];

// ========== Platform-specific SDK paths for Swift ==========
const SWIFT_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "/usr/bin",
        relative: "sourcekit-lsp",
        description: "System sourcekit-lsp",
    },
    SdkPath {
        base: "/usr/local/bin",
        relative: "sourcekit-lsp",
        description: "Local sourcekit-lsp",
    },
    // swiftly's own directory, which only a shell that sourced its env.sh
    // has on PATH: `~/.local/share/swiftly` on Linux, `~/.swiftly` on macOS.
    SdkPath {
        base: "~/.local/share/swiftly/bin",
        relative: "sourcekit-lsp",
        description: "swiftly toolchain",
    },
    SdkPath {
        base: "~/.swiftly/bin",
        relative: "sourcekit-lsp",
        description: "swiftly toolchain",
    },
];

// ========== Platform-specific SDK paths for Ruby ==========
const RUBY_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/.gem/bin",
        relative: "ruby-lsp",
        description: "User gem bin",
    },
    SdkPath {
        base: "~/.rbenv/shims",
        relative: "ruby-lsp",
        description: "rbenv shims",
    },
    SdkPath {
        base: "~/.rvm/bin",
        relative: "ruby-lsp",
        description: "RVM bin",
    },
];

// ========== Platform-specific SDK paths for PHP ==========
const PHP_SDK_PATHS: &[SdkPath] = &[
    SdkPath {
        base: "~/bin",
        relative: "phpactor",
        description: "User bin",
    },
    SdkPath {
        base: "~/.local/bin",
        relative: "phpactor",
        description: "Local bin",
    },
];

/// Complete system detection registry for 21 languages
pub const SYSTEM_LSP_REGISTRY: &[SystemLspInfo] = &[
    // ========== Systems Languages ==========
    SystemLspInfo {
        server_id: "rust-analyzer",
        binary_names: &["rust-analyzer"],
        sdk_paths: RUST_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "clangd",
        binary_names: &[
            "clangd",
            "clangd-18",
            "clangd-17",
            "clangd-16",
            "clangd-15",
            "clangd-14",
        ],
        sdk_paths: CLANGD_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "gopls",
        binary_names: &["gopls"],
        sdk_paths: GO_SDK_PATHS,
        env_var: Some("GOPATH"),
        special_detector: None,
    },
    // ========== Python ==========
    SystemLspInfo {
        server_id: "pyright",
        binary_names: &["pyright-langserver", "pyright"],
        sdk_paths: PYRIGHT_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    // ========== Web Languages ==========
    SystemLspInfo {
        server_id: "typescript-language-server",
        binary_names: &["typescript-language-server", "tsserver"],
        sdk_paths: TYPESCRIPT_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "volar",
        binary_names: &["vue-language-server"],
        sdk_paths: VOLAR_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "svelte-language-server",
        binary_names: &["svelteserver", "svelte-language-server"],
        sdk_paths: SVELTE_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "html-languageserver",
        binary_names: &["vscode-html-language-server", "html-languageserver"],
        sdk_paths: HTML_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "sql-language-server",
        binary_names: &["sql-language-server"],
        sdk_paths: SQL_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "graphql-language-service",
        binary_names: &["graphql-lsp", "graphql-language-service-cli"],
        sdk_paths: GRAPHQL_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "nomicfoundation-solc",
        binary_names: &["nomicfoundation-solidity-language-server"],
        sdk_paths: SOLIDITY_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "bash-language-server",
        binary_names: &["bash-language-server"],
        sdk_paths: &[],
        env_var: None,
        special_detector: None,
    },
    // ========== JVM Languages ==========
    SystemLspInfo {
        server_id: "jdtls",
        binary_names: &["jdtls"],
        sdk_paths: JAVA_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "kotlin-lsp",
        binary_names: &["kotlin-lsp"],
        sdk_paths: KOTLIN_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "metals",
        binary_names: &["metals"],
        sdk_paths: SCALA_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    // ========== Microsoft Ecosystem ==========
    SystemLspInfo {
        server_id: "csharp-ls",
        binary_names: &["csharp-ls"],
        sdk_paths: CSHARP_LS_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "fsautocomplete",
        binary_names: &["fsautocomplete"],
        sdk_paths: FSAUTOCOMPLETE_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    // ========== Mobile Development ==========
    SystemLspInfo {
        server_id: "dart-ls",
        binary_names: &["dart"],
        sdk_paths: DART_SDK_PATHS,
        env_var: None,
        special_detector: Some(detect_flutter_dart),
    },
    SystemLspInfo {
        server_id: "sourcekit-lsp",
        binary_names: &["sourcekit-lsp"],
        sdk_paths: SWIFT_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    // ========== Functional Languages ==========

    // ========== Other Languages ==========
    SystemLspInfo {
        server_id: "ruby-lsp",
        binary_names: &["ruby-lsp"],
        sdk_paths: RUBY_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
    SystemLspInfo {
        server_id: "phpactor",
        binary_names: &["phpactor"],
        sdk_paths: PHP_SDK_PATHS,
        env_var: None,
        special_detector: None,
    },
];

/// Special detector for Flutter SDK Dart
fn detect_flutter_dart() -> Option<PathBuf> {
    // Check user's specific Flutter location first
    let home = dirs::home_dir()?;

    let flutter_paths = [
        home.join("develop/flutter/bin/dart"),
        home.join("development/flutter/bin/dart"),
        home.join("flutter/bin/dart"),
        PathBuf::from("/opt/flutter/bin/dart"),
    ];

    for path in &flutter_paths {
        if path.exists() {
            debug!("Found Flutter dart at: {}", path.display());
            return Some(path.clone());
        }
    }

    // Try to find via flutter command
    if let Ok(flutter_path) = which::which("flutter") {
        let dart_path = flutter_path
            .parent()?
            .join("cache")
            .join("dart-sdk")
            .join("bin")
            .join("dart");
        if dart_path.exists() {
            debug!(
                "Found Flutter dart via flutter command: {}",
                dart_path.display()
            );
            return Some(dart_path);
        }
    }

    None
}

/// Find system LSP info by server ID
#[must_use]
pub fn find_system_lsp(server_id: &str) -> Option<&'static SystemLspInfo> {
    SYSTEM_LSP_REGISTRY
        .iter()
        .find(|info| info.server_id == server_id)
}

/// Detect LSP server in system paths
pub fn detect_system_lsp(server_id: &str) -> Option<PathBuf> {
    let info = find_system_lsp(server_id)?;

    trace!("Detecting {} in system paths", server_id);

    // 1. Try special detector first
    if let Some(detector) = info.special_detector
        && let Some(path) = detector()
    {
        debug!(
            "Found {} via special detector: {}",
            server_id,
            path.display()
        );
        return Some(path);
    }

    // 2. Try PATH resolution with which
    for binary_name in info.binary_names {
        if let Ok(path) = which::which(binary_name) {
            debug!("Found {} in PATH: {}", server_id, path.display());
            return Some(path);
        }
    }

    // 3. Try SDK paths with platform-specific expansion
    let home = dirs::home_dir()?;

    for sdk_path in info.sdk_paths {
        let base_str = sdk_path.base;

        // Expand home directory
        let base = if base_str.starts_with('~') {
            home.join(&base_str[2..])
        } else {
            PathBuf::from(base_str)
        };

        let full_path = base.join(sdk_path.relative);

        if full_path.exists() {
            debug!(
                "Found {} at SDK path ({}): {}",
                server_id,
                sdk_path.description,
                full_path.display()
            );
            return Some(full_path);
        }
    }

    // 4. Try environment variable
    if let Some(env_var) = info.env_var
        && let Ok(value) = std::env::var(env_var)
    {
        // For GOPATH, check the bin subdirectory
        let path = if env_var == "GOPATH" {
            PathBuf::from(value).join("bin").join(info.binary_names[0])
        } else {
            PathBuf::from(value)
        };

        if path.exists() {
            debug!("Found {} via {}: {}", server_id, env_var, path.display());
            return Some(path);
        }
    }

    trace!("Could not find {} in system paths", server_id);
    None
}

/// Check if a server is available in system paths
#[must_use]
pub fn is_system_server_available(server_id: &str) -> bool {
    detect_system_lsp(server_id).is_some()
}

/// Get all available system LSP servers
#[must_use]
pub fn scan_all_system_servers() -> Vec<(&'static SystemLspInfo, PathBuf)> {
    let mut found = Vec::new();

    for info in SYSTEM_LSP_REGISTRY {
        if let Some(path) = detect_system_lsp(info.server_id) {
            found.push((info, path));
        }
    }

    found
}

/// Get all system servers for a specific binary name (reverse lookup)
#[must_use]
pub fn find_servers_by_binary(binary_name: &str) -> Vec<&'static SystemLspInfo> {
    SYSTEM_LSP_REGISTRY
        .iter()
        .filter(|info| info.binary_names.contains(&binary_name))
        .collect()
}

/// Expand a path that may contain `~` to the full home directory path.
///
/// Handles:
/// - `~/foo` → current user's home + `/foo`
/// - `~user/foo` → falls back to current user's home + `/foo` (cross-platform
///   user lookup is not provided by the `dirs` crate; for full `~user` support
///   consider `shellexpand`)
/// - `/foo/bar` → returned as-is
///
/// Returns `None` only when the home directory cannot be determined and the
/// path starts with `~`.
#[must_use]
pub fn expand_home(path: &str) -> Option<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().map(|home| home.join(rest))
    } else if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() {
            dirs::home_dir()
        } else if let Some(slash_idx) = rest.find('/') {
            let _username = &rest[..slash_idx];
            let remainder = &rest[slash_idx + 1..];
            // Cross-platform resolution of arbitrary user homes is complex.
            // Fall back to current user's home with the remainder path.
            dirs::home_dir().map(|home| home.join(remainder))
        } else {
            dirs::home_dir().map(|home| home.join(rest))
        }
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_system_lsp() {
        let info = find_system_lsp("rust-analyzer");
        assert!(info.is_some());
        assert_eq!(info.unwrap().server_id, "rust-analyzer");
    }

    #[test]
    fn test_expand_home() {
        let expanded = expand_home("~/.cargo/bin");
        assert!(expanded.is_some());
        assert!(!expanded.unwrap().to_string_lossy().contains('~'));
    }

    #[test]
    fn test_all_servers_have_binary_names() {
        for info in SYSTEM_LSP_REGISTRY {
            assert!(
                !info.binary_names.is_empty(),
                "{} has no binary names",
                info.server_id
            );
        }
    }

    #[test]
    fn test_dart_has_flutter_detection() {
        let info = find_system_lsp("dart-ls").unwrap();
        assert!(info.special_detector.is_some());
    }
}
