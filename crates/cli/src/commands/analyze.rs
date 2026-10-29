//! Analyze command implementation

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use devscriptor_core::analysis::{AnalysisApi, Severity};
use devscriptor_core::database::Database;
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::{AnalysisKind, OutputFormat, validate_path};

/// Run analyze command
pub async fn execute(
    path: PathBuf,
    database: Arc<Database>,
    kind: AnalysisKind,
    output: OutputFormat,
    severity: Option<devscriptor_core::analysis::Severity>,
) -> Result<()> {
    // Validate analysis target path (must exist and be a directory)
    let validated = validate_path(&path, &PathValidationOptions::must_be_directory())?;
    let api = AnalysisApi::new(database);

    let path_str = validated.to_string_lossy().to_string();
    let min_severity = severity.unwrap_or(Severity::Low);

    // Progress, not output: `-o json` must leave stdout pure JSON.
    eprintln!("Analyzing: {}", path.display());

    let results = match kind {
        AnalysisKind::DeadCode => api.analyze_dead_code(&path_str).await?,
        AnalysisKind::Complexity => api.analyze_complexity(&path_str).await?,
        AnalysisKind::Clones => {
            api.analyze(
                devscriptor_core::analysis::api::AnalysisType::CloneDetection,
                &path_str,
                min_severity,
            )
            .await?
        }
        AnalysisKind::CrossReference => api.analyze_cross_references(&path_str).await?,
        AnalysisKind::CallGraph => api.analyze_call_graph(&path_str).await?,
        AnalysisKind::CodeSmells => {
            api.analyze(
                devscriptor_core::analysis::api::AnalysisType::CodeSmells,
                &path_str,
                min_severity,
            )
            .await?
        }
        AnalysisKind::CrossFile => {
            api.analyze(
                devscriptor_core::analysis::api::AnalysisType::CrossFile,
                &path_str,
                min_severity,
            )
            .await?
        }
        AnalysisKind::All => api.analyze_all(&path_str).await?,
    };

    match output {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&results)?;
            println!("{json}");
        }
        OutputFormat::Text | OutputFormat::Markdown => {
            println!(
                "Analysis complete: {} findings, {} entities analyzed",
                results.summary.total_findings, results.summary.entities_analyzed
            );
            for finding in &results.findings {
                if finding.severity >= min_severity {
                    let location = finding
                        .file_path
                        .as_ref()
                        .map(|p| format!("{p}:{} ", finding.line_number.unwrap_or(0)))
                        .unwrap_or_default();
                    println!("  [{}] {}{}", finding.severity, location, finding.title);
                }
            }
        }
    }

    Ok(())
}
