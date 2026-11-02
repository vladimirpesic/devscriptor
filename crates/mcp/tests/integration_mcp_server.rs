//! Integration tests for MCP server lifecycle and state management.
//!
//! These tests exercise the [`DevscriptorServerState`](devscriptor_mcp::DevscriptorServerState)
//! public API using temporary databases to ensure isolation.

use std::sync::Arc;

use devscriptor_mcp::{DevscriptorServerState, JobStatus};
use tempfile::TempDir;
use tracing::{debug, info};

/// Create a [`DevscriptorServerState`] backed by a temporary database.
/// The `TempDir` is returned so it remains alive for the duration of the test.
fn create_test_state() -> (DevscriptorServerState, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    debug!("Creating test state with DB at {}", db_path.display());
    let state = DevscriptorServerState::with_db_path(db_path)
        .expect("failed to create server state with custom DB path");
    (state, temp_dir)
}

// ============================================================================
// Server State Creation
// ============================================================================

#[tokio::test]
async fn test_server_state_new() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    // Should be able to access the database without panicking
    let db = state.database();
    assert!(Arc::strong_count(&db) >= 1, "database Arc should be valid");
    info!("test_server_state_new passed");
}

#[tokio::test]
async fn test_server_state_with_db_path() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let temp_dir = TempDir::new().unwrap();
    let custom_path = temp_dir.path().join("custom_mcp.db");

    let state = DevscriptorServerState::with_db_path(custom_path.clone())
        .expect("with_db_path should succeed");

    let db = state.database();
    assert!(Arc::strong_count(&db) >= 1);
    info!("test_server_state_with_db_path passed");
}

#[tokio::test]
async fn test_database_returns_valid_arc() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let db1 = state.database();
    let db2 = state.database();

    // Both should point to the same underlying Database
    assert_eq!(
        Arc::as_ptr(&db1),
        Arc::as_ptr(&db2),
        "database() should return clones of the same Arc"
    );
    assert!(Arc::strong_count(&db1) >= 2);
    info!("test_database_returns_valid_arc passed");
}

#[tokio::test]
async fn test_analysis_api_returns_valid_arc() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let api1 = state.analysis_api();
    let api2 = state.analysis_api();

    assert_eq!(
        Arc::as_ptr(&api1),
        Arc::as_ptr(&api2),
        "analysis_api() should return clones of the same Arc"
    );
    assert!(Arc::strong_count(&api1) >= 2);
    info!("test_analysis_api_returns_valid_arc passed");
}

#[tokio::test]
async fn test_get_health_status_structure() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();

    // Create some jobs to populate health metrics
    let _id1 = state.create_job("/test/path1");
    let id2 = state.create_job("/test/path2");
    state.update_job_progress(
        &id2,
        Some(50.0),
        None,
        None,
        None,
        None,
        Some(JobStatus::Running),
    );

    let health = state.get_health_status();

    assert_eq!(health["status"], "healthy", "status should be healthy");
    assert_eq!(
        health["version"],
        devscriptor_core::VERSION,
        "version should match devscriptor-core"
    );
    assert!(
        health["database"]["connected"].as_bool().unwrap_or(false),
        "database should be connected"
    );
    assert!(
        health["database"]["path"].as_str().is_some(),
        "database path should be present"
    );
    assert_eq!(
        health["jobs"]["active"].as_u64().unwrap_or(99),
        1,
        "active jobs count should be 1"
    );
    assert_eq!(
        health["jobs"]["in_memory"].as_u64().unwrap_or(99),
        2,
        "in_memory jobs count should be 2"
    );
    assert!(
        health["devscriptor"]["initialized"]
            .as_bool()
            .unwrap_or(false),
        "devscriptor should be initialized"
    );
    info!("test_get_health_status_structure passed");
}

/// A state made for a path uses that database. (What `new()` would open is the
/// user's shared default database, which a test must leave alone.)
#[tokio::test]
async fn test_custom_db_path_is_used() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let temp_dir = TempDir::new().unwrap();
    let custom_path = temp_dir.path().join("my_custom_devscriptor.db");

    let state = DevscriptorServerState::with_db_path(custom_path.clone())
        .expect("should create state with custom path");

    let health = state.get_health_status();
    let db_path_str = health["database"]["path"]
        .as_str()
        .expect("db path should be a string");

    assert!(
        db_path_str.contains("my_custom_devscriptor"),
        "DB path should reflect custom location: got {db_path_str}"
    );
    info!("test_custom_db_path_is_used passed");
}

#[tokio::test]
async fn test_searcher_returns_functional_codebase_searcher() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let searcher = state.searcher();

    // We can call search methods on the searcher; with an empty DB it should
    // return empty results rather than panic.
    let result = searcher.search_functions("test", false, None, None).await;
    assert!(
        result.is_ok(),
        "searcher should be functional even on empty DB"
    );
    let functions = result.unwrap();
    assert!(functions.is_empty(), "empty DB should yield no functions");

    let result = searcher.search_classes("Test", false, None, None).await;
    assert!(result.is_ok(), "search_classes should be functional");
    info!("test_searcher_returns_functional_codebase_searcher passed");
}
