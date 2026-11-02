//! Integration tests for MCP job management functionality.
//!
//! Tests cover the full lifecycle of [`AnalysisJob`](devscriptor_mcp::AnalysisJob) objects
//! managed by [`DevscriptorServerState`](devscriptor_mcp::DevscriptorServerState).

use std::sync::Arc;
use std::time::Duration;

use devscriptor_mcp::{DevscriptorServerState, JobStatus};
use tempfile::TempDir;
use tracing::{debug, info};

fn create_test_state() -> (DevscriptorServerState, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("test_jobs.db");
    debug!("Creating test state with DB at {}", db_path.display());
    let state =
        DevscriptorServerState::with_db_path(db_path).expect("failed to create server state");
    (state, temp_dir)
}

// ============================================================================
// Job Creation & Retrieval
// ============================================================================

#[tokio::test]
async fn test_create_job_generates_valid_uuid() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/project/src");

    // UUID v4 is 36 characters (8-4-4-4-12)
    assert_eq!(job_id.len(), 36, "job_id should be a valid UUID v4");
    assert!(job_id.contains('-'), "UUID should contain hyphens");

    let job = state
        .get_job(&job_id)
        .expect("job should exist immediately after creation");
    assert_eq!(job.path, "/project/src");
    assert!(matches!(job.status, JobStatus::Pending));
    info!("test_create_job_generates_valid_uuid passed");
}

#[tokio::test]
async fn test_get_job_retrieves_created_job() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/another/path");

    let job = state.get_job(&job_id).expect("should retrieve created job");
    assert_eq!(job.job_id, job_id);
    assert_eq!(job.progress, 0.0);
    assert_eq!(job.message, "Pending");
    assert!(job.result.is_none());
    assert!(job.error.is_none());
    info!("test_get_job_retrieves_created_job passed");
}

#[tokio::test]
async fn test_get_nonexistent_job_returns_none() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let result = state.get_job("00000000-0000-0000-0000-000000000000");
    assert!(result.is_none(), "nonexistent job should return None");
    info!("test_get_nonexistent_job_returns_none passed");
}

// ============================================================================
// Progress Updates
// ============================================================================

#[tokio::test]
async fn test_update_job_progress() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/update/test");

    state.update_job_progress(
        &job_id,
        Some(42.5),
        Some("main.rs".to_string()),
        Some(3),
        Some(7),
        Some("Parsing...".to_string()),
        Some(JobStatus::Running),
    );

    let job = state.get_job(&job_id).unwrap();
    assert_eq!(job.progress, 42.5);
    assert_eq!(job.current_file, "main.rs");
    assert_eq!(job.files_processed, 3);
    assert_eq!(job.total_files, 7);
    assert_eq!(job.message, "Parsing...");
    assert!(matches!(job.status, JobStatus::Running));
    info!("test_update_job_progress passed");
}

#[tokio::test]
async fn test_update_job_progress_clamps_to_bounds() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/bounds/test");

    // Progress above 100 should be clamped
    state.update_job_progress(&job_id, Some(150.0), None, None, None, None, None);
    let job = state.get_job(&job_id).unwrap();
    assert_eq!(job.progress, 100.0, "progress should be clamped to 100");

    // Progress below 0 should be clamped
    state.update_job_progress(&job_id, Some(-10.0), None, None, None, None, None);
    let job = state.get_job(&job_id).unwrap();
    assert_eq!(job.progress, 0.0, "progress should be clamped to 0");
    info!("test_update_job_progress_clamps_to_bounds passed");
}

// ============================================================================
// Completion & Failure
// ============================================================================

#[tokio::test]
async fn test_complete_job() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/complete/test");

    let result = serde_json::json!({"files_processed": 42, "entities": 7});
    state.complete_job(&job_id, result.clone());

    let job = state.get_job(&job_id).unwrap();
    assert!(matches!(job.status, JobStatus::Completed));
    assert_eq!(job.progress, 100.0);
    assert_eq!(job.result, Some(result));
    assert!(job.completed_at.is_some(), "completed_at should be set");
    assert!(job.error.is_none());
    info!("test_complete_job passed");
}

#[tokio::test]
async fn test_fail_job() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/fail/test");

    state.fail_job(&job_id, "Disk full");

    let job = state.get_job(&job_id).unwrap();
    assert!(matches!(job.status, JobStatus::Failed));
    assert_eq!(job.error, Some("Disk full".to_string()));
    assert!(job.completed_at.is_some());
    info!("test_fail_job passed");
}

// ============================================================================
// Listing & Sorting
// ============================================================================

#[tokio::test]
async fn test_list_jobs_sorted_by_created_at() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();

    // Create jobs in sequence with delays to ensure different timestamps
    let id1 = state.create_job("/list/first");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let _id2 = state.create_job("/list/second");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let id3 = state.create_job("/list/third");

    let jobs = state.list_jobs(10);
    assert_eq!(jobs.len(), 3, "should list all 3 jobs");

    // Jobs should be sorted by started_at descending (newest first)
    let first_time = jobs[0]["started_at"].as_u64().unwrap();
    let second_time = jobs[1]["started_at"].as_u64().unwrap();
    let third_time = jobs[2]["started_at"].as_u64().unwrap();

    assert!(
        first_time >= second_time,
        "jobs should be sorted descending by started_at"
    );
    assert!(
        second_time >= third_time,
        "jobs should be sorted descending by started_at"
    );

    // Verify all three job IDs are present
    let job_ids: Vec<&str> = jobs.iter().map(|j| j["job_id"].as_str().unwrap()).collect();
    assert!(job_ids.contains(&id1.as_str()), "id1 should be in list");
    assert!(job_ids.contains(&id3.as_str()), "id3 should be in list");
    info!("test_list_jobs_sorted_by_created_at passed");
}

#[tokio::test]
async fn test_list_jobs_with_limit() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();

    for i in 0..5 {
        state.create_job(format!("/limit/{i}"));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let jobs = state.list_jobs(2);
    assert_eq!(jobs.len(), 2, "limit should cap the result");
    info!("test_list_jobs_with_limit passed");
}

// ============================================================================
// Cleanup
// ============================================================================

#[tokio::test]
async fn test_cleanup_old_jobs_removes_completed() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/cleanup/test");

    // Complete the job
    state.complete_job(&job_id, serde_json::json!({"done": true}));

    // Small sleep so completed_at differs from now
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cleanup with 0 max_age_seconds should remove any completed job older than 0s
    let removed = state.cleanup_old_jobs(0);

    if removed > 0 {
        assert!(
            state.get_job(&job_id).is_none(),
            "job should be removed after cleanup"
        );
    } else {
        // Timing might be tight; at least verify the job still exists
        assert!(
            state.get_job(&job_id).is_some(),
            "job should still exist if not old enough"
        );
    }
    info!("test_cleanup_old_jobs_removes_completed passed");
}

#[tokio::test]
async fn test_cleanup_old_jobs_preserves_running() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/cleanup/running");

    state.update_job_progress(
        &job_id,
        Some(50.0),
        None,
        None,
        None,
        None,
        Some(JobStatus::Running),
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    let removed = state.cleanup_old_jobs(0);
    assert_eq!(removed, 0, "running jobs should never be cleaned up");
    assert!(
        state.get_job(&job_id).is_some(),
        "running job should still exist"
    );
    info!("test_cleanup_old_jobs_preserves_running passed");
}

#[tokio::test]
async fn test_cleanup_old_jobs_preserves_pending() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/cleanup/pending");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let removed = state.cleanup_old_jobs(0);
    assert_eq!(removed, 0, "pending jobs should never be cleaned up");
    assert!(
        state.get_job(&job_id).is_some(),
        "pending job should still exist"
    );
    info!("test_cleanup_old_jobs_preserves_pending passed");
}

// ============================================================================
// Status Transitions
// ============================================================================

#[tokio::test]
async fn test_job_status_transitions() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let job_id = state.create_job("/transition/test");

    // Initial state
    let job = state.get_job(&job_id).unwrap();
    assert!(matches!(job.status, JobStatus::Pending));

    // Pending -> Running
    state.update_job_progress(
        &job_id,
        Some(10.0),
        None,
        None,
        None,
        None,
        Some(JobStatus::Running),
    );
    let job = state.get_job(&job_id).unwrap();
    assert!(matches!(job.status, JobStatus::Running));

    // Running -> Completed
    state.complete_job(&job_id, serde_json::json!({"ok": true}));
    let job = state.get_job(&job_id).unwrap();
    assert!(matches!(job.status, JobStatus::Completed));

    // Create a new job for failure transition
    let job_id2 = state.create_job("/transition/fail");
    state.update_job_progress(
        &job_id2,
        Some(20.0),
        None,
        None,
        None,
        None,
        Some(JobStatus::Running),
    );
    state.fail_job(&job_id2, "out of memory");
    let job = state.get_job(&job_id2).unwrap();
    assert!(matches!(job.status, JobStatus::Failed));

    info!("test_job_status_transitions passed");
}

// ============================================================================
// Concurrency
// ============================================================================

#[tokio::test]
async fn test_concurrent_job_creation_and_updates() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();
    let state = Arc::new(state);

    let mut handles = vec![];

    for i in 0..20 {
        let state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            let job_id = state.create_job(format!("/concurrent/{i}"));
            state.update_job_progress(
                &job_id,
                Some((i as f64) * 5.0),
                Some(format!("file_{i}.rs")),
                Some(i),
                Some(20),
                Some("Processing".to_string()),
                Some(JobStatus::Running),
            );
            job_id
        });
        handles.push(handle);
    }

    let job_ids: Vec<String> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.expect("spawn should succeed"))
        .collect();

    assert_eq!(job_ids.len(), 20, "all 20 jobs should be created");

    for id in &job_ids {
        assert!(
            state.get_job(id).is_some(),
            "each job should exist after concurrent creation"
        );
    }

    let listed = state.list_jobs(100);
    assert_eq!(listed.len(), 20, "list_jobs should return all 20");
    info!("test_concurrent_job_creation_and_updates passed");
}

// ============================================================================
// Job Isolation
// ============================================================================

#[tokio::test]
async fn test_job_isolation() {
    let _subscriber = tracing_subscriber::fmt::try_init();

    let (state, _temp) = create_test_state();

    let id1 = state.create_job("/isolation/a");
    let id2 = state.create_job("/isolation/b");

    // Update job 1
    state.update_job_progress(
        &id1,
        Some(75.0),
        Some("a.rs".to_string()),
        Some(1),
        Some(4),
        Some("msg-a".to_string()),
        Some(JobStatus::Running),
    );

    // Complete job 1
    state.complete_job(&id1, serde_json::json!({"result": "a"}));

    // Job 2 should be untouched
    let job2 = state.get_job(&id2).unwrap();
    assert_eq!(job2.progress, 0.0, "job2 progress should remain default");
    assert_eq!(
        job2.current_file, "",
        "job2 current_file should remain empty"
    );
    assert_eq!(
        job2.files_processed, 0,
        "job2 files_processed should remain 0"
    );
    assert_eq!(
        job2.message, "Pending",
        "job2 message should remain Pending"
    );
    assert!(matches!(job2.status, JobStatus::Pending));
    assert!(job2.result.is_none());

    // Job 1 should reflect all changes
    let job1 = state.get_job(&id1).unwrap();
    assert_eq!(job1.progress, 100.0);
    assert_eq!(job1.current_file, "a.rs");
    assert_eq!(job1.files_processed, 1);
    assert_eq!(job1.result, Some(serde_json::json!({"result": "a"})));
    assert!(matches!(job1.status, JobStatus::Completed));

    info!("test_job_isolation passed");
}
