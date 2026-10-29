//! MCP Tool implementations for Devscriptor
//!
//! Provides comprehensive tools for code analysis, search, and context management.

use serde::{Deserialize, Serialize};

// ============================================================================
// Job Management Types
// ============================================================================

/// Analysis job information (wire-format mirror of `server::AnalysisJob::to_dict`).
///
/// Use [`server::AnalysisJob`](crate::server::AnalysisJob) when you need the
/// in-memory representation with the [`JobStatus`](crate::server::JobStatus)
/// enum; use this type only for JSON de/serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisJobWire {
    pub job_id: String,
    pub path: String,
    pub status: String,
    pub progress: f64,
    pub current_file: String,
    pub files_processed: usize,
    pub total_files: usize,
    pub message: String,
    pub error: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub elapsed_seconds: u64,
}

/// Request to start analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartAnalysisRequest {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_patterns: Option<Vec<String>>,
}

/// Response from starting analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartAnalysisResponse {
    pub job_id: String,
    pub status: String,
    pub message: String,
}

/// Request to get job progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetJobProgressRequest {
    pub job_id: String,
    #[serde(default)]
    pub wait: bool,
    #[serde(default = "default_timeout")]
    pub timeout: u32,
}

fn default_timeout() -> u32 {
    120
}

/// Response from cancelling an analysis job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelAnalysisResponse {
    pub job_id: String,
    pub cancelled: bool,
    pub message: String,
}

/// List jobs response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListJobsResponse {
    pub total_jobs: usize,
    pub jobs: Vec<AnalysisJobWire>,
}

/// Cleanup jobs response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupJobsResponse {
    pub removed_jobs: usize,
    pub message: String,
}

// ============================================================================
// Search Types
// ============================================================================

/// Request to search functions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFunctionsRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default = "default_limit_20")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// Request to search classes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchClassesRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default = "default_limit_20")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

fn default_limit_20() -> usize {
    20
}

/// Function information response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docstring: Option<String>,
}

/// Class information response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassInfo {
    pub name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub language: String,
    pub method_count: usize,
    pub methods: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docstring: Option<String>,
}

/// Search response for functions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFunctionsResponse {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub limit: usize,
    pub functions: Vec<FunctionInfo>,
}

/// Search response for classes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchClassesResponse {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub limit: usize,
    pub classes: Vec<ClassInfo>,
}

/// Request for fuzzy search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzySearchRequest {
    pub query: String,
    #[serde(default = "default_limit_10")]
    pub limit: usize,
}

fn default_limit_10() -> usize {
    10
}

/// Fuzzy search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzySearchResult {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub file_path: String,
    /// First line of the entity (1-indexed).
    pub start_line: u32,
    /// Last line of the entity (1-indexed).
    pub end_line: u32,
    pub match_score: f64,
}

/// Fuzzy search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzySearchResponse {
    pub query: String,
    pub results: Vec<FuzzySearchResult>,
    pub total: usize,
}

/// Request for semantic search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchRequest {
    pub query: String,
    #[serde(default = "default_function")]
    pub entity_type: String,
    #[serde(default = "default_limit_10")]
    pub limit: usize,
}

fn default_function() -> String {
    "function".to_string()
}

/// Semantic search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResult {
    pub name: String,
    pub file_path: String,
    /// First line of the entity (1-indexed). With `file_path` it locates the
    /// match without reading the file: results without lines left agents
    /// counting lines by hand to cite `file:line`, and miscounting.
    pub start_line: u32,
    /// Last line of the entity (1-indexed).
    pub end_line: u32,
    pub entity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub similarity_score: f64,
    /// The entity's first lines, each as `<line><TAB><text>`: its line in the
    /// file, so a line inside the preview is cited without counting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
    /// `true` when results came from a keyword fallback because the embedding
    /// service was unavailable.
    #[serde(default)]
    pub fallback: bool,
}

/// Semantic search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticSearchResponse {
    pub query: String,
    pub entity_type: String,
    pub results: Vec<SemanticSearchResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Whether the embedding service was available for this query.
    /// `false` indicates a fallback to keyword search or no results due to
    /// service unavailability rather than genuine absence of matches.
    #[serde(default = "default_true")]
    pub service_available: bool,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Relationship Types
// ============================================================================

/// Relationship information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipInfo {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    /// The file the source is defined in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// The line the source starts on, counted from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<u32>,
    /// The file the target is defined in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_file: Option<String>,
    /// The line the target starts on, counted from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_line: Option<u32>,
    /// For a call, a use or a supertype, the lines of the source's file it is
    /// made on, counted from 1. Absent for an edge the file's structure gives
    /// (`has_method`).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub lines: Vec<u32>,
    /// The edge is one of several equally good resolutions of a name (for
    /// example a call to overloads), not a certain link. Absent when false.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub ambiguous: bool,
}

/// Relationships response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRelationshipsResponse {
    pub entity: String,
    pub entity_type: String,
    pub total_relationships: usize,
    pub relationships: Vec<RelationshipInfo>,
}

// ============================================================================
// Analysis Types
// ============================================================================

/// Dead code item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeItem {
    pub name: String,
    pub file_path: String,
    pub line: u32,
    /// Confidence in `[0.0, 1.0]` that the entity is genuinely dead.
    /// Public symbols score lower because they may be used outside the
    /// codebase, and those whose name is written elsewhere lower still.
    pub confidence: f64,
    /// Why the entity was flagged (e.g. "No incoming calls or references").
    pub reason: String,
}

/// Dead code response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindDeadCodeResponse {
    /// The functions and classes in scope, all of them: none means nothing
    /// under the path is indexed, not that none of it is dead.
    pub total_entities_analyzed: usize,
    /// The surest findings first, at most `limit` of them with the classes.
    pub unused_functions: Vec<DeadCodeItem>,
    pub unused_classes: Vec<DeadCodeItem>,
    pub total_unused_functions: usize,
    pub total_unused_classes: usize,
}

/// Complexity result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityResult {
    pub name: String,
    pub file_path: String,
    pub line: u32,
    pub complexity_score: u32,
    pub risk_level: String,
}

/// Complexity analysis response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeComplexityResponse {
    /// The functions in scope, all of them.
    pub total_functions_analyzed: usize,
    /// Their mean cyclomatic complexity.
    pub average_complexity: f64,
    /// The cyclomatic complexity above which a function is complex: `medium`
    /// risk above it, `high` above twice it.
    pub threshold: u32,
    /// How many functions in scope are above the threshold.
    pub functions_above_threshold: usize,
    /// The `top_n` most complex functions in scope, whatever their score.
    pub most_complex_functions: Vec<ComplexityResult>,
}

// ============================================================================
// Context Graph Types
// ============================================================================

/// Context search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextNoteResult {
    pub id: i64,
    pub note_type: String,
    pub title: String,
    pub content: String,
    pub priority: u32,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f64>,
}

/// Context search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchContextResponse {
    pub query: String,
    pub results_count: usize,
    pub results: Vec<ContextNoteResult>,
}

/// Prune context response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneContextResponse {
    pub archived_count: usize,
    pub deleted_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub would_archive: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub would_delete: Option<usize>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statistics: Option<PruneStatistics>,
}

/// Statistics for pruning operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneStatistics {
    pub total_notes: usize,
    pub active_notes: usize,
    pub archived_notes: usize,
    pub superseded_notes: usize,
    pub old_active_notes: usize,
    pub old_archived_notes: usize,
    pub max_age_days: u32,
}

// ============================================================================
// Original Core Response Types (for backwards compatibility)
// ============================================================================

/// Response from scanning a codebase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCodebaseResponse {
    /// Absolute, canonicalized path that was actually scanned. Surfaced because a
    /// relative `path` (e.g. `"."`) resolves against the MCP server's working
    /// directory — which is not always the caller's — making it explicit what
    /// was indexed.
    pub scanned_path: String,
    pub files_scanned: usize,
    pub entities_found: usize,
    pub duration_seconds: f64,
}

/// Information about a code entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityInfo {
    pub name: String,
    pub entity_type: String,
    pub file_path: String,
    pub line_number: u32,
}

/// Response from searching entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEntitiesResponse {
    pub entities: Vec<EntityInfo>,
    pub total_count: usize,
}

/// Analysis finding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisFinding {
    pub title: String,
    pub description: String,
    pub severity: String,
    /// The file it is in, when it is in one: a summary of the whole index is
    /// in none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// The line it is on, counted from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// What the analysis knows of it besides: a clone's other place, a
    /// cycle's functions and files, a smell's suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Response from code analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeCodeResponse {
    pub analysis_type: String,
    pub findings: Vec<AnalysisFinding>,
    pub total_findings: usize,
}

/// Response with codebase statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatisticsResponse {
    /// Path the statistics were scoped to, when `get_statistics` received a
    /// `path` argument. Absent means database-wide totals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub total_files: usize,
    pub total_entities: usize,
    pub total_relationships: usize,
}

// ============================================================================
// File Description Types
// ============================================================================

/// File description response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeFileResponse {
    pub file_path: String,
    pub language: String,
    /// Lines in the file; absent when it could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<usize>,
    pub entity_count: usize,
    pub entities: Vec<EntitySummary>,
    /// The file's start, each line as `<line><TAB><text>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
}

/// Entity summary for file description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySummary {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub line: u32,
}

/// Modification description response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeModificationResponse {
    pub file_path: String,
    pub changes_summary: String,
    pub entities_added: Vec<String>,
    pub entities_removed: Vec<String>,
    pub entities_modified: Vec<String>,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// File context response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileContextResponse {
    pub file_path: String,
    pub imports: Vec<String>,
    pub dependencies: Vec<String>,
    pub related_files: Vec<String>,
    pub context_summary: String,
    /// Source excerpt around each entity, `context_lines` lines on either side
    /// of its definition. Empty when the file could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entity_contexts: Vec<EntityContext>,
}

/// A source excerpt surrounding one entity definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityContext {
    pub name: String,
    pub entity_type: String,
    /// 1-indexed line of the entity definition.
    pub line: u32,
    /// 1-indexed line of the first line in `snippet`.
    pub snippet_start_line: u32,
    /// The excerpt, each line as `<line><TAB><text>`.
    pub snippet: String,
}
