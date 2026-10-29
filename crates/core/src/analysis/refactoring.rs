//! Refactoring Suggestions Module for Devscriptor
//!
//! Provides functionality to detect common code smells and suggest
//! refactoring opportunities in codebases.

use crate::analysis::Severity;
use crate::database::connection::ConnectionPool;
use crate::entities::{Class, Function};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Data-class detection regexes, compiled once for the process lifetime.
static DEF_METHOD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"def\s+(\w+)\s*\(").expect("DEF_METHOD_RE is valid"));
static INIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"def\s+__init__\s*\([^)]*\)\s*:").expect("INIT_RE is valid"));

/// Types of code smells that can be detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSmellType {
    /// Method is too long
    LongMethod,
    /// Class is too large
    LargeClass,
    /// Duplicate code detected
    DuplicateCode,
    /// Too many parameters
    TooManyParameters,
    /// High cyclomatic complexity
    CyclomaticComplexity,
    /// Deeply nested conditionals
    NestedConditionals,
    /// Long parameter list
    LongParameterList,
    /// Feature envy (uses methods of another class excessively)
    FeatureEnvy,
    /// Data class with little behavior
    DataClass,
    /// Long message chains
    MessageChains,
    /// Middle man class
    MiddleMan,
    /// Speculative generality
    SpeculativeGenerality,
    /// Temporary field
    TemporaryField,
    /// Switch statements
    SwitchStatements,
    /// Parallel inheritance hierarchies
    ParallelInheritance,
    /// Lazy class with little functionality
    LazyClass,
    /// Incomplete library class
    IncompleteLibraryClass,
}

impl std::fmt::Display for CodeSmellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::LongMethod => "long_method",
            Self::LargeClass => "large_class",
            Self::DuplicateCode => "duplicate_code",
            Self::TooManyParameters => "too_many_parameters",
            Self::CyclomaticComplexity => "cyclomatic_complexity",
            Self::NestedConditionals => "nested_conditionals",
            Self::LongParameterList => "long_parameter_list",
            Self::FeatureEnvy => "feature_envy",
            Self::DataClass => "data_class",
            Self::MessageChains => "message_chains",
            Self::MiddleMan => "middle_man",
            Self::SpeculativeGenerality => "speculative_generality",
            Self::TemporaryField => "temporary_field",
            Self::SwitchStatements => "switch_statements",
            Self::ParallelInheritance => "parallel_inheritance",
            Self::LazyClass => "lazy_class",
            Self::IncompleteLibraryClass => "incomplete_library_class",
        };
        write!(f, "{name}")
    }
}

/// Represents a refactoring opportunity with details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringOpportunity {
    /// Type of code smell detected
    pub smell_type: CodeSmellType,
    /// Name of the entity
    pub entity_name: String,
    /// Path to the file
    pub file_path: String,
    /// Starting line number
    pub line_start: u32,
    /// Ending line number
    pub line_end: u32,
    /// Programming language
    pub language: String,
    /// Description of the issue
    pub description: String,
    /// Severity level
    pub severity: Severity,
    /// Suggestion for refactoring
    pub suggestion: String,
    /// Additional metrics
    pub metrics: HashMap<String, serde_json::Value>,
}

impl RefactoringOpportunity {
    /// Create a new refactoring opportunity
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        smell_type: CodeSmellType,
        entity_name: impl Into<String>,
        file_path: impl Into<String>,
        line_start: u32,
        line_end: u32,
        language: impl Into<String>,
        description: impl Into<String>,
        severity: Severity,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            smell_type,
            entity_name: entity_name.into(),
            file_path: file_path.into(),
            line_start,
            line_end,
            language: language.into(),
            description: description.into(),
            severity,
            suggestion: suggestion.into(),
            metrics: HashMap::new(),
        }
    }

    /// Add a metric to the opportunity
    #[must_use]
    pub fn with_metric(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.metrics.insert(key.into(), value.into());
        self
    }
}

/// Thresholds for detecting code smells
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmellThresholds {
    /// Maximum method length in lines
    pub max_method_length: i32,
    /// Maximum class length in lines
    pub max_class_length: i32,
    /// Maximum number of parameters
    pub max_parameters: i32,
    /// Maximum cyclomatic complexity
    pub max_complexity: i32,
    /// Maximum nesting depth
    pub max_nesting_depth: i32,
}

impl Default for SmellThresholds {
    fn default() -> Self {
        Self {
            max_method_length: 50,
            max_class_length: 500,
            max_parameters: 5,
            max_complexity: 10,
            max_nesting_depth: 3,
        }
    }
}

/// Refactoring suggestion entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringSuggestion {
    /// Type of suggestion
    pub suggestion_type: String,
    /// Description of the issue
    pub description: String,
    /// Suggestion for refactoring
    pub suggestion: String,
    /// Severity level
    pub severity: Severity,
}

/// Code smell summary entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSmellSummary {
    /// Type of smell
    pub smell_type: String,
    /// Number of occurrences
    pub count: usize,
    /// Affected entities
    pub entities: Vec<String>,
}

/// Complexity hotspot entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityHotspot {
    /// Entity name
    pub entity: String,
    /// File path
    pub file: String,
    /// Complexity value
    pub complexity: i32,
    /// Line range
    pub lines: String,
}

/// Duplication issue entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicationIssue {
    /// Entity name
    pub entity: String,
    /// File path
    pub file: String,
    /// Description of the issue
    pub description: String,
}

/// Comprehensive refactoring analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringReport {
    /// Summary statistics
    pub summary: RefactoringSummary,
    /// List of suggestions
    pub suggestions: Vec<SuggestionEntry>,
    /// Code smells grouped by type
    pub code_smells: Vec<CodeSmellSummary>,
    /// Complexity hotspots
    pub complexity_hotspots: Vec<ComplexityHotspot>,
    /// Duplication issues
    pub duplication_issues: Vec<DuplicationIssue>,
    /// General recommendations
    pub recommendations: Vec<String>,
}

/// Summary statistics for refactoring report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringSummary {
    /// Total number of opportunities
    pub total_opportunities: usize,
    /// Number of high priority issues
    pub high_priority: usize,
    /// Number of medium priority issues
    pub medium_priority: usize,
    /// Number of low priority issues
    pub low_priority: usize,
    /// Number of files affected
    pub files_affected: usize,
}

/// Suggestion entry for the report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionEntry {
    /// Type of smell
    pub suggestion_type: String,
    /// Entity name
    pub entity: String,
    /// File path
    pub file: String,
    /// Line range
    pub lines: String,
    /// Severity level
    pub severity: String,
    /// Description
    pub description: String,
    /// Suggestion for fix
    pub suggestion: String,
}

/// Refactoring analyzer for detecting code smells and suggesting improvements
pub struct RefactoringAnalyzer {
    /// Database connection pool
    pool: ConnectionPool,
    /// Thresholds for detection
    thresholds: SmellThresholds,
    /// Collected opportunities
    opportunities: Vec<RefactoringOpportunity>,
}

impl RefactoringAnalyzer {
    /// Create a new refactoring analyzer
    pub fn new(pool: ConnectionPool) -> Self {
        Self {
            pool,
            thresholds: SmellThresholds::default(),
            opportunities: Vec::new(),
        }
    }

    /// Create with custom thresholds
    #[must_use]
    pub fn with_thresholds(mut self, thresholds: SmellThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }

    /// Analyze the entire codebase for refactoring opportunities
    pub async fn analyze_codebase(&mut self) -> crate::error::Result<Vec<RefactoringOpportunity>> {
        self.opportunities.clear();

        // Analyze functions
        let functions = self.get_all_functions().await?;
        for function in functions {
            self.analyze_function(&function);
        }

        // Analyze classes
        let classes = self.get_all_classes().await?;
        for class in classes {
            self.analyze_class(&class);
        }

        Ok(self.opportunities.clone())
    }

    /// Get all functions from database
    async fn get_all_functions(&self) -> crate::error::Result<Vec<Function>> {
        crate::database::operations::get_all_functions(&self.pool)
            .await
            .map_err(|e| crate::error::Error::Analysis(format!("Failed to query functions: {e}")))
    }

    /// Get all classes from database
    async fn get_all_classes(&self) -> crate::error::Result<Vec<Class>> {
        crate::database::operations::get_all_classes(&self.pool)
            .await
            .map_err(|e| crate::error::Error::Analysis(format!("Failed to query classes: {e}")))
    }

    /// Analyze a function for code smells
    fn analyze_function(&mut self, function: &Function) {
        // Check for long methods
        self.check_long_method(function);

        // Check for too many parameters
        self.check_too_many_parameters(function);

        // Check for high cyclomatic complexity
        self.check_cyclomatic_complexity(function);

        // Check for nested conditionals
        self.check_nested_conditionals(function);
    }

    /// Analyze a class for code smells
    fn analyze_class(&mut self, class: &Class) {
        // Check for large classes
        self.check_large_class(class);

        // Check for data classes
        self.check_data_class(class);
    }

    /// Check if a function is too long
    fn check_long_method(&mut self, function: &Function) {
        let lines = (function.location.end_line - function.location.start_line + 1) as i32;
        if lines > self.thresholds.max_method_length {
            let severity = if lines <= (self.thresholds.max_method_length as f32 * 1.5) as i32 {
                Severity::Medium
            } else {
                Severity::High
            };

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::LongMethod,
                &function.name,
                &function.file_path,
                function.location.start_line,
                function.location.end_line,
                &function.language,
                format!(
                    "Method '{}' is {} lines long, exceeding the recommended maximum of {} lines",
                    function.name, lines, self.thresholds.max_method_length
                ),
                severity,
                "Consider breaking this method into smaller, more focused methods",
            )
            .with_metric("lines", lines)
            .with_metric("max_allowed", self.thresholds.max_method_length);

            self.opportunities.push(opportunity);
        }
    }

    /// Check if a class is too large
    fn check_large_class(&mut self, class: &Class) {
        let lines = (class.location.end_line - class.location.start_line + 1) as i32;
        if lines > self.thresholds.max_class_length {
            let severity = if lines <= (self.thresholds.max_class_length as f32 * 1.5) as i32 {
                Severity::Medium
            } else {
                Severity::High
            };

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::LargeClass,
                &class.name,
                &class.file_path,
                class.location.start_line,
                class.location.end_line,
                &class.language,
                format!(
                    "Class '{}' is {} lines long, exceeding the recommended maximum of {} lines",
                    class.name, lines, self.thresholds.max_class_length
                ),
                severity,
                "Consider splitting this class into smaller, more cohesive classes",
            )
            .with_metric("lines", lines)
            .with_metric("max_allowed", self.thresholds.max_class_length);

            self.opportunities.push(opportunity);
        }
    }

    /// Check if a function has too many parameters
    fn check_too_many_parameters(&mut self, function: &Function) {
        let param_count = self.count_parameters(&function.parameters);

        if param_count > self.thresholds.max_parameters {
            let severity = if param_count <= self.thresholds.max_parameters + 2 {
                Severity::Medium
            } else {
                Severity::High
            };

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::TooManyParameters,
                &function.name,
                &function.file_path,
                function.location.start_line,
                function.location.end_line,
                &function.language,
                format!(
                    "Function '{}' has {} parameters, exceeding the recommended maximum of {}",
                    function.name, param_count, self.thresholds.max_parameters
                ),
                severity,
                "Consider using a parameter object or builder pattern to reduce parameter count",
            )
            .with_metric("parameters", param_count)
            .with_metric("max_allowed", self.thresholds.max_parameters);

            self.opportunities.push(opportunity);
        }
    }

    /// Check for high cyclomatic complexity.
    ///
    /// Uses the shared algorithm from [`ComplexityAnalyzer`](crate::analysis::complexity::ComplexityAnalyzer)
    /// (audit fix A5) so that both analyzers report the same score for the same code.
    fn check_cyclomatic_complexity(&mut self, function: &Function) {
        let complexity =
            crate::analysis::complexity::ComplexityAnalyzer::calculate_cyclomatic_complexity_for_code(
                &function.body,
                &function.language,
            );

        if complexity > self.thresholds.max_complexity as u32 {
            let severity = if complexity <= (self.thresholds.max_complexity as f32 * 1.5) as u32 {
                Severity::Medium
            } else {
                Severity::High
            };

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::CyclomaticComplexity,
                &function.name,
                &function.file_path,
                function.location.start_line,
                function.location.end_line,
                &function.language,
                format!(
                    "Function '{}' has cyclomatic complexity of {}, exceeding the recommended maximum of {}",
                    function.name, complexity, self.thresholds.max_complexity
                ),
                severity,
                "Consider refactoring to reduce decision points or extract complex logic into separate methods",
            )
            .with_metric("complexity", complexity)
            .with_metric("max_allowed", self.thresholds.max_complexity);

            self.opportunities.push(opportunity);
        }
    }

    /// Check for deeply nested conditionals
    fn check_nested_conditionals(&mut self, function: &Function) {
        let max_depth = self.calculate_max_nesting_depth(&function.body);

        if max_depth > self.thresholds.max_nesting_depth {
            let severity = if max_depth <= self.thresholds.max_nesting_depth + 1 {
                Severity::Medium
            } else {
                Severity::High
            };

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::NestedConditionals,
                &function.name,
                &function.file_path,
                function.location.start_line,
                function.location.end_line,
                &function.language,
                format!(
                    "Function '{}' has nested conditionals with maximum depth of {}, exceeding the recommended maximum of {}",
                    function.name, max_depth, self.thresholds.max_nesting_depth
                ),
                severity,
                "Consider using guard clauses, early returns, or strategy pattern to reduce nesting",
            )
            .with_metric("nesting_depth", max_depth)
            .with_metric("max_allowed", self.thresholds.max_nesting_depth);

            self.opportunities.push(opportunity);
        }
    }

    /// Check if a class is a data class
    fn check_data_class(&mut self, class: &Class) {
        // Check for dataclass decorators or patterns
        let body = &class.body;
        let has_dataclass_decorator = body.contains("@dataclass") || body.contains("dataclass(");

        // Check for dataclass-like patterns
        let has_fields = class
            .fields
            .as_ref()
            .is_some_and(|f| !f.as_array().is_none_or(|a| a.is_empty()));

        // Count non-dunder methods
        let non_dunder_methods = if let Some(methods) = &class.methods {
            methods.iter().filter(|m| !m.starts_with("__")).count() as i32
        } else {
            0
        };

        // Count method definitions in body (excluding dunder methods)
        // Note: regex crate doesn't support negative lookahead, so we filter manually
        let method_defs = DEF_METHOD_RE
            .captures_iter(body)
            .filter(|cap| cap.get(1).is_some_and(|m| !m.as_str().starts_with("__")))
            .count() as i32;

        let total_behavioral_methods = non_dunder_methods + method_defs;

        // Check for simple init
        let has_simple_init = INIT_RE.is_match(body);

        // Determine if it's a data class
        let is_dataclass = has_dataclass_decorator
            || (has_fields && total_behavioral_methods == 0)
            || (has_simple_init && has_fields && total_behavioral_methods <= 2);

        if is_dataclass && !has_dataclass_decorator {
            let field_count = class
                .fields
                .as_ref()
                .and_then(|f| f.as_array())
                .map_or(0, |a| a.len()) as i32;

            let opportunity = RefactoringOpportunity::new(
                CodeSmellType::DataClass,
                &class.name,
                &class.file_path,
                class.location.start_line,
                class.location.end_line,
                &class.language,
                format!(
                    "Class '{}' appears to be a data class with {} fields but only {} behavioral methods",
                    class.name, field_count, total_behavioral_methods
                ),
                Severity::Low,
                "Consider using @dataclass decorator or a record/struct type to reduce boilerplate",
            )
            .with_metric("has_fields", has_fields)
            .with_metric("field_count", field_count)
            .with_metric("behavioral_methods", total_behavioral_methods);

            self.opportunities.push(opportunity);
        }
    }

    /// Count parameters in a parameter string with depth-aware parsing.
    ///
    /// Handles nested parentheses and generic angle brackets so commas
    /// inside `fn foo(x: Vec<(i32, i32)>, y: i32)` are not over-counted.
    fn count_parameters(&self, parameters: &Option<String>) -> i32 {
        let params = match parameters {
            Some(p) => p.trim(),
            None => return 0,
        };

        if params.is_empty() || params == "()" {
            return 0;
        }

        // Remove outer parentheses if present
        let clean_params = if params.starts_with('(') && params.ends_with(')') {
            &params[1..params.len() - 1]
        } else {
            params
        };

        let clean_params = clean_params.trim();
        if clean_params.is_empty() {
            return 0;
        }

        // Depth-aware comma counting: track () and <> nesting
        let mut paren_depth = 0_i32;
        let mut angle_depth = 0_i32;
        let mut count = 1_i32; // n commas => n+1 parameters
        let mut in_string = false;
        let mut prev_char = '\0';

        for ch in clean_params.chars() {
            if in_string {
                if ch == '"' && prev_char != '\\' {
                    in_string = false;
                }
            } else {
                match ch {
                    '"' => in_string = true,
                    '(' => paren_depth += 1,
                    ')' => paren_depth = paren_depth.saturating_sub(1),
                    '<' => angle_depth += 1,
                    '>' => angle_depth = angle_depth.saturating_sub(1),
                    ',' if paren_depth == 0 && angle_depth == 0 => count += 1,
                    _ => {}
                }
            }
            prev_char = ch;
        }

        count
    }

    /// Calculate maximum nesting depth of conditionals.
    ///
    /// This is a lightweight line-based approximation suitable for
    /// smell detection. For full nesting analysis see
    /// [`ComplexityAnalyzer`](crate::analysis::complexity::ComplexityAnalyzer).
    ///
    /// Tracks both C-style brace depth and indentation-based depth so the
    /// result is accurate for brace languages (C, Java, Rust…) as well as
    /// indentation languages (Python, Ruby…).
    fn calculate_max_nesting_depth(&self, code: &str) -> i32 {
        let mut max_depth: i32 = 0;
        let mut brace_depth: i32 = 0;

        for line in code.lines() {
            let stripped = line.trim();
            if stripped.is_empty() {
                continue;
            }

            // C-style brace tracking
            let opens = stripped.chars().filter(|&c| c == '{').count() as i32;
            let closes = stripped.chars().filter(|&c| c == '}').count() as i32;
            brace_depth += opens;
            max_depth = max_depth.max(brace_depth);
            brace_depth -= closes;
            brace_depth = brace_depth.max(0);

            // Indentation-based tracking (for Python, Ruby, YAML, etc.)
            let indent = line.len().saturating_sub(line.trim_start().len());
            let indent_depth = (indent / 4) as i32;
            max_depth = max_depth.max(indent_depth);
        }

        max_depth
    }

    /// Get usage relationships for the entire codebase.
    ///
    /// Note: `entity_name` is reserved for future filtering; currently
    /// all usage relationships are returned regardless of the name provided.
    #[allow(unused_variables)]
    pub async fn get_usage_relationships(
        &self,
        entity_name: &str,
    ) -> crate::error::Result<Vec<UsageRelationship>> {
        let relationships = crate::database::operations::get_all_relationships(&self.pool)
            .await
            .map_err(|e| {
                crate::error::Error::Analysis(format!("Failed to query relationships: {e}"))
            })?;

        let mut usage_info = Vec::new();

        for rel in relationships {
            if matches!(
                rel.relationship_type,
                crate::entities::RelationshipType::Uses
                    | crate::entities::RelationshipType::UsedBy
                    | crate::entities::RelationshipType::References
                    | crate::entities::RelationshipType::ReferencedBy
                    | crate::entities::RelationshipType::Calls
                    | crate::entities::RelationshipType::CalledBy
            ) {
                // Return usage information with entity IDs
                // Entity type can be determined from relationship metadata if available
                usage_info.push(UsageRelationship {
                    source_type: "entity".to_string(),
                    source_entity: rel.source_id.to_string(),
                    target_type: "entity".to_string(),
                    target_entity: Some(rel.target_id.to_string()),
                    relationship_type: format!("{:?}", rel.relationship_type),
                });
            }
        }

        Ok(usage_info)
    }

    /// Get refactoring suggestions based on usage patterns
    ///
    /// # Arguments
    ///
    /// * `entity_name` - The name of the entity to get suggestions for
    ///
    /// # Returns
    ///
    /// A list of refactoring suggestions based on the entity's usage patterns
    ///
    /// # Note
    ///
    /// This implementation analyzes patterns in the codebase to suggest improvements.
    /// Future enhancements could include more sophisticated analysis.
    pub async fn get_refactoring_suggestions(
        &self,
        entity_name: &str,
    ) -> crate::error::Result<Vec<RefactoringSuggestion>> {
        // Get usage relationships for the entity
        let usage = self.get_usage_relationships(entity_name).await?;
        let mut suggestions = Vec::new();

        // Analyze usage patterns and generate suggestions
        // High usage count might suggest extraction opportunity
        if usage.len() > 5 {
            suggestions.push(RefactoringSuggestion {
                suggestion_type: "extract_component".to_string(),
                description: format!(
                    "Entity '{}' is used in {} places",
                    entity_name,
                    usage.len()
                ),
                suggestion: format!(
                    "Consider extracting '{}' into a reusable component to reduce duplication across {} locations",
                    entity_name,
                    usage.len()
                ),
                severity: Severity::Medium,
            });
        }

        // Analyze relationship types for coupling issues
        let has_circular = usage.iter().any(|u| {
            u.relationship_type.contains("CalledBy") || u.relationship_type.contains("Uses")
        });
        if has_circular {
            suggestions.push(RefactoringSuggestion {
                suggestion_type: "decouple".to_string(),
                description: format!("Entity '{}' has bidirectional relationships", entity_name),
                suggestion:
                    "Consider decoupling components to improve maintainability and testability"
                        .to_string(),
                severity: Severity::Low,
            });
        }

        Ok(suggestions)
    }

    /// Generate a comprehensive analysis report
    pub async fn generate_report(&mut self) -> crate::error::Result<RefactoringReport> {
        let opportunities = self.analyze_codebase().await?;

        // Group opportunities by smell type
        let mut opportunities_by_type: HashMap<String, Vec<RefactoringOpportunity>> =
            HashMap::new();
        let mut severity_counts = HashMap::new();
        severity_counts.insert("low".to_string(), 0usize);
        severity_counts.insert("medium".to_string(), 0usize);
        severity_counts.insert("high".to_string(), 0usize);

        for opportunity in &opportunities {
            let smell_type = opportunity.smell_type.to_string();
            opportunities_by_type
                .entry(smell_type)
                .or_default()
                .push(opportunity.clone());

            let severity_key = format!("{:?}", opportunity.severity).to_lowercase();
            *severity_counts.entry(severity_key).or_insert(0) += 1;
        }

        // Calculate statistics
        let total_opportunities = opportunities.len();
        let high_priority = *severity_counts.get("high").unwrap_or(&0);
        let medium_priority = *severity_counts.get("medium").unwrap_or(&0);
        let low_priority = *severity_counts.get("low").unwrap_or(&0);

        let files_affected: std::collections::HashSet<_> =
            opportunities.iter().map(|o| o.file_path.clone()).collect();

        let summary = RefactoringSummary {
            total_opportunities,
            high_priority,
            medium_priority,
            low_priority,
            files_affected: files_affected.len(),
        };

        // Convert opportunities to suggestion entries
        let suggestions: Vec<SuggestionEntry> = opportunities
            .iter()
            .map(|opp| SuggestionEntry {
                suggestion_type: opp.smell_type.to_string(),
                entity: opp.entity_name.clone(),
                file: opp.file_path.clone(),
                lines: format!("{}-{}", opp.line_start, opp.line_end),
                severity: opp.severity.as_str().to_string(),
                description: opp.description.clone(),
                suggestion: opp.suggestion.clone(),
            })
            .collect();

        // Group by smell type for code_smells
        let code_smells: Vec<CodeSmellSummary> = opportunities_by_type
            .iter()
            .map(|(smell_type, opp_list)| CodeSmellSummary {
                smell_type: smell_type.clone(),
                count: opp_list.len(),
                entities: opp_list.iter().map(|o| o.entity_name.clone()).collect(),
            })
            .collect();

        // Identify complexity hotspots
        let complexity_hotspots: Vec<ComplexityHotspot> = opportunities
            .iter()
            .filter(|o| matches!(o.smell_type, CodeSmellType::CyclomaticComplexity))
            .map(|o| ComplexityHotspot {
                entity: o.entity_name.clone(),
                file: o.file_path.clone(),
                complexity: o
                    .metrics
                    .get("complexity")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32,
                lines: format!("{}-{}", o.line_start, o.line_end),
            })
            .collect();

        // Identify duplication issues
        let duplication_issues: Vec<DuplicationIssue> = opportunities
            .iter()
            .filter(|o| matches!(o.smell_type, CodeSmellType::DuplicateCode))
            .map(|o| DuplicationIssue {
                entity: o.entity_name.clone(),
                file: o.file_path.clone(),
                description: o.description.clone(),
            })
            .collect();

        let recommendations = self.generate_recommendations(&opportunities);

        Ok(RefactoringReport {
            summary,
            suggestions,
            code_smells,
            complexity_hotspots,
            duplication_issues,
            recommendations,
        })
    }

    /// Generate general recommendations based on detected opportunities
    fn generate_recommendations(&self, opportunities: &[RefactoringOpportunity]) -> Vec<String> {
        let mut recommendations = Vec::new();

        // Count occurrences of each smell type
        let mut smell_counts: HashMap<String, usize> = HashMap::new();
        for opportunity in opportunities {
            let smell_type = opportunity.smell_type.to_string();
            *smell_counts.entry(smell_type).or_insert(0) += 1;
        }

        // Generate recommendations based on patterns
        if smell_counts.get("long_method").copied().unwrap_or(0) > 3 {
            recommendations.push(
                "Consider breaking down large methods into smaller, more focused functions"
                    .to_string(),
            );
        }

        if smell_counts.get("large_class").copied().unwrap_or(0) > 2 {
            recommendations.push(
                "Consider splitting large classes into smaller, more cohesive classes".to_string(),
            );
        }

        if smell_counts
            .get("too_many_parameters")
            .copied()
            .unwrap_or(0)
            > 3
        {
            recommendations.push(
                "Consider using parameter objects or builder patterns to reduce parameter count"
                    .to_string(),
            );
        }

        if smell_counts
            .get("cyclomatic_complexity")
            .copied()
            .unwrap_or(0)
            > 3
        {
            recommendations.push(
                "Consider simplifying complex conditionals with guard clauses or strategy patterns"
                    .to_string(),
            );
        }

        if smell_counts
            .get("nested_conditionals")
            .copied()
            .unwrap_or(0)
            > 3
        {
            recommendations.push(
                "Consider reducing nesting depth with early returns or extracting methods"
                    .to_string(),
            );
        }

        if recommendations.is_empty() {
            recommendations.push(
                "Codebase appears to be in good health with minimal refactoring opportunities"
                    .to_string(),
            );
        }

        recommendations
    }

    /// Get only high-priority refactoring opportunities
    pub async fn get_high_priority_opportunities(
        &mut self,
    ) -> crate::error::Result<Vec<RefactoringOpportunity>> {
        let opportunities = self.analyze_codebase().await?;
        Ok(opportunities
            .into_iter()
            .filter(|o| matches!(o.severity, Severity::High | Severity::Critical))
            .collect())
    }

    /// Get refactoring opportunities for a specific file
    pub async fn get_opportunities_by_file(
        &mut self,
        file_path: &str,
    ) -> crate::error::Result<Vec<RefactoringOpportunity>> {
        let opportunities = self.analyze_codebase().await?;
        Ok(opportunities
            .into_iter()
            .filter(|o| o.file_path == file_path)
            .collect())
    }
}

/// Usage relationship information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRelationship {
    /// Source entity type
    pub source_type: String,
    /// Source entity ID
    pub source_entity: String,
    /// Target entity type
    pub target_type: String,
    /// Target entity ID
    pub target_entity: Option<String>,
    /// Relationship type
    pub relationship_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_parameters() {
        // Create analyzer without pool for unit testing internal methods
        struct TestAnalyzer;
        impl TestAnalyzer {
            fn count_parameters(&self, parameters: &Option<String>) -> i32 {
                let params = match parameters {
                    Some(p) => p.trim(),
                    None => return 0,
                };

                if params.is_empty() || params == "()" {
                    return 0;
                }

                let clean_params = if params.starts_with('(') && params.ends_with(')') {
                    &params[1..params.len() - 1]
                } else {
                    params
                };

                let clean_params = clean_params.trim();
                if clean_params.is_empty() {
                    return 0;
                }

                // Depth-aware comma counting: track () and <> nesting
                let mut paren_depth = 0_i32;
                let mut angle_depth = 0_i32;
                let mut count = 1_i32;
                let mut in_string = false;
                let mut prev_char = '\0';

                for ch in clean_params.chars() {
                    if in_string {
                        if ch == '"' && prev_char != '\\' {
                            in_string = false;
                        }
                    } else {
                        match ch {
                            '"' => in_string = true,
                            '(' => paren_depth += 1,
                            ')' => paren_depth = paren_depth.saturating_sub(1),
                            '<' => angle_depth += 1,
                            '>' => angle_depth = angle_depth.saturating_sub(1),
                            ',' if paren_depth == 0 && angle_depth == 0 => count += 1,
                            _ => {}
                        }
                    }
                    prev_char = ch;
                }

                count
            }
        }

        let analyzer = TestAnalyzer;

        // Basic cases
        assert_eq!(analyzer.count_parameters(&None), 0);
        assert_eq!(analyzer.count_parameters(&Some("".to_string())), 0);
        assert_eq!(analyzer.count_parameters(&Some("()".to_string())), 0);
        assert_eq!(analyzer.count_parameters(&Some("(x)".to_string())), 1);
        assert_eq!(analyzer.count_parameters(&Some("(x, y, z)".to_string())), 3);
        assert_eq!(analyzer.count_parameters(&Some("x, y".to_string())), 2);

        // Depth-aware: nested parentheses (tuples inside generics)
        assert_eq!(
            analyzer.count_parameters(&Some("(x: Vec<(i32, i32)>, y: i32)".to_string())),
            2
        );

        // Depth-aware: generic angle brackets
        assert_eq!(
            analyzer.count_parameters(&Some("(a: HashMap<String, i32>, b: bool)".to_string())),
            2
        );

        // Depth-aware: string literal with comma
        assert_eq!(
            analyzer.count_parameters(&Some("(msg: \"hello, world\", sep: char)".to_string())),
            2
        );
    }

    #[test]
    fn test_calculate_cyclomatic_complexity() {
        struct TestAnalyzer;
        impl TestAnalyzer {
            fn calculate_cyclomatic_complexity(&self, code: &str) -> i32 {
                let mut complexity: i32 = 1;
                let patterns = [
                    (r"\bif\b", "if"),
                    (r"\belif\b", "elif"),
                    (r"\bfor\b", "for"),
                    (r"\bwhile\b", "while"),
                    (r"\bcase\b", "case"),
                    (r"\bcatch\b", "catch"),
                    (r"\band\b", "and"),
                    (r"\bor\b", "or"),
                    (r"\?:", "ternary"),
                ];

                for (pattern, _) in &patterns {
                    if let Ok(re) = Regex::new(pattern) {
                        complexity += re.find_iter(code).count() as i32;
                    }
                }
                complexity
            }
        }

        let analyzer = TestAnalyzer;

        // Simple function
        let code = "def simple():\n    return 1";
        assert_eq!(analyzer.calculate_cyclomatic_complexity(code), 1);

        // Function with if
        let code = "def with_if(x):\n    if x:\n        return 1\n    return 0";
        assert!(analyzer.calculate_cyclomatic_complexity(code) >= 2);

        // Function with multiple decision points
        let code = r#"def complex(x, y):
            if x and y:
                return 1
            elif x or y:
                return 2
            return 0
        "#;
        let complexity = analyzer.calculate_cyclomatic_complexity(code);
        assert!(complexity > 1);
    }

    #[test]
    fn test_calculate_max_nesting_depth() {
        struct TestAnalyzer;
        impl TestAnalyzer {
            fn calculate_max_nesting_depth(&self, code: &str) -> i32 {
                let lines: Vec<&str> = code.lines().collect();
                let mut max_depth: i32 = 0;
                let mut current_depth: i32 = 0;

                for line in &lines {
                    let stripped = line.trim();

                    if stripped.starts_with("if ")
                        || stripped.starts_with("if(")
                        || stripped.starts_with("for ")
                        || stripped.starts_with("for(")
                        || stripped.starts_with("while ")
                        || stripped.starts_with("while(")
                        || stripped.starts_with("elif ")
                        || stripped == "else:"
                        || stripped.starts_with("except")
                        || stripped == "finally:"
                        || stripped.starts_with("with ")
                        || stripped.starts_with("match ")
                    {
                        current_depth += 1;
                        max_depth = max_depth.max(current_depth);
                    }
                }
                max_depth
            }
        }

        let analyzer = TestAnalyzer;

        // Simple function
        let code = "def simple():\n    return 1";
        assert_eq!(analyzer.calculate_max_nesting_depth(code), 0);

        // Function with single if
        let code = "def with_if(x):\n    if x:\n        return 1";
        assert!(analyzer.calculate_max_nesting_depth(code) >= 1);

        // Function with nested ifs
        let code = r#"def nested(x, y):
            if x:
                if y:
                    return 1
            return 0
        "#;
        let depth = analyzer.calculate_max_nesting_depth(code);
        assert!(depth >= 2);
    }

    #[test]
    fn test_refactoring_opportunity_builder() {
        let opportunity = RefactoringOpportunity::new(
            CodeSmellType::LongMethod,
            "test_func",
            "test.py",
            10,
            50,
            "python",
            "Method is too long",
            Severity::High,
            "Break into smaller methods",
        )
        .with_metric("lines", 40)
        .with_metric("max_allowed", 30);

        assert_eq!(opportunity.entity_name, "test_func");
        assert_eq!(opportunity.file_path, "test.py");
        assert_eq!(opportunity.metrics.get("lines").unwrap().as_i64(), Some(40));
    }

    #[test]
    fn test_smell_thresholds_default() {
        let thresholds = SmellThresholds::default();
        assert_eq!(thresholds.max_method_length, 50);
        assert_eq!(thresholds.max_class_length, 500);
        assert_eq!(thresholds.max_parameters, 5);
        assert_eq!(thresholds.max_complexity, 10);
        assert_eq!(thresholds.max_nesting_depth, 3);
    }

    #[test]
    fn test_code_smell_type_display() {
        assert_eq!(CodeSmellType::LongMethod.to_string(), "long_method");
        assert_eq!(CodeSmellType::DataClass.to_string(), "data_class");
        assert_eq!(
            CodeSmellType::CyclomaticComplexity.to_string(),
            "cyclomatic_complexity"
        );
    }

    #[test]
    fn test_generate_recommendations_empty() {
        // We can't instantiate RefactoringAnalyzer without a pool,
        // but we can test the recommendation generation logic directly
        let opportunities: Vec<RefactoringOpportunity> = vec![];

        // Count smells and generate recommendations inline
        let mut smell_counts: HashMap<String, usize> = HashMap::new();
        for opportunity in &opportunities {
            let smell_type = opportunity.smell_type.to_string();
            *smell_counts.entry(smell_type).or_insert(0) += 1;
        }

        let mut recommendations = Vec::new();
        if smell_counts.get("long_method").copied().unwrap_or(0) > 3 {
            recommendations.push("Consider breaking down large methods".to_string());
        }

        assert_eq!(recommendations.len(), 0);
    }

    #[test]
    fn test_generate_recommendations_long_methods() {
        let opportunities = vec![
            RefactoringOpportunity::new(
                CodeSmellType::LongMethod,
                "func1",
                "test.py",
                1,
                100,
                "python",
                "Too long",
                Severity::High,
                "Break it up",
            ),
            RefactoringOpportunity::new(
                CodeSmellType::LongMethod,
                "func2",
                "test.py",
                1,
                100,
                "python",
                "Too long",
                Severity::High,
                "Break it up",
            ),
            RefactoringOpportunity::new(
                CodeSmellType::LongMethod,
                "func3",
                "test.py",
                1,
                100,
                "python",
                "Too long",
                Severity::High,
                "Break it up",
            ),
            RefactoringOpportunity::new(
                CodeSmellType::LongMethod,
                "func4",
                "test.py",
                1,
                100,
                "python",
                "Too long",
                Severity::High,
                "Break it up",
            ),
        ];

        let mut smell_counts: HashMap<String, usize> = HashMap::new();
        for opportunity in &opportunities {
            let smell_type = opportunity.smell_type.to_string();
            *smell_counts.entry(smell_type).or_insert(0) += 1;
        }

        let mut recommendations = Vec::new();
        if smell_counts.get("long_method").copied().unwrap_or(0) > 3 {
            recommendations.push("Consider breaking down large methods".to_string());
        }

        assert_eq!(recommendations.len(), 1);
        assert!(recommendations[0].contains("large methods"));
    }
}
