//! Incremental scan integration tests.
//!
//! Covers four scenarios with a temp-dir fixture repo:
//!   1. Full scan → no-op re-scan: all files skipped, 0 re-parsed.
//!   2. Modify one file → only that file re-parsed, others skipped.
//!   3. Same content, mtime bumped (hash-same fast path) → file skipped.
//!   4. Delete a file → re-scan → its entities (and cascaded relationships) gone.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    embeddings::{EmbeddingService, MockEmbeddingProvider},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

use filetime::FileTime;

// ── Test fixture builder ──────────────────────────────────────────────────────

struct Fixture {
    _dir: TempDir,
    /// The folder's real path, which the index stores its files under.
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dunce::canonicalize(dir.path()).expect("the tempdir exists");
        Self { _dir: dir, root }
    }

    fn path(&self) -> &std::path::Path {
        &self.root
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, content).expect("write fixture");
        p
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.root.join(name)).expect("remove fixture");
    }
}

// ── Pipeline helpers ──────────────────────────────────────────────────────────

fn make_pipeline(
    _fixture: &Fixture,
    db_path: &std::path::Path,
    incremental: bool,
) -> CodebasePipeline {
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            cache_size: 512,
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            emit_decorators: false,
            ..ParserConfig::default()
        },
        incremental,
        enable_cross_file: false,
        file_batch_size: 50,
        embedding_batch_size: 16,
        use_gitignore: false,
        // Only scan .py files to avoid picking up Rust test binaries or anything else.
        include_extensions: vec!["py".to_string()],
        max_concurrency: 1,
        ..PipelineConfig::default()
    };

    let db = Arc::new(Database::new(config.database.clone()).expect("db"));
    let svc = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));

    CodebasePipeline::with_database_and_service(config, db, Some(svc)).expect("pipeline")
}

fn db_path(fixture: &Fixture) -> PathBuf {
    fixture.path().join("test.db")
}

async fn entity_count_for_file(db: &std::path::Path, file_path: &std::path::Path) -> usize {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    let path_str = file_path.to_string_lossy().to_string();
    conn.query_row(
        "SELECT COUNT(*) FROM entities WHERE file_path = ?1",
        rusqlite::params![path_str],
        |r| r.get::<_, usize>(0),
    )
    .unwrap_or(0)
}

async fn total_entity_count(db: &std::path::Path) -> usize {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    conn.query_row("SELECT COUNT(*) FROM entities", [], |r| {
        r.get::<_, usize>(0)
    })
    .unwrap_or(0)
}

// ── Scenario 1: no-op re-scan ─────────────────────────────────────────────────

/// Full scan followed by an incremental re-scan with no changes.
/// All files must be skipped; `files_parsed` must be 0.
#[tokio::test]
async fn test_incremental_noop_rescan() {
    let fix = Fixture::new();
    let db = db_path(&fix);

    fix.write(
        "module_a.py",
        "def greet(name):\n    return f'hello {name}'\n\ndef farewell(name):\n    return 'bye'\n",
    );
    fix.write(
        "module_b.py",
        "class Calculator:\n    def add(self, a, b):\n        return a + b\n",
    );

    // ── Full scan (incremental=false, writes the scan_manifest baseline) ──────
    let r1 = make_pipeline(&fix, &db, false)
        .analyze_directory(fix.path())
        .await
        .expect("full scan");

    assert_eq!(r1.files_discovered, 2, "must discover both py files");
    assert!(r1.files_parsed >= 1, "must parse at least one file");

    // ── Incremental re-scan: nothing changed ──────────────────────────────────
    let r2 = make_pipeline(&fix, &db, true)
        .analyze_directory(fix.path())
        .await
        .expect("incremental noop");

    assert_eq!(r2.files_parsed, 0, "unchanged files must not be re-parsed");
    assert_eq!(
        r2.files_skipped, r2.files_discovered,
        "every discovered file must be skipped"
    );
}

// ── Scenario 2: modify one file → only it re-parsed ──────────────────────────

/// After a full scan, modify exactly one file.  The incremental re-scan must
/// parse that file and skip all others.
#[tokio::test]
async fn test_incremental_one_file_modified() {
    let fix = Fixture::new();
    let db = db_path(&fix);

    let path_a = fix.write("alpha.py", "def alpha():\n    pass\n");
    fix.write("beta.py", "def beta():\n    pass\n");

    // Full scan
    let r1 = make_pipeline(&fix, &db, false)
        .analyze_directory(fix.path())
        .await
        .expect("full scan");
    assert_eq!(r1.files_discovered, 2);

    let entities_before = entity_count_for_file(&db, &path_a).await;
    assert!(entities_before >= 1, "alpha.py must have at least 1 entity");

    // Modify alpha.py (more functions → entity count must grow)
    fix.write(
        "alpha.py",
        "def alpha():\n    pass\n\ndef alpha_two():\n    pass\n\ndef alpha_three():\n    pass\n",
    );

    // Incremental re-scan
    let r2 = make_pipeline(&fix, &db, true)
        .analyze_directory(fix.path())
        .await
        .expect("incremental after modify");

    assert_eq!(
        r2.files_parsed, 1,
        "only the modified file must be re-parsed; got {}",
        r2.files_parsed
    );
    assert_eq!(
        r2.files_skipped, 1,
        "beta.py must be skipped; got skipped={}",
        r2.files_skipped
    );

    // Entity count for alpha.py must have increased
    let entities_after = entity_count_for_file(&db, &path_a).await;
    assert!(
        entities_after > entities_before,
        "alpha.py entity count must increase after adding functions: before={entities_before} after={entities_after}"
    );
}

// ── Scenario 3: same-content mtime bump (hash-same fast path) ────────────────

/// Overwrite a file with identical content (OS bumps mtime).
/// The incremental scanner detects mtime changed → hashes the file →
/// hash matches manifest → skips.  `files_parsed` must be 0.
#[tokio::test]
async fn test_incremental_mtime_bump_same_content() {
    let fix = Fixture::new();
    let db = db_path(&fix);

    let content = "def stable():\n    return 42\n";
    let path = fix.write("stable.py", content);

    // Full scan
    make_pipeline(&fix, &db, false)
        .analyze_directory(fix.path())
        .await
        .expect("full scan");

    // Rewrite the file with identical content.  On Linux this bumps ctime;
    // whether mtime changes within the same second depends on the OS/FS.
    // We set mtime explicitly so the test is deterministic: advance mtime
    // by 2 seconds to guarantee it differs.
    std::fs::write(&path, content).expect("overwrite");
    let future_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 2;
    let ft = FileTime::from_unix_time(i64::try_from(future_secs).unwrap(), 0);
    filetime::set_file_mtime(&path, ft).expect("set mtime");

    // Incremental re-scan: mtime changed but hash same → must skip
    let r2 = make_pipeline(&fix, &db, true)
        .analyze_directory(fix.path())
        .await
        .expect("incremental mtime-bump");

    assert_eq!(
        r2.files_parsed, 0,
        "hash-same file must be skipped even with mtime bump; got parsed={}",
        r2.files_parsed
    );
}

// ── Scenario 4: delete a file → entities removed via cascade ─────────────────

/// Delete one file.  After an incremental re-scan its entities must be gone
/// from the `entities` table (FK cascade through `scan_manifest` deletion
/// triggers the `DELETE FROM entities WHERE file_path = ?` in the pipeline).
#[tokio::test]
async fn test_incremental_deleted_file_cascade() {
    let fix = Fixture::new();
    let db = db_path(&fix);

    fix.write("keep.py", "def keeper():\n    pass\n");
    let path_del = fix.write(
        "delete_me.py",
        "def gone_one():\n    pass\n\ndef gone_two():\n    pass\n",
    );

    // Full scan
    let r1 = make_pipeline(&fix, &db, false)
        .analyze_directory(fix.path())
        .await
        .expect("full scan");
    assert_eq!(r1.files_discovered, 2);

    let gone_before = entity_count_for_file(&db, &path_del).await;
    let total_before = total_entity_count(&db).await;
    assert!(
        gone_before >= 2,
        "delete_me.py must have at least 2 entities"
    );

    // Delete one file
    fix.remove("delete_me.py");

    // Incremental re-scan
    let r2 = make_pipeline(&fix, &db, true)
        .analyze_directory(fix.path())
        .await
        .expect("incremental after delete");

    assert_eq!(r2.files_discovered, 1, "only keep.py remains");

    let gone_after = entity_count_for_file(&db, &path_del).await;
    assert_eq!(
        gone_after, 0,
        "deleted file's entities must be removed; still found {gone_after}"
    );

    let total_after = total_entity_count(&db).await;
    assert!(
        total_after < total_before,
        "total entity count must drop after file deletion: before={total_before} after={total_after}"
    );

    // keep.py entities are still present
    let keep_path = fix.path().join("keep.py");
    let keep_count = entity_count_for_file(&db, &keep_path).await;
    assert!(keep_count >= 1, "keep.py entities must survive");

    // The deleted file must not appear in relationships
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    // Check via entities: no entities → no relationships (FK cascade)
    let orphan: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM relationships r
             WHERE NOT EXISTS (SELECT 1 FROM entities e WHERE e.id = r.source_id)
                OR NOT EXISTS (SELECT 1 FROM entities e WHERE e.id = r.target_id)",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert_eq!(orphan, 0, "no orphaned relationship edges after deletion");
}
