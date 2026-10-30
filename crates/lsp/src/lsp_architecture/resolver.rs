//! LSP Architecture Resolver
//!
//! Provides 4-tier resolution for 21 LSP servers:
//! 1. User configuration override
//! 2. Bundled installation (bundle/lsp-servers/)
//! 3. System-wide detection (PATH, SDK paths)
//! 4. VS Code extension detection

use std::path::{Path, PathBuf};
use tracing::{debug, info, trace, warn};

use crate::config::{LspServerConfig, ServerHeuristics};
use crate::generated_registry;
use crate::manager::registry::SERVER_DIR_PLACEHOLDER;

use super::system::detect_system_lsp;
use super::vscode::detect_lsp_server;

/// Source of an LSP server resolution
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspSource {
    /// User-provided configuration
    UserConfig { path: PathBuf, args: Vec<String> },
    /// Installed on demand by Devscriptor (`~/.cache/devscriptor/lsp-servers/`)
    Installed(PathBuf),
    /// Bundled with Devscriptor (shipped alongside the executable)
    Bundled(PathBuf),
    /// Found in system PATH or SDK paths
    SystemWide(PathBuf),
    /// Found in VS Code extensions
    VsCodeExtension(PathBuf),
}

impl LspSource {
    /// Get the path to the LSP binary
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            LspSource::UserConfig { path, .. } => path,
            LspSource::Installed(path) => path,
            LspSource::Bundled(path) => path,
            LspSource::SystemWide(path) => path,
            LspSource::VsCodeExtension(path) => path,
        }
    }

    /// Get the source type as a string
    #[must_use]
    pub fn source_type(&self) -> &'static str {
        match self {
            LspSource::UserConfig { .. } => "user config",
            LspSource::Installed(_) => "installed",
            LspSource::Bundled(_) => "bundled",
            LspSource::SystemWide(_) => "system",
            LspSource::VsCodeExtension(_) => "vscode extension",
        }
    }

    /// Check if this is a user configuration
    #[must_use]
    pub fn is_user_config(&self) -> bool {
        matches!(self, LspSource::UserConfig { .. })
    }
}

/// Result of LSP server resolution
#[derive(Debug, Clone)]
pub struct ResolutionResult {
    /// The resolved LSP source
    pub source: LspSource,
    /// Server configuration
    pub config: LspServerConfig,
}

/// LSP Server Resolver with 4-tier resolution
#[derive(Clone)]
pub struct LspResolver {
    /// Devscriptor on-demand install directory (`~/.cache/devscriptor/lsp-servers/`)
    install_dir: PathBuf,
    /// Bundled installation directory (shipped alongside the executable)
    bundled_dir: PathBuf,
    /// User configuration (optional)
    user_config: Option<UserLspConfig>,
}

/// User-provided LSP configuration
#[derive(Debug, Clone, Default)]
pub struct UserLspConfig {
    /// Server ID to custom path mapping
    pub server_paths: std::collections::HashMap<String, ServerOverride>,
    /// Whether to enable VS Code extension discovery
    pub vscode_discovery: bool,
    /// Whether to enable system PATH discovery
    pub system_discovery: bool,
    /// Additional paths to search
    pub extra_paths: Vec<PathBuf>,
}

/// User override for a specific server
#[derive(Debug, Clone)]
pub struct ServerOverride {
    /// Path to the binary
    pub path: PathBuf,
    /// Custom arguments
    pub args: Option<Vec<String>>,
    /// Environment variables
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Disable auto-detection for this server
    pub auto_detect: bool,
}

impl LspResolver {
    /// Create a new resolver with default bundled directory
    pub fn new() -> anyhow::Result<Self> {
        let bundled_dir = Self::default_bundled_dir()?;
        Ok(Self {
            install_dir: crate::manager::install_root(),
            bundled_dir,
            user_config: None,
        })
    }

    /// Create a new resolver with a custom bundled directory.
    ///
    /// `install_dir` (the on-demand managed-install tier) is set to a
    /// non-overlapping subdirectory so that managed-install results cannot
    /// shadow bundled results.  Use [`Self::with_dirs`] when you need to
    /// configure both tiers independently (e.g. in tests for the install tier).
    #[must_use]
    pub fn with_bundled_dir(bundled_dir: PathBuf) -> Self {
        Self {
            install_dir: bundled_dir.join("__managed"),
            bundled_dir,
            user_config: None,
        }
    }

    /// Create a resolver with explicit install and bundled directories.
    ///
    /// Intended for tests that need to control both tiers independently.
    #[must_use]
    pub fn with_dirs(install_dir: PathBuf, bundled_dir: PathBuf) -> Self {
        Self {
            install_dir,
            bundled_dir,
            user_config: None,
        }
    }

    /// Set user configuration
    #[must_use]
    pub fn with_user_config(mut self, config: UserLspConfig) -> Self {
        self.user_config = Some(config);
        self
    }

    /// Get the default bundled installation directory
    fn default_bundled_dir() -> anyhow::Result<PathBuf> {
        // Check for bundle/lsp-servers relative to executable
        let exe_path = std::env::current_exe()?;
        let exe_dir = exe_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not determine executable directory"))?;

        // Try several locations
        let candidates = [
            exe_dir.join("bundle/lsp-servers"),
            exe_dir.join("../bundle/lsp-servers"),
            exe_dir.join("../../bundle/lsp-servers"),
            PathBuf::from("./bundle/lsp-servers"),
            PathBuf::from("../bundle/lsp-servers"),
        ];

        for candidate in &candidates {
            if candidate.exists() {
                debug!("Using bundled LSP directory: {}", candidate.display());
                return Ok(candidate.clone());
            }
        }

        // Default to standard location
        Ok(PathBuf::from("./bundle/lsp-servers"))
    }

    /// Async version of [`LspResolver::resolve`] that avoids blocking the async executor
    /// during filesystem checks.
    pub async fn resolve_async(&self, server_id: String) -> Option<ResolutionResult> {
        let resolver = self.clone();
        tokio::task::spawn_blocking(move || resolver.resolve(&server_id))
            .await
            .ok()
            .flatten()
    }

    /// Resolve an LSP server using the 4-tier strategy
    pub fn resolve(&self, server_id: &str) -> Option<ResolutionResult> {
        trace!("Resolving LSP server: {}", server_id);

        // Reject unknown or traversal-bearing server IDs before any path construction.
        if !generated_registry::SERVER_IDS.contains(&server_id) {
            warn!(
                "Rejected unknown server_id '{}' — not in whitelist",
                server_id
            );
            return None;
        }

        // Tier 1: User configuration (highest priority)
        if let Some(result) = self.resolve_from_user_config(server_id) {
            info!("Resolved {} from user configuration", server_id);
            return Some(result);
        }

        // Tier 2: Devscriptor-managed installs (on-demand cache, then bundled)
        if let Some(result) = self.resolve_from_managed(server_id) {
            return Some(result);
        }

        // Check if system discovery is enabled (default: true)
        let system_enabled = self.user_config.as_ref().is_none_or(|c| c.system_discovery);

        if system_enabled {
            // Tier 3: System-wide detection
            if let Some(result) = self.resolve_from_system(server_id) {
                info!("Resolved {} from system PATH", server_id);
                return Some(result);
            }
        }

        // Check if VS Code discovery is enabled (default: true)
        let vscode_enabled = self.user_config.as_ref().is_none_or(|c| c.vscode_discovery);

        if vscode_enabled {
            // Tier 4: VS Code extensions
            if let Some(result) = self.resolve_from_vscode(server_id) {
                info!("Resolved {} from VS Code extension", server_id);
                return Some(result);
            }
        }

        warn!("Could not resolve LSP server: {}", server_id);
        None
    }

    /// Resolve from user configuration (Tier 1)
    fn resolve_from_user_config(&self, server_id: &str) -> Option<ResolutionResult> {
        let config = self.user_config.as_ref()?;
        let override_config = config.server_paths.get(server_id)?;

        if !override_config.auto_detect {
            trace!("Auto-detection disabled for {} in user config", server_id);
        }

        if override_config.path.exists() {
            let source = LspSource::UserConfig {
                path: override_config.path.clone(),
                args: override_config.args.clone().unwrap_or_default(),
            };

            let lsp_config = self.build_config(server_id, &source);

            Some(ResolutionResult {
                source,
                config: lsp_config,
            })
        } else {
            warn!(
                "User-configured path for {} does not exist: {}",
                server_id,
                override_config.path.display()
            );
            None
        }
    }

    /// Resolve from Devscriptor-managed installs (Tier 2).
    ///
    /// Checks the on-demand install directory (`~/.cache/devscriptor/lsp-servers/`,
    /// populated by `lsp install`) first, then servers shipped alongside the
    /// executable (`bundle/lsp-servers/`).
    ///
    /// A server that comes with its toolchain (sourcekit-lsp) runs from the
    /// toolchain: a copy here is one `lsp setup` took from the mirror, on
    /// Linux swiftly's proxy, which kept the toolchain's own from running.
    fn resolve_from_managed(&self, server_id: &str) -> Option<ResolutionResult> {
        if Self::registry_info(server_id).is_some_and(|info| {
            info.source.source_type == crate::manager::registry::SourceType::System
        }) {
            return None;
        }
        if let Some(path) = Self::find_server_binary(&self.install_dir, server_id)
            && !Self::is_outdated(&self.install_dir, server_id)
            && !Self::is_built_for_another_ruby(&self.install_dir, server_id)
        {
            info!("Resolved {} from Devscriptor install", server_id);
            let source = LspSource::Installed(path);
            let config = self.build_config(server_id, &source);
            return Some(ResolutionResult { source, config });
        }

        if let Some(path) = Self::find_server_binary(&self.bundled_dir, server_id)
            && !Self::is_built_for_another_ruby(&self.bundled_dir, server_id)
        {
            info!("Resolved {} from bundled installation", server_id);
            let source = LspSource::Bundled(path);
            let config = self.build_config(server_id, &source);
            return Some(ResolutionResult { source, config });
        }

        trace!("No Devscriptor-managed install for {}", server_id);
        None
    }

    /// Whether the gem install of `server_id` under `root` was built for another
    /// Ruby than this machine's, whose native extensions it cannot load (see
    /// [`crate::manager::ruby`]): it is skipped, as it would exit before
    /// answering, until it is installed for this Ruby — by `devscriptor lsp
    /// setup`, or where a linked one was made. Said once per server and
    /// process, with how ([`crate::manager::Unavailable`]).
    fn is_built_for_another_ruby(root: &Path, server_id: &str) -> bool {
        let Some(info) = Self::registry_info(server_id) else {
            return false;
        };
        if info.source.source_type != crate::manager::registry::SourceType::Gem {
            return false;
        }
        let local = crate::manager::ruby::local_extension_api();
        if crate::manager::ruby::built_for_another_ruby(&root.join(server_id), local).is_none() {
            return false;
        }
        static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut said = SAID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !said.iter().any(|id| id == server_id) {
            said.push(server_id.to_string());
            warn!(
                "{} {}",
                info.name,
                crate::manager::Unavailable::of(root, info).reason
            );
        }
        true
    }

    /// Whether the install of `server_id` under `root` records another version
    /// than the build its registry entry pins (see
    /// [`ServerInfo::outdated_by`](crate::manager::registry::ServerInfo::outdated_by)):
    /// it is skipped, as the build it is may not start with this registry's
    /// settings, until `devscriptor lsp setup` replaces it, or a linked one is
    /// updated where it was made. Said once per server and process, with how
    /// ([`crate::manager::Unavailable`]).
    fn is_outdated(root: &Path, server_id: &str) -> bool {
        let Some(info) = Self::registry_info(server_id) else {
            return false;
        };
        let recorded = std::fs::read_to_string(root.join(server_id).join(".version")).ok();
        if !info.outdated_by(recorded.as_deref()) {
            return false;
        }
        static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut said = SAID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !said.iter().any(|id| id == server_id) {
            said.push(server_id.to_string());
            warn!(
                "{} {}",
                info.name,
                crate::manager::Unavailable::of(root, info).reason
            );
        }
        true
    }

    /// Find an LSP server binary under `root/<server_id>/`.
    ///
    /// The registry's `bin_path` is authoritative — it is what the installer uses
    /// to place the binary, and it is the only thing that knows a layout like
    /// `node_modules/.bin/vue-language-server` or `bin/intellij-server` (Kotlin).
    /// The four fixed layouts (`bin/<id>`, `<id>`, `server/<id>`,
    /// `<id>-language-server`) are a fallback, for hand-placed binaries.
    ///
    /// Validates `server_id` against the canonical `SERVER_IDS` whitelist
    /// before joining onto the root path. An unrecognised or traversal-bearing
    /// ID (e.g. `"../../../etc"`) would otherwise resolve to an arbitrary path
    /// on the filesystem.
    fn find_server_binary(root: &Path, server_id: &str) -> Option<PathBuf> {
        if !generated_registry::SERVER_IDS.contains(&server_id) {
            return None;
        }
        let base = root.join(server_id);
        if !base.exists() {
            return None;
        }

        // A gem's executable runs through `ruby` (see `build_config`), so it
        // is the Ruby script, not the `.bat` that wraps it on Windows.
        let script = Self::registry_info(server_id)
            .is_some_and(|i| i.source.source_type == crate::manager::registry::SourceType::Gem);
        if let Some(info) = Self::registry_info(server_id) {
            // The second layout is a mirror package unpacked without being lifted out of
            // its top-level `<id>/` folder.
            let layouts = [
                base.join(&info.bin_path),
                base.join(server_id).join(&info.bin_path),
            ];
            if let Some(found) = layouts
                .iter()
                .find_map(|candidate| Self::first_executable(candidate, cfg!(windows), script))
            {
                return Some(found);
            }
        }

        let candidates = [
            base.join("bin").join(server_id),
            base.join(server_id),
            base.join("server").join(server_id),
            base.join(format!("{server_id}-language-server")),
        ];
        candidates
            .into_iter()
            .find_map(|candidate| Self::first_executable(&candidate, cfg!(windows), script))
    }

    /// `path`, or on Windows `path` plus an executable suffix.
    ///
    /// `bin_path` in the registry is written without one — `bin/clangd`,
    /// `bin/intellij-server`, `node_modules/.bin/pyright-langserver`. The
    /// Windows installs hold `clangd.exe`, `intellij-server.exe` and
    /// `pyright-langserver.cmd`, so an exact match would find nothing there and
    /// every managed install would look absent.
    ///
    /// On `windows` only a suffixed file qualifies: npm puts a POSIX shell
    /// script beside each `.cmd` (`node_modules/.bin/pyright-langserver`), and
    /// jdtls a Python one beside `bin/jdtls.bat`, and Windows starts neither.
    /// A `script` (a gem's executable, which runs through `ruby`) is the
    /// exception: it is the bare file, the Ruby script, or none.
    ///
    /// Only a file qualifies: a directory that happens to carry the server's
    /// name (as a mirror package's unlifted `<id>/` folder does) is not a binary.
    fn first_executable(path: &Path, windows: bool, script: bool) -> Option<PathBuf> {
        if !windows || script {
            return path.is_file().then(|| path.to_path_buf());
        }
        let name = path.file_name()?.to_str()?;
        let parent = path.parent()?;
        ["exe", "cmd", "bat", "ps1"]
            .iter()
            .map(|ext| parent.join(format!("{name}.{ext}")))
            .find(|candidate| candidate.is_file())
    }

    /// The registry entry for `server_id`, loaded once from the embedded JSON.
    fn registry_info(server_id: &str) -> Option<&'static crate::manager::registry::ServerInfo> {
        crate::manager::registry::ServerRegistry::entry(server_id)
    }

    /// Resolve from system-wide detection (Tier 3)
    fn resolve_from_system(&self, server_id: &str) -> Option<ResolutionResult> {
        let system_path = detect_system_lsp(server_id)?;

        let source = LspSource::SystemWide(system_path);
        let lsp_config = self.build_config(server_id, &source);

        Some(ResolutionResult {
            source,
            config: lsp_config,
        })
    }

    /// Resolve from VS Code extensions (Tier 4)
    fn resolve_from_vscode(&self, server_id: &str) -> Option<ResolutionResult> {
        let vscode_path = detect_lsp_server(server_id)?;

        let source = LspSource::VsCodeExtension(vscode_path);
        let lsp_config = self.build_config(server_id, &source);

        Some(ResolutionResult {
            source,
            config: lsp_config,
        })
    }

    /// Build LSP server configuration from source.
    ///
    /// The launch details come from the registry: the arguments that put a server
    /// into stdio mode (`--stdio`, `start`, `language-server`, …), the environment
    /// it needs to find itself, and any `initializationOptions` it refuses to start
    /// without: a server that does not default to stdio, started without its
    /// arguments, produces no output and times out.
    ///
    /// A user-supplied config overrides the registry's arguments, since that is the
    /// whole point of the first resolution tier.
    fn build_config(&self, server_id: &str, source: &LspSource) -> LspServerConfig {
        use crate::config::ServerSettings;
        use crate::manager::registry::Placeholders;

        let path = source.path().to_string_lossy().to_string();
        let info = Self::registry_info(server_id);
        // A gem Devscriptor installed runs under this machine's `ruby`: its
        // executable's shebang names the Ruby the mirror built it with
        // (`#!/usr/bin/env ruby3.2`), which exited 127 wherever that Ruby is
        // not installed. One found elsewhere runs as its own installer set it.
        let managed = matches!(source, LspSource::Installed(_) | LspSource::Bundled(_));
        let through_ruby = managed
            && info
                .is_some_and(|i| i.source.source_type == crate::manager::registry::SourceType::Gem);
        let command = if through_ruby {
            "ruby".to_string()
        } else {
            path.clone()
        };

        // `{server_dir}` in the registry refers to the directory holding the
        // server, which is the binary's location minus the declared `bin_path`.
        let server_dir = info
            .and_then(|i| {
                let depth = Path::new(&i.bin_path).components().count();
                let mut dir = Path::new(&path);
                for _ in 0..depth {
                    dir = dir.parent()?;
                }
                Some(dir.to_string_lossy().to_string())
            })
            .unwrap_or_default();

        let cache_dir = crate::manager::cache_root().to_string_lossy().into_owned();
        let placeholders = Placeholders {
            server_dir: &server_dir,
            cache_dir: &cache_dir,
        };

        // The placeholders are substituted in `args` as well as in `env` and
        // `initialization_options`: `clangd` is handed the resource directory
        // shipped inside its own install, since the copied binary otherwise
        // looks for builtin headers at the absolute path it was built with.
        //
        // `{server_dir}` names a file of Devscriptor's own install (clangd's
        // headers, the TypeScript installed beside volar, ruby-lsp's gems),
        // which a server found on PATH or in an editor's extensions does not
        // have: given `--resource-dir=/usr/resource-dir`, a system clangd
        // finds no `stddef.h`, and given `GEM_HOME=/usr/local`, a system
        // ruby-lsp no gems. Such a server runs without what names one, as its
        // own installer set it up.
        let applies = |template: &str| managed || !template.contains(SERVER_DIR_PLACEHOLDER);
        let mut args: Vec<String> = match source {
            LspSource::UserConfig { args, .. } => args.clone(),
            _ => info
                .map(|i| {
                    i.args
                        .iter()
                        .filter(|a| applies(a))
                        .map(|a| placeholders.apply(a))
                        .collect()
                })
                .unwrap_or_default(),
        };
        if through_ruby {
            args.insert(0, path);
        }

        let mut env: std::collections::HashMap<String, String> = info
            .map(|i| {
                i.env
                    .iter()
                    .filter(|(_, v)| applies(v))
                    .map(|(k, v)| (k.clone(), placeholders.apply(v)))
                    .collect()
            })
            .unwrap_or_default();
        if info.is_some_and(|i| {
            i.source.source_type == crate::manager::registry::SourceType::DotnetTool
        }) && let Some(root) = dotnet_root(
            std::env::var_os("DOTNET_ROOT").is_some(),
            which::which("dotnet").ok(),
        ) {
            env.entry("DOTNET_ROOT".to_string()).or_insert(root);
        }
        if let Some(path) = info.and_then(|i| {
            server_path(
                i.path
                    .iter()
                    .filter(|dir| applies(dir))
                    .map(|dir| PathBuf::from(placeholders.apply(dir))),
                std::env::var_os("PATH"),
            )
        }) {
            env.insert("PATH".to_string(), path);
        }

        let initialization_options =
            info.and_then(|i| i.initialization_options.as_ref())
                .map(|opts| {
                    if managed {
                        placeholders.apply_json(opts)
                    } else {
                        placeholders.apply_json(&without_own_files(opts))
                    }
                });

        // Get language-specific settings from presets
        let (language_id, extensions, heuristics) = self.get_server_metadata(server_id);

        let mut settings = ServerSettings::default();
        if let Some(secs) = info.and_then(|i| i.timeout_seconds) {
            settings.timeout_seconds = secs;
        }

        LspServerConfig {
            language_id,
            command,
            args,
            env,
            working_dir: None,
            extensions,
            heuristics,
            initialization_options,
            settings,
            configuration: info.and_then(|i| i.configuration.clone()),
        }
    }

    /// Get metadata for a server (derived from [`generated_registry`]).
    fn get_server_metadata(
        &self,
        server_id: &str,
    ) -> (String, Vec<String>, Option<ServerHeuristics>) {
        let language_id = generated_registry::language_id(server_id)
            .unwrap_or(server_id)
            .to_string();
        let extensions = generated_registry::extensions(server_id)
            .map(|exts| exts.iter().map(|e| e.to_string()).collect())
            .unwrap_or_default();
        let heuristics = generated_registry::heuristics(server_id);
        (language_id, extensions, heuristics)
    }

    /// Get all available servers from all sources
    #[must_use]
    pub fn scan_all_available(&self) -> Vec<(String, LspSource)> {
        let mut results = Vec::new();
        let server_ids = self.get_all_server_ids();

        for server_id in server_ids {
            if let Some(result) = self.resolve(&server_id) {
                results.push((server_id, result.source));
            }
        }

        results
    }

    /// Get list of all supported server IDs (derived from [`generated_registry`]).
    fn get_all_server_ids(&self) -> Vec<String> {
        generated_registry::SERVER_IDS
            .iter()
            .map(|&s| s.to_string())
            .collect()
    }
}

/// The `PATH` a server is started with where the registry puts `dirs` before
/// the one it inherits (`inherited`), or none where it puts none, or where
/// the result is not text the environment can hold.
fn server_path(
    dirs: impl Iterator<Item = PathBuf>,
    inherited: Option<std::ffi::OsString>,
) -> Option<String> {
    let mut dirs = dirs.peekable();
    dirs.peek()?;
    let inherited: Vec<PathBuf> = inherited
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    std::env::join_paths(dirs.chain(inherited))
        .ok()?
        .into_string()
        .ok()
}

/// The `DOTNET_ROOT` a .NET tool is started with where the environment sets
/// none: the directory of the `dotnet` found on PATH, links followed. A
/// tool's launcher finds the runtime by `DOTNET_ROOT`, or in the system-wide
/// locations only, so one installed in the user's home (`~/.dotnet`, where
/// dotnet-install.sh puts it) would not be found by a server started without
/// the profile that sets `DOTNET_ROOT`, though `dotnet` is on its PATH.
fn dotnet_root(set: bool, dotnet: Option<PathBuf>) -> Option<String> {
    if set {
        return None;
    }
    let dotnet = dunce::canonicalize(dotnet?).ok()?;
    Some(dotnet.parent()?.to_string_lossy().into_owned())
}

/// `value` without the strings that name a file of Devscriptor's own install
/// ([`SERVER_DIR_PLACEHOLDER`]), dropped from the objects and arrays holding
/// them.
fn without_own_files(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let names_own_file =
        |v: &Value| matches!(v, Value::String(s) if s.contains(SERVER_DIR_PLACEHOLDER));
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !names_own_file(v))
                .map(|(k, v)| (k.clone(), without_own_files(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|v| !names_own_file(v))
                .map(without_own_files)
                .collect(),
        ),
        other => other.clone(),
    }
}

impl Default for LspResolver {
    fn default() -> Self {
        Self::new()
            .unwrap_or_else(|_| Self::with_bundled_dir(PathBuf::from("./bundle/lsp-servers")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `path` as an executable is named here: with `.exe` on Windows, where
    /// only a suffixed file qualifies.
    fn executable(path: PathBuf) -> PathBuf {
        if cfg!(windows) {
            path.with_extension("exe")
        } else {
            path
        }
    }

    #[test]
    fn test_lsp_source_path() {
        let path = PathBuf::from("/usr/bin/rust-analyzer");
        let source = LspSource::SystemWide(path.clone());
        assert_eq!(source.path(), path);
    }

    #[test]
    fn test_lsp_source_type() {
        assert_eq!(LspSource::Bundled(PathBuf::new()).source_type(), "bundled");
        assert_eq!(
            LspSource::SystemWide(PathBuf::new()).source_type(),
            "system"
        );
    }

    #[test]
    fn test_resolver_new() {
        let resolver = LspResolver::new();
        assert!(resolver.is_ok());
    }

    #[test]
    fn test_get_server_metadata() {
        let resolver = LspResolver::with_bundled_dir(PathBuf::from("/tmp"));
        let (lang, exts, _) = resolver.get_server_metadata("rust-analyzer");
        assert_eq!(lang, "rust");
        assert!(exts.contains(&"rs".to_string()));
    }

    #[test]
    fn test_resolves_devscriptor_installed_server() {
        // A server installed into the managed install dir must be discovered
        // as `Installed` (Tier 2 — on-demand install), not `Bundled` (Tier 2b).
        // Use `with_dirs` so the two tiers are independent: install_dir has
        // the fake binary; bundled_dir is nonexistent.
        let tmp = std::env::temp_dir().join(format!("ds-lsp-installed-{}", std::process::id()));
        let server = "rust-analyzer";
        let bin_dir = tmp.join(server).join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = executable(bin_dir.join(server));
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();

        let resolver = LspResolver::with_dirs(tmp.clone(), PathBuf::from("/nonexistent/bundled"));
        let result = resolver
            .resolve(server)
            .expect("installed server should resolve");
        assert!(
            matches!(result.source, LspSource::Installed(_)),
            "expected Installed, got {:?}",
            result.source
        );
        assert_eq!(result.source.path(), bin);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `{server_dir}` names a file of Devscriptor's own install, which a
    /// server found on PATH or in an editor's extensions does not have: it is
    /// launched without what names one.
    #[test]
    fn a_server_found_elsewhere_is_not_pointed_at_an_installs_files() {
        let tmp = tempfile::tempdir().unwrap();
        let resolver = LspResolver::with_dirs(tmp.path().to_path_buf(), tmp.path().to_path_buf());

        let system = resolver.build_config(
            "clangd",
            &LspSource::SystemWide(PathBuf::from("/usr/bin/clangd")),
        );
        assert_eq!(
            system.args,
            ["--compile-commands-dir={compile_commands_dir}"]
        );
        let installed = resolver.build_config(
            "clangd",
            &LspSource::Installed(PathBuf::from("/srv/lsp/clangd/bin/clangd")),
        );
        assert_eq!(
            installed.args[0],
            "--resource-dir=/srv/lsp/clangd/resource-dir"
        );

        let ruby = resolver.build_config(
            "ruby-lsp",
            &LspSource::SystemWide(PathBuf::from("/usr/local/bin/ruby-lsp-launcher")),
        );
        assert!(!ruby.env.contains_key("GEM_HOME"), "{:?}", ruby.env);

        let ts = resolver.build_config(
            "typescript-language-server",
            &LspSource::VsCodeExtension(PathBuf::from(
                "/home/u/.vscode/extensions/ts/node_modules/.bin/typescript-language-server",
            )),
        );
        let options = serde_json::to_string(&ts.initialization_options).unwrap();
        assert!(!options.contains("node_modules/typescript"), "{options}");
    }

    /// FsAutoComplete installed by Devscriptor finds the Fantomas installed
    /// beside it first on its PATH; one found elsewhere runs with the PATH it
    /// inherits.
    #[test]
    fn a_server_finds_the_tools_installed_beside_it_on_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let resolver = LspResolver::with_dirs(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        let installed = resolver.build_config(
            "fsautocomplete",
            &LspSource::Installed(PathBuf::from("/srv/lsp/fsautocomplete/bin/fsautocomplete")),
        );
        let path = installed.env.get("PATH").expect("a PATH of its own");
        assert_eq!(
            std::env::split_paths(path).next(),
            Some(PathBuf::from("/srv/lsp/fsautocomplete/bin"))
        );
        let elsewhere = resolver.build_config(
            "fsautocomplete",
            &LspSource::SystemWide(PathBuf::from("/home/u/.dotnet/tools/fsautocomplete")),
        );
        assert!(!elsewhere.env.contains_key("PATH"), "{:?}", elsewhere.env);
    }

    /// The registry's directories come before the inherited PATH; with none,
    /// the server inherits it as it is.
    #[test]
    fn a_servers_directories_come_before_the_path_it_inherits() {
        let inherited = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let path = server_path(
            [PathBuf::from("/srv/lsp/fsautocomplete/bin")].into_iter(),
            Some(inherited.clone()),
        )
        .unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            ["/srv/lsp/fsautocomplete/bin", "/usr/bin", "/bin"].map(PathBuf::from)
        );
        assert_eq!(
            server_path(
                [PathBuf::from("/srv/lsp/fsautocomplete/bin")].into_iter(),
                None
            )
            .as_deref()
            .map(PathBuf::from),
            Some(PathBuf::from("/srv/lsp/fsautocomplete/bin"))
        );
        assert_eq!(server_path(std::iter::empty(), Some(inherited)), None);
    }

    /// A .NET tool is told where the `dotnet` on PATH is when the environment
    /// does not say, following the link a system install puts on PATH.
    #[cfg(unix)]
    #[test]
    fn a_dotnet_tool_is_told_where_dotnet_is() {
        let tmp = tempfile::tempdir().unwrap();
        let home = dunce::canonicalize(tmp.path()).unwrap().join(".dotnet");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("dotnet"), "").unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(home.join("dotnet"), bin.join("dotnet")).unwrap();

        assert_eq!(
            dotnet_root(false, Some(bin.join("dotnet"))),
            Some(home.to_string_lossy().into_owned())
        );
        assert_eq!(dotnet_root(true, Some(bin.join("dotnet"))), None);
        assert_eq!(dotnet_root(false, None), None);
    }

    /// A sourcekit-lsp in the managed directory, as `lsp setup` took it from
    /// the mirror, is not resolved: the toolchain's own is.
    #[test]
    fn a_managed_copy_of_a_toolchain_server_is_not_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("sourcekit-lsp").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("sourcekit-lsp"), b"#!/bin/sh\n").unwrap();
        let resolver = LspResolver::with_dirs(tmp.path().to_path_buf(), tmp.path().to_path_buf());
        assert!(resolver.resolve_from_managed("sourcekit-lsp").is_none());
    }

    /// On Windows the `.cmd` npm writes, and the `.bat` jdtls ships, start;
    /// the POSIX shell and Python scripts beside them do not. A gem's
    /// executable runs through `ruby`, so it stays the Ruby script.
    #[test]
    fn windows_starts_the_suffixed_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let touch = |rel: &str| {
            let path = tmp.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            path
        };
        let shim = touch("pyright/node_modules/.bin/pyright-langserver");
        let cmd = touch("pyright/node_modules/.bin/pyright-langserver.cmd");
        touch("pyright/node_modules/.bin/pyright-langserver.ps1");
        assert_eq!(LspResolver::first_executable(&shim, true, false), Some(cmd));
        assert_eq!(
            LspResolver::first_executable(&shim, false, false),
            Some(shim)
        );

        let jdtls = touch("jdtls/bin/jdtls");
        let bat = touch("jdtls/bin/jdtls.bat");
        assert_eq!(
            LspResolver::first_executable(&jdtls, true, false),
            Some(bat)
        );

        let launcher = touch("ruby-lsp/bin/ruby-lsp-launcher");
        touch("ruby-lsp/bin/ruby-lsp-launcher.bat");
        assert_eq!(
            LspResolver::first_executable(&launcher, true, true),
            Some(launcher.clone())
        );

        // The Kotlin LSP's Windows build has only `intellij-server.exe`.
        let exe = touch("kotlin-lsp/bin/intellij-server.exe");
        let bare = tmp.path().join("kotlin-lsp/bin/intellij-server");
        assert_eq!(LspResolver::first_executable(&bare, true, false), Some(exe));
        assert_eq!(LspResolver::first_executable(&bare, false, false), None);
    }

    /// A gem Devscriptor installed runs under this machine's `ruby`, not its
    /// executable's shebang, which names the Ruby the mirror built it with;
    /// and one built for another Ruby, whose native extensions this one cannot
    /// load, is not resolved at all.
    #[test]
    fn a_managed_gem_runs_under_this_ruby_or_not_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let server = "ruby-lsp";
        let bin_dir = tmp.path().join(server).join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("ruby-lsp-launcher");
        std::fs::write(&bin, b"#!/usr/bin/env ruby3.2\n").unwrap();
        let resolver = LspResolver::with_dirs(
            tmp.path().to_path_buf(),
            PathBuf::from("/nonexistent/bundled"),
        );

        let resolved = resolver
            .resolve_from_managed(server)
            .expect("a gem with no native extensions runs on any Ruby");
        assert_eq!(resolved.config.command, "ruby");
        // As paths: the registry's `bin_path` is written with `/`.
        assert_eq!(
            resolved.config.args.first().map(Path::new),
            Some(bin.as_path())
        );
        assert_eq!(
            resolved.config.env.get("GEM_HOME"),
            Some(&tmp.path().join(server).to_string_lossy().into_owned())
        );

        // Built for an API version no Ruby has.
        std::fs::create_dir_all(
            tmp.path()
                .join(server)
                .join("extensions/x86_64-linux/0.0.1"),
        )
        .unwrap();
        if crate::manager::ruby::local_extension_api().is_some() {
            assert!(resolver.resolve_from_managed(server).is_none());
        }
    }

    /// A mirror package unpacked without lifting its top-level `<id>/` folder
    /// still resolves to the binary inside it.
    #[test]
    fn a_nested_mirror_install_resolves_to_its_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let server = "rust-analyzer";
        let bin_dir = tmp.path().join(server).join(server).join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = executable(bin_dir.join(server));
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();

        assert_eq!(
            LspResolver::find_server_binary(tmp.path(), server),
            Some(bin)
        );
    }

    /// That nested folder matches the hand-placed `<id>/<id>` fallback, and is
    /// no binary: spawned as the server, it would fail with "Permission denied".
    #[test]
    fn a_folder_named_like_the_server_is_not_its_binary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("rust-analyzer").join("rust-analyzer")).unwrap();
        assert_eq!(
            LspResolver::find_server_binary(tmp.path(), "rust-analyzer"),
            None
        );
    }

    #[test]
    fn installed_source_type_label() {
        assert_eq!(
            LspSource::Installed(PathBuf::new()).source_type(),
            "installed"
        );
    }

    /// An install recorded at another build than the pinned one is passed
    /// over: the build may not start with this registry's settings.
    #[test]
    fn an_outdated_install_is_passed_over() {
        let tmp = tempfile::tempdir().unwrap();
        let info = LspResolver::registry_info("jdtls").unwrap();
        let bin = executable(tmp.path().join("jdtls").join(&info.bin_path));
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        let resolver = LspResolver::with_dirs(
            tmp.path().to_path_buf(),
            PathBuf::from("/nonexistent/bundled"),
        );
        let from_install = |resolver: &LspResolver| {
            resolver
                .resolve("jdtls")
                .is_some_and(|r| matches!(&r.source, LspSource::Installed(path) if path == &bin))
        };

        std::fs::write(tmp.path().join("jdtls/.version"), "1.44.0").unwrap();
        assert!(!from_install(&resolver));

        let pin = info.pinned_build().unwrap();
        std::fs::write(tmp.path().join("jdtls/.version"), pin).unwrap();
        assert!(from_install(&resolver));
    }
}
