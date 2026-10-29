//! Context Graph entity definitions for Devscriptor
//!
//! Provides structs for context notes, links, and related entities
//! for the AI agent long-term memory system.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Types of context notes with specific structured fields
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum NoteType {
    /// Architecture Decision Record
    Adr,
    /// Coding standard or convention
    Standard,
    /// Performance optimization record
    Optimization,
    /// Failure or mistake record
    Failure,
    /// Best practice documentation
    Practice,
    /// Reflection or retrospective
    Reflection,
    /// General knowledge note
    #[default]
    General,
    /// File description
    FileDescription,
    /// Code modification record
    Modification,
}

impl NoteType {
    /// Convert to string representation
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Adr => "adr",
            Self::Standard => "standard",
            Self::Optimization => "optimization",
            Self::Failure => "failure",
            Self::Practice => "practice",
            Self::Reflection => "reflection",
            Self::General => "general",
            Self::FileDescription => "file_description",
            Self::Modification => "modification",
        }
    }

    /// Parse from string
    ///
    /// # Errors
    /// Returns an error if the string is not a valid note type
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> crate::error::Result<Self> {
        match s {
            "adr" => Ok(Self::Adr),
            "standard" => Ok(Self::Standard),
            "optimization" => Ok(Self::Optimization),
            "failure" => Ok(Self::Failure),
            "practice" => Ok(Self::Practice),
            "reflection" => Ok(Self::Reflection),
            "general" => Ok(Self::General),
            "file_description" => Ok(Self::FileDescription),
            "modification" => Ok(Self::Modification),
            _ => Err(crate::error::Error::Analysis(format!(
                "Invalid note type: {s}"
            ))),
        }
    }
}

/// Status of a context note
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum NoteStatus {
    /// Note is active and searchable
    #[default]
    Active,
    /// Note has been superseded by another
    Superseded,
    /// Note is archived
    Archived,
}

impl NoteStatus {
    /// Convert to string representation
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
        }
    }

    /// Parse from string
    ///
    /// # Errors
    /// Returns an error if the string is not a valid note status
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> crate::error::Result<Self> {
        match s {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "archived" => Ok(Self::Archived),
            _ => Err(crate::error::Error::Analysis(format!(
                "Invalid note status: {s}"
            ))),
        }
    }
}

/// A context note representing project knowledge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextNote {
    /// Database ID (None for new notes)
    pub id: Option<i64>,
    /// Type of note (ADR, standard, etc.)
    pub note_type: NoteType,
    /// Brief descriptive title
    pub title: String,
    /// Full note content
    pub content: String,
    /// Optional one-line summary
    pub summary: Option<String>,
    /// Creator of note ('agent', 'user', 'system')
    pub author: String,
    /// 1-10, affects search ranking
    pub priority: i32,
    /// Status: 'active', 'superseded', or 'archived'
    pub status: NoteStatus,
    /// Creation timestamp
    pub created_at: Option<OffsetDateTime>,
    /// Last update timestamp
    pub updated_at: Option<OffsetDateTime>,
    /// When knowledge became applicable
    pub effective_from: Option<OffsetDateTime>,
    /// When knowledge expires (optional)
    pub effective_until: Option<OffsetDateTime>,
    /// List of categorization tags
    pub tags: Vec<String>,
    /// Type-specific fields as JSON
    pub structured_data: serde_json::Value,
    /// Extensibility JSON
    pub metadata: serde_json::Value,
}

impl ContextNote {
    /// Create a new context note
    #[must_use]
    pub fn new(note_type: NoteType, title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: None,
            note_type,
            title: title.into(),
            content: content.into(),
            summary: None,
            author: "agent".to_string(),
            priority: 5,
            status: NoteStatus::Active,
            created_at: None,
            updated_at: None,
            effective_from: None,
            effective_until: None,
            tags: Vec::new(),
            structured_data: serde_json::Value::Object(serde_json::Map::new()),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Set the author
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }

    /// Set the priority
    #[must_use]
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set tags
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Set summary
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set structured data
    #[must_use]
    pub fn with_structured_data(mut self, data: serde_json::Value) -> Self {
        self.structured_data = data;
        self
    }

    /// Validate note structure
    ///
    /// # Errors
    /// Returns an error if validation fails
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.title.is_empty() || self.content.is_empty() {
            return Err(crate::error::Error::Analysis(
                "Title and content are required".to_string(),
            ));
        }
        if !(1..=10).contains(&self.priority) {
            return Err(crate::error::Error::Analysis(
                "Priority must be 1-10".to_string(),
            ));
        }
        Ok(())
    }

    /// Deserialize a [`ContextNote`] from a `rusqlite` row using named columns:
    /// the one reader [`ContextOperations`] and [`ContextSearch`] share.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let note_type_str: String = row.get("note_type")?;
        let tags_str: Option<String> = row.get("tags")?;
        let structured_str: Option<String> = row.get("structured_data")?;
        let metadata_str: Option<String> = row.get("metadata")?;

        Ok(Self {
            id: row.get("id")?,
            note_type: NoteType::from_str(&note_type_str).unwrap_or(NoteType::General),
            title: row.get("title")?,
            content: row.get("content")?,
            summary: row.get("summary")?,
            author: row.get("author")?,
            priority: row.get("priority")?,
            status: NoteStatus::from_str(&row.get::<_, String>("status")?)
                .unwrap_or(NoteStatus::Active),
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            effective_from: row.get("effective_from")?,
            effective_until: row.get("effective_until")?,
            tags: tags_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| {
                    tracing::warn!("Failed to deserialize tags JSON");
                    Vec::new()
                }),
            structured_data: structured_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            metadata: metadata_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
        })
    }
}

/// Link between context note and concrete code entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityLink {
    /// Database ID
    pub id: Option<i64>,
    /// ID of linked context note
    pub context_note_id: i64,
    /// Type of entity ('function', 'class', etc.)
    pub entity_type: String,
    /// Database ID in respective entity table
    pub entity_id: Option<String>,
    /// Name for rename resilience
    pub entity_name: Option<String>,
    /// File path for rename resilience
    pub file_path: Option<String>,
    /// Relationship type
    pub link_type: String,
    /// Creation timestamp
    pub created_at: Option<OffsetDateTime>,
}

impl EntityLink {
    /// Create a new entity link
    #[must_use]
    pub fn new(
        context_note_id: i64,
        entity_type: impl Into<String>,
        entity_name: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            context_note_id,
            entity_type: entity_type.into(),
            entity_id: None,
            entity_name: Some(entity_name.into()),
            file_path: None,
            link_type: "references".to_string(),
            created_at: None,
        }
    }

    /// Set entity ID
    #[must_use]
    pub fn with_entity_id(mut self, entity_id: impl Into<String>) -> Self {
        self.entity_id = Some(entity_id.into());
        self
    }

    /// Set file path
    #[must_use]
    pub fn with_file_path(mut self, file_path: impl Into<String>) -> Self {
        self.file_path = Some(file_path.into());
        self
    }

    /// Set link type
    #[must_use]
    pub fn with_link_type(mut self, link_type: impl Into<String>) -> Self {
        self.link_type = link_type.into();
        self
    }
}

/// Link between context note and abstract concept
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptLink {
    /// Database ID
    pub id: Option<i64>,
    /// ID of linked context note
    pub context_note_id: i64,
    /// Type of concept ('module', 'concept', 'pattern')
    pub concept_type: String,
    /// Name of the concept
    pub concept_name: String,
    /// Optional path reference
    pub concept_path: Option<String>,
    /// Relationship type
    pub link_type: String,
    /// Creation timestamp
    pub created_at: Option<OffsetDateTime>,
}

impl ConceptLink {
    /// Create a new concept link
    #[must_use]
    pub fn new(
        context_note_id: i64,
        concept_type: impl Into<String>,
        concept_name: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            context_note_id,
            concept_type: concept_type.into(),
            concept_name: concept_name.into(),
            concept_path: None,
            link_type: "references".to_string(),
            created_at: None,
        }
    }

    /// Set concept path
    #[must_use]
    pub fn with_concept_path(mut self, path: impl Into<String>) -> Self {
        self.concept_path = Some(path.into());
        self
    }

    /// Set link type
    #[must_use]
    pub fn with_link_type(mut self, link_type: impl Into<String>) -> Self {
        self.link_type = link_type.into();
        self
    }
}

/// Specification for creating an entity link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityLinkSpec {
    /// Type of entity
    pub entity_type: String,
    /// Optional entity ID
    pub entity_id: Option<String>,
    /// Optional entity name
    pub entity_name: Option<String>,
    /// Optional file path
    pub file_path: Option<String>,
    /// Link type
    pub link_type: String,
}

impl EntityLinkSpec {
    /// Create a new entity link specification
    #[must_use]
    pub fn new(entity_type: impl Into<String>) -> Self {
        Self {
            entity_type: entity_type.into(),
            entity_id: None,
            entity_name: None,
            file_path: None,
            link_type: "references".to_string(),
        }
    }

    /// Set entity ID
    #[must_use]
    pub fn with_entity_id(mut self, entity_id: impl Into<String>) -> Self {
        self.entity_id = Some(entity_id.into());
        self
    }

    /// Set entity name
    #[must_use]
    pub fn with_entity_name(mut self, entity_name: impl Into<String>) -> Self {
        self.entity_name = Some(entity_name.into());
        self
    }

    /// Set file path
    #[must_use]
    pub fn with_file_path(mut self, file_path: impl Into<String>) -> Self {
        self.file_path = Some(file_path.into());
        self
    }

    /// Set link type
    #[must_use]
    pub fn with_link_type(mut self, link_type: impl Into<String>) -> Self {
        self.link_type = link_type.into();
        self
    }
}

/// Specification for creating a concept link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptLinkSpec {
    /// Type of concept
    pub concept_type: String,
    /// Concept name
    pub concept_name: String,
    /// Optional concept path
    pub concept_path: Option<String>,
    /// Link type
    pub link_type: String,
}

impl ConceptLinkSpec {
    /// Create a new concept link specification
    #[must_use]
    pub fn new(concept_type: impl Into<String>, concept_name: impl Into<String>) -> Self {
        Self {
            concept_type: concept_type.into(),
            concept_name: concept_name.into(),
            concept_path: None,
            link_type: "references".to_string(),
        }
    }

    /// Set concept path
    #[must_use]
    pub fn with_concept_path(mut self, path: impl Into<String>) -> Self {
        self.concept_path = Some(path.into());
        self
    }

    /// Set link type
    #[must_use]
    pub fn with_link_type(mut self, link_type: impl Into<String>) -> Self {
        self.link_type = link_type.into();
        self
    }
}

/// Search result from context graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSearchResult {
    /// The matching note
    pub note: ContextNote,
    /// Relevance in [0, 1], higher is better: the cosine similarity for
    /// semantic search, the best match's share for keyword and hybrid search.
    pub score: f64,
    /// Search result metadata
    pub metadata: serde_json::Value,
    /// `true` when the search used a fallback method (keyword/FTS5 instead of
    /// true vector embeddings). Results from fallback search have no semantic
    /// meaning — similar concepts will **not** produce similar scores.
    pub fallback: bool,
}

/// Filters for context searches
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Filter by note type
    pub note_type: Option<NoteType>,
    /// Filter by tags (all must match)
    pub tags: Vec<String>,
    /// Filter by status
    pub status: Option<NoteStatus>,
    /// Minimum priority
    pub min_priority: Option<i32>,
    /// Maximum priority
    pub max_priority: Option<i32>,
}

impl SearchFilters {
    /// Create new empty filters
    #[must_use]
    pub const fn new() -> Self {
        Self {
            note_type: None,
            tags: Vec::new(),
            status: None,
            min_priority: None,
            max_priority: None,
        }
    }

    /// Set note type filter
    #[must_use]
    pub const fn with_note_type(mut self, note_type: NoteType) -> Self {
        self.note_type = Some(note_type);
        self
    }

    /// Set tags filter
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Set status filter
    #[must_use]
    pub const fn with_status(mut self, status: NoteStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set priority range
    #[must_use]
    pub const fn with_priority_range(mut self, min: i32, max: i32) -> Self {
        self.min_priority = Some(min);
        self.max_priority = Some(max);
        self
    }
}

/// Type of search to perform
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchType {
    /// Full-text keyword search
    Keyword,
    /// Vector similarity search
    Semantic,
    /// Hybrid combination
    #[default]
    Hybrid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_type_from_str() {
        assert_eq!(NoteType::from_str("adr").unwrap(), NoteType::Adr);
        assert_eq!(NoteType::from_str("standard").unwrap(), NoteType::Standard);
        assert_eq!(NoteType::from_str("general").unwrap(), NoteType::General);
        assert!(NoteType::from_str("invalid").is_err());
    }

    #[test]
    fn test_note_status_from_str() {
        assert_eq!(NoteStatus::from_str("active").unwrap(), NoteStatus::Active);
        assert_eq!(
            NoteStatus::from_str("superseded").unwrap(),
            NoteStatus::Superseded
        );
        assert!(NoteStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_context_note_validation() {
        let note = ContextNote::new(NoteType::General, "Test", "Content");
        assert!(note.validate().is_ok());

        let invalid = ContextNote::new(NoteType::General, "", "Content");
        assert!(invalid.validate().is_err());

        let invalid_priority =
            ContextNote::new(NoteType::General, "Test", "Content").with_priority(15);
        assert!(invalid_priority.validate().is_err());
    }

    #[test]
    fn test_entity_link_builder() {
        let link = EntityLink::new(1, "function", "test_func")
            .with_entity_id("uuid-123")
            .with_file_path("/test/file.rs")
            .with_link_type("implements");

        assert_eq!(link.context_note_id, 1);
        assert_eq!(link.entity_type, "function");
        assert_eq!(link.entity_name, Some("test_func".to_string()));
        assert_eq!(link.entity_id, Some("uuid-123".to_string()));
        assert_eq!(link.file_path, Some("/test/file.rs".to_string()));
        assert_eq!(link.link_type, "implements");
    }

    #[test]
    fn test_concept_link_builder() {
        let link = ConceptLink::new(1, "module", "auth")
            .with_concept_path("src/auth")
            .with_link_type("depends_on");

        assert_eq!(link.context_note_id, 1);
        assert_eq!(link.concept_type, "module");
        assert_eq!(link.concept_name, "auth");
        assert_eq!(link.concept_path, Some("src/auth".to_string()));
        assert_eq!(link.link_type, "depends_on");
    }

    #[test]
    fn test_search_filters_builder() {
        let filters = SearchFilters::new()
            .with_note_type(NoteType::Adr)
            .with_tags(vec!["security".to_string()])
            .with_status(NoteStatus::Active)
            .with_priority_range(3, 8);

        assert_eq!(filters.note_type, Some(NoteType::Adr));
        assert_eq!(filters.tags, vec!["security"]);
        assert_eq!(filters.status, Some(NoteStatus::Active));
        assert_eq!(filters.min_priority, Some(3));
        assert_eq!(filters.max_priority, Some(8));
    }
}
