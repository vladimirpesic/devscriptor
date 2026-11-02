//! Context Graph module for AI agent long-term memory
//!
//! Provides functionality for managing context notes, linking them to code entities,
//! and searching through project knowledge.

mod entities;
mod linker;
mod manager;
mod operations;
mod search;

pub use entities::{
    ConceptLink, ConceptLinkSpec, ContextNote, ContextSearchResult, EntityLink, EntityLinkSpec,
    NoteStatus, NoteType, SearchFilters, SearchType,
};
pub use linker::NoteLinker;
pub use manager::ContextGraphManager;
pub use operations::{ContextOperations, PruneStatistics};
pub use search::ContextSearch;

use crate::error::Result;

/// Configuration for context graph operations
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Default priority for new notes (1-10)
    pub default_priority: i32,
    /// Maximum number of search results to return
    pub max_search_results: usize,
    /// Whether to enable vector embeddings for semantic search
    pub enable_embeddings: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            default_priority: 5,
            max_search_results: 50,
            enable_embeddings: true,
        }
    }
}

/// Create the Context Graph's tables on `conn`, unless they exist: the notes,
/// their FTS5 index and its sync triggers, and the entity and concept links.
/// `Database::initialize` calls it on every open.
///
/// # Errors
/// Returns an error if a statement fails.
pub fn initialize_context_tables_on_conn(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS context_notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            note_type TEXT NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            summary TEXT,
            author TEXT DEFAULT 'agent',
            priority INTEGER DEFAULT 5,
            status TEXT DEFAULT 'active',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            effective_from TIMESTAMP,
            effective_until TIMESTAMP,
            tags TEXT,
            structured_data TEXT,
            metadata TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_context_notes_type ON context_notes(note_type);
        CREATE INDEX IF NOT EXISTS idx_context_notes_status ON context_notes(status);
        CREATE INDEX IF NOT EXISTS idx_context_notes_priority ON context_notes(priority);

        CREATE VIRTUAL TABLE IF NOT EXISTS context_notes_fts USING fts5(
            title, content, summary,
            content='context_notes',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS context_notes_fts_insert
        AFTER INSERT ON context_notes
        BEGIN
            INSERT INTO context_notes_fts(rowid, title, content, summary)
            VALUES (new.id, new.title, new.content, new.summary);
        END;

        CREATE TRIGGER IF NOT EXISTS context_notes_fts_delete
        AFTER DELETE ON context_notes
        BEGIN
            INSERT INTO context_notes_fts(context_notes_fts, rowid, title, content, summary)
            VALUES('delete', old.id, old.title, old.content, old.summary);
        END;

        CREATE TRIGGER IF NOT EXISTS context_notes_fts_update
        AFTER UPDATE ON context_notes
        BEGIN
            INSERT INTO context_notes_fts(context_notes_fts, rowid, title, content, summary)
            VALUES('delete', old.id, old.title, old.content, old.summary);
            INSERT INTO context_notes_fts(rowid, title, content, summary)
            VALUES (new.id, new.title, new.content, new.summary);
        END;

        CREATE TABLE IF NOT EXISTS context_entity_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            context_note_id INTEGER NOT NULL,
            entity_type TEXT NOT NULL,
            entity_id TEXT,
            entity_name TEXT,
            file_path TEXT,
            link_type TEXT DEFAULT 'references',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (context_note_id) REFERENCES context_notes(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_entity_links_note ON context_entity_links(context_note_id);
        CREATE INDEX IF NOT EXISTS idx_entity_links_entity ON context_entity_links(entity_type, entity_id);
        CREATE INDEX IF NOT EXISTS idx_entity_links_name ON context_entity_links(entity_name);

        CREATE TABLE IF NOT EXISTS context_concept_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            context_note_id INTEGER NOT NULL,
            concept_type TEXT NOT NULL,
            concept_name TEXT NOT NULL,
            concept_path TEXT,
            link_type TEXT DEFAULT 'references',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (context_note_id) REFERENCES context_notes(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_concept_links_note ON context_concept_links(context_note_id);
        CREATE INDEX IF NOT EXISTS idx_concept_links_name ON context_concept_links(concept_name);
        "#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_config_default() {
        let config = ContextConfig::default();
        assert_eq!(config.default_priority, 5);
        assert_eq!(config.max_search_results, 50);
        assert!(config.enable_embeddings);
    }
}
