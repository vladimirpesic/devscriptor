//! Code Clone Detector for Devscriptor
//!
//! This module provides functionality to detect duplicated code blocks across files
//! using AST-based analysis and similarity scoring.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Keywords and placeholders that must NOT be replaced by `$IDENT` during
/// code normalization.  Preserving these keeps the structural skeleton
/// intact so that Type-3 clones are detected without destroying syntax.
static STRUCTURAL_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "if",
        "else",
        "for",
        "while",
        "do",
        "switch",
        "case",
        "break",
        "continue",
        "return",
        "def",
        "fn",
        "function",
        "class",
        "struct",
        "impl",
        "trait",
        "pub",
        "let",
        "const",
        "var",
        "import",
        "use",
        "from",
        "in",
        "match",
        "with",
        "try",
        "catch",
        "except",
        "finally",
        "raise",
        "throw",
        "async",
        "await",
        "yield",
        "new",
        "delete",
        "typeof",
        "void",
        "goto",
        "static",
        "mut",
        "ref",
        "enum",
        "union",
        "type",
        "namespace",
        "module",
        "interface",
        "extends",
        "implements",
        "abstract",
        "final",
        "override",
        "virtual",
        "inline",
        "extern",
        "default",
        "assert",
        "sizeof",
        "and",
        "or",
        "not",
        "is",
        "as",
        "where",
        "loop",
        "unsafe",
        "dyn",
        "box",
        "crate",
        "super",
        "self",
        "this",
        "true",
        "false",
        "null",
        "none",
        "nil",
        "undefined",
        "public",
        "private",
        "protected",
        "internal",
        "partial",
        "sealed",
        "readonly",
        "volatile",
        "transient",
        "native",
        "strictfp",
        "synchronized",
        "package",
        "include",
        "define",
        "ifdef",
        "endif",
        "pragma",
        "using",
        // Normalized placeholders (must not be re-replaced)
        "STR",
        "CHAR",
        "NUM",
    ]
    .iter()
    .copied()
    .collect()
});

static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[a-zA-Z_][a-zA-Z0-9_]*\b").unwrap());

/// Normalize a single token for clone-similarity comparison.
///
/// Converts camelCase / PascalCase boundaries into snake_case so that
/// identifiers such as `fooBar` and `foo_bar` share the same tokens.
/// Structural keywords are left untouched.
fn normalize_token(token: &str) -> String {
    // Preserve structural keywords and existing placeholders.
    if STRUCTURAL_KEYWORDS.contains(token) || token.starts_with('$') {
        return token.to_lowercase();
    }

    // If it's not an identifier-like token, lowercase it.
    if token.is_empty() || !token.starts_with(|c: char| c.is_alphabetic() || c == '_') {
        return token.to_lowercase();
    }

    // Convert camelCase boundaries to underscores.
    let mut result = String::with_capacity(token.len() + 4);
    let chars: Vec<char> = token.chars().collect();
    for i in 0..chars.len() {
        let ch = chars[i];
        if ch.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            // Insert underscore when transitioning from lower→upper
            // or from upper→lower after a run of uppers (e.g., HTTPRequest).
            if prev.is_lowercase() || (i + 1 < chars.len() && chars[i + 1].is_lowercase()) {
                result.push('_');
            }
        }
        result.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    result
}
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::analysis::api::AnalysisSummary;
use crate::analysis::{AnalysisGuardConfig, Finding, Severity};
use crate::database::Database;
use crate::error::Result;

/// Type of code clone
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneType {
    /// Type 1: Identical code (except whitespace/comments)
    Type1,
    /// Type 2: Syntactically identical (renamed identifiers)
    Type2,
    /// Type 3: Modified/gapped clones
    Type3,
}

impl CloneType {
    /// Get description of clone type
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Type1 => "Identical code",
            Self::Type2 => "Syntactically identical with renamed identifiers",
            Self::Type3 => "Modified/gapped code",
        }
    }
}

/// Represents a pair of code clones with similarity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClonePair {
    /// First file path
    pub file1_path: String,
    /// Second file path
    pub file2_path: String,
    /// First entity name
    pub entity1_name: String,
    /// Second entity name
    pub entity2_name: String,
    /// First entity type
    pub entity1_type: String,
    /// Second entity type
    pub entity2_type: String,
    /// Start line in first file
    pub start_line1: u32,
    /// End line in first file
    pub end_line1: u32,
    /// Start line in second file
    pub start_line2: u32,
    /// End line in second file
    pub end_line2: u32,
    /// Similarity score (0.0-1.0)
    pub similarity_score: f64,
    /// Type of clone
    pub clone_type: CloneType,
    /// Code fragment from first file
    pub code_fragment1: String,
    /// Code fragment from second file
    pub code_fragment2: String,
}

impl ClonePair {
    /// Create a new clone pair
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        file1_path: impl Into<String>,
        file2_path: impl Into<String>,
        entity1_name: impl Into<String>,
        entity2_name: impl Into<String>,
        similarity_score: f64,
        clone_type: CloneType,
    ) -> Self {
        Self {
            file1_path: file1_path.into(),
            file2_path: file2_path.into(),
            entity1_name: entity1_name.into(),
            entity2_name: entity2_name.into(),
            entity1_type: "function".to_string(),
            entity2_type: "function".to_string(),
            start_line1: 0,
            end_line1: 0,
            start_line2: 0,
            end_line2: 0,
            similarity_score,
            clone_type,
            code_fragment1: String::new(),
            code_fragment2: String::new(),
        }
    }

    /// Get total lines of cloned code
    #[must_use]
    pub fn total_lines(&self) -> u32 {
        let lines1 = self.end_line1.saturating_sub(self.start_line1) + 1;
        let lines2 = self.end_line2.saturating_sub(self.start_line2) + 1;
        lines1 + lines2
    }

    /// Check if this is a self-clone (same file)
    #[must_use]
    pub fn is_self_clone(&self) -> bool {
        self.file1_path == self.file2_path
    }
}

/// Represents a group of related code clones
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneGroup {
    /// Clone pairs in this group
    pub clone_pairs: Vec<ClonePair>,
    /// Size of the group
    pub group_size: usize,
    /// Average similarity
    pub average_similarity: f64,
    /// Unique file paths involved
    pub file_paths: Vec<String>,
}

impl CloneGroup {
    /// Create a new clone group from pairs
    #[must_use]
    pub fn from_pairs(pairs: Vec<ClonePair>) -> Self {
        let group_size = pairs.len();
        let average_similarity = if pairs.is_empty() {
            0.0
        } else {
            pairs.iter().map(|p| p.similarity_score).sum::<f64>() / pairs.len() as f64
        };

        let mut file_paths: HashSet<String> = HashSet::new();
        for pair in &pairs {
            file_paths.insert(pair.file1_path.clone());
            file_paths.insert(pair.file2_path.clone());
        }

        Self {
            clone_pairs: pairs,
            group_size,
            average_similarity,
            file_paths: file_paths.into_iter().collect(),
        }
    }

    /// Get total lines of code in this clone group
    #[must_use]
    pub fn total_lines(&self) -> u32 {
        self.clone_pairs.iter().map(ClonePair::total_lines).sum()
    }
}

/// Comprehensive clone detection analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneDetectionReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// Clone pairs found
    pub clones: Vec<ClonePair>,
    /// Clone groups
    pub clone_groups: Vec<CloneGroup>,
    /// Clone type distribution
    pub clone_type_distribution: HashMap<String, usize>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

impl CloneDetectionReport {
    /// Create a new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            clones: Vec::new(),
            clone_groups: Vec::new(),
            clone_type_distribution: HashMap::new(),
            recommendations: Vec::new(),
        }
    }

    /// Add a clone pair
    pub fn add_clone(&mut self, clone: ClonePair) {
        let clone_type_key = format!("{:?}", clone.clone_type).to_lowercase();
        *self
            .clone_type_distribution
            .entry(clone_type_key)
            .or_insert(0) += 1;
        self.clones.push(clone);
        self.summary.add_finding(Severity::Medium);
    }

    /// Generate recommendations based on findings
    pub fn generate_recommendations(&mut self) {
        let mut recs = Vec::new();

        let type1_count = self
            .clone_type_distribution
            .get("type1")
            .copied()
            .unwrap_or(0);
        if type1_count > 0 {
            recs.push(format!(
                "Found {} identical code clones (Type 1). Consider extracting common code into shared functions.",
                type1_count
            ));
        }

        let type2_count = self
            .clone_type_distribution
            .get("type2")
            .copied()
            .unwrap_or(0);
        if type2_count > 0 {
            recs.push(format!(
                "Found {} syntactically identical clones with different names (Type 2). Consider unifying naming conventions.",
                type2_count
            ));
        }

        let type3_count = self
            .clone_type_distribution
            .get("type3")
            .copied()
            .unwrap_or(0);
        if type3_count > 0 {
            recs.push(format!(
                "Found {} modified/gapped clones (Type 3). Review for potential abstraction opportunities.",
                type3_count
            ));
        }

        if recs.is_empty() {
            recs.push("No code clones detected - your codebase is DRY!".to_string());
        }

        self.recommendations = recs;
    }

    /// Convert clones to findings
    #[must_use]
    pub fn to_findings(&self) -> Vec<Finding> {
        self.clones
            .iter()
            .map(|clone| {
                Finding::new(
                    Severity::Medium,
                    format!(
                        "Code clone detected: {}% similarity with {}",
                        (clone.similarity_score * 100.0) as u32,
                        clone.entity2_name
                    ),
                    &clone.file1_path,
                    clone.start_line1,
                    &clone.entity1_name,
                    &clone.entity1_type,
                )
                .with_context(format!(
                    "Clone in {} at line {}",
                    clone.file2_path, clone.start_line2
                ))
                .with_suggestion(
                    "Consider extracting common code into a shared function".to_string(),
                )
            })
            .collect()
    }
}

impl Default for CloneDetectionReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Code fragment with metadata for comparison
#[derive(Debug, Clone)]
pub(crate) struct CodeFragment {
    entity_type: String,
    name: String,
    file_path: String,
    start_line: u32,
    end_line: u32,
    code: String,
    normalized_code: String,
    hash: String,
}

/// Detector for code clones using similarity scoring
#[derive(Clone)]
pub struct CloneDetector {
    /// Minimum lines for a code block to be considered
    min_lines: u32,
    /// Minimum similarity score to consider as clone
    similarity_threshold: f64,
    /// Normalization patterns
    normalization_patterns: Vec<(Regex, &'static str)>,
    /// Input-size guard configuration
    guard_config: AnalysisGuardConfig,
}

impl CloneDetector {
    /// Create a new clone detector with default settings
    #[must_use]
    pub fn new() -> Self {
        // Comments must be stripped BEFORE collapsing whitespace. If whitespace
        // is collapsed first, `//.*$` matches to end-of-string (not end-of-line)
        // and erases everything after the first comment on any line.
        let patterns = vec![
            (Regex::new(r"(?m)//[^\n]*").unwrap(), ""), // single-line comments (per-line)
            (Regex::new(r"/\*[\s\S]*?\*/").unwrap(), ""), // multi-line block comments
            (Regex::new(r"(?m)#[^\n]*").unwrap(), ""),  // preprocessor / Python comments
            (Regex::new(r#"".*?""#).unwrap(), "STR"),   // string literals
            (Regex::new(r"'\.'").unwrap(), "CHAR"),     // char literals
            (Regex::new(r"\d+").unwrap(), "NUM"),       // numbers
            (Regex::new(r"\s+").unwrap(), " "),         // collapse whitespace last
        ];

        Self {
            min_lines: 5,
            similarity_threshold: 0.8,
            normalization_patterns: patterns,
            guard_config: AnalysisGuardConfig::default(),
        }
    }

    /// Create detector with custom settings
    #[must_use]
    pub fn with_settings(min_lines: u32, similarity_threshold: f64) -> Self {
        let mut detector = Self::new();
        detector.min_lines = min_lines;
        detector.similarity_threshold = similarity_threshold;
        detector
    }

    /// Set the guard configuration for this detector.
    #[must_use]
    pub fn with_guard_config(mut self, config: AnalysisGuardConfig) -> Self {
        self.guard_config = config;
        self
    }

    /// Analyze the codebase for code clones
    ///
    /// # Returns
    /// Clone detection analysis report
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn analyze(
        &self,
        database: std::sync::Arc<Database>,
    ) -> Result<CloneDetectionReport> {
        let start_time = std::time::Instant::now();
        let mut report = CloneDetectionReport::new();

        // Extract code fragments
        let fragments = self.extract_fragments(database).await?;

        // Find similar fragments, chunking when the fragment count would make
        // the O(n²) comparison too expensive.  Each chunk is compared within
        // itself; cross-chunk clones may be missed but the result is never
        // empty when a large repo has real clones within any single chunk.
        let max = self.guard_config.max_fragments_for_clone_detection;
        if fragments.len() > max {
            warn!(
                "Clone detection: {} fragments exceed chunk size {}; comparing in chunks.",
                fragments.len(),
                max
            );
            report.recommendations.push(format!(
                "Clone detection processed {} fragments in chunks of {} (cross-chunk clones may be missed).",
                fragments.len(),
                max
            ));
        }
        let clones = self.find_similar_fragments_chunked(&fragments, max);

        for clone in clones {
            report.add_clone(clone);
        }

        // Group related clones
        let groups = self.group_clones(&report.clones);
        report.clone_groups = groups;

        // Generate recommendations
        report.generate_recommendations();

        // Update summary
        report.summary.duration_ms = start_time.elapsed().as_millis() as u64;
        report.summary.entities_analyzed = fragments.len();

        Ok(report)
    }

    /// Extract code fragments with metadata for comparison
    async fn extract_fragments(
        &self,
        database: std::sync::Arc<Database>,
    ) -> Result<Vec<CodeFragment>> {
        let pool = database.conn_pool().clone();

        #[allow(clippy::type_complexity)]
        let entities: Vec<(String, String, String, String, u32, u32, String, String)> =
            tokio::task::spawn_blocking(move || {
                let conn = pool.get()?;
                let mut stmt = conn.prepare(
                    "SELECT id, type, name, file_path, start_line, end_line, language, body
                     FROM entities WHERE type IN ('function', 'class', 'method')",
                )?;

                let results = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, u32>(4)?,
                            row.get::<_, u32>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok::<Vec<_>, crate::error::Error>(results)
            })
            .await
            .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        let mut fragments = Vec::new();
        for (_id, entity_type, name, file_path, start_line, end_line, _language, body) in entities {
            let lines = end_line.saturating_sub(start_line) + 1;

            if lines >= self.min_lines {
                let normalized = self.normalize_code(&body);
                let hash = self.compute_hash(&normalized);

                fragments.push(CodeFragment {
                    entity_type,
                    name,
                    file_path,
                    start_line,
                    end_line,
                    code: body,
                    normalized_code: normalized,
                    hash,
                });
            }
        }

        Ok(fragments)
    }

    /// Normalize code for comparison.
    ///
    /// Applies configurable regex patterns (whitespace, comments, literals,
    /// numbers) and collapses remaining whitespace.  Identifiers are replaced
    /// with the placeholder `$IDENT` (structural keywords are preserved), which
    /// enables detection of Type-3 clones where only variable or function
    /// names differ.
    fn normalize_code(&self, code: &str) -> String {
        let mut normalized = code.to_string();

        // Apply normalization patterns
        for (pattern, replacement) in &self.normalization_patterns {
            normalized = pattern.replace_all(&normalized, *replacement).to_string();
        }

        // Replace identifiers with placeholder for Type-3 clone detection
        normalized = IDENT_RE
            .replace_all(&normalized, |caps: &Captures| {
                let matched = caps.get(0).unwrap().as_str();
                if STRUCTURAL_KEYWORDS.contains(matched) {
                    matched.to_string()
                } else {
                    "$IDENT".to_string()
                }
            })
            .to_string();

        // Remove extra whitespace
        normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");

        normalized
    }

    /// Compute hash of normalized code.
    ///
    /// Uses xxHash3-64 for fast, stable exact-match (Type-1) clone detection.
    /// xxHash3 is sufficient here because we only need to group identical
    /// fragments; cryptographic guarantees are unnecessary.
    fn compute_hash(&self, code: &str) -> String {
        use xxhash_rust::xxh3::xxh3_64;

        let hash = xxh3_64(code.as_bytes());
        format!("{:016x}", hash)
    }

    /// Find similar fragments
    fn find_similar_fragments(&self, fragments: &[CodeFragment]) -> Vec<ClonePair> {
        let mut clones = Vec::new();

        // Compare each pair of fragments
        for (i, frag1) in fragments.iter().enumerate() {
            for frag2 in fragments.iter().skip(i + 1) {
                // Skip if same file and overlapping
                if frag1.file_path == frag2.file_path {
                    let overlap =
                        frag1.start_line <= frag2.end_line && frag2.start_line <= frag1.end_line;
                    if overlap {
                        continue;
                    }
                }

                // Quick check: if hashes match exactly, it's a Type 1 clone
                if frag1.hash == frag2.hash {
                    clones.push(ClonePair {
                        file1_path: frag1.file_path.clone(),
                        file2_path: frag2.file_path.clone(),
                        entity1_name: frag1.name.clone(),
                        entity2_name: frag2.name.clone(),
                        entity1_type: frag1.entity_type.clone(),
                        entity2_type: frag2.entity_type.clone(),
                        start_line1: frag1.start_line,
                        end_line1: frag1.end_line,
                        start_line2: frag2.start_line,
                        end_line2: frag2.end_line,
                        similarity_score: 1.0,
                        clone_type: CloneType::Type1,
                        code_fragment1: frag1.code.clone(),
                        code_fragment2: frag2.code.clone(),
                    });
                    continue;
                }

                // Calculate similarity
                let similarity =
                    self.calculate_similarity(&frag1.normalized_code, &frag2.normalized_code);

                if similarity >= self.similarity_threshold {
                    let clone_type = if similarity > 0.95 {
                        CloneType::Type2
                    } else {
                        CloneType::Type3
                    };

                    clones.push(ClonePair {
                        file1_path: frag1.file_path.clone(),
                        file2_path: frag2.file_path.clone(),
                        entity1_name: frag1.name.clone(),
                        entity2_name: frag2.name.clone(),
                        entity1_type: frag1.entity_type.clone(),
                        entity2_type: frag2.entity_type.clone(),
                        start_line1: frag1.start_line,
                        end_line1: frag1.end_line,
                        start_line2: frag2.start_line,
                        end_line2: frag2.end_line,
                        similarity_score: similarity,
                        clone_type,
                        code_fragment1: frag1.code.clone(),
                        code_fragment2: frag2.code.clone(),
                    });
                }
            }
        }

        clones
    }

    /// Compare fragments in chunks of `chunk_size` to bound O(n²) cost.
    /// If all fragments fit in one chunk (len ≤ chunk_size), this is
    /// identical to `find_similar_fragments`. Cross-chunk clones may be missed.
    pub(crate) fn find_similar_fragments_chunked(
        &self,
        fragments: &[CodeFragment],
        chunk_size: usize,
    ) -> Vec<ClonePair> {
        if chunk_size == 0 || fragments.len() <= chunk_size {
            return self.find_similar_fragments(fragments);
        }
        let mut all_clones = Vec::new();
        for chunk in fragments.chunks(chunk_size) {
            all_clones.extend(self.find_similar_fragments(chunk));
        }
        all_clones
    }

    /// Calculate similarity between two code strings.
    ///
    /// Uses token-based Jaccard similarity on whitespace-split tokens.
    /// This is fast and works well for near-duplicate (Type-2) clones but
    /// may under-report similarity for refactored or reordered (Type-3/4)
    /// clones. An AST-based or tree-edit distance metric would be more
    /// accurate for those cases.
    fn calculate_similarity(&self, s1: &str, s2: &str) -> f64 {
        if s1.is_empty() && s2.is_empty() {
            return 1.0;
        }
        if s1.is_empty() || s2.is_empty() {
            return 0.0;
        }

        // Use a token-based similarity with identifier normalization
        // so that camelCase and snake_case variants match (e.g. fooBar ↔ foo_bar).
        let tokens1: Vec<String> = s1.split_whitespace().map(normalize_token).collect();
        let tokens2: Vec<String> = s2.split_whitespace().map(normalize_token).collect();

        // Calculate Jaccard similarity
        let set1: HashSet<&str> = tokens1.iter().map(|s| s.as_str()).collect();
        let set2: HashSet<&str> = tokens2.iter().map(|s| s.as_str()).collect();

        let intersection: HashSet<&str> = set1.intersection(&set2).copied().collect();
        let union: HashSet<&str> = set1.union(&set2).copied().collect();

        if union.is_empty() {
            return 0.0;
        }

        intersection.len() as f64 / union.len() as f64
    }

    /// Group related clones into clusters
    fn group_clones(&self, clones: &[ClonePair]) -> Vec<CloneGroup> {
        let mut groups: Vec<Vec<ClonePair>> = Vec::new();
        let mut entity_to_group: HashMap<String, usize> = HashMap::new();

        for clone in clones {
            let key1 = format!(
                "{}::{}::{}::{}::{}::{}::{}::{}::{}::{}::{:?}",
                clone.file1_path,
                clone.entity1_name,
                clone.entity1_type,
                clone.file2_path,
                clone.entity2_name,
                clone.entity2_type,
                clone.start_line1,
                clone.end_line1,
                clone.start_line2,
                clone.end_line2,
                clone.clone_type
            );
            let key2 = format!(
                "{}::{}::{}::{}::{}::{}::{}::{}::{}::{}::{:?}",
                clone.file2_path,
                clone.entity2_name,
                clone.entity2_type,
                clone.file1_path,
                clone.entity1_name,
                clone.entity1_type,
                clone.start_line2,
                clone.end_line2,
                clone.start_line1,
                clone.end_line1,
                clone.clone_type
            );

            let group1 = entity_to_group.get(&key1).copied();
            let group2 = entity_to_group.get(&key2).copied();

            match (group1, group2) {
                (Some(g1), Some(g2)) if g1 == g2 => {
                    groups[g1].push(clone.clone());
                }
                (Some(g1), None) => {
                    groups[g1].push(clone.clone());
                    entity_to_group.insert(key2, g1);
                }
                (None, Some(g2)) => {
                    groups[g2].push(clone.clone());
                    entity_to_group.insert(key1, g2);
                }
                (None, None) => {
                    let idx = groups.len();
                    groups.push(vec![clone.clone()]);
                    entity_to_group.insert(key1, idx);
                    entity_to_group.insert(key2, idx);
                }
                (Some(_), Some(_)) => {
                    // Different groups - merge them (simplified: just add to first)
                    if let Some(&g1) = entity_to_group.get(&key1) {
                        groups[g1].push(clone.clone());
                    }
                }
            }
        }

        groups.into_iter().map(CloneGroup::from_pairs).collect()
    }

    /// Get minimum lines setting
    #[must_use]
    pub const fn min_lines(&self) -> u32 {
        self.min_lines
    }

    /// Get similarity threshold
    #[must_use]
    pub const fn similarity_threshold(&self) -> f64 {
        self.similarity_threshold
    }
}

impl Default for CloneDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clone_pair_creation() {
        let pair = ClonePair::new(
            "/file1.rs",
            "/file2.rs",
            "func1",
            "func2",
            0.95,
            CloneType::Type1,
        );

        assert_eq!(pair.similarity_score, 0.95);
        assert_eq!(pair.clone_type, CloneType::Type1);
        assert!(!pair.is_self_clone());
    }

    #[test]
    fn test_clone_group() {
        let pairs = vec![
            ClonePair::new("/a.rs", "/b.rs", "f1", "f2", 0.9, CloneType::Type1),
            ClonePair::new("/a.rs", "/c.rs", "f1", "f3", 0.85, CloneType::Type2),
        ];

        let group = CloneGroup::from_pairs(pairs);
        assert_eq!(group.group_size, 2);
        assert_eq!(group.file_paths.len(), 3);
    }

    #[test]
    fn test_clone_detection_report() {
        let mut report = CloneDetectionReport::new();

        report.add_clone(ClonePair::new(
            "/a.rs",
            "/b.rs",
            "f1",
            "f2",
            1.0,
            CloneType::Type1,
        ));

        assert_eq!(report.clones.len(), 1);
        assert_eq!(report.clone_type_distribution.get("type1"), Some(&1));

        report.generate_recommendations();
        assert!(!report.recommendations.is_empty());
    }

    #[test]
    fn test_normalize_code() {
        let detector = CloneDetector::new();
        let code = r#"function test() { // comment
            return 42; /* another comment */
        }"#;

        let normalized = detector.normalize_code(code);
        assert!(!normalized.contains("//"));
        assert!(!normalized.contains("/*"));
        assert!(normalized.contains("function")); // keyword preserved
        assert!(!normalized.contains("test")); // identifier replaced
        assert!(normalized.contains("$IDENT")); // placeholder present
    }

    #[test]
    fn test_normalize_code_type3_clones() {
        let detector = CloneDetector::new();
        let code1 = "function foo() { return x + 1; }";
        let code2 = "function bar() { return y + 2; }";

        let norm1 = detector.normalize_code(code1);
        let norm2 = detector.normalize_code(code2);

        // Both should normalize to identical structure (Type-3 clone detection)
        assert_eq!(norm1, norm2);
    }

    #[test]
    fn test_calculate_similarity() {
        let detector = CloneDetector::new();

        // Identical strings
        assert!((detector.calculate_similarity("hello world", "hello world") - 1.0).abs() < 0.01);

        // Completely different
        assert!(detector.calculate_similarity("abc", "xyz") < 0.5);

        // Empty strings (identical, so similarity = 1.0)
        assert!((detector.calculate_similarity("", "") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_clone_type_description() {
        assert_eq!(CloneType::Type1.description(), "Identical code");
        assert_eq!(
            CloneType::Type2.description(),
            "Syntactically identical with renamed identifiers"
        );
    }

    /// A single-line comment at the end of a line takes nothing on the lines
    /// after it with it, as `//.*$` applied once the newlines are collapsed
    /// would.
    #[test]
    fn test_comment_stripping_does_not_eat_subsequent_lines() {
        let detector = CloneDetector::new();
        let code = "let x = 1; // this is a comment\nlet y = 2;\nreturn x + y;";
        let normalized = detector.normalize_code(code);
        // `y` and `return` must survive — only the comment text is removed.
        assert!(
            normalized.contains("return"),
            "code after the comment line must survive normalization; got: {normalized:?}"
        );
    }

    /// An input of more than 2,000 fragments returns results rather than none.
    #[test]
    fn test_chunked_processing_returns_results_above_limit() {
        use crate::analysis::AnalysisGuardConfig;

        let detector = CloneDetector::new().with_guard_config(AnalysisGuardConfig {
            max_fragments_for_clone_detection: 3,
            ..AnalysisGuardConfig::default()
        });

        // Build 7 fragments (> chunk size 3) where two pairs are identical clones.
        let make_frag = |name: &str, code: &str, line: u32| CodeFragment {
            entity_type: "function".to_string(),
            name: name.to_string(),
            file_path: format!("file_{name}.rs"),
            start_line: line,
            end_line: line + 5,
            code: code.to_string(),
            normalized_code: detector.normalize_code(code),
            hash: detector.compute_hash(&detector.normalize_code(code)),
        };

        let body = "fn foo() { let x = 1; let y = 2; return x + y; }";
        let fragments: Vec<CodeFragment> = vec![
            make_frag("a", body, 1),
            make_frag("b", "fn bar() { let p = 3; }", 10),
            make_frag("c", body, 20), // clone of a
            make_frag("d", "fn baz() { let q = 4; }", 30),
            make_frag("e", body, 40), // clone of a and c
            make_frag("f", "fn qux() { let r = 5; }", 50),
            make_frag("g", "fn quux() { let s = 6; }", 60),
        ];

        // Run the chunked path (fragments.len()=7 > limit=3)
        let clones = detector.find_similar_fragments_chunked(&fragments, 3);
        // At least one clone pair must be found somewhere in the chunks.
        assert!(
            !clones.is_empty(),
            "chunked processing of {}-fragment input must find clones within chunks",
            fragments.len()
        );
    }
}
