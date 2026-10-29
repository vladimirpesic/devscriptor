//! The servers' ids, languages, extensions and project markers, kept in step with
//! `manager/registry.json` (`registry::tests::generated_registry_matches_registry_json`).

use crate::config::ServerHeuristics;

/// All known server IDs, ordered as in the registry.
pub const SERVER_IDS: &[&str] = &[
    "rust-analyzer",
    "clangd",
    "gopls",
    "typescript-language-server",
    "volar",
    "svelte-language-server",
    "html-languageserver",
    "graphql-language-service",
    "pyright",
    "sql-language-server",
    "jdtls",
    "kotlin-lsp",
    "metals",
    "csharp-ls",
    "fsautocomplete",
    "sourcekit-lsp",
    "dart-ls",
    "ruby-lsp",
    "bash-language-server",
    "nomicfoundation-solc",
    "phpactor",
];

/// Language ID for a given server ID.
#[must_use]
pub fn language_id(server_id: &str) -> Option<&'static str> {
    Some(match server_id {
        "rust-analyzer" => "rust",
        "clangd" => "cpp",
        "gopls" => "go",
        "typescript-language-server" => "typescript",
        "volar" => "vue",
        "svelte-language-server" => "svelte",
        "html-languageserver" => "html",
        "graphql-language-service" => "graphql",
        "pyright" => "python",
        "sql-language-server" => "sql",
        "jdtls" => "java",
        "kotlin-lsp" => "kotlin",
        "metals" => "scala",
        "csharp-ls" => "csharp",
        "fsautocomplete" => "fsharp",
        "sourcekit-lsp" => "swift",
        "dart-ls" => "dart",
        "ruby-lsp" => "ruby",
        "bash-language-server" => "bash",
        "nomicfoundation-solc" => "solidity",
        "phpactor" => "php",
        _ => return None,
    })
}

/// File extensions for a given server ID.
#[must_use]
pub fn extensions(server_id: &str) -> Option<&'static [&'static str]> {
    Some(match server_id {
        "rust-analyzer" => &["rs"],
        "clangd" => &["c", "cpp", "cc", "cxx", "h", "hpp", "cu", "cuh", "mm", "m"],
        "gopls" => &["go"],
        "typescript-language-server" => &["ts", "tsx", "js", "jsx"],
        "volar" => &["vue"],
        "svelte-language-server" => &["svelte"],
        "html-languageserver" => &["html", "htm"],
        "graphql-language-service" => &["graphql", "gql"],
        "pyright" => &["py", "pyw", "pyi"],
        "sql-language-server" => &["sql"],
        "jdtls" => &["java"],
        "kotlin-lsp" => &["kt", "kts"],
        "metals" => &["scala", "sc"],
        "csharp-ls" => &["cs"],
        "fsautocomplete" => &["fs", "fsx", "fsi"],
        "sourcekit-lsp" => &["swift"],
        "dart-ls" => &["dart"],
        "ruby-lsp" => &["rb", "erb"],
        "bash-language-server" => &["sh", "bash", "zsh"],
        "nomicfoundation-solc" => &["sol"],
        "phpactor" => &["php"],
        _ => return None,
    })
}

/// Heuristics for a given server ID.
#[must_use]
pub fn heuristics(server_id: &str) -> Option<ServerHeuristics> {
    Some(match server_id {
        "rust-analyzer" => ServerHeuristics {
            markers: vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()],
            max_depth: 3,
        },
        "clangd" => ServerHeuristics {
            markers: vec![
                "compile_commands.json".to_string(),
                "compile_flags.txt".to_string(),
                ".clangd".to_string(),
                "CMakeLists.txt".to_string(),
                "Makefile".to_string(),
            ],
            max_depth: 3,
        },
        "gopls" => ServerHeuristics {
            markers: vec!["go.mod".to_string(), "go.sum".to_string()],
            max_depth: 3,
        },
        "typescript-language-server" => ServerHeuristics {
            markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
            max_depth: 3,
        },
        "volar" => ServerHeuristics {
            markers: vec!["vite.config.ts".to_string(), "nuxt.config.ts".to_string()],
            max_depth: 2,
        },
        "svelte-language-server" => ServerHeuristics {
            markers: vec!["svelte.config.js".to_string()],
            max_depth: 2,
        },
        "html-languageserver" => return None,
        // Every name graphql-config looks for, `.graphqlconfig*` included, which
        // graphql-language-service loads with `legacy: true`. A `graphql` key in
        // `package.json` also works, but `package.json` alone says nothing.
        "graphql-language-service" => ServerHeuristics {
            markers: vec![
                "graphql.config.ts".to_string(),
                "graphql.config.cts".to_string(),
                "graphql.config.mts".to_string(),
                "graphql.config.js".to_string(),
                "graphql.config.cjs".to_string(),
                "graphql.config.mjs".to_string(),
                "graphql.config.json".to_string(),
                "graphql.config.yaml".to_string(),
                "graphql.config.yml".to_string(),
                "graphql.config.toml".to_string(),
                ".graphqlrc".to_string(),
                ".graphqlrc.ts".to_string(),
                ".graphqlrc.cts".to_string(),
                ".graphqlrc.mts".to_string(),
                ".graphqlrc.js".to_string(),
                ".graphqlrc.cjs".to_string(),
                ".graphqlrc.mjs".to_string(),
                ".graphqlrc.json".to_string(),
                ".graphqlrc.yml".to_string(),
                ".graphqlrc.yaml".to_string(),
                ".graphqlrc.toml".to_string(),
                ".graphqlconfig".to_string(),
                ".graphqlconfig.json".to_string(),
                ".graphqlconfig.yaml".to_string(),
                ".graphqlconfig.yml".to_string(),
            ],
            max_depth: 2,
        },
        "pyright" => ServerHeuristics {
            markers: vec!["pyproject.toml".to_string(), "requirements.txt".to_string()],
            max_depth: 3,
        },
        "sql-language-server" => return None,
        "jdtls" => ServerHeuristics {
            markers: vec!["pom.xml".to_string(), "build.gradle".to_string()],
            max_depth: 3,
        },
        "kotlin-lsp" => ServerHeuristics {
            markers: vec![
                "settings.gradle.kts".to_string(),
                "settings.gradle".to_string(),
                "build.gradle.kts".to_string(),
                "build.gradle".to_string(),
                "pom.xml".to_string(),
            ],
            max_depth: 3,
        },
        "metals" => ServerHeuristics {
            markers: vec!["build.sbt".to_string()],
            max_depth: 3,
        },
        "csharp-ls" => ServerHeuristics {
            markers: vec!["*.csproj".to_string(), "*.sln".to_string()],
            max_depth: 3,
        },
        "fsautocomplete" => ServerHeuristics {
            markers: vec!["*.fsproj".to_string(), "*.sln".to_string()],
            max_depth: 3,
        },
        "sourcekit-lsp" => ServerHeuristics {
            markers: vec!["Package.swift".to_string()],
            max_depth: 3,
        },
        "dart-ls" => ServerHeuristics {
            markers: vec!["pubspec.yaml".to_string()],
            max_depth: 3,
        },
        "ruby-lsp" => ServerHeuristics {
            markers: vec!["Gemfile".to_string()],
            max_depth: 3,
        },
        "bash-language-server" => return None,
        "nomicfoundation-solc" => ServerHeuristics {
            markers: vec!["hardhat.config.js".to_string(), "foundry.toml".to_string()],
            max_depth: 3,
        },
        "phpactor" => ServerHeuristics {
            markers: vec!["composer.json".to_string()],
            max_depth: 3,
        },
        _ => return None,
    })
}
