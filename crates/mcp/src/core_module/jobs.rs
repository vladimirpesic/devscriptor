//! Job management tool implementations

use futures::FutureExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::info;

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::server::JobStatus;
use crate::tools::{AnalysisJobWire, CleanupJobsResponse, ListJobsResponse, StartAnalysisResponse};

impl CoreModule {
    pub(super) async fn start_analysis(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'path' argument".into()))?;

        let path_buf = Self::validate_tool_path(path)?;
        if !path_buf.exists() {
            return Err(ToolError::InvalidParams(format!(
                "Path does not exist: {path}"
            )));
        }
        if !path_buf.is_dir() {
            return Err(ToolError::InvalidParams(format!(
                "Path is not a directory: {path}"
            )));
        }

        // `extensions` whitelists file extensions; `exclude_patterns` are globs.
        let extensions = super::string_array_arg(&args, "extensions");
        let exclude_patterns = super::string_array_arg(&args, "exclude_patterns");

        let job_id = self.state.create_job(path);
        info!(
            "Started analysis job {} for path: {} (extensions: {:?}, exclude: {:?})",
            job_id, path, extensions, exclude_patterns
        );

        let state = Arc::clone(&self.state);
        let job_id_clone = job_id.clone();
        let path_clone = path_buf.clone();
        let state_for_handle = Arc::clone(&self.state);

        let handle = tokio::spawn(async move {
            state.update_job_progress(
                &job_id_clone,
                Some(0.0),
                None,
                None,
                None,
                Some("Initializing analysis...".to_string()),
                Some(JobStatus::Running),
            );

            // The full CodebasePipeline, so entities, relationships, checksums and
            // languages are written. Wrapped in catch_unwind so a panic doesn't leave
            // the job "Running".
            let db_clone = state.database();
            let project = state.project_config();
            let job_id_progress = job_id_clone.clone();
            let state_progress = Arc::clone(&state);
            let result = std::panic::AssertUnwindSafe(async {
                let pipeline_result = super::run_pipeline_with_progress(
                    db_clone,
                    &project,
                    &path_clone,
                    extensions,
                    exclude_patterns,
                    move |parsed, discovered, path_str| {
                        let progress = if discovered > 0 {
                            10.0 + (parsed as f64 / discovered as f64 * 80.0)
                        } else {
                            50.0
                        };
                        state_progress.update_job_progress(
                            &job_id_progress,
                            Some(progress),
                            Some(path_str),
                            Some(parsed),
                            Some(discovered),
                            Some(format!("Processed {parsed} of {discovered} files")),
                            None,
                        );
                    },
                )
                .await
                .map_err(|e| devscriptor_core::Error::Analysis(e.to_string()))?;

                let files_parsed = pipeline_result.files_parsed;
                let entity_count = pipeline_result.entity_count;

                state.update_job_progress(
                    &job_id_clone,
                    Some(90.0),
                    None,
                    Some(files_parsed),
                    Some(pipeline_result.files_discovered),
                    Some("Finalizing...".to_string()),
                    None,
                );

                let db = state.database();
                let (total_entities, total_relationships) = match db.get_stats().await {
                    Ok((_, entities, rels)) => (entities, rels),
                    Err(_) => (entity_count, 0),
                };

                let result = json!({
                    "status": "completed",
                    "files_processed": files_parsed,
                    "entities_extracted": entity_count,
                    "message": format!(
                        "Successfully analyzed {} files, extracted {} entities",
                        files_parsed, entity_count
                    ),
                    "stats": {
                        "total_entities": total_entities,
                        "total_relationships": total_relationships,
                    }
                });

                state.complete_job(&job_id_clone, result);
                // The scan may have indexed the workspace the server watches
                // by itself.
                state.watch_after_scan().await;
                Ok::<(), devscriptor_core::Error>(())
            })
            .catch_unwind()
            .await;

            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => state.fail_job(&job_id_clone, format!("Scan failed: {e}")),
                Err(_) => {
                    tracing::error!("Analysis job {} panicked", job_id_clone);
                    state.fail_job(&job_id_clone, "Analysis task panicked".to_string());
                }
            }
        });

        state_for_handle.store_job_handle(&job_id, handle);

        let response = StartAnalysisResponse {
            job_id,
            status: "started".to_string(),
            message: format!("Analysis started for: {path}"),
        };

        ok_json(&response)
    }

    pub(super) async fn get_analysis_progress(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let job_id = args
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'job_id' argument".into()))?;

        let wait = args
            .get("wait")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let timeout = args
            .get("timeout")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(30);

        let job = self
            .state
            .get_job(job_id)
            .ok_or_else(|| ToolError::InvalidParams(format!("Job not found: {job_id}")))?;

        if wait && job.status == JobStatus::Running {
            let start_time = std::time::Instant::now();
            let timeout_duration = std::time::Duration::from_secs(timeout);

            while self
                .state
                .get_job(job_id)
                .map_or(JobStatus::Completed, |j| j.status)
                == JobStatus::Running
            {
                if start_time.elapsed() > timeout_duration {
                    break;
                }
                // Use Notify channel instead of busy-wait polling.
                // When the job completes or fails, complete_job/fail_job calls
                // notify_waiters(), waking us up instantly.
                let remaining = timeout_duration.saturating_sub(start_time.elapsed());
                tokio::time::timeout(remaining, self.state.job_notify().notified())
                    .await
                    .ok();
            }
        }

        let job = self
            .state
            .get_job(job_id)
            .ok_or_else(|| ToolError::InvalidParams(format!("Job not found: {job_id}")))?;

        ok_json(&job.to_dict())
    }

    pub(super) async fn list_analysis_jobs(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let limit = arguments
            .as_ref()
            .and_then(|a| a.get("limit"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        let jobs_data = self.state.list_jobs(limit);

        let jobs: Vec<AnalysisJobWire> = jobs_data
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();

        let response = ListJobsResponse {
            total_jobs: jobs.len(),
            jobs,
        };

        ok_json(&response)
    }

    pub(super) async fn cleanup_old_jobs(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let max_age_seconds = arguments
            .as_ref()
            .and_then(|a| a.get("max_age_seconds"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(3600);

        let removed = self.state.cleanup_old_jobs(max_age_seconds);

        let response = CleanupJobsResponse {
            removed_jobs: removed,
            message: format!("Removed {removed} old jobs"),
        };

        ok_json(&response)
    }

    pub(super) async fn cancel_analysis(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let job_id = args
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'job_id' argument".into()))?;

        let cancelled = self.state.cancel_job(job_id);

        let response = crate::tools::CancelAnalysisResponse {
            job_id: job_id.to_string(),
            cancelled,
            message: if cancelled {
                format!("Job {job_id} cancelled successfully")
            } else {
                format!("Job {job_id} not found or already completed")
            },
        };

        ok_json(&response)
    }
}
