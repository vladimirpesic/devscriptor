//! Common types and utilities for CLI commands

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devscriptor_core::config::{LocatedConfig, ProjectConfig};
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::utils::validation::{PathValidationOptions, validate_file_path};

/// Validate a user-provided path, converting core errors to [`anyhow::Error`].
///
/// The one path-validation entry point for every CLI command that accepts a
/// filesystem path from the user.
pub fn validate_path<P: AsRef<Path>>(
    path: P,
    options: &PathValidationOptions,
) -> anyhow::Result<PathBuf> {
    validate_file_path(path, None, options).map_err(anyhow::Error::from)
}

/// The project configuration a command applies: the file `--config` names
/// (`explicit`), else `devscriptor.yaml` in `dir`, if there is one. Its
/// warnings go to stderr, which carries no command's output.
///
/// # Errors
/// Returns an error if the file cannot be read or parsed, or if its settings
/// cannot be applied.
pub fn find_project_config(
    explicit: Option<&Path>,
    dir: &Path,
) -> anyhow::Result<Option<LocatedConfig>> {
    let located = ProjectConfig::find(explicit, dir)?;
    if let Some(located) = &located {
        for warning in located.config.check().warnings {
            eprintln!("⚠️  {}: {warning}", located.path.display());
        }
    }
    Ok(located)
}

/// The project configuration of a command that works in the working
/// directory ([`find_project_config`]), or the defaults when there is none.
///
/// # Errors
/// As [`find_project_config`], or if there is no working directory.
pub fn working_directory_config(explicit: Option<&Path>) -> anyhow::Result<ProjectConfig> {
    let dir = std::env::current_dir()?;
    Ok(find_project_config(explicit, &dir)?
        .map(|located| located.config)
        .unwrap_or_default())
}

/// The configuration of the database at `path`, with the project's pool size.
#[must_use]
pub fn database_config(path: &Path, project: &ProjectConfig) -> DatabaseConfig {
    DatabaseConfig {
        path: path.to_string_lossy().to_string(),
        pool_size: project.database.pool_size,
        ..DatabaseConfig::default()
    }
}

/// Open a database at the given path, returning a shared [`Arc<Database>`].
///
/// One CLI invocation opens one database, so its commands share one
/// connection pool.
pub fn open_database(path: &Path, project: &ProjectConfig) -> anyhow::Result<Arc<Database>> {
    Ok(Arc::new(Database::new(database_config(path, project))?))
}

/// Type of analysis to perform
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AnalysisKind {
    /// Dead code analysis
    DeadCode,
    /// Complexity analysis
    Complexity,
    /// Clone detection
    Clones,
    /// Cross-reference analysis
    CrossReference,
    /// Call graph analysis
    CallGraph,
    /// Code smells detection
    CodeSmells,
    /// Cross-file analysis
    CrossFile,
    /// Run all analyses
    All,
}

/// Output format for reports
#[derive(Debug, Clone, Copy, clap::ValueEnum, Default)]
pub enum OutputFormat {
    /// Plain text output
    #[default]
    Text,
    /// JSON output
    Json,
    /// Markdown output
    Markdown,
}

/// Type of search to perform
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SearchType {
    /// Search for functions
    Function,
    /// Search for classes
    Class,
    /// Search for any symbol
    Symbol,
    /// Semantic search
    Semantic,
}

/// Format for call graph output
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum CallGraphFormat {
    /// Text format
    Text,
    /// JSON format
    Json,
    /// DOT format for graphviz
    Dot,
    /// Mermaid format
    Mermaid,
}

/// Format for architecture report
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ReportFormat {
    /// Text format
    Text,
    /// JSON format
    Json,
    /// Markdown format
    Markdown,
}

/// Print the application banner on stderr, when stderr is a terminal.
///
/// stdout carries command output: `stats`, `analyze -o json`, `call-graph -f
/// json`, `definition` and `references` print JSON there, which a banner ahead
/// of it would make unparseable for scripts and the VS Code extension.
pub fn print_banner() {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        eprintln!("{}", banner(env!("CARGO_PKG_VERSION")));
    }
}

/// The banner text, 61 columns wide.
fn banner(version: &str) -> String {
    format!(
        "
╔═══════════════════════════════════════════════════════════╗
║                    DEVSCRIPTOR v{version: <26}║
║           Advanced Code Analysis Platform                 ║
╚═══════════════════════════════════════════════════════════╝
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every line of the box is the same width, whatever the version string.
    #[test]
    fn the_banner_box_is_aligned() {
        for version in ["1.0.0", "10.20.30-beta.1"] {
            let text = banner(version);
            let widths: Vec<usize> = text
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.chars().count())
                .collect();
            assert_eq!(widths, vec![61; 4], "{text}");
        }
    }
}
