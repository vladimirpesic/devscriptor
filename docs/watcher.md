# Devscriptor Real-time Watcher

Keep the knowledge graph current automatically, as you code.

The **watcher** monitors a project directory and re-indexes the source files that
change — created, modified, deleted or renamed — so the code knowledge graph
(entities, relationships, FTS index) reflects the working tree without a manual
`devscriptor scan`. `devscriptor mcp` runs one by itself in the scanned project it
is started in, and on top of that every answer of its query tools checks the files
it names ([Answers check their files](#answers-check-their-files)).

## Table of Contents

- [How it Works](#how-it-works)
- [What Triggers a Re-index](#what-triggers-a-re-index)
- [Polling](#polling)
- [CLI: `devscriptor watch`](#cli-devscriptor-watch)
- [MCP: watching by default](#mcp-watching-by-default)
- [MCP Tools](#mcp-tools)
- [Answers check their files](#answers-check-their-files)
- [Embeddings](#embeddings)
- [Design Notes](#design-notes)

## How it Works

1. The watcher watches each directory a scan of the project descends into, one
   non-recursive watch per directory, through the OS-native `notify` backend
   (inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows). The
   scan's own rules decide which directories: ignore files, hidden directories and
   the built-in excludes, so trees like `target/` and `node_modules/` cost no
   watches. A directory created later is watched as it appears, and the files
   already in it are indexed.
2. Filesystem events are **debounced** (default 400 ms): a burst of edits — an
   editor "save all", a `git checkout`, a formatter run — collapses into a
   single re-index.
3. The source files that changed during that window are re-indexed together,
   without walking the tree (`CodebasePipeline::reindex_paths`):
   - a file is re-parsed only if its content changed (size and modification
     time first, then an XXH3 hash),
   - its entities are updated in place, the ones that vanished are removed, and
     its own edges (its calls, type uses, supertypes) are replaced,
   - edges **into** it from unchanged files are kept, and its calls into
     unchanged files are linked again,
   - a call in an unchanged file to a name whose definitions changed (a
     function the edit added, removed or defined a second time) is resolved
     again,
   - an entity keeps its embedding vector unless its body changed,
   - a deleted file, or every file of a deleted directory, is removed, and
   - a **new** file is added.
4. When the operating system drops events (an overflow) or the watcher reports an
   error, the next re-index is a full incremental scan of the project instead.

A re-index of an unchanged file is effectively free, and the graph it leaves
matches a fresh scan (see [Architecture](architecture.md#re-indexing)).

## What Triggers a Re-index

The watcher only reacts to **parseable source files** — those whose extension
maps to one of Devscriptor's supported languages (or a well-known filename like
`Makefile`). Everything else is ignored, which keeps the watcher quiet and
prevents feedback loops:

- Ignored directories: `.git`, `.hg`, `.svn`, `target`, `node_modules`, `dist`,
  `build`, `.venv`, `venv`, `__pycache__`, `.mypy_cache`, `.pytest_cache`,
  `.idea`, `.cache`.
- Ignored files: the knowledge-graph database and its `-wal`/`-shm` artifacts,
  logs, binary/data files, and editor swap/backup/temp files (`*~`, `*.swp`,
  `*.tmp`).

Because the database files are ignored, the watcher never retriggers on its own
writes. A file the scan rules exclude is never indexed, even when it changes. The
scan rules are the project's `devscriptor.yaml` scope (see
[Configuration](configuration.md)), with the flags above outranking it:
`devscriptor watch` reads the file when it starts, while the MCP server's watcher
takes an edit of it at the next tool call.

## Polling

Where native events cannot be had, the watcher polls the same directories every
2 seconds instead: when the native watcher cannot be created, or when the
system's limit on watches or watcher instances is reached, at start or later
when a new directory is registered. On Linux those limits are
`fs.inotify.max_user_watches` and `fs.inotify.max_user_instances`, which editors
and other watchers often use up. The watch keeps the index current, a little
later; its status says how it works (`mode: "poll"` and the `fallback_reason`,
which the CLI prints too).

Polling compares modification times in whole seconds, so an edit within the same
second as the file's previous change is noticed with the next one. An answer's
own check compares sizes as well ([below](#answers-check-their-files)).

## CLI: `devscriptor watch`

```bash
devscriptor watch <PATH> [OPTIONS]
```

Runs an initial incremental scan, then watches until you press **Ctrl-C**.

| Option | Default | Description |
| --- | --- | --- |
| `-i, --include <EXT,…>` | the project's, else all | Restrict to these file extensions; replaces `scan.include` |
| `-e, --exclude <GLOB,…>` | the project's | Exclude paths matching these globs; replaces `scan.exclude` |
| `--embed` | off | Re-embed changed entities inline after each re-index (requires ~4 GB RAM; keeps the model resident) |
| `--debounce-ms <MS>` | 400 | Quiet period before a re-index runs |

The database is selected with the global `--database` flag, as for `scan`.

```bash
# Watch the current project, structural updates only
devscriptor watch .

# Watch with a specific database and a snappier debounce
devscriptor --database ./devscriptor.db watch ./my-project --debounce-ms 250
```

Example output:

```text
👁  Devscriptor Watch
   Path      : ./my-project
   Database  : ./devscriptor.db
   Embeddings: structural only
   Debounce  : 250 ms
   Mode      : file events, 212 directories

✅ Initial scan: 1247 files, 3891 entities, 5120 relationships written
   Watching for changes… press Ctrl-C to stop.

🔄 Updated: 2 re-parsed, 0 unchanged, 14 entities, 37 relationships written (0.12s)
```

Each line counts that run alone: the files it re-parsed and skipped, the entities
it extracted from the re-parsed files, and the relationship rows it wrote. When
the watcher polls, the `Mode` line says so and why:
`Mode      : polling 212 directories (the system's limit on file watches was reached: …)`.

## MCP: watching by default

`devscriptor mcp` watches the project it runs in, so an agent's edits reach the
index without the agent asking. Under the workspace's shared daemon (the default,
see [MCP server](mcp.md#one-server-per-workspace)), the daemon watches its
workspace, once for all the sessions it serves. Every full scan — `devscriptor scan`,
`scan_codebase` and `start_analysis` — records its root in the database. Once the
server serves, it starts watching the deepest recorded root that contains its
working directory, in the background, with the default debounce and without
embeddings. A server started before any scan of its working directory watches it
once a scan has indexed it: right after `scan_codebase` or `start_analysis`, and,
for a scan made by another process (`devscriptor scan` in a terminal), at its next
tool call, which looks at most every 10 seconds. A server started outside every
scanned root watches nothing until then.

- `devscriptor mcp --no-watch`, or `DEVSCRIPTOR_WATCH=0` (or `false`), turns this
  off.
- `stop_watching` stops the watch for the rest of the server's life: a later scan
  does not start it again (`start_watching` still does).
- A watcher that fails to start is logged and leaves the server running;
  `get_watch_status` then reports `watching: false` with the reason in
  `auto_watch_error`.

## MCP Tools

The watcher is exposed as **3 core MCP tools**. They operate on the server's
shared database.

| Tool | Description | Parameters |
| --- | --- | --- |
| `start_watching` | Start watching a directory | `path` (required); `debounce_ms` (default 400); `embed` (default false) |
| `stop_watching` | Stop the watcher, and keep scans from starting it again | *(none)* |
| `get_watch_status` | Report status and last re-index stats | *(none)* |

**`start_watching`:**

```json
{ "path": "/path/to/project", "debounce_ms": 400 }
```

It answers once the watch is registered and an incremental scan has brought the
graph up to date, so every change made after the answer — and any made during
that scan — triggers a re-index. When the project directory itself cannot be
watched, it returns the error instead of `watching: true`.

One watcher runs per server. Asked for the root already being watched — the one
`devscriptor mcp` watches by itself, for instance — `start_watching` returns the
running watch's status with `already_watching: true`. Asked for another root, it
fails with an error that names the watched one; call `stop_watching` first.
The sessions of a workspace's shared daemon share its one watcher: what one starts
or stops, the others see.

**`get_watch_status` result:**

```json
{
  "watching": true,
  "path": "/path/to/project",
  "debounce_ms": 400,
  "embed": false,
  "mode": "events",
  "watched_directories": 212,
  "rescans": 7,
  "last_rescan": {
    "files_parsed": 2,
    "files_skipped": 0,
    "entity_count": 14,
    "relationship_count": 37,
    "seconds": 0.12
  }
}
```

`mode` is `events` or `poll`; a polling watch adds `fallback_reason`.
`watched_directories` counts the directories watched. `last_rescan` counts the
latest re-index alone: `entity_count` is the entities extracted from the files it
re-parsed, `relationship_count` the relationship rows it wrote. `get_statistics`
reports what the index holds. When nothing is watched, the status is
`{"watching": false}`, with `auto_watch_error` when the server's own watcher did
not start. See [MCP server](mcp.md) for the full tool list.

## Answers check their files

A watcher can lag: within its debounce, while a re-index runs, while it polls,
after it failed, or with `--no-watch`. So the query tools — `semantic_search`,
`search_entities`, `search_functions`, `search_classes`, `fuzzy_search`,
`query_with_context`, `get_entity_details`, `get_relationships`,
`get_file_context` and `describe_file` — and the `name` resolution of the LSP
position tools check the files of their answer against the disk before
answering. A file whose size or modification time differs from what the last
scan recorded is re-indexed, within a budget. If any was, or if the watcher
re-indexed one of them while the query ran, the query runs again.
The answer names the files it refreshed (`refreshed_files`) and those that may
be newer than it (`stale_files`). See
[MCP server](mcp.md#answers-follow-the-files-on-disk).

## Embeddings

By default the watcher performs **structural** updates only (no embedding model
loaded) — fast and low-memory, consistent with Devscriptor's two-phase design.
FTS5 keyword search and all structural queries stay current automatically.

With `--embed` (CLI) or `"embed": true` (MCP), the watcher also re-embeds changed
entities inline after each re-index and keeps the ONNX model resident so
interactive semantic search stays warm. This needs ~4 GB of free RAM. Both check:
below that the watcher runs without embeddings and still keeps the structural
graph current, and `start_watching` says why in `embed_skipped`.

## Design Notes

- The watcher re-indexes the changed paths through the same pipeline code as a
  scan, rather than editing the database per event, so deletion and rename
  reconciliation and cross-file relationship updates are handled by the same
  tested code path as `scan`.
- A single failed re-index (e.g. a transient parse error) is logged and does not
  tear down the watcher; the next change triggers another attempt.
- Changes that land **during** a re-index trigger the next one, so nothing is
  missed.
- In `devscriptor mcp`, a watcher without embeddings and the answers' own checks
  re-index through one pipeline, so the entities it resolves calls against
  between runs follow every change either makes.

## Related Documentation

- [CLI](cli.md) — the `watch` command in context
- [MCP server](mcp.md) — the 3 watcher MCP tools
- [Architecture](architecture.md#keeping-the-index-fresh) — how the watcher,
  `reindex_paths` and the query-time check fit together
- [Performance](performance.md) — incremental scan internals
