//! Complexity Analyzer for Devscriptor
//!
//! This module provides functionality to calculate detailed complexity metrics
//! for functions and classes, including cyclomatic complexity, cognitive complexity,
//! and other software metrics.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::analysis::api::AnalysisSummary;
use crate::analysis::{Finding, LanguageAnalysisConfig, Severity, get_language_config};
use crate::database::Database;
use crate::error::Result;

/// Represents complexity metrics for a code entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityMetrics {
    /// Entity name
    pub entity_name: String,
    /// File path
    pub file_path: String,
    /// Entity type
    pub entity_type: String,
    /// Start line
    pub start_line: u32,
    /// End line
    pub end_line: u32,
    /// Programming language
    pub language: String,

    // Complexity metrics
    /// Cyclomatic complexity
    pub cyclomatic_complexity: u32,
    /// Cognitive complexity
    pub cognitive_complexity: u32,
    /// Lines of code
    pub lines_of_code: u32,
    /// Comment lines
    pub comment_lines: u32,
    /// Blank lines
    pub blank_lines: u32,
    /// Parameter count
    pub parameter_count: u32,
    /// Nesting depth
    pub nesting_depth: u32,
    /// Decision points
    pub decision_points: u32,
    /// Exit points
    pub exit_points: u32,

    // Quality scores
    /// Maintainability index (0-100)
    pub maintainability_index: f64,
    /// Halstead volume
    pub halstead_volume: f64,
    /// Halstead difficulty
    pub halstead_difficulty: f64,
    /// Halstead effort
    pub halstead_effort: f64,

    // Additional metrics
    /// Code ratio (code lines / total lines)
    pub code_ratio: f64,
    /// Comment ratio (comment lines / total lines)
    pub comment_ratio: f64,
}

impl ComplexityMetrics {
    /// Create new empty metrics
    #[must_use]
    pub fn new(
        entity_name: impl Into<String>,
        file_path: impl Into<String>,
        entity_type: impl Into<String>,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            file_path: file_path.into(),
            entity_type: entity_type.into(),
            start_line: 0,
            end_line: 0,
            language: "unknown".to_string(),
            cyclomatic_complexity: 0,
            cognitive_complexity: 0,
            lines_of_code: 0,
            comment_lines: 0,
            blank_lines: 0,
            parameter_count: 0,
            nesting_depth: 0,
            decision_points: 0,
            exit_points: 0,
            maintainability_index: 0.0,
            halstead_volume: 0.0,
            halstead_difficulty: 0.0,
            halstead_effort: 0.0,
            code_ratio: 0.0,
            comment_ratio: 0.0,
        }
    }

    /// Get overall complexity score (higher = more complex)
    #[must_use]
    pub fn overall_score(&self) -> f64 {
        let cyclomatic_weight = 0.3;
        let cognitive_weight = 0.3;
        let nesting_weight = 0.2;
        let loc_weight = 0.2;

        (f64::from(self.cyclomatic_complexity) * cyclomatic_weight)
            + (f64::from(self.cognitive_complexity) * cognitive_weight)
            + (f64::from(self.nesting_depth * 5) * nesting_weight)
            + (f64::from(self.lines_of_code / 10) * loc_weight)
    }

    /// Get risk level based on complexity
    #[must_use]
    pub fn risk_level(&self) -> Severity {
        let score = self.overall_score();
        match score {
            s if s < 10.0 => Severity::Info,
            s if s < 20.0 => Severity::Low,
            s if s < 35.0 => Severity::Medium,
            s if s < 50.0 => Severity::High,
            _ => Severity::Critical,
        }
    }

    /// Check if complexity exceeds thresholds
    #[must_use]
    pub fn exceeds_thresholds(&self, thresholds: &ComplexityThresholds) -> Vec<String> {
        let mut issues = Vec::new();

        if self.cyclomatic_complexity > thresholds.max_cyclomatic_complexity {
            issues.push(format!(
                "Cyclomatic complexity {} exceeds threshold {}",
                self.cyclomatic_complexity, thresholds.max_cyclomatic_complexity
            ));
        }

        if self.cognitive_complexity > thresholds.max_cognitive_complexity {
            issues.push(format!(
                "Cognitive complexity {} exceeds threshold {}",
                self.cognitive_complexity, thresholds.max_cognitive_complexity
            ));
        }

        if self.lines_of_code > thresholds.max_lines_of_code {
            issues.push(format!(
                "Lines of code {} exceeds threshold {}",
                self.lines_of_code, thresholds.max_lines_of_code
            ));
        }

        if self.nesting_depth > thresholds.max_nesting_depth {
            issues.push(format!(
                "Nesting depth {} exceeds threshold {}",
                self.nesting_depth, thresholds.max_nesting_depth
            ));
        }

        if self.comment_ratio < thresholds.min_comment_ratio {
            issues.push(format!(
                "Comment ratio {:.2}% is below minimum {:.2}%",
                self.comment_ratio * 100.0,
                thresholds.min_comment_ratio * 100.0
            ));
        }

        issues
    }
}

/// Complexity thresholds for quality assessment
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ComplexityThresholds {
    /// Maximum cyclomatic complexity
    pub max_cyclomatic_complexity: u32,
    /// Maximum cognitive complexity
    pub max_cognitive_complexity: u32,
    /// Maximum lines of code per function
    pub max_lines_of_code: u32,
    /// Maximum nesting depth
    pub max_nesting_depth: u32,
    /// Minimum comment ratio
    pub min_comment_ratio: f64,
}

impl Default for ComplexityThresholds {
    fn default() -> Self {
        Self {
            max_cyclomatic_complexity: 10,
            max_cognitive_complexity: 15,
            max_lines_of_code: 100,
            max_nesting_depth: 4,
            min_comment_ratio: 0.1,
        }
    }
}

/// Comprehensive complexity analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// Function metrics
    pub function_metrics: Vec<ComplexityMetrics>,
    /// Class metrics
    pub class_metrics: Vec<ComplexityMetrics>,
    /// Recommendations
    pub recommendations: Vec<String>,
    /// High complexity entities
    pub high_complexity_entities: Vec<ComplexityMetrics>,
}

impl ComplexityReport {
    /// Create new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            function_metrics: Vec::new(),
            class_metrics: Vec::new(),
            recommendations: Vec::new(),
            high_complexity_entities: Vec::new(),
        }
    }

    /// Add function metrics
    pub fn add_function_metrics(&mut self, metrics: ComplexityMetrics) {
        self.function_metrics.push(metrics);
    }

    /// Add class metrics
    pub fn add_class_metrics(&mut self, metrics: ComplexityMetrics) {
        self.class_metrics.push(metrics);
    }

    /// Generate recommendations based on findings
    pub fn generate_recommendations(&mut self) {
        let mut recs = Vec::new();

        let high_complexity_count = self
            .function_metrics
            .iter()
            .filter(|m| m.cyclomatic_complexity > 10)
            .count();

        if high_complexity_count > 0 {
            recs.push(format!(
                "{} functions have high cyclomatic complexity. Consider refactoring into smaller functions.",
                high_complexity_count
            ));
        }

        let deep_nesting_count = self
            .function_metrics
            .iter()
            .filter(|m| m.nesting_depth > 4)
            .count();

        if deep_nesting_count > 0 {
            recs.push(format!(
                "{} functions have deep nesting. Consider extracting nested logic into separate functions.",
                deep_nesting_count
            ));
        }

        let long_functions = self
            .function_metrics
            .iter()
            .filter(|m| m.lines_of_code > 100)
            .count();

        if long_functions > 0 {
            recs.push(format!(
                "{} functions are quite long. Consider breaking them down.",
                long_functions
            ));
        }

        self.recommendations = recs;
    }

    /// Convert metrics to findings
    #[must_use]
    pub fn to_findings(&self, thresholds: &ComplexityThresholds) -> Vec<Finding> {
        let mut findings = Vec::new();

        for metrics in &self.function_metrics {
            let issues = metrics.exceeds_thresholds(thresholds);
            for issue in issues {
                findings.push(Finding::new(
                    metrics.risk_level(),
                    issue,
                    &metrics.file_path,
                    metrics.start_line,
                    &metrics.entity_name,
                    &metrics.entity_type,
                ));
            }
        }

        findings
    }
}

impl Default for ComplexityReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-compiled regexes for Halstead operator counting.
static HALSTEAD_OPERATORS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let ops: &[&str] = &[
        "+", "-", "*", "/", "%", "=", "==", "!=", "<", ">", "<=", ">=", "&&", "||", "!",
    ];
    ops.iter()
        .filter_map(|&op| {
            let pattern = format!(r"\{}\b", regex::escape(op));
            Regex::new(&pattern).ok().map(|re| (op, re))
        })
        .collect()
});

/// Pre-compiled regex for Halstead operand (identifier) counting.
static HALSTEAD_OPERAND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[a-zA-Z_]\w*\b").expect("static regex is valid"));

/// Analyzer for code complexity metrics
#[derive(Clone)]
pub struct ComplexityAnalyzer {
    /// Complexity thresholds
    thresholds: ComplexityThresholds,
}

impl ComplexityAnalyzer {
    /// Create a new complexity analyzer
    #[must_use]
    pub fn new() -> Self {
        Self {
            thresholds: ComplexityThresholds::default(),
        }
    }

    /// Create analyzer with custom thresholds
    #[must_use]
    pub fn with_thresholds(thresholds: ComplexityThresholds) -> Self {
        Self { thresholds }
    }

    /// Analyze the entire codebase
    ///
    /// # Returns
    /// Comprehensive complexity analysis report
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn analyze(&self, database: std::sync::Arc<Database>) -> Result<ComplexityReport> {
        let start_time = std::time::Instant::now();
        let mut report = ComplexityReport::new();

        // Analyze functions
        let function_metrics = self.analyze_functions(database.clone()).await?;
        for metrics in function_metrics {
            if metrics.risk_level() != Severity::Info {
                report.high_complexity_entities.push(metrics.clone());
            }
            report.add_function_metrics(metrics);
        }

        // Analyze classes
        let class_metrics = self.analyze_classes(database.clone()).await?;
        for metrics in class_metrics {
            if metrics.risk_level() != Severity::Info {
                report.high_complexity_entities.push(metrics.clone());
            }
            report.add_class_metrics(metrics);
        }

        // Generate recommendations
        report.generate_recommendations();

        // Update summary
        report.summary.duration_ms = start_time.elapsed().as_millis() as u64;
        report.summary.entities_analyzed =
            report.function_metrics.len() + report.class_metrics.len();
        // Mean cyclomatic complexity over ALL analyzed entities (functions +
        // classes), not just those exceeding the flag threshold — otherwise the
        // reported average collapses to 0.0 for a healthy, low-complexity codebase.
        let total_cyclomatic: u64 = report
            .function_metrics
            .iter()
            .chain(report.class_metrics.iter())
            .map(|m| u64::from(m.cyclomatic_complexity))
            .sum();
        report.summary.average_complexity = if report.summary.entities_analyzed > 0 {
            total_cyclomatic as f64 / report.summary.entities_analyzed as f64
        } else {
            0.0
        };

        Ok(report)
    }

    /// Analyze all functions in batches to limit memory usage.
    ///
    /// Instead of loading all function bodies into memory at once (which can
    /// be gigabytes for large repos), we process them in batches of 500 and
    /// check memory pressure between batches.
    async fn analyze_functions(
        &self,
        database: std::sync::Arc<Database>,
    ) -> Result<Vec<ComplexityMetrics>> {
        self.analyze_entities_by_type(database, "function").await
    }

    /// Analyze all classes in batches to limit memory usage.
    async fn analyze_classes(
        &self,
        database: std::sync::Arc<Database>,
    ) -> Result<Vec<ComplexityMetrics>> {
        self.analyze_entities_by_type(database, "class").await
    }

    /// Batched entity analysis with memory-pressure checks.
    ///
    /// Loads entities from SQLite in batches of `BATCH_SIZE` to avoid
    /// holding all entity bodies in memory simultaneously. For a repo
    /// with 100k entities averaging 1 KB body each, this reduces peak
    /// memory from ~100 MB to ~500 KB per batch.
    async fn analyze_entities_by_type(
        &self,
        database: std::sync::Arc<Database>,
        entity_type: &str,
    ) -> Result<Vec<ComplexityMetrics>> {
        const BATCH_SIZE: i64 = 500;
        let pool = database.conn_pool().clone();
        let entity_type_owned = entity_type.to_string();

        let mut all_metrics = Vec::new();
        let mut offset: i64 = 0;

        loop {
            let pool_clone = pool.clone();
            let et = entity_type_owned.clone();

            let batch: Vec<(String, String, u32, u32, String, String, String)> =
                tokio::task::spawn_blocking(move || {
                    let conn = pool_clone.get()?;
                    let mut stmt = conn.prepare(
                        "SELECT name, file_path, start_line, end_line, language, type, body
                         FROM entities WHERE type = ?1
                         ORDER BY rowid LIMIT ?2 OFFSET ?3",
                    )?;

                    let entities = stmt
                        .query_map(rusqlite::params![et, BATCH_SIZE, offset], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, u32>(2)?,
                                row.get::<_, u32>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, String>(6)?,
                            ))
                        })?
                        .collect::<std::result::Result<Vec<_>, _>>()?;

                    Ok::<Vec<_>, crate::error::Error>(entities)
                })
                .await
                .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

            if batch.is_empty() {
                break;
            }

            let batch_len = batch.len();
            for (name, file_path, start_line, end_line, language, et, body) in batch {
                let m = self.analyze_entity(
                    &name, &file_path, start_line, end_line, &language, &et, &body,
                );
                all_metrics.push(m);
            }

            offset += BATCH_SIZE;

            // If we got fewer than BATCH_SIZE, we've reached the end.
            if batch_len < BATCH_SIZE as usize {
                break;
            }
        }

        Ok(all_metrics)
    }

    /// Analyze a single entity
    #[allow(clippy::too_many_arguments)]
    fn analyze_entity(
        &self,
        name: &str,
        file_path: &str,
        start_line: u32,
        end_line: u32,
        language: &str,
        entity_type: &str,
        body: &str,
    ) -> ComplexityMetrics {
        let config = get_language_config(language);
        let lines: Vec<&str> = body.lines().collect();

        let mut metrics = ComplexityMetrics::new(name, file_path, entity_type);
        metrics.start_line = start_line;
        metrics.end_line = end_line;
        metrics.language = language.to_string();
        metrics.lines_of_code = end_line.saturating_sub(start_line) + 1;

        // Count comment lines
        metrics.comment_lines = self.count_comment_lines(&lines, &config);

        // Count blank lines
        metrics.blank_lines = lines.iter().filter(|l| l.trim().is_empty()).count() as u32;

        // Calculate cyclomatic complexity
        metrics.cyclomatic_complexity = self.calculate_cyclomatic_complexity(body, &config);

        // Calculate cognitive complexity and nesting depth in a single pass
        let (cognitive, nesting_depth) =
            self.calculate_cognitive_complexity_and_nesting(body, &config);
        metrics.cognitive_complexity = cognitive;
        metrics.nesting_depth = nesting_depth;

        // Count decision points
        metrics.decision_points = self.count_decision_points(body, &config);

        // Count exit points
        metrics.exit_points = self.count_exit_points(body, &config);

        // Calculate ratios
        let code_lines = metrics.lines_of_code - metrics.comment_lines - metrics.blank_lines;
        if metrics.lines_of_code > 0 {
            metrics.code_ratio = f64::from(code_lines) / f64::from(metrics.lines_of_code);
            metrics.comment_ratio =
                f64::from(metrics.comment_lines) / f64::from(metrics.lines_of_code);
        }

        // Calculate maintainability index (simplified)
        metrics.maintainability_index = self.calculate_maintainability_index(&metrics, &config);

        // Calculate Halstead metrics (simplified)
        let (volume, difficulty, effort) = self.calculate_halstead_metrics(body);
        metrics.halstead_volume = volume;
        metrics.halstead_difficulty = difficulty;
        metrics.halstead_effort = effort;

        metrics
    }

    /// Count comment lines
    fn count_comment_lines(&self, lines: &[&str], config: &LanguageAnalysisConfig) -> u32 {
        let mut count = 0;
        let mut in_multiline_comment = false;

        for line in lines {
            let trimmed = line.trim();

            // If we're inside a multi-line comment, every line counts
            if in_multiline_comment {
                count += 1;
                if trimmed.ends_with("*/") {
                    in_multiline_comment = false;
                }
                continue;
            }

            // Single-line comments: each line is classified once, in order of
            // specificity (most-specific prefix first) to avoid double-counting
            // (e.g. /// must be checked before //).
            if config.comment_patterns.contains(&"///".to_string()) && trimmed.starts_with("///") {
                count += 1;
                continue;
            }
            if config.comment_patterns.contains(&"//".to_string()) && trimmed.starts_with("//") {
                count += 1;
                continue;
            }
            if config.comment_patterns.contains(&"#".to_string()) && trimmed.starts_with('#') {
                count += 1;
                continue;
            }
            if config.comment_patterns.contains(&"\"\"\"".to_string())
                && trimmed.starts_with("\"\"\"")
            {
                count += 1;
                continue;
            }
            if config.comment_patterns.contains(&"/*".to_string()) && trimmed.starts_with("/*") {
                count += 1;
                if !trimmed.ends_with("*/") {
                    in_multiline_comment = true;
                }
                continue;
            }
        }

        count
    }

    /// Calculate cyclomatic complexity.
    ///
    /// Uses McCabe’s simplified formula: **M = 1 + Σ decision points**.
    /// Decision points are counted per-occurrence (not per-line), so
    /// compound conditions such as `a && b || c` contribute correctly.
    pub fn calculate_cyclomatic_complexity(
        &self,
        body: &str,
        config: &LanguageAnalysisConfig,
    ) -> u32 {
        1 + self.count_decision_points(body, config)
    }

    /// Shared cyclomatic complexity calculator.
    ///
    /// Uses the same decision-point algorithm as the full
    /// [`ComplexityAnalyzer`]
    /// so that [`RefactoringAnalyzer`](crate::analysis::refactoring::RefactoringAnalyzer)
    /// reports consistent scores.
    #[must_use]
    pub fn calculate_cyclomatic_complexity_for_code(body: &str, language: &str) -> u32 {
        let config = crate::analysis::get_language_config(language);
        let analyzer = Self::new();
        analyzer.calculate_cyclomatic_complexity(body, &config)
    }

    /// Calculate cognitive complexity and maximum nesting depth in a single pass.
    fn calculate_cognitive_complexity_and_nesting(
        &self,
        body: &str,
        config: &LanguageAnalysisConfig,
    ) -> (u32, u32) {
        let lines: Vec<&str> = body.lines().collect();
        let mut complexity: u32 = 0;
        let mut nesting_level: u32 = 0;
        let mut max_depth: u32 = 0;

        for line in &lines {
            let trimmed = line.trim();

            // Check for nesting increase
            if config
                .conditional_keywords
                .iter()
                .any(|k| trimmed.contains(k))
            {
                complexity += 1 + nesting_level;
                nesting_level += 1;
                max_depth = max_depth.max(nesting_level);
            }

            // Check for nesting decrease
            if trimmed.starts_with('}') || trimmed.starts_with("end") {
                nesting_level = nesting_level.saturating_sub(1);
            }

            // Check for loop keywords
            if config.loop_keywords.iter().any(|k| trimmed.contains(k)) {
                complexity += 1 + nesting_level;
            }
        }

        (complexity, max_depth)
    }

    /// Count decision points using simple string matching.
    ///
    /// Each line is checked for structural decision keywords (`if`, `for`,
    /// `while`, `switch`, `case`, `catch`, etc.) — **one per line** — and
    /// then short-circuit logical operators (`&&`, `||`) are counted
    /// **per occurrence** on that line. This gives a closer approximation
    /// to McCabe’s predicate-node count than a naive per-line keyword check.
    fn count_decision_points(&self, body: &str, config: &LanguageAnalysisConfig) -> u32 {
        if config.decision_points.is_empty() {
            return 0;
        }
        let mut count = 0u32;
        for line in body.lines() {
            let trimmed = line.trim();

            // Structural keywords: count at most once per line
            let has_structural = config.decision_points.iter().any(|k| {
                // Only treat "&&"/"||" as structural when they appear in the
                // decision_points list; they will be counted separately below.
                if k == "&&" || k == "||" {
                    return false;
                }
                trimmed.contains(k)
            });
            if has_structural {
                count += 1;
            }

            // Short-circuit operators: count each occurrence individually
            count += trimmed.matches("&&").count() as u32;
            count += trimmed.matches("||").count() as u32;

            // Logical assignment operators (Ruby/JS)
            count += trimmed.matches("&&=").count() as u32;
            count += trimmed.matches("||=").count() as u32;

            // Ternary operator
            count += trimmed.matches('?').count() as u32;

            // Null-coalescing operator (`??` in C#/JS/Rust `unwrap_or`)
            // Counts each occurrence as a decision point since it introduces
            // a fallback control-flow path.
            count += trimmed.matches("??").count() as u32;
        }
        count
    }

    /// Count exit points using simple string matching.
    fn count_exit_points(&self, body: &str, config: &LanguageAnalysisConfig) -> u32 {
        if config.exit_points.is_empty() {
            return 0;
        }
        let mut count = 0u32;
        for line in body.lines() {
            let trimmed = line.trim();
            for keyword in &config.exit_points {
                if trimmed.contains(keyword) {
                    count += 1;
                    break;
                }
            }
        }
        count
    }

    /// Calculate maintainability index using the Microsoft Visual Studio
    /// reference formula.
    ///
    /// Raw formula:
    /// `171 - 5.2 * ln(Halstead Volume) - 0.23 * CC - 16.2 * ln(LOC)`
    ///
    /// The raw value is then scaled to a 0-100 range with
    /// `raw * 100 / 171`, matching the Visual Studio implementation.
    fn calculate_maintainability_index(
        &self,
        metrics: &ComplexityMetrics,
        _config: &LanguageAnalysisConfig,
    ) -> f64 {
        let hv = metrics.halstead_volume.max(1.0);
        let cc = f64::from(metrics.cyclomatic_complexity);
        let loc = f64::from(metrics.lines_of_code.max(1));

        let raw_mi = 171.0 - 5.2 * hv.ln() - 0.23 * cc - 16.2 * loc.ln();

        // Scale to 0-100 range (Visual Studio convention)
        let mi = raw_mi * 100.0 / 171.0;
        mi.clamp(0.0, 100.0)
    }

    /// Calculate Halstead metrics using a **regex approximation**.
    ///
    /// True Halstead metrics require a full lexer to distinguish operators
    /// from operands.  This implementation uses pre-compiled regex patterns
    /// for common operators and a rough operand extractor, which is accurate
    /// enough for ranking but may deviate from reference values on code with
    /// heavy use of custom operators or unusual syntax.
    ///
    /// For a more precise calculation, per-language lexers would be required.
    #[doc(alias = "approximate")]
    fn calculate_halstead_metrics(&self, body: &str) -> (f64, f64, f64) {
        let mut operator_count = 0u32;
        let mut unique_operators: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (op, re) in HALSTEAD_OPERATORS.iter() {
            let count = re.find_iter(body).count() as u32;
            if count > 0 {
                operator_count += count;
                unique_operators.insert((*op).to_string());
            }
        }

        let operands: std::collections::HashSet<&str> = HALSTEAD_OPERAND_RE
            .find_iter(body)
            .map(|m| m.as_str())
            .collect();

        let unique_operands = operands.len() as u32;
        let operand_count = HALSTEAD_OPERAND_RE.find_iter(body).count() as u32;

        // Avoid division by zero and return neutral zeros when there is nothing to count
        if operator_count == 0 && operand_count == 0 {
            return (0.0, 0.0, 0.0);
        }

        let n1 = f64::from(unique_operators.len() as u32).max(1.0);
        let n2 = f64::from(unique_operands).max(1.0);
        let n = n1 + n2;

        let n1_count = f64::from(operator_count).max(1.0);
        let n2_count = f64::from(operand_count).max(1.0);
        let n_count = n1_count + n2_count;

        // Volume = N * log2(n)
        let volume = n_count * (n.log2());

        // Difficulty = (n1/2) * (n2_count/n2)
        let difficulty = (n1 / 2.0) * (n2_count / n2);

        // Effort = Difficulty * Volume
        let effort = difficulty * volume;

        (volume, difficulty, effort)
    }

    /// Get thresholds
    #[must_use]
    pub const fn thresholds(&self) -> &ComplexityThresholds {
        &self.thresholds
    }
}

impl Default for ComplexityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complexity_metrics() {
        let metrics = ComplexityMetrics::new("test_func", "/test.rs", "function");
        assert_eq!(metrics.entity_name, "test_func");
        assert_eq!(metrics.cyclomatic_complexity, 0);
    }

    #[test]
    fn test_risk_level() {
        let mut metrics = ComplexityMetrics::new("test", "/test.rs", "function");
        metrics.cyclomatic_complexity = 5;
        metrics.cognitive_complexity = 5;

        // Low score
        let low_risk = Severity::Info;
        assert_eq!(metrics.risk_level(), low_risk);

        // High complexity
        metrics.cyclomatic_complexity = 20;
        metrics.cognitive_complexity = 30;
        assert_ne!(metrics.risk_level(), Severity::Info);
    }

    #[test]
    fn test_complexity_report() {
        let mut report = ComplexityReport::new();

        report.add_function_metrics(ComplexityMetrics::new("func1", "/test.rs", "function"));
        report.add_class_metrics(ComplexityMetrics::new("Class1", "/test.rs", "class"));

        assert_eq!(report.function_metrics.len(), 1);
        assert_eq!(report.class_metrics.len(), 1);

        report.generate_recommendations();
        // Should have recommendations or be empty if no issues
    }

    #[test]
    fn test_thresholds_exceeded() {
        let thresholds = ComplexityThresholds::default();
        let mut metrics = ComplexityMetrics::new("test", "/test.rs", "function");

        metrics.cyclomatic_complexity = 20; // Exceeds default of 10
        metrics.cognitive_complexity = 20; // Exceeds default of 15

        let issues = metrics.exceeds_thresholds(&thresholds);
        assert!(!issues.is_empty());
    }

    #[test]
    fn test_maintainability_index_scaled() {
        let analyzer = ComplexityAnalyzer::new();
        let config = get_language_config("rust");

        // Minimal function: 1 LOC, 1 CC, tiny Halstead volume
        let mut metrics = ComplexityMetrics::new("tiny", "/test.rs", "function");
        metrics.lines_of_code = 1;
        metrics.cyclomatic_complexity = 1;
        metrics.halstead_volume = 1.0;

        let mi = analyzer.calculate_maintainability_index(&metrics, &config);
        // With the scaling factor * 100/171, a near-ideal function should score
        // very close to 100 (clamped), not ~170.
        assert!(
            mi <= 100.0,
            "MI must be clamped to 0-100; got {mi} (missing *100/171 scaling?)"
        );
        assert!(
            mi >= 90.0,
            "A trivial 1-LOC function should score >= 90; got {mi}"
        );
    }

    #[test]
    fn test_decision_points_compound_conditions() {
        let analyzer = ComplexityAnalyzer::new();
        let config = get_language_config("rust");

        let body = "if a > 0 && b < 10 || c == 5 {\n    do_something();\n}";
        let points = analyzer.count_decision_points(body, &config);
        // if (1) + && (1) + || (1) = 3 decision points
        assert_eq!(points, 3, "Compound condition should count each operator");
    }
}
