//! An index records the schema version it was written with. One written with
//! another version, or with none recorded, is emptied when it is opened, its
//! Context Graph notes kept, and a scan builds it again.

use std::path::{Path, PathBuf};

use devscriptor_core::database::schema::{SCHEMA_VERSION, SCHEMA_VERSION_KEY};
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

fn config(db: &Path) -> DatabaseConfig {
    DatabaseConfig {
        path: db.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    }
}

/// The `metadata` rows of `db`, key, value and time, read without opening it
/// through `Database`.
fn metadata(db: &Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(db).expect("open");
    let mut stmt = conn
        .prepare("SELECT key, value, updated_at FROM metadata ORDER BY key")
        .expect("select metadata");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query metadata")
        .collect::<rusqlite::Result<_>>()
        .expect("rows")
}

fn version(db: &Path) -> Option<String> {
    metadata(db)
        .into_iter()
        .find(|(key, ..)| key == SCHEMA_VERSION_KEY)
        .map(|(_, value, _)| value)
}

fn count(db: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(db)
        .expect("open")
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
}

async fn scan(db: &Path, root: &Path) {
    CodebasePipeline::new(PipelineConfig {
        database: config(db),
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(root)
    .await
    .expect("scan");
}

/// A scanned project with one Context Graph note: the directory, the
/// project's root and the database.
async fn scanned_project() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).expect("mkdir");
    let root = dunce::canonicalize(root).expect("canonical root");
    std::fs::write(
        root.join("app.py"),
        "def handler():\n    return helper()\n\ndef helper():\n    return 1\n",
    )
    .expect("write");
    let db = dir.path().join("index.db");
    scan(&db, &root).await;
    rusqlite::Connection::open(&db)
        .expect("open")
        .execute(
            "INSERT INTO context_notes (note_type, title, content) \
             VALUES ('adr', 'Keep handlers thin', 'They call helpers.')",
            [],
        )
        .expect("add a note");
    (dir, root, db)
}

#[tokio::test]
async fn a_new_database_records_the_schema_version() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("index.db");
    drop(Database::new(config(&db)).expect("create"));
    assert_eq!(version(&db), Some(SCHEMA_VERSION.to_string()));
}

#[tokio::test]
async fn opening_a_current_database_writes_nothing() {
    let (_dir, _root, db) = scanned_project().await;
    let before = metadata(&db);
    drop(Database::new(config(&db)).expect("open again"));
    assert_eq!(metadata(&db), before);
    assert!(count(&db, "entities") > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_of_another_version_is_emptied_and_its_notes_kept() {
    let (_dir, root, db) = scanned_project().await;
    assert!(count(&db, "entities") > 0);
    assert!(count(&db, "relationships") > 0);
    rusqlite::Connection::open(&db)
        .expect("open")
        .execute(
            "UPDATE metadata SET value = '0' WHERE key = ?1",
            [SCHEMA_VERSION_KEY],
        )
        .expect("record another version");

    drop(Database::new(config(&db)).expect("open"));
    for table in [
        "files",
        "entities",
        "relationships",
        "symbol_refs",
        "scan_manifest",
    ] {
        assert_eq!(count(&db, table), 0, "{table}");
    }
    assert_eq!(count(&db, "context_notes"), 1);
    assert_eq!(version(&db), Some(SCHEMA_VERSION.to_string()));

    scan(&db, &root).await;
    assert!(count(&db, "entities") > 0);
    assert!(count(&db, "relationships") > 0);
}

#[tokio::test]
async fn an_index_without_a_recorded_version_is_emptied() {
    let (_dir, _root, db) = scanned_project().await;
    rusqlite::Connection::open(&db)
        .expect("open")
        .execute("DELETE FROM metadata WHERE key = ?1", [SCHEMA_VERSION_KEY])
        .expect("remove the version");

    drop(Database::new(config(&db)).expect("open"));
    assert_eq!(count(&db, "entities"), 0);
    assert_eq!(count(&db, "context_notes"), 1);
    assert_eq!(version(&db), Some(SCHEMA_VERSION.to_string()));
}
