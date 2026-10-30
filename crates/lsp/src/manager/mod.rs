//! LSP Server Manager - On-demand installation and management
//!
//! Provides functionality to:
//! - List available LSP servers from the registry
//! - Install servers on-demand
//! - Manage installed servers
//! - Detect project types and suggest appropriate servers

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::lsp_architecture::prerequisites::{Prerequisite, check_prerequisites_async};

mod downloader;
mod installer;
pub mod mirror;
pub mod registry;
pub(crate) mod ruby;

/// Devscriptor's cache directory (`~/.cache/devscriptor/`, falling back to the
/// system temp dir): installed servers, and the state servers keep between
/// runs (the registry's `{cache_dir}`).
pub(crate) fn cache_root() -> PathBuf {
    crate::user_dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("devscriptor")
}

/// Canonical directory under which Devscriptor installs LSP servers on demand
/// (`~/.cache/devscriptor/lsp-servers/`).
///
/// Installation and the 4-tier discovery resolver share this helper so a server
/// installed by `lsp setup` is found again on the next run.
pub(crate) fn install_root() -> PathBuf {
    cache_root().join("lsp-servers")
}

pub use downloader::{DownloadProgress, download_from_mirror, download_server};
pub use installer::InstallationResult;
pub use registry::{ServerInfo, ServerRegistry, SourceType};

/// Manager for LSP server installation and lifecycle
pub struct LspManager {
    registry: ServerRegistry,
    install_dir: PathBuf,
}

impl LspManager {
    /// Create a new LSP manager with default install directory
    pub fn new() -> Result<Self> {
        let install_dir = Self::default_install_dir()?;
        Self::with_install_dir(install_dir)
    }

    /// Create a new LSP manager with custom install directory
    pub fn with_install_dir(install_dir: PathBuf) -> Result<Self> {
        // Ensure install directory exists
        std::fs::create_dir_all(&install_dir).with_context(|| {
            format!(
                "Failed to create install directory: {}",
                install_dir.display()
            )
        })?;

        let registry = ServerRegistry::load()?;

        Ok(Self {
            registry,
            install_dir,
        })
    }

    /// Get the default installation directory
    fn default_install_dir() -> Result<PathBuf> {
        Ok(install_root())
    }

    /// List all available servers from the registry
    #[must_use]
    pub fn list_available(&self) -> Vec<&ServerInfo> {
        self.registry.list_servers()
    }

    /// List all installed servers
    pub fn list_installed(&self) -> Result<Vec<InstalledServer>> {
        let mut installed = Vec::new();

        for entry in std::fs::read_dir(&self.install_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let server_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Check if this is a known server
                if let Some(info) = self.registry.get_server(&server_id) {
                    let version = self.get_installed_version(&server_id)?;
                    installed.push(InstalledServer {
                        id: server_id,
                        name: info.name.clone(),
                        version,
                        path: path.clone(),
                        languages: info.languages.clone(),
                    });
                }
            }
        }

        Ok(installed)
    }

    /// Check if a server is installed, at the build its registry entry pins
    /// if it pins one ([`Self::is_outdated`]).
    #[must_use]
    pub fn is_installed(&self, server_id: &str) -> bool {
        let server_dir = self.install_dir.join(server_id);
        server_dir.exists()
            && server_dir.join(".installed").exists()
            && !self.is_outdated(server_id)
            && self.built_for_another_ruby(server_id).is_none()
    }

    /// The Ruby the gem install of `server_id` was built for, when it is not
    /// this machine's: its native extensions cannot load here (a package built
    /// with Ruby 3.2 does not load under Ruby 3.3). [`Self::install`] replaces
    /// it.
    #[must_use]
    pub fn built_for_another_ruby(&self, server_id: &str) -> Option<String> {
        let info = self.registry.get_server(server_id)?;
        if info.source.source_type != registry::SourceType::Gem {
            return None;
        }
        ruby::built_for_another_ruby(
            &self.install_dir.join(server_id),
            ruby::local_extension_api(),
        )
    }

    /// Whether [`Self::install`] installs `server_id`: it is not installed,
    /// its launcher fetches the libraries it runs on
    /// ([`Self::fetches_its_libraries`]), or it was installed without the
    /// tools it runs beside it ([`Self::lacks_companion`]).
    #[must_use]
    pub fn needs_install(&self, server_id: &str) -> bool {
        !self.is_installed(server_id)
            || self.fetches_its_libraries(server_id)
            || self.lacks_companion(server_id).is_some()
    }

    /// The first of the tools installed beside `server_id` that its install
    /// lacks (FsAutoComplete's Fantomas, which it formats with):
    /// [`Self::install`] installs it again, with them.
    #[must_use]
    pub fn lacks_companion(&self, server_id: &str) -> Option<&str> {
        let info = self.registry.get_server(server_id)?;
        let server_dir = self.install_dir.join(server_id);
        if !server_dir.join(".installed").exists() {
            return None;
        }
        info.lacks_companion(&server_dir)
    }

    /// Whether the Coursier launcher of `server_id` is a thin one, which
    /// fetches the libraries it runs on into Coursier's cache at its first
    /// start: [`Self::install`] replaces it with one that holds them. It still
    /// runs where it can download them.
    #[must_use]
    pub fn fetches_its_libraries(&self, server_id: &str) -> bool {
        let Some(info) = self.registry.get_server(server_id) else {
            return false;
        };
        let launcher = self.install_dir.join(server_id).join(&info.bin_path);
        info.source.source_type == registry::SourceType::Coursier
            && launcher.is_file()
            && !downloader::holds_its_libraries(&launcher)
    }

    /// Whether the install of `server_id` records another version than the
    /// build its registry entry pins: one made for an earlier registry, which
    /// [`Self::install`] replaces.
    #[must_use]
    pub fn is_outdated(&self, server_id: &str) -> bool {
        let recorded = self.get_installed_version(server_id).ok().flatten();
        self.registry
            .get_server(server_id)
            .is_some_and(|info| info.outdated_by(recorded.as_deref()))
    }

    /// Get the installed version of a server
    fn get_installed_version(&self, server_id: &str) -> Result<Option<String>> {
        let version_file = self.install_dir.join(server_id).join(".version");
        if version_file.exists() {
            let version = std::fs::read_to_string(version_file)?;
            Ok(Some(version.trim().to_string()))
        } else {
            Ok(None)
        }
    }

    /// Get the path to the server binary
    #[must_use]
    pub fn get_server_path(&self, server_id: &str) -> Option<PathBuf> {
        if !self.is_installed(server_id) {
            return None;
        }

        let server_info = self.registry.get_server(server_id)?;
        let bin_path = self.install_dir.join(server_id).join(&server_info.bin_path);

        if bin_path.exists() {
            Some(bin_path)
        } else {
            None
        }
    }

    /// Install a server by ID
    pub async fn install(
        &self,
        server_id: &str,
        progress: Option<&dyn DownloadProgress>,
    ) -> Result<InstallationResult> {
        let server_info = self
            .registry
            .get_server(server_id)
            .context(format!("Unknown server: {server_id}"))?;

        info!("Installing LSP server: {}", server_info.name);

        // A server whose builds expire is installed at its latest build, and
        // an install of an earlier one is replaced: it stops starting some
        // weeks after its release. Where the latest cannot be found, an
        // install is kept as it is.
        let latest;
        let mut superseded = None;
        let server_info = if server_info.builds_expire && server_info.tracks_latest() {
            match downloader::latest_version(server_info).await {
                Ok(version) => {
                    let recorded = self.get_installed_version(server_id).ok().flatten();
                    if self.is_installed(server_id) && recorded.as_deref() != Some(version.as_str())
                    {
                        superseded = Some(recorded.unwrap_or_else(|| "?".into()));
                    }
                    latest = server_info.at_version(&version);
                    &latest
                }
                Err(e) if self.is_installed(server_id) => {
                    warn!(
                        "Keeping the installed {}: its latest build could not be found: {e:#}",
                        server_info.name
                    );
                    return Ok(InstallationResult::AlreadyInstalled);
                }
                Err(e) => return Err(e),
            }
        } else {
            server_info
        };

        if superseded.is_none() && !self.needs_install(server_id) {
            return Ok(InstallationResult::AlreadyInstalled);
        }

        // A server linked in from elsewhere — a maintainer's mirror build, a
        // package manager's install — is set up outside Devscriptor and stays
        // as it is: replacing it would remove the link and put a download in its place.
        let server_dir = self.install_dir.join(server_id);
        if let Some(target) = link_target(&server_dir) {
            let why = self
                .built_for_another_ruby(server_id)
                .map_or_else(String::new, |ruby| {
                    format!(" It was built for Ruby {ruby}, which this machine does not run.")
                });
            return Ok(InstallationResult::Failed(format!(
                "{} is a link to {}, set up outside Devscriptor, so it is left as it is.{why} \
                 Update it there, or remove the link for Devscriptor to install its own.",
                server_dir.display(),
                target.display()
            )));
        }

        // The prerequisites `lsp setup` checks and reports (one list for both),
        // looked up without blocking the executor.
        let prerequisites = check_prerequisites_async(server_id).await;
        if !prerequisites.is_satisfied() {
            let missing: Vec<String> = prerequisites
                .missing
                .iter()
                .map(Prerequisite::label)
                .collect();
            return Ok(InstallationResult::MissingPrerequisite(missing.join(", ")));
        }

        // An install of another build than the pinned or the latest one, one
        // without its companions, or one built for another Ruby, is replaced,
        // not added to: its files would mix with the new build's.
        if let Some(recorded) = superseded {
            info!(
                "Replacing {} {recorded} with its latest build, {}",
                server_info.name,
                server_info.source.version.as_deref().unwrap_or("?")
            );
            std::fs::remove_dir_all(&server_dir)?;
        } else if let Some(companion) = self.lacks_companion(server_id) {
            info!(
                "Replacing {}, installed without {companion}, with one beside it",
                server_info.name
            );
            std::fs::remove_dir_all(&server_dir)?;
        } else if self.is_outdated(server_id) {
            let recorded = self.get_installed_version(server_id).ok().flatten();
            info!(
                "Replacing {} {} with the pinned {}",
                server_info.name,
                recorded.as_deref().unwrap_or("?"),
                server_info.pinned_build().unwrap_or("?")
            );
            std::fs::remove_dir_all(&server_dir)?;
        } else if let Some(built_for) = self.built_for_another_ruby(server_id) {
            info!(
                "Replacing {}, built for Ruby {built_for}, with one for this Ruby",
                server_info.name
            );
            std::fs::remove_dir_all(&server_dir)?;
        }

        // Create server directory
        let created = !server_dir.exists();
        std::fs::create_dir_all(&server_dir)?;

        // Download and install. A failed one leaves no partial install the
        // resolver could take for a server: it looks for the binary, not for
        // the `.installed` marker.
        if let Err(e) = downloader::download_server(server_info, &server_dir, progress).await {
            if created {
                let _ = std::fs::remove_dir_all(&server_dir);
            }
            return Err(e);
        }

        // Mark as installed
        std::fs::write(server_dir.join(".installed"), "")?;

        // Save version info
        if let Some(ref version) = server_info.source.version {
            std::fs::write(server_dir.join(".version"), version)?;
        }

        info!("Successfully installed {}", server_info.name);
        Ok(InstallationResult::Success)
    }

    /// Detect which servers are appropriate for a project
    #[must_use]
    pub fn detect_for_project(&self, path: &Path) -> Vec<&ServerInfo> {
        let mut detected: Vec<&ServerInfo> = Vec::new();

        // Helper to avoid adding the same `&ServerInfo` twice by comparing pointers.
        let already_detected =
            |d: &[&ServerInfo], s: &ServerInfo| d.iter().any(|existing| std::ptr::eq(*existing, s));

        // Check for common project files and map to languages
        let project_files: HashMap<&str, Vec<&str>> = [
            ("Cargo.toml", vec!["rust"]),
            ("package.json", vec!["typescript", "javascript"]),
            ("go.mod", vec!["go"]),
            ("requirements.txt", vec!["python"]),
            ("pyproject.toml", vec!["python"]),
            ("setup.py", vec!["python"]),
            ("pom.xml", vec!["java"]),
            ("build.gradle", vec!["java", "kotlin"]),
            ("Gemfile", vec!["ruby"]),
            ("composer.json", vec!["php"]),
            ("*.fsproj", vec!["fsharp"]),
            ("*.csproj", vec!["csharp"]),
            ("pubspec.yaml", vec!["dart"]),
            ("Package.swift", vec!["swift"]),
            ("*.scala", vec!["scala"]),
            ("build.sbt", vec!["scala"]),
        ]
        .into_iter()
        .collect();

        // Check each project file
        for (pattern, languages) in project_files {
            if pattern.contains('*') {
                // Glob pattern - check if any matching files exist
                if Self::has_matching_files(path, pattern) {
                    for lang in languages {
                        if let Some(server) = self.registry.find_by_language(lang)
                            && !already_detected(&detected, server)
                        {
                            detected.push(server);
                        }
                    }
                }
            } else if path.join(pattern).exists() {
                for lang in languages {
                    if let Some(server) = self.registry.find_by_language(lang)
                        && !already_detected(&detected, server)
                    {
                        detected.push(server);
                    }
                }
            }
        }

        // Also check file extensions in the project
        if let Ok(entries) = std::fs::read_dir(path) {
            let mut found_languages = std::collections::HashSet::new();

            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    let lang = match ext.as_str() {
                        "rs" => "rust",
                        "go" => "go",
                        "py" => "python",
                        "js" => "javascript",
                        "ts" => "typescript",
                        "tsx" => "typescript",
                        "java" => "java",
                        "kt" => "kotlin",
                        "scala" => "scala",
                        "rb" => "ruby",
                        "php" => "php",
                        "c" | "h" => "c",
                        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
                        "swift" => "swift",
                        "dart" => "dart",
                        "fs" | "fsx" => "fsharp",
                        "cs" => "csharp",
                        "sh" | "bash" => "bash",
                        "sql" => "sql",
                        "vue" => "vue",
                        "svelte" => "svelte",
                        "html" => "html",
                        "graphql" | "gql" => "graphql",
                        "sol" => "solidity",
                        "vb" => "vb",
                        "cu" | "cuh" => "cuda",
                        "m" => "objective-c",
                        "mm" => "objective-cpp",
                        _ => continue,
                    };

                    if found_languages.insert(lang)
                        && let Some(server) = self.registry.find_by_language(lang)
                        && !already_detected(&detected, server)
                    {
                        detected.push(server);
                    }
                }
            }
        }

        detected
    }

    /// Check if any files match a glob pattern
    fn has_matching_files(path: &Path, pattern: &str) -> bool {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    // Simple glob matching
                    if pattern.starts_with("*.") {
                        let ext = &pattern[1..]; // Remove *
                        if name.ends_with(ext) {
                            return true;
                        }
                    } else if name == pattern {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Update an installed server
    pub async fn update(&self, server_id: &str) -> Result<InstallationResult> {
        if !self.is_installed(server_id) {
            return Ok(InstallationResult::NotInstalled);
        }

        // For now, just reinstall
        // In a real implementation, we'd check for updates first
        let server_dir = self.install_dir.join(server_id);
        std::fs::remove_dir_all(&server_dir)?;

        self.install(server_id, None).await
    }

    /// Uninstall a server
    pub fn uninstall(&self, server_id: &str) -> Result<()> {
        if !self.is_installed(server_id) {
            return Ok(());
        }

        let server_dir = self.install_dir.join(server_id);
        std::fs::remove_dir_all(&server_dir)?;

        info!("Uninstalled server: {}", server_id);
        Ok(())
    }

    /// Get the total size of installed servers
    pub fn get_total_size(&self) -> Result<u64> {
        let mut total = 0u64;

        for server in self.list_installed()? {
            total += Self::dir_size(&server.path)?;
        }

        Ok(total)
    }

    /// Calculate directory size recursively
    fn dir_size(path: &Path) -> Result<u64> {
        let mut size = 0u64;

        for entry in walkdir::WalkDir::new(path) {
            let entry = entry?;
            if entry.file_type().is_file() {
                size += entry.metadata()?.len();
            }
        }

        Ok(size)
    }
}

/// Information about an installed server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledServer {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub languages: Vec<String>,
}

/// Where `path` links to, when it is a symbolic link.
/// Why a server does not run here, as its install shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    /// Whether there is an install of it here, one that cannot run.
    pub installed: bool,
    /// Why, and what makes it run, said after the server's name: "is not
    /// installed: `devscriptor lsp setup …` installs it".
    pub reason: String,
}

impl Unavailable {
    /// Why the registry's server `info`, which does not resolve, does not
    /// run here, as its install under `root` shows: it was built for another
    /// Ruby, or is another build than the one pinned, or is not installed,
    /// or holds no binary. For a server linked in from elsewhere, which
    /// `devscriptor lsp setup` leaves alone, the fix is where the link was
    /// made.
    #[must_use]
    pub fn of(root: &std::path::Path, info: &ServerInfo) -> Self {
        Self::from_install(root, info, ruby::local_extension_api())
    }

    /// [`Self::of`], with `local_ruby` as this machine's Ruby.
    fn from_install(root: &std::path::Path, info: &ServerInfo, local_ruby: Option<&str>) -> Self {
        let id = info.id.as_str();
        let dir = root.join(id);
        let linked = link_target(&dir);
        let fix = |there: &str, here: String| match &linked {
            Some(target) => format!(
                "; it is a link to {}, set up outside Devscriptor: {there}",
                target.display()
            ),
            None => format!(": {here}"),
        };
        if info.source.source_type == SourceType::Gem
            && let Some(built_for) = ruby::built_for_another_ruby(&dir, local_ruby)
        {
            return Self {
                installed: true,
                reason: format!(
                    "was built for Ruby {built_for}, and this machine runs Ruby {}, so it cannot \
                     load its native extensions{}",
                    local_ruby.unwrap_or("?"),
                    fix(
                        "rebuild it there for this Ruby",
                        format!("`devscriptor lsp setup {id}` installs it for this Ruby")
                    )
                ),
            };
        }
        let recorded = std::fs::read_to_string(dir.join(".version")).ok();
        if info.outdated_by(recorded.as_deref()) {
            return Self {
                installed: true,
                reason: format!(
                    "is build {}, not the {} this Devscriptor pins{}",
                    recorded.as_deref().map_or("", str::trim),
                    info.pinned_build().unwrap_or("?"),
                    fix(
                        "update it there",
                        format!("`devscriptor lsp setup {id}` replaces it")
                    )
                ),
            };
        }
        if std::fs::symlink_metadata(&dir).is_err() {
            return Self {
                installed: false,
                reason: format!("is not installed: `devscriptor lsp setup {id}` installs it"),
            };
        }
        Self {
            installed: true,
            reason: format!(
                "is installed at {}, but holds no `{}`{}",
                dir.display(),
                info.bin_path,
                fix(
                    "fix it there",
                    format!("remove it, and `devscriptor lsp setup {id}` installs it again")
                )
            ),
        }
    }
}

/// Why the registry's server `server_id` does not run here, as its install
/// under Devscriptor's install root shows ([`Unavailable::of`]); `None` for
/// a server the registry does not know.
#[must_use]
pub fn why_unavailable(server_id: &str) -> Option<Unavailable> {
    ServerRegistry::entry(server_id).map(|info| Unavailable::of(&install_root(), info))
}

fn link_target(path: &std::path::Path) -> Option<PathBuf> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.file_type().is_symlink())
        .and_then(|_| std::fs::read_link(path).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// A server linked in from elsewhere stays as it is: `install` neither
    /// removes the link nor downloads, and says why.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_server_is_left_as_it_is() {
        let temp = TempDir::new().unwrap();
        let mirror = TempDir::new().unwrap();
        std::fs::write(mirror.path().join("marker"), "mirror").unwrap();
        let manager = LspManager::with_install_dir(temp.path().to_path_buf()).unwrap();
        let link = temp.path().join("csharp-ls");
        std::os::unix::fs::symlink(mirror.path(), &link).unwrap();

        // The linked build has no `.installed`: not installed, yet not
        // replaced — and it is said before any prerequisite is checked.
        let result = manager.install("csharp-ls", None).await.unwrap();
        assert!(
            matches!(&result, InstallationResult::Failed(why) if why.contains("left as it is")),
            "{result:?}"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(mirror.path().join("marker")).unwrap(),
            "mirror"
        );
    }

    /// The reason a server does not run is its install's: missing; holding
    /// no binary; another build than the pinned one; built for another Ruby,
    /// with both versions. Each says what makes it run, and for a server
    /// linked in from elsewhere, which setup leaves alone, that the fix is
    /// where the link was made.
    #[test]
    fn the_reason_a_server_does_not_run_is_its_installs() {
        let root = TempDir::new().unwrap();
        let entry = |id| ServerRegistry::entry(id).unwrap();
        let why = |id| Unavailable::from_install(root.path(), entry(id), Some("3.3.0"));

        let missing = why("ruby-lsp");
        assert!(!missing.installed);
        assert_eq!(
            missing.reason,
            "is not installed: `devscriptor lsp setup ruby-lsp` installs it"
        );

        std::fs::create_dir_all(root.path().join("gopls")).unwrap();
        let empty = why("gopls");
        assert!(empty.installed);
        assert!(
            empty.reason.ends_with(
                "but holds no `bin/gopls`: remove it, and `devscriptor lsp setup gopls` installs \
                 it again"
            ),
            "{}",
            empty.reason
        );

        std::fs::create_dir_all(root.path().join("jdtls")).unwrap();
        std::fs::write(root.path().join("jdtls/.version"), "1.40.0\n").unwrap();
        assert_eq!(
            why("jdtls").reason,
            "is build 1.40.0, not the 1.61.0 this Devscriptor pins: `devscriptor lsp setup \
             jdtls` replaces it"
        );

        let build = TempDir::new().unwrap();
        std::fs::create_dir_all(build.path().join("extensions/x86_64-linux/3.2.0/prism")).unwrap();
        std::fs::rename(build.path(), root.path().join("ruby-lsp")).unwrap();
        let rebuilt = why("ruby-lsp");
        assert!(rebuilt.installed);
        assert_eq!(
            rebuilt.reason,
            "was built for Ruby 3.2.0, and this machine runs Ruby 3.3.0, so it cannot load its \
             native extensions: `devscriptor lsp setup ruby-lsp` installs it for this Ruby"
        );
        #[cfg(unix)]
        {
            let mirror = root.path().join("mirror-ruby-lsp");
            std::fs::rename(root.path().join("ruby-lsp"), &mirror).unwrap();
            std::os::unix::fs::symlink(&mirror, root.path().join("ruby-lsp")).unwrap();
            assert_eq!(
                why("ruby-lsp").reason,
                format!(
                    "was built for Ruby 3.2.0, and this machine runs Ruby 3.3.0, so it cannot \
                     load its native extensions; it is a link to {}, set up outside \
                     Devscriptor: rebuild it there for this Ruby",
                    mirror.display()
                )
            );
        }
    }

    #[test]
    fn test_default_install_dir() {
        let dir = LspManager::default_install_dir().unwrap();
        assert!(dir.to_string_lossy().contains("devscriptor"));
        assert!(dir.to_string_lossy().contains("lsp-servers"));
    }

    #[test]
    fn test_detect_rust_project() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = temp.path().join("Cargo.toml");
        let mut file = std::fs::File::create(&cargo_toml).unwrap();
        writeln!(file, "[package]").unwrap();
        writeln!(file, "name = \"test\"").unwrap();

        let manager = LspManager::new().unwrap();
        let detected = manager.detect_for_project(temp.path());

        assert!(!detected.is_empty());
        assert!(
            detected
                .iter()
                .any(|s| s.languages.contains(&"rust".to_string()))
        );
    }

    #[test]
    fn test_detect_node_project() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        let mut file = std::fs::File::create(&package_json).unwrap();
        writeln!(file, "{{}}").unwrap();

        let manager = LspManager::new().unwrap();
        let detected = manager.detect_for_project(temp.path());

        assert!(!detected.is_empty());
        assert!(
            detected
                .iter()
                .any(|s| s.languages.contains(&"typescript".to_string()))
        );
    }

    /// A pinned server installed at another build is not installed as far as
    /// `lsp setup` goes: it is replaced.
    #[test]
    fn an_outdated_install_is_not_installed() {
        let temp = TempDir::new().unwrap();
        let manager = LspManager::with_install_dir(temp.path().to_path_buf()).unwrap();
        let dir = temp.path().join("jdtls");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".installed"), "").unwrap();

        std::fs::write(dir.join(".version"), "1.44.0").unwrap();
        assert!(manager.is_outdated("jdtls"));
        assert!(!manager.is_installed("jdtls"));

        let pin = manager
            .registry
            .get_server("jdtls")
            .and_then(ServerInfo::pinned_build)
            .unwrap()
            .to_string();
        std::fs::write(dir.join(".version"), &pin).unwrap();
        assert!(!manager.is_outdated("jdtls"));
        assert!(manager.is_installed("jdtls"));
    }

    /// A thin Coursier launcher fetches the libraries it runs on at its first
    /// start: `lsp setup` replaces it with one that holds them. A server of
    /// another source is no launcher.
    #[test]
    fn a_launcher_fetching_its_libraries_is_replaced() {
        let temp = TempDir::new().unwrap();
        let manager = LspManager::with_install_dir(temp.path().to_path_buf()).unwrap();
        let bin = temp.path().join("metals/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(temp.path().join("metals/.installed"), "").unwrap();
        std::fs::write(temp.path().join("metals/.version"), "1.6.7").unwrap();

        std::fs::write(
            bin.join("metals"),
            "#!/usr/bin/env sh\nPK coursier/bootstrap/launcher/bootstrap-jar-urls",
        )
        .unwrap();
        assert!(manager.is_installed("metals"));
        assert!(manager.fetches_its_libraries("metals"));
        assert!(manager.needs_install("metals"));

        std::fs::write(
            bin.join("metals"),
            "#!/usr/bin/env sh\nPK coursier/bootstrap/launcher/jars/metals_2.13-1.6.7.jar",
        )
        .unwrap();
        assert!(!manager.fetches_its_libraries("metals"));
        assert!(!manager.needs_install("metals"));

        let jdtls = temp.path().join("jdtls/bin");
        std::fs::create_dir_all(&jdtls).unwrap();
        std::fs::write(jdtls.join("jdtls"), "#!/usr/bin/env python3\n").unwrap();
        assert!(!manager.fetches_its_libraries("jdtls"));
    }
}
