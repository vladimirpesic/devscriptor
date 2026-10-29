//! Tech Stack Detection Module
//!
//! Tells what a project is built with from its marker files (or, without
//! any, from its dominant file extension), and suggests the project
//! configuration that suits it: `devscriptor template analyze`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::project::{ProjectConfig, ScanSettings};
use crate::processing::scanner::is_scannable_extension;
use crate::utils::gitignore_filter::GitignoreFilter;

/// Represents a detected technology stack
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TechStack {
    /// Rust project
    Rust,
    /// Node.js/JavaScript project
    NodeJs,
    /// Python project
    Python,
    /// Java project with Maven
    JavaMaven,
    /// Java project with Gradle
    JavaGradle,
    /// Go project
    Go,
    /// Ruby project
    Ruby,
    /// PHP project
    Php,
    /// Swift project
    Swift,
    /// Kotlin project
    Kotlin,
    /// Scala project
    Scala,
    /// C#/.NET project
    DotNet,
    /// C/C++ project
    Cpp,
    /// Mobile project (React Native, Flutter, etc.)
    Mobile,
    /// Web project (generic)
    Web,
    /// Data science project
    DataScience,
    /// DevOps/Infrastructure project
    DevOps,
    /// Game development project
    GameDev,
    /// Embedded systems project
    Embedded,
    /// Unknown or generic project
    Unknown,
}

impl TechStack {
    /// Get a human-readable name for the stack
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            TechStack::Rust => "Rust",
            TechStack::NodeJs => "Node.js",
            TechStack::Python => "Python",
            TechStack::JavaMaven => "Java (Maven)",
            TechStack::JavaGradle => "Java (Gradle)",
            TechStack::Go => "Go",
            TechStack::Ruby => "Ruby",
            TechStack::Php => "PHP",
            TechStack::Swift => "Swift",
            TechStack::Kotlin => "Kotlin",
            TechStack::Scala => "Scala",
            TechStack::DotNet => "C#/.NET",
            TechStack::Cpp => "C/C++",
            TechStack::Mobile => "Mobile",
            TechStack::Web => "Web",
            TechStack::DataScience => "Data Science",
            TechStack::DevOps => "DevOps/Infrastructure",
            TechStack::GameDev => "Game Development",
            TechStack::Embedded => "Embedded Systems",
            TechStack::Unknown => "Unknown",
        }
    }

    /// The built-in template that suits this stack: one whose scan takes in
    /// the stack's files. None suits Rust, Go, Ruby, PHP or .NET.
    #[must_use]
    pub fn recommended_template(&self) -> Option<&'static str> {
        match self {
            TechStack::NodeJs | TechStack::Web => Some("web_development"),
            TechStack::Python => Some("python_services"),
            TechStack::JavaMaven | TechStack::JavaGradle | TechStack::Scala => {
                Some("enterprise_java")
            }
            TechStack::Swift | TechStack::Kotlin | TechStack::Mobile => Some("mobile_development"),
            TechStack::Cpp | TechStack::Embedded => Some("embedded_systems"),
            TechStack::DataScience => Some("data_science"),
            TechStack::DevOps => Some("devops_infrastructure"),
            TechStack::GameDev => Some("game_development"),
            TechStack::Rust
            | TechStack::Go
            | TechStack::Ruby
            | TechStack::Php
            | TechStack::DotNet
            | TechStack::Unknown => None,
        }
    }

    /// Get default file extensions for this stack
    #[must_use]
    pub fn default_extensions(&self) -> Vec<&'static str> {
        match self {
            TechStack::Rust => vec!["rs"],
            TechStack::NodeJs => vec!["js", "jsx", "ts", "tsx", "mjs"],
            TechStack::Python => vec!["py", "pyi", "pyx"],
            TechStack::JavaMaven | TechStack::JavaGradle => vec!["java", "kt", "scala"],
            TechStack::Go => vec!["go"],
            TechStack::Ruby => vec!["rb", "erb"],
            TechStack::Php => vec!["php"],
            TechStack::Swift => vec!["swift"],
            TechStack::Kotlin => vec!["kt", "kts"],
            TechStack::Scala => vec!["scala", "sc"],
            TechStack::DotNet => vec!["cs", "csx", "vb"],
            TechStack::Cpp => vec!["c", "cpp", "cc", "h", "hpp"],
            TechStack::Mobile => vec!["swift", "kt", "dart", "java"],
            TechStack::Web => vec!["html", "css", "js", "ts"],
            TechStack::DataScience => vec!["py", "ipynb"],
            TechStack::DevOps => vec!["yaml", "yml", "tf", "hcl", "sh"],
            TechStack::GameDev => vec!["cs", "cpp", "gd", "unity"],
            TechStack::Embedded => vec!["c", "cpp", "h", "rs"],
            TechStack::Unknown => vec![],
        }
    }

    /// Get default exclude patterns for this stack
    #[must_use]
    pub fn default_excludes(&self) -> Vec<&'static str> {
        match self {
            TechStack::Rust => vec!["target", "Cargo.lock"],
            TechStack::NodeJs => vec!["node_modules", "dist", "build", ".next"],
            TechStack::Python => vec!["__pycache__", ".venv", "venv", "*.pyc", ".pytest_cache"],
            TechStack::JavaMaven => vec!["target", ".m2"],
            TechStack::JavaGradle => vec!["build", ".gradle"],
            TechStack::Go => vec!["vendor"],
            TechStack::Ruby => vec!["vendor", ".bundle"],
            TechStack::Php => vec!["vendor", "composer.lock"],
            TechStack::Swift => vec![".build", "Packages"],
            TechStack::Kotlin => vec!["build", ".gradle"],
            TechStack::Scala => vec!["target", ".bloop", ".metals"],
            TechStack::DotNet => vec!["bin", "obj"],
            TechStack::Cpp => vec!["build", "cmake-build-*"],
            TechStack::Mobile => vec!["Pods", "build", ".gradle"],
            TechStack::Web => vec!["node_modules", "dist", "build"],
            TechStack::DataScience => vec![".ipynb_checkpoints", "__pycache__"],
            TechStack::DevOps => vec![".terraform", "*.tfstate*"],
            TechStack::GameDev => vec!["Library", "Temp", "obj", "Build"],
            TechStack::Embedded => vec!["build", "target"],
            TechStack::Unknown => vec![],
        }
    }
}

impl std::fmt::Display for TechStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Tech stack detector that analyzes project files
#[derive(Debug, Clone)]
pub struct StackDetector {
    /// Marker files in the order their stacks are preferred, each with the
    /// stack it marks. `*.<ext>` stands for any file with that extension in
    /// the project's root.
    markers: Vec<(String, TechStack)>,
}

impl Default for StackDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StackDetector {
    /// Create a new stack detector with default marker files: a language's
    /// build files first, then those of a kind of project (mobile, embedded,
    /// game, data science, DevOps), which many projects of every language
    /// have besides.
    #[must_use]
    pub fn new() -> Self {
        let markers = [
            ("Cargo.toml", TechStack::Rust),
            ("package.json", TechStack::NodeJs),
            ("package-lock.json", TechStack::NodeJs),
            ("yarn.lock", TechStack::NodeJs),
            ("pnpm-lock.yaml", TechStack::NodeJs),
            ("requirements.txt", TechStack::Python),
            ("setup.py", TechStack::Python),
            ("pyproject.toml", TechStack::Python),
            ("Pipfile", TechStack::Python),
            ("poetry.lock", TechStack::Python),
            ("pom.xml", TechStack::JavaMaven),
            ("build.gradle", TechStack::JavaGradle),
            ("build.gradle.kts", TechStack::JavaGradle),
            ("build.sbt", TechStack::Scala),
            ("go.mod", TechStack::Go),
            ("go.sum", TechStack::Go),
            ("Gemfile", TechStack::Ruby),
            ("Gemfile.lock", TechStack::Ruby),
            ("composer.json", TechStack::Php),
            ("composer.lock", TechStack::Php),
            ("Package.swift", TechStack::Swift),
            ("*.csproj", TechStack::DotNet),
            ("*.sln", TechStack::DotNet),
            ("CMakeLists.txt", TechStack::Cpp),
            ("pubspec.yaml", TechStack::Mobile),
            ("Podfile", TechStack::Mobile),
            ("platformio.ini", TechStack::Embedded),
            ("*.ino", TechStack::Embedded),
            ("project.godot", TechStack::GameDev),
            ("*.uproject", TechStack::GameDev),
            ("*.ipynb", TechStack::DataScience),
            ("*.tf", TechStack::DevOps),
        ];
        Self {
            markers: markers
                .into_iter()
                .map(|(marker, stack)| (marker.to_string(), stack))
                .collect(),
        }
    }

    /// Add custom marker files to the detector (G4). They are preferred to
    /// the built-in ones, and replace a built-in marker of the same name.
    #[must_use]
    pub fn with_custom_markers(mut self, custom: Vec<(String, TechStack)>) -> Self {
        self.markers
            .retain(|(marker, _)| !custom.iter().any(|(name, _)| name == marker));
        self.markers.splice(0..0, custom);
        self
    }

    /// Detect the tech stack of a project
    ///
    /// # Arguments
    ///
    /// * `project_path` - Path to the project directory
    ///
    /// # Returns
    ///
    /// The stack of the first marker the project has, else the one its
    /// dominant file extension tells, else `TechStack::Unknown`.
    pub fn detect(&self, project_path: &Path) -> TechStack {
        self.markers
            .iter()
            .find(|(marker, _)| Self::has_marker(project_path, marker))
            .map(|(_, stack)| stack.clone())
            .or_else(|| self.detect_from_extensions(project_path))
            .unwrap_or(TechStack::Unknown)
    }

    /// Detect multiple stacks (for polyglot projects)
    ///
    /// # Arguments
    ///
    /// * `project_path` - Path to the project directory
    ///
    /// # Returns
    ///
    /// The stacks of every marker the project has, preferred first; without
    /// any, the one [`Self::detect`] tells.
    pub fn detect_all(&self, project_path: &Path) -> Vec<TechStack> {
        let mut stacks: Vec<TechStack> = Vec::new();
        for (marker, stack) in &self.markers {
            if !stacks.contains(stack) && Self::has_marker(project_path, marker) {
                stacks.push(stack.clone());
            }
        }
        if stacks.is_empty() {
            stacks.push(self.detect(project_path));
        }
        stacks
    }

    /// Whether the project has `marker`: the file itself, or for `*.<ext>`
    /// a file with that extension in the project's root.
    fn has_marker(project_path: &Path, marker: &str) -> bool {
        let Some(extension) = marker.strip_prefix("*.") else {
            return project_path.join(marker).exists();
        };
        std::fs::read_dir(project_path).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
            })
        })
    }

    /// Detect stack from file extensions
    ///
    /// Returns the dominant tech stack only when it accounts for at least
    /// 50 % of recognized source files. This prevents mis-classifying
    /// polyglot projects.
    fn detect_from_extensions(&self, project_path: &Path) -> Option<TechStack> {
        let extension_counts = self.count_extensions(project_path);

        if extension_counts.is_empty() {
            return None;
        }

        // Rank extensions by count, ties by name, so that the result is the
        // same at every run.
        let mut ranked: Vec<(&String, &usize)> = extension_counts.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

        let dominant = ranked.first()?;
        let total: usize = ranked.iter().map(|(_, c)| *c).sum();

        // Require at least 50 % dominance to avoid mis-classifying polyglot projects
        if *dominant.1 * 2 < total {
            return None;
        }

        match dominant.0.as_str() {
            "rs" => Some(TechStack::Rust),
            "py" => Some(TechStack::Python),
            "js" | "ts" | "jsx" | "tsx" => Some(TechStack::NodeJs),
            "java" => Some(TechStack::JavaMaven),
            "go" => Some(TechStack::Go),
            "rb" => Some(TechStack::Ruby),
            "php" => Some(TechStack::Php),
            "swift" => Some(TechStack::Swift),
            "kt" => Some(TechStack::Kotlin),
            "scala" | "sc" => Some(TechStack::Scala),
            "cs" => Some(TechStack::DotNet),
            "c" | "cpp" | "cc" => Some(TechStack::Cpp),
            _ => None,
        }
    }

    /// Count file extensions in a directory
    fn count_extensions(&self, project_path: &Path) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        let filter = GitignoreFilter::new(project_path).ok();
        self.count_extensions_recursive(project_path, &mut counts, 0, 3, filter.as_ref());
        counts
    }

    fn count_extensions_recursive(
        &self,
        dir: &Path,
        counts: &mut HashMap<String, usize>,
        depth: usize,
        max_depth: usize,
        filter: Option<&GitignoreFilter>,
    ) {
        if depth > max_depth {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(f) = filter
                && f.is_ignored(&path)
            {
                continue;
            }
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    *counts.entry(ext.to_string()).or_insert(0) += 1;
                }
            } else if path.is_dir() {
                // Skip hidden directories and common dependency folders
                if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    if name.starts_with('.')
                        || name == "node_modules"
                        || name == "target"
                        || name == "__pycache__"
                        || name == "vendor"
                        || name == "dist"
                        || name == "build"
                    {
                        continue;
                    }
                }
                self.count_extensions_recursive(&path, counts, depth + 1, max_depth, filter);
            }
        }
    }
}

/// Detected stack information with recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackDetectionResult {
    /// Primary detected stack
    pub primary_stack: TechStack,
    /// All detected stacks (for polyglot projects)
    pub all_stacks: Vec<TechStack>,
    /// Recommended template
    pub recommended_template: Option<String>,
    /// The extensions of the detected stacks that a scan takes in
    pub suggested_extensions: Vec<String>,
    /// The exclude patterns of the detected stacks
    pub suggested_excludes: Vec<String>,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
}

impl StackDetectionResult {
    /// A result for `stacks`, the first of them the primary one.
    #[must_use]
    pub fn from_stacks(stacks: Vec<TechStack>) -> Self {
        let primary = stacks.first().cloned().unwrap_or(TechStack::Unknown);
        let mut suggested_extensions: Vec<String> = Vec::new();
        let mut suggested_excludes: Vec<String> = Vec::new();
        for stack in &stacks {
            for extension in stack.default_extensions() {
                if is_scannable_extension(extension)
                    && !suggested_extensions.iter().any(|e| e == extension)
                {
                    suggested_extensions.push(extension.to_string());
                }
            }
            for pattern in stack.default_excludes() {
                if !suggested_excludes.iter().any(|p| p == pattern) {
                    suggested_excludes.push(pattern.to_string());
                }
            }
        }
        Self {
            confidence: if primary == TechStack::Unknown {
                0.0
            } else {
                1.0
            },
            recommended_template: primary.recommended_template().map(str::to_string),
            primary_stack: primary,
            all_stacks: stacks,
            suggested_extensions,
            suggested_excludes,
        }
    }

    /// Detect the stacks of the project at `project_path`.
    #[must_use]
    pub fn detect(project_path: &Path) -> Self {
        Self::from_stacks(StackDetector::new().detect_all(project_path))
    }

    /// The project configuration the detected stacks suggest: their
    /// extensions, and their exclude patterns.
    #[must_use]
    pub fn suggested_config(&self) -> ProjectConfig {
        ProjectConfig {
            scan: ScanSettings {
                include: self.suggested_extensions.clone(),
                exclude: self.suggested_excludes.clone(),
                ..ScanSettings::default()
            },
            ..ProjectConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profiles::ConfigurationTemplates;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_tech_stack_display() {
        assert_eq!(TechStack::Rust.display_name(), "Rust");
        assert_eq!(TechStack::NodeJs.display_name(), "Node.js");
        assert_eq!(TechStack::Python.display_name(), "Python");
    }

    /// A recommended template's scan takes in the stack's files: Rust, Go,
    /// Ruby and PHP are not sent to `web_development`, which leaves them all out.
    #[test]
    fn a_recommended_template_takes_in_the_stacks_files() {
        let templates = ConfigurationTemplates::new();
        for stack in [
            TechStack::Rust,
            TechStack::NodeJs,
            TechStack::Python,
            TechStack::JavaMaven,
            TechStack::JavaGradle,
            TechStack::Go,
            TechStack::Ruby,
            TechStack::Php,
            TechStack::Swift,
            TechStack::Kotlin,
            TechStack::Scala,
            TechStack::DotNet,
            TechStack::Cpp,
            TechStack::Mobile,
            TechStack::Web,
            TechStack::DataScience,
            TechStack::DevOps,
            TechStack::GameDev,
            TechStack::Embedded,
        ] {
            let Some(name) = stack.recommended_template() else {
                continue;
            };
            let include = &templates.apply_template(name).unwrap().scan.include;
            let main = stack
                .default_extensions()
                .into_iter()
                .find(|extension| is_scannable_extension(extension))
                .unwrap();
            assert!(include.iter().any(|e| e == main), "{stack}: {name}");
        }
        assert_eq!(TechStack::Rust.recommended_template(), None);
        assert_eq!(TechStack::Unknown.recommended_template(), None);
    }

    #[test]
    fn test_stack_detector_rust() {
        let temp_dir = TempDir::new().unwrap();
        File::create(temp_dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(
            StackDetector::new().detect(temp_dir.path()),
            TechStack::Rust
        );
    }

    #[test]
    fn test_stack_detector_nodejs() {
        let temp_dir = TempDir::new().unwrap();
        let mut file = File::create(temp_dir.path().join("package.json")).unwrap();
        writeln!(file, "{{}}").unwrap();
        assert_eq!(
            StackDetector::new().detect(temp_dir.path()),
            TechStack::NodeJs
        );
    }

    #[test]
    fn test_stack_detector_python() {
        let temp_dir = TempDir::new().unwrap();
        File::create(temp_dir.path().join("requirements.txt")).unwrap();
        assert_eq!(
            StackDetector::new().detect(temp_dir.path()),
            TechStack::Python
        );
    }

    #[test]
    fn test_stack_detector_unknown() {
        let temp_dir = TempDir::new().unwrap();
        assert_eq!(
            StackDetector::new().detect(temp_dir.path()),
            TechStack::Unknown
        );
    }

    /// A polyglot project has the same primary stack at every run, whatever the
    /// order of the markers, and a `*.csproj` marker is found.
    #[test]
    fn detection_is_the_same_at_every_run_and_finds_project_files() {
        let temp_dir = TempDir::new().unwrap();
        for file in ["package.json", "Cargo.toml", "pyproject.toml", "App.csproj"] {
            File::create(temp_dir.path().join(file)).unwrap();
        }
        let detector = StackDetector::new();
        for _ in 0..5 {
            assert_eq!(detector.detect(temp_dir.path()), TechStack::Rust);
            assert_eq!(
                detector.detect_all(temp_dir.path()),
                [
                    TechStack::Rust,
                    TechStack::NodeJs,
                    TechStack::Python,
                    TechStack::DotNet
                ]
            );
        }
        let custom = detector
            .with_custom_markers(vec![("pyproject.toml".to_string(), TechStack::DataScience)]);
        assert_eq!(custom.detect(temp_dir.path()), TechStack::DataScience);
    }

    #[test]
    fn the_suggestion_covers_every_detected_stack_with_scannable_extensions() {
        let temp_dir = TempDir::new().unwrap();
        File::create(temp_dir.path().join("Cargo.toml")).unwrap();
        File::create(temp_dir.path().join("notebook.ipynb")).unwrap();
        let result = StackDetectionResult::detect(temp_dir.path());
        assert_eq!(result.primary_stack, TechStack::Rust);
        assert_eq!(result.all_stacks, [TechStack::Rust, TechStack::DataScience]);
        let config = result.suggested_config();
        assert_eq!(config.scan.include, ["rs", "py"]);
        assert!(config.scan.exclude.contains(&"target".to_string()));
        assert!(
            config
                .scan
                .exclude
                .contains(&".ipynb_checkpoints".to_string())
        );
        assert_eq!(config.check(), crate::config::project::Findings::default());
    }
}
