//! Cross-Reference Analyzer for Devscriptor
//!
//! This module provides functionality to track all usages of functions, classes,
//! and other entities across the codebase, providing comprehensive cross-reference
//! analysis with context information.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analysis::Severity;
use crate::analysis::api::AnalysisSummary;
use crate::database::Database;
use crate::error::Result;

/// Type of reference between code entities
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceType {
    /// Function/method call
    Call,
    /// Import or include
    Import,
    /// Class inheritance
    Inheritance,
    /// Interface implementation
    Implementation,
    /// Attribute/field access
    Attribute,
    /// Object instantiation
    Instantiation,
    /// Variable reference
    Variable,
    /// Type reference
    Type,
    /// Generic/other reference
    Other,
}

impl ReferenceType {
    /// Convert to string representation
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Import => "import",
            Self::Inheritance => "inheritance",
            Self::Implementation => "implementation",
            Self::Attribute => "attribute",
            Self::Instantiation => "instantiation",
            Self::Variable => "variable",
            Self::Type => "type",
            Self::Other => "other",
        }
    }

    /// Parse from string
    ///
    /// # Errors
    /// Returns an error if the string is not a valid reference type
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> crate::error::Result<Self> {
        match s {
            "call" => Ok(Self::Call),
            "import" => Ok(Self::Import),
            "inheritance" => Ok(Self::Inheritance),
            "implementation" => Ok(Self::Implementation),
            "attribute" => Ok(Self::Attribute),
            "instantiation" => Ok(Self::Instantiation),
            "variable" => Ok(Self::Variable),
            "type" => Ok(Self::Type),
            "other" => Ok(Self::Other),
            _ => Err(crate::error::Error::Analysis(format!(
                "Invalid reference type: {s}"
            ))),
        }
    }
}

/// Represents a cross-reference between code entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossReference {
    /// Source entity name
    pub source_entity_name: String,
    /// Source entity type
    pub source_entity_type: String,
    /// Source file path
    pub source_file_path: String,
    /// Target entity name
    pub target_entity_name: String,
    /// Target entity type
    pub target_entity_type: String,
    /// Target file path
    pub target_file_path: String,
    /// Reference type
    pub reference_type: ReferenceType,
    /// The line the reference is made on — the line the referencing entity
    /// starts on, where the index records no line of the reference itself.
    pub line_number: u32,
    /// The line the referenced entity starts on.
    pub target_line_number: u32,
    /// Context (the line of code)
    pub context: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f64,
}

impl CrossReference {
    /// Create a new cross-reference
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        source_entity_name: impl Into<String>,
        source_file_path: impl Into<String>,
        target_entity_name: impl Into<String>,
        target_file_path: impl Into<String>,
        reference_type: ReferenceType,
        line_number: u32,
    ) -> Self {
        Self {
            source_entity_name: source_entity_name.into(),
            source_entity_type: "function".to_string(),
            source_file_path: source_file_path.into(),
            target_entity_name: target_entity_name.into(),
            target_entity_type: "function".to_string(),
            target_file_path: target_file_path.into(),
            reference_type,
            line_number,
            target_line_number: 0,
            context: String::new(),
            confidence: 1.0,
        }
    }

    /// Set the line the referenced entity starts on
    #[must_use]
    pub fn with_target_line(mut self, line: u32) -> Self {
        self.target_line_number = line;
        self
    }

    /// Set context
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }

    /// Set confidence
    #[must_use]
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set entity types
    #[must_use]
    pub fn with_entity_types(
        mut self,
        source_type: impl Into<String>,
        target_type: impl Into<String>,
    ) -> Self {
        self.source_entity_type = source_type.into();
        self.target_entity_type = target_type.into();
        self
    }

    /// Get unique identifier for this reference
    #[must_use]
    pub fn unique_id(&self) -> String {
        format!(
            "{}:{}->{}:{}@{}:{:?}",
            self.source_file_path,
            self.source_entity_name,
            self.target_file_path,
            self.target_entity_name,
            self.line_number,
            self.reference_type
        )
    }
}

/// Statistics about entity usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStatistics {
    /// Entity name
    pub entity_name: String,
    /// Entity type
    pub entity_type: String,
    /// File path
    pub file_path: String,
    /// The line it starts on
    pub line: u32,
    /// Number of incoming references
    pub incoming_count: usize,
    /// Number of outgoing references
    pub outgoing_count: usize,
    /// Referenced by (source entities)
    pub referenced_by: Vec<String>,
    /// References to (target entities)
    pub references_to: Vec<String>,
}

impl UsageStatistics {
    /// Create new usage statistics
    #[must_use]
    pub fn new(
        entity_name: impl Into<String>,
        entity_type: impl Into<String>,
        file_path: impl Into<String>,
        line: u32,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            entity_type: entity_type.into(),
            file_path: file_path.into(),
            line,
            incoming_count: 0,
            outgoing_count: 0,
            referenced_by: Vec::new(),
            references_to: Vec::new(),
        }
    }

    /// Add incoming reference
    pub fn add_incoming(&mut self, source: impl Into<String>) {
        self.incoming_count += 1;
        self.referenced_by.push(source.into());
    }

    /// Add outgoing reference
    pub fn add_outgoing(&mut self, target: impl Into<String>) {
        self.outgoing_count += 1;
        self.references_to.push(target.into());
    }
}

/// Comprehensive cross-reference analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossReferenceReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// All cross-references
    pub references: Vec<CrossReference>,
    /// Usage statistics by entity
    pub usage_statistics: HashMap<String, UsageStatistics>,
    /// References grouped by file
    pub references_by_file: HashMap<String, Vec<CrossReference>>,
    /// Unused entities (no incoming references)
    pub unused_entities: Vec<UsageStatistics>,
    /// Highly used entities
    pub highly_used_entities: Vec<UsageStatistics>,
    /// Whether the highly-used list was truncated to the top 20.
    pub highly_used_truncated: bool,
    /// Recommendations
    pub recommendations: Vec<String>,
}

impl CrossReferenceReport {
    /// Create a new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            references: Vec::new(),
            usage_statistics: HashMap::new(),
            references_by_file: HashMap::new(),
            unused_entities: Vec::new(),
            highly_used_entities: Vec::new(),
            highly_used_truncated: false,
            recommendations: Vec::new(),
        }
    }

    /// Add a cross-reference
    pub fn add_reference(&mut self, reference: CrossReference) {
        self.references.push(reference);
        self.summary.add_finding(Severity::Info);
    }

    /// Add usage statistics
    pub fn add_usage_stats(&mut self, stats: UsageStatistics) {
        let key = format!(
            "{}::{}:: {}",
            stats.file_path, stats.entity_name, stats.entity_type
        );
        self.usage_statistics.insert(key, stats);
    }

    /// Generate recommendations based on findings
    pub fn generate_recommendations(&mut self) {
        let mut recs = Vec::new();

        if !self.unused_entities.is_empty() {
            recs.push(format!(
                "Found {} potentially unused entities. Consider reviewing for dead code elimination.",
                self.unused_entities.len()
            ));
        }

        if !self.highly_used_entities.is_empty() {
            recs.push(format!(
                "Found {} highly-used entities (hotspots). Consider monitoring for performance impact.",
                self.highly_used_entities.len()
            ));
        }

        if recs.is_empty() {
            recs.push("Cross-reference analysis complete - no major issues found.".to_string());
        }

        self.recommendations = recs;
    }
}

impl Default for CrossReferenceReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Analyzer for cross-references between code entities
#[derive(Clone)]
pub struct CrossReferenceAnalyzer {
    /// Minimum confidence threshold for references
    confidence_threshold: f64,
    /// Maximum references to track per entity
    max_references: usize,
}

impl CrossReferenceAnalyzer {
    /// Create a new cross-reference analyzer
    #[must_use]
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.8,
            max_references: 1000,
        }
    }

    /// Create analyzer with custom settings
    #[must_use]
    pub fn with_settings(confidence_threshold: f64, max_references: usize) -> Self {
        Self {
            confidence_threshold: confidence_threshold.clamp(0.0, 1.0),
            max_references,
        }
    }

    /// Analyze the codebase for cross-references
    ///
    /// # Returns
    /// Cross-reference analysis report
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn analyze(&self, database: &Database) -> Result<CrossReferenceReport> {
        let start_time = std::time::Instant::now();
        let mut report = CrossReferenceReport::new();

        // Build cross-references from database
        let references = self.build_references(database).await?;

        for reference in references {
            report.add_reference(reference);
        }

        // Build usage statistics
        let stats = self.build_usage_statistics(&report.references);
        for (key, stat) in stats {
            report.usage_statistics.insert(key, stat);
        }

        // Group references by file
        report.references_by_file = self.group_by_file(&report.references);

        // Find unused entities
        report.unused_entities = self.find_unused_entities(&report.usage_statistics);

        // Find highly used entities
        let (highly_used, truncated) = self.find_highly_used(&report.usage_statistics);
        report.highly_used_entities = highly_used;
        report.highly_used_truncated = truncated;

        // Generate recommendations
        report.generate_recommendations();

        // Update summary
        report.summary.duration_ms = start_time.elapsed().as_millis() as u64;
        report.summary.entities_analyzed = report.usage_statistics.len();

        Ok(report)
    }

    /// Build cross-references from database: one per forward edge — a call,
    /// a use, a reference, a supertype — between the entities at its ends,
    /// each with its file and the line it starts on. Reverse rows (`called_by`,
    /// `used_by`, …) are not references of their own, or a caller would be
    /// referenced by what it calls; and the target's file is set, so an
    /// entity's records as a source and as a target meet.
    async fn build_references(&self, database: &Database) -> Result<Vec<CrossReference>> {
        let pool = database.conn_pool().clone();

        type Row = (
            String,
            String,
            String,
            u32,
            String,
            String,
            String,
            u32,
            String,
        );
        let relationships: Vec<Row> = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT e1.name, e1.type, e1.file_path, e1.start_line,
                        e2.name, e2.type, e2.file_path, e2.start_line, r.relationship_type
                 FROM relationships r
                 JOIN entities e1 ON r.source_id = e1.id
                 JOIN entities e2 ON r.target_id = e2.id
                 WHERE r.relationship_type IN
                     ('calls', 'uses', 'references', 'inherits', 'implements')",
            )?;
            let results = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok::<Vec<_>, crate::error::Error>(results)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        let references = relationships
            .into_iter()
            .map(
                |(
                    source_name,
                    source_type,
                    source_path,
                    source_line,
                    target_name,
                    target_type,
                    target_path,
                    target_line,
                    rel_type,
                )| {
                    let reference_type = match crate::entities::RelationshipType::parse(&rel_type) {
                        Ok(crate::entities::RelationshipType::Calls) => ReferenceType::Call,
                        Ok(crate::entities::RelationshipType::Inherits) => {
                            ReferenceType::Inheritance
                        }
                        Ok(crate::entities::RelationshipType::Implements) => {
                            ReferenceType::Implementation
                        }
                        Ok(crate::entities::RelationshipType::Uses) => ReferenceType::Variable,
                        _ => ReferenceType::Type,
                    };
                    CrossReference::new(
                        &source_name,
                        &source_path,
                        &target_name,
                        &target_path,
                        reference_type,
                        source_line,
                    )
                    .with_target_line(target_line)
                    .with_entity_types(&source_type, &target_type)
                },
            )
            .collect();

        Ok(references)
    }

    /// Build usage statistics from references
    fn build_usage_statistics(
        &self,
        references: &[CrossReference],
    ) -> HashMap<String, UsageStatistics> {
        let mut stats: HashMap<String, UsageStatistics> = HashMap::new();

        for reference in references {
            // Update source entity stats (outgoing)
            let source_key = format!(
                "{}::{}:: {}",
                reference.source_file_path,
                reference.source_entity_name,
                reference.source_entity_type
            );

            stats
                .entry(source_key.clone())
                .or_insert_with(|| {
                    UsageStatistics::new(
                        &reference.source_entity_name,
                        &reference.source_entity_type,
                        &reference.source_file_path,
                        reference.line_number,
                    )
                })
                .add_outgoing(format!(
                    "{}:: {}",
                    reference.target_file_path, reference.target_entity_name
                ));

            // Update target entity stats (incoming)
            let target_key = format!(
                "{}::{}:: {}",
                reference.target_file_path,
                reference.target_entity_name,
                reference.target_entity_type
            );

            stats
                .entry(target_key.clone())
                .or_insert_with(|| {
                    UsageStatistics::new(
                        &reference.target_entity_name,
                        &reference.target_entity_type,
                        &reference.target_file_path,
                        reference.target_line_number,
                    )
                })
                .add_incoming(format!(
                    "{}:: {}",
                    reference.source_file_path, reference.source_entity_name
                ));
        }

        stats
    }

    /// Group references by source file
    fn group_by_file(&self, references: &[CrossReference]) -> HashMap<String, Vec<CrossReference>> {
        let mut by_file: HashMap<String, Vec<CrossReference>> = HashMap::new();

        for reference in references {
            by_file
                .entry(reference.source_file_path.clone())
                .or_default()
                .push(reference.clone());
        }

        by_file
    }

    /// Find unused entities (no incoming references)
    fn find_unused_entities(
        &self,
        stats: &HashMap<String, UsageStatistics>,
    ) -> Vec<UsageStatistics> {
        stats
            .values()
            .filter(|s| s.incoming_count == 0)
            .cloned()
            .collect()
    }

    /// Find highly used entities (many incoming references).
    ///
    /// Returns `(entities, truncated)` where `truncated` is `true` when more
    /// than 20 entities matched and the result was cut to the top 20.
    fn find_highly_used(
        &self,
        stats: &HashMap<String, UsageStatistics>,
    ) -> (Vec<UsageStatistics>, bool) {
        let mut highly_used: Vec<UsageStatistics> = stats
            .values()
            .filter(|s| s.incoming_count >= 10)
            .cloned()
            .collect();

        // Sort by incoming count descending
        highly_used.sort_by_key(|b| std::cmp::Reverse(b.incoming_count));

        let truncated = highly_used.len() > 20;
        highly_used.truncate(20); // Top 20

        (highly_used, truncated)
    }

    /// Get confidence threshold
    #[must_use]
    pub const fn confidence_threshold(&self) -> f64 {
        self.confidence_threshold
    }

    /// Get max references setting
    #[must_use]
    pub const fn max_references(&self) -> usize {
        self.max_references
    }
}

impl Default for CrossReferenceAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reference_type_from_str() {
        assert!(matches!(
            ReferenceType::from_str("call").unwrap(),
            ReferenceType::Call
        ));
        assert!(matches!(
            ReferenceType::from_str("import").unwrap(),
            ReferenceType::Import
        ));
        assert!(ReferenceType::from_str("invalid").is_err());
    }

    #[test]
    fn test_cross_reference_creation() {
        let reference = CrossReference::new(
            "func1",
            "/test.rs",
            "func2",
            "/test.rs",
            ReferenceType::Call,
            42,
        )
        .with_context("func1.call(func2)")
        .with_confidence(0.95);

        assert_eq!(reference.source_entity_name, "func1");
        assert_eq!(reference.target_entity_name, "func2");
        assert_eq!(reference.line_number, 42);
        assert_eq!(reference.context, "func1.call(func2)");
        assert!((reference.confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn test_usage_statistics() {
        let mut stats = UsageStatistics::new("MyClass", "class", "/test.py", 3);

        stats.add_incoming("func1");
        stats.add_incoming("func2");
        stats.add_outgoing("BaseClass");

        assert_eq!(stats.incoming_count, 2);
        assert_eq!(stats.outgoing_count, 1);
        assert_eq!(stats.referenced_by.len(), 2);
        assert_eq!(stats.references_to.len(), 1);
    }

    #[test]
    fn test_cross_reference_report() {
        let mut report = CrossReferenceReport::new();

        report.add_reference(CrossReference::new(
            "a",
            "/a.rs",
            "b",
            "/b.rs",
            ReferenceType::Call,
            10,
        ));

        assert_eq!(report.references.len(), 1);
        assert_eq!(report.summary.total_findings, 1);
    }
}
