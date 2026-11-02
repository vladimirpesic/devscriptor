//! The project configuration a server follows: `devscriptor.yaml` in its
//! workspace, or the file `--config` names.
//!
//! A server lives long (the workspace's daemon serves every session), so the
//! file is read again when it changes: [`ProjectSettings::refresh`] looks at
//! its modification time and size, a `stat`, and the server calls it at every
//! tool call. A file that cannot be applied changes nothing: its error is
//! reported (`health_check`, the log), and the settings stay as they were, the
//! defaults at start and else the last valid ones. The pool size is the one
//! setting a running server cannot take: the database opened with the one in
//! force at start.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use devscriptor_core::config::{PROJECT_CONFIG_FILE, ProjectConfig};
use parking_lot::Mutex;
use tracing::{info, warn};

/// The project configuration a server follows, read again when its file
/// changes.
#[derive(Debug)]
pub struct ProjectSettings {
    /// The file: the one `--config` named, else `devscriptor.yaml` in the
    /// workspace, which need not exist. None for a server with no project.
    path: Option<PathBuf>,
    /// Whether `--config` named the file, which then must exist.
    explicit: bool,
    loaded: Mutex<Loaded>,
}

#[derive(Debug)]
struct Loaded {
    /// The file's modification time and size when last read; none when it
    /// did not exist.
    stamp: Option<(SystemTime, u64)>,
    /// The settings in force.
    config: Arc<ProjectConfig>,
    /// Why the file's settings are not in force, when they are not.
    error: Option<String>,
}

/// What [`ProjectSettings::refresh`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// The file is as when last read.
    Unchanged,
    /// The file changed, and its settings are in force now.
    Applied,
    /// The file changed, but cannot be applied: the settings stay.
    Rejected,
}

impl ProjectSettings {
    /// No project: the defaults, always.
    #[must_use]
    pub fn none() -> Self {
        Self {
            path: None,
            explicit: false,
            loaded: Mutex::new(Loaded {
                stamp: None,
                config: Arc::new(ProjectConfig::default()),
                error: None,
            }),
        }
    }

    /// The configuration of the project in `workspace`: the file `explicit`
    /// names, else `devscriptor.yaml` there; read now.
    #[must_use]
    pub fn for_workspace(workspace: &Path, explicit: Option<&Path>) -> Self {
        let path = match explicit {
            Some(path) => dunce::canonicalize(path)
                .or_else(|_| std::path::absolute(path))
                .unwrap_or_else(|_| path.to_path_buf()),
            None => workspace.join(PROJECT_CONFIG_FILE),
        };
        let settings = Self {
            path: Some(path.clone()),
            explicit: explicit.is_some(),
            ..Self::none()
        };
        {
            let mut loaded = settings.loaded.lock();
            settings.read(&path, stamp(Some(&path)), &mut loaded);
        }
        settings
    }

    /// The file this server follows, if it follows one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The settings in force.
    #[must_use]
    pub fn current(&self) -> Arc<ProjectConfig> {
        Arc::clone(&self.loaded.lock().config)
    }

    /// Read the file again if it changed since it was last read: its
    /// settings come in force if they can be applied.
    pub fn refresh(&self) -> Refresh {
        let Some(path) = self.path.as_deref() else {
            return Refresh::Unchanged;
        };
        let now = stamp(Some(path));
        let mut loaded = self.loaded.lock();
        if now == loaded.stamp {
            return Refresh::Unchanged;
        }
        self.read(path, now, &mut loaded)
    }

    /// Read the file at `path`, as `now` found it, into `loaded`.
    fn read(&self, path: &Path, now: Option<(SystemTime, u64)>, loaded: &mut Loaded) -> Refresh {
        loaded.stamp = now;
        let read = match now {
            // A file `--config` named must exist, as for a command.
            None if self.explicit => Err(format!("{}: no such file", path.display())),
            // The workspace's own file need not: without it, the defaults.
            None => Ok(ProjectConfig::default()),
            Some(_) => ProjectConfig::find(Some(path), Path::new("."))
                .map(|located| located.map(|located| located.config).unwrap_or_default())
                .map_err(|e| e.to_string()),
        };
        match read {
            Ok(config) => {
                for warning in config.check().warnings {
                    warn!("{}: {warning}", path.display());
                }
                if *loaded.config != config || loaded.error.is_some() {
                    info!("Project configuration: {}", path.display());
                }
                loaded.config = Arc::new(config);
                loaded.error = None;
                Refresh::Applied
            }
            Err(e) => {
                warn!("Project configuration not applied: {e}");
                loaded.error = Some(e);
                Refresh::Rejected
            }
        }
    }

    /// The file and why it is not in force, for `health_check`.
    #[must_use]
    pub fn status(&self) -> serde_json::Value {
        let loaded = self.loaded.lock();
        serde_json::json!({
            "path": self.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
            "exists": loaded.stamp.is_some(),
            "error": loaded.error,
        })
    }
}

/// The modification time and size of the file at `path`, if it is one.
fn stamp(path: Option<&Path>) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path?)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    Some((metadata.modified().ok()?, metadata.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file's modification time can stay the same across two quick
    /// writes; the size tells these apart.
    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_change_to_the_file_is_taken_and_a_broken_one_keeps_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECT_CONFIG_FILE);
        let settings = ProjectSettings::for_workspace(dir.path(), None);
        assert_eq!(*settings.current(), ProjectConfig::default());
        assert_eq!(settings.refresh(), Refresh::Unchanged);

        write(&path, "scan:\n  include: [rs]\n");
        assert_eq!(settings.refresh(), Refresh::Applied);
        assert_eq!(settings.current().scan.include, ["rs"]);
        assert_eq!(settings.refresh(), Refresh::Unchanged);

        // Broken mid-edit: the last valid settings stay, and the error shows.
        write(&path, "scan:\n  include: [rs, py\n");
        assert_eq!(settings.refresh(), Refresh::Rejected);
        assert_eq!(settings.current().scan.include, ["rs"]);
        assert!(settings.status()["error"].is_string());

        write(&path, "scan:\n  include: [rs, py]\n");
        assert_eq!(settings.refresh(), Refresh::Applied);
        assert_eq!(settings.current().scan.include, ["rs", "py"]);
        assert!(settings.status()["error"].is_null());

        // Removed: the defaults again.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(settings.refresh(), Refresh::Applied);
        assert_eq!(*settings.current(), ProjectConfig::default());
        assert_eq!(settings.status()["exists"], false);
    }

    #[test]
    fn an_explicit_file_that_does_not_exist_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("elsewhere.yaml");
        let settings = ProjectSettings::for_workspace(dir.path(), Some(&missing));
        assert_eq!(*settings.current(), ProjectConfig::default());
        let error = settings.status()["error"].as_str().unwrap().to_string();
        assert!(error.contains("no such file"), "{error}");
        assert_eq!(settings.path(), Some(missing.as_path()));
    }

    #[test]
    fn no_project_follows_no_file() {
        let settings = ProjectSettings::none();
        assert_eq!(settings.refresh(), Refresh::Unchanged);
        assert_eq!(settings.path(), None);
        assert!(settings.status()["path"].is_null());
    }
}
