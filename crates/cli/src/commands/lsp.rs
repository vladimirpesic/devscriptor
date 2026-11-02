//! LSP (Language Server Protocol) commands for Devscriptor CLI
//!
//! Implements automated LSP server detection and setup:
//! 1. Detect languages in project
//! 2. Check for existing LSP servers (VS Code, system, bundled)
//! 3. Check prerequisites for missing servers
//! 4. Prompt user for installation
//! 5. Install servers or guide user through setup

use anyhow::{Context, Result};
use console::{Term, style};
use devscriptor_core::utils::validation::PathValidationOptions;
use devscriptor_lsp::lsp_architecture::prerequisites::{InstallDifficulty, get_install_difficulty};
use devscriptor_lsp::lsp_architecture::{
    DetectedLanguage, LspArchitectureManager, LspArchitectureResult, LspSource,
    PrerequisitesResult, batch_check, check_prerequisites, quick_check, server_id_for,
};
// `LspManager::install` downloads, verifies and installs a server.
use devscriptor_lsp::{InstallationResult as LspInstallResult, LspManager};
use dialoguer::Confirm;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use crate::commands::common::validate_path;

/// LSP command handler
pub struct LspCommand {
    manager: LspArchitectureManager,
    term: Term,
}

impl LspCommand {
    /// Create a new LSP command handler
    pub fn new() -> Result<Self> {
        let manager = LspArchitectureManager::new()
            .context("Failed to initialize LSP architecture manager")?;

        Ok(Self {
            manager,
            term: Term::stdout(),
        })
    }

    /// `lsp setup` of `targets`: each a project, whose languages' servers are
    /// set up, or a server, by id or language, which is installed whether a
    /// project calls for it or not. A name that is also a directory here is
    /// the directory; none is the current one. Every target is tried, and the
    /// command fails where one did.
    pub async fn setup_targets(&mut self, targets: &[String], auto_install: bool) -> Result<()> {
        let here = [".".to_string()];
        let targets = if targets.is_empty() {
            &here[..]
        } else {
            targets
        };
        let mut failed = Vec::new();
        for target in targets {
            let path = Path::new(target);
            let result = if path.is_dir() {
                self.setup(path, auto_install).await
            } else if let Some(server_id) = server_id_for(target) {
                self.install_named(server_id).await
            } else {
                Err(anyhow::anyhow!(
                    "{target} is neither a directory nor a language server; the servers are {}",
                    devscriptor_lsp::generated_registry::SERVER_IDS.join(", ")
                ))
            };
            if let Err(e) = result {
                if targets.len() == 1 {
                    return Err(e);
                }
                self.term
                    .write_line(&style(format!("{target}: {e:#}")).red().to_string())?;
                failed.push(target.as_str());
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{} of {} failed: {}",
                failed.len(),
                targets.len(),
                failed.join(", ")
            ))
        }
    }

    /// Install `server_id`, named on the command line, as `lsp setup` installs
    /// a project's: its prerequisites checked, into Devscriptor's own servers,
    /// whatever else is on PATH. A server that comes with its toolchain
    /// (sourcekit-lsp) is not installed: the toolchain's own is run.
    async fn install_named(&mut self, server_id: &str) -> Result<()> {
        self.print_header(&format!("📦 {server_id}"));
        let comes_with_its_toolchain = devscriptor_lsp::manager::ServerRegistry::load()?
            .get_server(server_id)
            .is_some_and(|s| s.source.source_type == devscriptor_lsp::manager::SourceType::System);
        if comes_with_its_toolchain {
            self.term.write_line(&format!(
                "  {server_id} comes with its toolchain, and the toolchain's own is run: it is not installed"
            ))?;
            return Ok(());
        }
        self.install_server(server_id).await
    }

    /// Run the setup workflow for a project
    pub async fn setup(&mut self, project_path: &Path, auto_install: bool) -> Result<()> {
        // Validate project path (must exist and be a directory)
        let _ = validate_path(project_path, &PathValidationOptions::must_be_directory())?;
        self.print_header("🔧 LSP Setup");
        self.term.write_line(&format!(
            "Project: {}",
            style(project_path.display()).cyan()
        ))?;
        self.term.write_line("")?;

        // Step 1: Analyze project and detect languages
        let spinner = self.create_spinner("Detecting project languages...");
        let result = tokio::task::block_in_place(|| self.manager.analyze_project(project_path));
        spinner.finish_and_clear();

        self.print_detected_languages(&result.languages);

        // Step 2: Show resolved servers
        if !result.resolved_servers.is_empty() {
            self.print_header("✅ Found LSP Servers");
            for (server_id, source, _config) in &result.resolved_servers {
                self.print_server_found(server_id, source);
            }
            self.term.write_line("")?;
        }

        // Step 3: Handle servers that can be auto-installed. One server that
        // fails to install keeps none of the others from installing; the
        // command fails once everything is reported.
        let mut failed = Vec::new();
        if !result.needs_install.is_empty() {
            self.print_header("📦 Ready to Install");

            for (server_id, _prereqs) in &result.needs_install {
                self.print_install_ready(server_id);
            }

            let should_install = if auto_install {
                true
            } else if std::io::stdin().is_terminal() {
                Confirm::new()
                    .with_prompt("Install these LSP servers?")
                    .default(true)
                    .interact()?
            } else {
                // Non-TTY environment: skip interactive prompt and log why
                tracing::info!(
                    "Skipping interactive LSP install prompt in non-TTY environment. \
                     Use --yes to install automatically."
                );
                false
            };

            if should_install {
                for (server_id, _prereqs) in &result.needs_install {
                    if let Err(e) = self.install_server(server_id).await {
                        self.term
                            .write_line(&style(format!("{server_id}: {e:#}")).red().to_string())?;
                        failed.push(server_id.as_str());
                    }
                }
            }
            self.term.write_line("")?;
        }

        // Step 4: Handle missing prerequisites
        if !result.missing_prerequisites.is_empty() {
            self.print_header("⚠️  Missing Prerequisites");

            for (server_id, prereqs) in &result.missing_prerequisites {
                self.print_missing_prerequisites(server_id, prereqs)?;
            }

            self.term.write_line(&style("Please install the missing prerequisites and run 'devscriptor lsp setup' again.").yellow().to_string())?;
        }

        // Summary
        self.print_summary(&result);

        if failed.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{} of {} servers failed to install: {}",
                failed.len(),
                result.needs_install.len(),
                failed.join(", ")
            ))
        }
    }

    /// Detect LSP servers for a project
    pub async fn detect(&mut self, project_path: &Path) -> Result<()> {
        // Validate project path (must exist and be a directory)
        let _ = validate_path(project_path, &PathValidationOptions::must_be_directory())?;
        self.print_header("🔍 LSP Detection");

        let spinner = self.create_spinner("Analyzing project...");
        let result = tokio::task::block_in_place(|| self.manager.analyze_project(project_path));
        spinner.finish_and_clear();

        // Languages detected
        if result.languages.is_empty() {
            self.term.write_line(
                &style("No languages detected in project.")
                    .yellow()
                    .to_string(),
            )?;
            return Ok(());
        }

        self.print_detected_languages(&result.languages);

        // Servers found
        if !result.resolved_servers.is_empty() {
            self.term.write_line("")?;
            self.print_header("Found LSP Servers");
            for (server_id, source, _config) in &result.resolved_servers {
                self.print_server_found(server_id, source);
            }
        }

        // Servers needing installation
        if !result.needs_install.is_empty() {
            self.term.write_line("")?;
            self.print_header("Servers Ready to Install");
            for (server_id, _prereqs) in &result.needs_install {
                self.term.write_line(&format!(
                    "  {} - prerequisites satisfied",
                    style(server_id).green()
                ))?;
            }
        }

        // Missing prerequisites
        if !result.missing_prerequisites.is_empty() {
            self.term.write_line("")?;
            self.print_header("Missing Prerequisites");
            for (server_id, prereqs) in &result.missing_prerequisites {
                self.term
                    .write_line(&format!("  {}", style(server_id).red()))?;
                for prereq in &prereqs.missing {
                    let name: &str = prereq.name();
                    self.term.write_line(&format!("    ❌ {name}"))?;
                }
            }
        }

        Ok(())
    }

    /// Show status of all available LSP servers
    pub fn status(&self) -> Result<()> {
        self.print_header("📊 LSP Status");

        // Check system-wide servers
        let spinner = self.create_spinner("Scanning system for LSP servers...");
        let available = self.manager.get_all_available_servers();
        spinner.finish_and_clear();

        self.term.write_line(&format!(
            "Found {} LSP servers:",
            style(available.len()).green()
        ))?;
        self.term.write_line("")?;

        // Group by source
        let mut by_source: HashMap<&str, Vec<&str>> = HashMap::new();
        for (server_id, source) in &available {
            let source_type: &str = source.source_type();
            by_source
                .entry(source_type)
                .or_default()
                .push(server_id.as_str());
        }

        for (source, servers) in by_source {
            self.term.write_line(&format!(
                "{} ({})",
                style(source).cyan().bold(),
                servers.len()
            ))?;
            for server in servers {
                self.term.write_line(&format!("  ✅ {server}"))?;
            }
            self.term.write_line("")?;
        }

        // The rest of the registry's servers, each with why it does not run.
        let unavailable: Vec<(&str, String)> = devscriptor_lsp::generated_registry::SERVER_IDS
            .iter()
            .filter(|id| !available.iter().any(|(found, _)| found == *id))
            .filter_map(|id| {
                devscriptor_lsp::manager::why_unavailable(id).map(|why| (*id, why.reason))
            })
            .collect();
        if !unavailable.is_empty() {
            self.term.write_line(&format!(
                "{} ({})",
                style("not available").yellow().bold(),
                unavailable.len()
            ))?;
            for (server, reason) in unavailable {
                self.term.write_line(&format!("  ❌ {server} {reason}"))?;
            }
            self.term.write_line("")?;
        }

        Ok(())
    }

    /// Quick check if a specific server is available
    pub fn check(&self, server_id: &str) -> Result<()> {
        self.print_header(&format!("🔍 Checking: {server_id}"));

        if let Some(ref source) = quick_check(server_id) {
            let path: &std::path::Path = source.path();
            let source_type: &str = source.source_type();
            self.term.write_line(&format!(
                "{} found at: {}",
                style("✅").green(),
                style(path.display()).cyan()
            ))?;
            self.term
                .write_line(&format!("Source: {}", style(source_type).yellow()))?;
        } else {
            self.term
                .write_line(&format!("{} {} not found", style("❌").red(), server_id))?;

            // Why, as its install shows, and what makes it run.
            if let Some(why) = devscriptor_lsp::manager::why_unavailable(server_id) {
                self.term
                    .write_line(&format!("{server_id} {}", why.reason))?;
            }

            // Check prerequisites
            let prereqs = check_prerequisites(server_id);
            if !prereqs.is_satisfied() {
                self.term.write_line("")?;
                self.term.write_line("Missing prerequisites:")?;
                for prereq in &prereqs.missing {
                    let name: &str = prereq.name();
                    let instructions = prereq.install_instructions();
                    self.term
                        .write_line(&format!("  • {name}: {instructions}"))?;
                }
            }
        }

        Ok(())
    }

    /// Batch check multiple servers
    pub fn batch_check(&self, server_ids: &[String]) -> Result<()> {
        self.print_header("🔍 Batch Server Check");

        let spinner = self.create_spinner("Checking servers...");
        let results = batch_check(server_ids);
        spinner.finish_and_clear();

        for (server_id, source) in results {
            match source {
                Some(ref src) => {
                    let path: &std::path::Path = src.path();
                    let source_type: &str = src.source_type();
                    self.term.write_line(&format!(
                        "✅ {} - {} ({})",
                        style(&server_id).green(),
                        path.display(),
                        source_type
                    ))?;
                }
                None => {
                    let why = devscriptor_lsp::manager::why_unavailable(&server_id)
                        .map_or_else(|| "not found".to_string(), |why| why.reason);
                    self.term
                        .write_line(&format!("❌ {} - {why}", style(&server_id).red()))?;
                }
            }
        }

        Ok(())
    }

    /// Interactive server installation.
    ///
    /// Through `LspManager::install`, which downloads the server, and replaces an
    /// install of another build than the one the registry pins.
    async fn install_server(&mut self, server_id: &str) -> Result<()> {
        let spinner = self.create_spinner(&format!("Installing {server_id}..."));

        let lsp_manager =
            LspManager::new().map_err(|e| anyhow::anyhow!("Failed to create LspManager: {e}"))?;
        let result = lsp_manager.install(server_id, None).await;
        // A spinner shows nothing where stdout is no terminal: the outcome is
        // a line of its own, and an error is the caller's to report.
        spinner.finish_and_clear();

        let outcome = match result? {
            LspInstallResult::Success => format!("{server_id} installed"),
            LspInstallResult::AlreadyInstalled => format!("{server_id} is already installed"),
            LspInstallResult::MissingPrerequisite(req) => {
                return Err(anyhow::anyhow!("Missing prerequisite: {req}"));
            }
            LspInstallResult::Failed(msg) => {
                return Err(anyhow::anyhow!("Installation failed: {msg}"));
            }
            LspInstallResult::NotInstalled => {
                return Err(anyhow::anyhow!("Server was not installed"));
            }
        };
        self.term
            .write_line(&format!("  {} {outcome}", style("✓").green()))?;
        Ok(())
    }

    // Helper methods

    fn print_header(&self, text: &str) {
        let _ = self
            .term
            .write_line(&style(text).bold().underlined().to_string());
    }

    fn print_detected_languages(&self, languages: &[DetectedLanguage]) {
        let _ = self
            .term
            .write_line(&style("Languages Detected:").bold().to_string());
        for lang in languages {
            let confidence_str = format!("{:.0}%", lang.confidence * 100.0);
            let confidence_style = if lang.confidence > 0.8 {
                style(confidence_str).green()
            } else if lang.confidence > 0.5 {
                style(confidence_str).yellow()
            } else {
                style(confidence_str).red()
            };

            let _ = self.term.write_line(&format!(
                "  • {} (confidence: {})",
                style(&lang.language_id).cyan(),
                confidence_style
            ));

            if !lang.indicator_files.is_empty() {
                let _ = self.term.write_line(&format!(
                    "    Indicators: {}",
                    lang.indicator_files
                        .iter()
                        .map(|p: &std::path::PathBuf| p
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }

    fn print_server_found(&self, server_id: &str, source: &LspSource) {
        let source_icon = match source {
            LspSource::VsCodeExtension(_) => "📝",
            LspSource::SystemWide(_) => "🖥️ ",
            LspSource::Installed(_) => "⬇️ ",
            LspSource::Bundled(_) => "📦",
            LspSource::UserConfig { .. } => "⚙️ ",
        };

        let path: &std::path::Path = source.path();
        let source_type: &str = source.source_type();
        let _ = self.term.write_line(&format!(
            "  {} {} {} - {}",
            source_icon,
            style(server_id).green(),
            style(path.display()).cyan(),
            style(source_type).dim()
        ));
    }

    fn print_install_ready(&self, server_id: &str) {
        let _ = self.term.write_line(&format!(
            "  {} {} - ready to install",
            style("📦").cyan(),
            style(server_id).green()
        ));
    }

    fn print_missing_prerequisites(
        &self,
        server_id: &str,
        prereqs: &PrerequisitesResult,
    ) -> Result<()> {
        self.term.write_line(&format!(
            "  {} {}",
            style("⚠️").yellow(),
            style(server_id).yellow()
        ))?;

        for prereq in &prereqs.missing {
            let icon = match get_install_difficulty(prereq) {
                InstallDifficulty::Auto => "🔄",
                InstallDifficulty::PackageManager => "📦",
                InstallDifficulty::Manual => "🔧",
                InstallDifficulty::LargeDownload => "💾",
            };

            let name: &str = prereq.name();
            let instructions = prereq.install_instructions();
            self.term
                .write_line(&format!("    {icon} {name} - {instructions}"))?;
        }

        Ok(())
    }

    fn print_summary(&self, result: &LspArchitectureResult) {
        self.print_header("📋 Summary");

        let _ = self.term.write_line(&format!(
            "Total languages detected: {}",
            style(result.languages.len()).cyan()
        ));
        let _ = self.term.write_line(&format!(
            "LSP servers available: {}",
            style(result.resolved_servers.len()).green()
        ));
        let _ = self.term.write_line(&format!(
            "Ready to install: {}",
            style(result.needs_install.len()).yellow()
        ));
        let _ = self.term.write_line(&format!(
            "Missing prerequisites: {}",
            style(result.missing_prerequisites.len()).red()
        ));
    }

    fn create_spinner(&self, msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .expect("Spinner template is valid"),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }
}

// Re-export commands for use in main.rs
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum LspCommands {
    /// Set up the language servers of a project, or install servers by name
    Setup {
        /// Projects, whose languages' servers are set up, or servers to
        /// install, by id (`bash-language-server`) or language (`bash`); a
        /// name that is also a directory here is the directory [default: .]
        targets: Vec<String>,

        /// Auto-install without prompting
        #[arg(short, long)]
        yes: bool,
    },

    /// Detect LSP servers for a project
    Detect {
        /// Path to the project
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Show status of all available LSP servers
    Status,

    /// Check if a specific LSP server is available
    Check {
        /// Server ID to check (e.g., rust-analyzer, typescript-language-server)
        server_id: String,
    },

    /// Check multiple LSP servers at once
    BatchCheck {
        /// Server IDs to check
        server_ids: Vec<String>,
    },

    /// Trust a project's devscriptor-lsp.toml, after showing the commands it runs
    Trust {
        /// The configuration file
        #[arg(default_value = "devscriptor-lsp.toml")]
        path: PathBuf,
    },

    /// Withdraw trust from a project's devscriptor-lsp.toml
    Untrust {
        /// The configuration file
        #[arg(default_value = "devscriptor-lsp.toml")]
        path: PathBuf,
    },
}

/// Handle LSP subcommands
pub async fn handle_lsp_command(command: LspCommands) -> Result<()> {
    match command {
        LspCommands::Trust { path } => return trust_config(&path),
        LspCommands::Untrust { path } => return untrust_config(&path),
        _ => {}
    }
    let mut cmd = LspCommand::new()?;

    match command {
        LspCommands::Setup { targets, yes } => cmd.setup_targets(&targets, yes).await,
        LspCommands::Detect { path } => cmd.detect(&path).await,
        LspCommands::Status => cmd.status(),
        LspCommands::Check { server_id } => cmd.check(&server_id),
        LspCommands::BatchCheck { server_ids } => cmd.batch_check(&server_ids),
        LspCommands::Trust { .. } | LspCommands::Untrust { .. } => unreachable!("handled above"),
    }
}

/// `devscriptor lsp trust`: show the commands a project configuration runs,
/// then trust exactly that content (a later edit needs trusting again).
fn trust_config(path: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;
    let config = devscriptor_lsp::Config::parse(&content)?;
    println!("{} runs these language servers:", path.display());
    for server in &config.servers {
        let mut line = format!("  {:<12} {}", server.language_id, server.command);
        for arg in &server.args {
            line.push(' ');
            line.push_str(arg);
        }
        println!("{line}");
    }
    devscriptor_lsp::config::trust::trust(path, &content)?;
    println!("Trusted. Devscriptor uses it until the file changes.");
    Ok(())
}

/// `devscriptor lsp untrust`.
fn untrust_config(path: &std::path::Path) -> Result<()> {
    if devscriptor_lsp::config::trust::untrust(path)? {
        println!("{} is no longer trusted.", path.display());
    } else {
        println!("{} was not trusted.", path.display());
    }
    Ok(())
}

/// Go-to-definition via a live LSP server.
///
/// Requires a language server running for the file's extension.
/// Outputs JSON to stdout.
pub async fn execute_definition(
    _db_path: std::path::PathBuf,
    file_path: String,
    line: u32,
    character: u32,
) -> anyhow::Result<()> {
    // Same catalog the MCP server uses: an explicit config file wins, otherwise
    // every registry server installed here — not just the built-in three.
    let config = devscriptor_lsp::Config::explicit()
        .ok()
        .flatten()
        .unwrap_or_else(devscriptor_lsp::Config::from_installed_servers);
    let integration = devscriptor_lsp::initialize(config)
        .await
        .map_err(|e| anyhow::anyhow!("LSP init failed: {e}"))?;

    let params = devscriptor_lsp::mcp::tools::DefinitionParams {
        file_path,
        line,
        character,
    };
    let result = integration
        .translator()
        .handle_definition(params)
        .await
        .map_err(|e| anyhow::anyhow!("definition request failed: {e}"))?;

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

/// Find-references via a live LSP server.
pub async fn execute_references(
    _db_path: std::path::PathBuf,
    file_path: String,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> anyhow::Result<()> {
    // Same catalog the MCP server uses: an explicit config file wins, otherwise
    // every registry server installed here — not just the built-in three.
    let config = devscriptor_lsp::Config::explicit()
        .ok()
        .flatten()
        .unwrap_or_else(devscriptor_lsp::Config::from_installed_servers);
    let integration = devscriptor_lsp::initialize(config)
        .await
        .map_err(|e| anyhow::anyhow!("LSP init failed: {e}"))?;

    let params = devscriptor_lsp::mcp::tools::ReferencesParams {
        file_path,
        line,
        character,
        include_declaration,
    };
    let result = integration
        .translator()
        .handle_references(params)
        .await
        .map_err(|e| anyhow::anyhow!("references request failed: {e}"))?;

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
