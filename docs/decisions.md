# Design decisions

The decisions that shape Devscriptor: what was chosen, how it works, and why.
Read the relevant section before changing one of these areas; a change to a
decision here is a change of design, not a fix.

- [Embeddings](#embeddings)
  - [One embedding model, 768 dimensions](#one-embedding-model-768-dimensions)
  - [Matrix products in f32 over the quantized weights](#matrix-products-in-f32-over-the-quantized-weights)
  - [Selective embedding](#selective-embedding)
  - [Content-stable entity ids](#content-stable-entity-ids)
  - [One shared model, concurrent runs](#one-shared-model-concurrent-runs)
- [The index](#the-index)
  - [Edges are derived from stored references](#edges-are-derived-from-stored-references)
  - [The index keeps itself current](#the-index-keeps-itself-current)
  - [The scan manifest is written on every scan](#the-scan-manifest-is-written-on-every-scan)
  - [An index of another schema version is rebuilt](#an-index-of-another-schema-version-is-rebuilt)
  - [Relationship types are canonical](#relationship-types-are-canonical)
  - [SQL tables are their own entity kind](#sql-tables-are-their-own-entity-kind)
  - [Parsing cannot crash a scan](#parsing-cannot-crash-a-scan)
- [Tools and processes](#tools-and-processes)
  - [MCP scans run the full pipeline](#mcp-scans-run-the-full-pipeline)
  - [`definition` and `references` ask a live language server](#definition-and-references-ask-a-live-language-server)
  - [Language servers start on demand](#language-servers-start-on-demand)
  - [One Devscriptor process per workspace](#one-devscriptor-process-per-workspace)
  - [The VS Code extension is an MCP client](#the-vs-code-extension-is-an-mcp-client)
  - [One project configuration file, of settings that act](#one-project-configuration-file-of-settings-that-act)
- [Code patterns](#code-patterns)
- [Deliberate choices that look like mistakes](#deliberate-choices-that-look-like-mistakes)
- [Duplicate dependencies](#duplicate-dependencies)

## Embeddings

### One embedding model, 768 dimensions

`jinaai/jina-embeddings-v2-base-code`, from its INT8-quantized ONNX export
(`onnx/model_quantized.onnx`), is the only embedding model. Its vectors have 768
dimensions, and `entity_vectors` is always `FLOAT[768]`. There is no fallback
model, no model chosen by the hardware, and no setting that changes the
dimensions.

**Why:** vectors of different dimensions cannot share a table, and vectors of
different models are not comparable, so one index holds one model's vectors.
Retrieval quality for code is the priority. A static embedding backend
(`model2vec`-style token-bag vectors) runs about 100× faster on a CPU but is
context-free: it cannot tell code apart by what it does, which code search depends
on, and a static model distilled from this one narrows the gap without closing it.
When embedding is too slow for a large corpus, the levers are embedding fewer
entities ([Selective embedding](#selective-embedding)), embedding them
incrementally ([Content-stable entity ids](#content-stable-entity-ids)) and faster
hardware, never a weaker vector.

### Matrix products in f32 over the quantized weights

The download is the 162 MB quantized file, but its matrix products run in f32: at
load, each `DynamicQuantizeLinear → MatMulInteger → Cast → Mul` chain of the
quantized graph is rewritten into one `MatMul` over its weights dequantized to f32
(`crates/core/src/embeddings/dequantize.rs`).

**Why:** measured on 600 entities of real code on 12 cores:

| Compute | Best layout | Texts/s | Peak RSS | Mean cosine to the unquantized model | Nearest-10 shared |
| --- | --- | --- | --- | --- | --- |
| Integer products (as quantized) | 8 runs × 1 thread | 2.24 | 0.85 GB | 0.9919 | 92.8% |
| **f32 over the dequantized weights** | 2 runs × 6 threads | 4.81 | 1.34 GB | 0.9963 | 96.9% |
| The unquantized model (641 MB) | 2 runs × 6 threads | 5.33 | 1.57 GB | 1 | 100% |

f32 doubles the throughput and brings the vectors closer to the unquantized
model's, for half a gigabyte of memory. The unquantized model would add little
more for four times the download.

**The vector space is recorded.** The index stores the space its vectors are in
(`embedding_space` in `metadata`, from `EMBEDDING_SPACE`: the model, its revision,
its weights and its arithmetic). An index of another space drops its vectors and
its embedding cache once, when it is opened, so vectors of two spaces never mix;
`devscriptor embed` and on-demand embedding make them again.

### Selective embedding

`EmbeddingSelection` (`embeddings.selection` in `devscriptor.yaml`, or
`DEVSCRIPTOR_EMBEDDING_SELECTION`; default `standard`) embeds only substantive
units — functions, classes, types, modules, documentation sections — and skips
non-semantic noise such as constants, decorators and indexes. Skipped entities stay
findable through keyword and name search. Every path that embeds (inline in a scan
or watch, `embed`, `generate_embeddings`, on-demand in `semantic_search`) applies
the one selection, through `should_embed` or its SQL form `sql_condition`.

**Why:** it trims embedding cost and improves precision without touching the model
or its dimensions: it changes which entities get a vector, never how.

### Content-stable entity ids

An entity's id is derived from what it is, never from where it sits:
`UUIDv5(ENTITY_ID_NAMESPACE, "file␟type␟name␟disambiguator␟ordinal")`
(`parser::assign_stable_ids`). Editing or inserting code elsewhere in a file leaves
an unchanged entity's id, its incoming edges and its vector as they were. The
namespace constant is fixed: changing it would orphan every vector of every index.

- Re-parsing a file **upserts** its entities in place. An entity keeps its vector
  unless its body changed; a changed body loses it in the same transaction and is
  embedded again.
- `entity_vectors` is a `vec0` table with no foreign-key cascade, so vectors are
  deleted explicitly: `delete_entities_by_file_paths` (removed files) and
  `operations::reconcile_file_entities` (entities that vanished from a re-parsed
  file) drop them, and no orphans accumulate.
- **Embedding is incremental**: a scan with the default selection embeds the
  substantive entities, and `devscriptor embed` (`get_unembedded_entity_ids`)
  backfills whatever still lacks a vector, in any number of passes. Search uses the
  vectors that exist and keyword ranking for the rest, so a partly embedded index
  is always usable.

### One shared model, concurrent runs

The embedding engine is the pure-Rust `tract` runtime, whose `run(&self)` is safe to
call concurrently, so the model is loaded once and shared; concurrency comes from
running several inferences on it at once. `compute_session_count()` gives two runs
(`PREFERRED_SESSIONS`), each with `cores / runs` matrix-multiplication threads, and
one when there is a single core or two runs (`batch × PER_ITEM_MB`, 100 MB each)
would not fit `free − HEADROOM_MB (1536) − MODEL_BASE_MB (1300)`. Each run embeds
one text (`compute_batch_size()` = 1). `DEVSCRIPTOR_EMBEDDING_SESSIONS`,
`DEVSCRIPTOR_EMBEDDING_THREADS` and `DEVSCRIPTOR_EMBEDDING_BATCH` override the three.

**Why:** the f32 products speed up with threads, and two runs at once keep the
cores busy between them: on 12 cores, 2 runs × 6 threads embedded 5.4 texts/s,
against 4.9 for 1 × 12, 4.7 for 3 × 4, 3.8 for 4 × 3 and 1.5 for 8 × 1. One text
per run because a run is padded to its longest text: 5.6 texts/s with one text per
run against 3.2 in batches of 4 and 2.0 in batches of 16, at 1.36 GB peak against
2.18 GB. The RAM budget scales the parallelism, never the model. See
[Performance](performance.md#embedding).

## The index

### Edges are derived from stored references

Every call, name used as a value, type use, supertype and import an entity makes
is stored with its file, resolved or not (`symbol_refs`, `file_imports`). Every
edge but the nesting of members in their types is derived from those references
and the definitions in the index, a chunk of sources at a time, by one function
(`graph::resolve::rebuild_edges_for_sources`). A scan derives the edges of every
entity it stored. A re-index — by a scan, the watcher or the query-time check —
also derives again those of every other entity under the root with a reference to
a name whose definitions it changed: added, removed, other entities, or reordered,
named directly or through an import alias. Resolution sees only the entities under
the scan root.

**Why:** an edge resolved once, when its source file is parsed, against the
entities parsed with it, can never be revisited by a re-index of another file: a
call to a function defined later stays unresolved, and a call to a function that
gains a second definition keeps one target, until a full scan. With the references
stored, the edges of any set of entities can be derived again from the database
alone, so a re-index leaves the graph a fresh scan builds, and fresh and
incremental scans share one code path. "Changed" compares the definitions in
resolution order, not just their names, because a second definition, or a function
becoming a class, changes how unchanged callers resolve while the name stays. The
definitions a chunk resolves against are read per chunk rather than held for the
whole project, so memory stays bounded ([Performance](performance.md#phase-3-edges-from-references)).

### The index keeps itself current

`devscriptor mcp` watches the scanned project it runs in, and every answer from
the index is checked against the files it names. Every full scan records its
canonical root in `metadata.indexed_roots`. Once the server serves, it watches the
deepest recorded root that contains its working directory, or, started before any
scan of it, does once a scan has indexed it; `--no-watch` and
`DEVSCRIPTOR_WATCH=0` turn that off. The watcher watches only the directories a
scan reads, polls where native events cannot be had, and re-indexes the changed
paths with `reindex_paths` instead of scanning the tree.

The query tools, and the `name` resolution of the LSP position tools, compare the
files of their answer with the scan manifest. They re-index those that changed, at
most 20, waiting at most `DEVSCRIPTOR_FRESHNESS_BUDGET_MS` (250 ms), and run the
query again if any were. The answer names the files refreshed for it
(`refreshed_files`) and those it may be older than (`stale_files`).

**Why:** an agent that edits files and then searches must see its edits, and
agents rarely ask for a watch: without one, sub-agents read the code back from
search previews as it was before the edit. A watcher alone does not close the
gap: an answer can come within its debounce, before its re-index finishes, while
it polls, after it failed, or with `--no-watch`. The per-answer check covers those
cases, and when nothing changed it costs one manifest lookup and a `stat` per
file. A recursive watch would spend a watch on every directory of the tree,
`target/` and `node_modules/` included, and run out of inotify watches where
editors hold most of them; watching only what a scan reads, and polling at the
limit, keeps the watch working there.

### The scan manifest is written on every scan

`pipeline.rs` writes the `scan_manifest` table after every scan, incremental or
not.

**Why:** the first scan of a project is a full one. If it did not seed the
manifest, the first incremental scan would find no baseline, treat every file as
changed, and re-parse and re-embed the whole repository.

### An index of another schema version is rebuilt

An index records the schema version it was written with (`schema_version` in
`metadata`). When a database written with another version is opened, its index
tables — files, entities, relationships, references, imports, the manifest, the
vectors, the embedding cache and the index's metadata — are dropped and made again,
empty, and the next scan fills them. The Context Graph's notes are kept.

**Why:** the index is derived data: a scan rebuilds it exactly. Upgrading it in
place would need a migration per change, and a partial upgrade risks rows that this
build reads differently from the build that wrote them. Notes are what a user or
an agent wrote, so they are never dropped with the index.

### Relationship types are canonical

Every comparison of a relationship type goes through `RelationshipType::parse(s)`
or matches enum variants; no comparison is written against a string literal such
as `"call"` or `"calledby"`.

**Why:** the stored form is `RelationshipType::as_str()`'s (`called_by`,
`method_of`, …). A hand-written string that differs from it by a character matches
nothing, and every analysis built on that comparison — call graphs, dead code,
architecture — would silently come back empty.

### SQL tables are their own entity kind

SQL `CREATE TABLE`, `CREATE VIEW` and `CREATE MATERIALIZED VIEW` produce an
`Entity::Table` (type `table`), not a class. The variant reuses the `Class` struct
(`Table(Class)`), so the `match_entity!` macro, cleaning, validation and JSON all
work unchanged; only the type string, the embedding text and the two type-string
reverse maps (`vector.rs`, `database/mod.rs`) know the kind. Columns are not
entities: they stay in the table's body, searchable and embedded with it.
`CREATE INDEX` is an `Entity::Index`.

**Why:** the embedding text starts with the kind, and `table orders …` lies nearer
a query about database tables than `class orders …` does. A `table` kind also keeps
two similar schemas out of code-clone detection, which compares functions, classes
and methods.

### Parsing cannot crash a scan

A single pathological file must never take down a whole scan. Two guards in
`parser/mod.rs` make tree-sitter parsing safe on any codebase:

1. **A large-stack parse thread.** `parse_content_large_stack` runs each parse on a
   dedicated `std::thread::scope` thread with a 256 MB stack
   (`constants::PARSE_THREAD_STACK_SIZE`; committed lazily, so it costs address
   space, not RAM). Tree-sitter recurses deeply in C on machine-generated or deeply
   nested input, and on a default ~2 MB stack that raises an uncatchable
   `SIGSEGV`/`SIGBUS` that aborts the process. The large stack turns those files
   into successful parses, and a Rust panic on the thread becomes a recoverable
   per-file error.
2. **A line-length guard.** `read_file_content` skips a file with a line longer
   than `constants::MAX_LINE_LENGTH` (2 MB), such as minified or generated bundles
   that drive tree-sitter into pathological time and memory, with a recoverable
   error.

**Why:** the crashes deep input causes are stack exhaustion, not file size or
memory mapping, so the fix is the stack, at the application level, rather than
excluding directories or capping file sizes: all 2,251 generated and native files
of the OpenJDK tree parse with these guards, without a crash. The extractors' own
recursion has a depth cap (`MAX_TRAVERSE_DEPTH`, 128); tree-sitter's is covered by
the stack.

## Tools and processes

### MCP scans run the full pipeline

The MCP tools `scan_codebase` and `start_analysis` run `CodebasePipeline`, as
`devscriptor scan` does: entities, references, edges, checksums, languages and
line counts.

**Why:** an agent scans and then searches. A scan that recorded only file metadata
would leave the graph empty and the manifest claiming every file as current, so
that a later incremental scan would skip them all.

### `definition` and `references` ask a live language server

The CLI's `definition` and `references` send their request to the language server
for the file's language and print its answer. They do not read the index.

**Why:** the knowledge graph is an offline, structural index, built for search and
analysis; navigation at a cursor needs the language server's live type system.
The two are complementary, and routing cursor navigation through the index would
give staler, less precise answers.

### Language servers start on demand

`devscriptor mcp` answers `initialize` without starting a language server. A
server starts when a tool first needs one of its files, when a workspace-wide tool
finds none running, or in the background once `lsp_arch_detect` detects its
language (`prewarming`), and stops after `idle_timeout_secs` (600) unused.
`max_running_servers` can cap how many run, stopping the least recently used.
Eager startup (`--lsp-startup eager`, `DEVSCRIPTOR_LSP_STARTUP`, or
`startup = "eager"` in `[lifecycle]`) starts every applicable server up front.
However a server starts, it gets the same folders: the workspace roots, or, for a
server that needs its project manifest at its root, the projects inside them; for
a file outside the workspace, the file's project. Such a server, and clangd, whose
compilation database lists only its folders' files, is started again to take in a
project outside them.

**Why:** a large polyglot workspace has many applicable servers, each up to about
1.8 GB, while most sessions use few languages; starting them all before the first
answer costs memory and time for nothing. The cold start moves to the first LSP
call of a language, which is why `lsp_arch_detect` warms the detected ones. Idleness
is measured by the client — no request, notification or readiness wait in progress,
and none for the timeout — never by a timestamp a long request could outlive. Idle
and cap stops are not crashes, and do not use up the three restarts in five minutes
a crashing server gets.

### One Devscriptor process per workspace

`devscriptor mcp` is a proxy. It relays its stdin and stdout, byte for byte, to the
server of its workspace and database, `devscriptor serve`, which holds the database
pool, the embedding service, the language servers (`Translator`) and the watcher
once, and serves each connection as an MCP session of its own
(`crates/mcp/src/daemon.rs`, `crates/cli/src/commands/mcp.rs`). The proxy starts
the server when none answers, stops one of another version first, and serves in its
own process when none can be reached within 10 s; `--no-daemon` and
`DEVSCRIPTOR_DAEMON=0` keep a server per client. The server locks `<key>.lock` for
its whole life, writes its pid, version and start time to `<key>.json`, listens
only once it can serve, and stops after `DEVSCRIPTOR_DAEMON_IDLE_SECS` (600) with
no session open, or on SIGTERM or Ctrl-C. [Architecture](architecture.md#mcp-server-processes)
describes the processes.

- **The key** is the first 32 hex digits of the SHA-256 of the canonical workspace
  path, a NUL byte and the canonical database path. 128 bits keep two workspaces
  apart; the full 64 digits would put `~/.cache/devscriptor/run/<key>.sock` past
  the 104 bytes macOS allows a socket path, in a home directory of ordinary length.
  The database counts because a server serves one: a client with another
  `--database` in the same directory gets its own.
- **The proxy relays bytes** and parses no JSON, so the protocol version,
  cancellation and every MCP feature pass between the client and the server
  untouched, and the proxy cannot write anything but the server's bytes to stdout.
- **Logs.** The server outlives its clients and has no terminal, so it logs to its
  own file at `info` (renamed to `.log.1` past 10 MB), and to stderr too when run in
  a terminal. A server a proxy starts has its stdout and stderr appended to that
  file, so a panic before logging starts lands there. The proxy's stderr, which the
  client shows, carries only the proxy's own messages.
- **One server per key.** A second server cannot take the lock and exits with
  status 0. A stopped server removes its lock file, so a server that opened the
  file just before its removal could lock a file no other server sees; a server
  therefore keeps its lock only on the file the path still names (device and inode
  on Unix, volume serial number and file index on Windows), and otherwise takes it
  again.
- **The client's version serves it.** A proxy stops a server of another version
  (the sidecar's), so an upgraded client is served by the code it ships with.
- **Both ends check the other** (see [Security](security.md#the-shared-servers-endpoint)).

Sessions share state as the concurrent calls of one session do: a language server
starts once per language and stays while any session uses it, `start_watching`
and `stop_watching` act on the one watcher, and `health_check` counts the analyses
of every session. The first client's settings hold: a server takes the
`--lsp-startup`, `--no-watch`, `--config` and environment of the proxy that starts
it, and later clients are served by it as it is.

**Why:** when every MCP client starts a server of its own, two agent sessions in
one repository, or an agent beside an editor, run two connection pools on one
index, two embedding models, two sets of language servers and two watchers
re-indexing every edit, while each answer sees only its own process's re-indexes.
One process per workspace makes each of these once, and every session's answers see
every re-index. The proxy keeps the clients' contract — the command they start,
stdio and the tools — and the in-process server remains the fallback, so no client
is left without a server.

### The VS Code extension is an MCP client

The extension runs each of its commands as an MCP tool call over one
`devscriptor mcp --database <db>` it starts in the first workspace folder
(`vscode-extension/src/mcpClient.ts`), so it shares the workspace's process with
the agents working there, and runs the equivalent CLI command when the connection
cannot be made. It spawns `devscriptor` with the `mcp` argument and piped stdio,
drains the child's stderr into its **Devscriptor** output channel, and listens for
spawn errors. Its default database is `devscriptor.db`, the one `devscriptor scan`
writes.

**Why:** one program answering to one name works on every platform: an alternate
entry point needs a symlink, which dangles when the binary is installed alone, or on
Windows a `.cmd` shim, which Node's `child_process.spawn` cannot run without
`shell: true`. An unread stderr pipe blocks the child once its buffer fills, and an
unhandled spawn error throws.

### One project configuration file, of settings that act

A project keeps its settings in `devscriptor.yaml` in its root
(`crates/core/src/config/project.rs`): the scan's scope, its cross-file mode, the
embedding selection and the database pool. See [Configuration](configuration.md).

- **Only settings that act.** Every field is read by every command it concerns, and
  a field the schema does not name is an error (`deny_unknown_fields`), so a typo is
  reported instead of passing for a setting. A setting that could act on nothing
  does not enter the schema.
- **Precedence**, highest first: a command's flags and an MCP tool's arguments, the
  environment, the file, the defaults. A flag replaces the file's setting rather
  than adding to it, so a command line reads the same in every project.
- **Errors stop a command, not a server.** A command with a file it cannot apply
  exits with the reason; a server keeps serving with the settings it has, and
  `health_check` reports the error under `project_config`.
- **A server follows edits.** The file is read again when its modification time or
  size changes, checked with a `stat` at every tool call
  (`crates/mcp/src/project.rs`); the pipelines the server keeps take the new scope
  between runs (`CodebasePipeline::set_scope`). A broken edit keeps the settings in
  force; only the pool size waits for a restart.

**Why:** settings that live only in flags and environment variables have to be
repeated on every command line, and the shared per-workspace server, which takes no
flags from later clients, could not be given them at all. A file whose fields might
not act misleads its reader.

## Code patterns

Deviations from these patterns are bugs.

**Errors.** `devscriptor-core` uses `thiserror` for typed errors.
`devscriptor-lsp` exposes a typed `LspError` at its public boundary, through
hand-written `Display` and `std::error::Error` impls, and uses `anyhow`
internally. The applications (`devscriptor-cli`, `devscriptor-mcp`) use `anyhow`.
Errors are propagated with `?` or logged with `tracing::warn!`, never dropped with
`.ok()` on a path that matters. A failure the user or the environment can cause is
returned as an error; `unwrap`, `expect` and `panic!` are kept for broken
invariants, such as a regular-expression literal that does not compile.

**Database access.**

- The database is shared as `Arc<Database>`; independent `Database` instances on
  one file compete for its locks.
- Every value is bound as a parameter (`rusqlite::params![]`), never interpolated
  into SQL. Hot queries use `prepare_cached`. Filtered searches assemble their SQL
  from fixed fragments, adding only placeholders.
- Writes of scanned data use `ON CONFLICT … DO NOTHING` / `DO UPDATE`, so re-scans
  are idempotent. `entity_vectors` (`vec0` has no `ON CONFLICT`) is upserted as a
  `DELETE` then an `INSERT`. Context Graph notes and links are plain `INSERT`s.
- Entities and relationships are read through the canonical mappers
  (`data_to_entity`, `row_to_entity`, `row_to_relationship`), and entity queries
  select only `type, data`: the entity is rebuilt from its `data` JSON.
- Timestamps (`OffsetDateTime`) are stored by passing `&value` to `params![]`,
  whose `ToSql` writes an RFC 3339 form; `value.to_string()` is not RFC 3339 and is
  never stored. `parse_db_timestamp` reads them, and reads a value that is not
  RFC 3339 as the Unix epoch with a warning rather than failing the whole read.
- A file-reading tool validates its path with
  `validate_file_path(path, Some(project_root), …)` and uses the canonical path it
  returns.
- Deleting entities deletes their vectors first (`vec0` has no cascade). That
  vector `DELETE` is best-effort: without vector support the table is absent, and a
  real database error surfaces on the entity `DELETE` that follows.

**Async.** CPU-bound work (inference, parsing, AST traversal) runs in
`tokio::task::spawn_blocking`; I/O is async. An async context sleeps with
`tokio::time::sleep`, never `std::thread::sleep`.

**Embedding.** Production searchers are built `with_embedding_service`;
`CodebaseSearcher::new(pool)` alone is for keyword-only uses. `flush_embedding_batch`
looks up the embedding cache before running the model. Tests that need vectors but
not semantic quality use `MockEmbeddingProvider`; tests that need the real model
are gated with `#[cfg_attr(not(feature = "slow-tests"), ignore)]`.

**Scanner excludes** all go into a single `OverrideBuilder`: one built per pattern
keeps only the last.

**`buffer_unordered` over a work set that can be empty** is floored at 1:
`futures::stream::buffer_unordered(0)` never polls its stream and hangs forever
(`spawn_servers_in` in `crates/lsp/src/lsp/lifecycle.rs` computes
`MAX_CONCURRENT_SPAWNS.min(pairs.len()).max(1)` for this reason).

## Deliberate choices that look like mistakes

- **`MockEmbeddingProvider` makes 768-dimension vectors from XXH3 hashes.** They
  are deterministic, unit-length and of the production dimension, with no model
  loaded; random or shorter vectors would be non-deterministic or fail the
  `vec0` column.
- **`compute_session_count` and `compute_thread_count` return 1 under
  `cfg!(test)`**, after the environment overrides, so unit tests do not run several
  inferences at once on the shared model, each adding its activations to its
  memory; concurrency is still testable through `DEVSCRIPTOR_EMBEDDING_SESSIONS` and
  `DEVSCRIPTOR_EMBEDDING_THREADS`.
- **`flush_embedding_batch` returns `(stored, cache_hits)`**: new vectors written,
  and vectors served from the cache without inference, both reported by
  `scan --metrics`.
- **Search scores are ranks, not cosines.** `entity_vectors` ranks by cosine
  distance (`distance_metric=cosine`), and entity search fuses that ranking with
  BM25's by Reciprocal Rank Fusion (k = 60), then scales the fused scores so the top
  hit is 1.0: a result's `similarity` is its relevance within one result set, not
  comparable across queries. Context notes report `1 − distance`, clamped to
  [0, 1]: a cosine similarity.
- **Objective-C `.m` files are recognized by content, not extension.** `.m` is not
  in the extension map, to avoid MATLAB: a `.m` file whose name says Objective-C is
  taken as it, and any other is parsed as Objective-C when its first 4 KiB hold
  `@interface`, `@implementation` or `#import`, and skipped otherwise. The
  language-server bridge tells them apart by the same markers and sends clangd only
  Objective-C.
- **Concurrent embedding does not scale linearly.** Inference is CPU-bound: once
  the cores are busy, more runs add no throughput (8 runs of one thread are more
  than three times slower than 2 runs of six), so the run count is not a speed
  knob. The tests assert
  the real guarantees — the configured concurrency is honoured, concurrency and
  batching give every text the vector it gets alone, and the RAM guard keeps the
  count in [1, 8] — not a throughput ratio.

## Duplicate dependencies

`cargo tree -d` lists some crates in two versions. The older version of each is
required by a dependency, so it goes only when that dependency moves on:

| Older version | Required by |
| --- | --- |
| getrandom 0.3, rand 0.9, itertools 0.14, bit-set 0.8 (fancy-regex 0.17) | tokenizers 0.23 |
| base64 0.13, nom 7 | spm_precompiled (via tokenizers) |
| bitflags 1 | lsp-types 0.97 |
| getrandom 0.2 | ring 0.17 |
| hashbrown 0.16 | string-interner 0.20 (via tract) |
| syn 2 | proc-macro crates: darling 0.20, derive_builder 0.20, derive-new 0.7, monostate 0.1, prost-derive 0.14, tracing-attributes 0.1, zerocopy-derive 0.8 |

Pairs of one version (memchr, serde, num-traits, …) are a build dependency and a
normal one with different features, not duplicates.
