//! Analysis API for Devscriptor
//!
//! This module provides a unified analysis interface with job management,
//! result aggregation, and async analysis coordination.

use crate::analysis::{
    AnalysisGuardConfig, CallGraphGenerator, CloneDetector, CodeSmellDetector, ComplexityAnalyzer,
    CrossFileAnalyzer, CrossReferenceAnalyzer, DeadCodeDetector, Severity,
};
use crate::database::Database;
use crate::entities::EntityId;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};
use uuid::Uuid;

/// Unique identifier for analysis jobs
pub type JobId = Uuid;

/// Status of an analysis job
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Job is pending
    Pending,
    /// Job is running
    Running,
    /// Job completed successfully
    Completed,
    /// Job failed
    Failed,
    /// Job was cancelled
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Type of analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisType {
    /// Dead code detection
    DeadCode,
    /// Complexity analysis
    Complexity,
    /// Clone detection
    CloneDetection,
    /// Cross-reference analysis
    CrossReference,
    /// Call graph analysis
    CallGraph,
    /// Code smells detection
    CodeSmells,
    /// Cross-file analysis
    CrossFile,
    /// All analyses
    All,
}

impl std::fmt::Display for AnalysisType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisType::DeadCode => write!(f, "dead_code"),
            AnalysisType::Complexity => write!(f, "complexity"),
            AnalysisType::CloneDetection => write!(f, "clone_detection"),
            AnalysisType::CrossReference => write!(f, "cross_reference"),
            AnalysisType::CallGraph => write!(f, "call_graph"),
            AnalysisType::CodeSmells => write!(f, "code_smells"),
            AnalysisType::CrossFile => write!(f, "cross_file"),
            AnalysisType::All => write!(f, "all"),
        }
    }
}

/// Single analysis finding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisFinding {
    /// Finding ID
    pub id: Uuid,
    /// Analysis type that generated this finding
    pub analysis_type: AnalysisType,
    /// Severity level
    pub severity: Severity,
    /// Finding title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// File path (if applicable)
    pub file_path: Option<String>,
    /// Line number (if applicable)
    pub line_number: Option<u32>,
    /// Entity ID (if applicable)
    pub entity_id: Option<EntityId>,
    /// Additional metadata
    pub metadata: Option<serde_json::Value>,
    /// Timestamp when finding was created (RFC 3339 when serialized)
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

impl AnalysisFinding {
    /// Create a new analysis finding
    #[must_use]
    pub fn new(
        analysis_type: AnalysisType,
        severity: Severity,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            analysis_type,
            severity,
            title: title.into(),
            description: description.into(),
            file_path: None,
            line_number: None,
            entity_id: None,
            metadata: None,
            created_at: time::OffsetDateTime::now_utc(),
        }
    }

    /// Set file path
    pub fn with_file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Set line number
    pub fn with_line_number(mut self, line: u32) -> Self {
        self.line_number = Some(line);
        self
    }

    /// Set entity ID
    pub fn with_entity_id(mut self, id: EntityId) -> Self {
        self.entity_id = Some(id);
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Analysis job information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisJob {
    /// Job ID
    pub id: JobId,
    /// Job status
    pub status: JobStatus,
    /// Analysis type
    pub analysis_type: AnalysisType,
    /// Target codebase path
    pub target_path: String,
    /// Minimum severity to report
    pub min_severity: Severity,
    /// Progress (0.0 to 1.0)
    pub progress: f64,
    /// Current status message
    pub status_message: String,
    /// Created timestamp (RFC 3339 when serialized, as are the two below)
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Started timestamp
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<time::OffsetDateTime>,
    /// Completed timestamp
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<time::OffsetDateTime>,
    /// Error message (if failed)
    pub error_message: Option<String>,
}

impl AnalysisJob {
    /// Create a new analysis job
    #[must_use]
    pub fn new(
        analysis_type: AnalysisType,
        target_path: impl Into<String>,
        min_severity: Severity,
    ) -> Self {
        Self {
            id: JobId::new_v4(),
            status: JobStatus::Pending,
            analysis_type,
            target_path: target_path.into(),
            min_severity,
            progress: 0.0,
            status_message: "Pending".to_string(),
            created_at: time::OffsetDateTime::now_utc(),
            started_at: None,
            completed_at: None,
            error_message: None,
        }
    }

    /// Mark job as running
    pub fn mark_running(&mut self) {
        self.status = JobStatus::Running;
        self.started_at = Some(time::OffsetDateTime::now_utc());
        self.status_message = "Running".to_string();
    }

    /// Mark job as completed
    pub fn mark_completed(&mut self) {
        self.status = JobStatus::Completed;
        self.completed_at = Some(time::OffsetDateTime::now_utc());
        self.progress = 1.0;
        self.status_message = "Completed".to_string();
    }

    /// Mark job as failed
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.status = JobStatus::Failed;
        self.completed_at = Some(time::OffsetDateTime::now_utc());
        self.error_message = Some(error.into());
        self.status_message = "Failed".to_string();
    }

    /// Update progress
    pub fn update_progress(&mut self, progress: f64, message: impl Into<String>) {
        self.progress = progress.clamp(0.0, 1.0);
        self.status_message = message.into();
    }

    /// Get duration since job started
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let start = self.started_at?;
        let end = self
            .completed_at
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        let duration_secs = (end - start).as_seconds_f64();
        Some(Duration::from_secs_f64(duration_secs))
    }
}

/// Analysis results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisResults {
    /// Job ID
    pub job_id: Option<JobId>,
    /// All findings (shared via Arc to avoid cloning during categorisation)
    pub findings: Vec<Arc<AnalysisFinding>>,
    /// Findings grouped by severity
    pub findings_by_severity: HashMap<String, Vec<Arc<AnalysisFinding>>>,
    /// Findings grouped by type
    pub findings_by_type: HashMap<String, Vec<Arc<AnalysisFinding>>>,
    /// Summary statistics
    pub summary: AnalysisSummary,
    /// Execution duration
    pub duration: Option<Duration>,
}

/// Analysis summary statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnalysisSummary {
    /// Total number of findings
    pub total_findings: usize,
    /// Critical findings
    pub critical_count: usize,
    /// High severity findings
    pub high_count: usize,
    /// Medium severity findings
    pub medium_count: usize,
    /// Low severity findings
    pub low_count: usize,
    /// Info findings
    pub info_count: usize,
    /// Number of files analyzed
    pub files_analyzed: usize,
    /// Number of entities analyzed
    pub entities_analyzed: usize,
    /// Mean cyclomatic complexity across **all** analyzed entities (not only those
    /// flagged as findings). Populated by complexity analysis; 0.0 otherwise.
    pub average_complexity: f64,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl AnalysisSummary {
    /// Create a new analysis summary
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment finding count by severity
    pub fn add_finding(&mut self, severity: Severity) {
        self.total_findings += 1;
        match severity {
            Severity::Critical => self.critical_count += 1,
            Severity::High => self.high_count += 1,
            Severity::Medium => self.medium_count += 1,
            Severity::Low => self.low_count += 1,
            Severity::Info => self.info_count += 1,
        }
    }
}

impl AnalysisResults {
    /// Create new empty results
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a finding to the results.
    ///
    /// Wraps the finding in an [`Arc`] so that categorisation into
    /// `findings_by_severity` and `findings_by_type` only requires cheap
    /// refcount bumps instead of deep clones.
    pub fn add_finding(&mut self, finding: AnalysisFinding) {
        self.add_arc_finding(Arc::new(finding));
    }

    /// Add a pre-wrapped finding (used by [`merge`] to move Arcs without clones).
    fn add_arc_finding(&mut self, finding: Arc<AnalysisFinding>) {
        let severity_key = finding.severity.to_string();
        let type_key = finding.analysis_type.to_string();

        // Update summary counts
        self.summary.total_findings += 1;
        match finding.severity {
            Severity::Critical => self.summary.critical_count += 1,
            Severity::High => self.summary.high_count += 1,
            Severity::Medium => self.summary.medium_count += 1,
            Severity::Low => self.summary.low_count += 1,
            Severity::Info => self.summary.info_count += 1,
        }

        self.findings_by_severity
            .entry(severity_key)
            .or_default()
            .push(Arc::clone(&finding));
        self.findings_by_type
            .entry(type_key)
            .or_default()
            .push(Arc::clone(&finding));
        self.findings.push(finding);
    }

    /// Add multiple findings
    pub fn add_findings(&mut self, findings: Vec<AnalysisFinding>) {
        for finding in findings {
            self.add_finding(finding);
        }
    }

    /// Get findings filtered by minimum severity
    #[must_use]
    pub fn filtered_by_severity(&self, min_severity: Severity) -> Vec<&AnalysisFinding> {
        self.findings
            .iter()
            .filter(|f| f.severity >= min_severity)
            .map(|f| f.as_ref())
            .collect()
    }

    /// Get findings for a specific analysis type
    #[must_use]
    pub fn get_by_type(&self, analysis_type: AnalysisType) -> Vec<&AnalysisFinding> {
        self.findings
            .iter()
            .filter(|f| f.analysis_type == analysis_type)
            .map(|f| f.as_ref())
            .collect()
    }

    /// Merge another results set into this one
    pub fn merge(&mut self, other: AnalysisResults) {
        for finding in other.findings {
            self.add_arc_finding(finding);
        }
    }

    /// Consume `self` and return the findings located under `root` — in their
    /// file, or, for one spanning several (a clone pair, a call cycle), in any
    /// of the `files` its metadata lists — counted again. A finding of no
    /// file, a summary of the whole index, is left out.
    #[must_use]
    pub fn into_within(self, root: &std::path::Path) -> Self {
        let entities_analyzed = self.summary.entities_analyzed;
        let average_complexity = self.summary.average_complexity;
        let mut out = Self {
            job_id: self.job_id,
            duration: self.duration,
            ..Self::new()
        };
        for arc in self.findings {
            let listed = arc
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("files"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str);
            let mut files = arc.file_path.as_deref().into_iter().chain(listed);
            if files.any(|file| std::path::Path::new(file).starts_with(root)) {
                out.add_arc_finding(arc);
            }
        }
        out.summary.entities_analyzed = entities_analyzed;
        out.summary.average_complexity = average_complexity;
        out
    }

    /// Consume `self` and return a new `AnalysisResults` containing only
    /// findings whose severity is ≥ `min`.  Summary counts are recomputed
    /// from scratch so they stay consistent with the filtered finding list.
    #[must_use]
    pub fn into_filtered(self, min: Severity) -> Self {
        let duration = self.duration;
        let job_id = self.job_id;
        // `entities_analyzed` and `average_complexity` describe the analysis
        // *scope* (how many entities were examined, and their mean complexity),
        // not the findings, so they must survive severity filtering. Without this
        // they reset to 0 whenever a filter runs (e.g. complexity analysis, which
        // always filters at `Low`), making the MCP `analyze_complexity` tool
        // report `total_functions_analyzed: 0` / `average_complexity: 0.0`.
        let entities_analyzed = self.summary.entities_analyzed;
        let average_complexity = self.summary.average_complexity;
        let mut out = Self {
            job_id,
            duration,
            ..Self::new()
        };
        for arc in self.findings {
            if arc.severity >= min {
                out.add_arc_finding(arc);
            }
        }
        out.summary.entities_analyzed = entities_analyzed;
        out.summary.average_complexity = average_complexity;
        out
    }
}

/// Job manager for coordinating analysis jobs
pub struct JobManager {
    jobs: Arc<parking_lot::RwLock<HashMap<JobId, AnalysisJob>>>,
    results: Arc<parking_lot::RwLock<HashMap<JobId, AnalysisResults>>>,
    database: Arc<Database>,
    guard_config: AnalysisGuardConfig,
}

impl JobManager {
    /// Create a new job manager with default guard configuration
    pub fn new(database: Arc<Database>) -> Self {
        Self::with_config(database, AnalysisGuardConfig::default())
    }

    /// Create a new job manager with a specific guard configuration
    #[must_use]
    pub fn with_config(database: Arc<Database>, guard_config: AnalysisGuardConfig) -> Self {
        Self {
            jobs: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            results: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            database,
            guard_config,
        }
    }

    /// Create and queue a new analysis job.
    pub async fn create_job(
        &self,
        analysis_type: AnalysisType,
        target_path: impl Into<String>,
        min_severity: Severity,
    ) -> JobId {
        let job = AnalysisJob::new(analysis_type, target_path, min_severity);
        let job_id = job.id;

        {
            let mut jobs = self.jobs.write();
            jobs.insert(job_id, job);
        }

        self.prune_old_jobs().await;

        info!("Created analysis job {} of type {}", job_id, analysis_type);
        job_id
    }

    /// Remove oldest completed jobs (and their results) until the total job
    /// count is within `guard_config.max_jobs`.
    async fn prune_old_jobs(&self) {
        let max_jobs = self.guard_config.max_jobs;
        if max_jobs == usize::MAX {
            return;
        }

        loop {
            let job_count = {
                let jobs = self.jobs.read();
                jobs.len()
            };
            if job_count <= max_jobs {
                break;
            }

            let mut jobs = self.jobs.write();
            let mut results = self.results.write();

            let candidate = jobs
                .iter()
                .filter(|(_, job)| {
                    job.status == JobStatus::Completed
                        || job.status == JobStatus::Failed
                        || job.status == JobStatus::Cancelled
                })
                .min_by_key(|(_, job)| job.created_at)
                .map(|(id, _)| *id);

            if let Some(id) = candidate {
                jobs.remove(&id);
                results.remove(&id);
            } else {
                break;
            }
        }
    }

    /// Get job information
    pub async fn get_job(&self, job_id: JobId) -> Option<AnalysisJob> {
        let jobs = self.jobs.read();
        jobs.get(&job_id).cloned()
    }

    /// Get job results
    pub async fn get_results(&self, job_id: JobId) -> Option<AnalysisResults> {
        let results = self.results.read();
        results.get(&job_id).cloned()
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<AnalysisJob> {
        let jobs = self.jobs.read();
        jobs.values().cloned().collect()
    }

    /// List jobs filtered by status
    pub async fn list_jobs_by_status(&self, status: JobStatus) -> Vec<AnalysisJob> {
        let jobs = self.jobs.read();
        jobs.values()
            .filter(|j| j.status == status)
            .cloned()
            .collect()
    }

    /// Cancel a job
    pub async fn cancel_job(&self, job_id: JobId) -> Result<()> {
        let mut jobs = self.jobs.write();
        if let Some(job) = jobs.get_mut(&job_id) {
            if job.status == JobStatus::Running || job.status == JobStatus::Pending {
                job.status = JobStatus::Cancelled;
                job.completed_at = Some(time::OffsetDateTime::now_utc());
                job.status_message = "Cancelled".to_string();
                info!("Cancelled analysis job {}", job_id);
                Ok(())
            } else {
                Err(Error::Generic(format!(
                    "Job {} cannot be cancelled (status: {})",
                    job_id, job.status
                )))
            }
        } else {
            Err(Error::Generic(format!("Job {} not found", job_id)))
        }
    }

    /// Check if a job has been cancelled
    async fn is_job_cancelled(&self, job_id: JobId) -> bool {
        self.get_job(job_id)
            .await
            .is_none_or(|job| job.status == JobStatus::Cancelled)
    }

    /// Delete a job and its results
    pub async fn delete_job(&self, job_id: JobId) -> Result<()> {
        let mut jobs = self.jobs.write();
        let mut results = self.results.write();

        if jobs.remove(&job_id).is_none() {
            return Err(Error::Generic(format!("Job {} not found", job_id)));
        }

        results.remove(&job_id);
        debug!("Deleted analysis job {}", job_id);
        Ok(())
    }

    /// Execute an analysis job
    async fn execute_job(&self, job_id: JobId) -> Result<AnalysisResults> {
        let job = self
            .get_job(job_id)
            .await
            .ok_or_else(|| Error::Generic(format!("Job {} not found", job_id)))?;

        if job.status != JobStatus::Running {
            return Err(Error::Generic(format!(
                "Job {} is not in running state",
                job_id
            )));
        }

        let start_time = Instant::now();
        let mut results = AnalysisResults::new();
        results.job_id = Some(job_id);

        info!(
            "Executing analysis job {} of type {}",
            job_id, job.analysis_type
        );

        if self.is_job_cancelled(job_id).await {
            return Err(Error::Generic(format!(
                "Job {} was cancelled before execution",
                job_id
            )));
        }

        match job.analysis_type {
            AnalysisType::DeadCode => {
                self.run_dead_code_analysis(job_id, &job.target_path, &mut results)
                    .await?;
            }
            AnalysisType::Complexity => {
                self.run_complexity_analysis(job_id, &mut results).await?;
            }
            AnalysisType::CloneDetection => {
                self.run_clone_detection(job_id, &mut results).await?;
            }
            AnalysisType::CrossReference => {
                self.run_cross_reference_analysis(job_id, &mut results)
                    .await?;
            }
            AnalysisType::CallGraph => {
                self.run_call_graph_analysis(job_id, &mut results).await?;
            }
            AnalysisType::CodeSmells => {
                self.run_code_smells_analysis(job_id, &mut results).await?;
            }
            AnalysisType::CrossFile => {
                self.run_cross_file_analysis(job_id, &mut results).await?;
            }
            AnalysisType::All => {
                self.run_all_analyses(job_id, &job.target_path, &mut results)
                    .await?;
            }
        }

        results.duration = Some(start_time.elapsed());

        // A path keeps the findings located under it, for every analysis; `.` or
        // none is the whole index.
        if !matches!(job.target_path.as_str(), "" | ".") {
            let root = std::path::Path::new(&job.target_path);
            let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            results = results.into_within(&root);
        }

        // Apply min_severity before persisting and returning: callers that request
        // High+ must not receive Info/Low findings.
        let min_severity = job.min_severity;
        if min_severity > Severity::Info {
            results = results.into_filtered(min_severity);
        }

        {
            let mut results_map = self.results.write();
            results_map.insert(job_id, results.clone());
        }

        info!(
            "Analysis job {} completed with {} findings (min_severity={:?}) in {:?}",
            job_id, results.summary.total_findings, min_severity, results.duration
        );

        Ok(results)
    }

    // ── Individual analysis runners ──────────────────────────────────

    /// The dead code of the entities under `target_path` — of the whole index
    /// for `.` or no path; what uses them is looked for everywhere.
    async fn run_dead_code_analysis(
        &self,
        job_id: JobId,
        target_path: &str,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running dead code analysis")
            .await;

        let mut detector = DeadCodeDetector::new(self.database.clone());
        if !matches!(target_path, "" | ".") {
            detector = detector.within(target_path);
        }
        let report = detector.analyze().await?;

        // Build findings straight from the report rather than through
        // `to_findings()`: a `Finding` has no confidence field, and would drop the
        // per-entity `confidence` and `reason`. The MCP `find_dead_code` tool
        // filters on `metadata.confidence` (its `min_confidence` parameter).
        let findings: Vec<AnalysisFinding> = report
            .dead_functions
            .iter()
            .chain(report.dead_classes.iter())
            .map(|e| {
                AnalysisFinding::new(
                    AnalysisType::DeadCode,
                    Severity::Medium,
                    format!("Unused {}: {}", e.entity_type, e.name),
                    e.reason.clone(),
                )
                .with_file_path(e.file_path.clone())
                .with_line_number(e.line)
                .with_metadata(serde_json::json!({
                    "entity_name": e.name,
                    "entity_type": e.entity_type,
                    "language": e.language,
                    "reason": e.reason,
                    "confidence": e.confidence,
                }))
            })
            .collect();

        results.add_findings(findings);
        // Propagate the analysis scope so the count survives severity filtering
        // (matches complexity; without it the MCP tool reports 0 entities).
        results.summary.entities_analyzed = report.summary.entities_analyzed;

        self.update_job_progress(job_id, 1.0, "Dead code analysis complete")
            .await;
        Ok(())
    }

    async fn run_complexity_analysis(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running complexity analysis")
            .await;

        let analyzer = ComplexityAnalyzer::new();
        let report = analyzer.analyze(self.database.clone()).await?;

        for metrics in &report.high_complexity_entities {
            let severity = if metrics.cyclomatic_complexity > 20 {
                Severity::High
            } else if metrics.cyclomatic_complexity > 10 {
                Severity::Medium
            } else {
                Severity::Low
            };

            let finding = AnalysisFinding::new(
                AnalysisType::Complexity,
                severity,
                format!("Complex function: {}", metrics.entity_name),
                format!(
                    "Cyclomatic complexity: {}, Lines: {}",
                    metrics.cyclomatic_complexity, metrics.lines_of_code
                ),
            )
            .with_file_path(&metrics.file_path)
            .with_line_number(metrics.start_line)
            .with_metadata(serde_json::json!({
                "entity_name": metrics.entity_name,
                "cyclomatic": metrics.cyclomatic_complexity,
                "cognitive": metrics.cognitive_complexity,
                "lines": metrics.lines_of_code,
            }));

            results.add_finding(finding);
        }

        // Propagate the analyzer's own count so callers report how many functions
        // were actually examined, not just how many exceeded the complexity
        // threshold. Without this the MCP `analyze_complexity` tool's
        // `total_functions_analyzed` is always 0 whenever nothing is flagged.
        results.summary.entities_analyzed = report.summary.entities_analyzed;
        results.summary.average_complexity = report.summary.average_complexity;

        self.update_job_progress(job_id, 1.0, "Complexity analysis complete")
            .await;
        Ok(())
    }

    async fn run_clone_detection(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running clone detection")
            .await;

        let detector = CloneDetector::new().with_guard_config(self.guard_config);
        let report = detector.analyze(self.database.clone()).await?;

        for pair in &report.clones {
            let finding = AnalysisFinding::new(
                AnalysisType::CloneDetection,
                Severity::Medium,
                format!(
                    "Code clone detected: {:.1}% similar",
                    pair.similarity_score * 100.0
                ),
                format!(
                    "Similar code found in {}:{} and {}:{}",
                    pair.file1_path, pair.start_line1, pair.file2_path, pair.start_line2
                ),
            )
            .with_file_path(&pair.file1_path)
            .with_line_number(pair.start_line1)
            .with_metadata(serde_json::json!({
                "similarity": pair.similarity_score,
                "source_file": pair.file1_path,
                "target_file": pair.file2_path,
                "source_lines": pair.start_line1,
                "target_lines": pair.start_line2,
                "files": [pair.file1_path, pair.file2_path],
            }));

            results.add_finding(finding);
        }
        results.summary.entities_analyzed = report.summary.entities_analyzed;

        self.update_job_progress(job_id, 1.0, "Clone detection complete")
            .await;
        Ok(())
    }

    async fn run_cross_reference_analysis(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running cross-reference analysis")
            .await;

        let analyzer = CrossReferenceAnalyzer::new();
        let report = analyzer.analyze(&self.database).await?;

        for unused in &report.unused_entities {
            let finding = AnalysisFinding::new(
                AnalysisType::CrossReference,
                Severity::Low,
                format!("Unused entity: {}", unused.entity_name),
                "Entity is not referenced by any other code".to_string(),
            )
            .with_file_path(&unused.file_path)
            .with_line_number(unused.line);

            results.add_finding(finding);
        }
        results.summary.entities_analyzed = report.summary.entities_analyzed;

        self.update_job_progress(job_id, 1.0, "Cross-reference analysis complete")
            .await;
        Ok(())
    }

    async fn run_call_graph_analysis(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running call graph analysis")
            .await;

        let functions = self.database.get_all_functions().await?;
        let classes = self.database.get_all_classes().await?;
        let relationships = self.database.get_all_relationships().await?;

        let guard_config = self.guard_config;
        let report = tokio::task::spawn_blocking(move || {
            let mut generator = CallGraphGenerator::new().with_guard_config(guard_config);
            let call_graph = generator.generate_call_graph(&functions, &classes, &relationships)?;
            generator.generate_report(&call_graph)
        })
        .await
        .map_err(|e| Error::Generic(format!("Call graph task failed: {e}")))??;

        if !report.circular_dependencies.is_empty() {
            for cycle in &report.circular_dependencies {
                let functions = cycle.functions.join(", ");
                results.add_finding(
                    AnalysisFinding::new(
                        AnalysisType::CallGraph,
                        Severity::Medium,
                        "Circular dependency detected",
                        format!("Cycle {}: {}", cycle.cycle_id, functions),
                    )
                    .with_file_path(&cycle.file_path)
                    .with_line_number(cycle.line)
                    .with_metadata(serde_json::json!({
                        "functions": cycle.functions,
                        "files": cycle.files,
                    })),
                );
            }
        }

        if !report.isolated_functions.is_empty() {
            for func in &report.isolated_functions {
                results.add_finding(
                    AnalysisFinding::new(
                        AnalysisType::CallGraph,
                        Severity::Low,
                        format!("Isolated function: {}", func.name),
                        format!(
                            "Function '{}' in '{}' has no calls in or out",
                            func.name, func.file_path
                        ),
                    )
                    .with_file_path(&func.file_path)
                    .with_line_number(func.start_line),
                );
            }
        }

        results.add_finding(
            AnalysisFinding::new(
                AnalysisType::CallGraph,
                Severity::Info,
                "Call graph analysis summary",
                format!(
                    "Total functions: {}, Total calls: {}, Root functions: {}, Max depth: {}, Circular dependencies: {}",
                    report.summary.total_functions,
                    report.summary.total_calls,
                    report.summary.root_functions,
                    report.summary.max_call_depth,
                    report.summary.circular_dependencies
                ),
            )
            .with_metadata(serde_json::json!({
                "summary": report.summary,
                "recommendations": report.recommendations,
            })),
        );

        self.update_job_progress(job_id, 1.0, "Call graph analysis complete")
            .await;
        Ok(())
    }

    async fn run_code_smells_analysis(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running code smells analysis")
            .await;

        let functions = self.database.get_all_functions().await?;
        let classes = self.database.get_all_classes().await?;

        let mut files_content: HashMap<String, String> = HashMap::new();
        for func in &functions {
            if !files_content.contains_key(&func.file_path)
                && let Ok(content) = tokio::fs::read_to_string(&func.file_path).await
            {
                files_content.insert(func.file_path.clone(), content);
            }
        }

        let report = tokio::task::spawn_blocking(move || {
            let detector = CodeSmellDetector::new();
            detector.detect(&functions, &classes, &files_content)
        })
        .await
        .map_err(|e| Error::Generic(format!("Code smell task failed: {e}")))??;

        for smell in &report.code_smells {
            results.add_finding(
                AnalysisFinding::new(
                    AnalysisType::CodeSmells,
                    smell.severity,
                    format!("{}: {}", smell.name, smell.description),
                    smell.context.clone(),
                )
                .with_file_path(&smell.file_path)
                .with_line_number(smell.line_number)
                .with_metadata(serde_json::json!({
                    "entity_name": smell.entity_name,
                    "entity_type": smell.entity_type,
                    "suggestions": smell.suggestions,
                })),
            );
        }

        results.add_finding(
            AnalysisFinding::new(
                AnalysisType::CodeSmells,
                Severity::Info,
                "Code smells analysis summary",
                format!(
                    "Total smells: {}, Critical: {}, High: {}, Medium: {}, Low: {}",
                    report.summary.total_findings,
                    report
                        .severity_distribution
                        .get("critical")
                        .copied()
                        .unwrap_or(0),
                    report
                        .severity_distribution
                        .get("high")
                        .copied()
                        .unwrap_or(0),
                    report
                        .severity_distribution
                        .get("medium")
                        .copied()
                        .unwrap_or(0),
                    report
                        .severity_distribution
                        .get("low")
                        .copied()
                        .unwrap_or(0),
                ),
            )
            .with_metadata(serde_json::json!({
                "smell_distribution": report.smell_distribution,
                "recommendations": report.recommendations,
            })),
        );

        self.update_job_progress(job_id, 1.0, "Code smells analysis complete")
            .await;
        Ok(())
    }

    async fn run_cross_file_analysis(
        &self,
        job_id: JobId,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        self.update_job_progress(job_id, 0.1, "Running cross-file analysis")
            .await;

        let functions = self.database.get_all_functions().await?;
        let classes = self.database.get_all_classes().await?;

        let mut files_content: HashMap<String, String> = HashMap::new();
        for func in &functions {
            if !files_content.contains_key(&func.file_path)
                && let Ok(content) = tokio::fs::read_to_string(&func.file_path).await
            {
                files_content.insert(func.file_path.clone(), content);
            }
        }
        for class in &classes {
            if !files_content.contains_key(&class.file_path)
                && let Ok(content) = tokio::fs::read_to_string(&class.file_path).await
            {
                files_content.insert(class.file_path.clone(), content);
            }
        }

        let report = tokio::task::spawn_blocking(move || {
            let analyzer = CrossFileAnalyzer::new();
            analyzer.analyze(&functions, &classes, &files_content)
        })
        .await
        .map_err(|e| Error::Generic(format!("Cross-file task failed: {e}")))??;

        for finding in report.to_findings() {
            results.add_finding(
                AnalysisFinding::new(
                    AnalysisType::CrossFile,
                    finding.severity,
                    finding.message.clone(),
                    finding.context.clone().unwrap_or_default(),
                )
                .with_file_path(&finding.file_path)
                .with_line_number(finding.line)
                .with_metadata(serde_json::json!({
                    "entity_name": finding.entity_name,
                    "entity_type": finding.entity_type,
                    "suggestion": finding.suggestion,
                })),
            );
        }

        results.add_finding(
            AnalysisFinding::new(
                AnalysisType::CrossFile,
                Severity::Info,
                "Cross-file analysis summary",
                format!(
                    "Total files: {}, Dependencies: {}, Clusters: {}, Circular dependencies: {}",
                    report.summary.files_analyzed,
                    report.dependencies.len(),
                    report.clusters.len(),
                    report.circular_dependencies.len()
                ),
            )
            .with_metadata(serde_json::json!({
                "architecture_layers": report.architecture_layers,
                "recommendations": report.recommendations,
            })),
        );

        self.update_job_progress(job_id, 1.0, "Cross-file analysis complete")
            .await;
        Ok(())
    }

    // ── "Run all" — sequential to bound memory ───────────────────────

    async fn run_all_analyses(
        &self,
        job_id: JobId,
        target_path: &str,
        results: &mut AnalysisResults,
    ) -> Result<()> {
        if self.is_job_cancelled(job_id).await {
            return Err(Error::Generic(format!(
                "Job {} cancelled before analyses",
                job_id
            )));
        }

        self.update_job_progress(job_id, 0.0, "Running all analyses sequentially")
            .await;

        // Run the analyses one after another, to bound memory: each loads its own
        // copy of all functions, classes and files (~50 MB), and seven at once
        // would hold seven.

        let mut first_error: Option<Error> = None;

        macro_rules! run_step {
            ($idx:expr, $name:expr, $fn:ident $(, $arg:expr)*) => {
                if self.is_job_cancelled(job_id).await {
                    return Err(Error::Generic(format!(
                        "Job {} cancelled during analyses",
                        job_id
                    )));
                }
                self.update_job_progress(
                    job_id,
                    ($idx as f64) / 7.0,
                    format!("Running {} analysis ({}/7)", $name, $idx + 1),
                )
                .await;
                if let Err(e) = self.$fn(job_id, $($arg,)* results).await {
                    tracing::warn!("Analysis '{}' failed: {}", $name, e);
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            };
        }

        run_step!(0, "dead_code", run_dead_code_analysis, target_path);
        run_step!(1, "complexity", run_complexity_analysis);
        run_step!(2, "clone_detection", run_clone_detection);
        run_step!(3, "cross_reference", run_cross_reference_analysis);
        run_step!(4, "call_graph", run_call_graph_analysis);
        run_step!(5, "code_smells", run_code_smells_analysis);
        run_step!(6, "cross_file", run_cross_file_analysis);

        if let Some(e) = first_error {
            return Err(e);
        }

        self.update_job_progress(job_id, 1.0, "All analyses complete")
            .await;

        Ok(())
    }

    async fn update_job_progress(&self, job_id: JobId, progress: f64, message: impl Into<String>) {
        let mut jobs = self.jobs.write();
        if let Some(job) = jobs.get_mut(&job_id) {
            job.update_progress(progress, message);
        }
    }

    /// Start and run a job to completion
    pub async fn run_job(&self, job_id: JobId) -> Result<AnalysisResults> {
        {
            let mut jobs = self.jobs.write();
            if let Some(job) = jobs.get_mut(&job_id) {
                if job.status != JobStatus::Pending {
                    return Err(Error::Generic(format!(
                        "Job {} is not in pending state",
                        job_id
                    )));
                }
                job.mark_running();
            } else {
                return Err(Error::Generic(format!("Job {} not found", job_id)));
            }
        }

        let result = self.execute_job(job_id).await;

        {
            let mut jobs = self.jobs.write();
            if let Some(job) = jobs.get_mut(&job_id) {
                match result {
                    Ok(_) => job.mark_completed(),
                    Err(ref e) => job.mark_failed(e.to_string()),
                }
            }
        }

        result
    }
}

/// Main analysis API
pub struct AnalysisApi {
    job_manager: Arc<JobManager>,
}

impl AnalysisApi {
    /// Create a new analysis API
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            job_manager: Arc::new(JobManager::new(database)),
        }
    }

    /// Run a single analysis synchronously
    ///
    /// # Errors
    /// Returns an error if the analysis fails
    pub async fn analyze(
        &self,
        analysis_type: AnalysisType,
        target_path: impl Into<String>,
        min_severity: Severity,
    ) -> Result<AnalysisResults> {
        let job_id = self
            .job_manager
            .create_job(analysis_type, target_path, min_severity)
            .await;
        self.job_manager.run_job(job_id).await
    }

    /// Run dead code analysis
    pub async fn analyze_dead_code(
        &self,
        target_path: impl Into<String>,
    ) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::DeadCode, target_path, Severity::Low)
            .await
    }

    /// Run complexity analysis
    pub async fn analyze_complexity(
        &self,
        target_path: impl Into<String>,
    ) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::Complexity, target_path, Severity::Low)
            .await
    }

    /// Run clone detection
    pub async fn detect_clones(&self, target_path: impl Into<String>) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::CloneDetection, target_path, Severity::Low)
            .await
    }

    /// Run cross-reference analysis
    pub async fn analyze_cross_references(
        &self,
        target_path: impl Into<String>,
    ) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::CrossReference, target_path, Severity::Low)
            .await
    }

    /// Run call graph analysis
    pub async fn analyze_call_graph(
        &self,
        target_path: impl Into<String>,
    ) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::CallGraph, target_path, Severity::Low)
            .await
    }

    /// Run all analyses
    pub async fn analyze_all(&self, target_path: impl Into<String>) -> Result<AnalysisResults> {
        self.analyze(AnalysisType::All, target_path, Severity::Low)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_results() {
        let mut results = AnalysisResults::new();

        let finding1 = AnalysisFinding::new(
            AnalysisType::DeadCode,
            Severity::High,
            "Finding 1",
            "Description 1",
        );

        let finding2 = AnalysisFinding::new(
            AnalysisType::Complexity,
            Severity::Low,
            "Finding 2",
            "Description 2",
        );

        results.add_finding(finding1);
        results.add_finding(finding2);

        assert_eq!(results.summary.total_findings, 2);
        assert_eq!(results.summary.high_count, 1);
        assert_eq!(results.summary.low_count, 1);

        let filtered = results.filtered_by_severity(Severity::Medium);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn into_filtered_preserves_entities_analyzed() {
        // `entities_analyzed` is the analysis scope, not a finding count, so it
        // survives severity filtering: the MCP `analyze_complexity` tool reports it
        // as `total_functions_analyzed` when nothing exceeds the threshold too.
        let mut results = AnalysisResults::new();
        results.summary.entities_analyzed = 11;
        results.summary.average_complexity = 2.5;
        results.add_finding(AnalysisFinding::new(
            AnalysisType::Complexity,
            Severity::Low,
            "Low finding",
            "dropped by a High filter",
        ));

        let filtered = results.into_filtered(Severity::High);

        assert_eq!(
            filtered.summary.entities_analyzed, 11,
            "entities_analyzed must be preserved across filtering"
        );
        assert!(
            (filtered.summary.average_complexity - 2.5).abs() < f64::EPSILON,
            "average_complexity (scope metadata) must survive filtering, else the \
             MCP analyze_complexity tool reports 0.0 for healthy codebases"
        );
        assert_eq!(
            filtered.summary.total_findings, 0,
            "the low-severity finding should have been filtered out"
        );
    }
}
