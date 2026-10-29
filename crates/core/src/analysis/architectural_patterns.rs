//! Architectural Pattern Recognition Module for Devscriptor-Core
//!
//! This module provides functionality to recognize common architectural patterns
//! and design patterns in source code, helping understand the codebase structure.

use crate::entities::Entity;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Helper function to get body from an entity.
///
/// Uses the shared `match_entity!` macro so adding a new entity variant
/// only requires a single-line change in [`entities.rs`](crate::entities).
fn get_entity_body(entity: &crate::entities::Entity) -> &str {
    crate::match_entity!(entity, e => &e.body)
}

/// Represents a detected architectural pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitecturalPattern {
    /// Pattern name
    pub name: String,
    /// Pattern type (creational, structural, behavioral, architectural)
    pub pattern_type: String,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
    /// Description of the pattern
    pub description: String,
    /// Names of participating classes/structs
    pub participating_classes: Vec<String>,
    /// Names of participating functions
    pub participating_functions: Vec<String>,
    /// File paths where pattern was detected
    pub file_paths: Vec<String>,
    /// Specific code evidence
    pub evidence: Vec<String>,
    /// Benefits of this pattern
    pub benefits: Vec<String>,
    /// Considerations for this pattern
    pub considerations: Vec<String>,
}

impl ArchitecturalPattern {
    /// Create a new architectural pattern
    pub fn new(
        name: impl Into<String>,
        pattern_type: impl Into<String>,
        confidence: f64,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            pattern_type: pattern_type.into(),
            confidence: confidence.clamp(0.0, 1.0),
            description: description.into(),
            participating_classes: Vec::new(),
            participating_functions: Vec::new(),
            file_paths: Vec::new(),
            evidence: Vec::new(),
            benefits: Vec::new(),
            considerations: Vec::new(),
        }
    }

    /// Add a participating class
    pub fn with_class(mut self, class: impl Into<String>) -> Self {
        self.participating_classes.push(class.into());
        self
    }

    /// Add a participating function
    pub fn with_function(mut self, func: impl Into<String>) -> Self {
        self.participating_functions.push(func.into());
        self
    }

    /// Add a file path
    pub fn with_file_path(mut self, path: impl Into<String>) -> Self {
        self.file_paths.push(path.into());
        self
    }

    /// Add evidence
    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence.push(evidence.into());
        self
    }

    /// Add a benefit
    pub fn with_benefit(mut self, benefit: impl Into<String>) -> Self {
        self.benefits.push(benefit.into());
        self
    }

    /// Add a consideration
    pub fn with_consideration(mut self, consideration: impl Into<String>) -> Self {
        self.considerations.push(consideration.into());
        self
    }
}

/// Pattern definition for detection
#[derive(Debug, Clone)]
struct PatternDefinition {
    name: String,
    pattern_type: String,
    description: String,
    class_patterns: Vec<Regex>,
    function_patterns: Vec<Regex>,
    benefits: Vec<String>,
    considerations: Vec<String>,
}

/// Comprehensive architectural pattern analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternReport {
    /// Summary statistics
    pub summary: HashMap<String, serde_json::Value>,
    /// Detected patterns
    pub detected_patterns: Vec<ArchitecturalPattern>,
    /// Pattern distribution by type
    pub pattern_distribution: HashMap<String, usize>,
    /// Architectural smells detected
    pub architectural_smells: Vec<String>,
    /// Recommendations
    pub recommendations: Vec<String>,
    /// Complexity analysis
    pub complexity_analysis: HashMap<String, serde_json::Value>,
}

impl PatternReport {
    /// Create a new pattern report
    pub fn new() -> Self {
        Self {
            summary: HashMap::new(),
            detected_patterns: Vec::new(),
            pattern_distribution: HashMap::new(),
            architectural_smells: Vec::new(),
            recommendations: Vec::new(),
            complexity_analysis: HashMap::new(),
        }
    }

    /// Add a detected pattern
    pub fn add_pattern(&mut self, pattern: ArchitecturalPattern) {
        let count = self
            .pattern_distribution
            .entry(pattern.pattern_type.clone())
            .or_insert(0);
        *count += 1;
        self.detected_patterns.push(pattern);
    }

    /// Get patterns by type
    pub fn patterns_by_type(&self, pattern_type: &str) -> Vec<&ArchitecturalPattern> {
        self.detected_patterns
            .iter()
            .filter(|p| p.pattern_type == pattern_type)
            .collect()
    }

    /// Generate summary statistics
    pub fn generate_summary(&mut self) {
        self.summary.insert(
            "total_patterns".to_string(),
            serde_json::json!(self.detected_patterns.len()),
        );
        self.summary.insert(
            "pattern_types".to_string(),
            serde_json::json!(self.pattern_distribution.len()),
        );

        let avg_confidence = if self.detected_patterns.is_empty() {
            0.0
        } else {
            self.detected_patterns
                .iter()
                .map(|p| p.confidence)
                .sum::<f64>()
                / self.detected_patterns.len() as f64
        };
        self.summary.insert(
            "average_confidence".to_string(),
            serde_json::json!(avg_confidence),
        );
    }
}

impl Default for PatternReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Recognizer for architectural and design patterns
#[derive(Debug)]
pub struct ArchitecturalPatternRecognizer {
    patterns: Vec<PatternDefinition>,
}

impl ArchitecturalPatternRecognizer {
    /// Create a new pattern recognizer with default patterns
    pub fn new() -> Self {
        let mut recognizer = Self {
            patterns: Vec::new(),
        };
        recognizer.initialize_patterns();
        recognizer
    }

    /// Initialize pattern definitions
    fn initialize_patterns(&mut self) {
        // Creational Patterns
        self.add_pattern(
            "singleton",
            "creational",
            "Ensures a class has only one instance and provides global access",
            vec![
                r"class\s+\w+.*?(?:private\s+static\s+\w+\s+instance|_instance|__instance__)",
                r"(?:fn|def|function)\s+(?:getInstance|get_instance|__new__)",
                r"(?:private|protected)\s+(?:__init__|new)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:getInstance|get_instance)",
                r"(?:static|class_method)\s+(?:fn|def)\s+(?:get|instance)",
            ],
            vec![
                "Controlled access to sole instance".to_string(),
                "Reduced memory footprint".to_string(),
                "Global point of access".to_string(),
            ],
            vec![
                "Global state can be problematic".to_string(),
                "Testing difficulties".to_string(),
                "Tight coupling".to_string(),
            ],
        );

        self.add_pattern(
            "factory",
            "creational",
            "Creates objects without specifying exact classes",
            vec![
                r"class\s+(?:\w*Factory\w*)",
                r"(?:interface|trait)\s+(?:\w*Factory\w*)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:create\w*|build\w*|make\w*)\s*\(",
                r"(?:static|class_method)\s+(?:fn|def)\s+create",
            ],
            vec![
                "Loose coupling".to_string(),
                "Extensibility".to_string(),
                "Code reuse".to_string(),
            ],
            vec!["Complexity".to_string(), "Parameter explosion".to_string()],
        );

        self.add_pattern(
            "builder",
            "creational",
            "Constructs complex objects step by step",
            vec![
                r"class\s+(?:\w*Builder\w*)",
                r"(?:struct|interface)\s+(?:\w*Builder\w*)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:build|construct)\s*\(",
                r"(?:fn|def|function)\s+(?:set\w+|with\w+)\s*\(",
            ],
            vec![
                "Clear construction process".to_string(),
                "Supports immutable objects".to_string(),
                "Flexible object construction".to_string(),
            ],
            vec!["Verbosity".to_string(), "Overhead".to_string()],
        );

        // Structural Patterns
        self.add_pattern(
            "adapter",
            "structural",
            "Allows incompatible interfaces to work together",
            vec![
                r"class\s+(?:\w*Adapter\w*)",
                r"(?:impl|implements)\s+\w*Target\w*",
            ],
            vec![r"(?:fn|def|function)\s+(?:adapt|convert|transform)\s*\("],
            vec![
                "Integration".to_string(),
                "Reusability".to_string(),
                "Flexibility".to_string(),
            ],
            vec!["Complexity".to_string(), "Performance overhead".to_string()],
        );

        self.add_pattern(
            "decorator",
            "structural",
            "Adds functionality to objects dynamically",
            vec![r"class\s+(?:\w*Decorator\w*)", r"(?:@|#)\w*decorator\w*"],
            vec![
                r"(?:fn|def|function)\s+wrap\s*\(",
                r"(?:fn|def|function)\s+__call__\s*\(",
            ],
            vec![
                "Flexibility".to_string(),
                "Open/closed principle".to_string(),
            ],
            vec!["Complexity".to_string(), "Debugging difficulty".to_string()],
        );

        self.add_pattern(
            "facade",
            "structural",
            "Provides simplified interface to complex subsystem",
            vec![r"class\s+(?:\w*Facade\w*)"],
            vec![r"(?:fn|def|function)\s+(?:simplify|delegate)\s*\("],
            vec![
                "Simplicity".to_string(),
                "Decoupling".to_string(),
                "Layering".to_string(),
            ],
            vec![
                "God object risk".to_string(),
                "Abstraction leakage".to_string(),
            ],
        );

        // Behavioral Patterns
        self.add_pattern(
            "observer",
            "behavioral",
            "Defines one-to-many dependency between objects",
            vec![
                r"class\s+(?:\w*Observer\w*|\w*Subject\w*)",
                r"(?:interface|trait)\s+(?:\w*Observer\w*|\w*Subject\w*)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:notify|subscribe|attach|detach)\s*\(",
                r"(?:fn|def|function)\s+(?:update|on_change)\s*\(",
            ],
            vec![
                "Loose coupling".to_string(),
                "Dynamic relationships".to_string(),
            ],
            vec![
                "Memory leaks potential".to_string(),
                "Update order issues".to_string(),
                "Performance overhead".to_string(),
            ],
        );

        self.add_pattern(
            "strategy",
            "behavioral",
            "Defines family of algorithms, encapsulates each one",
            vec![
                r"class\s+(?:\w*Strategy\w*)",
                r"(?:interface|trait)\s+(?:\w*Strategy\w*)",
            ],
            vec![r"(?:fn|def|function)\s+(?:execute|algorithm|process)\s*\("],
            vec![
                "Flexibility".to_string(),
                "Testability".to_string(),
                "Open/closed principle".to_string(),
            ],
            vec![
                "Number of classes".to_string(),
                "Context awareness needed".to_string(),
            ],
        );

        self.add_pattern(
            "command",
            "behavioral",
            "Encapsulates request as object",
            vec![
                r"class\s+(?:\w*Command\w*)",
                r"(?:interface|trait)\s+(?:\w*Command\w*)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:execute|run|perform)\s*\(",
                r"(?:fn|def|function)\s+(?:undo|reverse)\s*\(",
            ],
            vec![
                "Decoupling".to_string(),
                "Undo/redo support".to_string(),
                "Queuing".to_string(),
            ],
            vec!["Class explosion".to_string(), "Complexity".to_string()],
        );

        // Architectural Patterns
        self.add_pattern(
            "mvc",
            "architectural",
            "Model-View-Controller pattern",
            vec![
                r"(?:class|module)\s+(?:\w*Controller\w*)",
                r"(?:class|module)\s+(?:\w*Model\w*)",
                r"(?:class|module)\s+(?:\w*View\w*)",
            ],
            vec![
                r"(?:fn|def|function)\s+(?:render|display|show)\s*\(",
                r"(?:fn|def|function)\s+(?:handle|process_request)\s*\(",
            ],
            vec![
                "Separation of concerns".to_string(),
                "Parallel development".to_string(),
                "Testability".to_string(),
            ],
            vec![
                "Complexity".to_string(),
                "Tight coupling in views".to_string(),
            ],
        );

        self.add_pattern(
            "repository",
            "architectural",
            "Repository pattern for data access",
            vec![r"(?:class|interface|trait)\s+(?:\w*Repository\w*)"],
            vec![r"(?:fn|def|function)\s+(?:find|get|save|delete|update)\s*\("],
            vec![
                "Abstraction of data layer".to_string(),
                "Testability".to_string(),
                "Centralized data access".to_string(),
            ],
            vec![
                "Over-abstraction".to_string(),
                "Query complexity".to_string(),
            ],
        );

        self.add_pattern(
            "service_layer",
            "architectural",
            "Service layer pattern for business logic",
            vec![r"(?:class|interface|trait)\s+(?:\w*Service\w*)"],
            vec![r"(?:fn|def|function)\s+(?:process|handle|execute)\s*\("],
            vec![
                "Business logic separation".to_string(),
                "Reusability".to_string(),
                "Transaction control".to_string(),
            ],
            vec![
                "Service explosion".to_string(),
                "Over-engineering".to_string(),
            ],
        );
    }

    /// Add a pattern definition
    #[allow(clippy::too_many_arguments)]
    fn add_pattern(
        &mut self,
        name: &str,
        pattern_type: &str,
        description: &str,
        class_patterns: Vec<&str>,
        function_patterns: Vec<&str>,
        benefits: Vec<String>,
        considerations: Vec<String>,
    ) {
        let class_regexes: Vec<Regex> = class_patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        let function_regexes: Vec<Regex> = function_patterns
            .into_iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        self.patterns.push(PatternDefinition {
            name: name.to_string(),
            pattern_type: pattern_type.to_string(),
            description: description.to_string(),
            class_patterns: class_regexes,
            function_patterns: function_regexes,
            benefits,
            considerations,
        });
    }

    /// Analyze entities for architectural patterns
    pub fn analyze(&self, entities: &[Entity]) -> PatternReport {
        let mut report = PatternReport::new();

        // Group entities by file
        let mut file_entities: HashMap<String, Vec<&crate::entities::Entity>> = HashMap::new();
        for entity in entities {
            file_entities
                .entry(entity.file_path().to_string())
                .or_default()
                .push(entity);
        }

        // Detect patterns in each file
        for (file_path, file_ents) in &file_entities {
            for pattern_def in &self.patterns {
                if let Some(pattern) =
                    self.detect_pattern_in_file(pattern_def, file_ents, file_path)
                {
                    report.add_pattern(pattern);
                }
            }
        }

        // Detect cross-file patterns
        self.detect_cross_file_patterns(entities, &mut report);

        // Generate recommendations based on detected patterns
        self.generate_recommendations(&mut report);

        // Generate summary
        report.generate_summary();

        report
    }

    /// Detect a pattern in a file
    fn detect_pattern_in_file(
        &self,
        pattern_def: &PatternDefinition,
        entities: &[&crate::entities::Entity],
        file_path: &str,
    ) -> Option<ArchitecturalPattern> {
        let mut matches = 0;
        let mut participating_classes = Vec::new();
        let mut participating_functions = Vec::new();
        let mut evidence = Vec::new();

        for entity in entities {
            let name = entity.name().to_string();
            let body = get_entity_body(entity);

            match entity {
                crate::entities::Entity::Class(_)
                | crate::entities::Entity::Struct(_)
                | crate::entities::Entity::Interface(_)
                | crate::entities::Entity::Trait(_) => {
                    for regex in &pattern_def.class_patterns {
                        if regex.is_match(&name) || regex.is_match(body) {
                            matches += 1;
                            participating_classes.push(name.clone());
                            evidence.push(format!("Class/Struct '{}' matches pattern", name));
                            break;
                        }
                    }
                }
                crate::entities::Entity::Function(_) => {
                    for regex in &pattern_def.function_patterns {
                        if regex.is_match(&name) || regex.is_match(body) {
                            matches += 1;
                            participating_functions.push(name.clone());
                            evidence.push(format!("Function '{}' matches pattern", name));
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        if matches >= 2 {
            let confidence = (matches as f64 / 5.0).min(1.0);

            let mut pattern = ArchitecturalPattern::new(
                &pattern_def.name,
                &pattern_def.pattern_type,
                confidence,
                &pattern_def.description,
            )
            .with_file_path(file_path);

            for class in participating_classes {
                pattern = pattern.with_class(class);
            }
            for func in participating_functions {
                pattern = pattern.with_function(func);
            }
            for ev in evidence {
                pattern = pattern.with_evidence(ev);
            }
            for benefit in &pattern_def.benefits {
                pattern = pattern.with_benefit(benefit.clone());
            }
            for consideration in &pattern_def.considerations {
                pattern = pattern.with_consideration(consideration.clone());
            }

            Some(pattern)
        } else {
            None
        }
    }

    /// Detect patterns that span multiple files
    fn detect_cross_file_patterns(
        &self,
        entities: &[crate::entities::Entity],
        report: &mut PatternReport,
    ) {
        // Detect MVC pattern
        let mut has_models = false;
        let mut has_views = false;
        let mut has_controllers = false;

        for entity in entities {
            let name_lower = entity.name().to_lowercase();
            if name_lower.contains("model") {
                has_models = true;
            }
            if name_lower.contains("view") || name_lower.contains("template") {
                has_views = true;
            }
            if name_lower.contains("controller") || name_lower.contains("handler") {
                has_controllers = true;
            }
        }

        if has_models && has_views && has_controllers {
            let pattern = ArchitecturalPattern::new(
                "mvc",
                "architectural",
                0.8,
                "Model-View-Controller architecture detected",
            )
            .with_evidence("Models, Views, and Controllers found in codebase");

            report.add_pattern(pattern);
        }

        // Detect layered architecture
        let mut layer_count = 0;
        let layers = ["presentation", "business", "service", "data", "persistence"];
        for entity in entities {
            let name_lower = entity.name().to_lowercase();
            for layer in &layers {
                if name_lower.contains(layer) {
                    layer_count += 1;
                    break;
                }
            }
        }

        if layer_count >= 3 {
            let pattern = ArchitecturalPattern::new(
                "layered_architecture",
                "architectural",
                0.7,
                "Layered architecture detected",
            )
            .with_evidence(format!("{} architectural layers found", layer_count));

            report.add_pattern(pattern);
        }
    }

    /// Generate recommendations based on detected patterns
    fn generate_recommendations(&self, report: &mut PatternReport) {
        // Check for common architectural smells
        let creational_count = report.patterns_by_type("creational").len();
        let structural_count = report.patterns_by_type("structural").len();
        let behavioral_count = report.patterns_by_type("behavioral").len();

        if creational_count > 5 {
            report.architectural_smells.push(
                "High number of creational patterns - consider simplifying object creation"
                    .to_string(),
            );
            report
                .recommendations
                .push("Review creational patterns for potential consolidation".to_string());
        }

        if behavioral_count == 0 && !report.detected_patterns.is_empty() {
            report.architectural_smells.push(
                "No behavioral patterns detected - consider improving object communication"
                    .to_string(),
            );
            report.recommendations.push(
                "Consider implementing Observer or Strategy patterns for better decoupling"
                    .to_string(),
            );
        }

        if structural_count == 0 && !report.detected_patterns.is_empty() {
            report
                .architectural_smells
                .push("No structural patterns detected - code may be tightly coupled".to_string());
        }

        // General recommendations
        if report.detected_patterns.len() < 2 {
            report.recommendations.push(
                "Consider implementing design patterns to improve code structure".to_string(),
            );
        }
    }
}

impl Default for ArchitecturalPatternRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_architectural_pattern() {
        let pattern =
            ArchitecturalPattern::new("singleton", "creational", 0.9, "Ensures a single instance")
                .with_class("DatabaseConnection")
                .with_function("get_instance")
                .with_benefit("Memory efficiency")
                .with_consideration("Testing difficulties");

        assert_eq!(pattern.name, "singleton");
        assert_eq!(pattern.pattern_type, "creational");
        assert!((pattern.confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(pattern.participating_classes.len(), 1);
        assert_eq!(pattern.benefits.len(), 1);
    }

    #[test]
    fn test_pattern_report() {
        let mut report = PatternReport::new();

        let pattern = ArchitecturalPattern::new("factory", "creational", 0.8, "Creates objects");
        report.add_pattern(pattern);

        assert_eq!(report.detected_patterns.len(), 1);
        assert_eq!(report.pattern_distribution.get("creational"), Some(&1));
    }

    #[test]
    fn test_pattern_recognizer() {
        let recognizer = ArchitecturalPatternRecognizer::new();
        assert!(!recognizer.patterns.is_empty());
    }
}
