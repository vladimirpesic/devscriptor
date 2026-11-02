//! Main orchestrator for Context Graph operations

use std::sync::Arc;

use crate::context_graph::{
    ConceptLinkSpec, ContextConfig, ContextNote, ContextSearchResult, EntityLinkSpec,
    linker::NoteLinker,
    operations::{ContextOperations, NoteUpdates},
    search::ContextSearch,
};
use crate::database::Database;
use crate::embeddings::EmbeddingService;
use crate::error::Result;

/// Main orchestrator for Context Graph operations
#[derive(Clone)]
pub struct ContextGraphManager {
    /// Configuration
    config: ContextConfig,
    /// CRUD operations
    operations: ContextOperations,
    /// Search operations
    search: ContextSearch,
    /// Link management
    linker: NoteLinker,
}

impl ContextGraphManager {
    /// Get a reference to the underlying database
    #[must_use]
    pub fn database(&self) -> Arc<Database> {
        self.operations.database()
    }

    /// Create a new context graph manager without an embedding service.
    ///
    /// Semantic search will degrade to FTS5 keyword search.
    /// Use [`Self::with_embedding_service`] to enable vector-based ranking.
    pub fn new(database: Arc<Database>, config: ContextConfig) -> Self {
        let operations = ContextOperations::new(database.clone(), config.clone());
        let search = ContextSearch::new(database.clone(), config.clone());
        let linker = NoteLinker::new(database.clone(), config.clone());

        Self {
            config,
            operations,
            search,
            linker,
        }
    }

    /// Create a new context graph manager sharing an existing embedding service.
    ///
    /// The service is shared (backed by `Arc`) so no second model load occurs.
    pub fn with_embedding_service(
        database: Arc<Database>,
        config: ContextConfig,
        embedding_service: EmbeddingService,
    ) -> Self {
        let operations = ContextOperations::new(database.clone(), config.clone());
        let search = ContextSearch::with_embedding_service(
            database.clone(),
            config.clone(),
            embedding_service,
        );
        let linker = NoteLinker::new(database.clone(), config.clone());

        Self {
            config,
            operations,
            search,
            linker,
        }
    }

    /// Add a new context note with optional links — **transactional**.
    ///
    /// All inserts (note + entity links + concept links) run inside a single
    /// SQLite transaction. If any step fails, the whole operation is rolled
    /// back so the database never ends up with a note that has missing links.
    ///
    /// # Arguments
    /// * `note` - The context note to add
    /// * `related_entities` - List of entity link specifications
    /// * `related_concepts` - List of concept link specifications
    ///
    /// # Returns
    /// Dictionary with note_id and created note info
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn add_note(
        &self,
        mut note: ContextNote,
        related_entities: Option<Vec<EntityLinkSpec>>,
        related_concepts: Option<Vec<ConceptLinkSpec>>,
    ) -> Result<NoteCreationResult> {
        let pool = self.operations.database().conn_pool().clone();

        // Serialize JSON fields before moving into the blocking task.
        let note_type = note.note_type.as_str().to_string();
        let title = note.title.clone();
        let content = note.content.clone();
        let summary = note.summary.clone();
        let author = note.author.clone();
        let priority = note.priority;
        let status = note.status.as_str().to_string();
        let tags = serde_json::to_string(&note.tags)?;
        let structured = serde_json::to_string(&note.structured_data)?;
        let metadata = serde_json::to_string(&note.metadata)?;
        let related_entities = related_entities.unwrap_or_default();
        let related_concepts = related_concepts.unwrap_or_default();

        let (note_id, entity_links, concept_links) =
            tokio::task::spawn_blocking(move || {
                let conn = pool.get()?;

                // BEGIN TRANSACTION
                conn.execute("BEGIN", [])?;

                let result: std::result::Result<(i64, Vec<i64>, Vec<i64>), crate::error::Error> = (|| {
                    // Insert the note
                    conn.execute(
                        "INSERT INTO context_notes (note_type, title, content, summary, author, priority, status, tags, structured_data, metadata)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        [
                            &note_type,
                            &title,
                            &content,
                            &summary.unwrap_or_default(),
                            &author,
                            &priority.to_string(),
                            &status,
                            &tags,
                            &structured,
                            &metadata,
                        ],
                    )?;

                    let note_id = conn.last_insert_rowid();

                    // Insert entity links
                    let mut entity_links = Vec::new();
                    for spec in related_entities {
                        conn.execute(
                            "INSERT INTO context_entity_links (context_note_id, entity_type, entity_id, entity_name, file_path, link_type)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            [
                                &note_id.to_string(),
                                &spec.entity_type,
                                &spec.entity_id.unwrap_or_default(),
                                &spec.entity_name.unwrap_or_default(),
                                &spec.file_path.unwrap_or_default(),
                                &spec.link_type,
                            ],
                        )?;
                        entity_links.push(conn.last_insert_rowid());
                    }

                    // Insert concept links
                    let mut concept_links = Vec::new();
                    for spec in related_concepts {
                        conn.execute(
                            "INSERT INTO context_concept_links (context_note_id, concept_type, concept_name, concept_path, link_type)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            [
                                &note_id.to_string(),
                                &spec.concept_type,
                                &spec.concept_name,
                                &spec.concept_path.unwrap_or_default(),
                                &spec.link_type,
                            ],
                        )?;
                        concept_links.push(conn.last_insert_rowid());
                    }

                    Ok((note_id, entity_links, concept_links))
                })();

                // Commit or roll back based on result.
                match result {
                    Ok(vals) => {
                        conn.execute("COMMIT", [])?;
                        Ok(vals)
                    }
                    Err(e) => {
                        if let Err(rollback_err) = conn.execute("ROLLBACK", []) {
                            return Err(crate::error::Error::Database(rollback_err));
                        }
                        Err(e)
                    }
                }
            })
            .await
            .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        note.id = Some(note_id);

        Ok(NoteCreationResult {
            note_id,
            status: "created".to_string(),
            title: note.title,
            note_type: note.note_type.as_str().to_string(),
            entity_links,
            concept_links,
        })
    }

    /// Update an existing note
    ///
    /// # Arguments
    /// * `note_id` - ID of the note to update
    /// * `updates` - The updates to apply
    ///
    /// # Returns
    /// Result of the update operation
    ///
    /// # Errors
    /// Returns an error if the note is not found or database operations fail
    pub async fn update_note(
        &self,
        note_id: i64,
        updates: &NoteUpdates,
    ) -> Result<NoteUpdateResult> {
        let affected = self.operations.update_note(note_id, updates).await?;

        Ok(NoteUpdateResult {
            note_id,
            status: if affected > 0 {
                "updated".to_string()
            } else {
                "no_changes".to_string()
            },
            fields_updated: affected,
        })
    }

    /// Delete a note
    ///
    /// # Arguments
    /// * `note_id` - ID of the note to delete
    ///
    /// # Returns
    /// Result of the deletion
    ///
    /// # Errors
    /// Returns an error if the note is not found or database operations fail
    pub async fn delete_note(&self, note_id: i64) -> Result<NoteDeletionResult> {
        // Delete all links first
        let links_deleted = self.linker.delete_all_links(note_id).await?;

        // Delete the note
        let note_deleted = self.operations.delete_note(note_id).await?;

        Ok(NoteDeletionResult {
            note_id,
            status: if note_deleted > 0 {
                "deleted"
            } else {
                "not_found"
            }
            .to_string(),
            links_deleted,
            note_deleted: note_deleted > 0,
        })
    }

    /// Get a note by ID
    ///
    /// # Arguments
    /// * `note_id` - The note ID
    ///
    /// # Returns
    /// The note if found
    ///
    /// # Errors
    /// Returns an error if the note is not found or database operations fail
    pub async fn get_note(&self, note_id: i64) -> Result<ContextNote> {
        self.operations.get_note(note_id).await
    }

    /// Search for notes using the configured search type
    ///
    /// # Arguments
    /// * `query` - The search query
    /// * `top_k` - Number of results to return
    /// * `filters` - Optional filters
    ///
    /// # Returns
    /// List of matching notes with scores
    ///
    /// # Errors
    /// Returns an error if search operations fail
    pub async fn search_notes(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<ContextSearchResult>> {
        // Use keyword search by default
        use crate::context_graph::SearchFilters;
        let filters = SearchFilters::new();
        self.search.keyword_search(query, top_k, &filters).await
    }

    /// Search notes using an explicit search strategy.
    ///
    /// Dispatches to keyword (FTS/LIKE), semantic (vector), or hybrid (RRF
    /// fusion of both) search. Semantic and hybrid require an embedding service
    /// (see [`Self::with_embedding_service`]); without one they degrade
    /// gracefully to keyword search.
    ///
    /// # Errors
    /// Returns an error if the underlying search operation fails.
    pub async fn search_notes_with_type(
        &self,
        query: &str,
        top_k: usize,
        search_type: crate::context_graph::SearchType,
        filters: &crate::context_graph::SearchFilters,
    ) -> Result<Vec<ContextSearchResult>> {
        use crate::context_graph::SearchType;
        match search_type {
            SearchType::Keyword => self.search.keyword_search(query, top_k, filters).await,
            SearchType::Semantic => self.search.semantic_search(query, top_k, filters).await,
            SearchType::Hybrid => self.search.hybrid_search(query, filters, top_k).await,
        }
    }

    /// Get context related to a specific entity
    ///
    /// # Arguments
    /// * `entity_type` - Type of entity ('function', 'class', etc.)
    ///
    /// # Returns
    /// List of related context notes
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_context_for_entity(
        &self,
        entity_type: &str,
    ) -> Result<Vec<ContextSearchResult>> {
        self.search.search_by_entity(entity_type).await
    }

    /// Get context related to an entity, narrowed by the linked entity's name
    /// and/or file path.
    ///
    /// # Arguments
    /// * `entity_type` - Type of entity ('function', 'class', etc.)
    /// * `entity_name` - Optional exact entity name
    /// * `file_path` - Optional file-path fragment (substring match)
    ///
    /// # Returns
    /// List of related context notes
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_context_for_entity_filtered(
        &self,
        entity_type: &str,
        entity_name: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<ContextSearchResult>> {
        self.search
            .search_by_entity_filtered(entity_type, entity_name, file_path)
            .await
    }

    /// Get context related to a concept
    ///
    /// # Arguments
    /// * `concept_name` - Name of the concept
    /// * `concept_type` - Optional concept type
    ///
    /// # Returns
    /// List of related context notes
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_context_for_concept(
        &self,
        concept_name: &str,
    ) -> Result<Vec<ContextSearchResult>> {
        self.search.search_by_concept(concept_name).await
    }

    /// List all notes with pagination
    ///
    /// # Arguments
    /// * `limit` - Maximum number of results
    /// * `offset` - Offset for pagination
    ///
    /// # Returns
    /// List of notes
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn list_notes(&self, limit: usize, offset: usize) -> Result<Vec<ContextNote>> {
        self.operations.list_notes(limit, offset).await
    }

    /// Prune old notes based on age and status
    ///
    /// # Arguments
    /// * `max_age_days` - Maximum age in days before archiving
    /// * `dry_run` - If true, only preview what would be archived
    ///
    /// # Returns
    /// Tuple of (archived_count, deleted_count) or preview count if dry_run
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn prune_notes(&self, max_age_days: u32, dry_run: bool) -> Result<(usize, usize)> {
        self.operations.prune_notes(max_age_days, dry_run).await
    }

    /// Prune notes by age and additionally cap the number of retained active
    /// notes at `max_notes` (lowest priority / least recently updated first).
    ///
    /// Returns `(archived_count, deleted_count)`.
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn prune_notes_capped(
        &self,
        max_age_days: u32,
        max_notes: Option<usize>,
        dry_run: bool,
    ) -> Result<(usize, usize)> {
        self.operations
            .prune_notes_capped(max_age_days, max_notes, dry_run)
            .await
    }

    /// Get pruning statistics
    ///
    /// # Arguments
    /// * `max_age_days` - Maximum age in days for "old" notes
    ///
    /// # Returns
    /// Statistics about notes eligible for pruning
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_prune_statistics(
        &self,
        max_age_days: u32,
    ) -> Result<super::operations::PruneStatistics> {
        self.operations.get_prune_statistics(max_age_days).await
    }

    /// Get the configuration
    #[must_use]
    pub const fn config(&self) -> &ContextConfig {
        &self.config
    }

    /// Get the operations handler
    #[must_use]
    pub const fn operations(&self) -> &ContextOperations {
        &self.operations
    }

    /// Get the search handler
    #[must_use]
    pub const fn search(&self) -> &ContextSearch {
        &self.search
    }

    /// Get the linker handler
    #[must_use]
    pub const fn linker(&self) -> &NoteLinker {
        &self.linker
    }
}

/// Result of creating a note
#[derive(Debug, Clone)]
pub struct NoteCreationResult {
    /// ID of the created note
    pub note_id: i64,
    /// Status of the operation
    pub status: String,
    /// Title of the note
    pub title: String,
    /// Type of the note
    pub note_type: String,
    /// IDs of created entity links
    pub entity_links: Vec<i64>,
    /// IDs of created concept links
    pub concept_links: Vec<i64>,
}

/// Result of updating a note
#[derive(Debug, Clone)]
pub struct NoteUpdateResult {
    /// ID of the updated note
    pub note_id: i64,
    /// Status of the operation
    pub status: String,
    /// Number of fields updated
    pub fields_updated: usize,
}

/// Result of deleting a note
#[derive(Debug, Clone)]
pub struct NoteDeletionResult {
    /// ID of the deleted note
    pub note_id: i64,
    /// Status of the operation
    pub status: String,
    /// Number of links deleted
    pub links_deleted: usize,
    /// Whether the note was deleted
    pub note_deleted: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_context_graph_manager_creation() {
        // This test just verifies the types compile correctly
        // Real tests would need a database connection
    }
}
