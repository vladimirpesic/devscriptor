//! Code Smell Detection for Devscriptor
//!
//! This module provides functionality to detect various code smells and anti-patterns
//! in source code, helping identify areas that need refactoring for better maintainability.

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

// Each detector regex is compiled once for the process lifetime (LazyLock).
// All patterns are static and valid.
static MAGIC_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[2-9]\d{1,}\b").expect("MAGIC_NUMBER_RE is valid"));
static CALLBACK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\)\s*\{\s*.*\)\s*\{\s*.*\)\s*\{").expect("CALLBACK_RE is valid"));
static JAVA_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(private|public|protected)\s+\w+\s+\w+\s*;").expect("JAVA_FIELD_RE is valid")
});
static JAVA_METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(private|public|protected)\s*(static\s*)?\w+\s+\w+\s*\([^)]*\)\s*\{")
        .expect("JAVA_METHOD_RE is valid")
});
static OTHER_CLASS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z]\w+\.").expect("OTHER_CLASS_RE is valid"));
static NUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+\b").expect("NUM_RE is valid"));
static STR_LIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"["'][^"']*["']"#).expect("STR_LIT_RE is valid"));
static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[a-zA-Z_]\w*\b").expect("IDENT_RE is valid"));
use serde::{Deserialize, Serialize};

use crate::analysis::api::AnalysisSummary;
use crate::analysis::{Finding, Severity};
use crate::entities::{Class, Function};
use crate::error::Result;

/// Represents a detected code smell
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSmell {
    /// Smell name
    pub name: String,
    /// Smell category (design, implementation, naming, coupling, documentation)
    pub category: String,
    /// Severity level
    pub severity: Severity,
    /// Description of the smell
    pub description: String,
    /// File path where the smell was found
    pub file_path: String,
    /// Line number
    pub line_number: u32,
    /// Entity name (function, class, etc.)
    pub entity_name: String,
    /// Entity type
    pub entity_type: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f64,
    /// Suggestions for fixing the smell
    pub suggestions: Vec<String>,
    /// Context (code snippet or additional info)
    pub context: String,
}

/// Comprehensive code smell analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSmellReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// Detected code smells
    pub code_smells: Vec<CodeSmell>,
    /// Smell distribution by type
    pub smell_distribution: HashMap<String, usize>,
    /// Severity distribution
    pub severity_distribution: HashMap<String, usize>,
    /// Recommendations for fixing smells
    pub recommendations: Vec<String>,
    /// Additional metrics
    pub metrics: CodeSmellMetrics,
}

/// Metrics for code smell analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSmellMetrics {
    /// Smells per function
    pub smells_per_function: f64,
    /// Smells per class
    pub smells_per_class: f64,
    /// Most common smell type
    pub most_common_smell: Option<String>,
    /// Average confidence score
    pub average_confidence: f64,
}

impl Default for CodeSmellMetrics {
    fn default() -> Self {
        Self {
            smells_per_function: 0.0,
            smells_per_class: 0.0,
            most_common_smell: None,
            average_confidence: 0.0,
        }
    }
}

impl CodeSmellReport {
    /// Create a new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            code_smells: Vec::new(),
            smell_distribution: HashMap::new(),
            severity_distribution: HashMap::new(),
            recommendations: Vec::new(),
            metrics: CodeSmellMetrics::default(),
        }
    }

    /// Add a code smell to the report
    pub fn add_smell(&mut self, smell: CodeSmell) {
        self.summary.add_finding(smell.severity);
        *self
            .smell_distribution
            .entry(smell.name.clone())
            .or_insert(0) += 1;
        let severity_str = format!("{:?}", smell.severity).to_lowercase();
        *self.severity_distribution.entry(severity_str).or_insert(0) += 1;
        self.code_smells.push(smell);
    }

    /// Convert smells to findings
    #[must_use]
    pub fn to_findings(&self) -> Vec<Finding> {
        self.code_smells
            .iter()
            .map(|smell| {
                let suggestion = if smell.suggestions.is_empty() {
                    None
                } else {
                    Some(smell.suggestions.join("; "))
                };
                Finding {
                    severity: smell.severity,
                    message: format!("{}: {}", smell.name, smell.description),
                    file_path: smell.file_path.clone(),
                    line: smell.line_number,
                    entity_name: smell.entity_name.clone(),
                    entity_type: smell.entity_type.clone(),
                    context: Some(smell.context.clone()),
                    suggestion,
                }
            })
            .collect()
    }
}

impl Default for CodeSmellReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for a specific smell type
#[derive(Debug, Clone)]
struct SmellConfig {
    /// Threshold value for triggering the smell
    threshold: Option<u32>,
    /// Maximum parameters for long parameter list
    max_params: Option<u32>,
    /// Minimum methods for lazy class
    min_methods: Option<u32>,
    /// Minimum lines for lazy class
    min_lines: Option<u32>,
    /// Description of the smell
    description: &'static str,
}

/// Detector for various code smells and anti-patterns
#[derive(Debug)]
pub struct CodeSmellDetector {
    /// Smell patterns for different languages
    smell_configs: HashMap<String, HashMap<String, SmellConfig>>,
    /// Severity mapping for smell types
    severity_mapping: HashMap<String, Severity>,
    /// Category mapping for smell types
    category_mapping: HashMap<String, String>,
}

impl Default for CodeSmellDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeSmellDetector {
    /// Create a new code smell detector with default configurations
    #[must_use]
    pub fn new() -> Self {
        let mut smell_configs: HashMap<String, HashMap<String, SmellConfig>> = HashMap::new();

        // Python-specific configurations
        let mut python_configs = HashMap::new();
        python_configs.insert(
            "long_method".to_string(),
            SmellConfig {
                threshold: Some(50),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Method is too long and may be doing too many things",
            },
        );
        python_configs.insert(
            "long_parameter_list".to_string(),
            SmellConfig {
                threshold: None,
                max_params: Some(7),
                min_methods: None,
                min_lines: None,
                description: "Method has too many parameters, consider using a parameter object",
            },
        );
        python_configs.insert(
            "large_class".to_string(),
            SmellConfig {
                threshold: Some(300),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Class is too large and may have too many responsibilities",
            },
        );
        python_configs.insert(
            "complex_conditional".to_string(),
            SmellConfig {
                threshold: None,
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Complex conditional with multiple elif statements",
            },
        );
        python_configs.insert(
            "deep_nesting".to_string(),
            SmellConfig {
                threshold: Some(16), // 16 spaces = 4 levels with 4-space indentation
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Code is nested too deeply, consider refactoring",
            },
        );
        python_configs.insert(
            "lazy_class".to_string(),
            SmellConfig {
                threshold: None,
                max_params: None,
                min_methods: Some(3),
                min_lines: Some(20),
                description: "Class that doesn't do enough to pay for itself",
            },
        );
        smell_configs.insert("python".to_string(), python_configs);

        // JavaScript/TypeScript configurations
        let mut js_configs = HashMap::new();
        js_configs.insert(
            "long_function".to_string(),
            SmellConfig {
                threshold: Some(50),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Function is too long and may be doing too many things",
            },
        );
        js_configs.insert(
            "long_parameter_list".to_string(),
            SmellConfig {
                threshold: None,
                max_params: Some(5),
                min_methods: None,
                min_lines: None,
                description: "Function has too many parameters",
            },
        );
        js_configs.insert(
            "callback_hell".to_string(),
            SmellConfig {
                threshold: None,
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Deeply nested callbacks, consider using async/await or promises",
            },
        );
        smell_configs.insert("javascript".to_string(), js_configs.clone());
        smell_configs.insert("typescript".to_string(), js_configs);

        // Java configurations
        let mut java_configs = HashMap::new();
        java_configs.insert(
            "long_method".to_string(),
            SmellConfig {
                threshold: Some(50),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Method is too long and may be doing too many things",
            },
        );
        java_configs.insert(
            "large_class".to_string(),
            SmellConfig {
                threshold: Some(300),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Class is too large and may have too many responsibilities",
            },
        );
        java_configs.insert(
            "data_class".to_string(),
            SmellConfig {
                threshold: None,
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Class that only contains data and no behavior",
            },
        );
        smell_configs.insert("java".to_string(), java_configs);

        // Generic configurations
        let mut generic_configs = HashMap::new();
        generic_configs.insert(
            "long_function".to_string(),
            SmellConfig {
                threshold: Some(50),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Function is too long and may be doing too many things",
            },
        );
        generic_configs.insert(
            "large_class".to_string(),
            SmellConfig {
                threshold: Some(300),
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Class is too large and may have too many responsibilities",
            },
        );
        generic_configs.insert(
            "complex_conditional".to_string(),
            SmellConfig {
                threshold: None,
                max_params: None,
                min_methods: None,
                min_lines: None,
                description: "Complex conditional with multiple else-if statements",
            },
        );
        smell_configs.insert("generic".to_string(), generic_configs);

        // Severity mapping
        let mut severity_mapping = HashMap::new();
        severity_mapping.insert("long_method".to_string(), Severity::Medium);
        severity_mapping.insert("long_function".to_string(), Severity::Medium);
        severity_mapping.insert("long_parameter_list".to_string(), Severity::Low);
        severity_mapping.insert("large_class".to_string(), Severity::High);
        severity_mapping.insert("god_class".to_string(), Severity::High);
        severity_mapping.insert("feature_envy".to_string(), Severity::Medium);
        severity_mapping.insert("data_clumps".to_string(), Severity::Low);
        severity_mapping.insert("inappropriate_intimacy".to_string(), Severity::Medium);
        severity_mapping.insert("lazy_class".to_string(), Severity::Low);
        severity_mapping.insert("speculative_generality".to_string(), Severity::Medium);
        severity_mapping.insert("complex_conditional".to_string(), Severity::Medium);
        severity_mapping.insert("magic_numbers".to_string(), Severity::Low);
        severity_mapping.insert("deep_nesting".to_string(), Severity::Medium);
        severity_mapping.insert("callback_hell".to_string(), Severity::High);
        severity_mapping.insert("data_class".to_string(), Severity::Low);
        severity_mapping.insert("duplicate_code".to_string(), Severity::Medium);

        // Category mapping
        let mut category_mapping = HashMap::new();
        category_mapping.insert("long_method".to_string(), "implementation".to_string());
        category_mapping.insert("long_function".to_string(), "implementation".to_string());
        category_mapping.insert("long_parameter_list".to_string(), "design".to_string());
        category_mapping.insert("large_class".to_string(), "design".to_string());
        category_mapping.insert("god_class".to_string(), "design".to_string());
        category_mapping.insert("feature_envy".to_string(), "design".to_string());
        category_mapping.insert("data_clumps".to_string(), "design".to_string());
        category_mapping.insert("inappropriate_intimacy".to_string(), "coupling".to_string());
        category_mapping.insert("lazy_class".to_string(), "design".to_string());
        category_mapping.insert("speculative_generality".to_string(), "design".to_string());
        category_mapping.insert(
            "complex_conditional".to_string(),
            "implementation".to_string(),
        );
        category_mapping.insert("magic_numbers".to_string(), "implementation".to_string());
        category_mapping.insert("deep_nesting".to_string(), "implementation".to_string());
        category_mapping.insert("callback_hell".to_string(), "implementation".to_string());
        category_mapping.insert("data_class".to_string(), "design".to_string());
        category_mapping.insert("duplicate_code".to_string(), "design".to_string());

        Self {
            smell_configs,
            severity_mapping,
            category_mapping,
        }
    }

    /// Detect code smells in the provided codebase
    ///
    /// # Arguments
    ///
    /// * `functions` - List of function entities to analyze
    /// * `classes` - List of class entities to analyze
    /// * `files_content` - Dictionary mapping file paths to their content
    ///
    /// # Returns
    ///
    /// A `CodeSmellReport` containing all detected smells
    pub fn detect(
        &self,
        functions: &[Function],
        classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Result<CodeSmellReport> {
        let mut report = CodeSmellReport::new();

        // Detect smells in functions
        for function in functions {
            let smells = self.detect_function_smells(function, files_content);
            for smell in smells {
                report.add_smell(smell);
            }
        }

        // Detect smells in classes
        for class in classes {
            let smells = self.detect_class_smells(class, files_content);
            for smell in smells {
                report.add_smell(smell);
            }
        }

        // Detect cross-entity smells
        let cross_smells = self.detect_cross_entity_smells(functions, classes, files_content);
        for smell in cross_smells {
            report.add_smell(smell);
        }

        // Generate report metadata
        self.finalize_report(&mut report, functions.len(), classes.len());

        Ok(report)
    }

    /// Detect smells in a specific function
    fn detect_function_smells(
        &self,
        function: &Function,
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();
        let language = function.language.to_lowercase();
        let configs = self
            .smell_configs
            .get(&language)
            .or_else(|| self.smell_configs.get("generic"))
            .cloned()
            .unwrap_or_default();

        // Get function content
        let content = if !function.body.is_empty() {
            function.body.clone()
        } else if let Some(file_content) = files_content.get(&function.file_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let start = (function.location.start_line.saturating_sub(1)) as usize;
            let end = (function.location.end_line as usize).min(lines.len());
            if start < lines.len() {
                lines[start..end].join("\n")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if content.is_empty() {
            return smells;
        }

        let lines: Vec<&str> = content.lines().collect();
        let line_count = lines.len() as u32;

        // Check for long function/method
        let long_key = if language == "python" {
            "long_method"
        } else {
            "long_function"
        };
        if let Some(config) = configs.get(long_key)
            && let Some(threshold) = config.threshold
            && line_count > threshold
        {
            smells.push(CodeSmell {
                name: long_key.to_string(),
                category: self
                    .category_mapping
                    .get(long_key)
                    .cloned()
                    .unwrap_or_else(|| "implementation".to_string()),
                severity: self
                    .severity_mapping
                    .get(long_key)
                    .copied()
                    .unwrap_or(Severity::Medium),
                description: config.description.to_string(),
                file_path: function.file_path.clone(),
                line_number: function.location.start_line,
                entity_name: function.name.clone(),
                entity_type: "function".to_string(),
                confidence: 0.9,
                suggestions: vec![
                    format!("Break down {} into smaller methods", function.name),
                    "Extract logical units into separate functions".to_string(),
                    "Consider using the Extract Method refactoring pattern".to_string(),
                ],
                context: if content.len() > 200 {
                    format!("{}...", content.chars().take(200).collect::<String>())
                } else {
                    content.clone()
                },
            });
        }

        // Check for long parameter list
        if let Some(config) = configs.get("long_parameter_list")
            && let Some(max_params) = config.max_params
        {
            let param_count = self.count_parameters(&function.parameters);
            if param_count > max_params {
                smells.push(CodeSmell {
                    name: "long_parameter_list".to_string(),
                    category: self
                        .category_mapping
                        .get("long_parameter_list")
                        .cloned()
                        .unwrap_or_else(|| "design".to_string()),
                    severity: self
                        .severity_mapping
                        .get("long_parameter_list")
                        .copied()
                        .unwrap_or(Severity::Low),
                    description: config.description.to_string(),
                    file_path: function.file_path.clone(),
                    line_number: function.location.start_line,
                    entity_name: function.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: 0.8,
                    suggestions: vec![
                        "Consider using a parameter object".to_string(),
                        "Group related parameters into a class or struct".to_string(),
                        "Use builder pattern for complex parameter combinations".to_string(),
                    ],
                    context: function.parameters.clone().unwrap_or_default(),
                });
            }
        }

        // Check for complex conditionals
        if configs.contains_key("complex_conditional") {
            let if_count = content.matches(" if ").count() + content.matches("\nif ").count();
            let elif_count = content.matches(" elif ").count()
                + content.matches("\nelif ").count()
                + content.matches(" else if ").count();

            if if_count > 0
                && elif_count > 3
                && let Some(config) = configs.get("complex_conditional")
            {
                smells.push(CodeSmell {
                    name: "complex_conditional".to_string(),
                    category: self
                        .category_mapping
                        .get("complex_conditional")
                        .cloned()
                        .unwrap_or_else(|| "implementation".to_string()),
                    severity: self
                        .severity_mapping
                        .get("complex_conditional")
                        .copied()
                        .unwrap_or(Severity::Medium),
                    description: config.description.to_string(),
                    file_path: function.file_path.clone(),
                    line_number: function.location.start_line,
                    entity_name: function.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: 0.7,
                    suggestions: vec![
                        "Consider using polymorphism instead of complex conditionals".to_string(),
                        "Extract conditional logic into separate methods".to_string(),
                        "Use strategy pattern or lookup tables".to_string(),
                    ],
                    context: if content.chars().count() > 200 {
                        format!("{}...", content.chars().take(200).collect::<String>())
                    } else {
                        content.clone()
                    },
                });
            }
        }

        // Check for deep nesting
        if let Some(config) = configs.get("deep_nesting")
            && let Some(threshold) = config.threshold
        {
            let max_indent = lines
                .iter()
                .filter(|line: &&&str| !line.trim().is_empty())
                .map(|line: &&str| line.len() - line.trim_start().len())
                .max()
                .unwrap_or(0) as u32;

            if max_indent >= threshold {
                smells.push(CodeSmell {
                    name: "deep_nesting".to_string(),
                    category: self
                        .category_mapping
                        .get("deep_nesting")
                        .cloned()
                        .unwrap_or_else(|| "implementation".to_string()),
                    severity: self
                        .severity_mapping
                        .get("deep_nesting")
                        .copied()
                        .unwrap_or(Severity::Medium),
                    description: config.description.to_string(),
                    file_path: function.file_path.clone(),
                    line_number: function.location.start_line,
                    entity_name: function.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: 0.8,
                    suggestions: vec![
                        "Extract nested code into separate methods".to_string(),
                        "Use guard clauses to reduce nesting".to_string(),
                        "Consider early return patterns".to_string(),
                    ],
                    context: format!("Maximum indentation: {max_indent} spaces"),
                });
            }
        }

        // Check for magic numbers
        if configs.contains_key("magic_numbers") {
            let magic_number_re = &*MAGIC_NUMBER_RE;
            let magic_numbers: Vec<&str> = magic_number_re
                .find_iter(&content)
                .map(|m| m.as_str())
                .collect();
            // Filter out common numbers
            let filtered: Vec<&&str> = magic_numbers
                .iter()
                .filter(|&&n| {
                    if let Ok(num) = n.parse::<i32>() {
                        !matches!(num, 0 | 1 | -1 | 2 | 10 | 100 | 1000)
                    } else {
                        true
                    }
                })
                .collect();

            if filtered.len() > 2 {
                let context_str = filtered
                    .iter()
                    .take(5)
                    .copied()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ");
                smells.push(CodeSmell {
                    name: "magic_numbers".to_string(),
                    category: self
                        .category_mapping
                        .get("magic_numbers")
                        .cloned()
                        .unwrap_or_else(|| "implementation".to_string()),
                    severity: self
                        .severity_mapping
                        .get("magic_numbers")
                        .copied()
                        .unwrap_or(Severity::Low),
                    description: "Magic numbers should be replaced with named constants"
                        .to_string(),
                    file_path: function.file_path.clone(),
                    line_number: function.location.start_line,
                    entity_name: function.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: 0.6,
                    suggestions: vec![
                        "Replace magic numbers with named constants".to_string(),
                        "Define configuration values in a central location".to_string(),
                        "Use enumeration for related numeric values".to_string(),
                    ],
                    context: format!("Magic numbers found: {context_str}"),
                });
            }
        }

        // Check for callback hell (JavaScript/TypeScript)
        if configs.contains_key("callback_hell")
            && (language == "javascript" || language == "typescript")
        {
            let callback_pattern = &*CALLBACK_RE;
            if callback_pattern.is_match(&content)
                && let Some(config) = configs.get("callback_hell")
            {
                smells.push(CodeSmell {
                    name: "callback_hell".to_string(),
                    category: self
                        .category_mapping
                        .get("callback_hell")
                        .cloned()
                        .unwrap_or_else(|| "implementation".to_string()),
                    severity: self
                        .severity_mapping
                        .get("callback_hell")
                        .copied()
                        .unwrap_or(Severity::High),
                    description: config.description.to_string(),
                    file_path: function.file_path.clone(),
                    line_number: function.location.start_line,
                    entity_name: function.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: 0.7,
                    suggestions: vec![
                        "Convert callbacks to async/await".to_string(),
                        "Use Promise-based APIs".to_string(),
                        "Consider using async libraries".to_string(),
                    ],
                    context: "Deeply nested callback pattern detected".to_string(),
                });
            }
        }

        smells
    }

    /// Detect smells in a specific class
    fn detect_class_smells(
        &self,
        class: &Class,
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();
        let language = class.language.to_lowercase();
        let configs = self
            .smell_configs
            .get(&language)
            .or_else(|| self.smell_configs.get("generic"))
            .cloned()
            .unwrap_or_default();

        // Get class content
        let content = if !class.body.is_empty() {
            class.body.clone()
        } else if let Some(file_content) = files_content.get(&class.file_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let start = (class.location.start_line.saturating_sub(1)) as usize;
            let end = (class.location.end_line as usize).min(lines.len());
            if start < lines.len() {
                lines[start..end].join("\n")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if content.is_empty() {
            return smells;
        }

        let lines_count = content.lines().count() as u32;

        // Check for large class
        if let Some(config) = configs.get("large_class")
            && let Some(threshold) = config.threshold
            && lines_count > threshold
        {
            smells.push(CodeSmell {
                name: "large_class".to_string(),
                category: self
                    .category_mapping
                    .get("large_class")
                    .cloned()
                    .unwrap_or_else(|| "design".to_string()),
                severity: self
                    .severity_mapping
                    .get("large_class")
                    .copied()
                    .unwrap_or(Severity::High),
                description: config.description.to_string(),
                file_path: class.file_path.clone(),
                line_number: class.location.start_line,
                entity_name: class.name.clone(),
                entity_type: "class".to_string(),
                confidence: 0.9,
                suggestions: vec![
                    format!("Split {} into smaller, focused classes", class.name),
                    "Extract related functionality into separate classes".to_string(),
                    "Apply Single Responsibility Principle".to_string(),
                ],
                context: if content.len() > 200 {
                    format!("{}...", content.chars().take(200).collect::<String>())
                } else {
                    content.clone()
                },
            });
        }

        // Check for lazy class
        if let Some(config) = configs.get("lazy_class") {
            let method_count = content.matches("def ").count()
                + content.matches("function ").count()
                + content.matches("fn ").count();
            let min_methods = config.min_methods.unwrap_or(3);
            let min_lines = config.min_lines.unwrap_or(20);

            if method_count < min_methods as usize && lines_count > min_lines {
                smells.push(CodeSmell {
                    name: "lazy_class".to_string(),
                    category: self
                        .category_mapping
                        .get("lazy_class")
                        .cloned()
                        .unwrap_or_else(|| "design".to_string()),
                    severity: self
                        .severity_mapping
                        .get("lazy_class")
                        .copied()
                        .unwrap_or(Severity::Low),
                    description: config.description.to_string(),
                    file_path: class.file_path.clone(),
                    line_number: class.location.start_line,
                    entity_name: class.name.clone(),
                    entity_type: "class".to_string(),
                    confidence: 0.7,
                    suggestions: vec![
                        "Add more functionality to make the class useful".to_string(),
                        "Consider merging with a related class".to_string(),
                        "Remove the class if it's not adding value".to_string(),
                    ],
                    context: format!("Methods: {method_count}, Lines: {lines_count}"),
                });
            }
        }

        // Check for data class (Java only)
        if configs.contains_key("data_class") && language == "java" {
            let field_pattern = &*JAVA_FIELD_RE;
            let method_pattern = &*JAVA_METHOD_RE;

            let fields = field_pattern.find_iter(&content).count();
            let methods = method_pattern.find_iter(&content).count();

            if fields > 3 && methods < 2 {
                smells.push(CodeSmell {
                    name: "data_class".to_string(),
                    category: self
                        .category_mapping
                        .get("data_class")
                        .cloned()
                        .unwrap_or_else(|| "design".to_string()),
                    severity: self
                        .severity_mapping
                        .get("data_class")
                        .copied()
                        .unwrap_or(Severity::Low),
                    description: "Class that only contains data and no behavior".to_string(),
                    file_path: class.file_path.clone(),
                    line_number: class.location.start_line,
                    entity_name: class.name.clone(),
                    entity_type: "class".to_string(),
                    confidence: 0.8,
                    suggestions: vec![
                        "Add behavior methods to the class".to_string(),
                        "Consider using records or value objects if language supports them"
                            .to_string(),
                        "Apply the 'Tell, Don't Ask' principle".to_string(),
                    ],
                    context: format!("Fields: {fields}, Methods: {methods}"),
                });
            }
        }

        smells
    }

    /// Detect cross-entity smells
    fn detect_cross_entity_smells(
        &self,
        functions: &[Function],
        classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();

        // Detect duplicate code
        let duplicate_smells = self.detect_duplicate_code(functions, files_content);
        smells.extend(duplicate_smells);

        // Detect feature envy
        let envy_smells = self.detect_feature_envy(functions, classes, files_content);
        smells.extend(envy_smells);

        // Detect inappropriate intimacy
        let intimacy_smells = self.detect_inappropriate_intimacy(classes, files_content);
        smells.extend(intimacy_smells);

        smells
    }

    /// Detect duplicate code patterns
    fn detect_duplicate_code(
        &self,
        functions: &[Function],
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();
        let mut signatures: HashMap<String, Vec<&Function>> = HashMap::new();

        for func in functions {
            let signature = self.create_function_signature(func, files_content);
            if let Some(sig) = signature {
                signatures.entry(sig).or_default().push(func);
            }
        }

        // Find functions with similar signatures
        for (_signature, func_list) in signatures {
            if func_list.len() > 1 {
                // Calculate similarity between functions
                for i in 0..func_list.len() {
                    for j in (i + 1)..func_list.len() {
                        let func1 = func_list[i];
                        let func2 = func_list[j];
                        let similarity =
                            self.calculate_function_similarity(func1, func2, files_content);

                        if similarity > 0.8 {
                            smells.push(CodeSmell {
                                name: "duplicate_code".to_string(),
                                category: self
                                    .category_mapping
                                    .get("duplicate_code")
                                    .cloned()
                                    .unwrap_or_else(|| "design".to_string()),
                                severity: self
                                    .severity_mapping
                                    .get("duplicate_code")
                                    .copied()
                                    .unwrap_or(Severity::Medium),
                                description: "Duplicate or very similar code found".to_string(),
                                file_path: func1.file_path.clone(),
                                line_number: func1.location.start_line,
                                entity_name: func1.name.clone(),
                                entity_type: "function".to_string(),
                                confidence: similarity,
                                suggestions: vec![
                                    "Extract common code into a shared function".to_string(),
                                    "Use inheritance or composition to share behavior".to_string(),
                                    "Consider template method pattern".to_string(),
                                ],
                                context: format!(
                                    "Similar to {} in {}",
                                    func2.name, func2.file_path
                                ),
                            });
                        }
                    }
                }
            }
        }

        smells
    }

    /// Detect feature envy smells
    fn detect_feature_envy(
        &self,
        functions: &[Function],
        _classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();

        for func in functions {
            // Skip standalone functions
            if func.parent_class.is_none() {
                continue;
            }

            // Get function content
            let content = if !func.body.is_empty() {
                func.body.clone()
            } else if let Some(file_content) = files_content.get(&func.file_path) {
                let lines: Vec<&str> = file_content.lines().collect();
                let start = (func.location.start_line.saturating_sub(1)) as usize;
                let end = (func.location.end_line as usize).min(lines.len());
                if start < lines.len() {
                    lines[start..end].join("\n")
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            if content.is_empty() {
                continue;
            }

            // Count references to own class vs other classes
            let own_refs = content.matches("self.").count() + content.matches("this.").count();
            let dot_refs = content.matches('.').count();
            let other_refs = dot_refs.saturating_sub(own_refs);

            // Count distinct other-type prefixes accessed (e.g. `foo.bar`, `baz.qux` → 2 types)
            let mut other_types = std::collections::HashSet::new();
            for word in content.split_whitespace() {
                if let Some(dot_pos) = word.find('.') {
                    let prefix = &word[..dot_pos];
                    if !prefix.is_empty() && prefix != "self" && prefix != "this" {
                        other_types.insert(prefix.to_string());
                    }
                }
            }

            let total_field_access = own_refs + other_refs;
            let self_ratio = if total_field_access > 0 {
                own_refs as f64 / total_field_access as f64
            } else {
                1.0
            };

            // Secondary filter (A13): only flag when self usage < 20% of total field
            // access AND the method accesses >3 distinct other types.  This reduces
            // false positives from methods that simply have few total field accesses.
            if self_ratio < 0.20 && other_types.len() > 3 && other_refs > 3 {
                smells.push(CodeSmell {
                    name: "feature_envy".to_string(),
                    category: self
                        .category_mapping
                        .get("feature_envy")
                        .cloned()
                        .unwrap_or_else(|| "design".to_string()),
                    severity: self
                        .severity_mapping
                        .get("feature_envy")
                        .copied()
                        .unwrap_or(Severity::Medium),
                    description: "Method uses more data from other classes than its own"
                        .to_string(),
                    file_path: func.file_path.clone(),
                    line_number: func.location.start_line,
                    entity_name: func.name.clone(),
                    entity_type: "function".to_string(),
                    confidence: if self_ratio < 0.10 { 0.9 } else { 0.7 },
                    suggestions: vec![
                        "Move method to the class it's most interested in".to_string(),
                        "Extract method to a separate class that owns the data".to_string(),
                        "Consider using parameter objects to reduce coupling".to_string(),
                    ],
                    context: format!(
                        "Own refs: {own_refs}, Other refs: {other_refs}, \
                         Self ratio: {:.0}%, Distinct other types: {}",
                        self_ratio * 100.0,
                        other_types.len()
                    ),
                });
            }
        }

        smells
    }

    /// Detect inappropriate intimacy between classes
    fn detect_inappropriate_intimacy(
        &self,
        classes: &[Class],
        files_content: &HashMap<String, String>,
    ) -> Vec<CodeSmell> {
        let mut smells = Vec::new();

        // Compile regex once outside the loop
        let other_class_re = &*OTHER_CLASS_RE;

        for class in classes {
            // Get class content
            let content = if !class.body.is_empty() {
                class.body.clone()
            } else if let Some(file_content) = files_content.get(&class.file_path) {
                let lines: Vec<&str> = file_content.lines().collect();
                let start = (class.location.start_line.saturating_sub(1)) as usize;
                let end = (class.location.end_line as usize).min(lines.len());
                if start < lines.len() {
                    lines[start..end].join("\n")
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            if content.is_empty() {
                continue;
            }

            // Count references to other classes (simplified - looks for capitalized names followed by dot)
            let other_class_refs = other_class_re.find_iter(&content).count();

            // If class references many other classes frequently
            if other_class_refs > 10 {
                smells.push(CodeSmell {
                    name: "inappropriate_intimacy".to_string(),
                    category: self
                        .category_mapping
                        .get("inappropriate_intimacy")
                        .cloned()
                        .unwrap_or_else(|| "coupling".to_string()),
                    severity: self
                        .severity_mapping
                        .get("inappropriate_intimacy")
                        .copied()
                        .unwrap_or(Severity::Medium),
                    description: "Class is too closely coupled with many other classes".to_string(),
                    file_path: class.file_path.clone(),
                    line_number: class.location.start_line,
                    entity_name: class.name.clone(),
                    entity_type: "class".to_string(),
                    confidence: 0.6,
                    suggestions: vec![
                        "Reduce direct dependencies between classes".to_string(),
                        "Use interfaces or abstract classes to decouple".to_string(),
                        "Consider applying Law of Demeter".to_string(),
                    ],
                    context: format!("External class references: {other_class_refs}"),
                });
            }
        }

        smells
    }

    /// Create a simplified signature for function comparison
    fn create_function_signature(
        &self,
        function: &Function,
        files_content: &HashMap<String, String>,
    ) -> Option<String> {
        // Get function content
        let content = if !function.body.is_empty() {
            function.body.clone()
        } else if let Some(file_content) = files_content.get(&function.file_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let start = (function.location.start_line.saturating_sub(1)) as usize;
            let end = (function.location.end_line as usize).min(lines.len());
            if start < lines.len() {
                lines[start..end].join("\n")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if content.is_empty() {
            return None;
        }

        // Simplify content by removing variable names, strings, numbers
        let simplified = NUM_RE.replace_all(&content, "NUM");
        let simplified = STR_LIT_RE.replace_all(&simplified, "STR");
        let simplified = IDENT_RE.replace_all(&simplified, "VAR");

        // Count structural elements
        let if_count = simplified.matches("if").count();
        let for_count = simplified.matches("for").count();
        let while_count = simplified.matches("while").count();
        let param_count = self.count_parameters(&function.parameters);

        Some(format!(
            "params:{param_count}_if:{if_count}_for:{for_count}_while:{while_count}"
        ))
    }

    /// Calculate similarity between two functions
    fn calculate_function_similarity(
        &self,
        func1: &Function,
        func2: &Function,
        files_content: &HashMap<String, String>,
    ) -> f64 {
        // Get function contents
        let content1 = if !func1.body.is_empty() {
            func1.body.clone()
        } else if let Some(file_content) = files_content.get(&func1.file_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let start = (func1.location.start_line.saturating_sub(1)) as usize;
            let end = (func1.location.end_line as usize).min(lines.len());
            if start < lines.len() {
                lines[start..end].join("\n")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let content2 = if !func2.body.is_empty() {
            func2.body.clone()
        } else if let Some(file_content) = files_content.get(&func2.file_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let start = (func2.location.start_line.saturating_sub(1)) as usize;
            let end = (func2.location.end_line as usize).min(lines.len());
            if start < lines.len() {
                lines[start..end].join("\n")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if content1.is_empty() || content2.is_empty() {
            return 0.0;
        }

        // Calculate similarity based on common lines
        let lines1: std::collections::HashSet<&str> = content1.lines().collect();
        let lines2: std::collections::HashSet<&str> = content2.lines().collect();

        let common_count = lines1.intersection(&lines2).count();
        let total_count = lines1.union(&lines2).count();

        if total_count == 0 {
            0.0
        } else {
            common_count as f64 / total_count as f64
        }
    }

    /// Count the number of parameters in a function signature
    fn count_parameters(&self, parameters: &Option<String>) -> u32 {
        let params = match parameters {
            Some(p) => p,
            None => return 0,
        };

        let trimmed = params.trim();
        if trimmed.is_empty() || trimmed == "()" {
            return 0;
        }

        // Remove parentheses if present
        let inner = if trimmed.starts_with('(') && trimmed.ends_with(')') {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };

        if inner.trim().is_empty() {
            return 0;
        }

        // Count non-empty parameters split by comma
        inner.split(',').filter(|p| !p.trim().is_empty()).count() as u32
    }

    /// Finalize the report with metrics and recommendations
    fn finalize_report(
        &self,
        report: &mut CodeSmellReport,
        function_count: usize,
        class_count: usize,
    ) {
        // Calculate metrics
        let total_smells = report.code_smells.len();
        report.metrics.smells_per_function = if function_count > 0 {
            total_smells as f64 / function_count as f64
        } else {
            0.0
        };
        report.metrics.smells_per_class = if class_count > 0 {
            total_smells as f64 / class_count as f64
        } else {
            0.0
        };

        // Find most common smell
        report.metrics.most_common_smell = report
            .smell_distribution
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, _)| name.clone());

        // Calculate average confidence
        report.metrics.average_confidence = if total_smells > 0 {
            report.code_smells.iter().map(|s| s.confidence).sum::<f64>() / total_smells as f64
        } else {
            0.0
        };

        // Generate recommendations
        report.recommendations = self.generate_recommendations(report);
    }

    /// Generate recommendations based on detected smells
    fn generate_recommendations(&self, report: &CodeSmellReport) -> Vec<String> {
        let mut recommendations = Vec::new();

        if report.code_smells.is_empty() {
            recommendations
                .push("✅ No code smells detected. Code quality appears to be good!".to_string());
            return recommendations;
        }

        // Critical and high severity smells
        let critical_count = report
            .severity_distribution
            .get("critical")
            .copied()
            .unwrap_or(0);
        let high_count = report
            .severity_distribution
            .get("high")
            .copied()
            .unwrap_or(0);

        if critical_count > 0 {
            recommendations.push(format!(
                "🚨 {critical_count} critical code smells found. Address these immediately."
            ));
        }

        if high_count > 0 {
            recommendations.push(format!(
                "⚠️ {high_count} high-severity code smells found. Prioritize these for refactoring."
            ));
        }

        // Most common smell
        if let Some(ref most_common) = report.metrics.most_common_smell
            && let Some(&count) = report.smell_distribution.get(most_common)
        {
            recommendations.push(format!(
                    "🔧 Most common smell: {most_common} ({count} occurrences). Focus refactoring efforts on this pattern."
                ));
        }

        // Specific recommendations based on smell types
        if report.smell_distribution.contains_key("duplicate_code") {
            recommendations.push(
                "🔄 Duplicate code detected. Consider extracting common functionality.".to_string(),
            );
        }

        if report.smell_distribution.contains_key("large_class")
            || report.smell_distribution.contains_key("god_class")
        {
            recommendations.push(
                "🏗️ Large classes detected. Apply Single Responsibility Principle.".to_string(),
            );
        }

        if report.smell_distribution.contains_key("long_method")
            || report.smell_distribution.contains_key("long_function")
        {
            recommendations.push(
                "📏 Long functions detected. Break them into smaller, focused methods.".to_string(),
            );
        }

        if report
            .smell_distribution
            .contains_key("complex_conditional")
        {
            recommendations.push(
                "🔀 Complex conditionals detected. Consider polymorphism or strategy patterns."
                    .to_string(),
            );
        }

        recommendations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_parameters() {
        let detector = CodeSmellDetector::new();

        assert_eq!(detector.count_parameters(&None), 0);
        assert_eq!(detector.count_parameters(&Some("()".to_string())), 0);
        assert_eq!(detector.count_parameters(&Some("".to_string())), 0);
        assert_eq!(detector.count_parameters(&Some("(a)".to_string())), 1);
        assert_eq!(detector.count_parameters(&Some("(a, b, c)".to_string())), 3);
        assert_eq!(detector.count_parameters(&Some("a, b, c".to_string())), 3);
    }

    #[test]
    fn test_code_smell_report() {
        let mut report = CodeSmellReport::new();

        let smell = CodeSmell {
            name: "long_method".to_string(),
            category: "implementation".to_string(),
            severity: Severity::Medium,
            description: "Method is too long".to_string(),
            file_path: "test.py".to_string(),
            line_number: 10,
            entity_name: "test_func".to_string(),
            entity_type: "function".to_string(),
            confidence: 0.9,
            suggestions: vec!["Break it down".to_string()],
            context: "def test_func(): ...".to_string(),
        };

        report.add_smell(smell.clone());
        report.add_smell(smell);

        assert_eq!(report.code_smells.len(), 2);
        assert_eq!(report.smell_distribution.get("long_method"), Some(&2));
    }
}
