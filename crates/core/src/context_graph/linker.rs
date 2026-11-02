//! Note linking system for connecting context to code entities and abstract concepts

use std::sync::Arc;

use crate::context_graph::{
    ConceptLink, ConceptLinkSpec, ContextConfig, EntityLink, EntityLinkSpec,
};
use crate::database::Database;
use crate::error::Result;

/// Manages links between context notes and code entities/concepts
#[derive(Clone)]
pub struct NoteLinker {
    /// Database reference
    database: Arc<Database>,
    /// Configuration
    config: ContextConfig,
}

impl NoteLinker {
    /// Create a new note linker
    pub fn new(database: Arc<Database>, config: ContextConfig) -> Self {
        Self { database, config }
    }

    /// Link a note to a code entity
    ///
    /// # Arguments
    /// * `note_id` - The ID of the context note
    /// * `spec` - The entity link specification
    ///
    /// # Returns
    /// The ID of the created link
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn link_to_entity(&self, note_id: i64, spec: &EntityLinkSpec) -> Result<i64> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let entity_type = spec.entity_type.clone();
        let entity_id = spec.entity_id.clone().unwrap_or_default();
        let entity_name = spec.entity_name.clone().unwrap_or_default();
        let file_path = spec.file_path.clone().unwrap_or_default();
        let link_type = spec.link_type.clone();

        let id = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            conn.execute(
                "INSERT INTO context_entity_links (context_note_id, entity_type, entity_id, entity_name, file_path, link_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                [
                    &note_id.to_string(),
                    &entity_type,
                    &entity_id,
                    &entity_name,
                    &file_path,
                    &link_type,
                ],
            )?;

            let id = conn.last_insert_rowid();
            Ok::<i64, crate::error::Error>(id)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(id)
    }

    /// Link a note to an abstract concept
    ///
    /// # Arguments
    /// * `note_id` - The ID of the context note
    /// * `spec` - The concept link specification
    ///
    /// # Returns
    /// The ID of the created link
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn link_to_concept(&self, note_id: i64, spec: &ConceptLinkSpec) -> Result<i64> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let concept_type = spec.concept_type.clone();
        let concept_name = spec.concept_name.clone();
        let concept_path = spec.concept_path.clone().unwrap_or_default();
        let link_type = spec.link_type.clone();

        let id = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            conn.execute(
                "INSERT INTO context_concept_links (context_note_id, concept_type, concept_name, concept_path, link_type)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                [
                    &note_id.to_string(),
                    &concept_type,
                    &concept_name,
                    &concept_path,
                    &link_type,
                ],
            )?;

            let id = conn.last_insert_rowid();
            Ok::<i64, crate::error::Error>(id)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(id)
    }

    /// Get all entity links for a note
    ///
    /// # Arguments
    /// * `note_id` - The ID of the note
    ///
    /// # Returns
    /// List of entity links
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_linked_entities(&self, note_id: i64) -> Result<Vec<EntityLink>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let links = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id, context_note_id, entity_type, entity_id, entity_name, file_path, link_type, created_at
                 FROM context_entity_links WHERE context_note_id = ?1",
            )?;

            let links = stmt
                .query_map([&note_id.to_string()], |row| {
                    Ok(EntityLink {
                        id: row.get(0)?,
                        context_note_id: row.get(1)?,
                        entity_type: row.get(2)?,
                        entity_id: row.get(3)?,
                        entity_name: row.get(4)?,
                        file_path: row.get(5)?,
                        link_type: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<EntityLink>, crate::error::Error>(links)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(links)
    }

    /// Get all concept links for a note
    ///
    /// # Arguments
    /// * `note_id` - The ID of the note
    ///
    /// # Returns
    /// List of concept links
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_linked_concepts(&self, note_id: i64) -> Result<Vec<ConceptLink>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let links = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut stmt = conn.prepare(
                "SELECT id, context_note_id, concept_type, concept_name, concept_path, link_type, created_at
                 FROM context_concept_links WHERE context_note_id = ?1",
            )?;

            let links = stmt
                .query_map([&note_id.to_string()], |row| {
                    Ok(ConceptLink {
                        id: row.get(0)?,
                        context_note_id: row.get(1)?,
                        concept_type: row.get(2)?,
                        concept_name: row.get(3)?,
                        concept_path: row.get(4)?,
                        link_type: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ConceptLink>, crate::error::Error>(links)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(links)
    }

    /// Get IDs of notes linked to a specific entity
    ///
    /// # Arguments
    /// * `entity_type` - Type of entity
    /// * `_entity_id` - Database ID of the entity
    /// * `_entity_name` - Name of the entity
    /// * `_file_path` - File path containing the entity
    ///
    /// # Returns
    /// List of note IDs
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_linked_notes(
        &self,
        entity_type: &str,
        entity_id: Option<&str>,
        entity_name: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<i64>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let entity_type = entity_type.to_string();
        let entity_id = entity_id.map(String::from);
        let entity_name = entity_name.map(String::from);
        let file_path = file_path.map(String::from);

        let note_ids = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sql =
                "SELECT DISTINCT context_note_id FROM context_entity_links WHERE entity_type = ?1"
                    .to_string();
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&entity_type];
            let mut next_param = 2;

            if let Some(ref id) = entity_id {
                sql.push_str(&format!(" AND entity_id = ?{next_param}"));
                next_param += 1;
                params.push(id);
            }
            if let Some(ref name) = entity_name {
                sql.push_str(&format!(" AND entity_name = ?{next_param}"));
                next_param += 1;
                params.push(name);
            }
            if let Some(ref path) = file_path {
                sql.push_str(&format!(" AND file_path = ?{next_param}"));
                params.push(path);
            }

            let note_ids: Vec<i64> = conn
                .prepare(&sql)?
                .query_map(params.as_slice(), |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<i64>, crate::error::Error>(note_ids)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(note_ids)
    }

    /// Get IDs of notes linked to a specific concept
    ///
    /// # Arguments
    /// * `concept_name` - Name of the concept
    /// * `concept_type` - Optional type filter
    ///
    /// # Returns
    /// List of note IDs
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn get_notes_by_concept(
        &self,
        concept_name: &str,
        concept_type: Option<&str>,
    ) -> Result<Vec<i64>> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let concept_name = concept_name.to_string();
        let concept_type = concept_type.map(String::from);

        let note_ids = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let sql = if concept_type.is_some() {
                "SELECT DISTINCT context_note_id FROM context_concept_links WHERE concept_name = ?1 AND concept_type = ?2"
            } else {
                "SELECT DISTINCT context_note_id FROM context_concept_links WHERE concept_name = ?1"
            };

            let note_ids: Vec<i64> = if let Some(ct) = concept_type {
                conn.prepare(sql)?
                    .query_map([&concept_name, &ct], |row| row.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            } else {
                conn.prepare(sql)?
                    .query_map([&concept_name], |row| row.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };

            Ok::<Vec<i64>, crate::error::Error>(note_ids)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(note_ids)
    }

    /// Delete all links for a note
    ///
    /// # Arguments
    /// * `note_id` - The ID of the note
    ///
    /// # Returns
    /// Number of links deleted
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn delete_all_links(&self, note_id: i64) -> Result<usize> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let count = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let entity_count = conn.execute(
                "DELETE FROM context_entity_links WHERE context_note_id = ?1",
                [&note_id.to_string()],
            )?;

            let concept_count = conn.execute(
                "DELETE FROM context_concept_links WHERE context_note_id = ?1",
                [&note_id.to_string()],
            )?;

            Ok::<usize, crate::error::Error>(entity_count + concept_count)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(count)
    }

    /// Delete a specific entity link
    ///
    /// # Arguments
    /// * `link_id` - The ID of the link to delete
    ///
    /// # Returns
    /// Number of rows affected
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn delete_entity_link(&self, link_id: i64) -> Result<usize> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let count = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let count = conn.execute(
                "DELETE FROM context_entity_links WHERE id = ?1",
                [&link_id.to_string()],
            )?;

            Ok::<usize, crate::error::Error>(count)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(count)
    }

    /// Delete a specific concept link
    ///
    /// # Arguments
    /// * `link_id` - The ID of the link to delete
    ///
    /// # Returns
    /// Number of rows affected
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn delete_concept_link(&self, link_id: i64) -> Result<usize> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let count = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let count = conn.execute(
                "DELETE FROM context_concept_links WHERE id = ?1",
                [&link_id.to_string()],
            )?;

            Ok::<usize, crate::error::Error>(count)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(count)
    }

    /// Get the configuration
    #[must_use]
    pub const fn config(&self) -> &ContextConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_link_spec() {
        let spec = EntityLinkSpec::new("function")
            .with_entity_name("test_func")
            .with_entity_id("123")
            .with_file_path("/test.py");

        assert_eq!(spec.entity_type, "function");
        assert_eq!(spec.entity_name, Some("test_func".to_string()));
        assert_eq!(spec.entity_id, Some("123".to_string()));
    }

    #[test]
    fn test_concept_link_spec() {
        let spec = ConceptLinkSpec::new("module", "auth").with_concept_path("src/auth");

        assert_eq!(spec.concept_type, "module");
        assert_eq!(spec.concept_name, "auth");
        assert_eq!(spec.concept_path, Some("src/auth".to_string()));
    }
}
