//! Database schema definitions

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension};

/// The schema version this build writes, recorded under
/// [`SCHEMA_VERSION_KEY`] in `metadata`.
pub const SCHEMA_VERSION: u32 = 1;

/// The `metadata` key holding the schema version.
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

/// The `metadata` key that names the vector space of the index's vectors
/// ([`crate::embeddings::EMBEDDING_SPACE`]).
pub const EMBEDDING_SPACE_KEY: &str = "embedding_space";

/// The tables of the index, which a scan writes again: dropped, dependents
/// first, from a database written with another schema version. The Context
/// Graph's tables hold what users and agents wrote, and are not among them.
const INDEX_TABLES: &[&str] = &[
    "entities_fts",
    "entity_vectors",
    "relationships",
    "symbol_refs",
    "file_imports",
    "entities",
    "files",
    "scan_manifest",
    "embedding_cache",
    "embedding_jobs",
    "metadata",
];

/// Create all database tables
pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Files table
        CREATE TABLE IF NOT EXISTS files (
            id TEXT PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            language TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            line_count INTEGER NOT NULL,
            checksum TEXT NOT NULL,
            last_modified TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- `path` is UNIQUE, which indexes it.
        CREATE INDEX IF NOT EXISTS idx_files_language ON files(language);

        -- Entities table (polymorphic via type column)
        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            -- Pre-split camelCase/snake_case tokens for code-aware FTS search.
            -- Populated by Rust via split_identifier_tokens() during batch insert.
            -- FTS trigger uses this column so `getUserById` matches `get user by id`.
            name_tokens TEXT NOT NULL DEFAULT '',
            file_path TEXT NOT NULL,
            body TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            start_column INTEGER NOT NULL,
            end_column INTEGER NOT NULL,
            language TEXT NOT NULL,
            data JSON,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (file_path) REFERENCES files(path) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
        CREATE INDEX IF NOT EXISTS idx_entities_file ON entities(file_path);
        CREATE INDEX IF NOT EXISTS idx_entities_language ON entities(language);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(type);

        -- Composite indexes for common multi-column filter patterns
        -- (e.g. "all functions in Python", "all classes in TypeScript")
        CREATE INDEX IF NOT EXISTS idx_entities_lang_type ON entities(language, type);

        -- Speeds up range queries for source location lookups
        CREATE INDEX IF NOT EXISTS idx_entities_lines ON entities(start_line, end_line);

        -- Speeds up incremental processing (files modified since last scan)
        CREATE INDEX IF NOT EXISTS idx_files_modified ON files(last_modified);

        -- Full-text search for entities.
        -- tokenize: 'unicode61 separators ...' splits on common code separators
        -- so snake_case, dot.notation and $sigils are indexed as separate tokens.
        -- A separate `name_tokens` column receives pre-split camelCase tokens
        -- (populated by an AFTER INSERT trigger via a Rust-side helper).
        CREATE VIRTUAL TABLE IF NOT EXISTS entities_fts USING fts5(
            name,
            name_tokens,
            body,
            content='entities',
            content_rowid='rowid',
            tokenize='unicode61 separators ''_.$/\-@#'''
        );

        -- FTS5 sync triggers: external content tables do NOT auto-update.
        -- These triggers keep the full-text index synchronized with the
        -- canonical `entities` table on every INSERT, UPDATE, and DELETE.
        -- name_tokens is pre-computed by Rust (split_identifier_tokens)
        -- and stored in entities.name_tokens; triggers use it for code-aware FTS.
        CREATE TRIGGER IF NOT EXISTS entities_fts_insert
        AFTER INSERT ON entities
        BEGIN
            INSERT INTO entities_fts(rowid, name, name_tokens, body)
            VALUES (new.rowid, new.name, new.name_tokens, new.body);
        END;

        CREATE TRIGGER IF NOT EXISTS entities_fts_delete
        AFTER DELETE ON entities
        BEGIN
            INSERT INTO entities_fts(entities_fts, rowid, name, name_tokens, body)
            VALUES('delete', old.rowid, old.name, old.name_tokens, old.body);
        END;

        CREATE TRIGGER IF NOT EXISTS entities_fts_update
        AFTER UPDATE ON entities
        BEGIN
            INSERT INTO entities_fts(entities_fts, rowid, name, name_tokens, body)
            VALUES('delete', old.rowid, old.name, old.name_tokens, old.body);
            INSERT INTO entities_fts(rowid, name, name_tokens, body)
            VALUES (new.rowid, new.name, new.name_tokens, new.body);
        END;

        -- Relationships table
        CREATE TABLE IF NOT EXISTS relationships (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            relationship_type TEXT NOT NULL,
            metadata JSON,
            created_at TEXT NOT NULL,
            FOREIGN KEY (source_id) REFERENCES entities(id) ON DELETE CASCADE,
            FOREIGN KEY (target_id) REFERENCES entities(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_rel_source ON relationships(source_id);
        CREATE INDEX IF NOT EXISTS idx_rel_target ON relationships(target_id);
        CREATE INDEX IF NOT EXISTS idx_rel_type ON relationships(relationship_type);

        -- Composite index for cross-reference lookups ("find all imports of X")
        CREATE INDEX IF NOT EXISTS idx_rel_source_type ON relationships(source_id, relationship_type);

        -- One edge per (source, target, type): a per-file and a cross-file pass
        -- that resolve the same reference store it once.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_rel_dedup
            ON relationships(source_id, target_id, relationship_type);

        -- References: each call, type use, inheritance or implementation
        -- found in a source entity, resolved or not. Edges are derived from
        -- them. (`references` is an SQL keyword.)
        CREATE TABLE IF NOT EXISTS symbol_refs (
            id INTEGER PRIMARY KEY,
            source_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            file_path TEXT NOT NULL,
            kind TEXT NOT NULL,      -- RelationshipType::as_str() of the forward edge
            target_name TEXT NOT NULL,
            qualifier TEXT,          -- receiver, or path ending in `::`, when the parser has one
            language TEXT NOT NULL,
            line INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_symbol_refs_target ON symbol_refs(target_name);
        CREATE INDEX IF NOT EXISTS idx_symbol_refs_source ON symbol_refs(source_id);
        CREATE INDEX IF NOT EXISTS idx_symbol_refs_file ON symbol_refs(file_path);

        -- The imports of each file, against which its references resolve.
        CREATE TABLE IF NOT EXISTS file_imports (
            file_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
            module TEXT NOT NULL,
            imported_name TEXT,      -- NULL for whole-module imports
            alias TEXT,
            line INTEGER             -- where it names the import; NULL before version 7
        );
        CREATE INDEX IF NOT EXISTS idx_file_imports_file ON file_imports(file_path);
        -- The files that import a name, whose references may resolve to it
        -- through an alias.
        CREATE INDEX IF NOT EXISTS idx_file_imports_name ON file_imports(imported_name);

        -- Metadata table
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        -- The Context Graph's tables are created by
        -- `context_graph::initialize_context_tables_on_conn`.

        -- Scan manifest table (for incremental scan support).
        -- size_bytes + mtime_secs enable a cheap fast-path: only hash/re-parse
        -- when size or mtime changed since the last scan.
        CREATE TABLE IF NOT EXISTS scan_manifest (
            path TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL,
            last_parsed_at TEXT NOT NULL,
            size_bytes INTEGER NOT NULL DEFAULT 0,
            mtime_secs INTEGER NOT NULL DEFAULT 0
        );

        -- No extra index needed: path is the PRIMARY KEY of scan_manifest.

        -- Embedding cache: avoids re-running ONNX inference for unchanged text.
        -- Key is a hex-encoded XXH3-64 hash of the entity text fed to the model.
        -- On incremental re-scans unchanged entities skip ONNX entirely.
        CREATE TABLE IF NOT EXISTS embedding_cache (
            text_hash TEXT PRIMARY KEY,
            vector    BLOB NOT NULL,
            created_at TEXT NOT NULL
        );

        -- Background embedding job progress tracker.
        -- `devscriptor embed --daemon` writes rows here; `embed --status` reads them.
        -- status: 'running' | 'done' | 'failed'
        CREATE TABLE IF NOT EXISTS embedding_jobs (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL DEFAULT 'running',
            total      INTEGER NOT NULL DEFAULT 0,
            done       INTEGER NOT NULL DEFAULT 0,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        "#,
    )?;

    Ok(())
}

/// Empty the index of a database written with another schema version, in one
/// transaction, before [`create_tables`] makes its tables again: drop the
/// [`INDEX_TABLES`] when the database holds an index whose `metadata` does not
/// name [`SCHEMA_VERSION`]. A scan builds the index again; the Context Graph's
/// notes are kept. Returns whether the index was dropped.
///
/// # Errors
/// Returns an error if a statement fails; nothing is changed then.
pub fn drop_index_of_other_version(conn: &Connection) -> Result<bool> {
    let exists = |name: &str| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )?)
    };
    let version: Option<String> = if exists("metadata")? {
        conn.query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            [SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?
    } else {
        None
    };
    let current = SCHEMA_VERSION.to_string();
    let other_version = match version {
        Some(version) => version != current,
        // No version: a new database, or one written before any was recorded.
        None => exists("entities")?,
    };
    if !other_version {
        return Ok(false);
    }
    // SQLite deletes a dropped table's rows first, and checks the foreign keys
    // of tables not yet dropped; the dependents go first, and the checks are
    // off while they do, as a table of another version may name others.
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let dropped = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        for table in INDEX_TABLES {
            tx.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))?;
        }
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    dropped?;
    Ok(true)
}

/// Record [`SCHEMA_VERSION`] in `metadata`, unless it is there: opening a
/// current database writes nothing.
///
/// # Errors
/// Returns an error if a statement fails.
pub fn record_schema_version(conn: &Connection) -> Result<()> {
    let current = SCHEMA_VERSION.to_string();
    let recorded: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            [SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if recorded.as_deref() != Some(current.as_str()) {
        set_metadata(conn, SCHEMA_VERSION_KEY, &current)?;
    }
    Ok(())
}

/// Keep the index's vectors in one space: when its stamp is not `space` (or
/// it has none), drop the stored vectors and the embedding cache, which
/// another model or arithmetic has made incomparable with new vectors, and
/// stamp it. Returns how many entity
/// vectors were dropped; the context notes' go too, and the next note search
/// embeds the notes again.
///
/// # Errors
/// Returns an error if a statement fails; nothing is changed then.
pub fn align_embedding_space(conn: &Connection, space: &str) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let stamp: Option<String> = tx
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            [EMBEDDING_SPACE_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if stamp.as_deref() == Some(space) {
        return Ok(0);
    }
    // Without sqlite-vec there is no vector table.
    let has_table = |name: &str| -> Result<bool> {
        Ok(tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?)
    };
    let dropped = if has_table("entity_vectors")? {
        let count: i64 =
            tx.query_row("SELECT COUNT(*) FROM entity_vectors", [], |row| row.get(0))?;
        tx.execute("DELETE FROM entity_vectors", [])?;
        usize::try_from(count).unwrap_or(0)
    } else {
        0
    };
    // Context notes are embedded in the same space; a search re-embeds them.
    if has_table("note_vectors")? {
        tx.execute("DELETE FROM note_vectors", [])?;
    }
    tx.execute("DELETE FROM embedding_cache", [])?;
    set_metadata(&tx, EMBEDDING_SPACE_KEY, space)?;
    tx.commit()?;
    Ok(dropped)
}

/// Set `metadata` key `key` to `value`.
fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO metadata (key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, time::OffsetDateTime::now_utc().to_string()],
    )?;
    Ok(())
}
