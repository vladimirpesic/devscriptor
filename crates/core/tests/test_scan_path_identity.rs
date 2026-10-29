//! A file's identity in the index must not depend on how its directory was
//! spelled on the command line.
//!
//! `devscriptor scan .` passes a relative root; the MCP `scan_codebase` tool
//! passes an absolute one. Both reach the same pipeline, and every stored
//! `file_path` is the root plus a relative remainder, so the roots are
//! canonicalized: otherwise the same file would be stored twice under two
//! strings. The re-scan cleanup (`DELETE FROM entities WHERE file_path IN
//! (...)`) compares literal strings, so it would never match across
//! spellings: a single `scan_codebase` call on an already-scanned project
//! would double the index, searches would return every symbol twice, and
//! embeddings would be computed twice.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    embeddings::{EmbeddingService, MockEmbeddingProvider},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
};
use std::{path::Path, sync::Arc};
use tempfile::TempDir;

fn make_pipeline(db_path: &Path, incremental: bool) -> CodebasePipeline {
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
        include_extensions: vec!["py".to_string()],
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    let db = Arc::new(Database::new(config.database.clone()).expect("db"));
    let svc = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));
    CodebasePipeline::with_database_and_service(config, db, Some(svc)).expect("pipeline")
}

fn total_entities(db: &Path) -> usize {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))
        .unwrap_or(0)
}

/// Distinct `file_path` spellings in the index. More spellings than files is the
/// duplication itself.
fn distinct_paths(db: &Path) -> usize {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    conn.query_row("SELECT COUNT(DISTINCT file_path) FROM entities", [], |r| {
        r.get(0)
    })
    .unwrap_or(0)
}

fn fixture() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("service.py"),
        "MAX_SAFE_CELSIUS = 85.0\n\n\ndef is_unsafe(c):\n    return c > MAX_SAFE_CELSIUS\n",
    )
    .expect("write");
    std::fs::write(
        dir.path().join("helper.py"),
        "def clamp(v, lo, hi):\n    return max(lo, min(v, hi))\n",
    )
    .expect("write");
    dir
}

/// Scanning by `.` and then by the absolute path must replace the index, not
/// add a second copy of it.
#[tokio::test]
async fn rescanning_by_a_different_root_spelling_does_not_duplicate() {
    let dir = fixture();
    let db = dir.path().join("index.db");

    // As the CLI does: relative root, with the project as the working directory.
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");
    let relative_scan = make_pipeline(&db, false)
        .analyze_directory(Path::new("."))
        .await;
    std::env::set_current_dir(&previous).expect("restore cwd");
    relative_scan.expect("relative scan");

    let after_first = total_entities(&db);
    let paths_after_first = distinct_paths(&db);
    assert!(after_first > 0, "the first scan must index something");

    // As the MCP `scan_codebase` tool does: absolute root.
    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("absolute scan");

    assert_eq!(
        total_entities(&db),
        after_first,
        "re-scanning the same project by its absolute path must replace the \
         index, not double it"
    );
    assert_eq!(
        distinct_paths(&db),
        paths_after_first,
        "each file must appear under exactly one path spelling"
    );
}

/// The same, in the other order: absolute first, then relative.
#[tokio::test]
async fn the_order_of_the_two_spellings_does_not_matter() {
    let dir = fixture();
    let db = dir.path().join("index.db");

    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("absolute scan");
    let after_first = total_entities(&db);

    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");
    let relative_scan = make_pipeline(&db, false)
        .analyze_directory(Path::new("."))
        .await;
    std::env::set_current_dir(&previous).expect("restore cwd");
    relative_scan.expect("relative scan");

    assert_eq!(total_entities(&db), after_first, "still one copy");
}

/// Scanning twice by the *same* spelling is idempotent on its own — the entity
/// id is deterministic and the insert upserts on it. That is exactly why a
/// second spelling would break it: a different `file_path` yields a different
/// id, so the upsert would have nothing to collide with and would insert a
/// parallel copy instead.
#[tokio::test]
async fn rescanning_by_the_same_root_is_still_idempotent() {
    let dir = fixture();
    let db = dir.path().join("index.db");

    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("first scan");
    let after_first = total_entities(&db);

    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("second scan");

    assert_eq!(total_entities(&db), after_first);
}

/// The scan the CLI and the MCP tool actually perform is **non-incremental**
/// (`--incremental` is opt-in, and `scan_codebase` takes the defaults). Both
/// cleanup paths are gated on that flag, so a re-scan in this mode appends
/// instead of replacing — the same file is indexed again on every run.
#[tokio::test]
async fn a_non_incremental_rescan_does_not_append_a_second_copy() {
    let dir = fixture();
    let db = dir.path().join("index.db");

    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("first scan");
    let after_first = total_entities(&db);
    assert!(after_first > 0);

    make_pipeline(&db, false)
        .analyze_directory(dir.path())
        .await
        .expect("second scan");

    assert_eq!(
        total_entities(&db),
        after_first,
        "a non-incremental re-scan must replace the index, not append to it"
    );
}
