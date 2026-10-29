//! The relationship counts a scan reports are the rows it stored.
//!
//! Added up as extracted, before the database skips duplicates, the count would
//! exceed the rows the index holds; and of the edges of the cross-file phase,
//! only those that cross files count as "cross-file".

use std::path::Path;

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig, PipelineResult};
use tempfile::TempDir;

/// Calls across files, a function calling the same function twice (one edge),
/// and calls within a file that both the per-file and the cross-file pass find.
const FIXTURE: &[(&str, &str)] = &[
    (
        "app/geometry.py",
        "def scale(value):\n    return value * 2\n\n\ndef twice(value):\n    return scale(scale(value))\n",
    ),
    (
        "app/report.py",
        "from geometry import scale\n\n\ndef summary():\n    return scale(1) + scale(2) + total()\n\n\n\
         def total():\n    return 7\n",
    ),
    (
        "app/caller.py",
        "from geometry import scale\n\n\ndef second_caller():\n    return scale(5)\n",
    ),
];

fn pipeline(db: &Path, incremental: bool) -> CodebasePipeline {
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            cache_size: 512,
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        incremental,
        enable_cross_file: true,
        use_gitignore: false,
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config).expect("pipeline")
}

async fn scan(db: &Path, root: &Path, incremental: bool) -> PipelineResult {
    pipeline(db, incremental)
        .analyze_directory(root)
        .await
        .expect("scan")
}

/// `(rows, rows whose endpoints lie in different files)` in the index.
fn stored(db: &Path) -> (usize, usize) {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the index");
    let total = conn
        .query_row("SELECT COUNT(*) FROM relationships", [], |row| {
            row.get::<_, usize>(0)
        })
        .expect("count relationships");
    let cross = conn
        .query_row(
            "SELECT COUNT(*) FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE s.file_path != t.file_path",
            [],
            |row| row.get::<_, usize>(0),
        )
        .expect("count cross-file relationships");
    (total, cross)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_reports_the_relationships_it_stored() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    for (path, content) in FIXTURE {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        std::fs::write(path, content).expect("write fixture");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");

    let fresh = scan(&db, root, false).await;
    let (total, cross) = stored(&db);
    assert_eq!(
        fresh.relationship_count, total,
        "rows stored by a fresh scan"
    );
    assert_eq!(fresh.cross_file_relationship_count, cross);
    assert!(
        0 < cross && cross < total,
        "the fixture has both kinds: {cross} of {total}"
    );

    // An incremental re-scan of the unchanged tree parses nothing and stores
    // nothing.
    let unchanged = scan(&db, root, true).await;
    assert_eq!(unchanged.relationship_count, 0);
    assert_eq!(unchanged.cross_file_relationship_count, 0);
    assert_eq!(stored(&db), (total, cross));

    // A full re-scan replaces every file's own edges, so it stores them all
    // again, and the index ends up exactly as before.
    let full = scan(&db, root, false).await;
    assert_eq!(full.relationship_count, total);
    assert_eq!(full.cross_file_relationship_count, cross);
    assert_eq!(stored(&db), (total, cross));
}
