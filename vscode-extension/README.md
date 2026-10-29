# Devscriptor VS Code Extension

Brings Devscriptor into Visual Studio Code: scan the workspace into a Devscriptor database,
search it, and run code analysis. The extension asks the workspace's shared Devscriptor
process, the one the AI agents working in the folder use too, and runs the CLI when it
cannot reach it.

> **Status: preview (0.1.0), not published.** There is no Marketplace listing and no
> `.vsix` download; build it from source (see [Installation](#installation)).

## Requirements

- VS Code 1.74.0 or higher
- The Devscriptor CLI (see [Installation](../docs/installation.md)). The extension uses the
  `devscriptor.binaryPath` setting; otherwise the first of `~/.local/bin`, `~/.cargo/bin`,
  `/usr/local/bin` and `/usr/bin` that holds `devscriptor`; otherwise the one on `PATH`.

## Installation

Build the `.vsix` and install it:

```bash
cd vscode-extension
npm install
npm run package          # vsce package → devscriptor-0.1.0.vsix
code --install-extension devscriptor-0.1.0.vsix
```

## Usage

When a workspace opens, the extension scans it (unless `devscriptor.autoScan` is off) into the
database at `devscriptor.databasePath`.

### Commands

Open the Command Palette (Ctrl+Shift+P / Cmd+Shift+P) and type `Devscriptor:`. Each command calls
a tool of the workspace's Devscriptor; when that cannot be reached, it runs the CLI command
instead (see [The shared Devscriptor](#the-shared-devscriptor)):

| Command | MCP tool | CLI fallback |
| --- | --- | --- |
| `Scan Workspace` | `scan_codebase` | `devscriptor scan <workspace>` |
| `Search` | `semantic_search` (20 hits); `search_entities` while `devscriptor.enableSemanticSearch` is off | `devscriptor search <query> --search-type semantic --limit 20 --output json` (`--search-type symbol` while it is off) |
| `Find Dead Code` | `find_dead_code`, every finding of confidence 0.5 or more | `devscriptor analyze --kind dead-code --output json` (severity medium or more) |
| `Analyze Complexity` | `analyze_complexity`, every complex function | `devscriptor analyze --kind complexity --output json` |
| `Show Call Graph` | `get_relationships`: the symbol's relationships | `devscriptor call-graph --entity <symbol> --format json`, to three steps away |
| `Go to Definition (Enhanced)` | `get_definition` | `devscriptor definition <file> --line … --character …` |
| `Find References (Enhanced)` | `get_references` | `devscriptor references <file> --line … --character …` |
| `Connect to the Shared Devscriptor` / `Disconnect from the Shared Devscriptor` | Opens or closes the extension's connection | — |
| `Check for Devscriptor Updates` | — | `devscriptor --version`, compared with the newest release (see [Updating](#updating)) |
| `Show Status` | `get_statistics` for the entity count | `devscriptor stats`; the version from `devscriptor --version` |

The connection and the CLI commands, `--version` aside, name the database
(`--database <db>`) and run in the first workspace folder. `Go to Definition (Enhanced)` and
`Find References (Enhanced)` are hidden while `devscriptor.lsp.enabled` is off.

### Keyboard Shortcuts

The bindings use Ctrl on every platform, macOS included:

- `Ctrl+Shift+S` — Search (the same keys as VS Code's *File: Save As* on Linux and Windows)
- `Ctrl+Alt+G` — Go to Definition (Enhanced), while `devscriptor.lsp.enabled` is on

### Context Menu

With text selected in the editor, the context menu offers Go to Definition (Enhanced) and Find
References (Enhanced), while `devscriptor.lsp.enabled` is on.

## Configuration

Settings (File > Preferences > Settings, search for "devscriptor"):

| Setting | Description | Default |
| --- | --- | --- |
| `devscriptor.binaryPath` | Path to the devscriptor binary | Auto-detected |
| `devscriptor.databasePath` | Analysis database, relative to the first workspace folder unless absolute | `devscriptor.db` |
| `devscriptor.autoScan` | Scan the workspace when it opens | `true` |
| `devscriptor.enableSemanticSearch` | `Search` finds code by meaning, with Devscriptor's embedding model (downloaded on first use, ~150 MB); off, it finds code by name, without the model | `true` |
| `devscriptor.mcpServer.enabled` | Offer the workspace's Devscriptor to VS Code's AI assistants (see [AI assistants in VS Code](#ai-assistants-in-vs-code)) | `false` |
| `devscriptor.lsp.enabled` | `Go to Definition (Enhanced)` and `Find References (Enhanced)`; off, they are hidden | `true` |

A change takes effect at the next command, without a reload; `devscriptor.autoScan` acts when a
workspace opens.

What a scan takes in is the project's to say: `scan.include` and `scan.exclude` in its
`devscriptor.yaml` (see [Configuration](../docs/configuration.md)), which the workspace's
shared Devscriptor, its watcher and every other client follow too. Devscriptor's own excludes
(`node_modules`, `.git`, `target`, `dist`, `build`, …) apply in any case.

## The shared Devscriptor

The extension keeps one MCP connection, made on its first command: it starts
`devscriptor mcp --database <db>` in the first workspace folder, and speaks JSON-RPC to it over
stdin and stdout. `devscriptor mcp` relays to the workspace's Devscriptor process,
`devscriptor serve`, and starts it when none runs. Every client started in that folder with
that database is served by the same process, with one index, watcher, embedding model and set of
language servers (see [MCP server](../docs/mcp.md#one-server-per-workspace)), so a language
server another client started answers the extension at once.

The default database, `devscriptor.db` in the workspace folder, is the one `devscriptor scan`
writes. An agent shares the extension's process when its MCP client starts
`devscriptor mcp --database devscriptor.db` in the folder; one started with plain
`devscriptor mcp` uses the shared default database, and a process of its own.

When the connection cannot be made, or breaks, the command runs the CLI instead, and the
**Devscriptor** output channel says why. That channel also shows what `devscriptor mcp` and the
CLI write to stderr; the shared process logs to its own file (see
[MCP server](../docs/mcp.md#runtime-directory-and-logs)). An error of the tool
itself, such as no language server for the file, is shown as the command's error.

`Disconnect from the Shared Devscriptor` closes the extension's connection; the shared process
goes on serving its other clients, and stops 10 minutes after its last one leaves. The next
command connects again.

## AI assistants in VS Code

With `devscriptor.mcpServer.enabled` on, the extension offers the workspace's Devscriptor to VS
Code's AI assistants, such as agent mode in chat, as the MCP server **Devscriptor**:
`devscriptor mcp --database <db>` in the first workspace folder. It shares the workspace's
Devscriptor process with the extension's commands, and with the other agents in the folder that
use that database, so they all use one index, watcher and set of language servers. VS Code lists it with its other MCP
servers (*MCP: List Servers*); after an update of Devscriptor, VS Code reads its tools again.

It needs VS Code 1.101 or newer. Turned on where the server cannot be offered, in an older VS
Code, with no workspace folder open or no Devscriptor found, the setting says why.

## Updating

`Check for Devscriptor Updates` compares the installed Devscriptor (`devscriptor --version`) with
the newest release, which the latest GitHub release's `latest.json` names, and offers the
[release page](https://github.com/vladimirpesic/devscriptor/releases/latest). It reaches the network only when you
run it. Devscriptor does not update itself: install the new release over the old one
([Installation](../docs/installation.md#updating)); the next connection replaces a shared process
of the old version.

## Troubleshooting

### "Devscriptor binary not found"

1. Check that `devscriptor --version` works in a terminal
2. Or set `devscriptor.binaryPath` to the binary

### "Database not found"

Run "Devscriptor: Scan Workspace" first to create the analysis database.

### Semantic search

The first semantic query downloads the embedding model (~150 MB) from Hugging Face, with the
Devscriptor mirror as a fallback, and caches it for offline use. It needs an internet connection
the first time and about 256 MB of free disk space. With `devscriptor.enableSemanticSearch` off,
`Search` finds code by name and never loads the model.

## Development

```bash
cd vscode-extension
npm install
npm run compile   # or: npm run watch
npm run test:unit # the tests that need no VS Code: parsers, the MCP client, the shared process
npm test          # every test, in a VS Code it downloads (needs a display; Xvfb on CI)
```

`npm run test:unit` checks the MCP client against a stand-in server. With `DEVSCRIPTOR_BIN`
naming a `devscriptor` binary, it also checks that the extension and another client share one
Devscriptor process; the test runs in a temporary directory and stops the process it started.

Press F5 in VS Code to run the extension in an Extension Development Host.

## License

Licensed under the [Apache License, Version 2.0](../LICENSE).
