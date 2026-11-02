//! Database operations for entities and relationships

use crate::database::connection::ConnectionPool;
use crate::database::{FileRecord, StoredRef};
use crate::entities::{Entity, EntityId, FileInfo, Relationship, RelationshipType, SymbolRef};
use crate::error::Result;
use crate::parser::ImportInfo;
use crate::parser::symbol_table::SymbolEntry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::utils::escape_like;
use rusqlite::OptionalExtension;

/// Alias for a batch of parsed files shared across thread boundaries.
type ParsedFileBatch = Arc<[FileRecord]>;

/// `n` SQL parameter placeholders, comma-separated.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// Delete everything indexed for the files `paths` names, within `tx`: every
/// edge touching their entities, their vectors, their entities and their
/// `files` rows. Returns `(entities_deleted, relationships_deleted)`.
fn delete_indexed_files(
    tx: &rusqlite::Transaction<'_>,
    paths: &[String],
) -> rusqlite::Result<(usize, usize)> {
    let files = placeholders(paths.len());
    let file_params: Vec<&dyn rusqlite::ToSql> =
        paths.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

    let rel_sql = format!(
        "DELETE FROM relationships WHERE source_id IN (
            SELECT id FROM entities WHERE file_path IN ({files})
        ) OR target_id IN (
            SELECT id FROM entities WHERE file_path IN ({files})
        )"
    );
    let rel_params: Vec<&dyn rusqlite::ToSql> = paths
        .iter()
        .chain(paths.iter())
        .map(|p| p as &dyn rusqlite::ToSql)
        .collect();
    let relationships = tx.execute(&rel_sql, &rel_params[..])?;

    // Vectors before their entities: `entity_vectors` is a vec0 virtual table
    // with no FK cascade, so its rows would otherwise orphan. Best-effort: the
    // table does not exist when vectors are disabled.
    let vec_sql = format!(
        "DELETE FROM entity_vectors WHERE entity_id IN (
            SELECT id FROM entities WHERE file_path IN ({files})
        )"
    );
    let _ = tx.execute(&vec_sql, &file_params[..]);

    let entities = tx.execute(
        &format!("DELETE FROM entities WHERE file_path IN ({files})"),
        &file_params[..],
    )?;
    tx.execute(
        &format!("DELETE FROM files WHERE path IN ({files})"),
        &file_params[..],
    )?;
    Ok((entities, relationships))
}

/// The half-open range `[low, high)` of the strings that are paths under
/// `root`, for an index-friendly `file_path >= low AND file_path < high`.
pub(crate) fn path_range(root: &Path) -> (String, String) {
    let separator = std::path::MAIN_SEPARATOR;
    let mut low = root.to_string_lossy().into_owned();
    if !low.ends_with(separator) {
        low.push(separator);
    }
    let mut high = low.clone();
    high.pop();
    // The character after the separator: no path under `root` reaches it.
    high.push(char::from(separator as u8 + 1));
    (low, high)
}

/// `DELETE` of the relationships of `kinds` (that many placeholders) whose
/// `side` (`source_id` or `target_id`) is in `ends`: placeholders, or a
/// subquery.
///
/// On the target side the kind is matched by value only (`+relationship_type`).
/// No index pairs the target with the kind, and with no statistics SQLite chose
/// the kind's own index, visiting every row of each kind in the database for
/// every statement: a quarter to a third of a second on an index of 1.3M edges,
/// paid by every re-parsed file. The target's index finds the few rows of
/// those targets instead. The source side has `idx_rel_source_type`.
fn delete_edges_sql(side: &str, kinds: usize, ends: &str) -> String {
    let kind = if side == "target_id" {
        "+relationship_type"
    } else {
        "relationship_type"
    };
    format!(
        "DELETE FROM relationships WHERE {kind} IN ({}) AND {side} IN ({ends})",
        placeholders(kinds)
    )
}

/// Delete the edges derived from one file's content: every non-reverse edge
/// whose source is an entity of the file, and every reverse edge whose target
/// is one, which are the reverse rows of the file's own forward edges.
/// `spellings` names the file in every spelling stored for it.
///
/// Edges another file's content produced into this file's entities stay: that
/// file was not re-parsed, so nothing would re-create them. The file's own
/// edges are stored again in the same run: the nesting of its members with
/// the file, and the rest, once every file of the run is stored, derived from
/// its references (`crate::graph::resolve`), or with the file too when
/// references do not resolve across files.
fn delete_edges_derived_from_file(
    tx: &rusqlite::Transaction<'_>,
    spellings: &[String],
) -> rusqlite::Result<usize> {
    let mut deleted = 0;
    for (reverse, side) in [(false, "source_id"), (true, "target_id")] {
        let kinds: Vec<&str> = RelationshipType::ALL
            .iter()
            .filter(|kind| kind.is_reverse() == reverse)
            .map(RelationshipType::as_str)
            .collect();
        let sql = delete_edges_sql(
            side,
            kinds.len(),
            &format!(
                "SELECT id FROM entities WHERE file_path IN ({})",
                placeholders(spellings.len())
            ),
        );
        let mut stmt = tx.prepare_cached(&sql)?;
        let params = kinds
            .iter()
            .map(|kind| kind as &dyn rusqlite::ToSql)
            .chain(spellings.iter().map(|s| s as &dyn rusqlite::ToSql));
        deleted += stmt.execute(rusqlite::params_from_iter(params))?;
    }
    Ok(deleted)
}

/// Attempts to acquire the write lock before giving up. Each attempt already
/// waits out `PRAGMA busy_timeout`, so this multiplies the effective patience.
const WRITE_LOCK_ATTEMPTS: u32 = 5;

/// True for the SQLite codes that mean "someone else holds the lock, try again".
fn is_contention(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// Begin a write transaction, waiting out contention from other writers.
///
/// Uses `BEGIN IMMEDIATE`, not the `BEGIN DEFERRED` that `unchecked_transaction`
/// emits. This matters: a deferred transaction takes a *read* snapshot first and
/// only upgrades to a write lock at the first write statement. In WAL mode, if
/// another connection committed in between, that upgrade fails immediately with
/// `SQLITE_BUSY_SNAPSHOT` — and SQLite does **not** invoke the busy handler for
/// it, so `PRAGMA busy_timeout` never applied. Concurrent tool calls against one
/// MCP server could therefore fail a scan outright with "database is locked".
///
/// `BEGIN IMMEDIATE` takes the write lock up front, where `busy_timeout` does
/// apply; the retry loop covers the case where it is exhausted anyway.
pub(crate) fn begin_write_transaction(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<rusqlite::Transaction<'_>> {
    let mut last_err = None;
    for attempt in 1..=WRITE_LOCK_ATTEMPTS {
        match rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => return Ok(tx),
            Err(e) if is_contention(&e) => {
                tracing::debug!(
                    "write lock busy (attempt {attempt}/{WRITE_LOCK_ATTEMPTS}); retrying"
                );
                last_err = Some(e);
                // Linear backoff; the busy handler has already slept once.
                std::thread::sleep(std::time::Duration::from_millis(50 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("loop runs at least once and only stores contention errors"))
}

/// Helper: run a closure inside a write transaction, commit on `Ok`, rollback on `Err`.
///
/// If `f` returns an error the transaction is rolled back automatically when `tx`
/// is dropped, but we also log the rollback for observability.
fn execute_in_transaction<T, E: From<rusqlite::Error> + std::fmt::Display>(
    conn: &rusqlite::Connection,
    f: impl FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let tx = begin_write_transaction(conn)?;
    match f(&tx) {
        Ok(value) => {
            tx.commit()?;
            Ok(value)
        }
        Err(e) => {
            if let Err(rollback_err) = tx.rollback() {
                tracing::error!("Failed to rollback transaction: {rollback_err}");
            } else {
                tracing::debug!("Rolled back transaction after error: {e}");
            }
            Err(e)
        }
    }
}

/// Parse a stored relationship-type string, `RelationshipType::as_str`'s form,
/// back to the enum. A value that is not one reads as `RelatedTo` rather than
/// failing the whole read.
fn parse_relationship_type(rel_type: &str) -> crate::entities::RelationshipType {
    crate::entities::RelationshipType::parse(rel_type)
        .unwrap_or(crate::entities::RelationshipType::RelatedTo)
}

/// Parse a timestamp column written by the insert paths.
///
/// Rows store `&OffsetDateTime` through rusqlite's `time` `ToSql`, whose output
/// the RFC 3339 reader accepts. A value that is not RFC 3339 reads as the Unix
/// epoch, with a warning, rather than failing the whole read: `created_at` is
/// non-critical bookkeeping, and a re-scan writes it again.
fn parse_db_timestamp(s: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap_or_else(
        |e| {
            tracing::warn!("non-RFC3339 timestamp {s:?} ({e}); using epoch (re-scan to fix)");
            time::OffsetDateTime::UNIX_EPOCH
        },
    )
}

/// Deserialise a `Relationship` from named columns on a rusqlite `Row`.
pub fn row_to_relationship(
    row: &rusqlite::Row,
) -> std::result::Result<Relationship, rusqlite::Error> {
    let id: String = row.get("id")?;
    let source_id: String = row.get("source_id")?;
    let target_id: String = row.get("target_id")?;
    let rel_type: String = row.get("relationship_type")?;
    let metadata: Option<String> = row.get("metadata")?;
    let created_at: String = row.get("created_at")?;

    let relationship_type = parse_relationship_type(&rel_type);

    let created_at = parse_db_timestamp(&created_at);

    Ok(Relationship {
        id: id.parse().map_err(|e| {
            rusqlite::Error::InvalidParameterName(format!(" malformed relationship id: {e}"))
        })?,
        source_id: source_id.parse().map_err(|e| {
            rusqlite::Error::InvalidParameterName(format!(" malformed source_id: {e}"))
        })?,
        target_id: target_id.parse().map_err(|e| {
            rusqlite::Error::InvalidParameterName(format!(" malformed target_id: {e}"))
        })?,
        relationship_type,
        metadata: metadata.and_then(|m| serde_json::from_str(&m).ok()),
        created_at,
    })
}

/// Insert or update a file
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn upsert_file(pool: &ConnectionPool, file_info: &FileInfo) -> Result<()> {
    let pool = pool.clone();
    let file_info = Arc::new(file_info.clone());

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        conn.execute(
            r#"
            INSERT INTO files (id, path, language, size_bytes, line_count, checksum, last_modified, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(path) DO UPDATE SET
                language = excluded.language,
                size_bytes = excluded.size_bytes,
                line_count = excluded.line_count,
                checksum = excluded.checksum,
                last_modified = excluded.last_modified,
                updated_at = excluded.updated_at
            "#,
            rusqlite::params![
                file_info.id.to_string(),
                file_info.path,
                file_info.language,
                file_info.size_bytes as i64,
                file_info.line_count as i64,
                file_info.checksum,
                file_info.last_modified.to_string(),
                file_info.created_at.to_string(),
                file_info.updated_at.to_string(),
            ],
        )?;
        Ok(())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Insert entities
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn insert_entities(pool: &ConnectionPool, entities: &[Entity]) -> Result<()> {
    let pool = pool.clone();
    let entities: Arc<[Entity]> = Arc::from(entities);

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        execute_in_transaction(&conn, |tx| {
            for entity in entities.iter() {
                let (id, name, file_path, body, location, language, data) = entity.to_db_fields()?;
                let entity_type = entity.entity_type();
                let now = time::OffsetDateTime::now_utc().to_string();
                let name_tokens = crate::database::embeddings::split_identifier_tokens(&name);
                tx.execute(
                    r#"
                    INSERT INTO entities (id, type, name, name_tokens, file_path, body, start_line, end_line, start_column, end_column, language, data, created_at, updated_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                    ON CONFLICT(id) DO UPDATE SET
                        name = excluded.name,
                        name_tokens = excluded.name_tokens,
                        body = excluded.body,
                        start_line = excluded.start_line,
                        end_line = excluded.end_line,
                        start_column = excluded.start_column,
                        end_column = excluded.end_column,
                        data = excluded.data,
                        updated_at = excluded.updated_at
                    "#,
                    rusqlite::params![
                        id,
                        entity_type,
                        name,
                        name_tokens,
                        file_path,
                        body,
                        location.start_line as i64,
                        location.end_line as i64,
                        location.start_column as i64,
                        location.end_column as i64,
                        language,
                        data.to_string(),
                            &now,
                        &now,
                    ],
                )?;
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Insert one relationship unless it is already stored (same source, target
/// and kind) or one of its ends is not (see [`insert_relationships_in`]).
const INSERT_RELATIONSHIP_IF_ENDS_EXIST: &str = "\
    INSERT INTO relationships (id, source_id, target_id, relationship_type, metadata, created_at) \
    SELECT ?1, ?2, ?3, ?4, ?5, ?6 \
    WHERE EXISTS (SELECT 1 FROM entities WHERE id = ?2) \
      AND EXISTS (SELECT 1 FROM entities WHERE id = ?3) \
    ON CONFLICT(source_id, target_id, relationship_type) DO NOTHING";

/// Insert `relationships` in one transaction, skipping any already stored
/// (same source, target and kind) and any whose source or target is not
/// stored. Returns, for each, whether it was inserted.
pub async fn insert_relationships(
    pool: &ConnectionPool,
    relationships: Vec<Relationship>,
) -> Result<Vec<bool>> {
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        execute_in_transaction(&conn, |tx| -> Result<Vec<bool>> {
            Ok(insert_relationships_in(tx, &relationships)?)
        })
    })
    .await
    .map_err(|e| crate::error::Error::task_join("insert edges", e))?
}

/// Batch-insert file info, entities and relationships for multiple parsed files
/// inside a single SQLite transaction.
///
/// `reindex_root` is the scan root when these files are being re-indexed into
/// a database that already holds them. Each file's previously derived edges
/// are then deleted first (see `delete_edges_derived_from_file`), in the same
/// transaction as the rows that replace them, so a call the file no longer
/// makes leaves no edge behind. `None` for a fresh index, where there is
/// nothing to replace.
///
/// Returns the number of relationship rows inserted. An edge that is already
/// stored (same source, target and kind) is skipped and not counted.
pub async fn batch_insert_parsed_files(
    pool: &ConnectionPool,
    batch: &[FileRecord],
    reindex_root: Option<&Path>,
) -> Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }

    let pool = pool.clone();
    let batch: ParsedFileBatch = Arc::from(batch);
    let reindex_root: Option<PathBuf> = reindex_root.map(Path::to_path_buf);

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let total_files = batch.len();
        let total_entities: usize = batch.iter().map(|record| record.entities.len()).sum();
        let total_rels: usize = batch.iter().map(|record| record.relationships.len()).sum();

        tracing::debug!(
            "batch_insert_parsed_files: starting batch of {total_files} files, {total_entities} entities, {total_rels} relationships"
        );

        let result = execute_in_transaction(&conn, |tx| {
            // Hoist the timestamp: one allocation for the entire transaction,
            // not once per entity.
            let now = time::OffsetDateTime::now_utc().to_string();

            // Pre-compile statements once per transaction via prepare_cached so
            // SQLite's statement cache (LRU in rusqlite) avoids redundant SQL
            // parsing for each of the potentially thousands of rows.
            let mut file_stmt = tx.prepare_cached(
                r#"INSERT INTO files (id, path, language, size_bytes, line_count, checksum, last_modified, created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                   ON CONFLICT(path) DO UPDATE SET
                       language = excluded.language,
                       size_bytes = excluded.size_bytes,
                       line_count = excluded.line_count,
                       checksum = excluded.checksum,
                       last_modified = excluded.last_modified,
                       updated_at = excluded.updated_at"#,
            )?;

            // `name_tokens` (the camelCase/snake_case parts of the name), which the FTS
            // trigger copies into entities_fts.
            let mut entity_stmt = tx.prepare_cached(
                r#"INSERT INTO entities (id, type, name, name_tokens, file_path, body, start_line, end_line, start_column, end_column, language, data, created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                   ON CONFLICT(id) DO UPDATE SET
                       name = excluded.name,
                       name_tokens = excluded.name_tokens,
                       body = excluded.body,
                       start_line = excluded.start_line,
                       end_line = excluded.end_line,
                       start_column = excluded.start_column,
                       end_column = excluded.end_column,
                       data = excluded.data,
                       updated_at = excluded.updated_at"#,
            )?;

            let mut rel_stmt = tx.prepare_cached(
                r#"INSERT INTO relationships (id, source_id, target_id, relationship_type, metadata, created_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(source_id, target_id, relationship_type) DO NOTHING"#,
            )?;

            // Best-effort: the vec0 table does not exist when vectors are
            // disabled, and then there is nothing to invalidate.
            let mut vector_delete = tx
                .prepare_cached("DELETE FROM entity_vectors WHERE entity_id = ?1")
                .ok();

            // A re-parsed file's references and imports replace its old ones.
            let mut ref_delete = tx.prepare_cached("DELETE FROM symbol_refs WHERE file_path = ?1")?;
            let mut import_delete =
                tx.prepare_cached("DELETE FROM file_imports WHERE file_path = ?1")?;
            let mut ref_stmt = tx.prepare_cached(
                "INSERT INTO symbol_refs (source_id, file_path, kind, target_name, qualifier, language, line) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut import_stmt = tx.prepare_cached(
                "INSERT INTO file_imports (file_path, module, imported_name, alias, line) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;

            let mut relationships_inserted = 0usize;
            for record in batch.iter() {
                let FileRecord {
                    file_info,
                    entities,
                    relationships,
                    refs,
                    imports,
                } = record;
                let file_path_str = &file_info.path;

                if reindex_root.is_some() {
                    delete_edges_derived_from_file(tx, std::slice::from_ref(file_path_str))?;
                }

                // On a re-index, the body each entity had, to tell which changed.
                let old_bodies: std::collections::HashMap<String, String> =
                    if reindex_root.is_some() {
                        let mut stmt =
                            tx.prepare_cached("SELECT id, body FROM entities WHERE file_path = ?1")?;
                        stmt.query_map([file_path_str], |row| Ok((row.get(0)?, row.get(1)?)))?
                            .collect::<rusqlite::Result<_>>()?
                    } else {
                        std::collections::HashMap::new()
                    };
                let mut changed_bodies: Vec<String> = Vec::new();

                file_stmt.execute(rusqlite::params![
                    file_info.id.to_string(),
                    file_info.path,
                    file_info.language,
                    file_info.size_bytes as i64,
                    file_info.line_count as i64,
                    file_info.checksum,
                    file_info.last_modified.to_string(),
                    file_info.created_at.to_string(),
                    file_info.updated_at.to_string(),
                ])?;

                for entity in entities {
                    let (id, name, file_path, body, location, language, data) =
                        entity.to_db_fields()?;
                    let entity_type = entity.entity_type();

                    if file_path != *file_path_str {
                        tracing::error!(
                            "FK VIOLATION: entity '{}' has file_path='{}' but parent file path='{}'. \
                             This will cause the entire batch transaction to roll back!",
                            name, file_path, file_path_str
                        );
                    }

                    if old_bodies.get(&id).is_some_and(|old| *old != body) {
                        changed_bodies.push(id.clone());
                    }

                    let name_tokens =
                        crate::database::embeddings::split_identifier_tokens(&name);
                    entity_stmt.execute(rusqlite::params![
                        id,
                        entity_type,
                        name,
                        name_tokens,
                        file_path,
                        body,
                        location.start_line as i64,
                        location.end_line as i64,
                        location.start_column as i64,
                        location.end_column as i64,
                        language,
                        data.to_string(),
                        &now,
                        &now,
                    ])?;
                }

                // A vector embeds its entity's body; once the body changes it
                // describes code that is gone. Dropping it makes the entity
                // count as unembedded, so it is embedded again on demand or by
                // the next embed run. Unchanged entities keep theirs: their ids
                // are stable, so their vectors still match.
                if let Some(stmt) = vector_delete.as_mut() {
                    for id in &changed_bodies {
                        let _ = stmt.execute([id]);
                    }
                }

                for rel in relationships {
                    relationships_inserted += rel_stmt.execute(rusqlite::params![
                        rel.id.to_string(),
                        rel.source_id.to_string(),
                        rel.target_id.to_string(),
                        rel.relationship_type.as_str(),
                        rel.metadata.as_ref().map(|m| m.to_string()),
                        &rel.created_at,
                    ])?;
                }

                ref_delete.execute([file_path_str])?;
                for reference in refs {
                    ref_stmt.execute(rusqlite::params![
                        reference.source_id.to_string(),
                        file_path_str,
                        reference.kind.as_str(),
                        reference.target_name,
                        reference.qualifier,
                        file_info.language,
                        i64::from(reference.line),
                    ])?;
                }
                // `alias` holds the name the import binds in the file,
                // `imported_name` the member it names, if any, and `line` where.
                import_delete.execute([file_path_str])?;
                for import in imports {
                    import_stmt.execute(rusqlite::params![
                        file_path_str,
                        import.source_module,
                        import.original_name,
                        import.local_name,
                        i64::from(import.line),
                    ])?;
                }
            }
            Ok(relationships_inserted)
        });

        if let Err(ref e) = result {
            // Run FK integrity check to give a more actionable diagnostic.
            if let Ok(fk_rows) = conn.execute("PRAGMA foreign_key_check", [])
                && fk_rows > 0
            {
                tracing::error!(
                    "FK integrity check found {fk_rows} violations after failed batch insert: {e}"
                );
            }
        } else {
            tracing::debug!(
                "batch_insert_parsed_files: committed {total_files} files, {total_entities} entities, {total_rels} relationships"
            );
        }

        result
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// The references the entities `source_ids` make, with the file each was
/// made in and its language, in the order they were stored. A kind this
/// build does not know is skipped.
pub async fn get_symbol_refs_for_sources(
    pool: &ConnectionPool,
    source_ids: &[EntityId],
) -> Result<Vec<StoredRef>> {
    let pool = pool.clone();
    let ids: Vec<String> = source_ids.iter().map(ToString::to_string).collect();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut refs = Vec::new();
        for chunk in ids.chunks(900) {
            let sql = format!(
                "SELECT source_id, kind, target_name, qualifier, line, file_path, language \
                 FROM symbol_refs WHERE source_id IN ({}) ORDER BY id",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    (
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ),
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            for row in rows {
                let ((source_id, kind, target_name, qualifier, line), file_path, language) = row?;
                let (Ok(source_id), Ok(kind)) = (
                    uuid::Uuid::parse_str(&source_id),
                    RelationshipType::parse(&kind),
                ) else {
                    continue;
                };
                refs.push(StoredRef {
                    reference: SymbolRef {
                        source_id,
                        kind,
                        target_name,
                        qualifier,
                        line: u32::try_from(line).unwrap_or(0),
                    },
                    file_path,
                    language,
                });
            }
        }
        Ok(refs)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read references", e))?
}

/// Insert relationships inside `tx`, skipping any that already exist (same
/// source, target and kind) and any whose source or target is not stored.
/// Returns, for each input row, whether it was inserted.
///
/// An endpoint is missing when another writer (the watcher, the query-time
/// freshness check, a scan in another process) deleted it after the
/// definitions a reference resolved against were read. With foreign keys on,
/// that one edge would fail the transaction.
fn insert_relationships_in(
    tx: &rusqlite::Transaction<'_>,
    relationships: &[Relationship],
) -> rusqlite::Result<Vec<bool>> {
    let mut stmt = tx.prepare_cached(INSERT_RELATIONSHIP_IF_ENDS_EXIST)?;
    let mut inserted = Vec::with_capacity(relationships.len());
    for rel in relationships {
        let rows = stmt.execute(rusqlite::params![
            rel.id.to_string(),
            rel.source_id.to_string(),
            rel.target_id.to_string(),
            rel.relationship_type.as_str(),
            rel.metadata
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_default(),
            &rel.created_at,
        ])?;
        inserted.push(rows == 1);
    }
    Ok(inserted)
}

/// See [`super::Database::replace_edges_of_sources`].
pub async fn replace_edges_of_sources(
    pool: &ConnectionPool,
    source_ids: &[EntityId],
    edges: Vec<Relationship>,
) -> Result<Vec<bool>> {
    let pool = pool.clone();
    let ids: Vec<String> = source_ids.iter().map(ToString::to_string).collect();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        execute_in_transaction(&conn, |tx| -> Result<Vec<bool>> {
            // The forward edges the sources' references yield, and their
            // reverse rows; not the nesting, which comes from the file.
            let forward: Vec<&str> = RelationshipType::ALL
                .iter()
                .filter(|kind| !kind.is_reverse() && !kind.is_nesting())
                .map(RelationshipType::as_str)
                .collect();
            let reverse: Vec<&str> = RelationshipType::ALL
                .iter()
                .filter(|kind| kind.is_reverse())
                .map(RelationshipType::as_str)
                .collect();
            for chunk in ids.chunks(800) {
                for (kinds, side) in [(&forward, "source_id"), (&reverse, "target_id")] {
                    let sql = delete_edges_sql(side, kinds.len(), &placeholders(chunk.len()));
                    let params = kinds
                        .iter()
                        .map(|kind| kind as &dyn rusqlite::ToSql)
                        .chain(chunk.iter().map(|id| id as &dyn rusqlite::ToSql));
                    tx.execute(&sql, rusqlite::params_from_iter(params))?;
                }
            }
            Ok(insert_relationships_in(tx, &edges)?)
        })
    })
    .await
    .map_err(|e| crate::error::Error::task_join("replace edges", e))?
}

/// See [`super::Database::get_file_imports`]. `alias` holds the name an
/// import binds, `imported_name` the member it names (none for a whole
/// module or a wildcard, whose binding is `*`).
pub async fn get_file_imports(
    pool: &ConnectionPool,
    paths: &[String],
) -> Result<std::collections::HashMap<String, Vec<ImportInfo>>> {
    let pool = pool.clone();
    let paths = paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut imports: std::collections::HashMap<String, Vec<ImportInfo>> =
            std::collections::HashMap::new();
        for chunk in paths.chunks(900) {
            let sql = format!(
                "SELECT file_path, module, imported_name, alias, line FROM file_imports \
                 WHERE file_path IN ({}) ORDER BY rowid",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })?;
            for row in rows {
                let (file_path, module, imported_name, alias, line) = row?;
                let local_name = alias
                    .or_else(|| imported_name.clone())
                    .unwrap_or_else(|| module.clone());
                imports.entry(file_path).or_default().push(ImportInfo {
                    is_wildcard: local_name == "*",
                    local_name,
                    source_module: module,
                    original_name: imported_name,
                    line: line.and_then(|line| u32::try_from(line).ok()).unwrap_or(0),
                });
            }
        }
        Ok(imports)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read imports", e))?
}

/// See [`super::Database::get_symbol_entries_named`].
pub async fn get_symbol_entries_named(
    pool: &ConnectionPool,
    names: &[String],
    root: &Path,
) -> Result<Vec<SymbolEntry>> {
    let pool = pool.clone();
    let names = names.to_vec();
    let (low, high) = path_range(root);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut entries = Vec::new();
        for chunk in names.chunks(900) {
            let sql = format!(
                "SELECT id, name, file_path, language, type, start_line, \
                        json_extract(data, '$.parent_class') FROM entities \
                 WHERE name IN ({}) AND file_path >= ? AND file_path < ?",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let params = chunk.iter().chain([&low, &high]);
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            for row in rows {
                let (id, name, file_path, language, entity_type, start_line, owner) = row?;
                let Ok(id) = uuid::Uuid::parse_str(&id) else {
                    continue;
                };
                entries.push(SymbolEntry {
                    id,
                    name,
                    file_path: PathBuf::from(file_path),
                    start_line,
                    language,
                    entity_type,
                    owner,
                });
            }
        }
        Ok(entries)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read symbol entries", e))?
}

/// See [`super::Database::get_member_owners`].
pub async fn get_member_owners(
    pool: &ConnectionPool,
    ids: &[EntityId],
) -> Result<Vec<(EntityId, String)>> {
    let pool = pool.clone();
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut owners = Vec::new();
        for chunk in ids.chunks(900) {
            let sql = format!(
                "SELECT id, json_extract(data, '$.parent_class') FROM entities \
                 WHERE id IN ({}) AND json_extract(data, '$.parent_class') IS NOT NULL",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, owner) = row?;
                if let Ok(id) = uuid::Uuid::parse_str(&id) {
                    owners.push((id, owner));
                }
            }
        }
        Ok(owners)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read member owners", e))?
}

/// See [`super::Database::get_definitions_in_files`].
pub async fn get_definitions_in_files(
    pool: &ConnectionPool,
    paths: &[String],
) -> Result<Vec<(String, EntityId)>> {
    let pool = pool.clone();
    let paths = paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut definitions = Vec::new();
        for chunk in paths.chunks(900) {
            let sql = format!(
                "SELECT name, id FROM entities WHERE file_path IN ({}) \
                 ORDER BY file_path, start_line, id",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (name, id) = row?;
                if let Ok(id) = uuid::Uuid::parse_str(&id) {
                    definitions.push((name, id));
                }
            }
        }
        Ok(definitions)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read definitions", e))?
}

/// See [`super::Database::get_sources_referring_to`].
pub async fn get_sources_referring_to(
    pool: &ConnectionPool,
    root: &Path,
    names: &[String],
) -> Result<Vec<(EntityId, String)>> {
    let pool = pool.clone();
    let names = names.to_vec();
    let (low, high) = path_range(root);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut sources = HashSet::new();
        for chunk in names.chunks(400) {
            // A reference by the name itself, or by the alias an import of
            // its file binds to it (`from m import name as alias`), which
            // resolution looks up by the imported name.
            let names = placeholders(chunk.len());
            let sql = format!(
                "SELECT source_id, file_path FROM symbol_refs \
                 WHERE target_name IN ({names}) AND file_path >= ? AND file_path < ? \
                 UNION \
                 SELECT r.source_id, r.file_path FROM file_imports i \
                 JOIN symbol_refs r ON r.file_path = i.file_path AND r.target_name = i.alias \
                 WHERE i.imported_name IN ({names}) AND i.file_path >= ? AND i.file_path < ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let bounds = [&low, &high];
            let params = chunk.iter().chain(bounds).chain(chunk.iter()).chain(bounds);
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source_id, file_path) = row?;
                if let Ok(source_id) = uuid::Uuid::parse_str(&source_id) {
                    sources.insert((source_id, file_path));
                }
            }
        }
        let mut sources: Vec<(EntityId, String)> = sources.into_iter().collect();
        sources.sort();
        Ok(sources)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read referring sources", e))?
}

/// See [`super::Database::get_reference_sites`].
pub async fn get_reference_sites(
    pool: &ConnectionPool,
    root: &Path,
    names: &[String],
) -> Result<Vec<(String, u32, String)>> {
    let pool = pool.clone();
    let names = names.to_vec();
    let (low, high) = path_range(root);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut sites = std::collections::BTreeSet::new();
        for chunk in names.chunks(400) {
            let names = placeholders(chunk.len());
            let sql = format!(
                "SELECT file_path, line, language FROM symbol_refs \
                 WHERE target_name IN ({names}) AND file_path >= ? AND file_path < ? \
                 UNION \
                 SELECT r.file_path, r.line, r.language FROM file_imports i \
                 JOIN symbol_refs r ON r.file_path = i.file_path AND r.target_name = i.alias \
                 WHERE i.imported_name IN ({names}) AND i.file_path >= ? AND i.file_path < ? \
                 UNION \
                 SELECT i.file_path, i.line, f.language FROM file_imports i \
                 JOIN files f ON f.path = i.file_path \
                 WHERE (i.imported_name IN ({names}) \
                        OR (i.imported_name IS NULL AND i.alias IN ({names}))) \
                 AND i.line IS NOT NULL AND i.file_path >= ? AND i.file_path < ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let bounds = [&low, &high];
            let params = chunk
                .iter()
                .chain(bounds)
                .chain(chunk.iter())
                .chain(bounds)
                .chain(chunk.iter())
                .chain(chunk.iter())
                .chain(bounds);
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (file, line, language) = row?;
                if let Ok(line) = u32::try_from(line) {
                    sites.insert((file, line, language));
                }
            }
        }
        Ok(sites.into_iter().collect())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read reference sites", e))?
}

/// See [`super::Database::get_call_sites`].
pub async fn get_call_sites(
    pool: &ConnectionPool,
    root: &Path,
    names: &[String],
) -> Result<Vec<(String, u32, String, String)>> {
    let pool = pool.clone();
    let names = names.to_vec();
    let (low, high) = path_range(root);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut sites = std::collections::BTreeSet::new();
        for chunk in names.chunks(400) {
            let names = placeholders(chunk.len());
            let sql = format!(
                "SELECT r.file_path, r.line, r.language, e.name FROM symbol_refs r \
                 JOIN entities e ON e.id = r.source_id \
                 WHERE r.kind = 'calls' AND r.target_name IN ({names}) \
                 AND r.file_path >= ? AND r.file_path < ? \
                 UNION \
                 SELECT r.file_path, r.line, r.language, e.name FROM file_imports i \
                 JOIN symbol_refs r ON r.file_path = i.file_path AND r.target_name = i.alias \
                 JOIN entities e ON e.id = r.source_id \
                 WHERE r.kind = 'calls' AND i.imported_name IN ({names}) \
                 AND i.file_path >= ? AND i.file_path < ?"
            );
            let mut stmt = conn.prepare(&sql)?;
            let bounds = [&low, &high];
            let params = chunk.iter().chain(bounds).chain(chunk.iter()).chain(bounds);
            let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (file, line, language, caller) = row?;
                if let Ok(line) = u32::try_from(line) {
                    sites.insert((file, line, language, caller));
                }
            }
        }
        Ok(sites.into_iter().collect())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read call sites", e))?
}

/// See [`super::Database::count_entities_under`].
pub async fn count_entities_under(pool: &ConnectionPool, root: &Path) -> Result<usize> {
    let pool = pool.clone();
    let (low, high) = path_range(root);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM entities WHERE file_path >= ?1 AND file_path < ?2",
            rusqlite::params![low, high],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    })
    .await
    .map_err(|e| crate::error::Error::task_join("count entities", e))?
}

/// FTS5-based BM25 entity search with optional entity-type pre-filter.
///
/// Returns results ranked by BM25 score (best match first).
/// When `entity_type` is `Some(t)` only entities of that type are returned.
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn fts_search_entities(
    pool: &ConnectionPool,
    query: &str,
    limit: usize,
    entity_type: Option<&str>,
) -> Result<Vec<Entity>> {
    // The query as FTS5 must see it: see `fts_query`.
    let Some(query) = super::fts_query::match_query(query) else {
        return Ok(Vec::new());
    };
    let pool = pool.clone();
    let entity_type = entity_type.map(str::to_string);

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;

        let entities = if let Some(ref et) = entity_type {
            let mut stmt = conn.prepare_cached(
                "SELECT e.type, e.data
                 FROM entities e
                 INNER JOIN (
                     SELECT rowid, rank
                     FROM entities_fts
                     WHERE entities_fts MATCH ?1
                     ORDER BY rank
                     LIMIT ?2
                 ) fts ON e.rowid = fts.rowid
                 WHERE e.type = ?3
                 ORDER BY fts.rank",
            )?;
            stmt.query_map(rusqlite::params![query, limit as i64, et], row_to_entity)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare_cached(
                "SELECT e.type, e.data
                 FROM entities e
                 INNER JOIN (
                     SELECT rowid, rank
                     FROM entities_fts
                     WHERE entities_fts MATCH ?1
                     ORDER BY rank
                     LIMIT ?2
                 ) fts ON e.rowid = fts.rowid
                 ORDER BY fts.rank",
            )?;
            stmt.query_map(rusqlite::params![query, limit as i64], row_to_entity)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Search entities by name (fuzzy)
pub async fn search_entities(
    pool: &ConnectionPool,
    query: &str,
    limit: usize,
) -> Result<Vec<Entity>> {
    let pool = pool.clone();
    let query = query.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT type, data
            FROM entities
            WHERE name LIKE ?1 ESCAPE '\'
            LIMIT ?2
            "#,
        )?;

        let pattern = format!("%{}%", escape_like(&query));
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let entities = stmt
            .query_map(rusqlite::params![&pattern, limit_i64], row_to_entity)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get entities by exact name match
pub async fn get_entities_by_exact_name(
    pool: &ConnectionPool,
    name: &str,
    limit: usize,
) -> Result<Vec<Entity>> {
    let pool = pool.clone();
    let name = name.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT type, data
            FROM entities
            WHERE name = ?1
            LIMIT ?2
            "#,
        )?;

        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let entities = stmt
            .query_map(rusqlite::params![&name, limit_i64], row_to_entity)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get entities by file path
/// Whether `file_path` has a row in the `files` table.
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn file_is_indexed(pool: &ConnectionPool, file_path: &str) -> Result<bool> {
    let pool = pool.clone();
    let file_path = file_path.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let exists = conn
            .prepare_cached("SELECT 1 FROM files WHERE path = ?1 LIMIT 1")?
            .exists(rusqlite::params![&file_path])?;
        Ok(exists)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get all entities recorded for a file.
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn get_entities_by_file(pool: &ConnectionPool, file_path: &str) -> Result<Vec<Entity>> {
    let pool = pool.clone();
    let file_path = file_path.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT type, data
            FROM entities WHERE file_path = ?1
            "#,
        )?;

        let entities = stmt
            .query_map(rusqlite::params![&file_path], row_to_entity)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get relationships for an entity
pub async fn get_relationships(
    pool: &ConnectionPool,
    entity_id: EntityId,
) -> Result<Vec<Relationship>> {
    let pool = pool.clone();
    let entity_id = entity_id.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, relationship_type, metadata, created_at \
             FROM relationships WHERE source_id = ?1 OR target_id = ?1",
        )?;

        let relationships = stmt
            .query_map(rusqlite::params![&entity_id], row_to_relationship)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(relationships)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get relationships for multiple entities in a single query.
///
/// This is the batched variant of [`get_relationships`]: the call-graph BFS
/// makes one query per level, not one per node.
pub async fn get_relationships_for_ids(
    pool: &ConnectionPool,
    entity_ids: &[String],
) -> Result<Vec<Relationship>> {
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }

    let pool = pool.clone();
    let ids = entity_ids.to_vec();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, source_id, target_id, relationship_type, metadata, created_at \
             FROM relationships WHERE source_id IN ({}) OR target_id IN ({})",
            placeholders, placeholders
        );
        let mut stmt = conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::ToSql> = ids
            .iter()
            .chain(ids.iter())
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();

        let relationships = stmt
            .query_map(&params[..], row_to_relationship)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(relationships)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get all functions from the database
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn get_all_functions(pool: &ConnectionPool) -> Result<Vec<crate::entities::Function>> {
    let pool = pool.clone();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT type, data
            FROM entities
            WHERE type = 'function'
            "#,
        )?;

        let entities = stmt
            .query_map([], |row| {
                let entity = row_to_entity(row)?;
                match entity {
                    crate::entities::Entity::Function(f) => Ok(f),
                    _ => Err(rusqlite::Error::InvalidParameterName(
                        "Expected function".to_string(),
                    )),
                }
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get all classes from the database
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn get_all_classes(pool: &ConnectionPool) -> Result<Vec<crate::entities::Class>> {
    let pool = pool.clone();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            r#"
            SELECT type, data
            FROM entities
            WHERE type = 'class'
            "#,
        )?;

        let entities = stmt
            .query_map([], |row| {
                let entity = row_to_entity(row)?;
                match entity {
                    crate::entities::Entity::Class(c) => Ok(c),
                    _ => Err(rusqlite::Error::InvalidParameterName(
                        "Expected class".to_string(),
                    )),
                }
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entities)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Get all relationships from the database
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn get_all_relationships(
    pool: &ConnectionPool,
) -> Result<Vec<crate::entities::Relationship>> {
    let pool = pool.clone();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, relationship_type, metadata, created_at \
             FROM relationships",
        )?;

        let relationships = stmt
            .query_map([], row_to_relationship)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(relationships)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("blocking task", e))?
}

/// Scan manifest row
#[derive(Debug, Clone)]
pub struct ScanManifestEntry {
    /// File path
    pub path: String,
    /// Content hash (XXH3 hex)
    pub content_hash: String,
    /// Last parsed timestamp (ISO 8601)
    pub last_parsed_at: String,
    /// File size in bytes at last parse
    pub size_bytes: u64,
    /// File mtime as UNIX epoch seconds at last parse
    pub mtime_secs: u64,
}

/// Load the entire scan_manifest into memory.
pub async fn load_scan_manifest(pool: &ConnectionPool) -> Result<Vec<ScanManifestEntry>> {
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt = conn.prepare_cached(
            "SELECT path, content_hash, last_parsed_at, size_bytes, mtime_secs FROM scan_manifest",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ScanManifestEntry {
                    path: row.get(0)?,
                    content_hash: row.get(1)?,
                    last_parsed_at: row.get(2)?,
                    size_bytes: row.get::<_, i64>(3).map(|v| v as u64).unwrap_or(0),
                    mtime_secs: row.get::<_, i64>(4).map(|v| v as u64).unwrap_or(0),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("load scan manifest", e))?
}

/// The manifest rows of `paths` (those it has).
pub async fn get_scan_manifest_entries(
    pool: &ConnectionPool,
    paths: &[String],
) -> Result<Vec<ScanManifestEntry>> {
    let pool = pool.clone();
    let paths = paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut entries = Vec::new();
        for chunk in paths.chunks(900) {
            let sql = format!(
                "SELECT path, content_hash, last_parsed_at, size_bytes, mtime_secs \
                 FROM scan_manifest WHERE path IN ({})",
                placeholders(chunk.len())
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok(ScanManifestEntry {
                    path: row.get(0)?,
                    content_hash: row.get(1)?,
                    last_parsed_at: row.get(2)?,
                    size_bytes: row.get::<_, i64>(3).map(|v| v as u64).unwrap_or(0),
                    mtime_secs: row.get::<_, i64>(4).map(|v| v as u64).unwrap_or(0),
                })
            })?;
            for row in rows {
                entries.push(row?);
            }
        }
        Ok(entries)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("get scan manifest entries", e))?
}

/// The manifest paths inside directory `dir`: the files a scan indexed there.
pub async fn scan_manifest_paths_under(pool: &ConnectionPool, dir: &Path) -> Result<Vec<String>> {
    let pool = pool.clone();
    let (low, high) = path_range(dir);
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut stmt =
            conn.prepare("SELECT path FROM scan_manifest WHERE path >= ?1 AND path < ?2")?;
        let paths = stmt
            .query_map(rusqlite::params![low, high], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(paths)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("scan manifest paths under", e))?
}

/// The `metadata` value of `key`, if set.
pub async fn get_metadata(pool: &ConnectionPool, key: &str) -> Result<Option<String>> {
    let pool = pool.clone();
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let value = conn
            .query_row("SELECT value FROM metadata WHERE key = ?1", [&key], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("read metadata", e))?
}

/// Add `root` to the JSON array of `metadata` key `indexed_roots`, once.
pub async fn add_indexed_root(pool: &ConnectionPool, root: &Path) -> Result<()> {
    let pool = pool.clone();
    let root = root.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get()?;
        let tx = conn.transaction()?;
        let stored: Option<String> = tx
            .query_row(
                "SELECT value FROM metadata WHERE key = 'indexed_roots'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut roots: Vec<String> = stored
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        if !roots.contains(&root) {
            roots.push(root);
            roots.sort();
            tx.execute(
                "INSERT INTO metadata (key, value, updated_at) VALUES ('indexed_roots', ?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, \
                 updated_at = excluded.updated_at",
                rusqlite::params![
                    serde_json::to_string(&roots).unwrap_or_else(|_| "[]".to_string()),
                    time::OffsetDateTime::now_utc().to_string()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("record an indexed root", e))?
}

/// Upsert a scan_manifest row.
pub async fn upsert_scan_manifest(
    pool: &ConnectionPool,
    path: &str,
    content_hash: &str,
    last_parsed_at: &str,
    size_bytes: u64,
    mtime_secs: u64,
) -> Result<()> {
    let pool = pool.clone();
    let path = path.to_string();
    let content_hash = content_hash.to_string();
    let last_parsed_at = last_parsed_at.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        conn.prepare_cached(
            "INSERT INTO scan_manifest (path, content_hash, last_parsed_at, size_bytes, mtime_secs)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
               content_hash = excluded.content_hash,
               last_parsed_at = excluded.last_parsed_at,
               size_bytes = excluded.size_bytes,
               mtime_secs = excluded.mtime_secs",
        )?
        .execute(rusqlite::params![
            &path,
            &content_hash,
            &last_parsed_at,
            size_bytes as i64,
            mtime_secs as i64,
        ])?;
        Ok(())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("upsert scan manifest", e))?
}

/// Upsert multiple scan_manifest rows in a single transaction.
///
/// Runs all upserts inside one `BEGIN`/`COMMIT` so incremental scans with
/// many changed files don't pay one `spawn_blocking`+transaction per file.
pub async fn batch_upsert_scan_manifest(
    pool: &ConnectionPool,
    entries: Vec<(String, String, String, u64, u64)>,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let tx = begin_write_transaction(&conn)?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO scan_manifest (path, content_hash, last_parsed_at, size_bytes, mtime_secs)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path) DO UPDATE SET
                   content_hash = excluded.content_hash,
                   last_parsed_at = excluded.last_parsed_at,
                   size_bytes = excluded.size_bytes,
                   mtime_secs = excluded.mtime_secs",
            )?;
            for (path, content_hash, last_parsed_at, size_bytes, mtime_secs) in &entries {
                stmt.execute(rusqlite::params![
                    path,
                    content_hash,
                    last_parsed_at,
                    *size_bytes as i64,
                    *mtime_secs as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
    .await
    .map_err(|e| crate::error::Error::task_join("batch upsert scan manifest", e))?
}

/// Delete scan_manifest rows for removed files.
pub async fn delete_scan_manifest_entries(
    pool: &ConnectionPool,
    paths: &[String],
) -> Result<usize> {
    let pool = pool.clone();
    let paths = paths.to_vec();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut total = 0usize;
        // SQLite has a limit on host parameters (usually 999); chunk deletes.
        for chunk in paths.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM scan_manifest WHERE path IN ({placeholders})");
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            total += conn.execute(&sql, &params[..])? as usize;
        }
        Ok(total)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("delete scan manifest", e))?
}

/// Delete everything indexed for a set of files that no longer exist: their
/// entities, every edge touching those entities, their vectors, and their
/// `files` rows (so file counts leave them out), in one transaction.
///
/// Returns `(entities_deleted, relationships_deleted)`. Uses chunking to stay
/// under SQLite's host parameter limit.
pub async fn delete_entities_by_file_paths(
    pool: &ConnectionPool,
    paths: &[String],
) -> Result<(usize, usize)> {
    if paths.is_empty() {
        return Ok((0, 0));
    }

    let pool = pool.clone();
    let paths = paths.to_vec();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        execute_in_transaction(&conn, |tx| {
            let mut total_entities = 0usize;
            let mut total_rels = 0usize;
            for chunk in paths.chunks(900) {
                let (entities, relationships) = delete_indexed_files(tx, chunk)?;
                total_entities += entities;
                total_rels += relationships;
            }
            Ok::<_, crate::error::Error>((total_entities, total_rels))
        })
    })
    .await
    .map_err(|e| crate::error::Error::task_join("delete entities by file", e))?
}

/// Reconcile each re-parsed file's entities against the database: delete the
/// entities (and their vectors) that are **no longer present** in the new parse,
/// while leaving unchanged entities — and the embeddings keyed by their stable
/// ids — untouched.
///
/// For each `(file_path, kept_ids)` this removes rows of that file whose id is
/// not in `kept_ids`: first from `entity_vectors` (a vec0 virtual table with no
/// FK cascade), then from `entities` (relationships cascade). An empty
/// `kept_ids` means the file now has no entities, so all are removed.
///
/// Combined with content-stable entity ids, this is the cleanup half of
/// reliable incremental indexing: re-parsing a changed file refreshes/keeps the
/// entities that remain and cleanly drops the ones that vanished, with no
/// orphaned vectors.
///
/// Returns the number of entities deleted.
///
/// # Errors
/// Returns an error if a database operation fails.
pub async fn reconcile_file_entities(
    pool: &ConnectionPool,
    files: &[(String, Vec<EntityId>)],
) -> Result<usize> {
    if files.is_empty() {
        return Ok(0);
    }

    let pool = pool.clone();
    // Stringify ids up front so the blocking task owns plain data.
    let files: Vec<(String, Vec<String>)> = files
        .iter()
        .map(|(path, ids)| (path.clone(), ids.iter().map(ToString::to_string).collect()))
        .collect();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let tx = begin_write_transaction(&conn)?;
        // A connection-local temp table holds the surviving ids for one file at a
        // time, so reconciliation scales to files with thousands of entities
        // (avoids a giant `NOT IN (?, ?, …)` parameter list).
        tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS _recon_kept(id TEXT PRIMARY KEY)")?;

        let mut removed = 0usize;
        for (file_path, kept) in &files {
            tx.execute("DELETE FROM _recon_kept", [])?;
            {
                let mut ins =
                    tx.prepare_cached("INSERT OR IGNORE INTO _recon_kept(id) VALUES (?1)")?;
                for id in kept {
                    ins.execute([id])?;
                }
            }

            // Vectors first (no FK cascade); best-effort if vectors are disabled.
            let _ = tx.execute(
                "DELETE FROM entity_vectors WHERE entity_id IN (
                    SELECT id FROM entities
                    WHERE file_path = ?1 AND id NOT IN (SELECT id FROM _recon_kept)
                 )",
                [file_path],
            );
            removed += tx.execute(
                "DELETE FROM entities
                 WHERE file_path = ?1 AND id NOT IN (SELECT id FROM _recon_kept)",
                [file_path],
            )?;
        }

        tx.commit()?;
        Ok(removed)
    })
    .await
    .map_err(|e| crate::error::Error::task_join("reconcile file entities", e))?
}

/// Convert a database row to an Entity.
/// Delegates to `vector::data_to_entity` which is the single canonical type→variant mapping.
///
/// # Errors
/// Returns an error if the row data is invalid
pub fn row_to_entity(row: &rusqlite::Row) -> std::result::Result<Entity, rusqlite::Error> {
    let entity_type: String = row.get("type")?;
    let data: String = row.get("data")?;
    crate::database::vector::data_to_entity(&entity_type, &data)
        .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan SQLite makes for `sql` on a database with the index schema.
    fn plan(sql: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        crate::database::schema::create_tables(&conn).expect("schema");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("plan");
        let unbound = std::iter::repeat_n(rusqlite::types::Null, stmt.parameter_count());
        stmt.query_map(rusqlite::params_from_iter(unbound), |row| {
            row.get::<_, String>(3)
        })
        .expect("plan rows")
        .collect::<rusqlite::Result<_>>()
        .expect("read the plan")
    }

    /// The reverse rows of a set of entities' edges are found through the
    /// target, not by visiting every row of their kinds.
    #[test]
    fn reverse_rows_are_deleted_through_the_target_index() {
        let subquery = "SELECT id FROM entities WHERE file_path IN (?)";
        for ends in ["?,?,?", subquery] {
            let reverse = plan(&delete_edges_sql("target_id", 6, ends));
            assert!(
                reverse.iter().any(|step| step.contains("idx_rel_target")),
                "{reverse:?}"
            );
            assert!(
                !reverse.iter().any(|step| step.contains("idx_rel_type")),
                "{reverse:?}"
            );
            let forward = plan(&delete_edges_sql("source_id", 8, ends));
            assert!(
                forward
                    .iter()
                    .any(|step| step.contains("idx_rel_source_type")),
                "{forward:?}"
            );
        }
    }

    /// The range holds exactly the paths under the root, whichever separator
    /// the platform uses.
    #[test]
    fn path_range_holds_exactly_the_paths_under_the_root() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = PathBuf::from(format!("{sep}srv{sep}app"));
        let (low, high) = path_range(&root);
        let under = |path: String| path >= low && path < high;

        assert!(under(format!("{sep}srv{sep}app{sep}x.rs")));
        assert!(under(format!("{sep}srv{sep}app{sep}deep{sep}y.rs")));
        assert!(!under(format!("{sep}srv{sep}app")), "the root itself");
        assert!(
            !under(format!("{sep}srv{sep}apple{sep}x.rs")),
            "a sibling sharing the prefix"
        );
        assert!(!under(format!("{sep}srv{sep}other{sep}x.rs")));
        assert!(!under(format!(".{sep}app{sep}x.rs")), "a relative spelling");
    }

    /// A root that already ends with the separator gets no second one.
    #[test]
    fn path_range_of_a_root_ending_in_a_separator() {
        let sep = std::path::MAIN_SEPARATOR;
        let (low, _) = path_range(Path::new(&format!("{sep}srv{sep}")));
        assert_eq!(low, format!("{sep}srv{sep}"));
    }
}
