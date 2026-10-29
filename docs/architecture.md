# Architecture

How Devscriptor is built: the crates, the indexing pipeline, the relationship
graph, how the index stays current, the database, search, and the MCP server's
processes. The reasons behind these choices are in
[Design decisions](decisions.md).

---

## Table of Contents

- [System Overview](#system-overview)
- [Module Architecture](#module-architecture)
- [Pipeline Data Flow](#pipeline-data-flow)
- [The Relationship Graph](#the-relationship-graph)
- [Keeping the Index Fresh](#keeping-the-index-fresh)
- [Database Schema](#database-schema)
- [Two-Phase Workflow](#two-phase-workflow)
- [Hybrid Retrieval Engine](#hybrid-retrieval-engine)
- [4-Tier LSP Detection System](#4-tier-lsp-detection-system)
- [LSP Bridge Lifecycle](#lsp-bridge-lifecycle)
- [MCP Server Processes](#mcp-server-processes)
- [Technology Stack](#technology-stack)

---

## System Overview

Devscriptor is a high-performance local code analysis platform built in Rust, providing:

- **26 languages** parsed with tree-sitter 0.27 — 26 grammar crates, 27 parsers, as the TypeScript crate provides both the TypeScript and TSX grammars — plus Markdown, a section at a time
- **Two-phase indexing**: fast structural scan + optional semantic embedding
- **75 MCP Tools** (32 Core + 38 LSP + 5 Architecture)
- **Real-time watcher** (`core::watcher`) keeping the graph current on file changes via the incremental pipeline
- **One process per workspace**: every MCP client of a workspace shares one index, watcher,
  embedding model and set of language servers (`devscriptor serve`)
- **4-Tier LSP Detection** maximising reuse of existing tooling
- **Parallel file discovery** via the `ignore` crate (same engine as ripgrep)
- **Vector Search** with local embeddings via sqlite-vec

```plaintext
┌─────────────────────────────────────────────────────────────────┐
│ devscriptor scan ./project                                       │
│   Phase 1: Discover → Parse → Store → FTS5 (fast, <30s)         │
└──────────────────────────────┬──────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────┐
│ devscriptor embed                                                 │
│   Phase 2: Load ONNX → Embed → Cache → Unload (~1.3 GB freed)   │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                ┌──────────────▼──────────────┐
                │  SQLite knowledge graph      │
                │  ┌─────────┐  ┌──────────┐  │
                │  │ FTS5    │  │ vec0     │  │
                │  │ keyword │  │ cosine   │  │
                │  │ search  │  │ search   │  │
                │  └─────────┘  └──────────┘  │
                └─────────────────────────────┘
```

---

## Module Architecture

### Workspace Layout

```plaintext
devscriptor/
├── crates/
│   ├── core/        # Library: parsing, DB, pipeline, search
│   ├── cli/         # devscriptor binary
│   ├── mcp/         # Model Context Protocol server
│   └── lsp/         # Language Server Protocol integration
└── vscode-extension/  # VS Code extension (TypeScript)
```

### Core Module (`crates/core/src/`)

```plaintext
crates/core/src/
├── lib.rs                      # Public API + re-exports
├── entities.rs                 # Entity types (Function, Class, Struct, …)
├── error.rs                    # Error + Result types
├── search.rs                   # CodebaseSearcher + Retriever (BM25 + vector, RRF fusion)
├── pipeline.rs                 # CodebasePipeline (streaming orchestrator)
├── watcher.rs                  # CodeWatcher — debounced incremental re-scans
├── analysis/                   # Static analysis engine
│   ├── api.rs                  # Analysis API surface
│   ├── dead_code.rs
│   ├── complexity.rs
│   ├── clone_detection.rs
│   ├── call_graph.rs
│   ├── code_smells.rs
│   ├── cross_reference.rs
│   ├── cross_file.rs
│   ├── architectural_patterns.rs
│   └── refactoring.rs
├── config/                     # The project configuration (devscriptor.yaml), templates, stack detection
├── context_graph/              # Notes, entity/concept links, note search
├── graph/
│   └── resolve.rs              # Edges derived from stored references (rebuild_edges_for_sources)
├── database/
│   ├── mod.rs                  # Database façade (public API)
│   ├── connection.rs           # r2d2 connection pool + pool cache; registers sqlite-vec
│   ├── schema.rs               # Table creation + FTS5 triggers + indexes
│   ├── operations.rs           # CRUD with prepare_cached; FTS5 entity search
│   ├── fts_query.rs            # Free text → FTS5 MATCH query (quoted, ORed, stemmed terms)
│   ├── embeddings.rs           # entity_to_text(), split_identifier_tokens()
│   ├── vector.rs               # sqlite-vec upsert, KNN search, embedding cache
│   ├── reindex_log.rs          # Which files each re-index run of the process covered
│   └── cache.rs                # Query result cache
├── embeddings/
│   ├── mod.rs                  # EmbeddingService, EmbeddingConfig (Jina 768-dim)
│   ├── selection.rs            # EmbeddingSelection / should_embed (which entities embed)
│   ├── provider.rs             # JinaEmbeddingProvider — the shared tract engine and its auto-tuner
│   ├── tract_backend.rs        # TractEmbedder — pure-Rust ONNX inference (tract-onnx)
│   ├── dequantize.rs           # Rewrites the quantized products to f32 at load
│   ├── model_fetch.rs          # Model download (Hugging Face, mirror fallback) + SHA-256 check
│   └── mock.rs                 # MockEmbeddingProvider (tests, no inference)
├── parser/
│   ├── mod.rs                  # TreeSitterParser; content-stable entity ids
│   ├── cache.rs                # ParserCache (Language cache, per-task Parser)
│   ├── constants.rs
│   ├── language_config.rs      # 27 language extension maps
│   ├── language_loader.rs      # tree-sitter language FFI loading
│   ├── entity_extraction.rs    # AST → Entity structs
│   ├── relationship_extraction.rs
│   ├── import_extraction.rs    # Import/dependency graph
│   ├── cross_file.rs           # CrossFileMode, SymbolResolution, built-in names
│   ├── symbol_table.rs         # Lightweight symbol index (SymbolTable): name resolution
│   ├── standard_methods.rs     # Standard types' methods, for calls through values
│   ├── prose.rs                # Markdown → documentation sections
│   ├── mmap_parser.rs          # Memory-mapped file reading
│   ├── cleaning.rs             # Entity normalisation
│   └── validation.rs           # Entity quality checks
├── processing/
│   └── scanner.rs              # FileScanner (ignore crate parallel walker)
└── utils/
    ├── gitignore_filter.rs     # .gitignore-aware path filter for stack detection (the scanner walks with the ignore crate)
    ├── memory_guard.rs         # Async-safe RAM pressure sentinel; free_ram_mb()
    ├── numbered.rs             # Line-numbered source excerpts
    ├── performance.rs
    ├── security.rs             # Path sanitization, file hash
    ├── type_helpers.rs
    └── validation.rs
```

### CLI Module (`crates/cli/src/commands/`)

| Command file | `devscriptor` subcommand | Notes |
| --- | --- | --- |
| `scan.rs` | `scan` | |
| `watch.rs` | `watch` | Initial incremental scan, then the real-time watcher |
| `embed.rs` | `embed` | |
| `search.rs` | `search`, `find-similar`, `search-function`, `search-class`, `search-pattern` | |
| `analyze.rs` | `analyze` | |
| `database.rs` | `status`, `clean`, `stats` | `stats` outputs JSON entity/rel/file counts |
| `doctor.rs` | `doctor` | |
| `call_graph.rs` | `call-graph` | |
| `architecture.rs` | `architecture-report` | |
| `context.rs` | `context` | |
| `repl.rs` | `repl` | |
| `lsp.rs` | `lsp`, `definition`, `references` | Proxy to a live LSP server |
| `mcp.rs` | `mcp` | Relays stdio to the workspace's daemon, starting it if none answers; serves in-process with `--no-daemon` (gated on the `mcp` feature) |
| `serve.rs` | `serve` | The workspace's daemon, which serves every MCP session of a workspace (gated on the `mcp` feature) |
| `performance.rs` | `perf-stats` | |
| `cache.rs` | `cache-stats` | |
| `template.rs` / `profile.rs` | `template`, `profile` | The configuration templates: write one as a `devscriptor.yaml`; `template analyze` suggests one from the project's stacks |
| `validate.rs` | `validate` | Checks a `devscriptor.yaml` |

**`definition` and `references` require a running LSP server.** They send requests to the
language server for the file's extension and return the server's response. The knowledge graph
DB and LSP capabilities are complementary surfaces — LSP for live navigation, DB for offline
analysis and semantic search.

---

## Pipeline Data Flow

`CodebasePipeline::analyze_directory()` processes a codebase in four phases:

### Phase 1 — File Discovery

```rust
FileScanner::scan(root)
  ignore::WalkBuilder::threads(min(num_cpus, 8))   ← parallel, gitignore-aware
  → Vec<FileEntry> { path, size, mtime, extension }
```

### Phase 1b — Incremental Filtering (when `incremental = true`)

```rust
db.load_scan_manifest()          → HashMap<path, (hash, size, mtime)>
For each discovered file:
  if size == manifest.size AND mtime == manifest.mtime → skip (fast path)
  else compute XXH3 hash:
    if hash == manifest.hash → skip (content unchanged despite mtime)
    else → parse this file
```

### Re-indexing

A scan into a database that already holds entities is a **re-index**, full or
incremental, and so is a re-index of given paths (`reindex_paths`, which the watcher
and the query-time freshness check use); all follow the same rules, and the result
equals a fresh scan of the tree (`tests/test_incremental_equivalence.rs`).

- **Removed files.** Manifest paths under the scan root that were not discovered
  and no longer exist on disk lose their entities, the edges on both sides, their
  vectors and their `files` rows, by every stored spelling of the path. Paths
  outside the root belong to other projects in the same database and are left
  alone.
- **A re-parsed file replaces the edges derived from it**, inside the insert
  transaction: the forward edges whose source is one of its entities and the
  reverse rows whose target is (`RelationshipType::is_reverse`). Edges other files
  hold into it, such as calls from an unchanged caller, stay: entity ids are
  [content-stable](decisions.md#content-stable-entity-ids), so they still point at the same entities. Its entities
  are upserted in place, and `reconcile_file_entities` removes the ones that
  vanished, with their edges and vectors. The transaction stores the nesting of its
  members in their types (`has_method`, `method_of`); its other edges come from its
  references in Phase 3.
- **References resolved again in Phase 3.** Besides the references of every file the
  run parsed, those of unchanged files that name a definition the run added, removed
  or changed are resolved again (see [Re-resolution](#re-resolution)): a call in an
  unchanged file to a function an edited file now defines is linked, and one to a
  function that gained a second definition links both, as a fresh scan does.
- **Vectors.** An entity keeps its vector unless its body changed; the vector of
  a changed body is deleted in the insert transaction, and the entity is embedded
  again on demand (`semantic_search`) or by `generate_embeddings` / `devscriptor
  embed`, the content-hash cache serving texts seen before.

### Phase 2 — Parallel Parsing

```rust
futures::stream::iter(entries)
  .map(|entry| parser.parse_file(entry.path))  ← spawn_blocking
  .buffer_unordered(max_concurrency)            ← default: num_cpus in prod, 2 in tests

For each ParsedFile:
  drop tree + content                               ← free AST memory immediately
  stored.extend(entity ids)                         ← the sources of Phase 3

  batch.push(FileRecord { file_info, entities, nesting edges, refs, imports })
  embed_texts.push(entity_to_text(entity))          ← accumulate for embedding

  if batch.len() >= file_batch_size (50):
    db.batch_insert_parsed_files(&batch)             ← single transaction
    flush_embedding_batch(service, texts, ids)       ← see below (only with --embed)
    batch.clear()
```

With cross-file analysis off (`CrossFileMode::None`, or `enable_cross_file = false`),
there is no Phase 3, and each file stores every edge its own pass resolves within it.

### Phase 3 — Relationships from References (streamed, bounded memory)

Phase 2 stores, with each file, every reference its entities make, resolved in the
file or not (`symbol_refs`): each call with the name it calls (the last name of the
callee: `add` in `calc.add(1)`) and its receiver, each name used as a value — a
constant or property read or written, a function passed — with what it is a member
of (`ReeferLimits` of `ReeferLimits.MAX_CELSIUS`), each type a signature names, each
supertype, the tables a foreign key names, and imported names. Phase 3 derives the
edges from them, a chunk of sources at a time (`CodebasePipeline::relink`, see
[The Relationship Graph](#the-relationship-graph)):

```rust
sources = every entity the run stored
        + entities under the root referring to a name it changed   ← re-index only
for chunk in sources.chunks(4096):
  refs       = db.get_symbol_refs_for_sources(chunk)
  imports    = db.get_file_imports(files of refs)
  candidates += db.get_symbol_entries_named(names not read yet, root)  ← kept
  for group in chunk, by sources, ≤ 32,768 references each:
    edges = resolve(group's refs) + their reverse rows   ← see Name Resolution
    db.replace_edges_of_sources(group, edges)  ← one transaction per group
                                                 (a fresh index only inserts)
```

Only one chunk's references, and one group's edges, are resident at a time: a chunk of
large test files can make ten times the references of another. The definitions and
imports a chunk reads are kept for the chunks after it, which name many of the same
ones, up to 500,000 definitions; past that the next chunk starts over, so memory stays
bounded at any repository size.

### Name Resolution

A reference (a call, a type, a supertype, an import, a foreign key) names its
target; resolution decides which entity that name means.

- **Phase 3** (`graph::resolve::resolve_reference` over a `SymbolTable` of the
  candidates): the first step that finds any definition decides: the same file, then
  the imported module (a file whose path contains the module's last segment), then
  the whole project under the scan root: definitions in a language whose code the
  caller's calls directly — its own, or one of its family: the JVM's, .NET's, C and
  those built on it (C++, CUDA, Objective-C, Swift), JavaScript's dialects and
  components — where those in the caller's language rank first and, among those,
  the ones sharing the most leading directories with the caller. A shell script
  naming `export_report` runs a program, not the Rust function of that name. Then
  external symbols and built-ins, which produce no edge. A call to a
  common built-in name (`print`, `len`, `log`, …), and in `Light` mode every call,
  resolves only to a definition in the caller's file.
- **Per-file pass** (Phase 2, `relationship_extraction`): a name resolves to the
  definitions of that name in the same file. Its edges are stored only when
  references do not resolve across files (cross-file analysis off).

A name in a type position (a supertype, an `impl … for`, a parameter, return or
field type) prefers type definitions (class, struct, enum, interface, trait, type
alias, union, protocol, contract) over any other definition of the name at each
step, so a constructor named after its class (Java, C#, C++, Dart) never competes
with the class.

When several definitions tie (overloads, or one name defined in equally near
files), every one of them is a candidate, ordered by file path, then start line,
then entity id. File paths inside a tree sort the same in every checkout, so the
id, which hashes the absolute path, decides only between definitions that start
on the same line. The reference is linked to the first `ambiguous_fanout`
candidates (`PipelineConfig`, default 3; 1 links only the first). Each of those
edges, forward and reverse, carries `{"ambiguous": true, "candidates": <tied>}` in
its metadata, and `get_relationships` reports it as `ambiguous: true`, so dead-code
and impact analysis see every possible target and can tell it from a certain one.

### Phase 3b — FTS5 Rebuild (bulk-load mode only)

```rust
db.end_bulk_load():
  INSERT INTO entities_fts(entities_fts) VALUES('rebuild')  ← single O(N) pass
  CREATE TRIGGER entities_fts_insert …                       ← restore sync triggers
```

### Phase 4 — Orphaned Vector Cleanup

```rust
db.cleanup_orphan_vectors()    ← removes entity_vectors rows with no matching entity
```

### `flush_embedding_batch` Detail

```rust
flush_embedding_batch(service, texts, ids):
  1. Compute XXH3 hash per text
  2. db.lookup_embedding_cache(hashes)     → HashMap<hash, Vec<f32>>
  3. For cache hits  → db.upsert_embeddings_batch(hit_items)   [no inference]
  4. For cache misses:
     chunks of embedding_batch_size (16),
     buffer_unordered(session_count) — two chunks at once on the shared model by default:
       spawn_blocking → service.generate_embeddings_batch(texts)
       db.upsert_embeddings_batch(items)
     Collect (hash, Vec<f32>) pairs
     db.store_embedding_cache(all_new_pairs)
  5. If unload_model_after_scan: service.unload_model()
```

---

## The Relationship Graph

Edges are a function of two stored things: the **references** each entity makes and
the **definitions** in the index. Any set of entities can have its edges derived
again from those alone, so a re-index of a few files leaves the graph a fresh scan
of the tree builds (`tests/test_incremental_equivalence.rs`).

### References

Every call, use of a name as a value (a constant or property read, a function passed),
type use, supertype and import an entity makes is a row of `symbol_refs`,
whether it resolves or not: its source entity, its kind (the forward edge's
`RelationshipType`), the name it targets, a qualifier (receiver or module, when the
parser has one), its language and line. Code outside every entity — the top of a
script, the callbacks a test file hands its test runner — is its file's module's:
a Python file's `module`, a Vue or Svelte file's component, and in other files a
`module` entity named after the file and spanning it, made when that code calls
something (an import alone makes none). Each file's imports are rows of
`file_imports`: the module, the name imported, and the name it binds in the file.
Both are replaced with their file's entities, in the same transaction.

### Derived edges

An edge is either the nesting of a member in its type (`has_method`, `method_of`),
stored with its file, or a resolved reference: `calls`, `uses`, `inherits` or
`implements`, with its reverse row (`called_by`, `used_by`, `inherited_by`,
`implemented_by`). `graph::resolve::rebuild_edges_for_sources` derives the edges of
a set of sources: it resolves their references against the definitions their names
and their files' imports name ([Name Resolution](#name-resolution)), and replaces
their forward edges, nesting excepted, and the reverse rows whose target they are.
Entities just stored have no such edges yet; for them
`derive_edges_for_new_sources` only inserts, as a fresh index does.
Only the entities under the scan root are candidates, as a scan of that root sees
them: the database may hold other projects, and a reference in one never links into
another.

### Re-resolution

Every run, a scan or a re-index of given paths, derives in Phase 3 the edges of every
entity it stored. When the index already held entities under the root, it also
derives again those of every other entity there with a reference to a name whose
definitions the run changed. It reads the names and ids of the definitions in its
files before it writes them and after (`Database::get_definitions_in_files`); a name
changed when its definitions, in resolution order, differ: added, removed, other
entities (another kind or signature) or reordered. A reference counts when it names
that name, or the alias an import of its file binds to it
(`from geometry import helper as aid`). Every other reference resolves as it did, so
it is left alone. A fresh index skips the two reads: every entity under its root is
one the run stored. The files of the entities resolved again join the run's record
for the query-time check.

The cross-file mode decides what resolves across files: `Deep` every reference,
`Light` every one but calls, which link only a definition in the caller's file, and
`Auto` is `Light` above 500,000 entities under the root and `Deep` otherwise.

An index records the schema version it was written with; one of another version
has its index tables dropped and made again when it is opened, and the next scan
fills them (the Context Graph's notes are kept).

## Keeping the Index Fresh

Answers should match the files on disk without the agent doing anything. Three
layers keep them so, each built on the one before.

### Re-indexing given paths

`CodebasePipeline::reindex_paths(root, paths)` re-indexes files known to have
changed, without walking the tree, by the [re-index rules](#re-indexing):

- A path is re-indexed only if a scan of `root` would index it.
  `FileScanner::entries_for` checks the file, and each directory between it and
  `root`, against a one-level listing of its parent made with the scan's own
  walker, so the two cannot disagree on ignore files, hidden directories and
  excludes.
- A file whose size and modification time match its manifest row is skipped, and
  one whose content hash matches only has those refreshed. A path that no longer
  exists, file or directory, is removed with what the manifest holds under it.
- Calls and type uses are resolved against everything stored under `root`. The
  pipeline reads those entities once and keeps them for its next run (the symbol
  cache), updated with each run's changes; a full scan drops them, and so does a
  run that fails or is cancelled. Another writer may have removed an entity the
  cache still holds, so a cross-file edge is inserted only where both ends exist.

### The watcher

`CodeWatcher` ([Watcher](watcher.md)) watches each directory a scan of the
root descends into, non-recursively, and watches new ones as they appear. It
batches the source files that change within a debounce window and re-indexes
them with `reindex_paths`; when events were lost it runs a full incremental scan.
Where native events cannot be had, because no watcher can be created or the
system's limit on watches is reached, it polls the same directories.

Every full scan adds its canonical root to `metadata.indexed_roots`. The MCP
server starts a watcher in the background on the deepest of those roots that
contains its workspace, unless `--no-watch` or `DEVSCRIPTOR_WATCH=0` say
otherwise. Its workspace is the daemon's `--workspace` or, with
`devscriptor mcp --no-daemon`, the working directory. A server started before a
scan indexed its workspace keeps it (`watch_workspace`) and watches it once one
has: after `scan_codebase` and `start_analysis`, and at a tool call, at most every
10 s, for a scan made by another process. `stop_watching` forgets the workspace.

### The query-time check

A watcher lags by its debounce and its re-index, and it may poll, fail, or be
off. So the MCP query tools check their own answers (`crates/mcp/src/freshness.rs`).
`CoreModule::handle_tool` runs the query and collects the `file_path`s the answer
names; for `get_relationships`, the files of the entity and of each edge's other
end. It then calls `ensure_fresh`, which:

1. compares each file's size and modification time with its manifest row;
2. re-indexes the changed, deleted, and new-but-indexable files, each under the
   deepest indexed root that contains it, at most 20 per answer;
3. waits for that at most `DEVSCRIPTOR_FRESHNESS_BUDGET_MS` (250 ms). A re-index
   that takes longer goes on in a background task.

If any file was re-indexed, the query runs again, and its files are checked in
turn. A check alone can miss a file that another re-index (another answer's, the
watcher's) committed after the query read it and before the check: the file then
looks current. So `Database` records which files each re-index run of the process
covered, stamped once the run's last write is done (`reindexing`,
`reindexed_since`); an answer notes the stamp before its query, and runs again
while any of its files were re-indexed since, at most twice more
(`freshness::fresh_answer`). The answer reports `refreshed_files`, and
`stale_files` for files that changed but were not re-indexed: over a limit,
outside every indexed root, excluded by the scan rules, unparsable, or still
changing under it. The LSP position tools run the same check on the candidate
files of a symbol `name` before resolving it to a position.

The re-indexes go through one incremental, structural pipeline per server, which
a watcher without embeddings shares, so their runs never overlap: each derives its
edges from the index as the run before it left it.

---

## Database Schema

### Tables

| Table | Purpose |
| --- | --- |
| `files` | One row per scanned file; path is primary key |
| `entities` | All extracted code entities; polymorphic via `type` column |
| `relationships` | Directed edges between entities |
| `entity_vectors` | `vec0` virtual table — **always `FLOAT[768]`** f32 BLOBs (sqlite-vec); dimension is fixed at DB creation |
| `entities_fts` | FTS5 external-content table keyed by entity rowid (code-aware tokenizer) |
| `embedding_cache` | `text_hash → vector` — skips ONNX on re-scans |
| `embedding_jobs` | Background `embed --daemon` progress (`id`, `status`, `total`, `done`, timestamps) |
| `scan_manifest` | Incremental scan state: path, hash, size, mtime |
| `symbol_refs` | The references each entity makes, which edges are derived from |
| `file_imports` | Each file's imports: module, imported name, the name it binds, its line |
| `metadata` | Key-value store: the schema version, the embedding space, the scanned roots |
| `context_notes` / `context_notes_fts` | Context Graph notes and their FTS5 index |
| `context_entity_links` / `context_concept_links` | Note→entity and note→concept links |
| `note_vectors` | `vec0` table of the notes' embeddings |

### Key Indexes

```sql
idx_entities_name       ON entities(name)
idx_entities_file       ON entities(file_path)
idx_entities_type       ON entities(type)
idx_entities_lang_type  ON entities(language, type)  -- common multi-column filter
idx_rel_source          ON relationships(source_id)
idx_rel_source_type     ON relationships(source_id, relationship_type)
idx_symbol_refs_target  ON symbol_refs(target_name)   -- references to a changed name
idx_symbol_refs_source  ON symbol_refs(source_id)     -- a chunk's references
idx_file_imports_name   ON file_imports(imported_name) -- aliases of a changed name
```

### FTS5 Configuration

```sql
CREATE VIRTUAL TABLE entities_fts USING fts5(
    name,
    name_tokens,   -- pre-split camelCase/snake_case tokens
    body,
    content='entities',
    content_rowid='rowid',
    tokenize='unicode61 separators ''_.$/\-@#'''
);
```

Three sync triggers (`entities_fts_insert/delete/update`) keep the index current.
`COUNT(*)` on FTS5 external-content tables returns -1 by SQLite design; use a `MATCH` query
to confirm the index is populated.

---

## Two-Phase Workflow

### Why Two Phases?

Loading the ONNX model (~1.3 GB resident) during an interactive scan on 8 GB hardware
also running VS Code, rust-analyzer, and an LSP server (which together use several GB) would
leave insufficient headroom.
Separating scanning (always fast, always safe) from embedding (slow, memory-intensive) ensures
`devscriptor scan` is always usable.

### Phase 1: `devscriptor scan`

```rust
PipelineConfig {
    skip_embeddings: true,        // default; no ONNX loaded
    unload_model_after_scan: true, // CLI always sets this
    bulk_load: false,             // set to true for faster fresh scans
    incremental: false,           // set to true for re-scans
}
```

Available immediately after: FTS5 keyword search, entity/relationship queries, call graphs.

### Phase 2: `devscriptor embed`

```rust
1. Auto-tune (see performance.md, "The tuner"):
     batch_size  = 1 text per inference run (--batch-size overrides)
     concurrency = min(cores, 2), lowered while concurrency × batch_size × 100 MB > free − 2836 MB
     threads     = cores / concurrency matmul threads per run
   Load EmbeddingService (Jina 768-dim, INT8-quantized, ~150 MB on disk; downloaded on first use;
   its matrix products rewritten to run in f32)

2. db.get_unembedded_entity_ids(force=false)
   → entities of the semantic kinds missing from entity_vectors, largest first (idempotent)

3. chunks of min(max(batch_size, 16), ⌈n / (concurrency × 3)⌉) ids, buffer_unordered(concurrency):
     texts = db.get_entity_texts_for_ids(chunk)
     embeddings = service.generate_embeddings_batch(texts)  // 768-dim each
     db.upsert_embeddings_batch(items)       // into FLOAT[768] entity_vectors
     db.store_embedding_cache(pairs)         // written for the scan/watch/MCP paths, not read here

4. service.unload_model()   // frees ~1.3 GB immediately
```

Available after: semantic (cosine-distance) search via `find-similar` and `search --search-type semantic`.

---

## Hybrid Retrieval Engine

`CodebaseSearcher` (`crates/core/src/search.rs`) is the single retrieval path for the
code-search surfaces (CLI `search` and `find-similar`, the REPL, and the MCP code-search tools
such as `semantic_search` / `search_entities`). Name searches (`search_entities`, the CLI's
default `search`) match entity names with `LIKE`; the semantic surfaces go through
`Retriever::retrieve`, which fuses the two channels below. The Context Graph has its own separate note search
(`context_graph::ContextSearch`) and does not go through `CodebaseSearcher`.

### Channel A — FTS5 keyword search

```rust
operations::fts_search_entities(query, candidates, entity_type):
  fts_query::match_query(query)             // every term quoted and ORed, stop words dropped,
                                            // identifiers split into parts, stems prefix-matched
  FTS5 MATCH on entities_fts                // BM25 ranking via rank column
  JOIN entities ON entities.rowid = fts.rowid
  ORDER BY rank                             // BM25 order preserved end-to-end
```

Available immediately after `devscriptor scan`. No model required.

### Channel B — Vector cosine search

```rust
vector::search_similar_filtered(query_embedding, candidates, entity_type):
  sqlite-vec KNN on entity_vectors FLOAT[768]   // distance_metric=cosine
  similarity = 1 − distance                     // correct cosine mapping
  AND e.type = ? (when type specified)          // applied to the k nearest, not before
```

Available once `entity_vectors` contains rows. Vectors reach that table three ways:

| Path | Populates | When |
| --- | --- | --- |
| `devscriptor embed` (CLI) | every entity the embedding selection picks | one pass, up front |
| `generate_embeddings` (MCP tool) | the same set | one pass, up front — the MCP counterpart of the CLI command |
| On-demand top-K | only the BM25 candidates a query touches, of that set | during `semantic_search`, cache-first |

The policy is [`EmbeddingSelection`] (`embeddings.selection` in the project's
`devscriptor.yaml`, or `DEVSCRIPTOR_EMBEDDING_SELECTION`; default `standard`):
substantive named units get a vector; constants, config and index entries do not.
The passes that pick entities in SQL take it as `selection::sql_condition`, the same
policy `should_embed` applies entity by entity. Skipped entities remain findable — the retriever fuses a BM25
channel over `name` / `name_tokens` / `body` with the dense one, so an entity
without a vector still comes back through the keyword half. See
[Performance](performance.md#embedding).

On-demand embedding is opt-in per searcher via
`CodebaseSearcher::with_on_demand_embedding` and is **enabled on the MCP server**.
Without it an agent that scans a codebase and immediately calls `semantic_search`
gets a purely lexical result, because `scan_codebase` does not embed. The CLI
leaves it off, so `find-similar` and `search -s semantic` keep their cost profile
and only query vectors that a prior `embed` produced.

### Fusion

Both channels are fused using **Reciprocal Rank Fusion** with k=60:

```text
rrf_score(d) = Σ  1 / (k + rank_in_channel)
               channels
```

Each channel fetches `max(3 × limit, 20)` candidates. Channel A always runs; channel B
runs when the searcher has an embedding service and contributes whatever vectors exist,
so with no vectors the result is channel A alone. There is no mode switch. The fused
scores are divided by the top score, so the best result's relevance is 1.0.

Semantic search is therefore **non-blocking**: `devscriptor scan` gives a usable index
immediately; `devscriptor embed`, the `generate_embeddings` MCP tool, or on-demand
top-K embedding backfill vectors and promote channel-B results into the ranked mix
without any schema or config change.

---

## 4-Tier LSP Detection System

Language servers are found in priority order: a configuration file (authoritative
when present), Devscriptor's own installs, the system, and editor extensions; a
missing one is installed on request. See
[Language servers](lsp.md#finding-a-server).

---

## LSP Bridge Lifecycle

The `Translator` (`crates/lsp/src/bridge/translator.rs`) holds one client per language
and starts, restarts and stops servers. Its settings are `LifecycleConfig`, the
`[lifecycle]` table of `devscriptor-lsp.toml`, overridden by `DEVSCRIPTOR_LSP_*` variables
and then by `devscriptor mcp --lsp-startup` (`LifecycleConfig::resolve`).

```text
devscriptor mcp start ── eager ──▶ spawn_applicable(None) before `initialize` is answered
                     └─ lazy (default) ─▶ nothing
file request ─▶ open_file ─▶ start_server_for_file (rooted at the file's project)
                           └─ restart_if_exited (3 crash restarts per 5 min)
workspace request, none running ─▶ spawn_applicable(None | Some([language]))
lsp_arch_detect ─▶ startable_languages ─▶ tokio::spawn(spawn_applicable(Some(languages)))
every spawn ─▶ make_room: stop the least recently used beyond max_running_servers
spawn_maintenance (weak task, every maintenance_tick_ms)
    └─ stop_idle_servers: unused ≥ idle_timeout_secs ─▶ stop_language(Idle)
```

**Unused** is measured by the client. Every request, notification and readiness wait
holds `LspClient::in_use`, as does `open_file` until the document is open, and
`idle_for` is `None` while one is held, otherwise the time since the last ended. A
timestamp per language would let a request whose cold start outlasts the timeout lose
its server under it. Stops go through one path, `stop_language` (graceful shutdown,
client, folders and documents dropped), with a `stop_reason` that `get_server_status`
reports: `requested`, `idle` or `cap`, and `not started` for a server that never ran.
Idle and cap stops are not crashes and leave the restart budget untouched.

Some servers get settings of their own ([Language servers](lsp.md#what-servers-write-into-a-project)):
rust-analyzer builds in `target/rust-analyzer` (`cargo.targetDir: true`), off the
project's build lock, and resolves into a lock file there, and what a server writes
into a project that no option moves (`.metals/`, Gradle's `.gradle/`) is made ignored
by git before it starts (`lsp::lifecycle`). GraphQL is a strict-root language, as Rust,
C#, F# and Scala are: without a GraphQL config above it, a file has no project root and
its request fails at once. Every LSP result with a position carries
`cite`, `path:line:column` counted from 1, beside the LSP `range`.

---

## MCP Server Processes

MCP clients start `devscriptor mcp` and speak the Model Context Protocol to it over
**stdio** (JSON-RPC on stdin/stdout). By default that process is a proxy: the server
runs once per workspace, in `devscriptor serve`, and every client of the workspace
shares it.

| Entry point | Started by | Role |
| --- | --- | --- |
| `devscriptor mcp` | Every MCP client: AI agents, the VS Code extension and, through it, VS Code's AI assistants | The proxy: relays its stdio to the workspace's daemon, starting the daemon when none answers. With `--no-daemon` or `DEVSCRIPTOR_DAEMON=0`, it is the server, in its own process |
| `devscriptor serve --workspace <dir>` | The proxy, or a person | The daemon: holds the shared state and serves each connection as an MCP session |

```plaintext
 MCP client ─stdio─▶ devscriptor mcp ─┐
 MCP client ─stdio─▶ devscriptor mcp ─┼─▶ <key>.sock ─▶ devscriptor serve --workspace <dir>
 MCP client ─stdio─▶ devscriptor mcp ─┘   (a named pipe     one MCP session per connection;
                       (proxies)           on Windows)      one index pool, watcher, embedding
                                                            model and set of language servers
```

### Shared state

The daemon makes once what a server in each client's process would make each time:
the database pool and the embedding service (`DevscriptorServerState`), the
language-server `Translator` with its maintenance task, and the watcher of the
scanned root its workspace lies in. Each accepted connection is an MCP
session served by a clone of one server value, which shares those `Arc`s. Sessions
are independent, and the state they share is synchronized as it is for concurrent
tool calls in one session:

- A language server starts once per language for all sessions, and its idle time
  counts the requests of all of them.
- The watcher and the query-time check re-index through the daemon's one
  structural pipeline, whose runs never overlap. The files each run covered are
  recorded in the process, so every session's answers see the re-indexes of every
  other.
- `start_watching` and `stop_watching` act on the one watcher of the daemon.
- `health_check` reports the daemon's process id as `daemon_pid`, the same in
  every session it serves; an in-process server leaves it out.

### Endpoint and runtime directory

A daemon serves one workspace with one database. Its key is the first 32 hex
digits (128 bits) of the SHA-256 of the canonical workspace path, a NUL byte, and
the canonical database path; the full 64 would put a socket path past the 104 bytes
macOS allows. A proxy's workspace is its working directory, and its database is the
global `--database`, else the shared default (see
[MCP server](mcp.md#running-the-mcp-server)). Clients started in the same
directory with the same database share a daemon.

The daemon's files are named by its key, in the runtime directory:
`$XDG_RUNTIME_DIR/devscriptor/`, else `~/.cache/devscriptor/run/` on Linux and
macOS, and `devscriptor\run\` in the local application data directory on Windows.
On Unix the directory is made private to the user (mode 0700).

| File | Contents |
| --- | --- |
| `<key>.sock` | The daemon's Unix socket, mode 0600. On Windows the daemon listens on the named pipe `\\.\pipe\devscriptor-<key>` instead |
| `<key>.lock` | Locked by the daemon for its whole life (`std::fs::File::try_lock`) |
| `<key>.json` | The sidecar: the daemon's `pid`, `version`, and `started` (Unix seconds) |
| `<key>.log`, `<key>.log.1` | The daemon's log, at `info`. Past 10 MB it is renamed to `<key>.log.1` and started afresh |

### The proxy

`devscriptor mcp` (`crates/cli/src/commands/mcp.rs`):

1. computes the endpoint;
2. stops a daemon of another Devscriptor version (the sidecar's `version`) that
   holds the lock: SIGTERM on Unix, `taskkill` on Windows; it waits up to 10 s for
   the lock to be released;
3. connects. When nothing answers and no daemon holds the lock, it starts
   `devscriptor --database <db> serve --workspace <cwd>` detached, in a session of
   its own on Unix (`setsid`) and as a detached process on Windows, with its output
   appended to the log, and connects as soon as the daemon listens, within 10 s;
4. copies bytes between its stdio and the connection
   (`tokio::io::copy_bidirectional`) until either end closes. It parses no JSON and
   writes nothing else to stdout.

When no daemon can be started or reached within 10 s, the proxy serves in its own
process and says why on stderr. A daemon a proxy starts takes the proxy's
`--lsp-startup` and `--no-watch`; one already running keeps its own.

### The daemon's life

`devscriptor serve` (`crates/mcp/src/daemon.rs`) takes the lock first. A second
daemon for the key cannot, and exits at once with status 0, leaving its proxy to
connect to the first. A daemon removes its lock file when it stops; one that opened
the file just before, and locks it once released, would hold a lock no other daemon
sees. So a daemon keeps a lock only on the file the path still names, and otherwise
takes the lock again. The daemon then writes the sidecar, makes the server, and only
then listens, so a client that connects is served.

It stops when no session has been open for `DEVSCRIPTOR_DAEMON_IDLE_SECS` (600), or
on SIGTERM or Ctrl-C. Stopping, it shuts down the language servers, stops the
watcher, checkpoints the database, and removes its socket, sidecar and lock. A
daemon that dies without that leaves files the next one replaces: its socket is
removed before the new one listens, a sidecar counts only while its daemon holds the
lock, and a lock dies with its process.

### Who may connect

Both ends check the other:

- **The daemon serves only its own user.** On Unix the socket lies in the user's
  private directory with mode 0600, and the daemon closes a connection whose peer
  credentials (`SO_PEERCRED` on Linux, `getpeereid` on macOS) name another user. On
  Windows the pipe refuses remote clients, and its default security lets other
  users open it only for reading, so they cannot send a request.
- **A client connects only to its user's daemon.** On Unix the process serving the
  socket must run as the user. On Windows, where any user may create a pipe of a free
  name, the process serving the pipe must be the daemon the sidecar names, and a
  daemon serves no pipe another process made first.
- A proxy stops another process only by the pid in the sidecar of a daemon that
  holds the lock, and the system lets it stop only the user's own.

---

## Technology Stack

| Component | Technology | Version |
| --- | --- | --- |
| Language | Rust (edition 2024) | 1.95+ |
| Parsing | tree-sitter | 0.27 |
| Database | SQLite, bundled by rusqlite 0.40 | 3.53 |
| Vector search | sqlite-vec | 0.1.9 |
| Full-text search | FTS5 | (SQLite built-in) |
| Embeddings (runtime) | tract-onnx (pure Rust) | 0.23 |
| Embeddings (test) | MockEmbeddingProvider | built-in |
| Connection pool | r2d2 | 0.8 |
| File discovery | ignore (ripgrep engine) | 0.4 |
| MCP protocol | rmcp SDK | 3.5 |
| Async runtime | Tokio | 1.x |
| HTTP (model + LSP downloads) | reqwest, rustls + ring, bundled Mozilla roots | 0.13 |
