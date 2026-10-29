//! `devscriptor validate`: check a project configuration file.

use std::path::PathBuf;

use anyhow::Result;
use devscriptor_core::config::{PROJECT_CONFIG_FILE, ProjectConfig};
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::validate_path;

/// Check the project configuration `file`: by default the one `--config`
/// names (`config`), else `devscriptor.yaml` in the working directory. Its
/// errors (a field the schema does not name, a value that cannot apply) fail
/// the command; its warnings are listed.
///
/// # Errors
/// Returns an error if the file cannot be read or parsed, or holds settings
/// that cannot be applied.
pub fn execute(file: Option<PathBuf>, config: Option<PathBuf>) -> Result<()> {
    let path = file
        .or(config)
        .unwrap_or_else(|| PathBuf::from(PROJECT_CONFIG_FILE));
    let validated = validate_path(&path, &PathValidationOptions::must_be_file())?;
    println!("🔍 Validating configuration: {}", validated.display());

    let config = match ProjectConfig::load(&validated) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("❌ Configuration is invalid: {}", e.message);
            anyhow::bail!("Configuration validation failed: {e}");
        }
    };

    let findings = config.check();
    if findings.errors.is_empty() {
        println!("✅ Configuration is valid");
        if !findings.warnings.is_empty() {
            println!("\n⚠ Warnings:");
            for warn in &findings.warnings {
                println!("   • {warn}");
            }
        }
        Ok(())
    } else {
        eprintln!("❌ Configuration validation failed:");
        for err in &findings.errors {
            eprintln!("   • {err}");
        }
        for warn in &findings.warnings {
            eprintln!("   ⚠ {warn}");
        }
        Err(anyhow::anyhow!(
            "Configuration has {} error(s) and {} warning(s)",
            findings.errors.len(),
            findings.warnings.len()
        ))
    }
}
