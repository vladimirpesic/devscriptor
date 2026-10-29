//! Language server presets for 21 programming languages.
//!
//! This module provides pre-configured LSP server configurations for
//! a wide variety of programming languages.

use crate::config::{LspServerConfig, ServerHeuristics, ServerSettings};
use std::collections::HashMap;

macro_rules! preset {
    (
        $fn:ident,
        $lang:expr,
        $cmd:expr,
        [$($arg:expr),*],
        [$($ext:expr),*],
        [$($marker:expr),*],
        $max_depth:expr,
        $init:expr
    ) => {
        #[must_use]
        pub fn $fn() -> LspServerConfig {
            LspServerConfig {
                language_id: ($lang).into(),
                command: ($cmd).into(),
                args: vec![$($arg.into()),*],
                env: HashMap::new(),
                working_dir: None,
                extensions: vec![$($ext.into()),*],
                heuristics: Some(ServerHeuristics {
                    markers: vec![$($marker.into()),*],
                    max_depth: $max_depth,
                }),
                initialization_options: $init,
                settings: ServerSettings::default(),
                configuration: None,
            }
        }
    };

    (
        $fn:ident,
        $lang:expr,
        $cmd:expr,
        [$($arg:expr),*],
        [$($ext:expr),*],
        _no_heuristics_,
        $init:expr
    ) => {
        #[must_use]
        pub fn $fn() -> LspServerConfig {
            LspServerConfig {
                language_id: ($lang).into(),
                command: ($cmd).into(),
                args: vec![$($arg.into()),*],
                env: HashMap::new(),
                working_dir: None,
                extensions: vec![$($ext.into()),*],
                heuristics: None,
                initialization_options: $init,
                settings: ServerSettings::default(),
                configuration: None,
            }
        }
    };
}

// Systems languages

preset!(
    rust_analyzer,
    "rust",
    "rust-analyzer",
    [],
    ["rs"],
    ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"],
    3,
    // `targetDir: true`: build in `target/rust-analyzer`, off the user's
    // `cargo build` lock.
    Some(serde_json::json!({
        "cargo": { "features": "all", "targetDir": true },
        "checkOnSave": { "command": "clippy" }
    }))
);

preset!(
    clangd,
    "cpp",
    "clangd",
    ["--background-index"],
    ["c", "cpp", "cc", "cxx", "h", "hpp", "cu", "cuh", "mm", "m"],
    [
        "compile_commands.json",
        "compile_flags.txt",
        ".clangd",
        "CMakeLists.txt",
        "Makefile"
    ],
    3,
    None
);

// Web languages

preset!(
    typescript,
    "typescript",
    "typescript-language-server",
    ["--stdio"],
    ["ts", "tsx", "js", "jsx", "mjs", "cjs"],
    ["package.json", "tsconfig.json", "jsconfig.json"],
    3,
    None
);

preset!(
    volar,
    "vue",
    "vue-language-server",
    ["--stdio"],
    ["vue"],
    ["vite.config.ts", "nuxt.config.ts"],
    2,
    None
);

preset!(
    svelte_ls,
    "svelte",
    "svelte-language-server",
    ["--stdio"],
    ["svelte"],
    ["svelte.config.js"],
    2,
    None
);

preset!(
    html_ls,
    "html",
    "vscode-html-language-server",
    ["--stdio"],
    ["html", "htm"],
    _no_heuristics_,
    None
);

preset!(
    graphql_ls,
    "graphql",
    "graphql-lsp",
    ["server", "--method=stream"],
    ["graphql", "gql"],
    [".graphqlrc", ".graphqlrc.yml"],
    2,
    None
);

// Python

preset!(
    pyright,
    "python",
    "pyright-langserver",
    ["--stdio"],
    ["py", "pyw", "pyi"],
    [
        "pyproject.toml",
        "setup.py",
        "requirements.txt",
        "setup.cfg",
        "Pipfile"
    ],
    3,
    Some(serde_json::json!({
        "settings": { "python": { "analysis": { "typeCheckingMode": "basic" } } }
    }))
);

// JVM languages

// The property keeps jdtls's `.project`, `.classpath` and `.settings/` out of
// the project, in its own workspace.
preset!(
    jdtls,
    "java",
    "jdtls",
    ["--jvm-arg=-Djava.import.generatesMetadataFilesAtProjectRoot=false"],
    ["java"],
    ["pom.xml", "build.gradle", ".classpath"],
    3,
    Some(serde_json::json!({
        "bundles": [],
        "workspaceFolders": ["file://"]
    }))
);

preset!(
    kotlin_lsp,
    "kotlin",
    "kotlin-lsp",
    ["--stdio"],
    ["kt", "kts"],
    [
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
        "pom.xml"
    ],
    3,
    None
);

preset!(
    scala_metals,
    "scala",
    "metals",
    [],
    ["scala", "sc"],
    ["build.sbt"],
    3,
    Some(serde_json::json!({"isHttpEnabled": false}))
);

// .NET

preset!(
    csharp_ls,
    "csharp",
    "csharp-ls",
    [],
    ["cs"],
    ["*.csproj", "*.sln"],
    3,
    None
);

preset!(
    fsautocomplete,
    "fsharp",
    "fsautocomplete",
    ["--background-service-enabled"],
    ["fs", "fsx", "fsi"],
    ["*.fsproj", "*.sln"],
    3,
    None
);

// Go

preset!(
    gopls,
    "go",
    "gopls",
    [],
    ["go"],
    ["go.mod", "go.sum"],
    3,
    Some(serde_json::json!({
        "ui.diagnostic.annotations": {
            "bounds": true,
            "escape": true,
            "inline": true,
            "nil": true
        }
    }))
);

// Ruby

preset!(
    ruby_lsp,
    "ruby",
    "ruby-lsp",
    [],
    ["rb", "erb"],
    ["Gemfile", "*.gemspec"],
    3,
    None
);

// PHP

preset!(
    phpactor,
    "php",
    "phpactor",
    ["language-server"],
    ["php"],
    ["composer.json"],
    3,
    None
);

// Swift

preset!(
    sourcekit_lsp,
    "swift",
    "sourcekit-lsp",
    [],
    ["swift"],
    ["Package.swift"],
    3,
    None
);

// Shell

preset!(
    bash_ls,
    "bash",
    "bash-language-server",
    ["start"],
    ["sh", "bash", "zsh"],
    _no_heuristics_,
    None
);

// SQL

preset!(
    sqlls,
    "sql",
    "sql-language-server",
    ["up", "--method", "stdio"],
    ["sql"],
    _no_heuristics_,
    None
);

// Dart

preset!(
    dart_ls,
    "dart",
    "dart",
    ["language-server", "--protocol=lsp"],
    ["dart"],
    ["pubspec.yaml"],
    3,
    None
);

// Solidity

preset!(
    nomicfoundation_solidity_ls,
    "solidity",
    "nomicfoundation-solidity-language-server",
    ["--stdio"],
    ["sol"],
    [
        "hardhat.config.js",
        "hardhat.config.ts",
        "foundry.toml",
        "remappings.txt",
        "package.json"
    ],
    3,
    Some(serde_json::json!({
        "extensionName": "vscode-solidity",
        "env": "production"
    }))
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_analyzer_config() {
        let c = rust_analyzer();
        assert_eq!(c.language_id, "rust");
        assert_eq!(c.command, "rust-analyzer");
        assert!(c.extensions.contains(&"rs".into()));
        assert_eq!(
            c.initialization_options.unwrap()["cargo"]["targetDir"],
            serde_json::Value::Bool(true)
        );
    }

    #[test]
    fn test_typescript_config() {
        let c = typescript();
        assert_eq!(c.language_id, "typescript");
        assert_eq!(c.command, "typescript-language-server");
        assert!(c.args.contains(&"--stdio".into()));
    }

    #[test]
    fn test_pyright_config() {
        let c = pyright();
        assert_eq!(c.language_id, "python");
        assert_eq!(c.command, "pyright-langserver");
    }
}
