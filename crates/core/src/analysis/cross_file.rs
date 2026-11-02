//! Cross-File Relationship Analyzer for Devscriptor
//!
//! This module provides functionality to analyze relationships between files,
//! dependencies, imports, and cross-file usage patterns.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Helper macro to compile built-in regex patterns.
///
/// Built-in patterns are compile-time literals that have been tested; if one
/// fails to compile it indicates a serious bug in the source code.  We
/// `expect()` so the failure is loud and immediate rather than silently
/// producing wrong results (which would be far harder to debug).
macro_rules! safe_regex {
    ($pattern:expr) => {{
        let p: &str = $pattern;
        match Regex::new(p) {
            Ok(rx) => rx,
            Err(e) => {
                ::tracing::warn!(pattern = p, error = %e, "safe_regex pattern failed to compile");
                panic!("built-in regex pattern must be valid: {e}");
            }
        }
    }};
}

use crate::analysis::api::AnalysisSummary;
use crate::analysis::{Finding, Severity};
use crate::entities::{Class, Function};
use crate::error::Result;

/// Build a single alternation regex from a list of names.
///
/// Given `names = ["foo", "bar"]` and a pattern builder `|n| format!(r"\b{}\s*\(", n)`,
/// produces `\b(foo|bar)\s*\(` so that capture group 1 holds the matched name.
///
/// Returns `None` if the list is empty or the combined pattern exceeds a
/// reasonable size limit (to avoid pathological regex compilation).
fn build_alternation_regex(names: &[&str]) -> Option<Regex> {
    if names.is_empty() {
        return None;
    }

    // Safety limit: don't build a regex from more than 5000 alternations
    // (prevents pathological compilation times on huge codebases).
    const MAX_NAMES: usize = 5000;
    let iter: Vec<&str> = if names.len() > MAX_NAMES {
        names.iter().take(MAX_NAMES).copied().collect()
    } else {
        names.to_vec()
    };

    // Build alternation of escaped names: (name1|name2|...)
    let name_alts: String = iter
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");
    let combined = format!(r"\b({})\s*\(", name_alts);

    match Regex::new(&combined) {
        Ok(re) => Some(re),
        Err(e) => {
            tracing::warn!(
                "Failed to compile alternation regex ({} names): {e}",
                names.len()
            );
            None
        }
    }
}

/// Represents a dependency between files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDependency {
    /// Source file path
    pub source_file: String,
    /// Target file path
    pub target_file: String,
    /// Dependency type (import, include, require, use, entity_reference)
    pub dependency_type: String,
    /// Strength of dependency (0.0 to 1.0)
    pub strength: f64,
    /// Entities that create this dependency
    pub entities: Vec<String>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Represents a cluster of related files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCluster {
    /// Cluster ID
    pub cluster_id: usize,
    /// Files in the cluster
    pub files: Vec<String>,
    /// Cohesion score (0.0 to 1.0)
    pub cohesion_score: f64,
    /// Coupling score (0.0 to 1.0)
    pub coupling_score: f64,
    /// Common patterns found in the cluster
    pub common_patterns: Vec<String>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Comprehensive cross-file analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossFileReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// File dependencies
    pub dependencies: Vec<FileDependency>,
    /// File clusters
    pub clusters: Vec<FileCluster>,
    /// Architecture layers
    pub architecture_layers: HashMap<String, Vec<String>>,
    /// Circular dependencies
    pub circular_dependencies: Vec<(String, String)>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

impl CrossFileReport {
    /// Create a new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            dependencies: Vec::new(),
            clusters: Vec::new(),
            architecture_layers: HashMap::new(),
            circular_dependencies: Vec::new(),
            recommendations: Vec::new(),
        }
    }

    /// Add a dependency to the report
    pub fn add_dependency(&mut self, dependency: FileDependency) {
        self.dependencies.push(dependency);
    }

    /// Convert to findings for integration with analysis API
    #[must_use]
    pub fn to_findings(&self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Report circular dependencies
        for (source, target) in &self.circular_dependencies {
            findings.push(
                Finding::new(
                    Severity::High,
                    format!("Circular dependency: {source} -> {target}"),
                    source,
                    1,
                    "circular_dependency",
                    "dependency",
                )
                .with_suggestion("Refactor to break the circular dependency cycle"),
            );
        }

        // Report highly coupled files
        let mut dep_counts: HashMap<String, usize> = HashMap::new();
        for dep in &self.dependencies {
            *dep_counts.entry(dep.source_file.clone()).or_insert(0) += 1;
        }

        for (file, count) in dep_counts {
            if count > 10 {
                findings.push(
                    Finding::new(
                        Severity::Medium,
                        format!("High coupling: {file} has {count} dependencies"),
                        &file,
                        1,
                        &file,
                        "file",
                    )
                    .with_suggestion(
                        "Consider breaking the file into smaller, more focused modules",
                    ),
                );
            }
        }

        findings
    }
}

impl Default for CrossFileReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Analyzer for cross-file relationships and dependencies
/// Configuration for a single architectural layer.
#[derive(Debug, Clone)]
pub struct LayerPattern {
    /// Substrings to match in the full file path (case-insensitive).
    pub path_patterns: Vec<String>,
    /// Substrings to match in the file basename (case-insensitive).
    pub basename_patterns: Vec<String>,
}

impl LayerPattern {
    /// Create a new layer pattern.
    #[must_use]
    pub fn new(path_patterns: Vec<String>, basename_patterns: Vec<String>) -> Self {
        Self {
            path_patterns,
            basename_patterns,
        }
    }
}

/// Analyzes relationships between files in a codebase.
#[derive(Debug)]
pub struct CrossFileAnalyzer {
    /// Dependency patterns for different languages
    dependency_patterns: HashMap<String, HashMap<String, Vec<Regex>>>,
    /// File type to language mapping
    file_type_mapping: HashMap<String, String>,
    /// Configurable architectural layer patterns (layer name → patterns).
    layer_patterns: Vec<(String, LayerPattern)>,
}

impl Default for CrossFileAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossFileAnalyzer {
    /// Create a new cross-file analyzer with default configurations
    #[must_use]
    pub fn new() -> Self {
        let mut dependency_patterns: HashMap<String, HashMap<String, Vec<Regex>>> = HashMap::new();

        // Python patterns
        let mut python_patterns = HashMap::new();
        python_patterns.insert(
            "import".to_string(),
            vec![
                safe_regex!(r"^import\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)"),
                safe_regex!(
                    r"^from\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)\s+import"
                ),
            ],
        );
        dependency_patterns.insert("python".to_string(), python_patterns);

        // JavaScript/TypeScript patterns
        let mut js_patterns = HashMap::new();
        js_patterns.insert(
            "import".to_string(),
            vec![
                safe_regex!("import\\s+.*?\\s+from\\s+['\"]([^'\"]+)['\"]"),
                safe_regex!("import\\s*['\"]([^'\"]+)['\"]"),
                safe_regex!("require\\s*\\(\\s*['\"]([^'\"]+)['\"]\\)"),
            ],
        );
        dependency_patterns.insert("javascript".to_string(), js_patterns.clone());
        dependency_patterns.insert("typescript".to_string(), js_patterns);

        // Java patterns
        let mut java_patterns = HashMap::new();
        java_patterns.insert(
            "import".to_string(),
            vec![safe_regex!(
                r"import\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)"
            )],
        );
        dependency_patterns.insert("java".to_string(), java_patterns);

        // C/C++ patterns
        let mut cpp_patterns = HashMap::new();
        cpp_patterns.insert(
            "include".to_string(),
            vec![
                safe_regex!(r"#include\s*<([^>]+)>"),
                safe_regex!("#include\\s*\\\"([^\"]+)\\\""),
            ],
        );
        dependency_patterns.insert("cpp".to_string(), cpp_patterns.clone());
        dependency_patterns.insert("c".to_string(), cpp_patterns);

        // C# patterns
        let mut csharp_patterns = HashMap::new();
        csharp_patterns.insert(
            "import".to_string(),
            vec![safe_regex!(
                r"using\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)"
            )],
        );
        dependency_patterns.insert("csharp".to_string(), csharp_patterns);

        // Go patterns
        let mut go_patterns = HashMap::new();
        go_patterns.insert(
            "import".to_string(),
            vec![
                safe_regex!("import\\s*\\\"([^\"]+)\\\""),
                safe_regex!("import\\s*\\(\\s*\\\"([^\"]+)\\\""),
            ],
        );
        dependency_patterns.insert("go".to_string(), go_patterns);

        // Rust patterns
        let mut rust_patterns = HashMap::new();
        rust_patterns.insert(
            "use".to_string(),
            vec![safe_regex!(
                r"use\s+([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)*)"
            )],
        );
        rust_patterns.insert(
            "mod".to_string(),
            vec![safe_regex!(r"mod\s+([a-zA-Z_][a-zA-Z0-9_]*)")],
        );
        dependency_patterns.insert("rust".to_string(), rust_patterns);

        // PHP patterns
        let mut php_patterns = HashMap::new();
        php_patterns.insert(
            "use".to_string(),
            vec![
                safe_regex!(r"use\s+([a-zA-Z_][a-zA-Z0-9_]*(?:\\[a-zA-Z_][a-zA-Z0-9_]*)*)"),
                safe_regex!("require\\s*['\"]([^'\"]+)['\"]"),
                safe_regex!("include\\s*['\"]([^'\"]+)['\"]"),
            ],
        );
        dependency_patterns.insert("php".to_string(), php_patterns);

        // File type mapping
        let mut file_type_mapping = HashMap::new();
        file_type_mapping.insert(".py".to_string(), "python".to_string());
        file_type_mapping.insert(".js".to_string(), "javascript".to_string());
        file_type_mapping.insert(".ts".to_string(), "typescript".to_string());
        file_type_mapping.insert(".jsx".to_string(), "javascript".to_string());
        file_type_mapping.insert(".tsx".to_string(), "typescript".to_string());
        file_type_mapping.insert(".java".to_string(), "java".to_string());
        file_type_mapping.insert(".cpp".to_string(), "cpp".to_string());
        file_type_mapping.insert(".cc".to_string(), "cpp".to_string());
        file_type_mapping.insert(".cxx".to_string(), "cpp".to_string());
        file_type_mapping.insert(".c".to_string(), "c".to_string());
        file_type_mapping.insert(".h".to_string(), "c".to_string());
        file_type_mapping.insert(".hpp".to_string(), "cpp".to_string());
        file_type_mapping.insert(".cs".to_string(), "csharp".to_string());
        file_type_mapping.insert(".go".to_string(), "go".to_string());
        file_type_mapping.insert(".rs".to_string(), "rust".to_string());
        file_type_mapping.insert(".php".to_string(), "php".to_string());
        file_type_mapping.insert(".rb".to_string(), "ruby".to_string());
        file_type_mapping.insert(".swift".to_string(), "swift".to_string());
        file_type_mapping.insert(".kt".to_string(), "kotlin".to_string());
        file_type_mapping.insert(".scala".to_string(), "scala".to_string());
        file_type_mapping.insert(".dart".to_string(), "dart".to_string());

        Self {
            dependency_patterns,
            file_type_mapping,
            layer_patterns: Self::default_layer_patterns(),
        }
    }

    /// Replace the default layer classification patterns with custom ones.
    ///
    /// Each `(layer_name, LayerPattern)` entry is evaluated in order; the
    /// first matching layer wins.
    #[must_use]
    pub fn with_layer_patterns(mut self, patterns: Vec<(String, LayerPattern)>) -> Self {
        self.layer_patterns = patterns;
        self
    }

    /// Returns the built-in default layer patterns.
    #[must_use]
    pub fn default_layer_patterns() -> Vec<(String, LayerPattern)> {
        vec![
            (
                "presentation".to_string(),
                LayerPattern::new(
                    vec![
                        "/view/".to_string(),
                        "/ui/".to_string(),
                        "/frontend/".to_string(),
                        "/web/".to_string(),
                        "/www/".to_string(),
                        "/component/".to_string(),
                        "/page/".to_string(),
                    ],
                    vec![
                        "template".to_string(),
                        "component".to_string(),
                        "page".to_string(),
                    ],
                ),
            ),
            (
                "api".to_string(),
                LayerPattern::new(
                    vec![
                        "/api/".to_string(),
                        "/endpoint/".to_string(),
                        "/route/".to_string(),
                        "/controller/".to_string(),
                    ],
                    vec![
                        "controller".to_string(),
                        "handler".to_string(),
                        "endpoint".to_string(),
                    ],
                ),
            ),
            (
                "business".to_string(),
                LayerPattern::new(
                    vec![
                        "/service/".to_string(),
                        "/business/".to_string(),
                        "/logic/".to_string(),
                        "/domain/".to_string(),
                        "/usecase/".to_string(),
                        "/interactor/".to_string(),
                    ],
                    vec![
                        "service".to_string(),
                        "manager".to_string(),
                        "business".to_string(),
                    ],
                ),
            ),
            (
                "data".to_string(),
                LayerPattern::new(
                    vec![
                        "/data/".to_string(),
                        "/database/".to_string(),
                        "/db/".to_string(),
                        "/repository/".to_string(),
                    ],
                    vec![
                        "model".to_string(),
                        "entity".to_string(),
                        "repository".to_string(),
                        "dao".to_string(),
                    ],
                ),
            ),
            (
                "infrastructure".to_string(),
                LayerPattern::new(
                    vec![
                        "/config/".to_string(),
                        "/util/".to_string(),
                        "/helper/".to_string(),
                        "/infra/".to_string(),
                        "/common/".to_string(),
                    ],
                    vec![
                        "config".to_string(),
                        "util".to_string(),
                        "helper".to_string(),
                        "tool".to_string(),
                    ],
                ),
            ),
            (
                "tests".to_string(),
                LayerPattern::new(
                    vec![
                        "/test/".to_string(),
                        "/spec/".to_string(),
                        "/tests/".to_string(),
                    ],
                    vec!["test".to_string(), "spec".to_string()],
                ),
            ),
            (
                "docs".to_string(),
                LayerPattern::new(
                    vec![
                        "/doc/".to_string(),
                        "/docs/".to_string(),
                        "/documentation/".to_string(),
                    ],
                    vec![".md".to_string(), ".rst".to_string(), ".txt".to_string()],
                ),
            ),
            (
                "config".to_string(),
                LayerPattern::new(
                    vec![],
                    vec![
                        "config".to_string(),
                        "settings".to_string(),
                        ".env".to_string(),
                        ".yml".to_string(),
                        ".yaml".to_string(),
                        ".json".to_string(),
                        ".ini".to_string(),
                    ],
                ),
            ),
        ]
    }

    /// Analyze cross-file relationships and dependencies
    ///
    /// # Arguments
    ///
    /// * `functions` - List of function entities
    /// * `classes` - List of class entities
    /// * `files_content` - Dictionary mapping file paths to their content
    ///
    /// # Returns
    ///
    /// A `CrossFileReport` containing comprehensive cross-file analysis
    pub fn analyze(
        &self,
        functions: &[Function],
        classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Result<CrossFileReport> {
        let mut report = CrossFileReport::new();

        // Extract file dependencies
        let dependencies = self.extract_dependencies(files_content);
        for dep in dependencies {
            report.add_dependency(dep);
        }

        // Analyze entity usage across files
        let entity_deps = self.analyze_entity_usage(functions, classes, files_content);
        for dep in entity_deps {
            report.add_dependency(dep);
        }

        // Detect file clusters
        report.clusters = self.detect_file_clusters(&report.dependencies, files_content);

        // Identify architecture layers
        report.architecture_layers =
            self.identify_architecture_layers(files_content, &report.dependencies);

        // Detect circular dependencies
        report.circular_dependencies = self.detect_circular_dependencies(&report.dependencies);

        // Generate recommendations
        report.recommendations = self.generate_recommendations(&report);

        // Update summary
        report.summary.files_analyzed = files_content.len();
        report.summary.entities_analyzed = functions.len() + classes.len();
        report.summary.total_findings = report.circular_dependencies.len();

        Ok(report)
    }

    /// Extract dependencies from file content using patterns
    fn extract_dependencies(&self, files_content: &HashMap<String, String>) -> Vec<FileDependency> {
        let mut dependencies = Vec::new();

        for (file_path, content) in files_content {
            let file_language = self.detect_file_language(file_path, Some(content));

            if let Some(lang) = file_language
                && let Some(patterns) = self.dependency_patterns.get(&lang)
            {
                for (dep_type, pattern_list) in patterns {
                    for pattern in pattern_list {
                        for cap in pattern.captures_iter(content) {
                            if let Some(import_path) = cap.get(1) {
                                let import_str = import_path.as_str().trim();
                                if let Some(target_file) =
                                    self.resolve_import_path(import_str, file_path, files_content)
                                    && target_file != *file_path
                                {
                                    let strength =
                                        self.calculate_dependency_strength(import_str, content);

                                    let line_number =
                                        content[..import_path.start()].lines().count() + 1;

                                    let mut metadata = HashMap::new();
                                    metadata.insert(
                                        "line_number".to_string(),
                                        serde_json::json!(line_number),
                                    );
                                    metadata.insert(
                                        "import_statement".to_string(),
                                        serde_json::json!(
                                            cap.get(0).map(|m| m.as_str()).unwrap_or("")
                                        ),
                                    );
                                    metadata
                                        .insert("language".to_string(), serde_json::json!(lang));

                                    dependencies.push(FileDependency {
                                        source_file: file_path.clone(),
                                        target_file,
                                        dependency_type: dep_type.clone(),
                                        strength,
                                        entities: vec![import_str.to_string()],
                                        metadata,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        dependencies
    }

    /// Analyze cross-file entity usage patterns
    fn analyze_entity_usage(
        &self,
        functions: &[Function],
        classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Vec<FileDependency> {
        let mut dependencies: Vec<FileDependency> = Vec::new();

        // Create entity index by file
        let mut file_entities: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for func in functions {
            file_entities
                .entry(func.file_path.clone())
                .or_default()
                .push(("function".to_string(), func.name.clone()));
        }
        for cls in classes {
            file_entities
                .entry(cls.file_path.clone())
                .or_default()
                .push(("class".to_string(), cls.name.clone()));
        }

        // Analyze each file for entity references
        for (file_path, content) in files_content {
            let referenced = self.find_referenced_entities(content, functions, classes);

            for (entity_type, entity_name, source_file) in referenced {
                if source_file != *file_path {
                    // Check if this dependency already exists
                    let existing = dependencies
                        .iter_mut()
                        .find(|d| d.source_file == *file_path && d.target_file == source_file);

                    if let Some(existing_dep) = existing {
                        existing_dep
                            .entities
                            .push(format!("{entity_type}:{entity_name}"));
                        existing_dep.strength = (existing_dep.strength + 0.1).min(1.0);
                    } else {
                        let mut metadata = HashMap::new();
                        metadata
                            .insert("reference_type".to_string(), serde_json::json!(entity_type));
                        metadata.insert("entity_name".to_string(), serde_json::json!(entity_name));
                        metadata.insert("detected_in_content".to_string(), serde_json::json!(true));

                        dependencies.push(FileDependency {
                            source_file: file_path.clone(),
                            target_file: source_file,
                            dependency_type: "entity_reference".to_string(),
                            strength: 0.3,
                            entities: vec![format!("{entity_type}:{entity_name}")],
                            metadata,
                        });
                    }
                }
            }
        }

        dependencies
    }

    /// Detect clusters of related files
    fn detect_file_clusters(
        &self,
        dependencies: &[FileDependency],
        files_content: &HashMap<String, String>,
    ) -> Vec<FileCluster> {
        let files: Vec<String> = files_content.keys().cloned().collect();

        // Build adjacency list
        let mut adjacency: HashMap<String, HashSet<String>> = HashMap::new();
        for dep in dependencies {
            adjacency
                .entry(dep.source_file.clone())
                .or_default()
                .insert(dep.target_file.clone());
            adjacency
                .entry(dep.target_file.clone())
                .or_default()
                .insert(dep.source_file.clone());
        }

        // Find connected components (clusters)
        let mut visited: HashSet<String> = HashSet::new();
        let mut clusters = Vec::new();
        let mut cluster_id = 0;

        for file_path in &files {
            if !visited.contains(file_path) {
                let cluster_files = self.dfs_find_connected(file_path, &adjacency, &mut visited);

                if cluster_files.len() > 1 {
                    let cohesion_score =
                        self.calculate_cohesion_score(&cluster_files, dependencies);
                    let coupling_score =
                        self.calculate_coupling_score(&cluster_files, dependencies);
                    let common_patterns = self.find_common_patterns(&cluster_files, files_content);

                    let mut metadata = HashMap::new();
                    metadata.insert(
                        "file_count".to_string(),
                        serde_json::json!(cluster_files.len()),
                    );
                    let total_lines: usize = cluster_files
                        .iter()
                        .filter_map(|f| files_content.get(f))
                        .map(|c| c.lines().count())
                        .sum();
                    metadata.insert("total_lines".to_string(), serde_json::json!(total_lines));

                    clusters.push(FileCluster {
                        cluster_id,
                        files: cluster_files,
                        cohesion_score,
                        coupling_score,
                        common_patterns,
                        metadata,
                    });
                    cluster_id += 1;
                }
            }
        }

        clusters
    }

    /// Identify architectural layers based on file organization
    fn identify_architecture_layers(
        &self,
        files_content: &HashMap<String, String>,
        _dependencies: &[FileDependency],
    ) -> HashMap<String, Vec<String>> {
        let mut layers: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize layer categories
        layers.insert("presentation".to_string(), Vec::new());
        layers.insert("business".to_string(), Vec::new());
        layers.insert("data".to_string(), Vec::new());
        layers.insert("infrastructure".to_string(), Vec::new());
        layers.insert("api".to_string(), Vec::new());
        layers.insert("config".to_string(), Vec::new());
        layers.insert("tests".to_string(), Vec::new());
        layers.insert("utils".to_string(), Vec::new());
        layers.insert("docs".to_string(), Vec::new());
        layers.insert("other".to_string(), Vec::new());

        for file_path in files_content.keys() {
            let layer = self.classify_file_layer(file_path);
            layers.entry(layer).or_default().push(file_path.clone());
        }

        // Remove empty layers
        layers.retain(|_, v| !v.is_empty());

        layers
    }

    /// Detect circular dependencies between files
    fn detect_circular_dependencies(
        &self,
        dependencies: &[FileDependency],
    ) -> Vec<(String, String)> {
        // Build dependency graph
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for dep in dependencies {
            graph
                .entry(dep.source_file.clone())
                .or_default()
                .push(dep.target_file.clone());
        }

        // Detect cycles using DFS
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut cycles = Vec::new();

        fn dfs(
            node: &str,
            graph: &HashMap<String, Vec<String>>,
            visited: &mut HashSet<String>,
            rec_stack: &mut HashSet<String>,
            cycles: &mut Vec<(String, String)>,
            path: &mut Vec<String>,
        ) {
            if rec_stack.contains(node) {
                // Found a cycle
                if let Some(start_idx) = path.iter().position(|p| p == node) {
                    for i in start_idx..path.len() {
                        let current = path[i].clone();
                        let next = if i + 1 < path.len() {
                            path[i + 1].clone()
                        } else {
                            node.to_string()
                        };
                        cycles.push((current, next));
                    }
                }
                return;
            }

            if visited.contains(node) {
                return;
            }

            visited.insert(node.to_string());
            rec_stack.insert(node.to_string());
            path.push(node.to_string());

            if let Some(neighbors) = graph.get(node) {
                for neighbor in neighbors {
                    dfs(neighbor, graph, visited, rec_stack, cycles, path);
                }
            }

            path.pop();
            rec_stack.remove(node);
        }

        for file_path in graph.keys() {
            let mut path = Vec::new();
            dfs(
                file_path,
                &graph,
                &mut visited,
                &mut rec_stack,
                &mut cycles,
                &mut path,
            );
        }

        // Remove duplicates
        cycles.sort();
        cycles.dedup();

        cycles
    }

    /// Detect the programming language from a file path, with optional
    /// content-based shebang / marker sniffing.
    ///
    /// Falls back through three strategies:
    /// 1. File-name patterns (e.g. `Dockerfile`).
    /// 2. Extension lookup in the built-in mapping.
    /// 3. Content sniffing (shebang lines, known file markers) when `content`
    ///    is provided and extension detection fails.
    fn detect_file_language(&self, file_path: &str, content: Option<&str>) -> Option<String> {
        let path_lower = file_path.to_lowercase();

        // 1. File-name patterns
        if path_lower.ends_with("dockerfile") || path_lower.ends_with(".dockerfile") {
            return Some("dockerfile".to_string());
        }

        // 2. Extension lookup
        if let Some(ext_start) = path_lower.rfind('.') {
            let ext = &path_lower[ext_start..];
            if let Some(lang) = self.file_type_mapping.get(ext) {
                return Some(lang.clone());
            }
        }

        // 3. Content sniffing (shebangs and known markers)
        if let Some(content) = content {
            if let Some(first_line) = content.lines().next() {
                let first = first_line.trim();
                if first.starts_with("#!/bin/bash")
                    || first.starts_with("#!/usr/bin/env bash")
                    || first.starts_with("#!/bin/sh")
                    || first.starts_with("#!/usr/bin/env sh")
                {
                    return Some("bash".to_string());
                }
                if first.starts_with("#!/usr/bin/env python3")
                    || first.starts_with("#!/usr/bin/python")
                    || first.starts_with("#!/usr/bin/env python")
                {
                    return Some("python".to_string());
                }
                if first.starts_with("#!/usr/bin/env ruby") || first.starts_with("#!/usr/bin/ruby")
                {
                    return Some("ruby".to_string());
                }
                if first.starts_with("#!/usr/bin/env node") || first.starts_with("#!/usr/bin/node")
                {
                    return Some("javascript".to_string());
                }
                if first.starts_with("#!/usr/bin/env perl") || first.starts_with("#!/usr/bin/perl")
                {
                    return Some("perl".to_string());
                }
            }

            // Known file markers (first non-empty line only, for speed)
            for line in content.lines().map(str::trim) {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with("FROM ") {
                    return Some("dockerfile".to_string());
                }
                if line.starts_with("all:") || line.starts_with(".PHONY:") {
                    return Some("makefile".to_string());
                }
                break;
            }
        }

        None
    }

    /// Resolve import path to actual file path
    fn resolve_import_path(
        &self,
        import_path: &str,
        source_file: &str,
        files_content: &HashMap<String, String>,
    ) -> Option<String> {
        // Handle relative imports
        if import_path.starts_with('.') {
            let source_dir = std::path::Path::new(source_file)
                .parent()
                .map(|p| p.to_str().unwrap_or(""))
                .unwrap_or("");

            let import_clean = if let Some(stripped) = import_path.strip_prefix("./") {
                stripped
            } else {
                import_path
            };

            // Resolve relative path
            let resolved = if source_dir.is_empty() {
                import_clean.to_string()
            } else {
                format!("{source_dir}/{import_clean}")
            };

            // Try common extensions
            for ext in &[
                ".py", ".js", ".ts", ".java", ".cpp", ".go", ".rs", ".php", ".rb",
            ] {
                let test_path = format!("{resolved}{ext}");
                if files_content.contains_key(&test_path) {
                    return Some(test_path);
                }
            }

            // Try __init__.py for Python packages
            let init_path = format!("{resolved}/__init__.py");
            if files_content.contains_key(&init_path) {
                return Some(init_path);
            }

            // Try index files
            for index_file in &["index.js", "index.ts", "mod.rs", "lib.rs"] {
                let index_path = format!("{resolved}/{index_file}");
                if files_content.contains_key(&index_path) {
                    return Some(index_path);
                }
            }
        }

        // Handle absolute imports - try to find matching file
        // Try with extensions
        for ext in &[".py", ".js", ".ts", ".java", ".cpp", ".go", ".rs", ".php"] {
            let test_path = format!("{import_path}{ext}");
            if files_content.contains_key(&test_path) {
                return Some(test_path);
            }
        }

        // Try as directory with index file
        for index_file in &["__init__.py", "index.js", "index.ts", "mod.rs", "lib.rs"] {
            let test_path = format!("{import_path}/{index_file}");
            if files_content.contains_key(&test_path) {
                return Some(test_path);
            }
        }

        None
    }

    /// Calculate the strength of a dependency
    fn calculate_dependency_strength(&self, import_path: &str, content: &str) -> f64 {
        let base_strength = 0.5;

        // Count occurrences of the imported module
        let module_name = import_path
            .split('.')
            .next_back()
            .or_else(|| import_path.split('/').next_back())
            .unwrap_or(import_path);

        let escaped_name = regex::escape(module_name);
        let pattern = Regex::new(&format!(r"\b{}\b", escaped_name)).unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to compile regex for module name '{}': {}. Using literal fallback.",
                module_name,
                e
            );
            // Safe fallback: match nothing (valid regex guaranteed)
            Regex::new(r"a^").expect("empty regex is valid")
        });
        let occurrences = pattern.find_iter(content).count();

        // Adjust strength based on usage frequency
        match occurrences {
            0 => base_strength,
            1..=2 => base_strength + 0.1,
            3..=5 => base_strength + 0.2,
            _ => (base_strength + 0.3).min(1.0),
        }
    }

    /// Find entities referenced in content.
    ///
    /// Uses a single alternation regex per entity type instead of compiling
    /// one regex per entity, reducing compile cost from O(N) to O(1) per call.
    fn find_referenced_entities(
        &self,
        content: &str,
        functions: &[Function],
        classes: &[Class],
    ) -> Vec<(String, String, String)> {
        let mut referenced = Vec::new();

        // --- Function references: single regex `\b(name1|name2|…)\s*\(` ---
        if !functions.is_empty() {
            let func_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
            if let Some(re) = build_alternation_regex(&func_names) {
                for cap in re.captures_iter(content) {
                    if let Some(m) = cap.get(1) {
                        let matched = m.as_str();
                        // Find the function with this name (first match wins)
                        if let Some(func) = functions.iter().find(|f| f.name == matched) {
                            referenced.push((
                                "function".to_string(),
                                func.name.clone(),
                                func.file_path.clone(),
                            ));
                        }
                    }
                }
            }
        }

        // --- Class references: try several patterns, each as one regex ---
        if !classes.is_empty() {
            let class_names: Vec<&str> = classes.iter().map(|c| c.name.as_str()).collect();

            let class_patterns: Vec<String> = vec![
                // instantiation: `ClassName(`
                class_names
                    .iter()
                    .map(|n| regex::escape(n))
                    .collect::<Vec<_>>()
                    .join("|"),
            ];

            // Match any class name as a word boundary reference
            let alt_pattern = format!(r"\b({})\b", class_patterns[0]);
            if let Ok(re) = Regex::new(&alt_pattern) {
                for cap in re.captures_iter(content) {
                    if let Some(m) = cap.get(1) {
                        let matched = m.as_str();
                        if let Some(cls) = classes.iter().find(|c| c.name == matched) {
                            // Avoid duplicates for the same class
                            if !referenced.iter().any(|r| r.0 == "class" && r.1 == cls.name) {
                                referenced.push((
                                    "class".to_string(),
                                    cls.name.clone(),
                                    cls.file_path.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        referenced
    }

    /// Find all nodes connected to the start node using DFS
    fn dfs_find_connected(
        &self,
        start_node: &str,
        adjacency: &HashMap<String, HashSet<String>>,
        visited: &mut HashSet<String>,
    ) -> Vec<String> {
        let mut stack = vec![start_node.to_string()];
        let mut connected = Vec::new();

        while let Some(node) = stack.pop() {
            if !visited.contains(&node) {
                visited.insert(node.clone());
                connected.push(node.clone());

                if let Some(neighbors) = adjacency.get(&node) {
                    for neighbor in neighbors {
                        if !visited.contains(neighbor) {
                            stack.push(neighbor.clone());
                        }
                    }
                }
            }
        }

        connected
    }

    /// Calculate cohesion score for a file cluster
    fn calculate_cohesion_score(
        &self,
        cluster_files: &[String],
        dependencies: &[FileDependency],
    ) -> f64 {
        if cluster_files.len() <= 1 {
            return 1.0;
        }

        let total_possible = cluster_files.len() * (cluster_files.len() - 1);
        let mut internal_deps = 0.0;

        for dep in dependencies {
            if cluster_files.contains(&dep.source_file) && cluster_files.contains(&dep.target_file)
            {
                internal_deps += dep.strength;
            }
        }

        if total_possible == 0 {
            0.0
        } else {
            (internal_deps / total_possible as f64).min(1.0)
        }
    }

    /// Calculate coupling score for a file cluster
    fn calculate_coupling_score(
        &self,
        cluster_files: &[String],
        dependencies: &[FileDependency],
    ) -> f64 {
        if cluster_files.len() <= 1 {
            return 0.0;
        }

        let mut external_deps = 0.0;

        for dep in dependencies {
            let is_external = (cluster_files.contains(&dep.source_file)
                && !cluster_files.contains(&dep.target_file))
                || (!cluster_files.contains(&dep.source_file)
                    && cluster_files.contains(&dep.target_file));

            if is_external {
                external_deps += dep.strength;
            }
        }

        let max_possible = cluster_files.len() * 10; // Arbitrary max
        (external_deps / max_possible as f64).min(1.0)
    }

    /// Find common patterns in a file cluster
    fn find_common_patterns(
        &self,
        cluster_files: &[String],
        _files_content: &HashMap<String, String>,
    ) -> Vec<String> {
        let mut patterns = Vec::new();

        // Check for common directory prefix
        if let Some(first) = cluster_files.first() {
            let common_prefix = cluster_files.iter().fold(first.clone(), |acc, f| {
                let mut prefix = String::new();
                for (c1, c2) in acc.chars().zip(f.chars()) {
                    if c1 == c2 {
                        prefix.push(c1);
                    } else {
                        break;
                    }
                }
                prefix
            });

            if common_prefix.len() > 3 {
                patterns.push(format!("Common directory: {common_prefix}"));
            }
        }

        // Check for common file extensions
        let extensions: HashSet<&str> = cluster_files
            .iter()
            .filter_map(|f| f.rfind('.').map(|i| &f[i..]))
            .collect();

        if extensions.len() == 1
            && let Some(ext) = extensions.iter().next()
        {
            patterns.push(format!("Same language: {}", &ext[1..]));
        }

        // Check for naming patterns
        let base_names: Vec<String> = cluster_files
            .iter()
            .filter_map(|f| {
                std::path::Path::new(f)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .collect();

        let all_test_files = base_names
            .iter()
            .all(|n| n.ends_with("Test") || n.starts_with("test_"));
        let all_controllers = base_names.iter().all(|n| n.ends_with("Controller"));
        let all_services = base_names.iter().all(|n| n.ends_with("Service"));

        if all_test_files {
            patterns.push("Test files".to_string());
        } else if all_controllers {
            patterns.push("Controller files".to_string());
        } else if all_services {
            patterns.push("Service files".to_string());
        }

        patterns
    }

    /// Classify a file into an architectural layer.
    ///
    /// Patterns are evaluated in order from the `layer_patterns` vec.
    /// The first layer whose path or basename pattern matches wins.
    /// This is configurable via [`Self::with_layer_patterns`].
    fn classify_file_layer(&self, file_path: &str) -> String {
        let path_lower = file_path.to_lowercase();
        let basename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        for (layer_name, pattern) in &self.layer_patterns {
            let path_match = pattern
                .path_patterns
                .iter()
                .any(|p| path_lower.contains(&p.to_lowercase()));
            let basename_match = pattern
                .basename_patterns
                .iter()
                .any(|p| basename.contains(&p.to_lowercase()));
            if path_match || basename_match {
                return layer_name.clone();
            }
        }

        "other".to_string()
    }

    /// Generate recommendations based on cross-file analysis
    fn generate_recommendations(&self, report: &CrossFileReport) -> Vec<String> {
        let mut recommendations = Vec::new();

        // Circular dependencies
        if !report.circular_dependencies.is_empty() {
            recommendations.push(format!(
                "🔄 Found {} circular dependencies. Consider refactoring to break cycles.",
                report.circular_dependencies.len()
            ));
        }

        // Highly coupled files
        let mut dep_counts: HashMap<String, usize> = HashMap::new();
        for dep in &report.dependencies {
            *dep_counts.entry(dep.source_file.clone()).or_insert(0) += 1;
        }

        let highly_coupled: Vec<String> = dep_counts
            .iter()
            .filter(|(_, count)| **count > 10)
            .map(|(file, _)| file.clone())
            .collect();

        if !highly_coupled.is_empty() {
            recommendations.push(format!(
                "🔗 {} files have more than 10 dependencies. Consider breaking them into smaller modules.",
                highly_coupled.len()
            ));
        }

        // Low cohesion clusters
        let low_cohesion: Vec<&FileCluster> = report
            .clusters
            .iter()
            .filter(|c| c.cohesion_score < 0.3)
            .collect();

        if !low_cohesion.is_empty() {
            recommendations.push(format!(
                "📦 {} clusters have low cohesion. Review if files in these clusters belong together.",
                low_cohesion.len()
            ));
        }

        // High coupling clusters
        let high_coupling: Vec<&FileCluster> = report
            .clusters
            .iter()
            .filter(|c| c.coupling_score > 0.7)
            .collect();

        if !high_coupling.is_empty() {
            recommendations.push(format!(
                "🔌 {} clusters have high coupling. Consider reducing dependencies between modules.",
                high_coupling.len()
            ));
        }

        // Layer violations
        let violations = self.detect_layer_violations(&report.dependencies);
        if !violations.is_empty() {
            recommendations.push(format!(
                "🏗️ Found {} potential layer violations. Review architectural boundaries.",
                violations.len()
            ));
        }

        // Positive recommendations
        if report.circular_dependencies.is_empty() {
            recommendations
                .push("✅ No circular dependencies detected. Good architecture!".to_string());
        }

        if !report.dependencies.is_empty() {
            let avg_strength: f64 = report.dependencies.iter().map(|d| d.strength).sum::<f64>()
                / report.dependencies.len() as f64;
            if avg_strength < 0.4 {
                recommendations
                    .push("📉 Dependencies are generally weak. Good for modularity.".to_string());
            }
        }

        recommendations
    }

    /// Detect potential architectural layer violations
    fn detect_layer_violations(&self, dependencies: &[FileDependency]) -> Vec<String> {
        let layer_order: HashMap<&str, i32> = [
            ("presentation", 0),
            ("api", 1),
            ("business", 2),
            ("data", 3),
            ("infrastructure", 4),
            ("config", 5),
            ("tests", 6),
            ("utils", 7),
            ("docs", 8),
            ("other", 9),
        ]
        .iter()
        .cloned()
        .collect();

        let mut violations = Vec::new();

        for dep in dependencies {
            let source_layer = self.classify_file_layer(&dep.source_file);
            let target_layer = self.classify_file_layer(&dep.target_file);

            if let (Some(&source_level), Some(&target_level)) = (
                layer_order.get(source_layer.as_str()),
                layer_order.get(target_layer.as_str()),
            ) {
                // Check for violations (lower-level modules depending on higher-level modules)
                if source_level > target_level && source_level < 8 {
                    violations.push(format!(
                        "Layer violation: {} -> {} ({} -> {})",
                        source_layer, target_layer, dep.source_file, dep.target_file
                    ));
                }
            }
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_file_language() {
        let analyzer = CrossFileAnalyzer::new();

        assert_eq!(
            analyzer.detect_file_language("test.py", None),
            Some("python".to_string())
        );
        assert_eq!(
            analyzer.detect_file_language("test.js", None),
            Some("javascript".to_string())
        );
        assert_eq!(
            analyzer.detect_file_language("test.rs", None),
            Some("rust".to_string())
        );
        assert_eq!(analyzer.detect_file_language("test.txt", None), None);
        assert_eq!(
            analyzer.detect_file_language("Dockerfile", None),
            Some("dockerfile".to_string())
        );
        // Shebang detection
        assert_eq!(
            analyzer.detect_file_language("script", Some("#!/bin/bash\necho hi")),
            Some("bash".to_string())
        );
        assert_eq!(
            analyzer.detect_file_language("script", Some("#!/usr/bin/env python3\nprint(1)")),
            Some("python".to_string())
        );
        // Dockerfile marker
        assert_eq!(
            analyzer.detect_file_language("builder", Some("\n\nFROM alpine:latest\nRUN echo 1")),
            Some("dockerfile".to_string())
        );
    }

    #[test]
    fn test_classify_file_layer() {
        let analyzer = CrossFileAnalyzer::new();

        assert_eq!(
            analyzer.classify_file_layer("src/view/page.js"),
            "presentation"
        );
        assert_eq!(
            analyzer.classify_file_layer("src/controller/user.py"),
            "api"
        );
        assert_eq!(
            analyzer.classify_file_layer("src/service/order.rs"),
            "business"
        );
        assert_eq!(
            analyzer.classify_file_layer("src/repository/user.py"),
            "data"
        );
        assert_eq!(
            analyzer.classify_file_layer("src/util/helper.js"),
            "infrastructure"
        );
        assert_eq!(analyzer.classify_file_layer("tests/unit/test.py"), "tests");
    }

    #[test]
    fn test_calculate_dependency_strength() {
        let analyzer = CrossFileAnalyzer::new();

        let content = "import os; os.path.join('a', 'b'); os.path.exists('file'); os.getcwd(); os.environ; os.listdir(); os.mkdir(); os.remove(); os.rename();";
        let strength = analyzer.calculate_dependency_strength("os", content);
        assert!(strength > 0.5);
    }
}
