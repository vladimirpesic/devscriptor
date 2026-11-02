//! Configuration templates: named project configurations.
//!
//! A template is a [`ProjectConfig`] with a name, a description and the kind
//! of project it suits; `template apply` and `profile apply` write its
//! configuration as a `devscriptor.yaml`. Eight are built in, for web, data
//! science, mobile, embedded, enterprise Java, Python service, game and
//! DevOps projects; a user's own are loaded from a directory over them.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::project::ProjectConfig;

/// A configuration template: a project configuration, with what it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateConfig {
    /// Template name
    pub name: String,
    /// Template description
    #[serde(default)]
    pub description: String,
    /// Template category
    #[serde(default)]
    pub category: String,
    /// Template tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// The project configuration the template applies.
    #[serde(default)]
    pub config: ProjectConfig,
    /// Use cases for this template
    #[serde(default)]
    pub use_cases: Vec<String>,
}

impl TemplateConfig {
    /// Parse a template file's text, YAML when `yaml`, else JSON.
    ///
    /// # Errors
    /// Returns why the text is no template.
    pub fn parse(text: &str, yaml: bool) -> Result<Self, String> {
        if yaml {
            serde_yaml::from_str(text).map_err(|e| e.to_string())
        } else {
            serde_json::from_str(text).map_err(|e| e.to_string())
        }
    }

    /// Create a new template configuration
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            category: category.into(),
            tags: Vec::new(),
            config: ProjectConfig::default(),
            use_cases: Vec::new(),
        }
    }

    /// Add multiple tags
    #[must_use]
    pub fn with_tags(mut self, tags: &[&str]) -> Self {
        self.tags.extend(tags.iter().map(ToString::to_string));
        self
    }

    /// The extensions a scan takes in (none: every supported language).
    #[must_use]
    pub fn with_extensions(mut self, extensions: &[&str]) -> Self {
        self.config.scan.include = extensions.iter().map(ToString::to_string).collect();
        self
    }

    /// The patterns a scan leaves out.
    #[must_use]
    pub fn with_excludes(mut self, patterns: &[&str]) -> Self {
        self.config.scan.exclude = patterns.iter().map(ToString::to_string).collect();
        self
    }

    /// The largest file a scan takes in, in megabytes.
    #[must_use]
    pub fn with_max_file_size_mb(mut self, megabytes: u64) -> Self {
        self.config.scan.max_file_size_mb = Some(megabytes);
        self
    }

    /// Add multiple use cases
    #[must_use]
    pub fn with_use_cases(mut self, use_cases: &[&str]) -> Self {
        self.use_cases
            .extend(use_cases.iter().map(ToString::to_string));
        self
    }
}

/// Template manager providing access to all available templates
pub struct ConfigurationTemplates {
    /// All available templates
    templates: HashMap<String, TemplateConfig>,
}

/// An error loading the templates of a directory.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TemplateError(String);

impl ConfigurationTemplates {
    /// Create a new template manager with all templates initialized
    pub fn new() -> Self {
        let templates = [
            Self::web_development(),
            Self::data_science(),
            Self::mobile_development(),
            Self::embedded_systems(),
            Self::enterprise_java(),
            Self::python_services(),
            Self::game_development(),
            Self::devops_infrastructure(),
        ]
        .into_iter()
        .map(|template| (template.name.clone(), template))
        .collect();
        Self { templates }
    }

    /// Load additional templates from a directory at runtime.
    ///
    /// Reads all `.json` and `.yaml`/`.yml` files from the given path,
    /// deserialises them as [`TemplateConfig`], and merges them into the
    /// existing set. A template with the name of a built-in one replaces it.
    /// A file that is no template is skipped, and named in the error once
    /// the others are loaded.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be read, or names the files
    /// that could not be read or parsed.
    pub fn load_templates_from_dir(&mut self, path: &Path) -> Result<usize, TemplateError> {
        let entries = std::fs::read_dir(path).map_err(|e| {
            TemplateError(format!(
                "Cannot read template dir '{}': {e}",
                path.display()
            ))
        })?;

        let mut loaded = 0usize;
        let mut failures = Vec::new();
        for path in entries.filter_map(|entry| entry.ok().map(|entry| entry.path())) {
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !matches!(ext, "json" | "yaml" | "yml") {
                continue;
            }
            let template = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|contents| TemplateConfig::parse(&contents, ext != "json"));
            match template {
                Ok(template) => {
                    info!(
                        "Loaded runtime template '{}' from {}",
                        template.name,
                        path.display()
                    );
                    self.templates.insert(template.name.clone(), template);
                    loaded += 1;
                }
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }

        if failures.is_empty() {
            Ok(loaded)
        } else {
            Err(TemplateError(failures.join("; ")))
        }
    }

    /// Get a template by name
    pub fn get_template(&self, name: &str) -> Option<&TemplateConfig> {
        self.templates.get(name)
    }

    /// List all available template names, sorted
    pub fn list_templates(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.templates.keys().collect();
        names.sort();
        names
    }

    /// List templates filtered by category
    pub fn list_by_category(&self, category: &str) -> Vec<&TemplateConfig> {
        self.templates
            .values()
            .filter(|t| t.category.eq_ignore_ascii_case(category))
            .collect()
    }

    /// Get all categories
    pub fn get_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self
            .templates
            .values()
            .map(|t| t.category.clone())
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }

    /// Search templates by name, description, or tags
    pub fn search(&self, query: &str) -> Vec<&TemplateConfig> {
        let query = query.to_lowercase();
        self.templates
            .values()
            .filter(|t| {
                t.name.to_lowercase().contains(&query)
                    || t.description.to_lowercase().contains(&query)
                    || t.tags.iter().any(|tag| tag.to_lowercase().contains(&query))
            })
            .collect()
    }

    /// Add a custom template
    pub fn add_template(&mut self, template: TemplateConfig) {
        info!("Added custom template: {}", template.name);
        self.templates.insert(template.name.clone(), template);
    }

    /// Remove a template
    pub fn remove_template(&mut self, name: &str) -> bool {
        let removed = self.templates.remove(name).is_some();
        if removed {
            info!("Removed template: {}", name);
        }
        removed
    }

    /// The project configuration the template `name` applies.
    pub fn apply_template(&self, name: &str) -> Option<&ProjectConfig> {
        self.get_template(name).map(|template| &template.config)
    }

    /// Get the number of available templates
    pub fn count(&self) -> usize {
        self.templates.len()
    }

    /// Check if a template exists
    pub fn has_template(&self, name: &str) -> bool {
        self.templates.contains_key(name)
    }

    // The built-in templates. Their extensions are ones a scan takes in
    // (`validate` warns about any other), and their exclude patterns come on
    // top of those a scan always leaves out (`node_modules`, `target`, …).

    fn web_development() -> TemplateConfig {
        TemplateConfig::new(
            "web_development",
            "Optimized for modern web applications with frontend and backend code",
            "Web Development",
        )
        .with_tags(&[
            "javascript",
            "typescript",
            "react",
            "vue",
            "nodejs",
            "python",
            "django",
        ])
        .with_extensions(&[
            "js", "jsx", "ts", "tsx", "mjs", "cjs", "vue", "svelte", "py", "html", "htm", "css",
            "scss", "sass", "less", "json", "yaml", "yml", "md",
        ])
        .with_excludes(&[
            "node_modules/*",
            "dist/*",
            "build/*",
            ".next/*",
            ".nuxt/*",
            "coverage/*",
            "*.min.js",
            "*.min.css",
            "*.bundle.js",
            "*.chunk.js",
        ])
        .with_max_file_size_mb(5)
        .with_use_cases(&[
            "React/Vue/Angular applications",
            "Node.js backend services",
            "Full-stack web development",
            "Progressive Web Apps",
        ])
    }

    fn data_science() -> TemplateConfig {
        TemplateConfig::new(
            "data_science",
            "Optimized for data science projects with analysis scripts",
            "Data Science",
        )
        .with_tags(&["python", "jupyter", "pandas", "numpy", "ml", "analytics"])
        .with_extensions(&["py", "pyi", "sql", "json", "yaml", "yml", "md"])
        .with_excludes(&[
            "*.pkl",
            "*.pickle",
            "__pycache__/*",
            ".ipynb_checkpoints/*",
            "data/raw/*",
            "models/*",
            "*.h5",
            "*.hdf5",
            "venv/*",
            ".venv/*",
            "conda-env/*",
        ])
        .with_max_file_size_mb(50)
        .with_use_cases(&[
            "Machine learning projects",
            "Data analysis pipelines",
            "Research code",
            "Statistical analysis",
        ])
    }

    fn mobile_development() -> TemplateConfig {
        TemplateConfig::new(
            "mobile_development",
            "Optimized for iOS and Android mobile app development",
            "Mobile Development",
        )
        .with_tags(&[
            "swift",
            "kotlin",
            "react-native",
            "flutter",
            "ios",
            "android",
        ])
        .with_extensions(&[
            "swift", "kt", "kts", "java", "dart", "js", "jsx", "ts", "tsx", "xml", "m", "mm", "h",
        ])
        .with_excludes(&[
            "build/*",
            ".gradle/*",
            "ios/build/*",
            "android/build/*",
            "android/app/build/*",
            "Pods/*",
            "node_modules/*",
            "*.xcworkspace",
            "*.xcodeproj",
            "*.apk",
            "*.ipa",
            "DerivedData/*",
        ])
        .with_max_file_size_mb(10)
        .with_use_cases(&[
            "iOS native apps",
            "Android native apps",
            "React Native apps",
            "Flutter applications",
        ])
    }

    fn embedded_systems() -> TemplateConfig {
        TemplateConfig::new(
            "embedded_systems",
            "Optimized for embedded systems and IoT development with C/C++ focus",
            "Embedded Systems",
        )
        .with_tags(&["c", "cpp", "arduino", "esp32", "raspberry-pi", "iot"])
        .with_extensions(&["c", "cpp", "cc", "cxx", "h", "hpp", "rs", "py"])
        .with_excludes(&[
            "*.o",
            "*.obj",
            "*.elf",
            "*.bin",
            "*.hex",
            "*.map",
            "*.lst",
            "build/*",
            ".vs/*",
            ".settings/*",
            "Core/*",
            "lib/*",
            "libraries/*",
            "dist/*",
            "package/*",
        ])
        .with_max_file_size_mb(2)
        .with_use_cases(&[
            "Arduino projects",
            "ESP32/ESP8266 development",
            "STM32 applications",
            "Raspberry Pi IoT projects",
        ])
    }

    fn enterprise_java() -> TemplateConfig {
        TemplateConfig::new(
            "enterprise_java",
            "Optimized for large-scale enterprise Java applications",
            "Enterprise",
        )
        .with_tags(&[
            "java",
            "spring",
            "maven",
            "gradle",
            "enterprise",
            "microservices",
        ])
        .with_extensions(&[
            "java", "kt", "kts", "scala", "xml", "yml", "yaml", "html", "css", "js", "ts", "sql",
        ])
        .with_excludes(&[
            "target/*",
            "build/*",
            ".gradle/*",
            "node_modules/*",
            ".mvn/*",
            "*.class",
            "*.jar",
            "*.war",
            "*.ear",
            "WEB-INF/lib/*",
            "META-INF/*",
            ".idea/*",
            ".vscode/*",
            "*.log",
        ])
        .with_max_file_size_mb(20)
        .with_use_cases(&[
            "Spring Boot applications",
            "Enterprise microservices",
            "Maven/Gradle projects",
            "Java EE applications",
        ])
    }

    fn python_services() -> TemplateConfig {
        TemplateConfig::new(
            "python_services",
            "Optimized for Python microservices and API development",
            "Backend Services",
        )
        .with_tags(&[
            "python",
            "fastapi",
            "django",
            "flask",
            "microservices",
            "api",
        ])
        .with_extensions(&[
            "py", "pyi", "html", "htm", "xml", "json", "yaml", "yml", "toml", "sql",
        ])
        .with_excludes(&[
            "__pycache__/*",
            "*.pyc",
            "*.pyo",
            ".pytest_cache/*",
            ".tox/*",
            "venv/*",
            ".venv/*",
            "env/*",
            "*.egg-info/*",
            "build/*",
            "dist/*",
            ".coverage",
            "htmlcov/*",
            "*.log",
        ])
        .with_max_file_size_mb(8)
        .with_use_cases(&[
            "FastAPI services",
            "Django applications",
            "Flask APIs",
            "Python microservices",
        ])
    }

    fn game_development() -> TemplateConfig {
        TemplateConfig::new(
            "game_development",
            "Optimized for game development projects with Unity, Unreal, and custom engines",
            "Game Development",
        )
        .with_tags(&["unity", "unreal", "csharp", "cpp", "godot", "gaming"])
        .with_extensions(&[
            "cs", "cpp", "cc", "c", "h", "hpp", "ts", "js", "json", "xml", "yaml", "yml",
        ])
        .with_excludes(&[
            "Library/*",
            "Temp/*",
            "obj/*",
            "*.meta",
            "*.asset",
            "*.prefab",
            "Build/*",
            "Builds/*",
            "DerivedDataCache/*",
            "*.exe",
            "*.dll",
            "*.so",
            "Packages/*",
        ])
        .with_max_file_size_mb(15)
        .with_use_cases(&[
            "Unity games",
            "Unreal Engine projects",
            "Godot games",
            "Custom game engines",
        ])
    }

    fn devops_infrastructure() -> TemplateConfig {
        TemplateConfig::new(
            "devops_infrastructure",
            "Optimized for DevOps scripts, infrastructure as code, and automation",
            "DevOps",
        )
        .with_tags(&[
            "terraform",
            "ansible",
            "docker",
            "kubernetes",
            "ci-cd",
            "automation",
        ])
        .with_extensions(&[
            "yml", "yaml", "json", "toml", "sh", "bash", "zsh", "py", "rb", "go", "rs",
        ])
        .with_excludes(&[
            ".terraform/*",
            "*.tfstate",
            "*.tfstate.backup",
            "node_modules/*",
            ".venv/*",
            "venv/*",
            "*.log",
            "*.pid",
            "secrets/*",
            ".vault/*",
            "*.key",
            "*.pem",
            "*.crt",
        ])
        .with_max_file_size_mb(3)
        .with_use_cases(&[
            "Terraform configurations",
            "Docker/Kubernetes setups",
            "CI/CD pipelines",
            "Infrastructure automation",
        ])
    }
}

impl Default for ConfigurationTemplates {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ConfigurationTemplates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigurationTemplates")
            .field("count", &self.count())
            .field("templates", &self.list_templates())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_templates_count() {
        let templates = ConfigurationTemplates::new();
        assert_eq!(templates.count(), 8);
        assert_eq!(templates.list_templates().len(), 8);
    }

    #[test]
    fn test_get_template() {
        let templates = ConfigurationTemplates::new();
        assert!(templates.get_template("web_development").is_some());
        assert!(templates.get_template("data_science").is_some());
        assert!(templates.get_template("nonexistent").is_none());
    }

    #[test]
    fn test_list_by_category() {
        let templates = ConfigurationTemplates::new();
        let web_templates = templates.list_by_category("Web Development");
        assert_eq!(web_templates.len(), 1);
        assert_eq!(web_templates[0].name, "web_development");
    }

    #[test]
    fn test_get_categories() {
        let templates = ConfigurationTemplates::new();
        let categories = templates.get_categories();
        assert!(categories.contains(&"Web Development".to_string()));
        assert!(categories.contains(&"Data Science".to_string()));
    }

    #[test]
    fn test_search() {
        let templates = ConfigurationTemplates::new();
        let results = templates.search("web");
        assert!(results.iter().any(|t| t.name == "web_development"));
    }

    #[test]
    fn a_template_applies_its_scan_scope() {
        let templates = ConfigurationTemplates::new();
        let config = templates.apply_template("web_development").unwrap();
        assert_eq!(config.scan.max_file_size_mb, Some(5));
        assert!(config.scan.include.contains(&"tsx".to_string()));
        assert!(config.scan.exclude.contains(&"node_modules/*".to_string()));
        assert!(templates.apply_template("nonexistent").is_none());
    }

    /// `validate` warns about an extension no scan takes in: no built-in
    /// template may write one, nor any other doubtful setting.
    #[test]
    fn every_built_in_template_checks_clean() {
        let templates = ConfigurationTemplates::new();
        for name in templates.list_templates() {
            let findings = templates.apply_template(name).unwrap().check();
            assert_eq!(
                findings,
                crate::config::project::Findings::default(),
                "{name}"
            );
        }
    }

    /// A template with a field the schema does not name is reported.
    #[test]
    fn a_mistaken_template_is_reported() {
        for text in [
            r#"{"name": "x", "config": {"scan": {"includes": ["rs"]}}}"#,
            r#"{"name": "x", "use_case": []}"#,
        ] {
            let error = TemplateConfig::parse(text, false).unwrap_err();
            assert!(error.contains("unknown field"), "{text}: {error}");
        }
        let yaml = "name: y\nconfig:\n  scan:\n    include: [go]\n";
        let template = TemplateConfig::parse(yaml, true).unwrap();
        assert_eq!(template.config.scan.include, ["go"]);
    }

    #[test]
    fn test_add_remove_template() {
        let mut templates = ConfigurationTemplates::new();

        let custom = TemplateConfig::new("custom", "A custom template", "Custom");
        templates.add_template(custom);

        assert_eq!(templates.count(), 9);
        assert!(templates.has_template("custom"));

        templates.remove_template("custom");
        assert_eq!(templates.count(), 8);
        assert!(!templates.has_template("custom"));
    }

    #[test]
    fn one_bad_file_keeps_none_of_the_others_from_loading() {
        let dir = tempfile::tempdir().unwrap();
        let good = TemplateConfig::new("good", "A template", "Custom");
        std::fs::write(
            dir.path().join("good.json"),
            serde_json::to_string(&good).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("bad.json"), "{ not json").unwrap();

        let mut templates = ConfigurationTemplates::new();
        let error = templates.load_templates_from_dir(dir.path()).unwrap_err();
        assert!(error.to_string().contains("bad.json"), "{error}");
        assert!(templates.has_template("good"));
    }
}
