//! The project configuration file, `devscriptor.yaml`.
//!
//! A project keeps here the settings that every command indexing or searching
//! it applies: which files a scan takes in, how references resolve across
//! files, which entities get a vector, and the size of the database pool.
//! `scan`, `watch`, `embed`, `search` and the MCP server read it from the
//! project's root directory, or from the file `--config` names.
//!
//! Only settings that act belong in the file. A field the schema does not name
//! is an error, so a typo is reported instead of passing for a setting.
//!
//! Precedence, highest first: a command's flags (an MCP tool's arguments), the
//! environment (`DEVSCRIPTOR_EMBEDDING_SELECTION`, `DEVSCRIPTOR_DB_POOL_SIZE`),
//! this file, the defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingSelection;
use crate::parser::cross_file::CrossFileMode;
use crate::pipeline::PipelineConfig;
use crate::processing::scanner::is_scannable_extension;

/// The project configuration file's name, in a project's root directory.
pub const PROJECT_CONFIG_FILE: &str = "devscriptor.yaml";

/// A project's settings: the contents of `devscriptor.yaml`. Every field is
/// optional; one left out keeps its default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Which files a scan takes in, and how their references resolve.
    #[serde(default, skip_serializing_if = "ScanSettings::is_unset")]
    pub scan: ScanSettings,
    /// Which entities get a semantic vector.
    #[serde(default, skip_serializing_if = "EmbeddingSettings::is_unset")]
    pub embeddings: EmbeddingSettings,
    /// The database connection pool.
    #[serde(default, skip_serializing_if = "DatabaseSettings::is_unset")]
    pub database: DatabaseSettings,
}

/// `scan:` — which files a scan takes in, and how their references resolve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanSettings {
    /// File extensions to index (`rs`, `py`); none: every supported language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Glob patterns of paths to leave out, relative to the scanned root; a
    /// bare name (`target`) matches at any depth.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Files larger than this many megabytes are left out (default: 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_size_mb: Option<u64>,
    /// Whether `.gitignore` and git's other ignore files apply (default: yes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_gitignore: Option<bool>,
    /// Which references resolve across files: `none`, `light` (type uses and
    /// supertypes), `deep` (calls too) or `auto` (default: `deep` below
    /// 500,000 entities, `light` above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_file: Option<CrossFileMode>,
}

impl ScanSettings {
    fn is_unset(&self) -> bool {
        self == &Self::default()
    }
}

/// `embeddings:` — which entities get a semantic vector.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingSettings {
    /// `all`, `standard` (named code and type units, and documentation
    /// sections; the default) or `significant` (`standard` without bodies
    /// under 96 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<EmbeddingSelection>,
}

impl EmbeddingSettings {
    fn is_unset(&self) -> bool {
        self == &Self::default()
    }
}

/// `database:` — the database connection pool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseSettings {
    /// Connections in the pool (default: twice the CPU count, from 16 to 64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_size: Option<u32>,
}

impl DatabaseSettings {
    fn is_unset(&self) -> bool {
        self == &Self::default()
    }
}

/// A project configuration that could not be read: the file, and what is
/// wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", path.display())]
pub struct ProjectConfigError {
    /// The file.
    pub path: PathBuf,
    /// What is wrong.
    pub message: String,
}

/// A project configuration and the file it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedConfig {
    /// The file.
    pub path: PathBuf,
    /// Its settings.
    pub config: ProjectConfig,
}

/// What [`ProjectConfig::check`] finds: errors make the settings unusable,
/// warnings name settings that may not do what they seem to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    /// Settings that cannot be applied.
    pub errors: Vec<String>,
    /// Settings that apply, but probably not as meant.
    pub warnings: Vec<String>,
}

impl ProjectConfig {
    /// Parse the YAML text of a project configuration. An empty file is the
    /// defaults.
    ///
    /// # Errors
    /// Returns what makes the text no project configuration: invalid YAML, a
    /// field the schema does not name, a value of the wrong kind.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(text).map_err(|e| e.to_string())
    }

    /// Read the project configuration at `path`.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, ProjectConfigError> {
        let error = |message: String| ProjectConfigError {
            path: path.to_path_buf(),
            message,
        };
        let text = std::fs::read_to_string(path).map_err(|e| error(e.to_string()))?;
        Self::parse(&text).map_err(error)
    }

    /// The project configuration a command uses: the file `explicit` names
    /// (`--config`), which must exist, else `devscriptor.yaml` in `dir` if
    /// there is one, else none.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed, or if its
    /// settings cannot be applied ([`Self::check`]'s errors).
    pub fn find(
        explicit: Option<&Path>,
        dir: &Path,
    ) -> Result<Option<LocatedConfig>, ProjectConfigError> {
        let path = match explicit {
            Some(path) => path.to_path_buf(),
            None => {
                let path = dir.join(PROJECT_CONFIG_FILE);
                if !path.is_file() {
                    return Ok(None);
                }
                path
            }
        };
        let config = Self::load(&path)?;
        let findings = config.check();
        if !findings.errors.is_empty() {
            return Err(ProjectConfigError {
                path,
                message: findings.errors.join("; "),
            });
        }
        Ok(Some(LocatedConfig { path, config }))
    }

    /// What is wrong with these settings, or doubtful.
    #[must_use]
    pub fn check(&self) -> Findings {
        let mut findings = Findings::default();
        let scan = &self.scan;
        for extension in &scan.include {
            let bare = extension.trim().trim_start_matches('.');
            if bare.is_empty() {
                findings
                    .errors
                    .push("scan.include: an empty extension".to_string());
            } else if !is_scannable_extension(bare) {
                findings.warnings.push(format!(
                    "scan.include: Devscriptor indexes no `.{bare}` file, so it adds nothing"
                ));
            }
        }
        for pattern in &scan.exclude {
            if pattern.trim().is_empty() {
                findings
                    .errors
                    .push("scan.exclude: an empty pattern".to_string());
            } else if let Err(e) = globset::Glob::new(pattern) {
                findings
                    .errors
                    .push(format!("scan.exclude: `{pattern}` is no glob pattern: {e}"));
            }
        }
        if scan.max_file_size_mb == Some(0) {
            findings
                .errors
                .push("scan.max_file_size_mb: must be at least 1".to_string());
        }
        if self.database.pool_size == Some(0) {
            findings
                .errors
                .push("database.pool_size: must be at least 1".to_string());
        }
        findings
    }

    /// `config` with these settings applied: the scan scope, the cross-file
    /// mode, the embedding selection and the pool size. The environment still
    /// decides the embedding selection and the pool size where it sets them;
    /// a command applies its flags afterwards.
    pub fn apply_to(&self, config: &mut PipelineConfig) {
        let scan = &self.scan;
        if !scan.include.is_empty() {
            config.include_extensions = scan.include.clone();
        }
        if !scan.exclude.is_empty() {
            config.exclude_patterns = scan.exclude.clone();
        }
        if let Some(megabytes) = scan.max_file_size_mb {
            config.max_file_size = megabytes.saturating_mul(1024 * 1024);
        }
        if let Some(use_gitignore) = scan.use_gitignore {
            config.use_gitignore = use_gitignore;
        }
        if let Some(mode) = scan.cross_file {
            config.cross_file_mode = mode;
        }
        config.embedding_selection = self.embedding_selection();
        if self.database.pool_size.is_some() {
            config.database.pool_size = self.database.pool_size;
        }
    }

    /// The embedding selection: `DEVSCRIPTOR_EMBEDDING_SELECTION` where it is
    /// set, else this file's, else [`EmbeddingSelection::Standard`].
    #[must_use]
    pub fn embedding_selection(&self) -> EmbeddingSelection {
        EmbeddingSelection::from_env_or(self.embeddings.selection.unwrap_or_default())
    }

    /// The settings as the YAML of a `devscriptor.yaml`, headed by `comment`.
    ///
    /// # Errors
    /// Returns an error if the settings cannot be serialized.
    pub fn to_yaml(&self, comment: &str) -> Result<String, serde_yaml::Error> {
        let mut yaml = String::new();
        for line in comment.lines() {
            yaml.push_str("# ");
            yaml.push_str(line);
            yaml.push('\n');
        }
        if *self == Self::default() {
            // `{}` rather than nothing: the file says it holds no setting.
            yaml.push_str("{}\n");
        } else {
            yaml.push_str(&serde_yaml::to_string(self)?);
        }
        Ok(yaml)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_parses_and_applies() {
        let config = ProjectConfig::parse(
            "scan:\n  include: [rs, .py]\n  exclude: [\"vendor/**\", target]\n  \
             max_file_size_mb: 2\n  use_gitignore: false\n  cross_file: light\n\
             embeddings:\n  selection: significant\ndatabase:\n  pool_size: 8\n",
        )
        .unwrap();
        assert!(config.check().errors.is_empty());

        let mut pipeline = PipelineConfig::default();
        config.apply_to(&mut pipeline);
        assert_eq!(pipeline.include_extensions, ["rs", ".py"]);
        assert_eq!(pipeline.exclude_patterns, ["vendor/**", "target"]);
        assert_eq!(pipeline.max_file_size, 2 * 1024 * 1024);
        assert!(!pipeline.use_gitignore);
        assert_eq!(pipeline.cross_file_mode, CrossFileMode::Light);
        if std::env::var_os("DEVSCRIPTOR_EMBEDDING_SELECTION").is_none() {
            assert_eq!(
                pipeline.embedding_selection,
                EmbeddingSelection::Significant
            );
        }
        assert_eq!(pipeline.database.pool_size, Some(8));
    }

    #[test]
    fn an_empty_file_changes_nothing() {
        let config = ProjectConfig::parse("").unwrap();
        assert_eq!(config, ProjectConfig::default());
        let mut pipeline = PipelineConfig::default();
        let before = format!("{pipeline:?}");
        config.apply_to(&mut pipeline);
        assert_eq!(format!("{pipeline:?}"), before);
    }

    /// A field the schema does not name is an error: it is reported, not
    /// ignored.
    #[test]
    fn an_unknown_field_is_an_error() {
        for text in [
            "database:\n  url: sqlite://x.db\n",
            "logging:\n  level: debug\n",
            "scan:\n  includes: [rs]\n",
            "embeddings:\n  model: other\n",
        ] {
            let error = ProjectConfig::parse(text).unwrap_err();
            assert!(error.contains("unknown field"), "{text}: {error}");
        }
        let error = ProjectConfig::parse("scan:\n  cross_file: sideways\n").unwrap_err();
        assert!(error.contains("unknown variant"), "{error}");
    }

    #[test]
    fn checking_finds_unusable_and_doubtful_settings() {
        let config = ProjectConfig::parse(
            "scan:\n  include: [rs, docx, \"\"]\n  exclude: [\"src/[x\"]\n  max_file_size_mb: 0\n\
             database:\n  pool_size: 0\n",
        )
        .unwrap();
        let findings = config.check();
        assert_eq!(findings.errors.len(), 4, "{findings:?}");
        assert_eq!(findings.warnings.len(), 1, "{findings:?}");
        assert!(findings.warnings[0].contains(".docx"));
    }

    #[test]
    fn the_file_is_found_in_the_project_or_where_config_points() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ProjectConfig::find(None, dir.path()).unwrap(), None);

        let path = dir.path().join(PROJECT_CONFIG_FILE);
        std::fs::write(&path, "scan:\n  include: [go]\n").unwrap();
        let found = ProjectConfig::find(None, dir.path()).unwrap().unwrap();
        assert_eq!(found.path, path);
        assert_eq!(found.config.scan.include, ["go"]);

        let other = dir.path().join("other.yaml");
        assert!(ProjectConfig::find(Some(&other), dir.path()).is_err());
        std::fs::write(&other, "database:\n  pool_size: 0\n").unwrap();
        let error = ProjectConfig::find(Some(&other), dir.path()).unwrap_err();
        assert!(error.to_string().contains("pool_size"), "{error}");
    }

    #[test]
    fn the_yaml_written_reads_back_the_same() {
        let config = ProjectConfig {
            scan: ScanSettings {
                include: vec!["ts".into(), "tsx".into()],
                exclude: vec!["node_modules".into()],
                max_file_size_mb: Some(5),
                use_gitignore: None,
                cross_file: Some(CrossFileMode::Auto),
            },
            embeddings: EmbeddingSettings {
                selection: Some(EmbeddingSelection::All),
            },
            database: DatabaseSettings::default(),
        };
        let yaml = config.to_yaml("written by a test").unwrap();
        assert!(yaml.starts_with("# written by a test\n"), "{yaml}");
        assert!(!yaml.contains("database"), "{yaml}");
        assert_eq!(ProjectConfig::parse(&yaml).unwrap(), config);

        let empty = ProjectConfig::default().to_yaml("nothing").unwrap();
        assert_eq!(
            ProjectConfig::parse(&empty).unwrap(),
            ProjectConfig::default()
        );
    }
}
