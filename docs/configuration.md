# Configuration

Devscriptor needs no configuration. A project can keep its scan scope and a few
other settings in a `devscriptor.yaml`; the language servers have their own file;
and environment variables tune the rest.

- [The project file: devscriptor.yaml](#the-project-file-devscriptoryaml)
- [Language server configuration](#language-server-configuration)
- [Environment variables](#environment-variables)

## The project file: devscriptor.yaml

A project keeps its settings in `devscriptor.yaml`, in its root directory. Every
setting is optional; one left out keeps its default:

```yaml
scan:
  include: [rs, py]            # extensions to index (default: every supported language)
  exclude: ["vendor/**", gen]  # globs relative to the scanned root; a bare name matches at any depth
  max_file_size_mb: 10         # larger files are left out (default: 50)
  use_gitignore: true          # apply .gitignore and git's other ignore files (default: true)
  cross_file: auto             # none, light (type uses, supertypes), deep (calls too), auto
embeddings:
  selection: standard          # all, standard (default) or significant
database:
  pool_size: 16                # connections (default: twice the CPU count, 16 to 64)
```

- **`scan.exclude`** comes on top of the excludes every scan applies
  (`node_modules`, `target`, `.git`, `dist`, `build`, `vendor`, … — see
  [Security](security.md#file-access) for the full list).
- **`scan.cross_file`** decides which references resolve across files: `deep`
  resolves every one; `light` every one but calls, which link only to a definition
  in the caller's file; `none` resolves within each file only; `auto`, the default,
  is `light` above 500,000 entities under the scanned root and `deep` below.
- **`embeddings.selection`** decides which entities get a vector: `standard`
  embeds the substantive kinds (functions, classes, types, modules, documentation
  sections, …) and skips constants, decorators, indexes and other lexical noise;
  `significant` also skips entities whose body is under 96 bytes; `all` embeds
  every named entity. Skipped entities stay findable by name and keyword.

Only settings that act belong in the file: a field it does not name is an error,
so a typo is reported instead of passing for a setting.

**Who reads it.** `scan` and `watch` read the file in the directory they index;
`mcp` and `serve` the one in their workspace; `embed` and the commands that query
the index (`search`, `find-similar`, `search-function`, `search-class`,
`search-pattern`, `analyze`, `status`, `stats`, `call-graph`,
`architecture-report`, `context`, `repl`) the one in the working directory;
`doctor` reports on it. `--config <FILE>` names another file, which must exist. A
file that cannot be applied stops the command with the reason; `devscriptor
validate` checks one beforehand.

**Precedence**, highest first: a command's flags (or an MCP tool's arguments), the
environment (`DEVSCRIPTOR_EMBEDDING_SELECTION`, `DEVSCRIPTOR_DB_POOL_SIZE`), the
file, the defaults. A flag replaces the file's setting: `--include rs` takes the
place of `scan.include`, not an addition to it.

**A running server** (`devscriptor mcp`, or a workspace's shared server) reads the
file again when it changes, at the next tool call, and its scans, watcher and
embedding take the new settings; only the pool size waits for a restart. An edit
that leaves the file broken changes nothing: the server keeps the settings it had,
and `health_check` reports the error under `project_config`. A server the file
keeps from applying at start serves with the defaults rather than not at all.

**Writing one.** `devscriptor template apply <NAME>` and `devscriptor profile apply
<NAME>` write a template's configuration, and `devscriptor template analyze <PATH>
--output devscriptor.yaml` one suggested for the project's stacks (see
[CLI](cli.md#template)).

## Language server configuration

The language servers need no configuration: Devscriptor finds or installs them.
To choose them yourself, use the file `DEVSCRIPTOR_LSP_CONFIG` names, or a
`devscriptor-lsp.toml` in the working directory once you have trusted it with
`devscriptor lsp trust`. See [Language servers](lsp.md#configuration-file).

## Environment variables

### Embedding

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_EMBEDDING_SELECTION` | Which entities are embedded: `standard` (default), `significant`, `all`; outranks the project's `embeddings.selection` |
| `DEVSCRIPTOR_EMBEDDING_BATCH` | Texts per inference run for `embed`, `scan --embed` and the MCP server, in place of one (at most 256; `embed --batch-size` takes precedence) |
| `DEVSCRIPTOR_EMBEDDING_SESSIONS` | Inference runs at once (1–8; default two, one when RAM or the cores are short) |
| `DEVSCRIPTOR_EMBEDDING_THREADS` | Matrix-multiplication threads per run (default: the cores shared out among the runs) |
| `DEVSCRIPTOR_MODEL_PREFER_MIRROR` | `1` or `true`: download the model from the Devscriptor mirror before Hugging Face |
| `DEVSCRIPTOR_HF_BASE_URL`, `DEVSCRIPTOR_MODEL_MIRROR_URL` | Override the two model download locations; a mirror holds the files side by side (`model_quantized.onnx`, not `onnx/model_quantized.onnx`) |
| `DEVSCRIPTOR_SKIP_MODEL_VERIFY` | `1` or `true`: skip the model files' SHA-256 check, for files provisioned out of band |

### Database

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_DB_POOL_SIZE` | SQLite connection pool size (default: twice the core count, 16–64); outranks the project's `database.pool_size` |

### MCP server

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_DAEMON` | `0` or `false`: `devscriptor mcp` serves in its own process instead of relaying to the workspace's shared server (as `--no-daemon`) |
| `DEVSCRIPTOR_DAEMON_IDLE_SECS` | Seconds a shared server waits with no session open before it stops (default 600) |
| `DEVSCRIPTOR_WATCH` | `0` or `false`: the server does not watch the scanned project it runs in (as `--no-watch`) |
| `DEVSCRIPTOR_FRESHNESS_BUDGET_MS` | How long an answer waits for its changed files to be re-indexed (default 250) |
| `XDG_RUNTIME_DIR` | Where the shared servers keep their sockets, locks, sidecars and logs: `$XDG_RUNTIME_DIR/devscriptor/` when set |

### Language servers

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_LSP_CONFIG` | A language-server configuration file, used instead of the detected servers |
| `DEVSCRIPTOR_LSP_STARTUP` | `lazy` (default) or `eager`: when language servers start; `--lsp-startup` outranks it |
| `DEVSCRIPTOR_LSP_IDLE_SECS` | Seconds an unused language server runs before it stops (default 600; 0: never) |
| `DEVSCRIPTOR_LSP_MAX_SERVERS` | Most language servers running at once, the least recently used stopped first (default 0: no cap) |
| `DEVSCRIPTOR_LSP_MIRROR_URL` | Where the mirror's archives are fetched from, in place of the release this build pins: a place that holds them side by side, as the release does; they are checked against the SHA-256 this build pins |

### Scripts

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_MODEL_DIR` | Where `scripts/download-embedding-model.sh` puts the model (default: the model cache) |
| `DEVSCRIPTOR_REGISTRY_JSON` | The registry the `package-lsp-servers` scripts read each server's license files from, and check the manifest against (default: `crates/lsp/src/manager/registry.json`) |

Logging is set with the global `--verbose` flag (`debug` instead of `warn`), not
`RUST_LOG`; logs go to stderr.
