//! LSP server configuration types.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Configuration for an LSP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// Language ID (e.g., "rust", "python").
    pub language_id: String,
    /// Command to execute.
    pub command: String,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory override.
    pub working_dir: Option<PathBuf>,
    /// File extensions this server handles.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Heuristics for auto-detection.
    pub heuristics: Option<ServerHeuristics>,
    /// Initialization options.
    pub initialization_options: Option<Value>,
    /// Server-specific settings.
    #[serde(default)]
    pub settings: ServerSettings,
    /// Configuration to send to server.
    pub configuration: Option<Value>,
}

/// Server heuristics for auto-detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHeuristics {
    /// Marker files to look for.
    #[serde(default)]
    pub markers: Vec<String>,
    /// Maximum search depth.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
}

fn default_max_depth() -> usize {
    3
}

/// Server-specific settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// Request timeout in seconds.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Rate limit per minute.
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout_seconds(),
            rate_limit_per_minute: default_rate_limit(),
        }
    }
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_rate_limit() -> u32 {
    120
}

impl LspServerConfig {
    /// Create rust-analyzer configuration.
    #[must_use]
    pub fn rust_analyzer() -> Self {
        Self {
            language_id: "rust".into(),
            command: "rust-analyzer".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["rs".into()],
            heuristics: Some(ServerHeuristics {
                markers: vec!["Cargo.toml".into(), "Cargo.lock".into()],
                max_depth: 3,
            }),
            // Build in `target/rust-analyzer`, off the user's `cargo build` lock.
            initialization_options: Some(serde_json::json!({ "cargo": { "targetDir": true } })),
            settings: ServerSettings::default(),
            configuration: None,
        }
    }

    /// Create pyright configuration.
    #[must_use]
    pub fn pyright() -> Self {
        Self {
            language_id: "python".into(),
            command: "pyright-langserver".into(),
            args: vec!["--stdio".into()],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["py".into(), "pyw".into(), "pyi".into()],
            heuristics: Some(ServerHeuristics {
                markers: vec![
                    "pyproject.toml".into(),
                    "setup.py".into(),
                    "requirements.txt".into(),
                ],
                max_depth: 3,
            }),
            initialization_options: Some(serde_json::json!({
                "settings": { "python": { "analysis": { "typeCheckingMode": "basic" } } }
            })),
            settings: ServerSettings::default(),
            configuration: None,
        }
    }

    /// Create typescript-language-server configuration.
    #[must_use]
    pub fn typescript() -> Self {
        Self {
            language_id: "typescript".into(),
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["ts".into(), "tsx".into(), "js".into(), "jsx".into()],
            heuristics: Some(ServerHeuristics {
                markers: vec!["package.json".into(), "tsconfig.json".into()],
                max_depth: 3,
            }),
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }
    }

    /// Check if this server should be spawned for the given workspace.
    ///
    /// # Arguments
    ///
    /// * `workspace_root` - The workspace root directory
    /// * `max_depth` - Optional override for maximum search depth
    ///
    /// # Returns
    ///
    /// `true` if the server should be spawned, `false` otherwise.
    #[must_use]
    pub fn should_spawn(&self, workspace_root: &Path, max_depth: Option<usize>) -> bool {
        if let Some(heuristics) = &self.heuristics {
            heuristics.is_applicable_recursive(workspace_root, max_depth)
        } else {
            // No project markers (html, sql, bash): only a workspace that holds one
            // of its files starts such a server up front, rather than every workspace,
            // a Node process apiece. A request for any other file still starts it
            // lazily, rooted at that file.
            has_file_with_extension(
                workspace_root,
                &self.extensions,
                max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
            )
        }
    }
}

impl LspServerConfig {
    /// Whether a file of this server's language lies at most `max_depth`
    /// levels into `workspace_root` (`workspace_root` itself being the first).
    #[must_use]
    pub fn has_files_within(&self, workspace_root: &Path, max_depth: Option<usize>) -> bool {
        has_file_with_extension(
            workspace_root,
            &self.extensions,
            max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
        )
    }
}

/// The search depth for a server whose heuristics name none.
const DEFAULT_MAX_DEPTH: usize = 3;

/// Whether a file with one of `extensions` lies at most `max_depth` levels
/// into `root` (`root` itself being the first), outside hidden, dependency and
/// build directories.
fn has_file_with_extension(root: &Path, extensions: &[String], max_depth: usize) -> bool {
    if extensions.is_empty() {
        return false;
    }
    let mut level = vec![root.to_path_buf()];
    for _ in 0..max_depth {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    if !is_skipped_dir(&entry.file_name().to_string_lossy()) {
                        next.push(path);
                    }
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)))
                {
                    return true;
                }
            }
        }
        level = next;
    }
    false
}

impl ServerHeuristics {
    /// Check if heuristics match the workspace root.
    ///
    /// Markers that contain `*` are matched as glob patterns (e.g. `*.csproj`,
    /// `*.sln`), the others as literal paths: a literal join would never match a
    /// glob marker, and the C# and F# servers would never start.
    #[must_use]
    pub fn is_applicable(&self, workspace_root: &Path) -> bool {
        self.markers.iter().any(|marker| {
            if marker.contains('*') || marker.contains('?') || marker.contains('[') {
                // Glob pattern: check if any file in workspace_root matches.
                let pattern = format!("{}/{}", workspace_root.display(), marker);
                glob::glob(&pattern)
                    .ok()
                    .map(|mut g| g.next().is_some())
                    .unwrap_or(false)
            } else {
                workspace_root.join(marker).exists()
            }
        })
    }

    /// Check recursively up to `max_depth` directories.
    ///
    /// Returns `true` if any marker file is found within the specified depth.
    #[must_use]
    pub fn is_applicable_recursive(&self, workspace_root: &Path, max_depth: Option<usize>) -> bool {
        let max_depth = max_depth.unwrap_or(self.max_depth);
        self.find_any_marker_recursive(workspace_root, max_depth)
    }

    /// The directories at most `max_depth` levels into `root` — `root` itself
    /// being the first level — that are projects by these markers, shallowest
    /// first. A project is not searched for more projects inside it.
    ///
    /// Hidden directories and dependency and build trees are skipped: a
    /// package vendored into `node_modules` or `target` carries a marker of its
    /// own but is no project of the workspace.
    #[must_use]
    pub fn project_dirs(&self, root: &Path, max_depth: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut level = vec![root.to_path_buf()];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for dir in level {
                if self.is_applicable(&dir) {
                    found.push(dir);
                    continue;
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut children: Vec<PathBuf> = entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter(|entry| !is_skipped_dir(&entry.file_name().to_string_lossy()))
                    .map(|entry| entry.path())
                    .collect();
                children.sort();
                next.extend(children);
            }
            level = next;
        }
        found
    }

    fn find_any_marker_recursive(&self, dir: &Path, remaining_depth: usize) -> bool {
        if remaining_depth == 0 {
            return false;
        }

        // Check current directory
        if self.is_applicable(dir) {
            return true;
        }

        // Check subdirectories (limited depth)
        if remaining_depth > 1
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_dir()
                    && self.find_any_marker_recursive(&entry.path(), remaining_depth - 1)
                {
                    return true;
                }
            }
        }

        false
    }
}

/// A directory no project search looks into: hidden, or a tree of
/// dependencies or build output.
fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "vendor" | "build" | "dist" | "__pycache__"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A server without project markers (bash, html, sql) starts up front only
    /// for a workspace that holds one of its files.
    #[test]
    fn a_markerless_server_starts_only_where_its_files_are() {
        let dir = TempDir::new().unwrap();
        let mut bash = LspServerConfig::rust_analyzer();
        bash.language_id = "shellscript".into();
        bash.command = "bash-language-server".into();
        bash.extensions = vec!["sh".into()];
        bash.heuristics = None;
        assert!(!bash.should_spawn(dir.path(), Some(3)), "no shell files");

        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/install.sh"), "").unwrap();
        assert!(
            !bash.should_spawn(dir.path(), Some(3)),
            "dependency trees do not count"
        );

        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/deploy.SH"), "").unwrap();
        assert!(bash.should_spawn(dir.path(), Some(3)));
        assert!(!bash.should_spawn(dir.path(), Some(1)), "beyond the depth");
    }

    #[test]
    fn test_lsp_server_config_default_values() {
        let config = LspServerConfig::rust_analyzer();
        assert_eq!(config.language_id, "rust");
        assert_eq!(config.command, "rust-analyzer");
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
    }

    /// The projects inside a root that is none: shallowest first, not looked
    /// into, and never in a dependency or hidden tree.
    #[test]
    fn project_dirs_finds_the_projects_inside_a_root() {
        let root = TempDir::new().unwrap();
        for dir in [
            "services/pricing/nested",
            "tools/cli",
            "node_modules/napi",
            ".cache/x",
            "docs",
        ] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        for manifest in [
            "services/pricing/Cargo.toml",
            "services/pricing/nested/Cargo.toml",
            "tools/cli/Cargo.toml",
            "node_modules/napi/Cargo.toml",
            ".cache/x/Cargo.toml",
        ] {
            std::fs::write(root.path().join(manifest), "").unwrap();
        }
        let heuristics = ServerHeuristics {
            markers: vec!["Cargo.toml".into()],
            max_depth: 3,
        };
        assert_eq!(
            heuristics.project_dirs(root.path(), 3),
            vec![
                root.path().join("services/pricing"),
                root.path().join("tools/cli")
            ]
        );
        assert_eq!(
            heuristics.project_dirs(root.path(), 2),
            Vec::<PathBuf>::new(),
            "two levels reach no project"
        );

        // A root that is a project is the one project.
        std::fs::write(root.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            heuristics.project_dirs(root.path(), 3),
            vec![root.path().to_path_buf()]
        );
    }

    #[test]
    fn test_server_heuristics_is_applicable() {
        let temp_dir = TempDir::new().unwrap();
        let heuristics = ServerHeuristics {
            markers: vec!["Cargo.toml".into()],
            max_depth: 3,
        };

        // Should return false when marker doesn't exist
        assert!(!heuristics.is_applicable(temp_dir.path()));

        // Create marker file
        let marker_path = temp_dir.path().join("Cargo.toml");
        std::fs::File::create(&marker_path).unwrap();

        // Should return true when marker exists
        assert!(heuristics.is_applicable(temp_dir.path()));
    }

    #[test]
    fn test_server_heuristics_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let sub_dir = temp_dir.path().join("subdir");
        std::fs::create_dir(&sub_dir).unwrap();

        let heuristics = ServerHeuristics {
            markers: vec!["test.txt".into()],
            max_depth: 3,
        };

        // Create marker in subdirectory
        let marker_path = sub_dir.join("test.txt");
        std::fs::File::create(&marker_path).unwrap();

        // Should find marker when depth is sufficient
        assert!(heuristics.is_applicable_recursive(temp_dir.path(), Some(2)));

        // Should not find marker when depth is insufficient
        assert!(!heuristics.is_applicable_recursive(temp_dir.path(), Some(0)));
    }

    #[test]
    fn test_server_settings_default() {
        let settings = ServerSettings::default();
        assert_eq!(settings.timeout_seconds, 30);
        assert_eq!(settings.rate_limit_per_minute, 120);
    }

    /// Glob markers (*.csproj, *.fsproj) are expanded, not matched literally
    /// against file names.
    #[test]
    fn glob_markers_match_project_files() {
        let temp_dir = TempDir::new().unwrap();

        let heuristics = ServerHeuristics {
            markers: vec!["*.csproj".into()],
            max_depth: 1,
        };

        // No .csproj file yet → not applicable
        assert!(!heuristics.is_applicable(temp_dir.path()));

        // Create a .csproj file
        std::fs::File::create(temp_dir.path().join("MyApp.csproj")).unwrap();

        // The glob matches
        assert!(
            heuristics.is_applicable(temp_dir.path()),
            "*.csproj glob must match MyApp.csproj"
        );
    }
}
