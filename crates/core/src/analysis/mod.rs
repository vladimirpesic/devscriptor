//! Analysis Engine for Devscriptor
//!
//! Provides comprehensive code analysis capabilities including:
//! - Dead code detection
//! - Complexity analysis
//! - Clone detection
//! - Cross-reference analysis
//! - Call graph analysis
//! - Code smells detection
//! - Cross-file analysis

pub mod api;
pub mod architectural_patterns;
pub mod call_graph;
pub mod clone_detection;
pub mod code_smells;
pub mod complexity;
pub mod cross_file;
pub mod cross_reference;
pub mod dead_code;
pub mod refactoring;

pub use api::{
    AnalysisApi, AnalysisFinding, AnalysisJob, AnalysisResults, AnalysisSummary, AnalysisType,
    JobId, JobManager, JobStatus,
};
pub use architectural_patterns::{
    ArchitecturalPattern, ArchitecturalPatternRecognizer, PatternReport,
};
pub use call_graph::{
    CallGraph, CallGraphAnalyzer, CallGraphEdge, CallGraphGenerator, CallGraphNode,
    CallGraphReport, CallGraphSummary, CircularDependency, FunctionInfo,
};
pub use clone_detection::{CloneDetectionReport, CloneDetector, ClonePair};
pub use code_smells::{CodeSmell, CodeSmellDetector, CodeSmellMetrics, CodeSmellReport};
pub use complexity::{ComplexityAnalyzer, ComplexityMetrics, ComplexityReport};
pub use cross_file::{CrossFileAnalyzer, CrossFileReport, FileCluster, FileDependency};
pub use cross_reference::{
    CrossReference, CrossReferenceAnalyzer, CrossReferenceReport, UsageStatistics,
};
pub use dead_code::{DeadCodeDetector, DeadCodeReport};
pub use refactoring::{
    CodeSmellType, RefactoringAnalyzer, RefactoringOpportunity, RefactoringReport,
    RefactoringSuggestion, RefactoringSummary, SmellThresholds,
};

use serde::{Deserialize, Serialize};

/// Configuration for input-size guards on expensive analysis algorithms.
///
/// These limits prevent O(n²) or worse algorithms from running on
/// excessively large inputs, which would cause unbounded CPU/memory usage.
/// Also includes resource-guard limits (e.g. max concurrent jobs) to prevent
/// unbounded memory growth.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AnalysisGuardConfig {
    /// Maximum number of entities (functions/classes/methods) to feed into
    /// clone detection. The pairwise comparison is O(n²). Default: 5_000.
    pub max_entities_for_clone_detection: usize,
    /// Maximum number of code fragments to compare in clone detection.
    /// The fragment comparison loop is O(n²). Default: 2_000.
    pub max_fragments_for_clone_detection: usize,
    /// Maximum number of entities for call graph generation.
    /// Brandes' centrality is O(n·m). Default: 10_000.
    pub max_entities_for_call_graph: usize,
    /// Maximum number of nodes before skipping betweenness-centrality
    /// calculation in call graph reports. Default: 5_000.
    pub max_nodes_for_call_graph_centrality: usize,
    /// Maximum number of analysis jobs to retain in memory.
    /// Prevents unbounded growth in long-running services. Default: 1_000.
    pub max_jobs: usize,
}

impl Default for AnalysisGuardConfig {
    fn default() -> Self {
        Self {
            max_entities_for_clone_detection: 5_000,
            max_fragments_for_clone_detection: 2_000,
            max_entities_for_call_graph: 10_000,
            max_nodes_for_call_graph_centrality: 5_000,
            max_jobs: 1_000,
        }
    }
}

impl AnalysisGuardConfig {
    /// Create a new guard config with all defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable all guards (use with caution; only for small, trusted inputs).
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            max_entities_for_clone_detection: usize::MAX,
            max_fragments_for_clone_detection: usize::MAX,
            max_entities_for_call_graph: usize::MAX,
            max_nodes_for_call_graph_centrality: usize::MAX,
            max_jobs: usize::MAX,
        }
    }
}

/// Severity level for analysis findings.
///
/// Canonical severity enum used across `core`, `cli`, and `mcp` crates.
/// Derives `clap::ValueEnum` so it can be used directly as a CLI argument
/// without an intermediate wrapper type.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Severity {
    /// Informational finding
    #[default]
    Info,
    /// Low severity
    Low,
    /// Medium severity
    Medium,
    /// High severity
    High,
    /// Critical severity
    Critical,
}

impl Severity {
    /// Convert to numeric score (higher = more severe)
    #[must_use]
    pub const fn score(&self) -> u8 {
        match self {
            Self::Info => 1,
            Self::Low => 2,
            Self::Medium => 4,
            Self::High => 6,
            Self::Critical => 8,
        }
    }

    /// Get severity from score
    #[must_use]
    pub const fn from_score(score: u8) -> Self {
        match score {
            0 | 1 => Self::Info,
            2..=3 => Self::Low,
            4..=5 => Self::Medium,
            6..=7 => Self::High,
            8..=u8::MAX => Self::Critical,
        }
    }

    /// Get the lowercase string representation of the severity level.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A finding from code analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Finding severity
    pub severity: Severity,
    /// Finding message
    pub message: String,
    /// File path
    pub file_path: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Entity name (function, class, etc.)
    pub entity_name: String,
    /// Entity type
    pub entity_type: String,
    /// Additional context
    pub context: Option<String>,
    /// Suggested fix
    pub suggestion: Option<String>,
}

impl Finding {
    /// Create a new finding
    #[must_use]
    pub fn new(
        severity: Severity,
        message: impl Into<String>,
        file_path: impl Into<String>,
        line: u32,
        entity_name: impl Into<String>,
        entity_type: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            message: message.into(),
            file_path: file_path.into(),
            line,
            entity_name: entity_name.into(),
            entity_type: entity_type.into(),
            context: None,
            suggestion: None,
        }
    }

    /// Add context
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Add suggestion
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

/// Language-specific analysis configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageAnalysisConfig {
    /// Language name
    pub language: String,
    /// Decision point keywords
    pub decision_points: Vec<String>,
    /// Exit point keywords
    pub exit_points: Vec<String>,
    /// Loop keywords
    pub loop_keywords: Vec<String>,
    /// Conditional keywords
    pub conditional_keywords: Vec<String>,
    /// Comment patterns
    pub comment_patterns: Vec<String>,
}

impl Default for LanguageAnalysisConfig {
    fn default() -> Self {
        Self {
            language: "generic".to_string(),
            decision_points: vec![
                "if".to_string(),
                "for".to_string(),
                "while".to_string(),
                "switch".to_string(),
            ],
            exit_points: vec![
                "return".to_string(),
                "break".to_string(),
                "continue".to_string(),
            ],
            loop_keywords: vec!["for".to_string(), "while".to_string()],
            conditional_keywords: vec!["if".to_string(), "switch".to_string()],
            comment_patterns: vec!["//".to_string(), "/*".to_string()],
        }
    }
}

use std::sync::LazyLock;

static LANGUAGE_CONFIGS: LazyLock<std::collections::HashMap<&'static str, LanguageAnalysisConfig>> =
    LazyLock::new(|| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "python",
            LanguageAnalysisConfig {
                language: "python".to_string(),
                decision_points: vec![
                    "if".to_string(),
                    "elif".to_string(),
                    "for".to_string(),
                    "while".to_string(),
                    "and".to_string(),
                    "or".to_string(),
                    "except".to_string(),
                    "case".to_string(),
                ],
                exit_points: vec![
                    "return".to_string(),
                    "break".to_string(),
                    "continue".to_string(),
                    "raise".to_string(),
                ],
                loop_keywords: vec!["for".to_string(), "while".to_string()],
                conditional_keywords: vec!["if".to_string(), "elif".to_string()],
                comment_patterns: vec!["#".to_string(), "\"\"\"".to_string()],
            },
        );
        let js_ts = LanguageAnalysisConfig {
            language: "javascript".to_string(),
            decision_points: vec![
                "if".to_string(),
                "else if".to_string(),
                "for".to_string(),
                "while".to_string(),
                "switch".to_string(),
                "&&".to_string(),
                "||".to_string(),
                "catch".to_string(),
            ],
            exit_points: vec![
                "return".to_string(),
                "break".to_string(),
                "continue".to_string(),
                "throw".to_string(),
            ],
            loop_keywords: vec!["for".to_string(), "while".to_string(), "do".to_string()],
            conditional_keywords: vec!["if".to_string(), "switch".to_string()],
            comment_patterns: vec!["//".to_string(), "/*".to_string()],
        };
        m.insert("javascript", js_ts.clone());
        m.insert("typescript", js_ts.clone());
        m.insert("jsx", js_ts.clone());
        m.insert("tsx", js_ts.clone());
        m.insert(
            "java",
            LanguageAnalysisConfig {
                language: "java".to_string(),
                decision_points: vec![
                    "if".to_string(),
                    "else if".to_string(),
                    "for".to_string(),
                    "while".to_string(),
                    "switch".to_string(),
                    "&&".to_string(),
                    "||".to_string(),
                    "catch".to_string(),
                ],
                exit_points: vec![
                    "return".to_string(),
                    "break".to_string(),
                    "continue".to_string(),
                    "throw".to_string(),
                ],
                loop_keywords: vec!["for".to_string(), "while".to_string(), "do".to_string()],
                conditional_keywords: vec!["if".to_string(), "switch".to_string()],
                comment_patterns: vec!["//".to_string(), "/*".to_string()],
            },
        );
        m.insert(
            "rust",
            LanguageAnalysisConfig {
                language: "rust".to_string(),
                decision_points: vec![
                    "if".to_string(),
                    "else if".to_string(),
                    "for".to_string(),
                    "while".to_string(),
                    "match".to_string(),
                    "&&".to_string(),
                    "||".to_string(),
                ],
                exit_points: vec![
                    "return".to_string(),
                    "break".to_string(),
                    "continue".to_string(),
                    "panic!".to_string(),
                ],
                loop_keywords: vec!["for".to_string(), "while".to_string(), "loop".to_string()],
                conditional_keywords: vec!["if".to_string(), "match".to_string()],
                comment_patterns: vec!["//".to_string(), "/*".to_string(), "///".to_string()],
            },
        );
        let cpp = LanguageAnalysisConfig {
            language: "cpp".to_string(),
            decision_points: vec![
                "if".to_string(),
                "else if".to_string(),
                "for".to_string(),
                "while".to_string(),
                "switch".to_string(),
                "&&".to_string(),
                "||".to_string(),
                "catch".to_string(),
            ],
            exit_points: vec![
                "return".to_string(),
                "break".to_string(),
                "continue".to_string(),
                "throw".to_string(),
            ],
            loop_keywords: vec!["for".to_string(), "while".to_string(), "do".to_string()],
            conditional_keywords: vec!["if".to_string(), "switch".to_string()],
            comment_patterns: vec!["//".to_string(), "/*".to_string()],
        };
        m.insert("cpp", cpp.clone());
        m.insert("c++", cpp);
        m
    });

/// Get configuration for a specific language
#[must_use]
pub fn get_language_config(language: &str) -> LanguageAnalysisConfig {
    LANGUAGE_CONFIGS
        .get(language)
        .or_else(|| LANGUAGE_CONFIGS.get(language.to_lowercase().as_str()))
        .cloned()
        .unwrap_or_default()
}

/// Trait for analyzers that can analyze code.
///
/// This is a design contract for future analyzer implementations.
/// Currently no concrete type implements it, but it serves as the
/// canonical interface for analyzers that accept typed input and
/// produce typed reports.
pub trait Analyzer {
    /// The type of input this analyzer accepts
    type Input;
    /// The type of report this analyzer produces
    type Report;

    /// Analyze the input and produce a report
    ///
    /// # Errors
    /// Returns an error if analysis fails
    fn analyze(&self, input: Self::Input) -> crate::error::Result<Self::Report>;
}

/// Combine multiple findings into a single sorted list
pub fn combine_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut combined = findings;
    combined.sort_by(|a, b| {
        b.severity
            .score()
            .cmp(&a.severity.score())
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.line.cmp(&b.line))
    });
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_score() {
        assert_eq!(Severity::Info.score(), 1);
        assert_eq!(Severity::Low.score(), 2);
        assert_eq!(Severity::Medium.score(), 4);
        assert_eq!(Severity::High.score(), 6);
        assert_eq!(Severity::Critical.score(), 8);
    }

    #[test]
    fn test_severity_from_score() {
        assert!(matches!(Severity::from_score(1), Severity::Info));
        assert!(matches!(Severity::from_score(3), Severity::Low));
        assert!(matches!(Severity::from_score(5), Severity::Medium));
        assert!(matches!(Severity::from_score(7), Severity::High));
        assert!(matches!(Severity::from_score(9), Severity::Critical));
    }

    #[test]
    fn test_finding_builder() {
        let finding = Finding::new(
            Severity::High,
            "Test message",
            "/test/file.rs",
            42,
            "test_func",
            "function",
        )
        .with_context("Additional context")
        .with_suggestion("Fix suggestion");

        assert_eq!(finding.severity, Severity::High);
        assert_eq!(finding.message, "Test message");
        assert_eq!(finding.file_path, "/test/file.rs");
        assert_eq!(finding.line, 42);
        assert_eq!(finding.context, Some("Additional context".to_string()));
        assert_eq!(finding.suggestion, Some("Fix suggestion".to_string()));
    }

    #[test]
    fn test_analysis_summary() {
        use crate::analysis::api::AnalysisSummary;
        let mut summary = AnalysisSummary::new();
        summary.files_analyzed = 10;
        summary.entities_analyzed = 50;

        summary.add_finding(Severity::High);
        summary.add_finding(Severity::Medium);
        summary.add_finding(Severity::High);

        assert_eq!(summary.total_findings, 3);
        assert_eq!(summary.high_count, 2);
        assert_eq!(summary.medium_count, 1);
    }

    #[test]
    fn test_language_config() {
        let python_config = get_language_config("python");
        assert_eq!(python_config.language, "python");
        assert!(python_config.decision_points.contains(&"elif".to_string()));

        let rust_config = get_language_config("rust");
        assert_eq!(rust_config.language, "rust");
        assert!(rust_config.decision_points.contains(&"match".to_string()));

        let generic_config = get_language_config("unknown");
        assert_eq!(generic_config.language, "generic");
    }

    #[test]
    fn test_combine_findings() {
        let findings = vec![
            Finding::new(Severity::Low, "Low", "file.rs", 10, "a", "func"),
            Finding::new(Severity::High, "High", "file.rs", 20, "b", "func"),
            Finding::new(Severity::Medium, "Medium", "file.rs", 15, "c", "func"),
        ];

        let combined = combine_findings(findings);
        assert_eq!(combined[0].severity, Severity::High);
        assert_eq!(combined[1].severity, Severity::Medium);
        assert_eq!(combined[2].severity, Severity::Low);
    }
}
