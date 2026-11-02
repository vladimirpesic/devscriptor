//! sqlite-vec integration for vector similarity search

use crate::database::connection::ConnectionPool;
use crate::embeddings::EMBEDDING_DIMENSION;
use crate::entities::{Entity, EntityId};
use crate::error::{Error, Result};
use rusqlite::Connection;
use rusqlite::types::ToSql;

/// Look up pre-computed vectors from the embedding cache for a batch of
/// text hashes.  Returns a map `text_hash → f32 vector` for every cache hit.
///
/// Callers should compute `text_hash = format!("{:016x}", xxh3_64(text.as_bytes()))`
/// and use this to skip ONNX inference for unchanged entity text.
pub async fn lookup_embedding_cache(
    pool: &ConnectionPool,
    text_hashes: &[String],
) -> Result<std::collections::HashMap<String, Vec<f32>>> {
    if text_hashes.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let pool = pool.clone();
    let hashes = text_hashes.to_vec();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut result = std::collections::HashMap::with_capacity(hashes.len());

        for chunk in hashes.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT text_hash, vector FROM embedding_cache WHERE text_hash IN ({})",
                placeholders
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let params: Vec<&dyn ToSql> = chunk.iter().map(|h| h as &dyn ToSql).collect();

            let rows = stmt
                .query_map(&params[..], |row| {
                    let hash: String = row.get(0)?;
                    let bytes: Vec<u8> = row.get(1)?;
                    Ok((hash, bytes))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            for (hash, bytes) in rows {
                let floats: Vec<f32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                result.insert(hash, floats);
            }
        }
        Ok(result)
    })
    .await
    .map_err(|e| Error::task_join("lookup embedding cache", e))?
}

/// Store newly computed vectors into the embedding cache.
///
/// Items that already exist in the cache are updated (UPSERT).
pub async fn store_embedding_cache(
    pool: &ConnectionPool,
    items: &[(String, Vec<f32>)], // (text_hash, vector)
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let pool = pool.clone();
    let now = time::OffsetDateTime::now_utc().to_string();
    let items: Vec<(String, Vec<u8>)> = items
        .iter()
        .map(|(hash, vec)| {
            let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
            (hash.clone(), bytes)
        })
        .collect();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        {
            let tx = super::operations::begin_write_transaction(&conn)?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO embedding_cache (text_hash, vector, created_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(text_hash) DO UPDATE SET vector = excluded.vector",
                )?;
                for (hash, bytes) in &items {
                    stmt.execute(rusqlite::params![hash, bytes, &now])?;
                }
            }
            tx.commit()?;
        }
        Ok(())
    })
    .await
    .map_err(|e| Error::task_join("store embedding cache", e))?
}

/// Convert database row data to Entity.
/// The `data` column stores the inner struct (e.g. `Function`), not the `Entity` enum wrapper.
/// This is the single canonical place that maps type strings to entity variants.
pub(crate) fn data_to_entity(entity_type: &str, data: &str) -> Result<Entity> {
    macro_rules! map_entity {
        (
            $data:expr,
            $( $tag:literal => $variant:ident ),* $(,)?
        ) => {
            match entity_type {
                $( $tag => Entity::$variant(serde_json::from_str($data)?), )*
                _ => {
                    return Err(Error::EntityExtraction(format!(
                        "Unknown entity type: {entity_type}"
                    )));
                }
            }
        };
    }

    let entity = map_entity! {
        data,
        "function"      => Function,
        "class"         => Class,
        "table"         => Table,
        "index"         => Index,
        "struct"        => Struct,
        "enum"          => Enum,
        "interface"     => Interface,
        "trait"         => Trait,
        "module"        => Module,
        "namespace"     => Namespace,
        "constant"      => Constant,
        "type_alias"    => TypeAlias,
        "union"         => Union,
        "macro"         => Macro,
        "component"     => Component,
        "contract"      => Contract,
        "extension"     => Extension,
        "decorator"     => Decorator,
        "section"       => Section,
    };
    Ok(entity)
}

/// Initialize the sqlite-vec extension tables
///
/// This function assumes the extension has already been registered
/// via `sqlite3_auto_extension()` before the connection was opened.
///
/// # Errors
/// Returns an error if the virtual table creation fails
pub fn initialize(conn: &Connection) -> Result<()> {
    // Test that vec extension is loaded
    match conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0)) {
        Ok(version) => {
            tracing::info!("sqlite-vec version: {}", version);
        }
        Err(e) => {
            tracing::warn!("sqlite-vec extension not available: {}", e);
            tracing::warn!("Vector search functionality will be disabled");
            return Ok(()); // Continue without vector support
        }
    }

    // Create virtual table for entity embeddings.
    // `distance_metric=cosine` ensures vec0 returns cosine distance ∈ [0, 1]
    // for unit-normalised vectors (the embedder always L2-normalises).  Without this
    // flag vec0 defaults to L2, making `1 - distance` semantically wrong.
    if let Err(e) = conn.execute(
        &format!(
            r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS entity_vectors USING vec0(
            entity_id TEXT PRIMARY KEY,
            embedding FLOAT[{EMBEDDING_DIMENSION}] distance_metric=cosine
        )
        "#
        ),
        [],
    ) {
        tracing::warn!("Failed to create entity_vectors virtual table: {}", e);
    }

    // Context-note embeddings: a note's id as the rowid, and a hash of the text
    // the note was embedded from, by which a search finds the notes whose text
    // changed since (`context_graph::search`).
    if let Err(e) = conn.execute(
        &format!(
            r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS note_vectors USING vec0(
            embedding FLOAT[{EMBEDDING_DIMENSION}] distance_metric=cosine,
            +text_hash TEXT
        )
        "#
        ),
        [],
    ) {
        tracing::warn!("Failed to create note_vectors virtual table: {}", e);
    }

    Ok(())
}

/// Insert or update an entity embedding
///
/// sqlite-vec virtual tables don't support UPSERT, so this is a DELETE + INSERT,
/// through [`upsert_embeddings_batch`].
/// # Errors
/// Returns an error if the database operation fails
pub async fn upsert_embedding(
    pool: &ConnectionPool,
    entity_id: EntityId,
    embedding: &[f32],
) -> Result<()> {
    upsert_embeddings_batch(pool, &[(entity_id, embedding.to_vec())]).await?;
    Ok(())
}

/// Batch insert or update entity embeddings.
///
/// Wraps all `DELETE + INSERT` pairs in a **single** SQLite transaction so
/// the whole batch is atomic and fsync cost is amortised across many rows.
///
/// # Arguments
/// * `pool` — Database connection pool
/// * `items` — Slice of `(entity_id, embedding)` tuples
///
/// # Returns
/// Number of embeddings successfully stored.
///
/// # Errors
/// Returns an error if the transaction fails; all changes are rolled back.
///
/// One transaction per batch rather than per entity: for 8,745 entities at
/// batch size 64, ~137 transactions instead of ~8,745.
pub async fn upsert_embeddings_batch(
    pool: &ConnectionPool,
    items: &[(EntityId, Vec<f32>)],
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }

    let pool = pool.clone();
    let items: Vec<(String, Vec<u8>)> = items
        .iter()
        .map(|(id, emb)| {
            (
                id.to_string(),
                emb.iter().flat_map(|f| f.to_le_bytes()).collect(),
            )
        })
        .collect();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let tx = super::operations::begin_write_transaction(&conn)?;

        let mut count = 0usize;
        for (entity_id, embedding_bytes) in &items {
            tx.execute(
                "DELETE FROM entity_vectors WHERE entity_id = ?1",
                [entity_id as &dyn ToSql],
            )?;
            tx.execute(
                r#"INSERT INTO entity_vectors (entity_id, embedding) VALUES (?1, ?2)"#,
                [entity_id as &dyn ToSql, embedding_bytes as &dyn ToSql],
            )?;
            count += 1;
        }

        tx.commit()?;
        Ok(count)
    })
    .await
    .map_err(|e| Error::task_join("upsert embeddings batch", e))?
}

/// Search for similar entities using vector similarity.
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn search_similar(
    pool: &ConnectionPool,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<(Entity, f32)>> {
    search_similar_filtered(pool, query_embedding, limit, None).await
}

/// The most neighbours one sqlite-vec KNN query may ask for
/// (`SQLITE_VEC_VEC0_K_MAX` in sqlite-vec 0.1).
const KNN_K_MAX: i64 = 4096;

/// Search for similar entities, optionally only those of one entity type.
///
/// The `entity_vectors` vec0 table has no metadata column, so a type cannot
/// be filtered inside the KNN: the `e.type` condition applies to the `k`
/// neighbours the KNN returns. Asked for only `limit` neighbours, a type that
/// is rare near the query would come back short, often empty. A filtered search
/// therefore asks for the `KNN_K_MAX` nearest and keeps the first `limit`
/// of the type, and when even that falls short, ranks every vector of the
/// type exactly with `vec_distance_cosine` — the metric the table indexes
/// with, so both paths order and score alike.
///
/// # Errors
/// Returns an error if the database operation fails
pub async fn search_similar_filtered(
    pool: &ConnectionPool,
    query_embedding: &[f32],
    limit: usize,
    entity_type: Option<&str>,
) -> Result<Vec<(Entity, f32)>> {
    let pool = pool.clone();
    let query_bytes: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let limit = i64::try_from(limit).map_err(|_| Error::Generic("Limit too large".to_string()))?;
    let entity_type = entity_type.map(|s| s.to_string());

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;

        // Only the columns the row-mapper reads: the entity is rebuilt from the
        // `data` JSON, so `type`/`data`/`distance` suffice.
        let row_to_result = |row: &rusqlite::Row| {
            let entity_type_col: String = row.get("type")?;
            let entity_json: String = row.get("data")?;
            let entity = data_to_entity(&entity_type_col, &entity_json)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
            let distance: f32 = row.get("distance")?;
            Ok((entity, distance))
        };

        let Some(et) = entity_type else {
            // sqlite-vec requires a `k = ?` constraint in the WHERE clause for KNN.
            let mut stmt = conn.prepare_cached(
                r#"
                SELECT e.type, e.data, v.distance
                FROM entity_vectors v
                JOIN entities e ON v.entity_id = e.id
                WHERE v.embedding MATCH ?1 AND k = ?2
                ORDER BY v.distance
                "#,
            )?;
            return Ok(stmt
                .query_map(rusqlite::params![&query_bytes, limit], row_to_result)?
                .collect::<std::result::Result<Vec<_>, _>>()?);
        };

        let mut stmt = conn.prepare_cached(
            r#"
            SELECT e.type, e.data, v.distance
            FROM entity_vectors v
            JOIN entities e ON v.entity_id = e.id
            WHERE v.embedding MATCH ?1 AND k = ?2 AND e.type = ?3
            ORDER BY v.distance
            LIMIT ?4
            "#,
        )?;
        let nearest = stmt
            .query_map(
                rusqlite::params![&query_bytes, KNN_K_MAX, &et, limit],
                row_to_result,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if i64::try_from(nearest.len()).unwrap_or(i64::MAX) >= limit {
            return Ok(nearest);
        }

        let mut stmt = conn.prepare_cached(
            r#"
            SELECT e.type, e.data, vec_distance_cosine(v.embedding, ?1) AS distance
            FROM entities e
            JOIN entity_vectors v ON v.entity_id = e.id
            WHERE e.type = ?2
            ORDER BY distance
            LIMIT ?3
            "#,
        )?;
        Ok(stmt
            .query_map(rusqlite::params![&query_bytes, &et, limit], row_to_result)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })
    .await
    .map_err(|e| Error::task_join("search similar vectors", e))?
}

/// Return the set of entity IDs that already have a row in `entity_vectors`.
///
/// Used by the on-demand embedding path to decide which BM25 candidates need
/// to be embedded before they can contribute to the dense channel.
pub async fn entity_ids_with_vectors(
    pool: &ConnectionPool,
    ids: &[EntityId],
) -> Result<std::collections::HashSet<EntityId>> {
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    let pool = pool.clone();
    let id_strings: Vec<String> = ids.iter().map(|id| id.to_string()).collect();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        let mut result = std::collections::HashSet::with_capacity(id_strings.len());

        for chunk in id_strings.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT entity_id FROM entity_vectors WHERE entity_id IN ({})",
                placeholders
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();

            let found: Vec<String> = stmt
                .query_map(&params[..], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            for s in found {
                if let Ok(uuid) = uuid::Uuid::parse_str(&s) {
                    result.insert(uuid);
                }
            }
        }
        Ok(result)
    })
    .await
    .map_err(|e| Error::task_join("entity ids with vectors", e))?
}

/// Remove orphaned embedding vectors whose `entity_id` no longer exists in
/// the `entities` table.
///
/// This should be called periodically (e.g. after pipeline runs) to reclaim
/// space and prevent stale vectors from confusing similarity search results.
///
/// # Returns
/// Number of orphaned vectors removed.
pub async fn cleanup_orphan_vectors(pool: &ConnectionPool) -> Result<usize> {
    let pool = pool.clone();

    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        // Find orphan entity_ids
        let mut stmt = conn.prepare(
            "SELECT v.entity_id FROM entity_vectors v LEFT JOIN entities e ON v.entity_id = e.id WHERE e.id IS NULL"
        )?;
        let orphan_ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if orphan_ids.is_empty() {
            return Ok(0usize);
        }

        tracing::warn!(
            "Found {} orphaned embedding vectors, cleaning up...",
            orphan_ids.len()
        );

        let mut removed = 0usize;
        for id in &orphan_ids {
            removed += conn.execute(
                "DELETE FROM entity_vectors WHERE entity_id = ?1",
                [id as &dyn ToSql],
            )?;
        }

        tracing::info!("Removed {removed} orphaned embedding vectors");
        Ok(removed)
    })
    .await
    .map_err(|e| Error::task_join("cleanup orphan vectors", e))?
}
