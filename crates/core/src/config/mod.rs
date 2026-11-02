//! Configuration for Devscriptor projects.
//!
//! - [`project`]: the project configuration file, `devscriptor.yaml`, which
//!   `scan`, `watch`, `embed`, `search` and the MCP server read.
//! - [`profiles`]: configuration templates, named project configurations that
//!   `template apply` and `profile apply` write.
//! - [`stack_detection`]: what a project is built with, and the configuration
//!   that suits it (`template analyze`).

pub mod profiles;
pub mod project;
pub mod stack_detection;

pub use profiles::{ConfigurationTemplates, TemplateConfig, TemplateError};
pub use project::{
    DatabaseSettings, EmbeddingSettings, Findings, LocatedConfig, PROJECT_CONFIG_FILE,
    ProjectConfig, ProjectConfigError, ScanSettings,
};
pub use stack_detection::{StackDetectionResult, StackDetector, TechStack};
