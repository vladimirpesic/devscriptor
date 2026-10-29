# The LSP architecture module

## Overview

The LSP Architecture module finds the language servers a project needs, reusing what is already installed and falling back to on-demand installation.

## Four-Tier Resolution Strategy

`LspResolver::resolve` checks four sources in priority order and returns the first match. If none match it returns `None`, and the caller can install the server on demand.

```plaintext
┌──────────────────────────────────────────────────────────────┐
│                     LSP Resolution Flow                        │
└───────────────────────────────┬──────────────────────────────┘
                                │
                  ┌─────────────▼────────────────┐
                  │  Tier 1: User Config          │
                  │  UserLspConfig path overrides │
                  └─────────────┬────────────────┘
                                │ not configured
                  ┌─────────────▼────────────────┐
                  │  Tier 2: Devscriptor-managed  │
                  │  ~/.cache/devscriptor/        │
                  │    lsp-servers/  (installed)  │
                  │  then bundle/lsp-servers/     │
                  └─────────────┬────────────────┘
                                │ not found
                  ┌─────────────▼────────────────┐
                  │  Tier 3: System-Wide          │
                  │  $(which) + known SDK paths   │
                  └─────────────┬────────────────┘
                                │ not found
                  ┌─────────────▼────────────────┐
                  │  Tier 4: VS Code extensions   │
                  │  ~/.vscode/extensions/        │
                  └─────────────┬────────────────┘
                                │ still not found → None
                  ┌─────────────▼────────────────┐
                  │  Install on demand            │
                  │  (mirror → package manager)   │
                  └───────────────────────────────┘
```

`LspSource` records which tier resolved a server: `UserConfig`, `Installed`, `Bundled`, `SystemWide`, or `VsCodeExtension`.

Only server ids in `generated_registry::SERVER_IDS` are resolved; any other id is rejected before a path is built.

### Tier 1 and the LSP config file

Tier 1 is a `UserLspConfig` (per-server `path`, `args`, `env` and `auto_detect`, plus switches for system and VS Code discovery and `extra_paths`), set with `LspResolver::with_user_config` or `LspArchitectureManager::with_user_config`. No file feeds it in the shipped commands, so in practice resolution starts at tier 2.

Users configure servers through a different mechanism: the file `DEVSCRIPTOR_LSP_CONFIG` names, or a `devscriptor-lsp.toml` in the working directory once trusted with `devscriptor lsp trust` (`config::trust`; an untrusted or since-edited file is ignored, since it names commands to run). When present, it replaces the resolved catalog for the LSP tools; see [Language servers](../../../../docs/lsp.md#configuration-file).

## Components

### 1. Resolution Engine (`resolver.rs`)

`LspResolver` walks the tiers and builds each resolved server's `LspServerConfig` from `manager/registry.json` (arguments, environment, initialization options, timeout) and `generated_registry.rs` (language id, extensions, project markers). It also holds `UserLspConfig`.

### 2. VS Code Extension Mapper (`vscode.rs`)

Maps server ids to VS Code extension directories and the server binary inside them, for 18 of the 21 servers. pyright, sql-language-server and sourcekit-lsp have no mapping.

### 3. System Detector (`system.rs`)

Detects system-wide installations with `which` on the server's binary name, then known SDK paths (rustup, the Dart SDK, …).

### 4. Architecture Manager (`manager.rs`)

`LspArchitectureManager` orchestrates the rest:

- `analyze_project` detects a project's languages from file extensions and marker files, resolves a server for each, and lists the servers that still need installing, with their prerequisite check
- resolution results are cached for the manager's lifetime (`clear_cache` empties the cache)
- `get_prerequisites` / `can_auto_install` report whether a server can be installed here
- `get_server_status` / `get_all_available_servers` report where servers resolve

### 5. Prerequisites (`prerequisites.rs`)

`get_prerequisites` lists the runtimes a server needs, and `check_prerequisites` tests them:

```rust
enum Prerequisite {
    BuildTool { name, command, min_version },          // Node.js, Go, rustup, Python
    Sdk { name, command, env_var, min_version },       // Java, .NET SDK, Ruby, PHP
    PackageManager { name, command },                  // npm, gem
    Custom { name, check_command, install_instructions, install_url },  // the Swift toolchain
}
```

## Server Coverage (21 Servers)

| Server ID | Languages | Prerequisites | VS Code extension |
| --- | --- | --- | --- |
| rust-analyzer | Rust | rustup | rust-lang.rust-analyzer |
| clangd | C, C++, CUDA, Objective-C, Objective-C++ | — | llvm-vs-code-extensions.vscode-clangd |
| gopls | Go | Go | golang.go |
| typescript-language-server | TypeScript, JavaScript | Node.js ≥ 18, npm | vscode.typescript-language-features |
| volar | Vue | Node.js ≥ 18, npm | vue.volar |
| svelte-language-server | Svelte | Node.js ≥ 18, npm | svelte.svelte-vscode |
| html-languageserver | HTML | Node.js ≥ 18, npm | vscode.html-language-features |
| graphql-language-service | GraphQL | Node.js ≥ 18, npm | graphql.vscode-graphql |
| sql-language-server | SQL | Node.js ≥ 18, npm | — |
| bash-language-server | Bash | Node.js ≥ 18, npm | mads-hartmann.bash-ide-vscode |
| nomicfoundation-solc | Solidity | Node.js ≥ 18, npm | nomicfoundation.hardhat-solidity |
| pyright | Python | Node.js | — |
| jdtls | Java | Java 21+, Python 3 | redhat.java |
| kotlin-lsp | Kotlin | — (bundles its Java runtime) | jetbrains.kotlin-server |
| metals | Scala | Java | scala-lang.scala |
| csharp-ls | C# | .NET SDK | csharp-ls.csharp-ls |
| fsautocomplete | F# | .NET SDK | ionide.ionide-fsharp |
| dart-ls | Dart | — (the install is the Dart SDK) | dart-code.dart-code |
| sourcekit-lsp | Swift | Swift toolchain | — |
| ruby-lsp | Ruby | Ruby, gem | shopify.ruby-lsp |
| phpactor | PHP | PHP | phpactor.vscode-phpactor |

The prerequisites (`prerequisites.rs`) are what `lsp check`, `lsp setup` and the install itself test for: what a server needs on the host to run. dart-ls's install is the Dart SDK itself, and the Kotlin LSP's archive bundles its Java runtime, so neither needs anything. Objective-C++ (`.mm`) files go to clangd, and so do Objective-C `.m` files, told from MATLAB's by their content (`languages::is_objective_c`: `@interface`, `@implementation` or `#import` in the first 4 KiB). VB.NET files have no server: csharp-ls serves C# only.

## Setup Workflow

`devscriptor lsp setup [PATH]` and the `lsp_arch_setup` MCP tool run the same steps:

1. `analyze_project` detects the languages and resolves their servers through the four tiers.
2. For each server that did not resolve, the prerequisites are checked.
3. With approval (`--yes` on the CLI, `auto_install: true` over MCP), servers whose prerequisites are met are installed into `~/.cache/devscriptor/lsp-servers/<id>/` (`manager/downloader.rs`). The mirror is tried first, checked against its SHA-256 manifest. Without a mirror build, npm, `go install`, `dotnet tool`, gem and Coursier servers install through their package manager, GitHub-release servers from the release asset, checked against the SHA-256 `registry.json` pins for a pinned release or else the one GitHub publishes for it, and the direct downloads (jdtls, the Kotlin LSP, the Dart SDK) from their publishers at the pinned version, checked against the SHA-256 `registry.json` pins for the platform's build. Anything that cannot be verified is refused. `system` servers (sourcekit-lsp) are never installed. A server whose install fails leaves no directory behind and keeps none of the others from installing.
4. Servers with missing prerequisites are reported with install instructions.

## See Also

- [Language servers](../../../../docs/lsp.md) — detection, installation, lifecycle and configuration
- [Language server prerequisites](../../../../docs/lsp-servers.md) — what each server needs to run
- [Architecture](../../../../docs/architecture.md) — the system as a whole
- [MCP server](../../../../docs/mcp.md) — every MCP tool
