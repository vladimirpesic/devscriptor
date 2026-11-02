//! When language servers start and stop: the `[lifecycle]` table.

use crate::error::{LspError, Result};
use serde::{Deserialize, Serialize};

/// Environment variable overriding [`LifecycleConfig::startup`] (`lazy` or `eager`).
pub const STARTUP_ENV_VAR: &str = "DEVSCRIPTOR_LSP_STARTUP";
/// Environment variable overriding [`LifecycleConfig::idle_timeout_secs`].
pub const IDLE_SECS_ENV_VAR: &str = "DEVSCRIPTOR_LSP_IDLE_SECS";
/// Environment variable overriding [`LifecycleConfig::max_running_servers`].
pub const MAX_SERVERS_ENV_VAR: &str = "DEVSCRIPTOR_LSP_MAX_SERVERS";

/// When the language servers that apply to the workspace are started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupMode {
    /// Each language's server starts when a tool first needs it, or in the
    /// background once `lsp_arch_detect` finds the language. Nothing starts
    /// before the MCP server answers `initialize`.
    #[default]
    Lazy,
    /// Every applicable server starts before the MCP server answers
    /// `initialize`.
    Eager,
}

impl std::str::FromStr for StartupMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "lazy" => Ok(Self::Lazy),
            "eager" => Ok(Self::Eager),
            other => Err(format!("`{other}` is not `lazy` or `eager`")),
        }
    }
}

/// When language servers start and stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LifecycleConfig {
    /// When the applicable servers start. Default: [`StartupMode::Lazy`].
    pub startup: StartupMode,
    /// Seconds a running server may go unused before it is stopped; the next
    /// request starts it again. 0 never stops one. Default: 600.
    pub idle_timeout_secs: u64,
    /// The most servers running at once: starting one more first stops the
    /// least recently used. 0, the default, sets no cap.
    pub max_running_servers: usize,
    /// How often, in milliseconds, idle servers are looked for. A knob for
    /// tests; default 60 000.
    pub maintenance_tick_ms: u64,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            startup: StartupMode::Lazy,
            idle_timeout_secs: 600,
            max_running_servers: 0,
            maintenance_tick_ms: 60_000,
        }
    }
}

/// Lifecycle settings given on the command line (`devscriptor mcp
/// --lsp-startup`), which outrank every other source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LifecycleOverrides {
    /// `--lsp-startup`.
    pub startup: Option<StartupMode>,
}

impl LifecycleConfig {
    /// The settings in effect, highest precedence first: the command line
    /// (`overrides`), the environment (read through `env`), then `self` (the
    /// configuration file's `[lifecycle]` table, whose absent keys hold the
    /// defaults).
    ///
    /// # Errors
    ///
    /// Returns an error naming the variable when an environment value is not
    /// valid for it.
    pub fn resolve(
        mut self,
        env: impl Fn(&str) -> Option<String>,
        overrides: LifecycleOverrides,
    ) -> Result<Self> {
        if let Some(value) = env(STARTUP_ENV_VAR) {
            self.startup = value
                .parse::<StartupMode>()
                .map_err(|e| invalid(STARTUP_ENV_VAR, &e))?;
        }
        if let Some(value) = env(IDLE_SECS_ENV_VAR) {
            self.idle_timeout_secs = value
                .trim()
                .parse()
                .map_err(|e| invalid(IDLE_SECS_ENV_VAR, &format!("`{value}`: {e}")))?;
        }
        if let Some(value) = env(MAX_SERVERS_ENV_VAR) {
            self.max_running_servers = value
                .trim()
                .parse()
                .map_err(|e| invalid(MAX_SERVERS_ENV_VAR, &format!("`{value}`: {e}")))?;
        }
        if let Some(startup) = overrides.startup {
            self.startup = startup;
        }
        Ok(self)
    }

    /// [`Self::resolve`] against the process environment.
    ///
    /// # Errors
    ///
    /// See [`Self::resolve`].
    pub fn resolve_from_env(self, overrides: LifecycleOverrides) -> Result<Self> {
        self.resolve(|name| std::env::var(name).ok(), overrides)
    }
}

fn invalid(variable: &str, reason: &str) -> LspError {
    LspError::Config(format!("invalid {variable}: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn the_defaults_are_lazy_ten_idle_minutes_and_no_cap() {
        let config = LifecycleConfig::default();
        assert_eq!(config.startup, StartupMode::Lazy);
        assert_eq!(config.idle_timeout_secs, 600);
        assert_eq!(config.max_running_servers, 0);
        assert_eq!(config.maintenance_tick_ms, 60_000);
    }

    /// Command line, then environment, then file, then default.
    #[test]
    fn each_source_outranks_the_ones_below_it() {
        let file = LifecycleConfig {
            startup: StartupMode::Eager,
            idle_timeout_secs: 30,
            ..LifecycleConfig::default()
        };

        // The file over the defaults.
        let resolved = file
            .clone()
            .resolve(env(&[]), LifecycleOverrides::default())
            .unwrap();
        assert_eq!(resolved, file);

        // The environment over the file.
        let resolved = file
            .clone()
            .resolve(
                env(&[
                    (STARTUP_ENV_VAR, "lazy"),
                    (IDLE_SECS_ENV_VAR, "0"),
                    (MAX_SERVERS_ENV_VAR, "2"),
                ]),
                LifecycleOverrides::default(),
            )
            .unwrap();
        assert_eq!(resolved.startup, StartupMode::Lazy);
        assert_eq!(resolved.idle_timeout_secs, 0);
        assert_eq!(resolved.max_running_servers, 2);

        // The command line over the environment.
        let resolved = file
            .resolve(
                env(&[(STARTUP_ENV_VAR, "lazy")]),
                LifecycleOverrides {
                    startup: Some(StartupMode::Eager),
                },
            )
            .unwrap();
        assert_eq!(resolved.startup, StartupMode::Eager);
    }

    #[test]
    fn an_invalid_environment_value_names_its_variable() {
        for (variable, value) in [
            (STARTUP_ENV_VAR, "sometimes"),
            (IDLE_SECS_ENV_VAR, "ten"),
            (MAX_SERVERS_ENV_VAR, "-1"),
        ] {
            let err = LifecycleConfig::default()
                .resolve(env(&[(variable, value)]), LifecycleOverrides::default())
                .unwrap_err()
                .to_string();
            assert!(err.contains(variable), "{err}");
        }
    }

    #[test]
    fn startup_mode_parses_either_case() {
        assert_eq!("Eager".parse::<StartupMode>(), Ok(StartupMode::Eager));
        assert_eq!(" lazy ".parse::<StartupMode>(), Ok(StartupMode::Lazy));
        assert!("".parse::<StartupMode>().is_err());
    }
}
