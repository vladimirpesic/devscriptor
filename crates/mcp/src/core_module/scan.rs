//! Core tool implementations: scan, search, analyze, stats

use serde_json::Value;
use tracing::{info, warn};

use super::{CoreModule, ToolError, ToolResult, ok_json};
use crate::tools::{
    AnalysisFinding, AnalyzeCodeResponse, EntityInfo, ScanCodebaseResponse, SearchEntitiesResponse,
    StatisticsResponse,
};

impl CoreModule {
    pub(super) async fn scan_codebase(
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

        // `extensions` is a whitelist of file extensions (with or without a
        // leading dot); `exclude_patterns` are globs matched against file paths.
        // Both empty = scan every supported language, excluding the defaults.
        let extensions = super::string_array_arg(&args, "extensions");
        let exclude_patterns = super::string_array_arg(&args, "exclude_patterns");

        let start = std::time::Instant::now();
        info!(
            "Scanning codebase at: {} (extensions: {:?}, exclude: {:?})",
            path, extensions, exclude_patterns
        );

        // The full CodebasePipeline, so entities, relationships, checksums and
        // languages are all written to the database.
        let db = self.state.database();
        let project = self.state.project_config();
        match super::run_pipeline(db, &project, &path_buf, extensions, exclude_patterns).await {
            Ok(result) => {
                // The scan may have indexed the workspace the server watches
                // by itself: start that watch now, after this answer.
                let state = std::sync::Arc::clone(&self.state);
                tokio::spawn(async move {
                    state.watch_after_scan().await;
                });
                let response = ScanCodebaseResponse {
                    scanned_path: path_buf.display().to_string(),
                    files_scanned: result.files_parsed,
                    entities_found: result.entity_count,
                    duration_seconds: start.elapsed().as_secs_f64(),
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Scan failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn search_entities(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'query' argument".into()))?;

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        let entity_type = args.get("entity_type").and_then(|v| v.as_str());

        info!("Searching entities: {} (type: {:?})", query, entity_type);

        // When `entity_type` is given the filter is pushed into SQL through the
        // searcher; without it the FTS-backed `Database::search_entities` path
        // (better ranking) is used unchanged.
        let entities = match entity_type {
            Some(entity_type) => {
                let options = devscriptor_core::search::SearchOptions {
                    fuzzy: true,
                    entity_type: Some(entity_type.to_string()),
                    limit: Some(limit),
                    ..Default::default()
                };
                match self.state.searcher().search_entities(query, options).await {
                    Ok(results) => results.into_iter().map(|r| r.entity).collect(),
                    Err(e) => {
                        warn!("Search entities failed: {}", e);
                        return Err(ToolError::from(e));
                    }
                }
            }
            None => match self.state.database().search_entities(query, limit).await {
                Ok(entities) => entities,
                Err(e) => {
                    warn!("Search entities failed: {}", e);
                    return Err(ToolError::from(e));
                }
            },
        };

        let results: Vec<EntityInfo> = entities
            .into_iter()
            .map(|e| EntityInfo {
                name: e.name().to_string(),
                entity_type: e.entity_type().to_string(),
                file_path: e.file_path().to_string(),
                line_number: e.location().start_line,
            })
            .collect();

        let total = results.len();
        let response = SearchEntitiesResponse {
            entities: results,
            total_count: total,
        };

        ok_json(&response)
    }

    pub(super) async fn analyze_code(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        // `path` defaults to `.`, the whole index, as `find_dead_code`'s and
        // `analyze_complexity`'s do: the schema does not declare it required.
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let analysis_type = args
            .get("analysis_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'analysis_type' argument".into()))?;

        // `.` is the whole index; another path keeps the findings under it.
        let target = Self::scope_directory(path)?
            .map_or_else(|| ".".to_string(), |dir| dir.to_string_lossy().into_owned());
        info!("Analyzing code at: {} (type: {})", path, analysis_type);

        let analysis_type_enum = match analysis_type {
            "dead_code" => devscriptor_core::analysis::AnalysisType::DeadCode,
            "complexity" => devscriptor_core::analysis::AnalysisType::Complexity,
            "cross_reference" => devscriptor_core::analysis::AnalysisType::CrossReference,
            "call_graph" => devscriptor_core::analysis::AnalysisType::CallGraph,
            "code_smells" => devscriptor_core::analysis::AnalysisType::CodeSmells,
            "cross_file" => devscriptor_core::analysis::AnalysisType::CrossFile,
            "clones" => devscriptor_core::analysis::AnalysisType::CloneDetection,
            "all" => devscriptor_core::analysis::AnalysisType::All,
            _ => {
                return Err(ToolError::InvalidParams(format!(
                    "Invalid analysis_type: '{analysis_type}'. Valid values: dead_code, \
                     complexity, cross_reference, call_graph, code_smells, cross_file, clones, all"
                )));
            }
        };

        match self
            .state
            .analysis_api()
            .analyze(
                analysis_type_enum,
                target,
                devscriptor_core::analysis::Severity::Low,
            )
            .await
        {
            Ok(results) => {
                let findings: Vec<AnalysisFinding> = results
                    .findings
                    .iter()
                    .map(|f| AnalysisFinding {
                        title: f.title.clone(),
                        description: f.description.clone(),
                        severity: f.severity.to_string(),
                        file_path: f.file_path.clone(),
                        line: f.line_number,
                        details: f.metadata.clone(),
                    })
                    .collect();

                let response = AnalyzeCodeResponse {
                    analysis_type: analysis_type.to_string(),
                    findings,
                    total_findings: results.summary.total_findings,
                };

                ok_json(&response)
            }
            Err(e) => {
                warn!("Code analysis failed: {}", e);
                Err(ToolError::from(e))
            }
        }
    }

    pub(super) async fn get_statistics(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.unwrap_or_default();
        // Optional: scope the counts to entities whose file path contains `path`.
        let path = args.get("path").and_then(|v| v.as_str());

        info!("Getting statistics (path: {:?})", path);

        let db = self.state.database();
        let stats = match path {
            Some(path) => db.get_stats_for_path(path).await,
            None => db.get_stats().await,
        };
        let (total_files, total_entities, total_relationships) = match stats {
            Ok((files, entities, rels)) => (files, entities, rels),
            Err(e) => {
                warn!("Failed to get stats: {}", e);
                return Err(ToolError::from(e));
            }
        };

        let response = StatisticsResponse {
            path: path.map(ToString::to_string),
            total_files,
            total_entities,
            total_relationships,
        };

        ok_json(&response)
    }
}
