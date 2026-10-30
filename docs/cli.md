# CLI

Every `devscriptor` command and its options. `devscriptor <command> --help` prints
the same, from the binary itself.

## Table of Contents

  - [Global Options](#global-options)
  - [Two-Phase Workflow](#two-phase-workflow)
  - [Commands](#commands)
    - [`scan`](#scan)
    - [`watch`](#watch)
    - [`embed`](#embed)
    - [`search`](#search)
    - [`find-similar`](#find-similar)
    - [`search-function` / `search-class`](#search-function--search-class)
    - [`search-pattern`](#search-pattern)
    - [`analyze`](#analyze)
    - [`status`](#status)
    - [`stats`](#stats)
    - [`doctor`](#doctor)
    - [`cache-stats`](#cache-stats)
    - [`perf-stats`](#perf-stats)
    - [`clean`](#clean)
    - [`call-graph`](#call-graph)
    - [`architecture-report`](#architecture-report)
    - [`context`](#context)
    - [`template`](#template)
    - [`profile`](#profile)
    - [`validate`](#validate)
    - [`repl`](#repl)
    - [`lsp`](#lsp)
    - [`definition`](#definition)
    - [`references`](#references)
    - [`mcp`](#mcp)
    - [`serve`](#serve)
  - [Examples: Common Workflows](#examples-common-workflows)
    - [First scan of a new project](#first-scan-of-a-new-project)
    - [Add semantic search](#add-semantic-search)
    - [Nightly incremental re-scan](#nightly-incremental-re-scan)
    - [CI pipeline (fast, no model)](#ci-pipeline-fast-no-model)
    - [Large repo, first time](#large-repo-first-time)

---

## Global Options

Available for all commands:

```bash
devscriptor [OPTIONS] <COMMAND>

Options:
  -d, --database <DATABASE>  Database file path (default: devscriptor.db in the working directory)
      --config <FILE>        Project configuration file (default: devscriptor.yaml in the project)
  -v, --verbose              Enable verbose (debug-level) logging
  -h, --help                 Print help
  -V, --version              Print version
```

`--config` names the [project configuration](configuration.md) to use instead of the project's
own `devscriptor.yaml`.

> `mcp` and `serve` are the exceptions to the `devscriptor.db` default: with no
> `--database` they open a shared, cwd-independent database. See [`mcp`](#mcp).
> **Banner.** Run from a terminal, every command except `mcp`, `serve`, `lsp`, `repl`,
> `profile` and `context` first prints a Devscriptor banner on **stderr**, and only
> when stderr is a terminal. stdout carries nothing but the command's output, so
> the JSON of `stats`, `search -o json`, `analyze -o json`, `call-graph -f json`,
> `architecture-report -f json`, `definition` and `references` can be piped
> straight into a parser. `analyze` reports `Analyzing: <path>` on stderr too.

---

## Two-Phase Workflow

Devscriptor separates **indexing** (always fast) from **embedding** (slow, needs RAM):

```bash
devscriptor scan <path>      # Phase 1: parse → knowledge graph → FTS5 ready (<30s)
devscriptor embed             # Phase 2: load ONNX → embed → unload (~1.3 GB, run once)
```

**Why two phases?**

The ONNX embedding model (`jinaai/jina-embeddings-v2-base-code`) occupies ~1.3 GB of RAM.
On an 8 GB machine also running an IDE and language servers, loading it during an interactive scan
causes memory pressure. Separating the phases means:

- `devscriptor scan` is always fast and never loads the model.
- FTS5 keyword search and all structural analysis are available immediately after scan.
- `devscriptor embed` can be run in the background, overnight, or on demand.
- Re-running `embed` only processes new or changed entities.

---

## Commands

### `scan`

Parse a codebase and build the knowledge graph.

```bash
devscriptor scan [OPTIONS] <PATH>
```

**Arguments:**

| Argument | Description |
| --- | --- |
| `PATH` | Directory to scan |

**Options:**

| Option | Default | Description |
| --- | --- | --- |
| `-i, --include <EXTS>` | the project's, else all supported | Comma-separated file extensions to include (e.g. `rs,py`); replaces `scan.include` |
| `-e, --exclude <PATTERNS>` | the project's | Comma-separated glob patterns to exclude; replaces `scan.exclude` |
| `--max-file-size <MB>` | the project's, else 50 | Skip files larger than this many MB |
| `--batch-size <N>` | 50 | Files processed per DB transaction |
| `--incremental` | off | Skip unchanged files (uses mtime+size+hash manifest) |
| `--embed` | off | Generate embedding vectors inline after scanning. Requires ~4 GB free RAM; skipped automatically if RAM is insufficient |
| `--skip-embeddings` | off | Force-skip embeddings even when `--embed` is passed (useful in CI) |
| `--bulk-load` | off | Drop FTS5 triggers before scan, rebuild index at end (3–5× faster FTS for fresh full scans; unsafe for incremental) |
| `--metrics` | off | Print an extended quality-metrics summary after scanning: full entity/language distribution, embedding cache-hit rate, peak RSS |
| `-d, --database` | `devscriptor.db` | Path to the SQLite database |

**Project configuration:** `scan` applies the `devscriptor.yaml` in `PATH` (or the file
`--config` names): its scope, cross-file mode, embedding selection and pool size; the flags above
outrank it. Its header names the file in a `Config` line. See [Configuration](configuration.md).

**Embedding behaviour:**

By default `scan` is fast — it never loads the ONNX model. Pass `--embed` to generate vectors
inline (requires ≥ 4 GB available RAM). The model used is always
`jinaai/jina-embeddings-v2-base-code` (768-dim, INT8-quantized, ~150 MB on disk).

**Re-scans:**

Scanning a project the database already holds is a re-index, with or without
`--incremental` (which only skips the unchanged files): each re-parsed file
replaces its own entities and edges, edges from other files into it are kept, an
entity keeps its embedding vector unless its body changed, and files deleted from
the project are removed. Other projects in the same database are not touched. A
definition added to one file is linked from callers in other files only when they
are re-parsed too; a scan without `--incremental` re-parses everything.

**Summary:**

When it finishes, `scan` prints what the run did: files discovered, parsed,
skipped (incremental) and failed; entities extracted; `Relationships … stored this
run`, the relationship rows it inserted (an edge already stored is not counted, so
an incremental re-scan of an unchanged tree reports 0), with `across files` for
those whose endpoints lie in different files; vectors stored; and `Index totals`,
the files, entities and relationships the database holds afterwards.

**Examples:**

```bash
# Fast scan — FTS5 search ready in seconds
devscriptor scan ./my-project

# Scan + embed in one step (needs ~4 GB free RAM)
devscriptor scan ./my-project --embed

# Only Python files, exclude tests
devscriptor scan ./my-project -i py -e tests/,*_test.py

# Re-scan only changed files
devscriptor scan ./my-project --incremental

# First-time scan of a large repo (FTS5 bulk rebuild is faster)
devscriptor scan ./my-project --bulk-load

# CI scan — always skip embeddings regardless of other flags
devscriptor scan ./my-project --skip-embeddings
```

---

### `watch`

Keep the knowledge graph current in real time. Runs an initial incremental scan,
then watches the directory and re-scans incrementally on every source-file change
(create / modify / delete / rename) until you press Ctrl-C. See
[Watcher](watcher.md) for the full reference.

```bash
devscriptor watch <PATH> [OPTIONS]
```

| Option | Default | Description |
| --- | --- | --- |
| `-i, --include <EXT,…>` | the project's, else all | Restrict to these file extensions; replaces `scan.include` |
| `-e, --exclude <GLOB,…>` | the project's | Exclude paths matching these globs; replaces `scan.exclude` |
| `--embed` | off | Re-embed changed entities inline after each re-scan (requires ~4 GB RAM) |
| `--debounce-ms <MS>` | 400 | Quiet period before a re-scan runs (coalesces bursts of edits) |

```bash
# Watch the current project (structural updates only)
devscriptor watch .

# Watch a specific database with a snappier debounce
devscriptor --database ./devscriptor.db watch ./my-project --debounce-ms 250
```

Like `scan`, `watch` applies the project's `devscriptor.yaml` (in `PATH`, or `--config`), read
when it starts; restart it after editing the file.

The watcher only reacts to parseable source files; it ignores VCS/build
directories, the database, logs, and editor temp files. Where the system's limit on
file watches is reached, it polls instead and its `Mode` line says so.

---

### `embed`

Generate semantic embedding vectors for all entities that lack them.

```bash
devscriptor embed [OPTIONS]
```

**Options:**

| Option | Default | Description |
| --- | --- | --- |
| `--batch-size <N>` | 0 (auto) | Entities per inference run. `0` means one, which is fastest (see [Performance](performance.md#the-tuner-runs-texts-per-run-threads)); a larger value pads each run to its longest text |
| `--force` | off | Re-embed entities that already have vectors |
| `--daemon` | off | Fork a background embedding process and return immediately. Progress is tracked in the `embedding_jobs` table |
| `--status` | off | Print the latest background embedding job's progress and exit |
| `-d, --database` | `devscriptor.db` | Database to embed |

**Behaviour:**

1. Loads `jinaai/jina-embeddings-v2-base-code` (a few seconds; up to ~60 s under memory pressure;
   downloaded on first use and cached for later runs). It embeds one entity per inference run,
   two runs at once (one when free RAM or the cores are short), each with its share of the cores —
   see [Performance](performance.md#the-tuner-runs-texts-per-run-threads). Override with
   `--batch-size N` or the `DEVSCRIPTOR_EMBEDDING_{SESSIONS,BATCH,THREADS}` env vars.
2. Finds the entities of the embedded kinds (see *What gets embedded* below) that have no entry in
   `entity_vectors`, largest first.
3. Embeds them across the concurrent runs, showing a live progress bar.
4. Stores 768-dim vectors in `entity_vectors`, and their text hashes in `embedding_cache`, which
   `embed` itself, `scan --embed`, `watch --embed` and the MCP server consult to skip inference for
   text embedded before: an entity whose text is cached (a moved or renamed function, say) gets
   that vector without running the model, unless `--force` is given. Re-runs skip entities that
   already have a vector, so unchanged code is free.
5. **Unloads the model immediately** after all vectors are written, freeing ~1.3 GB.

**Sized for ≥ 4 GB available RAM.** With less it still runs, with fewer runs at a time. The
command is **idempotent** — re-running only processes entities still missing vectors.

**What gets embedded.** Only substantive kinds get a vector: functions, classes, structs, enums,
interfaces, traits, modules, namespaces, type aliases, unions, macros, components, contracts,
extensions, SQL tables, and documentation sections. Constants, decorators, indexes and
other lexical noise are skipped; they stay findable by name and keyword search. That is the
`standard` embedding selection, the default; the project's `embeddings.selection` (or
`DEVSCRIPTOR_EMBEDDING_SELECTION`, which outranks it) can make it `significant`, which also skips
entities whose body is under 96 bytes, or `all`, which embeds every named entity. `embed`, `scan --embed`,
`watch --embed`, the MCP server's `generate_embeddings` and its on-demand embedding all follow
the same selection, and `embed` prints it in its header. Because entity ids are content-stable, re-runs after code changes only
re-embed what actually changed. (`DEVSCRIPTOR_EMBEDDING_SESSIONS=1..8` overrides the auto
concurrency of two runs at once.)

**Examples:**

```bash
# Standard run — only missing vectors
devscriptor embed

# Eight entities per inference run instead of one
devscriptor embed --batch-size 8

# Embed in the background, then check on it
devscriptor embed --daemon
devscriptor embed --status

# Re-embed everything, overwriting the vectors already stored
devscriptor embed --force

# Point to a specific database
devscriptor embed -d /path/to/project.db
```

**Output example:**

```text
⚡ Devscriptor Embed
   Database : devscriptor.db
   Model    : jinaai/jina-embeddings-v2-base-code (768-dim)
   Tuning   : 2 concurrent run(s) × 1 text(s), 6 matmul thread(s)
   Selection: standard
   Entities : 534 to embed  (15 not embedded by policy or already vectorized)

⏳ Loading embedding model…  (this may take 30–60 seconds)
✅ Model loaded.

   [100%] 534/534 entities  (4.8/sec)

✅ Embedding complete!
   Vectors stored : 534
   Time elapsed   : 111.3s
   Throughput     : 4.8 entities/sec

🔍 Semantic (vector) search is now available.
   Try: devscriptor find-similar "your query"
```

---

### `search`

Search entities by name: by default, every entity whose name contains the query.

```bash
devscriptor search <QUERY> [OPTIONS]
```

**Options:**

| Option | Default | Description |
| --- | --- | --- |
| `-s, --search-type` | `symbol` | `symbol` (any entity whose name contains the query), `function` / `class` (that kind only), `semantic` (the hybrid search of [`find-similar`](#find-similar)) |
| `-l, --limit` | 20 | Maximum results |
| `-o, --output` | `text` | `text` (`markdown` prints the same), or `json`: an array of hits, each `{"name", "type", "file", "line", "end_line", "language"}`, plus `"score"` (relevance in `[0, 1]`) for `-s semantic` and `"fallback": true` when that search fell back to keywords |

**Examples:**

```bash
devscriptor search "getUserById"
devscriptor search "parse" -s function
devscriptor search "auth middleware" -s semantic
devscriptor search "auth middleware" -s semantic -o json
```

---

### `find-similar`

Hybrid search: an FTS5 keyword (BM25) ranking and a vector-similarity ranking, fused.

```bash
devscriptor find-similar <QUERY> [OPTIONS]
```

**Options:**

| Option | Default | Description |
| --- | --- | --- |
| `-l, --limit` | 10 | Maximum results |

The two rankings are combined with Reciprocal Rank Fusion, and each result's relevance is
scaled so the best match is 1.0. Keyword matches work right after `scan`; vector similarity
covers the entities `devscriptor embed` has embedded. The query itself is embedded too, so the
first query loads the model (downloading it if it is not cached yet).

---

### `search-function` / `search-class`

Optimised name search scoped to a single entity type.

```bash
devscriptor search-function <NAME> [--fuzzy] [--limit 50]
devscriptor search-class    <NAME> [--fuzzy] [--limit 50]
```

---

### `search-pattern`

Search entities by a name pattern, optionally scoped by entity type and language.

```bash
devscriptor search-pattern <PATTERN> [-t <ENTITY_TYPE>] [-l <LANGUAGE>]
```

| Option | Default | Description |
| --- | --- | --- |
| `<PATTERN>` | required | Name pattern to match |
| `-t, --entity-type` | any | Restrict to an entity type (e.g. `function`, `class`) |
| `-l, --language` | any | Restrict to a language (e.g. `rust`, `python`) |

```bash
devscriptor search-pattern "parse_" -t function -l rust
```

---

### `analyze`

Static analysis: dead code, complexity, code smells.

```bash
devscriptor analyze [OPTIONS] --kind <KIND>
```

**Options:**

| Option | Default | Description |
| --- | --- | --- |
| `-p, --path` | current dir | Path to the codebase (analysis reads the indexed database, so scan it first) |
| `-k, --kind` | required | `dead-code`, `complexity`, `clones`, `cross-reference`, `call-graph`, `code-smells`, `cross-file`, `all` |
| `--severity` | `medium` | Minimum severity: `info`, `low`, `medium`, `high`, `critical`. Text output hides findings below it. JSON applies it only to `clones`, `code-smells` and `cross-file`; the other kinds list every finding from `low` up |
| `-o, --output` | `text` | `text`, `json`, or `markdown`, which prints the same as `text` |

**Examples:**

```bash
devscriptor analyze -p ./my-project -k dead-code
devscriptor analyze -p ./my-project -k all -o json
devscriptor analyze -p ./my-project -k code-smells --severity high
```

---

### `status`

Print database statistics.

```bash
devscriptor status [--path <DIR>]
```

Shows the database path (and the project path, when given) with the number of files, entities
and relationships indexed.

---

### `stats`

Output database statistics as JSON (`{"files", "entities", "relationships"}`)
for scripting/CI. Unlike `status`, it takes no path and always prints JSON.

```bash
devscriptor stats
```

---

### `doctor`

Health check — verifies that all components are operational.

```bash
devscriptor doctor
```

Checks:

- Embedding model: whether the model files are in the local cache (a warning, not an error, when
  they are not: they are fetched on first use)
- Project config: the `devscriptor.yaml` in the working directory (or `--config`'s file), if any:
  valid, a warning it raises, or the error that keeps it from applying (then the other checks use
  the defaults)
- Database: opens it (`--database`, else `./devscriptor.db`) and reports its entity,
  relationship and file counts; a database that does not exist is reported, not created
- Embeddings: how many of the entities `embed` would embed (the project's selection) have no
  vector yet (a warning that
  suggests `devscriptor embed`; until then semantic search answers for them by keyword)
- Disk space: free space on the filesystem holding the database (an error below 1 GB)
- Memory: total and available RAM (a warning below 4 GB total)
- Tree-sitter grammars: the parser initialises

---

### `cache-stats`

Show parser cache statistics (cache hit/miss counts and effectiveness of the
content-hash parse cache). Takes no arguments.

```bash
devscriptor cache-stats
```

---

### `perf-stats`

Show performance metrics collected during the session. Takes no arguments.

```bash
devscriptor perf-stats
```

---

### `clean`

Drop all data and start fresh.

```bash
devscriptor clean
```

Removes the database file (`--database`, default `devscriptor.db`) with its `-wal`/`-shm` files.
Does not affect the embedding model cache.

---

### `call-graph`

Generate a function call graph starting from an entity.

```bash
devscriptor call-graph --entity <NAME> [--format text|json|dot|mermaid] [--max-depth 3] [--direction both|incoming|outgoing]
```

It follows every relationship of the entity (calls, uses, members, imports), up to
`--max-depth` steps away. Every format names the entity at each end of a relationship;
the JSON gives their ids too (`source`, `target`) with `source_name`, `source_type`,
`target_name` and `target_type`.

---

### `architecture-report`

Generate an architecture overview of the indexed codebase.

```bash
devscriptor architecture-report [--path <DIR>] [--format text|json|markdown] [--output <FILE>]
```

---

### `context`

Manage the Context Graph — the persistent long-term memory of notes (decisions,
standards, reflections, …) linked to code entities and concepts. See
[Context Graph](context-graph.md) for the full reference.

```bash
devscriptor context <SUBCOMMAND>
```

Subcommands: `add`, `list`, `search`, `show`, `delete`, `prune`, `entity`, `concept`.

- `add --title <T> --content <C> [--note-type <TYPE>] [--entities <UUIDs>] [--entity-type <TYPE>]` — add a note (optionally linked to entities).
- `list [--limit <N>] [--offset <N>]` — list notes with pagination.
- `search <QUERY> [--limit <N>]` — keyword search over note titles, content and summaries.
- `show <ID>` — show a single note by ID.
- `delete <ID>` — delete a note by ID.
- `prune [--max-age-days <N>] [--dry-run]` — archive active notes not updated in N days (default 90)
  and delete notes archived for more than twice that.
- `entity <ENTITY_TYPE>` — list notes linked to a given entity type.
- `concept <CONCEPT>` — list notes linked to a concept.

---

### `template`

Browse the configuration templates — the built-in set plus your own, which `profile` shares —
and write one out as a [project configuration](configuration.md). Your templates are JSON files in
`~/.config/devscriptor/templates/` (the platform's config directory elsewhere).

```bash
devscriptor template <SUBCOMMAND>
```

Subcommands: `list`, `show`, `apply`, `create`, `export`, `analyze`.

- `list [-c <CATEGORY>] [-s <SEARCH>]` — list available templates, optionally filtered.
- `show <NAME>` — show a template's details and the configuration it writes.
- `apply <NAME> [-o <FILE>]` — write the template's project configuration (default:
  `devscriptor.yaml`, which it replaces).
- `create <NAME> [-b <BASED_ON>]` — create a template, optionally a copy of another, and save it
  with your templates. Names are letters, digits, `-`, `_` and `.`.
- `export <NAME> [-o <FILE>]` — write the template definition as JSON (default:
  `<NAME>.template.json`).
- `analyze <PROJECT_PATH> [-o <FILE>]` — tell the project's stacks from their build files
  (`Cargo.toml`, `package.json`, `pom.xml`, …; without any, its dominant language), name the
  template that suits it, and print the configuration the stacks suggest: their extensions and
  their build directories to exclude. `--output` writes that configuration.

---

### `profile`

Browse the configuration profiles — the same set as `template`, built-in and your own — and write
one out as a configuration file.

```bash
devscriptor profile <SUBCOMMAND>
```

Subcommands: `list`, `show`, `create`, `apply`, `delete`.

- `list` — list available configuration profiles.
- `show <NAME>` — show a profile's details and the configuration it writes.
- `create <NAME> [-b <BASED_ON>]` — create a profile, optionally a copy of another, and save it
  with your templates (see [`template`](#template)).
- `apply <NAME> [-o <OUTPUT>]` — write the profile's project configuration (default:
  `devscriptor.yaml`, which it replaces).
- `delete <NAME>` — delete a profile you created. Built-in ones cannot be deleted.

---

### `validate`

Check a [project configuration](configuration.md): `FILE`, else the file `--config` names, else
`devscriptor.yaml` in the working directory. A setting that does not exist, or a value that
cannot apply, is an error (exit code 1); a doubtful setting, such as an extension Devscriptor
indexes no file for, is a warning.

```bash
devscriptor validate [FILE]
```

---

### `repl`

Interactive query session (REPL mode).

```bash
devscriptor repl
```

---

### `lsp`

Language server management (requires `lsp` feature).

```bash
devscriptor lsp <SUBCOMMAND>
```

Subcommands: `setup`, `detect`, `status`, `check`, `batch-check`, `trust`, `untrust`.

- `setup [TARGET]... [--yes]` — for a project (the default `.`), detect its languages and
  install the missing servers; for a server, named by id (`bash-language-server`) or language
  (`bash`), install it whether a project needs it or not. A name that is also a directory
  here is the directory.
- `detect [PATH]` — detect project languages and available servers without installing.
- `status` — show all discovered LSP servers grouped by source.
- `check <SERVER_ID>` — check whether a specific server is available.
- `batch-check <SERVER_ID>...` — check multiple servers at once.
- `trust [FILE]` — show the servers a project's `devscriptor-lsp.toml` (the default `FILE`)
  would start, then trust that exact content. An untrusted project file is ignored, since it
  names commands to run; editing it withdraws the trust. See
  [Language servers](lsp.md#configuration-file).
- `untrust [FILE]` — withdraw that trust.

---

### `definition`

Go to the definition of a symbol at a file position. Requires a live LSP server
for the file's language (see `lsp setup`).

```bash
devscriptor definition <FILE_PATH> -l <LINE> -c <CHARACTER>
```

`--line`/`-l` and `--character`/`-c` are **0-indexed**. `--character` counts UTF-16
code units, the LSP default: one per character, two for a character outside the Basic
Multilingual Plane such as an emoji.

---

### `references`

Find all references to a symbol at a file position. Requires a live LSP server.

```bash
devscriptor references <FILE_PATH> -l <LINE> -c <CHARACTER>
```

| Option | Default | Description |
| --- | --- | --- |
| `<FILE_PATH>` | required | Absolute path to the source file |
| `-l, --line` | required | Line number (0-indexed) |
| `-c, --character` | required | Character position (0-indexed) |
| `--include-declaration [true\|false]` | `true` | Include the declaration in results; `--include-declaration false` leaves it out |

---

### `mcp`

Launch the unified MCP server (JSON-RPC over stdio).

```bash
devscriptor mcp
```

This serves the 75-tool unified MCP server over stdin/stdout, by default through
the workspace's shared daemon; there is no TCP port and no `--port` flag.

Configure an AI agent's MCP client with `command: "devscriptor"` and
`args: ["mcp"]` (see [MCP server](mcp.md#mcp-client-configuration)).

**Database.** Unlike every other subcommand, `mcp` does *not* default to
`./devscriptor.db`. With no `--database`, it opens a shared, working-directory
independent database — typically `~/.local/share/devscriptor/default.db` — so an
agent can scan absolute paths from any cwd. Pass `--database` to override it:

```bash
devscriptor mcp --database /path/to/project.db
```

This means `devscriptor status` run in a project directory will not see a graph
built by the MCP server unless you point it at the same file with `--database`.

**Language servers.** None starts before the server answers; each starts when a tool
first needs it, or in the background once `lsp_arch_detect` finds its language, and
stops after 10 minutes unused. `--lsp-startup eager` starts every applicable one up
front instead:

| Option | Default | Description |
| --- | --- | --- |
| `--lsp-startup <lazy\|eager>` | `lazy` | When language servers start; overrides `DEVSCRIPTOR_LSP_STARTUP` and the `[lifecycle]` table of `devscriptor-lsp.toml` |

`DEVSCRIPTOR_LSP_IDLE_SECS` (0: never stop) and `DEVSCRIPTOR_LSP_MAX_SERVERS` (0: no
cap) set the idle timeout and a cap on running servers; see
[Language servers](lsp.md#lifecycle).

**Watching.** Started inside a project a scan indexed, the server watches it, so the
index follows file changes while it runs; started before the scan, it watches the
project once the scan is done. It watches the deepest root a scan (`scan`,
`scan_codebase`, `start_analysis`) recorded in the database that contains its working
directory; see [Watcher](watcher.md#mcp-watching-by-default). On top of that, each
query tool re-checks the files behind its answer
([MCP server](mcp.md#answers-follow-the-files-on-disk)).

| Option | Default | Description |
| --- | --- | --- |
| `--no-watch` | off | Do not watch the scanned project the server runs in; `DEVSCRIPTOR_WATCH=0` does the same |

**One process per workspace.** By default `mcp` relays its stdin and stdout to the
workspace's daemon, [`serve`](#serve), and starts the daemon when none answers, so
every client of a workspace shares one index, watcher, embedding model and set of
language servers. The workspace is the working directory: clients started in one
directory with one database share a daemon. A daemon that `mcp` starts takes its
`--lsp-startup`, `--no-watch` and environment; one already running keeps its own.
When no daemon can be started or reached within 10 s, `mcp` serves in its own
process and says why on stderr. See
[MCP server](mcp.md#one-server-per-workspace).

| Option | Default | Description |
| --- | --- | --- |
| `--no-daemon` | off | Serve in this process, one server per client, instead of relaying to the workspace's daemon; `DEVSCRIPTOR_DAEMON=0` does the same |

Logs go to stderr — stdout carries the JSON-RPC stream and a single stray byte
there breaks strict MCP clients. A shared daemon logs to its own log, so the
warnings of tool calls are there rather than on the stderr of `mcp`.

---

### `serve`

Run a workspace's daemon: the MCP server that every `devscriptor mcp` started in
the workspace relays to.

```bash
devscriptor serve --workspace <dir> [--lsp-startup <lazy|eager>] [--no-watch]
```

`devscriptor mcp` starts it when needed; run it yourself to read its log in a
terminal, or to start it before its clients. It holds the index, the watcher, the
embedding model and the language servers once, and serves each connection as an
MCP session of its own, over a local socket (a named pipe on Windows). It writes
nothing to stdout.

| Option | Default | Description |
| --- | --- | --- |
| `--workspace <dir>` | required | The workspace to serve; the scanned root it lies in is watched |
| `--lsp-startup <lazy\|eager>` | `lazy` | When language servers start, as for [`mcp`](#mcp) |
| `--no-watch` | off | Do not watch the workspace's scanned root |

The global `--database` chooses the database as it does for `mcp`: with none, the
shared default. A daemon serves one workspace with one database, and
`devscriptor mcp` started in `<dir>` with the same database connects to it. A
second `serve` for the same pair exits at once with status 0.

It logs at `info` (`debug` with `--verbose`) to `<key>.log` in the runtime
directory, and to stderr too when that is a terminal; see
[MCP server](mcp.md#runtime-directory-and-logs). It stops on SIGTERM or
Ctrl-C, or when no session has been open for `DEVSCRIPTOR_DAEMON_IDLE_SECS`
seconds (default 600). Stopping, it stops its language servers and its watcher,
checkpoints the database, and removes its socket, sidecar and lock.

---

## Configuration

A project's `devscriptor.yaml` sets its scan scope, cross-file mode, embedding
selection and database pool, and environment variables tune the rest: see
[Configuration](configuration.md).

---

## Examples: Common Workflows

### First scan of a new project

```bash
cd my-project
devscriptor scan .
devscriptor status          # confirm entities and relationships
devscriptor search "main"   # test keyword search
```

### Add semantic search

```bash
devscriptor embed           # run in a terminal, can take minutes to hours
devscriptor find-similar "error handling pattern"
```

### Nightly incremental re-scan

```bash
devscriptor scan . --incremental   # only re-parses changed files
devscriptor embed                   # only embeds newly added entities
```

### CI pipeline (fast, no model)

```bash
devscriptor scan . --skip-embeddings
devscriptor analyze -p . -k dead-code -o json > analysis.json
```

### Large repo, first time

```bash
# Use bulk-load for faster FTS5 indexing on a fresh database
devscriptor scan . --bulk-load

# Then embed in the background and check on it
devscriptor embed --daemon
devscriptor embed --status
```
