# Performance

How Devscriptor keeps scanning fast at any repository size, and embedding within a
4 GB memory budget. The decisions behind the numbers are in
[Design decisions](decisions.md).

- [Two-phase indexing](#two-phase-indexing)
- [Embedding](#embedding)
- [Parsing](#parsing)
- [The database layer](#the-database-layer)
- [Phase 3: edges from references](#phase-3-edges-from-references)
- [The file scanner](#the-file-scanner)
- [Full-text search](#full-text-search)
- [Vector search](#vector-search)
- [Incremental scans](#incremental-scans)
- [Tests](#tests)

## Two-phase indexing

| Phase | Command | Loads the model? | Search afterwards |
| --- | --- | --- | --- |
| Structural index | `devscriptor scan` | No | Name and FTS5 keyword search, every structural query |
| Semantic embedding | `devscriptor embed` | Yes, and unloads it when done | Keyword and vector search |

`devscriptor scan` never loads the embedding model: it parses, stores and indexes
in well under half a gigabyte and works on any hardware. The model takes about
1.3 GB while it embeds, so embedding is a separate, resumable, idempotent step,
which a user can run when the machine has the memory free.

| Command | Unloads the model when done |
| --- | --- |
| `devscriptor embed` | Yes |
| `devscriptor scan --embed` | Yes |
| `devscriptor watch --embed` | No: it keeps the model for the next change |
| The MCP server | No: it keeps the model for the next query, saving a reload of tens of seconds |

## Embedding

### The model

| Property | Value |
| --- | --- |
| Model | `jinaai/jina-embeddings-v2-base-code`, the INT8-quantized ONNX export (`onnx/model_quantized.onnx`) |
| Download | ~160 MB, into `<cache>/devscriptor/models/jina-embeddings-v2-base-code/` |
| In memory | One shared `tract` plan, ~1.25 GB, its weights dequantized to f32 at load |
| Vectors | 768 dimensions, f32; `entity_vectors` is `FLOAT[768]` with `distance_metric=cosine` |

### The tuner: runs, texts per run, threads

The model is loaded once and shared; `TractEmbedder`'s `run(&self)` borrows it
immutably, so concurrent runs share one plan and the only lock is a brief read to
clone an `Arc`. Three functions of `JinaEmbeddingProvider` size the work, each
overridable from the environment and fixed to 1 under `cfg!(test)`:

- **`compute_session_count()`, the runs at once.** Two (`PREFERRED_SESSIONS`); one
  on a single core, or when two runs would not fit the budget
  `free − HEADROOM_MB (1536) − MODEL_BASE_MB (1300)`, a run costing
  `texts × PER_ITEM_MB (100)` (~86 MB measured for a 512-token text, rounded up).
  It never risks running out of memory, even when every entity is 512 tokens.
  Override: `DEVSCRIPTOR_EMBEDDING_SESSIONS` (1–8).
- **`compute_batch_size()`, the texts per run.** One. A run is padded to its
  longest text, and real code varies in length, so batching is slower: on a
  release build, 5.6 texts/s with one text per run, against 3.2 in batches of 4,
  2.3 of 8 and 2.0 of 16, and 1.36 GB peak against 2.18 GB in batches of 16.
  Override: `embed --batch-size N` or `DEVSCRIPTOR_EMBEDDING_BATCH`; the runs at
  once are then sized for that many texts.
- **`compute_thread_count()`, matrix-multiplication threads per run.**
  `cores / runs`, so the runs together use every core at no extra memory. Override:
  `DEVSCRIPTOR_EMBEDDING_THREADS`.

On 12 cores this gives 2 runs × 6 threads: about 4.8 texts/s at 1.34 GB peak on
600 entities of real code. Batching changes no vector: the products run in f32, so a
text's vector does not depend on the texts beside it.

`devscriptor embed` takes the entities largest first, so the longest do not run
alone at the end while the other runs sit idle, and cuts the work into at least
three chunks per run (of at most 16 entities each), so a small job keeps every run
busy. Long, near-512-token entities embed about five times slower than short ones;
they are the case `PER_ITEM_MB` is sized for.

The budget depends on `free_ram_mb` (`crates/core/src/utils/memory_guard.rs`)
reporting the memory actually available; it converts sysinfo's bytes to megabytes.

### Concurrent flushes

`flush_embedding_batch` (`pipeline.rs`), the path of `scan --embed`, `watch --embed`
and the MCP server's embedding, fans the chunks out on the shared plan with
`buffer_unordered(session_count)`, asking the service for `session_count()` at
every flush, so the width is the tuner's (or the override's).

### The embedding cache

Before running the model, `flush_embedding_batch` hashes each text (XXH3) and looks
it up in `embedding_cache`; a hit is written to `entity_vectors` without inference,
and new vectors go into both tables. `devscriptor embed` and on-demand embedding do
the same (`embed --force` re-embeds regardless). Each text gets a run of its own,
so a cached vector is exactly what a new run would compute. A re-scan of unchanged
code serves every vector from the cache.

The cache and `entity_vectors` hold the vectors of one vector space, which the
index records (`embedding_space`); an index of another space drops both when it is
opened, so old and new vectors never mix.

### Memory

| Command | Resident memory |
| --- | --- |
| `devscriptor scan` | Under ~400 MB: no model |
| `devscriptor embed` | ~1.25 GB for the plan, plus each run's activations, at most ~100 MB for a 512-token text: 1.34 GB peak with two runs |

`scan --embed`, `watch --embed` and the MCP watcher's embedding need 4 GB free
(`MIN_INLINE_EMBED_FREE_MB`) and run without embedding below that; `devscriptor
embed` runs with less, one run at a time.

### Throughput

Embedding is bound by the CPU, so its time grows with the number of embedded
entities, not with anything the tuner can add: once the cores are busy, more runs
only add memory. A small or medium repository embeds in minutes to an hour or two;
a corpus of a million entities takes many hours on a CPU. The levers are embedding
fewer entities (the `standard` and `significant` selections) and embedding
incrementally, which `devscriptor embed` does: it backfills only what lacks a
vector, in any number of passes.

## Parsing

`ParserCache` holds `tree_sitter::Language` values, which are cheap to copy, and
each parse task makes its own `tree_sitter::Parser` from one, so files of the same
language parse in parallel with no shared lock. The pipeline parses with
`buffer_unordered(max_concurrency)` over `spawn_blocking` tasks (the core count by
default) and drops each file's syntax tree and text as soon as its entities and
references are extracted.

Under memory pressure the pipeline slows down instead of failing: `MemoryGuard`
compares the free memory with a threshold set by the total (512 MB below 4 GB of
RAM, 1 GB up to 8 GB, 2 GB up to 16 GB, 3 GB above) and returns a delay, which the
pipeline awaits with `tokio::time::sleep`, never blocking the async runtime.

## The database layer

**Every pooled connection** is set up in `connection.rs`'s `with_init`, because
these settings are per connection in SQLite:

```sql
PRAGMA journal_mode = WAL;      -- readers proceed during writes (set once, per file)
PRAGMA busy_timeout = 5000;     -- retry for 5 s on SQLITE_BUSY
PRAGMA synchronous = NORMAL;    -- fsync at checkpoints, not at every commit
PRAGMA cache_size = <scaled>;   -- 5, 10, 20 or 50 MB by total RAM (<4, 4–8, 8–16, >16 GB)
PRAGMA temp_store = FILE;       -- large temporary tables go to disk
PRAGMA foreign_keys = ON;       -- entities belong to files
```

**The pool** has twice the core count of connections, at least 16 and at most 64,
unless `DEVSCRIPTOR_DB_POOL_SIZE` or the project's `database.pool_size` sets it:
enough for concurrent parse tasks, embedding flushes, Phase 3 and an MCP server's
queries at once.

**Writes are batched:** `batch_insert_parsed_files` stores a batch of 50 files —
their rows, entities, references, imports and nesting edges — in one transaction
with cached statements, instead of one transaction per entity.

## Phase 3: edges from references

Phase 3 derives every edge but the nesting of members from the stored references
(`graph::resolve`), a chunk of 4,096 source entities at a time. For each chunk it
reads the sources' references (one `SELECT … WHERE source_id IN (…)` per 900 ids),
the imports of their files, the sources that extend a type, and the definitions
under the scan root of every name not read before (one query per 900 names). It
resolves and writes the edges in groups of sources with at most 32,768 references,
one transaction each. No bodies are read, and there is no query per entity.

Memory stays bounded at any repository size: one chunk's references and one
group's edges are resident at a time, and the definitions and imports already read
are kept for the following chunks, which name many of the same ones, up to 500,000
definitions. The groups matter in test-heavy code: in a Flutter checkout, three
chunks of widget tests make ten times the references of the others, about 400,000
edges each, and holding a whole chunk's edges with the copy its write takes would
raise the scan's peak from 730 MB to 1.3 GB. A fresh index inserts its edges
without looking for edges to replace. `devscriptor --verbose scan` logs where the
derivation spent its time ("Derived the edges of N sources: references read in …,
M definitions in …, resolved in …, written in …").

`scan.cross_file: auto` (the default) resolves calls across files below 500,000
entities under the scan root, and only within the caller's file above it
(`CrossFileMode::effective`).

## The file scanner

The scanner walks with `ignore::WalkBuilder` on `min(cores, 8)` threads, with
native `.gitignore` support, the walker ripgrep uses. The exclude globs are
compiled once into one `OverrideBuilder`, and each file is matched once
(`filter.matches(path, size)`).

## Full-text search

`entities_fts` is an FTS5 table over the entities' `name`, `name_tokens` and
`body`:

```sql
CREATE VIRTUAL TABLE entities_fts USING fts5(
    name, name_tokens, body,
    content='entities',
    content_rowid='rowid',
    tokenize='unicode61 separators ''_.$/\-@#'''
);
```

`name_tokens` holds the name split into its camelCase and snake_case parts
(`split_identifier_tokens`, at insert time), so a part of an identifier finds it.
`fts_search_entities`, the keyword channel of hybrid search, ranks in one query and
keeps BM25's order:

```sql
SELECT e.type, e.data FROM entities e
JOIN (SELECT rowid, rank FROM entities_fts WHERE entities_fts MATCH ?1 ORDER BY rank LIMIT ?2)
  AS fts ON e.rowid = fts.rowid
[WHERE e.type = ?3]
ORDER BY fts.rank
```

The `MATCH` text comes from `fts_query::match_query`, which quotes every term (so
no input is FTS5 syntax), ORs them, drops stop words, adds an identifier's parts
and prefix-matches word stems. Three triggers keep the table in step with
`entities`; `scan --bulk-load` drops them for a fresh scan and rebuilds the index
once at the end (`INSERT INTO entities_fts(entities_fts) VALUES('rebuild')`), which
is several times faster than a trigger per row.

## Vector search

`search_similar_filtered` asks `vec0` for the `k` nearest vectors and joins them
with `entities`, adding `AND e.type = ?` when a type is given:

```sql
SELECT e.type, e.data, v.distance FROM entity_vectors v JOIN entities e ON v.entity_id = e.id
WHERE v.embedding MATCH ?1 AND k = ?2 AND e.type = ?3
```

`vec0` answers the `MATCH … k` part over every vector first, and the join then
drops the neighbours of other types, so the filter narrows the results without
making the search faster, and a type that is rare among the `k` nearest returns
fewer than `k` rows. Hybrid search therefore asks each channel for
`max(3 × limit, 20)` candidates. sqlite-vec 0.1.9 compares the query with every
vector, so a query's time grows linearly with the vectors stored.

## Incremental scans

`scan_manifest` stores each file's path, content hash, size, modification time and
when it was parsed. An incremental scan, the watcher and the query-time check:

1. skip a file whose size and modification time match (no read at all);
2. otherwise hash it (XXH3), and skip it, refreshing its row, if the hash matches;
3. otherwise re-parse it: its entities are upserted by their content-stable ids,
   the ones that vanished are removed, and its references and own edges are
   replaced, without deleting the file first, so edges into it from unchanged files
   and the vectors of unchanged entities stay. Phase 3 then derives the edges the
   change may have touched.

## Tests

The workspace's tests use `MockEmbeddingProvider`
(`crates/core/src/embeddings/mock.rs`), which makes deterministic, unit-length
768-dimension vectors from an XXH3 hash, instantly and without the model. Tests
that need the real model's semantics are gated behind the `slow-tests` feature.
See [Building](building.md#tests).
