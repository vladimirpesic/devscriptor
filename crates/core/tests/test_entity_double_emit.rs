//! One piece of source must produce one entity.
//!
//! Several grammars nest a node inside another that covers the same code. Were
//! the extractor to take both kinds, each such pair would emit two entities for
//! one declaration — with different column spans, so nothing downstream would
//! collapse them: searches would return the symbol twice and `get_statistics`
//! would over-count.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    embeddings::{EmbeddingService, MockEmbeddingProvider},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
};
use std::{path::Path, sync::Arc};
use tempfile::TempDir;

fn scan(dir: &Path, extensions: &[&str]) -> std::path::PathBuf {
    let db_path = dir.join("index.db");
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            cache_size: 512,
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        incremental: false,
        enable_cross_file: false,
        use_gitignore: false,
        include_extensions: extensions.iter().map(|e| (*e).to_string()).collect(),
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    let db = Arc::new(Database::new(config.database.clone()).expect("db"));
    let svc = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));
    let pipeline =
        CodebasePipeline::with_database_and_service(config, db, Some(svc)).expect("pipeline");
    tokio::runtime::Handle::current()
        .block_on(async { pipeline.analyze_directory(dir).await })
        .expect("scan");
    db_path
}

/// Every `(name, type, line)` in one file, with how many rows carry it.
fn entity_counts(db: &Path) -> Vec<(String, String, i64, i64)> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    let mut stmt = conn
        .prepare(
            "SELECT name, type, start_line, COUNT(*) FROM entities \
             GROUP BY file_path, name, type, start_line ORDER BY start_line, name",
        )
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

/// C-family grammars nest an `init_declarator` inside its `declaration`; the
/// constant is emitted once, not for each.
#[tokio::test(flavor = "multi_thread")]
async fn a_c_constant_is_emitted_once() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("edge.cpp"),
        "constexpr double MAX_SAFE_CELSIUS = 85.0;\nconst char* NAME = \"x\";\n",
    )
    .expect("write");

    let db = tokio::task::spawn_blocking({
        let path = dir.path().to_path_buf();
        move || scan(&path, &["cpp"])
    })
    .await
    .expect("join");

    for (name, _type, line, count) in entity_counts(&db) {
        assert_eq!(count, 1, "{name} at line {line} was emitted {count} times");
    }
}

/// A declaration introducing several constants must keep all of them — the
/// reason the extractor cannot simply always prefer the outer node, which is
/// named after only the first declarator.
#[tokio::test(flavor = "multi_thread")]
async fn every_constant_in_a_multi_declarator_survives() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("multi.cpp"),
        "constexpr int ALPHA = 1, BETA = 2;\n",
    )
    .expect("write");

    let db = tokio::task::spawn_blocking({
        let path = dir.path().to_path_buf();
        move || scan(&path, &["cpp"])
    })
    .await
    .expect("join");

    let names: Vec<String> = entity_counts(&db).into_iter().map(|(n, ..)| n).collect();
    assert!(names.contains(&"ALPHA".to_string()), "{names:?}");
    assert!(
        names.contains(&"BETA".to_string()),
        "the second declarator must not be lost: {names:?}"
    );
    for (name, _t, _l, count) in entity_counts(&db) {
        assert_eq!(count, 1, "{name} duplicated");
    }
}

/// HTML has no `name` field; an element is still named for itself, not by the
/// component extractor's file-stem fallback after the page.
#[tokio::test(flavor = "multi_thread")]
async fn html_elements_are_named_for_themselves_not_the_file() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("index.html"),
        "<!doctype html>\n<html lang=\"en\">\n  <head><meta charset=\"utf-8\" /></head>\n  \
         <body><main id=\"app\"></main></body>\n</html>\n",
    )
    .expect("write");

    let db = tokio::task::spawn_blocking({
        let path = dir.path().to_path_buf();
        move || scan(&path, &["html"])
    })
    .await
    .expect("join");

    let rows = entity_counts(&db);
    let names: Vec<&str> = rows.iter().map(|(n, ..)| n.as_str()).collect();

    assert!(
        !names.contains(&"index"),
        "elements must not all be named after the file: {names:?}"
    );
    assert!(names.contains(&"html"), "{names:?}");
    assert!(
        names.contains(&"main#app"),
        "an element with an id should be identified by it: {names:?}"
    );

    // `<meta />` parses as an `element` wrapping a `self_closing_tag` of the
    // *same* extent, and both are component nodes.
    for (name, _t, line, count) in &rows {
        assert_eq!(*count, 1, "{name} at line {line} was emitted {count} times");
    }
}

/// SQL indexes are named schema objects worth finding, and they must not be
/// typed `table` — "which tables exist?" answered with index names is worse
/// than not indexing them at all.
#[tokio::test(flavor = "multi_thread")]
async fn sql_indexes_are_extracted_and_typed_apart_from_tables() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("schema.sql"),
        "CREATE TABLE products (id INT PRIMARY KEY, price DECIMAL(10,2));\n\
         CREATE INDEX idx_products_price ON products(price);\n",
    )
    .expect("write");

    let db = tokio::task::spawn_blocking({
        let path = dir.path().to_path_buf();
        move || scan(&path, &["sql"])
    })
    .await
    .expect("join");

    let rows = entity_counts(&db);
    let names: Vec<&str> = rows.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(names.contains(&"products"), "{names:?}");
    assert!(
        names.contains(&"idx_products_price"),
        "the index was not extracted: {names:?}"
    );

    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open db");
    let index_type: String = conn
        .query_row(
            "SELECT type FROM entities WHERE name = 'idx_products_price'",
            [],
            |r| r.get(0),
        )
        .expect("index row");
    assert_eq!(index_type, "index", "an index must not be typed as a table");
}
