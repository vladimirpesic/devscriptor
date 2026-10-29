//! Configuration loading and validation.
//!
//! This module handles loading LSP server configurations from files
//! and environment variables.

pub mod lifecycle;
pub mod server;
pub mod trust;
pub mod workspace;

pub use lifecycle::{LifecycleConfig, LifecycleOverrides, StartupMode};
pub use server::{LspServerConfig, ServerHeuristics, ServerSettings};
pub use workspace::WorkspaceConfig;

use crate::error::{LspError, Result};
use std::path::{Path, PathBuf};

/// Environment variable for config file path.
const CONFIG_ENV_VAR: &str = "DEVSCRIPTOR_LSP_CONFIG";
/// Local config file name.
const LOCAL_CONFIG_FILE: &str = "devscriptor-lsp.toml";

/// Main configuration structure.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// Workspace configuration.
    pub workspace: WorkspaceConfig,
    /// LSP server configurations.
    #[serde(rename = "lsp_servers")]
    pub servers: Vec<LspServerConfig>,
    /// When the servers start and stop: the `[lifecycle]` table. Every key
    /// is optional.
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
}

impl Config {
    /// Load configuration from default locations.
    ///
    /// Tries in order:
    /// 1. Environment variable `DEVSCRIPTOR_LSP_CONFIG`
    /// 2. Local file `devscriptor-lsp.toml`, if trusted (see [`trust`])
    /// 3. Default configuration
    ///
    /// # Errors
    ///
    /// Returns an error if configuration file exists but cannot be read or parsed.
    pub fn load() -> Result<Self> {
        Ok(Self::explicit()?.unwrap_or_default())
    }

    /// The configuration the user explicitly provided, if any.
    ///
    /// `Some` when `$DEVSCRIPTOR_LSP_CONFIG` is set, or when `devscriptor-lsp.toml`
    /// in the working directory is trusted; `None` otherwise, letting the
    /// caller choose its own fallback instead of silently taking
    /// [`Config::default`].
    ///
    /// The project file names commands to run, and the directory may be a
    /// repository someone else wrote, so it is used only as trusted with
    /// `devscriptor lsp trust` ([`trust`]); an untrusted or since-edited one is
    /// ignored with a warning.
    ///
    /// # Errors
    ///
    /// Returns an error if a configuration file exists but cannot be read or parsed.
    pub fn explicit() -> Result<Option<Self>> {
        if let Ok(path) = std::env::var(CONFIG_ENV_VAR) {
            return Self::load_from(Path::new(&path)).map(Some);
        }

        let local = PathBuf::from(LOCAL_CONFIG_FILE);
        if !local.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&local).map_err(|e| {
            LspError::Config(format!(
                "Failed to read config from {}: {e}",
                local.display()
            ))
        })?;
        if !trust::is_trusted(&local, content.as_bytes()) {
            tracing::warn!(
                "Ignoring ./{LOCAL_CONFIG_FILE}: it names commands to run as language servers, \
                 and this content has not been trusted. Review it, then run \
                 `devscriptor lsp trust` to use it."
            );
            return Ok(None);
        }
        Self::parse(&content).map(Some)
    }

    /// Build a configuration from every registry server that is actually
    /// installed on this machine.
    ///
    /// [`Config::default`] holds three servers — rust-analyzer, pyright and
    /// typescript-language-server — and the translator derives its extension →
    /// language map from `servers`, so with it the 38 LSP tools would serve only
    /// `.rs`, `.py` and `.ts`/`.js`, whatever else `devscriptor lsp status`
    /// reports available.
    ///
    /// The architecture tools know all 21 servers and locate their binaries with
    /// [`LspResolver`](crate::lsp_architecture::LspResolver)'s search (user
    /// config → Devscriptor's own installs → system `PATH` → editor extensions).
    /// This builds the translator's catalog from exactly that source, so both
    /// halves agree on which servers exist and where they live.
    ///
    /// Servers that are not installed are skipped: they cannot be spawned, and
    /// including them would only mask a missing binary as a spawn failure.
    #[must_use]
    pub fn from_installed_servers() -> Self {
        let Ok(resolver) = crate::lsp_architecture::LspResolver::new() else {
            tracing::warn!("LSP resolver unavailable; falling back to the built-in server set");
            return Self::default();
        };

        let servers: Vec<LspServerConfig> = crate::generated_registry::SERVER_IDS
            .iter()
            .filter_map(|id| resolver.resolve(id))
            .map(|resolution| resolution.config)
            .collect();

        if servers.is_empty() {
            tracing::info!("No language servers installed; LSP tools will report none available");
        } else {
            tracing::info!(
                "Language servers available: {}",
                servers
                    .iter()
                    .map(|s| s.language_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        Self {
            workspace: WorkspaceConfig::default(),
            servers,
            lifecycle: LifecycleConfig::default(),
        }
    }

    /// Load configuration from a specific file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the configuration file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            LspError::Config(format!(
                "Failed to read config from {}: {e}",
                path.display()
            ))
        })?;
        Self::parse(&content)
    }

    /// Parse and validate the TOML of a configuration file.
    ///
    /// # Errors
    ///
    /// Returns an error if `content` is not a valid configuration.
    pub fn parse(content: &str) -> Result<Self> {
        let config: Config = toml::from_str(content)
            .map_err(|e| LspError::Config(format!("Failed to parse config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration.
    fn validate(&self) -> Result<()> {
        for server in &self.servers {
            if server.language_id.is_empty() {
                return Err(LspError::Config(
                    "Server language_id cannot be empty".into(),
                ));
            }
            if server.command.is_empty() {
                return Err(LspError::Config(format!(
                    "Command cannot be empty for {}",
                    server.language_id
                )));
            }
        }
        Ok(())
    }

    /// Get server config for a language ID.
    #[must_use]
    pub fn get_server_for_language(&self, language_id: &str) -> Option<&LspServerConfig> {
        self.servers.iter().find(|s| s.language_id == language_id)
    }

    /// Get server config for a file extension.
    #[must_use]
    pub fn get_server_for_extension(&self, ext: &str) -> Option<&LspServerConfig> {
        // Compare `&str` against each element, without allocating a `String` for
        // every lookup.
        self.servers
            .iter()
            .find(|s| s.extensions.iter().any(|e| e == ext))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace: WorkspaceConfig::default(),
            servers: vec![
                LspServerConfig::rust_analyzer(),
                LspServerConfig::pyright(),
                LspServerConfig::typescript(),
            ],
            lifecycle: LifecycleConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The translator's catalog comes from the registry, not from a hard-coded
    /// list: `Config::default` alone would limit all 38 LSP tools to Rust,
    /// Python and TypeScript.
    #[test]
    fn installed_servers_are_drawn_from_the_registry() {
        let config = Config::from_installed_servers();

        // Whatever is installed here, every entry must be a registry server, and
        // there can never be more than the registry knows about.
        assert!(
            config.servers.len() <= crate::generated_registry::SERVER_IDS.len(),
            "more servers than the registry defines"
        );

        let known: Vec<&str> = crate::generated_registry::SERVER_IDS
            .iter()
            .filter_map(|id| crate::generated_registry::language_id(id))
            .collect();
        for server in &config.servers {
            assert!(
                known.contains(&server.language_id.as_str()),
                "`{}` is not a registry language",
                server.language_id
            );
            assert!(
                !server.extensions.is_empty(),
                "`{}` must map at least one extension",
                server.language_id
            );
        }
    }

    /// The registry covers every language Devscriptor claims to support, and is
    /// strictly larger than the three-server fallback.
    #[test]
    fn registry_is_larger_than_the_builtin_fallback() {
        assert_eq!(
            crate::generated_registry::SERVER_IDS.len(),
            21,
            "registry should define 21 language servers"
        );
        assert!(
            crate::generated_registry::SERVER_IDS.len() > Config::default().servers.len(),
            "the fallback must never be the whole catalog"
        );
    }

    /// A project's `devscriptor-lsp.toml` names commands to run, so it is used
    /// only with the content the user trusted. Linux only: the trust store is
    /// redirected through `XDG_CONFIG_HOME`, which other platforms ignore.
    #[cfg(target_os = "linux")]
    #[test]
    fn explicit_uses_a_project_file_only_as_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join(LOCAL_CONFIG_FILE);
        let config = "[workspace]\nroots = [\".\"]\n\n[[lsp_servers]]\nlanguage_id = \"rust\"\ncommand = \"rust-analyzer\"\nextensions = [\"rs\"]\n";
        std::fs::write(&file, config).unwrap();

        let previous_dir = std::env::current_dir().unwrap();
        let previous_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_current_dir(&project).unwrap();
        // SAFETY: single-threaded section; both restored below.
        unsafe {
            std::env::remove_var(CONFIG_ENV_VAR);
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        }

        let untrusted = Config::explicit();
        trust::trust(&file, config).unwrap();
        let trusted = Config::explicit();
        std::fs::write(&file, config.replace("rust-analyzer", "/tmp/elsewhere")).unwrap();
        let edited = Config::explicit();

        std::env::set_current_dir(previous_dir).unwrap();
        // SAFETY: restoring the previous state.
        unsafe {
            match previous_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }

        assert!(matches!(untrusted, Ok(None)), "untrusted: {untrusted:?}");
        let trusted = trusted.unwrap().expect("trusted file is used");
        assert_eq!(trusted.servers[0].command, "rust-analyzer");
        assert!(
            matches!(edited, Ok(None)),
            "an edit withdraws trust: {edited:?}"
        );
    }

    /// `explicit()` reports absence rather than silently substituting the
    /// three-server default, which is what let the fallback leak into production.
    #[test]
    fn explicit_returns_none_without_a_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        // SAFETY: single-threaded section; restored immediately below.
        unsafe { std::env::remove_var(CONFIG_ENV_VAR) };

        let result = Config::explicit();
        std::env::set_current_dir(previous).unwrap();

        assert!(
            matches!(result, Ok(None)),
            "expected Ok(None), got {result:?}"
        );
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(!config.servers.is_empty());
        assert_eq!(config.workspace.roots.len(), 1);
    }

    #[test]
    fn test_config_get_server_for_language() {
        let config = Config::default();
        assert!(config.get_server_for_language("rust").is_some());
        assert!(config.get_server_for_language("unknown").is_none());
    }

    #[test]
    fn test_config_get_server_for_extension() {
        let config = Config::default();
        assert!(config.get_server_for_extension("rs").is_some());
        assert!(config.get_server_for_extension("py").is_some());
        assert!(config.get_server_for_extension("unknown").is_none());
    }

    #[test]
    fn test_config_load_from_valid() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let content = r#"
[workspace]
roots = ["."]

[[lsp_servers]]
language_id = "test"
command = "test-server"
extensions = ["test"]
"#;

        std::fs::write(&config_path, content).unwrap();

        let config = Config::load_from(&config_path).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].language_id, "test");
    }

    #[test]
    fn test_config_load_from_invalid() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        std::fs::write(&config_path, "[invalid toml").unwrap();

        let result = Config::load_from(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validate_empty_language_id() {
        let config = Config {
            workspace: WorkspaceConfig::default(),
            servers: vec![LspServerConfig {
                language_id: String::new(),
                command: "test".into(),
                args: vec![],
                env: std::collections::HashMap::new(),
                working_dir: None,
                extensions: vec![],
                heuristics: None,
                initialization_options: None,
                settings: ServerSettings::default(),
                configuration: None,
            }],
            lifecycle: LifecycleConfig::default(),
        };

        assert!(config.validate().is_err());
    }

    /// The servers of a minimal configuration file, before its `[lifecycle]`.
    const SERVERS: &str = "[workspace]\nroots = [\".\"]\n\n\
                           [[lsp_servers]]\nlanguage_id = \"rust\"\n\
                           command = \"rust-analyzer\"\nextensions = [\"rs\"]\n";

    #[test]
    fn a_lifecycle_table_sets_the_keys_it_names() {
        let config = Config::parse(&format!(
            "{SERVERS}\n[lifecycle]\nstartup = \"eager\"\nidle_timeout_secs = 30\n"
        ))
        .unwrap();
        assert_eq!(
            config.lifecycle,
            LifecycleConfig {
                startup: StartupMode::Eager,
                idle_timeout_secs: 30,
                ..LifecycleConfig::default()
            }
        );
    }

    #[test]
    fn a_file_without_a_lifecycle_table_has_the_defaults() {
        let config = Config::parse(SERVERS).unwrap();
        assert_eq!(config.lifecycle, LifecycleConfig::default());
    }

    #[test]
    fn an_invalid_lifecycle_table_is_an_error() {
        for table in [
            "[lifecycle]\nstartup = \"sometimes\"\n",
            "[lifecycle]\nidle_timeout_secs = -1\n",
            "[lifecycle]\nmax_running_servers = \"two\"\n",
        ] {
            assert!(
                Config::parse(&format!("{SERVERS}\n{table}")).is_err(),
                "{table}"
            );
        }
    }
}
