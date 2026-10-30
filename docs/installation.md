# Installation

- [Requirements](#requirements)
- [Installing a release](#installing-a-release)
- [Building from source](#building-from-source)
- [Verifying the install](#verifying-the-install)
- [First steps](#first-steps)
- [Language servers](#language-servers)
- [Connecting an AI agent](#connecting-an-ai-agent)
- [Updating](#updating)
- [Uninstalling](#uninstalling)
- [Troubleshooting](#troubleshooting)

## Requirements

Devscriptor is one self-contained executable, `devscriptor`, holding the CLI, the
language-server integration, the Context Graph and the MCP server. TLS is rustls,
so it does not depend on the system's OpenSSL, and there are no other packages to
install.

| Platform | Requirement |
| --- | --- |
| Linux x86_64 / aarch64 | glibc 2.17 or newer (Ubuntu 14.04+, Debian 8+, RHEL/CentOS 7+) |
| macOS | macOS 11 Big Sur or newer, Apple Silicon (aarch64) or Intel (x86_64) |
| Windows | Windows 10 or 11, 64-bit (x86_64) |

| Resource | Minimum | Recommended |
| --- | --- | --- |
| Free RAM, for embedding | 4 GB | 8 GB |
| CPU cores | 2 | 4+ |
| Disk, for the embedding model | 256 MB | 512 MB |

`devscriptor scan` never loads the embedding model and runs on any hardware: keyword
search and every structural query work right after it. Only embedding needs the
memory above: the model takes about 1.3 GB while it runs (see
[Performance](performance.md)). `scan --embed` and `watch --embed` skip embedding
when less than 4 GB is free; `devscriptor embed` runs anyway, more slowly.

## Installing a release

Each release has an archive per platform, on its
[GitHub release page](https://github.com/vladimirpesic/devscriptor/releases/latest):
`devscriptor-<version>-<target>.tar.gz` (`.zip` on Windows), holding a directory
`devscriptor-<version>-<target>/` with the one executable, and a `.sha256` beside
it.

| Platform | Target |
| --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

### Linux and macOS

```bash
VERSION=1.0.1
TARGET=x86_64-unknown-linux-gnu       # see the table above
curl -LO "https://github.com/vladimirpesic/devscriptor/releases/download/v${VERSION}/devscriptor-${VERSION}-${TARGET}.tar.gz"
curl -LO "https://github.com/vladimirpesic/devscriptor/releases/download/v${VERSION}/devscriptor-${VERSION}-${TARGET}.tar.gz.sha256"
sha256sum -c "devscriptor-${VERSION}-${TARGET}.tar.gz.sha256"   # macOS: shasum -a 256 -c
tar -xzf "devscriptor-${VERSION}-${TARGET}.tar.gz"
sudo mv "devscriptor-${VERSION}-${TARGET}/devscriptor" /usr/local/bin/
```

On macOS, Gatekeeper may quarantine the unsigned binary on its first launch; clear
the flag with `xattr -d com.apple.quarantine /usr/local/bin/devscriptor`. The Intel
build also runs on Apple Silicon through Rosetta 2, but prefer the native one there.

### Windows

1. Download `devscriptor-<version>-x86_64-pc-windows-msvc.zip` from the
   [release page](https://github.com/vladimirpesic/devscriptor/releases/latest) and extract it. It holds a folder,
   `devscriptor-<version>-x86_64-pc-windows-msvc`, with the one file `devscriptor.exe` in it.
2. Move it to a folder on your `PATH`, or add the extracted folder to it.
3. Open a new PowerShell or Command Prompt window and run `devscriptor --version`.

Devscriptor also runs inside WSL 2, with the Linux build and Linux language
servers.

## Building from source

With Rust 1.95 or newer and a C compiler (the tree-sitter grammars and the bundled
SQLite are C):

```bash
git clone https://github.com/vladimirpesic/devscriptor.git && cd devscriptor
cargo build --release -p devscriptor-cli       # target/release/devscriptor
```

`devscriptor-cli`'s default features build everything: the CLI, the language-server
integration and all 75 MCP tools. See [Building](building.md) for portable release
builds.

## Verifying the install

```bash
devscriptor --version
devscriptor doctor
```

`doctor` checks the embedding model cache, the project configuration, the
database, disk space, memory and the tree-sitter grammars.

The MCP server waits for JSON-RPC on stdin and logs to stderr, so close stdin and
read its startup line. It is logged at `info`, which only `--verbose` shows, and
`--no-daemon` serves in this process rather than through the workspace's shared
server:

```bash
devscriptor --verbose mcp --no-daemon </dev/null 2>&1 >/dev/null | grep -m1 "Starting Unified MCP Server"
# Starting Unified MCP Server (75 tools)
```

## First steps

Devscriptor indexes in two phases: a fast structural scan, then, when you want
semantic search, embedding.

```bash
cd my-project
devscriptor scan .                     # entities, relationships, keyword search
devscriptor search "parse_config"      # entities whose name contains the query
devscriptor embed                      # vectors for semantic search
devscriptor find-similar "where are retries handled"
```

- `scan` parses the project into `devscriptor.db` in the working directory
  (`--database` names another). It never loads the embedding model.
- `embed` downloads the embedding model the first time —
  `jinaai/jina-embeddings-v2-base-code`, ~160 MB, from Hugging Face with the
  Devscriptor mirror as a fallback — into
  `<cache>/devscriptor/models/jina-embeddings-v2-base-code/` (`<cache>` is
  `~/.cache` on Linux, `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows).
  It then embeds the substantive entities — functions, classes, types, modules,
  documentation sections — that have no vector yet, and unloads the model. Running
  it again embeds only what is new or changed.
- `DEVSCRIPTOR_MODEL_PREFER_MIRROR=1` tries the mirror first;
  `DEVSCRIPTOR_HF_BASE_URL` and `DEVSCRIPTOR_MODEL_MIRROR_URL` override the
  sources. For a machine without a network, see
  [Privacy](privacy.md#air-gapped-and-offline-use).

The [CLI guide](cli.md) covers every command, and
[Configuration](configuration.md) a project's `devscriptor.yaml`.

## Language servers

The language-server features — definitions, references, hovers, diagnostics,
renames — need a server for the language. Devscriptor finds the ones on your
system and in VS Code's extensions, and installs missing ones when you ask:

```bash
devscriptor lsp setup             # detect the project's languages, then install
devscriptor lsp setup --yes       # without prompts
devscriptor lsp status            # every known server and where it resolves
```

Most servers need a language runtime (Node.js, Java, .NET, …), listed in
[Language server prerequisites](lsp-servers.md). The platform scripts install
those runtimes:

```bash
cd scripts/linux-x86_64 && sudo ./install-dependencies.sh     # or linux-aarch64
cd scripts/macos-aarch64 && ./install-dependencies.sh         # or macos-x86_64; as your user: Homebrew refuses root
```

```powershell
cd scripts\windows-x86_64; .\install-dependencies.ps1          # in an elevated PowerShell
```

[Language servers](lsp.md) explains detection, installation and configuration.

## Connecting an AI agent

Configure the agent's MCP client to start `devscriptor mcp`:

```json
{
  "mcpServers": {
    "devscriptor": { "command": "devscriptor", "args": ["mcp"] }
  }
}
```

The clients started in one directory share one Devscriptor process for that
workspace. [MCP server](mcp.md) describes the server, the shared process and the
75 tools.

## Updating

Devscriptor never checks for updates and does not update itself. Compare the
installed version with the newest release, which `latest.json` names:

```bash
devscriptor --version
curl -sL https://github.com/vladimirpesic/devscriptor/releases/latest/download/latest.json     # "tag_name": "v…"
```

The VS Code extension's **Devscriptor: Check for Devscriptor Updates** compares
the two for you. To update, install the new release over the old one.

- **Running MCP servers.** A workspace's shared server (`devscriptor serve`) goes on
  running the old binary until 10 minutes after its last session. The first
  `devscriptor mcp` of the new version stops it and starts its own; the sessions
  the old one still served end. To switch every workspace at once, stop the shared
  servers: `pkill -f 'devscriptor .*[s]erve --workspace'` on Linux and macOS,
  `taskkill /F /IM devscriptor.exe` on Windows (which stops every Devscriptor
  process). Windows cannot replace a running executable, so stop them there before
  replacing `devscriptor.exe`.
- **Indexes.** An index records the schema version it was written with. One of
  another version is emptied when it is opened — its notes in the Context Graph
  are kept — and the next scan builds it again. An index whose vectors were made by
  another embedding model or arithmetic drops them, and `devscriptor embed` makes
  them again; semantic search embeds what it needs in the meantime.
- **Language servers.** Re-run `devscriptor lsp setup --yes` to install what a
  project needs; an install that no longer matches the version the registry pins is
  replaced.

## Uninstalling

```bash
# Stop the shared MCP servers (each also stops 10 minutes after its last session)
pkill -f 'devscriptor .*[s]erve --workspace'

# Remove the binary
sudo rm "$(command -v devscriptor)"

# Remove the caches: the model, installed language servers, their state, and the
# shared servers' runtime files
rm -rf ~/.cache/devscriptor                 # macOS: also ~/Library/Caches/devscriptor
[ -n "$XDG_RUNTIME_DIR" ] && rm -rf "$XDG_RUNTIME_DIR/devscriptor"

# Optionally remove the MCP server's shared database (your indexes and Context Graph
# notes) and your templates; per-project devscriptor.db files stay where they are
rm -rf ~/.local/share/devscriptor ~/.config/devscriptor
```

[Privacy](privacy.md#where-data-is-stored) lists every location on each platform.

## Troubleshooting

### `devscriptor: command not found`

Check that the directory you moved it to is on your `PATH`
(`echo $PATH | tr ':' '\n'`), or add it in your shell profile.

### `version 'GLIBC_2.xx' not found`

The binary was built on a newer distribution than the one running it. Use a
release archive, or build with `cargo zigbuild` as [Building](building.md#linux)
describes.

### Language servers are not found

```bash
devscriptor lsp status               # where each server resolves, or why not
devscriptor lsp check <server-id>    # one server, with its missing prerequisites
devscriptor lsp setup                # install what the project needs
```

### The embedding model does not download

Check that `https://huggingface.co` is reachable, then remove the partial cache and
run `devscriptor embed` again:

```bash
rm -rf ~/.cache/devscriptor/models          # macOS: ~/Library/Caches/devscriptor/models
```

For help, write to **<contact@devscriptor.com>**.
