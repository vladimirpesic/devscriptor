//! Architecture report command implementation

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use devscriptor_core::database::Database;
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::{ReportFormat, validate_path};

/// Generate architecture report
pub async fn execute(
    path: PathBuf,
    database: Option<Arc<Database>>,
    format: ReportFormat,
    output: Option<PathBuf>,
) -> Result<()> {
    // Validate target path (must exist and be a directory)
    let validated_path = validate_path(&path, &PathValidationOptions::must_be_directory())?;

    // Validate output path if provided (check for dangerous patterns only)
    if let Some(ref out) = output {
        let _ = validate_path(out, &PathValidationOptions::default())?;
    }

    // Collect files from the codebase
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&validated_path)
        .follow_links(false)
        .max_depth(5)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_string();
        files.push((entry.path().to_string_lossy().to_string(), ext));
    }

    // Group by extension
    let mut by_extension: HashMap<String, Vec<String>> = HashMap::new();
    for (file_path, ext) in files {
        by_extension.entry(ext).or_default().push(file_path);
    }

    // Enrich with database statistics when available
    let db_stats = if let Some(ref db) = database {
        db.get_stats().await.ok()
    } else {
        None
    };

    let total_files = by_extension.values().map(std::vec::Vec::len).sum::<usize>();

    let output_content = match format {
        ReportFormat::Text => {
            let mut lines = vec![
                format!("Architecture Report for: {}", validated_path.display()),
                String::new(),
                format!("Total files: {total_files}"),
                String::new(),
                "Files by extension:".to_string(),
            ];
            let mut exts: Vec<_> = by_extension.keys().collect();
            exts.sort();
            for ext in exts {
                lines.push(format!("  .{ext}: {} files", by_extension[ext].len()));
            }

            lines.push(String::new());
            lines.push("Database Statistics:".to_string());
            if let Some((file_count, entity_count, relationship_count)) = db_stats {
                lines.push(format!("  Files indexed: {file_count}"));
                lines.push(format!("  Entities: {entity_count}"));
                lines.push(format!("  Relationships: {relationship_count}"));
            } else {
                lines.push("  (not available — run `devscriptor scan` first)".to_string());
            }

            lines.join("\n")
        }
        ReportFormat::Json => {
            let ext_counts: HashMap<String, usize> = by_extension
                .iter()
                .map(|(k, v)| (k.clone(), v.len()))
                .collect();
            let mut report = serde_json::json!({
                "project": validated_path.display().to_string(),
                "total_files": total_files,
                "extensions": ext_counts
            });
            if let Some((file_count, entity_count, relationship_count)) = db_stats {
                report["database"] = serde_json::json!({
                    "files_indexed": file_count,
                    "entities": entity_count,
                    "relationships": relationship_count
                });
            }
            report.to_string()
        }
        ReportFormat::Markdown => {
            let mut lines = vec![
                format!(
                    "# Architecture Report\n\n**Project:** {}\n\n## Overview\n\n- **Total files:** {total_files}",
                    validated_path.display()
                ),
                String::new(),
                "## Files by Extension".to_string(),
                String::new(),
                "| Extension | Count |".to_string(),
                "|-----------|-------|".to_string(),
            ];
            let mut exts: Vec<_> = by_extension.keys().collect();
            exts.sort();
            for ext in exts {
                lines.push(format!("| .{ext} | {} |", by_extension[ext].len()));
            }

            lines.push(String::new());
            lines.push("## Database Statistics".to_string());
            lines.push(String::new());
            if let Some((file_count, entity_count, relationship_count)) = db_stats {
                lines.push(format!("- **Files indexed:** {file_count}"));
                lines.push(format!("- **Entities:** {entity_count}"));
                lines.push(format!("- **Relationships:** {relationship_count}"));
            } else {
                lines.push("*(not available — run `devscriptor scan` first)*".to_string());
            }

            lines.join("\n")
        }
    };

    // Write to file or stdout
    if let Some(output_path) = output {
        tokio::fs::write(&output_path, output_content)
            .await
            .with_context(|| format!("Failed to write to {}", output_path.display()))?;
        println!("✅ Report written to {}", output_path.display());
    } else {
        println!("{output_content}");
    }

    Ok(())
}
