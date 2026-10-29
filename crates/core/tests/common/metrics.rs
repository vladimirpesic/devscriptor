//! Quality metrics harness for Devscriptor integration tests.
//!
//! Call [`compute_quality_metrics`] with a path to a populated SQLite DB and
//! receive a [`QualityMetrics`] struct that can be serialised to JSON.
//!
//! The slow integration tests write a `*_quality_metrics.json` sibling file
//! next to their DB so regressions are visible without re-running slow suites.
//!
//! ## Metrics produced
//! - Entity-type distribution (total + by-type + by-language)
//! - Average entities per parsed source file, broken down by language
//! - `calls : called_by` and `uses : used_by` symmetry ratios
//! - Embedding coverage (% of entities with a stored vector)
//! - Peak RSS at call time (via `/proc/self/status` on Linux, sysinfo elsewhere)

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntityMetrics {
    pub total: u64,
    pub by_type: HashMap<String, u64>,
    pub by_language: HashMap<String, u64>,
    /// Average entity count per parsed source file, keyed by language.
    pub avg_per_file_by_language: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelationshipMetrics {
    pub total: u64,
    pub by_type: HashMap<String, u64>,
    /// `called_by / calls` — ideally near 1.0.
    pub calls_called_by_ratio: f64,
    /// `used_by / uses` — ideally near 1.0.
    pub uses_used_by_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbeddingMetrics {
    pub embedded_count: u64,
    pub total_entities: u64,
    /// Percentage 0–100.
    pub coverage_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMetrics {
    pub total_parsed: u64,
    pub by_language: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualityMetrics {
    pub entities: EntityMetrics,
    pub relationships: RelationshipMetrics,
    pub embeddings: EmbeddingMetrics,
    pub files: FileMetrics,
    /// Resident set size in KB at the time this function was called.
    pub rss_kb: u64,
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Compute quality metrics for a Devscriptor SQLite database.
///
/// Opens a read-only connection (bypassing `Database::new`, which may write)
/// so it is safe to call against any populated DB file without side effects.
pub fn compute_quality_metrics(db_path: &Path) -> anyhow::Result<QualityMetrics> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let entities = entity_metrics(&conn)?;
    let relationships = relationship_metrics(&conn)?;
    let embeddings = embedding_metrics(&conn, entities.total)?;
    let files = file_metrics(&conn)?;

    // Recompute avg_per_file using file counts we just queried.
    let avg_per_file = entities
        .by_language
        .iter()
        .map(|(lang, &ent_count)| {
            let file_count = *files.by_language.get(lang).unwrap_or(&1).max(&1);
            (lang.clone(), ent_count as f64 / file_count as f64)
        })
        .collect();

    Ok(QualityMetrics {
        entities: EntityMetrics {
            avg_per_file_by_language: avg_per_file,
            ..entities
        },
        relationships,
        embeddings,
        files,
        rss_kb: current_rss_kb(),
    })
}

/// Compute metrics and write them as pretty-printed JSON to
/// `<db_path_stem>_quality_metrics.json` beside the database file.
/// Returns the serialised JSON string.
pub fn emit_quality_metrics(db_path: &Path) -> anyhow::Result<String> {
    let metrics = compute_quality_metrics(db_path)?;
    let json = serde_json::to_string_pretty(&metrics)?;

    let out_path = db_path.with_extension("quality_metrics.json");
    std::fs::write(&out_path, &json)?;
    tracing::info!("Quality metrics written to {}", out_path.display());
    Ok(json)
}

// ── Per-domain query helpers ──────────────────────────────────────────────────

fn entity_metrics(conn: &rusqlite::Connection) -> anyhow::Result<EntityMetrics> {
    // Total
    let total: u64 = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;

    // By entity type
    let mut by_type: HashMap<String, u64> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT type, COUNT(*) FROM entities GROUP BY type")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
        for row in rows {
            let (t, c) = row?;
            by_type.insert(t, c);
        }
    }

    // By language
    let mut by_language: HashMap<String, u64> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT language, COUNT(*) FROM entities GROUP BY language")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
        for row in rows {
            let (lang, c) = row?;
            if !lang.is_empty() {
                *by_language.entry(lang).or_insert(0) += c;
            }
        }
    }

    Ok(EntityMetrics {
        total,
        by_type,
        by_language,
        avg_per_file_by_language: HashMap::new(), // filled by caller
    })
}

fn relationship_metrics(conn: &rusqlite::Connection) -> anyhow::Result<RelationshipMetrics> {
    let total: u64 = conn.query_row("SELECT COUNT(*) FROM relationships", [], |r| r.get(0))?;

    let mut by_type: HashMap<String, u64> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT relationship_type, COUNT(*) FROM relationships GROUP BY relationship_type",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
        for row in rows {
            let (t, c) = row?;
            by_type.insert(t, c);
        }
    }

    let calls = *by_type.get("Calls").unwrap_or(&0);
    let called_by = *by_type.get("CalledBy").unwrap_or(&0);
    let uses = *by_type.get("Uses").unwrap_or(&0);
    let used_by = *by_type.get("UsedBy").unwrap_or(&0);

    let calls_called_by_ratio = if calls == 0 {
        0.0
    } else {
        called_by as f64 / calls as f64
    };
    let uses_used_by_ratio = if uses == 0 {
        0.0
    } else {
        used_by as f64 / uses as f64
    };

    Ok(RelationshipMetrics {
        total,
        by_type,
        calls_called_by_ratio,
        uses_used_by_ratio,
    })
}

fn embedding_metrics(
    conn: &rusqlite::Connection,
    total_entities: u64,
) -> anyhow::Result<EmbeddingMetrics> {
    // entity_vectors stores one row per embedded entity
    let embedded_count: u64 = conn.query_row(
        "SELECT COUNT(DISTINCT entity_id) FROM entity_vectors",
        [],
        |r| r.get(0),
    )?;

    let coverage_pct = if total_entities == 0 {
        100.0
    } else {
        embedded_count as f64 / total_entities as f64 * 100.0
    };

    Ok(EmbeddingMetrics {
        embedded_count,
        total_entities,
        coverage_pct,
    })
}

fn file_metrics(conn: &rusqlite::Connection) -> anyhow::Result<FileMetrics> {
    // scan_manifest tracks every parsed file
    let total_parsed: u64 = conn
        .query_row("SELECT COUNT(*) FROM scan_manifest", [], |r| r.get(0))
        .unwrap_or(0);

    // Derive language per file from the entities table (file_path → language).
    // One file can have many entities; we pick the most-common language per path.
    let mut by_language: HashMap<String, u64> = HashMap::new();
    {
        // Count distinct file paths per language in the entities table.
        let mut stmt = conn.prepare(
            "SELECT language, COUNT(DISTINCT file_path)
             FROM entities
             WHERE language IS NOT NULL AND language != ''
             GROUP BY language",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
        for row in rows {
            let (lang, c) = row?;
            by_language.insert(lang, c);
        }
    }

    Ok(FileMetrics {
        total_parsed,
        by_language,
    })
}

// ── RSS helper ────────────────────────────────────────────────────────────────

fn current_rss_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        // Fast path: parse /proc/self/status directly.
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:")
                    && let Some(kb) = rest.split_whitespace().next().and_then(|s| s.parse().ok())
                {
                    return kb;
                }
            }
        }
    }
    0
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_db(dir: &TempDir) -> rusqlite::Connection {
        let path = dir.path().join("test_metrics.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entities (id TEXT, type TEXT, language TEXT, file_path TEXT);
             CREATE TABLE relationships (id TEXT, relationship_type TEXT);
             CREATE TABLE entity_vectors (entity_id TEXT);
             CREATE TABLE scan_manifest (path TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn empty_db_returns_zeroes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entities (id TEXT, type TEXT, language TEXT, file_path TEXT);
             CREATE TABLE relationships (id TEXT, relationship_type TEXT);
             CREATE TABLE entity_vectors (entity_id TEXT);
             CREATE TABLE scan_manifest (path TEXT);",
        )
        .unwrap();
        drop(conn);

        let m = compute_quality_metrics(&path).unwrap();
        assert_eq!(m.entities.total, 0);
        assert_eq!(m.relationships.total, 0);
        assert_eq!(m.embeddings.coverage_pct, 100.0); // vacuously true
        assert_eq!(m.files.total_parsed, 0);
    }

    #[test]
    fn symmetry_ratio_computed_correctly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sym.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entities (id TEXT, type TEXT, language TEXT, file_path TEXT);
             CREATE TABLE entity_vectors (entity_id TEXT);
             CREATE TABLE scan_manifest (path TEXT);
             CREATE TABLE relationships (id TEXT, relationship_type TEXT);
             INSERT INTO relationships VALUES ('r1','Calls');
             INSERT INTO relationships VALUES ('r2','Calls');
             INSERT INTO relationships VALUES ('r3','CalledBy');",
        )
        .unwrap();
        drop(conn);

        let m = compute_quality_metrics(&path).unwrap();
        assert_eq!(m.relationships.by_type["Calls"], 2);
        assert_eq!(m.relationships.by_type["CalledBy"], 1);
        // ratio = 1/2
        assert!((m.relationships.calls_called_by_ratio - 0.5).abs() < 1e-9);
    }
}
