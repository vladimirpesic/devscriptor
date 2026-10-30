# Security

This page describes Devscriptor's trust model: what it verifies before it runs or
loads anything, what it reads, who may talk to its server, and where it uses
`unsafe` code.

- [Reporting a vulnerability](#reporting-a-vulnerability)
- [SQLite extensions](#sqlite-extensions)
- [What is verified before use](#what-is-verified-before-use)
- [File access](#file-access)
- [The shared server's endpoint](#the-shared-servers-endpoint)
- [Language servers](#language-servers)
- [Memory safety](#memory-safety)
- [Dependencies](#dependencies)
- [Running Devscriptor safely](#running-devscriptor-safely)

## Reporting a vulnerability

Report a vulnerability privately by email to **<contact@devscriptor.com>**, not in
a public issue.

## SQLite extensions

Devscriptor uses three pieces of SQLite functionality that are commonly shipped as
extensions:

- **sqlite-vec** (`vec0`) — vector search, for semantic code search
- **FTS5** — full-text search
- **JSON1** — JSON processing

**Devscriptor never loads an extension from disk.** SQLite's dynamic-extension API
is never enabled: the codebase contains no call to `enable_load_extension`,
`load_extension`, or `LoadExtensionGuard`, so there is no code path that reads an
extension binary at runtime. Instead:

1. **sqlite-vec** is a statically linked Rust crate
   ([`sqlite-vec`](https://github.com/asg017/sqlite-vec)). It is registered once,
   before any connection is opened, through SQLite's `sqlite3_auto_extension` hook
   ([`crates/core/src/database/connection.rs`](../crates/core/src/database/connection.rs)).
   The `vec0` code is compiled into the Devscriptor binary; nothing is fetched or
   `dlopen`ed.
2. **FTS5 and JSON1** are compiled into the SQLite that `rusqlite` bundles
   (`features = ["bundled", …]`). They are part of the binary, not separate files.

The trust boundary is therefore the Devscriptor binary and the crates it was built
from, the same boundary as every other dependency. There is no separate extension
attack surface, no allow-list to configure, and nothing to verify at runtime.

## What is verified before use

Everything Devscriptor downloads is checked before it is used, and a mismatch
fails the download (fail-closed):

| Asset | Verification | Where |
| --- | --- | --- |
| Embedding model files | SHA-256 of each of the four files, pinned in the code | `crates/core/src/embeddings/model_fetch.rs` |
| Embedding model revision | Hugging Face downloads name an immutable commit (`516f4baf…`), never `main` | `crates/core/src/embeddings/model_fetch.rs` |
| Language server from the mirror | SHA-256 of the archive, pinned in the build with the mirror release | `crates/lsp/src/manager/mirror.json`, `downloader.rs` |
| Language server from GitHub or its publisher | SHA-256 GitHub publishes for the release asset, the one the registry pins, or, for kotlin-lsp's latest build, the one JetBrains publishes beside it | `crates/lsp/src/manager/downloader.rs` |

`DEVSCRIPTOR_SKIP_MODEL_VERIFY=1` turns off the model checks, for model files
provisioned out of band; set it only for files verified by other means. Archive
entries are joined to their destination with `safe_join`, which refuses paths that
would leave it (zip or tar slip), and a language server id outside the registry is
refused before any path is built from it.

## File access

Devscriptor reads only what it needs to parse, and only inside the path you pass to
`scan` / `watch`. Three filters apply, in this order:

1. **`.gitignore` is honoured** by default (`scan.use_gitignore`, default `true`).
2. **A built-in exclude list** skips `node_modules`, `.git`, `target`,
   `__pycache__`, `.venv`, `venv`, `.env`, `.idea`, `.vscode`, `dist`, `build`,
   `.pytest_cache`, `.mypy_cache`, `.tox`, `vendor`, `third_party`, `*.min.js`,
   `*.min.css` and `*.map`. Add your own with `--exclude` (CLI), `exclude_patterns`
   (MCP) or `scan.exclude` in `devscriptor.yaml`.
3. **An extension allow-list**: only files whose extension is one of the supported
   source extensions are opened at all.

The third filter keeps most secrets out: an SSH private key (`id_rsa`, no
extension), `.aws/credentials` (no extension) and `*.db` / `*.sqlite` files are
outside the allow-list and never opened.

> The allow-list does include `json`, `yaml`, `yml`, `toml` and `xml`, so a
> configuration file *inside the scanned tree*, such as `.docker/config.json`, is
> read unless excluded. Devscriptor never transmits file contents (see
> [Privacy](privacy.md)), but if such files live in your repository and you do not
> want them indexed, add them to `.gitignore` or the excludes.

The MCP file tools (`describe_file`, `describe_modification`,
`validate_file_edit`) read only files under the server's working directory or the
database's directory, or files already indexed; any other path is rejected.

## The shared server's endpoint

`devscriptor mcp` relays to a per-workspace server, `devscriptor serve` (see
[Architecture](architecture.md#mcp-server-processes)). Its endpoint is the only
one Devscriptor listens on: a Unix socket in a directory only its user may enter,
or a named pipe on Windows that refuses remote clients. There is no network port.

| Check | Unix | Windows |
| --- | --- | --- |
| The server serves only its user | Directory mode 0700, socket mode 0600; the peer credentials (`SO_PEERCRED` on Linux, `getpeereid` on macOS) must name the user | The pipe's default security lets other users open it only for reading, so they cannot send a request |
| A client talks only to its user's server | The process serving the socket must run as the user | The process serving the pipe (`GetNamedPipeServerProcessId`) must be the server the sidecar names, which only the user can write |
| Only the user's processes are stopped | SIGTERM to the pid of a server that holds the lock; the kernel refuses other users' | `taskkill` of that pid; Windows refuses other users' |

## Language servers

Language servers run as subprocesses and talk to Devscriptor over stdin and stdout.
They are found on your system or in editor extensions, installed on request
(`devscriptor lsp setup`, or the MCP `lsp_arch_setup` tool), or named in a
`devscriptor-lsp.toml`. A project's own `devscriptor-lsp.toml` names commands to
run, so it is ignored until you trust its exact content with
`devscriptor lsp trust` (see [Language servers](lsp.md#configuration-file)).
Install language servers only from sources you trust.

## Memory safety

Devscriptor is written in Rust. Shipped code has sixteen `unsafe` blocks: eleven
in `devscriptor-core`, one in `devscriptor-lsp`, three in `devscriptor-mcp` and
one in `devscriptor-cli`.

| Where | Count | Why |
| --- | ---: | --- |
| `core/src/parser/mmap_parser.rs` | 4 | Memory-mapped reads of large files: `flock` and its release, `Mmap::map`, and zero-copy `from_utf8_unchecked` (UTF-8 validity is established when the file is opened) |
| `core/src/database/connection.rs` | 3 | C FFI: the `sqlite3_auto_extension` callback that registers the statically linked `sqlite-vec` |
| `core/src/utils/performance.rs` | 3 | `libc::sysconf` and platform memory queries |
| `core/src/embeddings/tract_backend.rs` | 1 | Reads the model's output tensor as an `f32` slice (`from_raw_parts` over the tensor tract allocated) |
| `lsp/src/lsp/lifecycle.rs` | 1 | `pre_exec` on Linux, setting `PR_SET_PDEATHSIG` so a language server dies with Devscriptor |
| `mcp/src/core_module/search.rs` | 1 | Zero-copy ASCII lowercasing in the fuzzy-score hot path |
| `mcp/src/daemon.rs` | 2 | Windows only: `GetFileInformationByHandle` (a lock file's identity) and `GetNamedPipeServerProcessId` (the pipe's server) |
| `cli/src/commands/mcp.rs` | 1 | Unix only: `pre_exec` calling `setsid`, so the server `devscriptor mcp` starts runs in a session of its own |

Unit tests add five more, for `std::env::set_var` / `remove_var` (unsafe since
Rust 2024) in `core/src/embeddings/provider.rs` and `lsp/src/config/mod.rs`, and
the integration tests under `crates/*/tests` a few of the same kind.

## Dependencies

Devscriptor depends on established crates of the Rust ecosystem. The
security-relevant ones:

- `rusqlite`, with SQLite bundled
- `sqlite-vec`, vector search
- `tokio`, the async runtime
- `tree-sitter` and its grammars, which are C
- `reqwest` with `rustls` and `ring`, and the Mozilla root certificates
  `webpki-roots` bundles: there is no OpenSSL in the tree

All dependencies are pinned in `Cargo.lock`, and CI runs `cargo audit` on every
push to `main` and every pull request.

## Running Devscriptor safely

1. **Use a verified build**: check the `.sha256` published beside each release
   archive, or build from source (see [Building](building.md)).
2. **Sandbox untrusted code**: run Devscriptor in a container or a restricted
   environment when you analyse a codebase you do not trust.
3. **Review language servers** before you install them, and trust a project's
   `devscriptor-lsp.toml` only after reading what it would start.
