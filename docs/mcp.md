# MCP server

Devscriptor serves its code intelligence to AI agents as one Model Context
Protocol server of **75 tools**. This page describes running the server and every
tool. The tables mirror the server's `tools/list`; parameters marked `*` are
required.

## Table of Contents

- [Overview](#overview)
- [Running the MCP Server](#running-the-mcp-server)
- [Getting all 75 tools](#getting-all-75-tools)
- [Tool annotations](#tool-annotations)
- [Failed calls](#failed-calls)
- [Parameter conventions](#parameter-conventions)
- [Core Tools (32)](#core-tools-32)
- [LSP Tools (38)](#lsp-tools-38)
- [Architecture Tools (5)](#architecture-tools-5)
- [MCP Client Configuration](#mcp-client-configuration)

## Overview

Devscriptor provides **75 MCP tools** across three categories:

| Category | Count | Description |
| --- | --- | --- |
| Core Tools | 32 | Code scanning, search, analysis, embeddings, context graph, real-time watcher |
| LSP Tools | 38 | Language Server Protocol features across 21 language servers |
| Architecture Tools | 5 | LSP detection, prerequisite checking, and setup |
| **Total** | **75** | Complete code-intelligence toolkit |

The 32 core tools break down as **24** scan/search/analyze/file tools, **5**
[Context Graph](context-graph.md) tools, and **3** watcher tools.

> The Context Graph is **5 MCP tools**, already counted inside the 32 core tools.
> The number **9** that sometimes gets attached to it is the count of *note types*
> (`adr`, `standard`, `optimization`, `failure`, `practice`, `reflection`,
> `general`, `file_description`, `modification`), not tools.

All tools are exposed through the **unified MCP server**, which runs inside the
same `devscriptor` binary used for the CLI. The server speaks the Model Context
Protocol as JSON-RPC over stdio, and there is no network port: the clients of a
workspace share one server process through a local socket (see
[Running the MCP Server](#running-the-mcp-server)). `tools/list` returns all 75
tools in a single page (`nextCursor` is always `null`).

## Running the MCP Server

Devscriptor is one executable, `devscriptor`. MCP clients start the server
through its `mcp` subcommand:

```bash
devscriptor mcp
```

An MCP client spawns `devscriptor` with `args: ["mcp"]` — see
[MCP Client Configuration](#mcp-client-configuration).

### One server per workspace

By default `devscriptor mcp` is a proxy. It relays its stdin and stdout, byte for
byte, to the workspace's daemon, `devscriptor serve`, and starts the daemon when
none answers. The daemon holds the index, the watcher, the embedding model and the
language servers once, and serves every client of the workspace in an MCP session
of its own: two agent sessions on one repository share one rust-analyzer instead
of starting one each, and the VS Code extension joins them when they use its
database (`--database devscriptor.db`). Nothing changes for a
client: the tools, their answers and the protocol are those of a server in its own
process.

- **Which daemon.** The workspace is the directory the client starts
  `devscriptor mcp` in, and the database is the one `--database` names, else the
  shared default. Clients with the same workspace and database share a daemon.
  `health_check` reports its process id as `daemon_pid`: sessions with the same
  `daemon_pid` share one server.
- **Its settings.** A daemon started by `devscriptor mcp` takes that command's
  `--lsp-startup` and `--no-watch`, and its environment (`DEVSCRIPTOR_LSP_*`,
  `DEVSCRIPTOR_WATCH`, `DEVSCRIPTOR_FRESHNESS_BUDGET_MS`, …). A daemon already
  running keeps those it was started with.
- **Its life.** It stops once no session has been open for 10 minutes, and its
  language servers and watcher with it. A `devscriptor mcp` of another Devscriptor
  version stops it and starts a daemon of its own version; sessions the old daemon
  still served end with it.
- **The fallback.** When no daemon can be started or reached within 10 s,
  `devscriptor mcp` serves in its own process and says why on stderr.
- **Who may connect.** A daemon serves only its own user's clients, and
  `devscriptor mcp` connects only to a daemon of its own user
  ([Architecture](architecture.md#who-may-connect)).

`devscriptor mcp --no-daemon`, or `DEVSCRIPTOR_DAEMON=0`, serves in the
`devscriptor mcp` process, one server per client.
`devscriptor serve --workspace <dir>` runs a workspace's daemon by hand
([CLI](cli.md#serve)).

| Variable | Effect |
| --- | --- |
| `DEVSCRIPTOR_DAEMON` | `0` or `false`: `devscriptor mcp` serves in its own process, as with `--no-daemon` |
| `DEVSCRIPTOR_DAEMON_IDLE_SECS` | Seconds a daemon waits with no session open before it stops (default 600); read when it starts |
| `XDG_RUNTIME_DIR` | When set, the runtime directory is `$XDG_RUNTIME_DIR/devscriptor/` |

### Runtime directory and logs

Each daemon keeps its files in the runtime directory, which only its user may
enter: `$XDG_RUNTIME_DIR/devscriptor/`, else `~/.cache/devscriptor/run/` on Linux
and macOS, and `%LOCALAPPDATA%\devscriptor\run\` on Windows. The files are named by
a key the workspace and database paths determine:

| File | Contents |
| --- | --- |
| `<key>.sock` | The socket the daemon listens on. On Windows it listens on the named pipe `\\.\pipe\devscriptor-<key>` instead |
| `<key>.lock` | Held by the daemon while it runs: one daemon per key |
| `<key>.json` | The daemon's `pid`, `version` and `started` (seconds since 1970) |
| `<key>.log` | The daemon's log, renamed to `<key>.log.1` once it would pass 10 MB |

A daemon removes all but its log when it stops. The log is at `info`: the start,
with the workspace, the database and the socket, the watcher, the language servers
started and stopped, sessions that failed, and why the daemon stopped. To find a
workspace's log:

```bash
dir=${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/devscriptor}
grep -lF "Serving $PWD (index" "${dir:-$HOME/.cache/devscriptor/run}"/*.log
```

Tool calls run in the daemon, so their warnings are in its log, not on the stderr of
`devscriptor mcp`, which carries only the proxy's own messages. To see a daemon's
debug output, stop it (`kill <daemon_pid>`) and run it in a terminal with the
clients' database: `devscriptor [--database <db>] --verbose serve --workspace <dir>`.
Clients started in `<dir>` then connect to it.

### Project configuration

The server follows the project configuration of its workspace: `devscriptor.yaml`
there, or the file `devscriptor mcp --config <FILE>` names (passed on to the daemon it
starts; one already running keeps the file it was started with). Its scans
(`scan_codebase`, `start_analysis`), its watcher and the freshness check take the
file's scope (extensions, excludes, size limit, ignore files, cross-file mode), and
`generate_embeddings` and on-demand embedding its embedding selection; a tool's own
arguments outrank the file. The schema is in
[Configuration](configuration.md#the-project-file-devscriptoryaml).

The server reads the file again when it changes, at the next tool call (a `stat`
when it has not): a running daemon takes an edit without a restart, but for the
pool size, which applies when the server starts. An edit that leaves the file
broken changes nothing, and `health_check` reports the error under `project_config`
until it is fixed; a file broken from the start leaves the defaults in force. The
server keeps serving either way.

### stdio hygiene

`devscriptor mcp` serves JSON-RPC on **stdout**. All logging goes to **stderr**,
because a single non-protocol byte on stdout corrupts the stream and strict
clients (the official SDKs, `rmcp`) drop the connection with
`serde error expected value at line 1 column 1`. Never add `println!` to an MCP
code path, and verify with the loudest setting, on a server in its own process
(the proxy writes to stdout only what the daemon sends):

```bash
devscriptor --verbose mcp --no-daemon 2>/dev/null | head -c 200   # must print nothing
devscriptor --verbose mcp --no-daemon 2>&1 >/dev/null | head      # logs appear here
```

Verbosity is controlled by the global `--verbose` flag. `RUST_LOG` is **not**
read: the tracing filter is a fixed directive (`warn` normally, `debug` with
`--verbose`). `devscriptor serve` logs at `info` to its log
([Runtime directory and logs](#runtime-directory-and-logs)), and to stderr as well
when that is a terminal.

## Getting all 75 tools

The 38 LSP and 5 Architecture tools live behind the **compile-time** `lsp`
feature. Which binary you run therefore decides how many tools you get:

| What you run | Tools |
| --- | ---: |
| `devscriptor mcp` — the CLI built with `devscriptor-cli` **default** features (`lsp` + `mcp`) | **75** |
| `devscriptor-mcp-standalone`, built from the `devscriptor-mcp` crate (its defaults omit `lsp`) | 32 |
| Any build with `--no-default-features` | ≤ 32 |

Do **not** pass `--features` when building for release; `devscriptor-cli`'s
defaults already produce the full universal binary:

```bash
cargo build --release -p devscriptor-cli   # → target/release/devscriptor
```

A 32-tool server silently omits every LSP and Architecture tool: the agent still
"works", it just never navigates code.

To verify after installing, read the tool count the server advertises. Its MCP
`initialize` response carries an `instructions` field beginning
`Devscriptor Unified MCP Server - Complete 75-tool implementation`, and a
`tools/list` call returns exactly 75 entries — including `get_hover` (LSP),
`lsp_arch_setup` (Architecture) and `start_watching` (watcher). Any MCP client's
tool inspector will show this; there is no startup banner to read, because the
server must not print to stdout.

## Tool annotations

Every tool in `tools/list` carries MCP annotations: a `title`, `readOnlyHint`,
and `openWorldHint`, plus `destructiveHint` and `idempotentHint` for the tools
that change something. A client can run the read-only tools without asking and
give them to read-only agents.

- **Read-only (59):** the search, entity and context-note queries, file
  context, the statistics and status tools, and every LSP query. An LSP query
  starts its language server on first use; that counts as reading.
- **Additive (7):** `scan_codebase`, `start_analysis`, `generate_embeddings`,
  `add_context_note`, `start_watching`, `start_server` and `lsp_arch_setup` add
  to or refresh Devscriptor's state; only `start_analysis` and
  `add_context_note` are not idempotent.
- **Destructive (9):** `format_document`, `get_range_format` and
  `rename_symbol` write files; `cleanup_old_jobs`, `cancel_analysis`,
  `prune_context`, `stop_watching`, `stop_server` and `restart_server` delete or
  stop something.
- **Open world:** only `lsp_arch_setup`, which can download servers.

## Failed calls

A call that runs and fails answers with a result whose `isError` is `true` and
whose text says why: a missing or invalid argument, a name the index does not
know, a language server that cannot start, a tool called faster than its rate
limit allows (10 calls a second per tool, in bursts of up to 20, then "retry
after 1s"). This is how MCP 2025-11-25 has a tool report a failure, so a client
shows it to the model, which can correct its call. A JSON-RPC error answers
only a call to a tool the server does not have (`-32602`) or a malformed
request.

## Parameter conventions

Take parameters from the live `tools/list` schema; the tables below mirror it.

- **Symbols by name or position.** LSP position tools take a symbol `name`,
  resolved from the code index (scan first; `entity_type` or `file_path` choose
  between symbols of that name), or `file_path`, `line`, `character` —
  0-indexed, never `column`. `character` counts UTF-16 code units, the LSP
  default: one per character, two for a character outside the Basic
  Multilingual Plane such as an emoji. Neither form is required on its own.
- **Ranges** take `start_line`, `start_character`, `end_line`, `end_character`
  (`get_code_actions`, `get_inlay_hints`, `get_range_format`).
- **Citing positions.** Every LSP result with a position carries `cite`,
  `path:line:column` counted from 1 (`a.rs:42:5` for LSP line 41, character 4):
  quote it as is. Its `range` and `position` stay LSP's, 0-based. Locations
  (definitions, references, symbols) carry it in `location`; diagnostics,
  highlights, links, inlay hints, folding and selection ranges and colors carry
  it beside their range; call and type hierarchy items cite the symbol's name.
  The search tools' `start_line`, `end_line` and `line_number` count from 1 too.
- **Numbered excerpts.** Source text in an answer comes numbered as agents'
  file readers number a file: each line as `<line><TAB><text>`, its line in the
  file counted from 1 — `semantic_search`'s `body_preview`, `get_file_context`'s
  `snippet` and `describe_file`'s `content_preview`. Cite a line inside one by
  its number rather than counting from `start_line`. A body Devscriptor cut short
  ends with an unnumbered `... (truncated)` line.
- **Three different path parameters**, one per tool family:
  - `path` — scan / analyze / watcher / statistics (a directory)
  - `project_path` — all `lsp_arch_*` tools
  - `file_path` — all LSP tools and the file/context tools
- **Path filters match as substrings**, so a relative fragment (`"src/auth.rs"`)
  matches an absolute stored path. This applies to `file_path` on
  `search_functions`, `search_classes`, `get_entity_details`,
  `analyze_complexity`, `get_related_context`, and to `path` on `get_statistics`.
- **Result limits** are `limit` everywhere except `search_context`, which uses
  `top_k`.
- **Hierarchy items.** `get_incoming_calls`, `get_outgoing_calls`,
  `get_subtypes` and `get_supertypes` take the symbol the same way and prepare
  the hierarchy themselves, or an `item` returned by `get_call_hierarchy` /
  `get_type_hierarchy`, passed back unchanged. A malformed item is answered
  with the shape it should have.
- **References in files no project lists.** Before `get_references` and
  `get_incoming_calls` ask the language server, the files the index says use
  the symbol — call it, read it, pass it, import it — are opened in it (up to
  64, of its language): a server that looks only in the projects of open
  files — typescript-language-server without a `jsconfig.json` — finds a test
  file's calls then. The uses it still does not report, the index adds:
  `index_locations` for `get_references`, `index_calls` for
  `get_incoming_calls`.
- **Opaque item parameters.** `get_completion_resolve` takes an `item` from
  `get_completions`, and `get_code_action_resolve` an `action` from
  `get_code_actions`. Pass the object through unchanged.

## Core Tools (32)

Core tools operate on the local SQLite knowledge graph built by `devscriptor
scan` or `scan_codebase`. They require no language server.

### Answers follow the files on disk

The tools that answer from the index — `semantic_search`, `search_entities`,
`search_functions`, `search_classes`, `fuzzy_search`, `query_with_context`,
`get_entity_details`, `get_relationships`, `get_file_context` and
`describe_file` — first check the files their answer names against the disk. A
file whose size or modification time differs from what the last scan recorded,
or that was deleted, is re-indexed, and if any was, the query runs again. So does
a query that read a file while another answer or the watcher re-indexed it. Two
fields, each present only when it names a file, say what happened:

| Field | Meaning |
| --- | --- |
| `refreshed_files` | Files re-indexed for this answer: it reflects them as they are on disk |
| `stale_files` | Files that changed on disk but were not re-indexed: the answer may be older than them, so read them from disk before relying on it |

An answer re-indexes at most 20 files and waits for them at most
`DEVSCRIPTOR_FRESHNESS_BUDGET_MS` milliseconds (default 250). A re-index that
takes longer finishes in the background, and the next answer is current. Files
past either limit are stale. So are files the index holds that lie outside every
scanned root or that the scan rules now exclude: those are never re-indexed. A
new file that a scan would index is indexed when an answer names it
(`describe_file`, `get_file_context`). `get_relationships` checks the files of
the entity asked about and of every edge's other end, and the LSP position tools
check the files of a symbol `name` before resolving it to a position. The
server's watcher ([Watcher](watcher.md)) keeps most files current before any
answer asks.

### Scanning & Analysis Jobs (7)

| Tool | Parameters | Description |
| --- | --- | --- |
| `scan_codebase` | `path`\*, `extensions`, `exclude_patterns` | Scan a codebase and build the entity database |
| `analyze_code` | `analysis_type`\*, `path` | Run an analysis and return its findings under the directory `path` (`.`, the default, is the whole index), each with its `file_path`, `line` and `details` |
| `start_analysis` | `path`\*, `extensions`, `exclude_patterns` | Start a background analysis job |
| `get_analysis_progress` | `job_id`\*, `wait`, `timeout` | Get the progress of an analysis job |
| `list_analysis_jobs` | `limit` | List all analysis jobs |
| `cancel_analysis` | `job_id`\* | Cancel a running analysis job |
| `cleanup_old_jobs` | `max_age_seconds` | Clean up old completed jobs |

`analyze_code` runs one analysis — `dead_code`, `complexity`,
`cross_reference`, `call_graph`, `code_smells`, `cross_file`, `clones` — or
`all`, over the whole index, and returns the findings located under `path`: in
its directory, or, for one that spans several files (a clone pair, a call
cycle), in any of them; a summary of the whole index is left out of a scoped
answer. A finding gives its `file_path` and `line` — a clone pair's first
copy, a cycle's first function — and what else the analysis knows of it in
`details` (a clone's other place, a cycle's functions and files, a smell's
suggestions). `cross_reference` reports what no call, use, reference or
supertype reaches; `find_dead_code` tells what of that a runtime, framework
or test runner may still reach. A `path` that is not a directory is an error,
as `find_dead_code`'s and `analyze_complexity`'s are.

`extensions` is a **whitelist** of file extensions, with or without a leading dot.
Omit it to scan what the project's `devscriptor.yaml` sets in `scan.include`, else
every supported language. `exclude_patterns` are globs matched against file paths,
applied on top of the built-in excludes (`node_modules`, `target`, `.git`, `vendor`,
`dist`, `build`, …); omitted, the project's `scan.exclude` apply. The scans take the
rest of the project's scope too (size limit, ignore files, cross-file mode): see
[Project configuration](#project-configuration).

**`scan_codebase` example:**

```json
{
  "path": "/path/to/project",
  "extensions": ["rs", ".toml"],
  "exclude_patterns": ["**/tests/**", "*_test.rs"]
}
```

`analysis_type` must be one of: `dead_code`, `complexity`, `cross_reference`,
`call_graph`, `code_smells`, `cross_file`, `clones`, `all`.

For large repositories prefer the background job pattern over a blocking scan:

```text
start_analysis(path)             → job_id
get_analysis_progress(job_id, wait=true, timeout=30)   # poll until complete
```

### Statistics & Health (2)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_statistics` | `path` | Database statistics; `path` scopes the counts to entities whose file path contains it |
| `health_check` | *(none)* | Check server health (database, jobs, version, the project configuration, and `daemon_pid` under a shared daemon) |

`health_check` returns `status`, `version`, `database`, `jobs`, `devscriptor`,
`hot_reload` and `project_config`. `hot_reload.enabled` says whether the server follows
a project configuration file; `project_config` names it (`path`), whether it `exists`,
and the `error` that keeps it from applying, or `null`. Served by a workspace's daemon,
it adds `daemon_pid`, the daemon's process id, the same in every session the daemon
serves; `jobs` then counts the analyses of all of them.

`get_statistics` returns `total_files`, `total_entities`, `total_relationships`,
and echoes `path` when scoped. An unmatched `path` returns zeros, not the global
totals.

### Search & Embeddings (6)

| Tool | Parameters | Description |
| --- | --- | --- |
| `search_entities` | `query`\*, `entity_type`, `limit` | Search entity **names**; `entity_type` filters in SQL. Use `semantic_search` to match on body text |
| `search_functions` | `query`\*, `file_path`, `limit`, `offset` | Search functions by name |
| `search_classes` | `query`\*, `file_path`, `limit`, `offset` | Search classes/types by name |
| `fuzzy_search` | `query`\*, `limit` | Typo-tolerant fuzzy search over entity names (scores every candidate, not a substring match) |
| `semantic_search` | `query`\*, `entity_type`, `limit` | Semantic search. `entity_type` is an optional filter — **omit it to search every kind**; naming one narrows to it |
| `generate_embeddings` | `force`, `batch_size` | Generate and store embedding vectors for the entities missing them that the embedding selection picks (idempotent) |

`semantic_search` does **not** require a separate embed pass over MCP: the server
embeds the candidates a query touches on demand (cache-first) before the dense
channel runs, so a `scan_codebase` followed immediately by `semantic_search`
returns vector matches. Call `generate_embeddings` to pre-build the whole index
in one pass — the MCP counterpart of the `devscriptor embed` CLI command — when
you would rather pay that cost up front than per query.

Both embed only the entities the project's embedding selection picks
(`embeddings.selection` in `devscriptor.yaml`, or `DEVSCRIPTOR_EMBEDDING_SELECTION`;
`standard` by default); the others stay found by keyword.

`generate_embeddings` only processes entities that lack vectors, so it is safe to
re-run; `force: true` re-embeds everything. `batch_size` defaults to 16 and is
clamped to 1–256. The response reports `embedded`, `pending_before`, `remaining`,
and `total_entities`.

When no embedding service is available at all, `semantic_search` falls back to
keyword search and sets `service_available: false` plus a `message` on the
response — check those before trusting the scores.

Each `semantic_search` and `fuzzy_search` result carries the entity's
`file_path`, `start_line` and `end_line` (1-indexed, as in an editor), so a
match can be cited as `file:line` without reading the file.

```json
{ "query": "function to read and parse configuration files", "entity_type": "function", "limit": 10 }
```

### Code Quality Analysis (2)

| Tool | Parameters | Description |
| --- | --- | --- |
| `find_dead_code` | `path`, `min_confidence`, `limit` | Find the functions and classes nothing reaches |
| `analyze_complexity` | `path`, `file_path`, `top_n` | Rank the functions in scope by cyclomatic complexity |

`find_dead_code` reports the functions and classes of the directory `path`
(`.`, the default, is the whole index; a path that is not a directory is an
error) that nothing reaches; what reaches them is looked for everywhere. A
call, a reference — a function passed as a value counts — a use or a supertype
reaches an entity, and so do the runtime, frameworks and test runners: test
code (test files and directories, `test_total`, `testTotal`, Go's `TestTotal`),
a function a decorator or an annotation registers (`@app.route`,
`@GetMapping`, `[HttpGet]`, `#[test]`, `@Test`) — not one that only describes
it (`@staticmethod`, `@dataclass`, `#[derive]`, `#[allow]`, `@Deprecated`,
`[Obsolete]`) — an override, a member a call through the type it overrides
or implements may run (an interface's implementations, a base class's
overrides, by name), one implementing a member of a type the index does not
hold (Rust's `fmt` of `impl Display`), what the runtime calls by name
(dunders, constructors, `main`, `apply`, `toString`) or through a protocol
(Go's `String` and `MarshalJSON`, Ruby's `to_s`, JavaScript's `toJSON`),
getters and setters, Solidity's `public` and `external` functions, and what
framework role files (controllers, views, resolvers, routes, handlers, jobs)
do not keep private. A class is reached when one of its members is. The surest findings come first,
`limit` (default 20) of them, and `total_entities_analyzed` counts what was in
scope: none means nothing under `path` is indexed, not that none of it is dead.

Every finding carries a `confidence` and a `reason`:

| Finding | Confidence | Why |
| --- | ---: | --- |
| Unreachable **private** entity | `0.9` | Nothing in the codebase can reach it |
| Unreachable **exported** entity | `0.5` | May be used outside the scanned codebase |
| Unreachable entity whose **name is written elsewhere** | `0.3` | May be reached in a way no reference records: another language, a template, a program a script runs, a string |
| Unreachable method of a type extending one **the index does not hold** | `0.3` | May implement a member of it, which code outside the index calls (a framework's base class, a standard interface) |

What an entity's language exports: Rust's `pub`, Go's capitalized names,
Python's and Dart's names without a leading `_`, a JavaScript or TypeScript
`export` or class member, a Java, C#, Swift or VB.NET `public`/`protected`/
`open`, a C-family function that is not `static`, and, in the languages public
by default (Kotlin, Scala, PHP, Ruby, F#), anything not `private` or
`internal`. `min_confidence` (default `0.5`) drops findings below the
threshold, so `0.9` returns private entities only and `0.3` every finding.

`analyze_complexity` counts every function in scope — the whole index, the
directory `path` names, or the file `file_path` matches — and returns
`total_functions_analyzed`, their `average_complexity`, the `top_n` most
complex whatever their score, `threshold` (10: a function above it is `medium`
risk, above twice it `high`) and how many are `functions_above_threshold`.

### Entities & Relationships (2)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_entity_details` | `entity_type`\*, `name`\*, `file_path` | Where an entity is, and its signature, members, fields and supertypes |
| `get_relationships` | `entity_name`\*, `entity_type`, `file_path`, `relationship_type` | What an entity is linked to (calls, uses, supertypes, members), with where each end is and the lines a call is made on |

`get_entity_details` reports where an entity is and what the index holds of
its kind: a function's `parameters`, `return_type`, `parent_class`,
`parent_function` (the function it is nested in), `implements` (the interface
or trait it implements a member of, where its definition says so),
`modifiers`, `access_modifiers` and `decorators`; a class's, struct's,
interface's, trait's or enum's `methods`, its `fields` or `properties` — each
`{"name", "type"}`, the type where written — and the types it `extends` and
`implements`; an enum's `variants`. Several entities of the name give
`total_matches` and `other_matches` (`file:line`), which `file_path`, matched
as a substring of the path, tells apart; when it matches nothing the tool
reports "not found" rather than returning an arbitrary entity.

`get_relationships` applies `entity_type`, `file_path` (matched as a substring
of the path, as `get_entity_details` matches it) and `relationship_type` **only
when supplied** — none has a silent default. The response echoes `entity_type:
"any"` when the filter was omitted. Each edge reports **resolved names** for
both endpoints (`source`/`target`) and their types, and where each is defined
(`source_file`/`source_line`, `target_file`/`target_line`, lines counted from 1)
— the target of an edge (a called function, a parent type, a referenced table)
is looked up by id, so you never get a bare UUID and need no follow-up
`get_entity_details` to read it. A call, a use of a type, a supertype or a
name used as a value (a function passed, a constant read) also gives `lines`: the lines of the source's file it
is made on, every one of them when the source makes it more than once. Without
`relationship_type`, each edge is given once, as its source's: `calls` and not
also the `called_by` row stored with it, `has_method` and not also `method_of`;
asking for a kind (`called_by` included) gives the edges of that kind. An edge
with `"ambiguous": true` is one of several equally good resolutions of a name,
such as a call to overloaded methods: the call is linked to each of them (up to
three, in a fixed order) rather than to one picked arbitrarily, so treat it as a
possible target, not a certain one. The field is absent when the edge's target
is the only definition that fits, or when one of the source's references to it
is certain.

```json
{ "entity_type": "function", "name": "parse_file", "file_path": "src/parser.rs" }
```

### Context Graph (5)

The context graph persists human/agent-authored notes alongside the code
entities. See [Context Graph](context-graph.md) for the full reference.

| Tool | Parameters | Description |
| --- | --- | --- |
| `add_context_note` | `note_type`\*, `title`\*, `content`\*, `summary`, `tags`, `priority`, `author`, `related_entities`, `related_concepts` | Add a note, optionally linked to code |
| `search_context` | `query`\*, `top_k`, `search_type`, `note_type`, `status`, `min_priority`, `max_content_length` | Search notes (keyword / semantic / hybrid) |
| `get_related_context` | `entity_type`\*, `entity_name`, `file_path`, `max_content_length` | Notes linked to an entity |
| `prune_context` | `max_age_days`, `max_notes`, `dry_run` | Archive old / surplus notes |
| `get_context_statistics` | *(none)* | Context graph statistics |

### File Context & Edit Validation (5)

| Tool | Parameters | Description |
| --- | --- | --- |
| `describe_file` | `file_path`\*, `include_content` | Describe a file's contents |
| `describe_modification` | `file_path`\*, `original_content`\*, `modified_content`\* | Describe what a modification would do |
| `get_file_context` | `file_path`\*, `context_lines` | Entities in a file plus a source excerpt around each |
| `validate_file_edit` | `file_path`\*, `original_content` | Validate a file edit with context |
| `query_with_context` | `query`\*, `entity_type`, `file_path`, `force_broad_search` | Query with context awareness |

`get_file_context`'s `context_lines` (default `10`) is the number of source lines
included on either side of each entity's definition, returned in
`entity_contexts[]` as `{name, entity_type, line, snippet_start_line, snippet}`,
each snippet line numbered. Snippets are clamped to the file's bounds and capped at
50 entities. `describe_file` answers with the file's `total_lines` (absent when the
file cannot be read), its entities, and with `include_content` (the default) the
start of the file as a numbered `content_preview`.

`describe_file`, `describe_modification` and `validate_file_edit` only read files
under the server's working directory or the database's directory, or files already
indexed in the graph; any other path is rejected.

### Real-time Watcher (3)

Keep the knowledge graph current while the MCP server runs: the watcher
re-indexes the source files that change. `devscriptor mcp` starts one by itself
on the scanned project it runs in; `--no-watch` or `DEVSCRIPTOR_WATCH=0` turn
that off. See [Watcher](watcher.md).

| Tool | Parameters | Description |
| --- | --- | --- |
| `start_watching` | `path`\*, `debounce_ms` (400), `embed` (false) | Watch a directory and auto-update the graph. On the root already watched it returns that watch's status with `already_watching: true`; while one root is watched another is refused (one watcher per server). `embed` needs 4 GB of free RAM; with less, the watcher runs without it and the response carries `embed_skipped` |
| `stop_watching` | *(none)* | Stop the watcher, and keep scans from starting it again |
| `get_watch_status` | *(none)* | Whether watching, and how: `mode` (`events`, or `poll` with a `fallback_reason`), `watched_directories`, re-index count, last re-index stats; `auto_watch_error` when the server's own watcher did not start |

## LSP Tools (38)

LSP tools proxy live language-server features. A call goes to the server for
the file's language. No server starts with the MCP server: each installed one
starts on first use, or in the background once `lsp_arch_detect` finds its
language (so call `lsp_arch_detect` first), and stops after 10 minutes unused
([Language servers](lsp.md#lifecycle); `--lsp-startup eager` starts them all
up front). Install missing ones with the [Architecture Tools](#architecture-tools-5)
or `devscriptor lsp setup`. A file that no installed server handles fails with
`No LSP server available for file: …`. The **first** call after a server starts
can be slow; that is the language server booting and indexing, not a hang.

All positions are **0-indexed**; quote a result's `cite` instead, which counts
from 1. † marks the tools that take a symbol by
`name` (with optional `entity_type`, `file_path`) or by `file_path`, `line`,
`character` — see [Parameter conventions](#parameter-conventions).

### Navigation & References (5)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_hover` | † | Hover information (type signature, docs) for a symbol |
| `get_definition` | † | Definition location of a symbol |
| `get_references` | †, `include_declaration` | All references to a symbol |
| `get_type_definitions` | † | Type definitions |
| `get_implementations` | † | Implementations |

```json
{ "name": "parse_config" }
{ "file_path": "/path/to/file.rs", "line": 10, "character": 15 }
```

### Completion (2)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_completions` | † | Code completions at a position |
| `get_completion_resolve` | `item`\* | Resolve a completion item from `get_completions` |

### Diagnostics & Code Actions (3)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_diagnostics` | `file_path`\* | Diagnostics (errors/warnings) for a file |
| `get_code_actions` | `file_path`\*, `start_line`\*, `start_character`\*, `end_line`\*, `end_character`\* | Available code actions for a range |
| `get_code_action_resolve` | `action`\* | Resolve an action from `get_code_actions` |

`get_diagnostics` pulls diagnostics from a server that offers them. Others —
typescript-language-server, clangd, gopls, jdtls — only push them after a file
is opened or changed: for those it reports what they push, waiting for the
pushes to settle, so the first call on a file takes a few seconds.

### Symbols (4)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_document_symbols` | `file_path`\* | All symbols in a file, parent first: each at its name, with the symbol it is declared in as `container_name` |
| `get_workspace_symbols` | `query`\* | Search symbols across the workspace: every running language server that offers it is asked and the answers merged; one that could not answer is named, with its error, in `note` |
| `get_symbols` | `file_path`\*, `limit`, `offset` | Symbols for a file as `get_document_symbols` gives them, a page at a time |
| `get_project_symbols` | `language_id`, `project`, `query`, `limit`, `offset` | A language server's symbols matching `query`; with `project`, those under that directory, whose files are opened in the server first |

### Call Hierarchy (3)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_call_hierarchy` | † | Call-hierarchy item(s) for a symbol |
| `get_incoming_calls` | †, or `item` | The calls into a function; those the code index records that the server did not report come in `index_calls`, each with its caller |
| `get_outgoing_calls` | †, or `item` | The calls a function makes |

### Type Hierarchy (3)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_type_hierarchy` | † | Type-hierarchy item(s) for a type |
| `get_subtypes` | †, or `item` | A type's subtypes |
| `get_supertypes` | †, or `item` | A type's supertypes |

### Document Features (8)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_highlights` | † | Document highlights for a symbol |
| `get_colors` | `file_path`\* | Document colors |
| `get_folding_ranges` | `file_path`\* | Folding ranges |
| `get_selection_range` | `file_path`\*, `positions`\* | Smart selection ranges; `positions` is a list of `{line, character}` |
| `get_linked_editing_range` | † | Linked (synchronized) editing ranges |
| `get_inlay_hints` | `file_path`\*, `start_line`\*, `start_character`\*, `end_line`\*, `end_character`\* | Inlay hints for a range |
| `get_links` | `file_path`\* | Document links |
| `get_semantic_tokens` | `file_path`\* | Semantic tokens |

### Formatting (2)

| Tool | Parameters | Description |
| --- | --- | --- |
| `format_document` | `file_path`\* | Format an entire document and write it to disk |
| `get_range_format` | `file_path`\*, `start_line`\*, `start_character`\*, `end_line`\*, `end_character`\* | Format a range and write it to disk |

### Refactoring (1)

| Tool | Parameters | Description |
| --- | --- | --- |
| `rename_symbol` | †, `new_name`\* | Rename a symbol across the workspace, editing every affected file on disk |

`format_document`, `get_range_format` and `rename_symbol` **write** their edits
to the files on disk, then return the edits they applied (`{"changes": [{"file_path",
"edits": [{"range", "new_text"}]}]}` for a rename). For a server that does not offer
formatting, the tool fails with *"the <language> language server does not support
formatting"*, and nothing is written.

### Server Management (7)

| Tool | Parameters | Description |
| --- | --- | --- |
| `get_server_status` | `language_id` | Language-server status, one server's or every configured one's: `running`, `exited` or `stopped`, with `stop_reason` (`not started`, `idle`, `requested`, `cap`) when stopped |
| `get_server_capabilities` | `language_id` | A server's capabilities |
| `get_server_projects` | `language_id` | The folders a running server has open, or the workspace's |
| `get_project_files` | `project`\*, `limit`, `offset` | A project's files, sorted, without hidden files and what its ignore files leave out |
| `start_server` | `language_id`\*, `project` | Start a language server for `project` (a path in it), or for the workspace; a running server is kept, restarted only to take a project it does not cover |
| `stop_server` | `language_id`\* | Stop a language server |
| `restart_server` | `language_id`\* | Restart a language server |

## Architecture Tools (5)

Architecture tools drive the 4-tier LSP detection, prerequisite checking, and
on-demand installation system (see [Language servers](lsp.md)).

| Tool | Parameters | Description |
| --- | --- | --- |
| `lsp_arch_detect` | `project_path` | Detect languages in a project and resolve/plan the LSP servers it needs, each one to install with the `reason` it does not run; starts the installed ones in the background and lists them in `prewarming` |
| `lsp_arch_check` | `server_id`\* | Check whether a single server is available and where it resolves, or the `reason` it does not run here and what makes it run; `server_id` is a server id (`dart-ls`) or its language (`dart`), and a name that is neither comes back as `unknown`, with the server ids |
| `lsp_arch_batch_check` | `server_ids`\* | Check availability of several servers at once, each by its id or its language, and each one that is not available with its `reason`; the names that are neither come back under `unknown`, with the server ids |
| `lsp_arch_status` | *(none)* | Resolution status for all known servers, grouped by source; one installed here that cannot run is under `cannot_run`, apart from those to install, and each not available has its `reason` |
| `lsp_arch_setup` | `project_path`, `auto_install`, `servers_to_install` | Detect a project's languages and install the servers it needs (`auto_install`), or install the servers `servers_to_install` names, by id or language, whether the project needs them or not |

`lsp_arch_setup` with `auto_install: true`, or with `servers_to_install`, downloads
and installs language servers — gate it behind user approval in an agent.

```json
{ "project_path": "/path/to/project", "auto_install": true }
{ "servers_to_install": ["rust-analyzer", "bash"] }
```

## MCP Client Configuration

Point your MCP client at a **75-tool** entry point (see
[Getting all 75 tools](#getting-all-75-tools)).

**Claude Desktop / Claude Code / Cline:**

```json
{
  "mcpServers": {
    "devscriptor": {
      "command": "devscriptor",
      "args": ["mcp"]
    }
  }
}
```

(Setting `RUST_LOG` in `env` has no effect — see [stdio hygiene](#stdio-hygiene).
`"args": ["--verbose", "mcp"]` gives debug logs on stderr from the server in that
process, so add `--no-daemon`; a shared daemon logs to its own log, see
[Runtime directory and logs](#runtime-directory-and-logs).)

**Cursor:**

```json
{
  "mcpServers": {
    "devscriptor": { "command": "devscriptor", "args": ["mcp"] }
  }
}
```

**VS Code:** the Devscriptor extension offers the server to VS Code's AI assistants, such as
agent mode in chat, while its setting `devscriptor.mcpServer.enabled` is on (VS Code 1.101 or
newer): `devscriptor mcp` with the extension's database, in the first workspace folder, so the
assistants share the extension's daemon. See
[the extension's README](../vscode-extension/README.md#ai-assistants-in-vs-code).

`devscriptor-mcp-standalone`, built from the `devscriptor-mcp` crate, is a
development binary that serves only the 32 core tools. It is not what agents run.

### Timeouts

Most clients default to a 60-second per-tool timeout, which is too short:

| Operation | Recommendation |
| --- | --- |
| `scan_codebase` on a large repo | raise the tool timeout to ≥ 180 s, or use `start_analysis` + `get_analysis_progress` |
| first LSP call per language | seconds to minutes while that language's server starts and indexes (lazy startup); call `lsp_arch_detect` first to start them in the background; do not retry immediately |
| cold `semantic_search` | allow ≥ 60 s while the embedding model loads |
| server startup | fast: no language server starts before `initialize` is answered. With `--lsp-startup eager`, allow ≈ 60 s while the servers its working directory needs start first |

## Related documentation

- [Context Graph](context-graph.md) — the 5 Context Graph tools and the 9 note types
- [Watcher](watcher.md) — the watcher and the 3 watcher tools
- [Language servers](lsp.md) — detection, installation and lifecycle
- [CLI](cli.md) — the `mcp` and `serve` commands
- [Architecture](architecture.md) — the server processes
