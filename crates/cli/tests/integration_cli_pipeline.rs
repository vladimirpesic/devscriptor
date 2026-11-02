//! End-to-end integration test for the CLI → Core → Database pipeline.

use std::sync::Arc;

use devscriptor_cli::commands::{
    self,
    common::{OutputFormat, SearchType},
};
use devscriptor_core::config::ProjectConfig;
use devscriptor_core::database::{Database, DatabaseConfig};

/// Test the complete pipeline: scan a small project, then query the database.
#[tokio::test]
async fn test_cli_scan_and_query_pipeline() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir(&src_dir).unwrap();

    // Create a small Python project
    std::fs::write(
        src_dir.join("main.py"),
        r#"
def greet(name: str) -> str:
    return f"Hello, {name}!"

class Greeter:
    def __init__(self, greeting: str):
        self.greeting = greeting

    def say(self, name: str) -> str:
        return f"{self.greeting}, {name}!"
"#,
    )
    .unwrap();

    // 1. Scan the project (CLI scan command path)
    let scan_args = commands::scan::ScanArgs {
        path: src_dir.clone(),
        database: db_path.clone(),
        project: None,
        include: vec!["py".to_string()],
        exclude: vec![],
        max_file_size: None,
        batch_size: Some(10),
        incremental: false,
        embed: false,
        skip_embeddings: true,
        bulk_load: false,
        unload_model_after_scan: false,
        metrics: false,
    };
    commands::scan::execute(scan_args).await.unwrap();

    // 2. Open the database (CLI shared-db path)
    let database = commands::common::open_database(&db_path, &ProjectConfig::default()).unwrap();

    // 3. Query database stats (CLI database::stats command path)
    let (_files, entities, _relationships) = database.get_stats().await.unwrap();
    assert!(
        entities >= 3,
        "Expected at least 3 entities (2 functions + 1 class), got {entities}"
    );
    assert!(
        _files >= 1,
        "Expected at least 1 file tracked, got {_files}"
    );

    // 4. Search entities (CLI search command path)
    commands::search::execute(
        "greet".to_string(),
        Arc::clone(&database),
        SearchType::Symbol,
        10,
        OutputFormat::Text,
    )
    .await
    .unwrap();

    // 5. Search functions specifically
    commands::search::search_function("greet".to_string(), Arc::clone(&database), true, 10)
        .await
        .unwrap();

    // 6. Search classes
    commands::search::search_class("Greeter".to_string(), Arc::clone(&database), true, 10)
        .await
        .unwrap();
}

/// Test that analysis commands work on a scanned database.
#[tokio::test]
async fn test_cli_analysis_pipeline() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir(&src_dir).unwrap();

    std::fs::write(
        src_dir.join("app.py"),
        r#"
def used() -> int:
    return 42

def unused() -> int:
    return 0

class Data:
    pass
"#,
    )
    .unwrap();

    // Scan
    let scan_args = commands::scan::ScanArgs {
        path: src_dir.clone(),
        database: db_path.clone(),
        project: None,
        include: vec!["py".to_string()],
        exclude: vec![],
        max_file_size: None,
        batch_size: Some(10),
        incremental: false,
        embed: false,
        skip_embeddings: true,
        bulk_load: false,
        unload_model_after_scan: false,
        metrics: false,
    };
    commands::scan::execute(scan_args).await.unwrap();

    let database = commands::common::open_database(&db_path, &ProjectConfig::default()).unwrap();

    // Run complexity analysis
    commands::analyze::execute(
        src_dir,
        database,
        commands::common::AnalysisKind::Complexity,
        commands::common::OutputFormat::Json,
        None,
    )
    .await
    .unwrap();
}

/// Test database status command on a freshly created DB.
#[tokio::test]
async fn test_cli_database_status() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create an empty database
    let db = Database::new(DatabaseConfig {
        path: db_path.to_string_lossy().to_string(),
        ..Default::default()
    })
    .unwrap();
    drop(db);

    let database = commands::common::open_database(&db_path, &ProjectConfig::default()).unwrap();
    commands::database::status(database, None).await.unwrap();
}
