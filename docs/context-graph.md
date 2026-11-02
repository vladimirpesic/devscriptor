# Devscriptor Context Graph

Long-term memory for AI coding agents.

The **Context Graph** is a persistent, queryable store of *notes* — architectural
decisions, coding standards, optimizations, failures, reflections, and general
knowledge — that an agent (or a human) records about a codebase and can recall in
later sessions. Notes can be linked to concrete code entities and to abstract
concepts, so an agent can retrieve "what do I already know about this function /
module?" alongside the structural code graph.

## Table of Contents

- [How it Works](#how-it-works)
- [Storage & Schema](#storage--schema)
- [Note Anatomy](#note-anatomy)
- [MCP Tools](#mcp-tools)
- [CLI Commands](#cli-commands)
- [Search Behavior](#search-behavior)
- [Linking Notes to Code](#linking-notes-to-code)
- [Pruning](#pruning)
- [Relationship to the Code Graph](#relationship-to-the-code-graph)

## How it Works

- Notes are stored in the **same local SQLite database** as the code knowledge
  graph (`devscriptor.db` by default, or the per-user `default.db` when the MCP
  server picks its own location). They persist across sessions and processes —
  this is what makes the Context Graph *long-term* memory.
- The context tables are created automatically during database initialization;
  no separate setup step is required.
- Notes are created and queried explicitly, through the MCP tools or the
  `devscriptor context` CLI. Nothing is written to the Context Graph
  automatically during a `scan`.

## Storage & Schema

Five tables back the Context Graph:

| Table | Purpose |
| --- | --- |
| `context_notes` | The notes themselves (title, content, type, priority, status, timestamps, tags, structured data, metadata) |
| `context_notes_fts` | FTS5 full-text index over `title`/`content`/`summary`, kept in sync by triggers |
| `context_entity_links` | Links from a note to concrete code entities (by type/id/name/file) |
| `context_concept_links` | Links from a note to abstract concepts/modules/patterns |
| `note_vectors` | The notes' embeddings for semantic search (a `vec0` table, keyed by note id, with a hash of the text each was embedded from) |

## Note Anatomy

Each note has the following fields:

| Field | Notes |
| --- | --- |
| `note_type` | One of the types below (default `general`) |
| `title` | Short title (required) |
| `content` | Note body (required) |
| `summary` | Optional short summary |
| `author` | Defaults to `agent` |
| `priority` | Integer 1–10 (default 5); higher ranks first in search/listing |
| `status` | `active`, `superseded`, or `archived` (new notes are `active`) |
| `tags` | Optional list of strings |
| `created_at` / `updated_at` | Timestamps, set automatically |
| `effective_from` / `effective_until` | Optional validity window |
| `structured_data` / `metadata` | Optional JSON blobs |

### Note types

There are **9** note types:

| Type | Meaning |
| --- | --- |
| `adr` | Architecture Decision Record |
| `standard` | Coding standard or convention |
| `optimization` | Performance optimization record |
| `failure` | Failure or mistake record |
| `practice` | Best-practice documentation |
| `reflection` | Reflection or retrospective |
| `general` | General knowledge note (default) |
| `file_description` | Description of a file's contents |
| `modification` | Code modification record |

## MCP Tools

The Context Graph exposes **5** of the 32 core MCP tools. (The **9** above is the
number of note *types*, not tools.) Parameters marked `*` are required.

| Tool | Parameters | Description |
| --- | --- | --- |
| `add_context_note` | `note_type`\*, `title`\*, `content`\*, `summary`, `tags`, `priority` (5), `author` (`agent`), `related_entities`, `related_concepts` | Create a note, optionally linked to code |
| `search_context` | `query`\*, `top_k` (10), `search_type` (`keyword`), `note_type`, `status`, `min_priority`, `max_content_length` (500) | Keyword / semantic / hybrid search over notes |
| `get_related_context` | `entity_type`\*, `entity_name`, `file_path`, `max_content_length` (300) | Retrieve notes linked to an entity |
| `prune_context` | `max_age_days` (90), `max_notes`, `dry_run` (`true`) | Archive old and/or surplus notes |
| `get_context_statistics` | *(none)* | Totals, counts by type, average priority |

**`add_context_note` example (with an entity link):**

```json
{
  "note_type": "adr",
  "title": "Use rustls instead of OpenSSL",
  "content": "All HTTPS goes through rustls with webpki-roots so the binary has no OpenSSL dependency.",
  "tags": ["tls", "dependencies"],
  "priority": 8,
  "related_entities": [
    { "entity_type": "function", "entity_name": "build_http_client", "file_path": "src/net.rs" }
  ]
}
```

Each `related_entities` item requires `entity_type`; `entity_id`, `entity_name`,
`file_path`, and `link_type` (default `references`) are optional. Each
`related_concepts` item requires `concept_type` and `concept_name`; `concept_path`
and `link_type` (default `references`) are optional.

**`search_context` example (hybrid + filter):**

```json
{ "query": "auth token verification", "search_type": "hybrid", "note_type": "adr", "top_k": 5 }
```

**`get_related_context` example (narrowed to one symbol):**

```json
{ "entity_type": "function", "entity_name": "verify_token", "file_path": "src/auth.rs" }
```

`entity_name` matches the linked entity's name exactly; `file_path` matches as a
substring, so a relative fragment matches an absolute stored path. Supplying both
ANDs them. Omitting both returns every note linked to that entity **type**.

## CLI Commands

```bash
devscriptor context <SUBCOMMAND>
```

| Subcommand | Description |
| --- | --- |
| `add --title <T> --content <C> [--note-type <TYPE>] [--entities <UUIDs>] [--entity-type <TYPE>]` | Add a note (optionally linked to entities). `--note-type` defaults to `general`, `--entity-type` to `function`, `--entities` is a comma-separated list. |
| `list [--limit <N>] [--offset <N>]` | List notes (defaults: limit 20, offset 0). |
| `search <QUERY> [--limit <N>]` | Keyword search, as `search_context`'s `keyword` strategy (default limit 10). |
| `show <ID>` | Show a single note by ID. |
| `delete <ID>` | Delete a note by ID. |
| `prune [--max-age-days <N>] [--dry-run]` | Archive active notes not updated in N days (default 90), and delete notes archived for more than twice that. |
| `entity <ENTITY_TYPE>` | List notes linked to a given entity type. |
| `concept <CONCEPT>` | List notes linked to a concept. |

> The CLI `prune` exposes only `--max-age-days` and `--dry-run`. The `max_notes`
> cap is available through the MCP `prune_context` tool.

## Search Behavior

The `search_context` MCP tool supports three strategies via `search_type`:

- **`keyword`** (default) — full-text search (FTS5) for the query's words over
  each note's `title`, `content` and `summary`, ranked by BM25, then `priority`,
  then recency; `relevance_score` is each match's share of the best one's BM25
  score, so the best match scores 1.0. When no note contains the words, it
  falls back to a case-insensitive substring match of the whole query over
  `title` and `content`, which still finds part of a word: the matches come by
  `priority`, then recency, and score 1.0 for a match in the title, 0.7 in the
  content.
- **`semantic`** — vector similarity: each note is embedded from its title,
  summary and content, and the notes nearest the embedded query come first,
  scored by cosine similarity. A note is embedded on the first search after it
  is added or changed, and its vector goes with it when it is deleted. Requires
  an embedding service; falls back to full-text search when the query cannot be
  embedded.
- **`hybrid`** — runs keyword and semantic in parallel and fuses the results
  with Reciprocal Rank Fusion (RRF); the best fused match scores 1.0. **Prefer
  `hybrid` for recall.**

Optional `note_type`, `status`, and `min_priority` filters narrow the results.
`max_content_length` (default 500) truncates each result's `content`.

> The `devscriptor context search` CLI performs keyword search. The
> `search_type` strategies are exposed through the MCP `search_context` tool.

Retrieval by association is also available:

- `get_related_context` / `context entity <TYPE>` returns notes whose
  entity-links match the given entity **type**, optionally narrowed by
  `entity_name` and `file_path`.
- `context concept <NAME>` returns notes linked to a named concept.

## Linking Notes to Code

A note can carry two kinds of links:

- **Entity links** (`context_entity_links`) — to concrete code entities,
  captured by entity type, optional id/name, and file path. Create them via the
  `related_entities` array of `add_context_note`, or the CLI
  `devscriptor context add --entities <UUIDs> --entity-type <TYPE>`.
- **Concept links** (`context_concept_links`) — to abstract concepts, modules,
  or architectural patterns (the `related_concepts` array of `add_context_note`).

Links let an agent later ask "what notes relate to this entity / concept?" via
`get_related_context` and the `entity` / `concept` CLI subcommands.

## Pruning

Pruning ages out stale knowledge by setting notes to `archived`. Archived notes
still appear in searches (the `status` filter separates them) until they are
deleted. Two independent limits apply:

- **`max_age_days`** (default 90) — active notes not updated within the window
  are archived. Notes that have stayed archived, untouched, for more than *twice*
  the window are then deleted (archiving counts as an update, so a note archived
  today is deleted no sooner than twice the window from today).
- **`max_notes`** — caps how many **active** notes are retained. Surplus notes
  are archived lowest-first by `priority DESC, updated_at DESC`, so the
  highest-priority and most recently touched notes survive. Omit for no cap.

`prune_context` defaults to `dry_run: true` — run it first to preview, then set
`dry_run: false` to apply.

```json
{ "max_age_days": 180, "max_notes": 500, "dry_run": true }
```

## Relationship to the Code Graph

The Context Graph and the code knowledge graph live in the same database but are
updated independently:

- The **code graph** is (re)built by `devscriptor scan` (incremental) or kept
  current automatically by the [real-time watcher](watcher.md).
- The **Context Graph** is updated only by explicit `add`/`prune`/`delete`
  operations.

Note *content* is authored by you or an agent and is never rewritten
automatically. Entity/concept links are stored by type/name/file, so they remain
valid across re-scans as long as those names persist; manage notes explicitly as
decisions and knowledge evolve.

## Related Documentation

- [MCP server](mcp.md) — all 75 MCP tools, including the 5 Context Graph tools
- [CLI](cli.md) — the `context` CLI command
- [Architecture](architecture.md) — database schema and system design
