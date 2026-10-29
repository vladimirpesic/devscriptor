# Privacy

Devscriptor analyses code on your machine. Scanning, indexing, search, analysis
and embedding make no network calls, there is no Devscriptor backend, and nothing
collects or sends usage data. This page lists the few things that do use the
network, and where Devscriptor keeps its data.

- [What stays local](#what-stays-local)
- [Network activity](#network-activity)
- [Verifying it](#verifying-it)
- [Air-gapped and offline use](#air-gapped-and-offline-use)
- [Where data is stored](#where-data-is-stored)

## What stays local

- **Scanning, parsing, indexing and search** run locally: FTS5 keyword search,
  vector search (`sqlite-vec`) and every structural analysis.
- **Embeddings are computed locally**: once the model is present,
  `jinaai/jina-embeddings-v2-base-code` runs offline in the pure-Rust `tract`
  inference engine compiled into the binary.
- **Language servers** run as local subprocesses and talk to Devscriptor over
  stdin and stdout.
- **No telemetry**: no usage data is collected or transmitted.

Your source code, its embeddings and your Context Graph notes never leave your
machine.

## Network activity

Devscriptor uses the network only for the following, and never sends your code.

### 1. Embedding model download (first use)

The default build has the `model-download` feature. The first time the model is
needed — `devscriptor embed`, `scan --embed`, `watch --embed`, a semantic query
(`find-similar`, `search -s semantic`) or the MCP server's `semantic_search`,
`query_with_context` or `generate_embeddings` — Devscriptor downloads
`jinaai/jina-embeddings-v2-base-code` (~160 MB) into its model cache. Later runs
load it from there with no network access.

- **From where:** Hugging Face (`huggingface.co`, at a pinned commit) first, then
  the Devscriptor mirror as a fallback: a GitHub release
  (`github.com/vladimirpesic/devscriptor/releases`, which GitHub redirects to its
  download host).
  `DEVSCRIPTOR_MODEL_PREFER_MIRROR=1` tries the mirror first;
  `DEVSCRIPTOR_HF_BASE_URL` and `DEVSCRIPTOR_MODEL_MIRROR_URL` override the two
  sources.
- **To avoid it:** place the model files in the cache before the model is first
  needed (see [Air-gapped and offline use](#air-gapped-and-offline-use)).

### 2. Language server installation (only on request)

Servers are installed only when you ask: `devscriptor lsp setup` (interactive, or
`--yes`) or the MCP `lsp_arch_setup` tool with `auto_install`. Scanning never
installs servers.

- **From where:** the Devscriptor mirror first, a GitHub release
  (`github.com/vladimirpesic/devscriptor/releases`, which GitHub redirects to its
  download host), checked against the SHA-256 the build pins for the archive.
  Without a mirror build, a server distributed through a
  package manager is installed with it (npm, `go install`, `dotnet tool`, gem,
  Coursier), which contacts that package registry, and the others come from their
  GitHub releases or publishers, checked against published or pinned SHA-256
  hashes. `DEVSCRIPTOR_LSP_MIRROR_URL` overrides the mirror.

### 3. Update check (the VS Code extension, on request)

Devscriptor never checks for updates and does not update itself. The VS Code
extension's **Check for Devscriptor Updates** command, only when you run it,
fetches the latest GitHub release's `latest.json`
(`https://github.com/vladimirpesic/devscriptor/releases/latest/download/latest.json`, which GitHub redirects to its download
host) to compare the newest release's version with the installed one; the request
carries nothing else.

### Summary

| Capability | Network |
| --- | --- |
| Scanning, parsing, indexing | Never |
| Keyword and vector search, analysis | Never |
| Embedding inference | Never, once the model is cached |
| LSP features | Never from Devscriptor (language servers are separate programs; see below) |
| Model download | Once, the first time the model is needed |
| Language server installation | Only on an explicit `lsp setup` or `lsp_arch_setup` |
| Update check | Only when the extension's command runs |

## Verifying it

Trace a scan's network system calls:

```bash
strace -f -e trace=network devscriptor scan ./my-project 2>&1 | grep -E "socket|connect"
# Only local (AF_UNIX) sockets, no TCP connections
```

The language servers Devscriptor starts are separate programs: some fetch a
project's dependencies while loading it (rust-analyzer through `cargo`, gopls
through the Go module proxy) unless their toolchain is configured for offline
use.

## Air-gapped and offline use

Devscriptor runs fully offline once the assets it would download are present:

- **Embedding model**: place `onnx/model_quantized.onnx`, `tokenizer.json`,
  `config.json` and `tokenizer_config.json` in the model cache
  (`<cache>/devscriptor/models/jina-embeddings-v2-base-code/`, see the table
  below) before the model is first needed; `scripts/download-embedding-model.sh`
  fetches them on a connected machine. Missing files are downloaded, which fails
  with an error when there is no network; keyword search keeps working.
- **Language servers**: place server builds under
  `<cache>/devscriptor/lsp-servers/<id>/`, install them on the `PATH`, or list
  them in a `devscriptor-lsp.toml`.

Model files are checked against pinned SHA-256 hashes. If you provision files
whose hashes differ, `DEVSCRIPTOR_SKIP_MODEL_VERIFY=1` turns off **all** model
checksum checks; set it only for files you have verified by other means (see
[Security](security.md#what-is-verified-before-use)).

## Where data is stored

`<cache>` is `~/.cache` on Linux, `~/Library/Caches` on macOS and `%LOCALAPPDATA%`
on Windows; `<config>` is `~/.config` on Linux, `~/Library/Application Support` on
macOS and `%APPDATA%` on Windows. `XDG_CACHE_HOME`, `XDG_CONFIG_HOME` and
`XDG_DATA_HOME`, set to an absolute path, take the place of `<cache>`, `<config>`
and the user data directory on every platform.

| Data | Location |
| --- | --- |
| Analysis database (CLI) | `./devscriptor.db` in the working directory, or `--database` |
| Analysis database (`devscriptor mcp` and `serve`) | `default.db` in the user data directory — `~/.local/share/devscriptor/` on Linux, `~/Library/Application Support/devscriptor/` on macOS, `%LOCALAPPDATA%\devscriptor\` on Windows — or `--database` |
| Context Graph notes | Inside the analysis database |
| Embedding model | `<cache>/devscriptor/models/jina-embeddings-v2-base-code/` |
| Installed language servers | `<cache>/devscriptor/lsp-servers/<id>/` |
| Language servers' state | `<cache>/devscriptor/`: the compilation databases written for clangd (`clangd/`), kotlin-lsp's indexes (`kotlin-lsp/`) |
| Shared MCP server files | The runtime directory: `$XDG_RUNTIME_DIR/devscriptor/`, else `~/.cache/devscriptor/run/` on Linux and macOS and `%LOCALAPPDATA%\devscriptor\run\` on Windows |
| Your configuration templates | `<config>/devscriptor/templates/` |
| Trusted `devscriptor-lsp.toml` files | `<config>/devscriptor/trusted-lsp-configs.json` |

Devscriptor reads only the projects you point it at, its own directories above,
and, read-only, the extension directories of VS Code and similar editors, to find
the language servers they bundle.

Questions about privacy: **<contact@devscriptor.com>**.
