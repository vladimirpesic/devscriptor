//! CRUD operations for context notes with embedding generation

use std::sync::Arc;

use crate::context_graph::{ContextConfig, ContextNote, NoteStatus};
use crate::database::Database;
use crate::error::Result;

/// Updates that can be applied to a note
#[derive(Debug, Clone, Default)]
pub struct NoteUpdates {
    /// New title
    pub title: Option<String>,
    /// New content
    pub content: Option<String>,
    /// New summary
    pub summary: Option<String>,
    /// New priority
    pub priority: Option<i32>,
    /// New status
    pub status: Option<NoteStatus>,
    /// New effective until date
    pub effective_until: Option<time::OffsetDateTime>,
    /// New tags
    pub tags: Option<Vec<String>>,
    /// New structured data
    pub structured_data: Option<serde_json::Value>,
    /// New metadata
    pub metadata: Option<serde_json::Value>,
}

impl NoteUpdates {
    /// Create empty updates
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: None,
            content: None,
            summary: None,
            priority: None,
            status: None,
            effective_until: None,
            tags: None,
            structured_data: None,
            metadata: None,
        }
    }

    /// Set title
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set content
    #[must_use]
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Set summary
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set priority
    #[must_use]
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set status
    #[must_use]
    pub const fn with_status(mut self, status: NoteStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set tags
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = Some(tags);
        self
    }

    /// Check if any updates are present
    #[must_use]
    pub fn has_updates(&self) -> bool {
        self.title.is_some()
            || self.content.is_some()
            || self.summary.is_some()
            || self.priority.is_some()
            || self.status.is_some()
            || self.effective_until.is_some()
            || self.tags.is_some()
            || self.structured_data.is_some()
            || self.metadata.is_some()
    }
}

/// Operations for managing context notes in the database
#[derive(Clone)]
pub struct ContextOperations {
    /// Database reference
    database: Arc<Database>,
    /// Configuration
    config: ContextConfig,
}

impl ContextOperations {
    /// Create new context operations
    pub fn new(database: Arc<Database>, config: ContextConfig) -> Self {
        Self { database, config }
    }

    /// Get a reference to the underlying database
    #[must_use]
    pub fn database(&self) -> Arc<Database> {
        Arc::clone(&self.database)
    }

    /// Create a new context note
    ///
    /// # Arguments
    /// * `note` - The note to create
    ///
    /// # Returns
    /// The ID of the created note
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn create_note(&self, note: &ContextNote) -> Result<i64> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
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

        let id = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

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

            let id = conn.last_insert_rowid();
            Ok::<i64, crate::error::Error>(id)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(id)
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
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let note = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut stmt = conn.prepare(
                "SELECT id, note_type, title, content, summary, author, priority, status,
                        created_at, updated_at, effective_from, effective_until,
                        tags, structured_data, metadata
                 FROM context_notes WHERE id = ?1",
            )?;

            let note = stmt.query_row([&note_id.to_string()], ContextNote::from_row)?;

            Ok::<ContextNote, crate::error::Error>(note)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(note)
    }

    /// Update a note
    ///
    /// # Arguments
    /// * `note_id` - The note ID to update
    /// * `updates` - The updates to apply
    ///
    /// # Returns
    /// Number of rows affected
    ///
    /// # Errors
    /// Returns an error if the note is not found or database operations fail
    pub async fn update_note(&self, note_id: i64, updates: &NoteUpdates) -> Result<usize> {
        if !updates.has_updates() {
            return Ok(0);
        }

        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let title = updates.title.clone();
        let content = updates.content.clone();
        let summary = updates.summary.clone();
        let priority = updates.priority;
        let status = updates.status.map(|s| s.as_str().to_string());
        let tags = updates
            .tags
            .as_ref()
            .and_then(|t| serde_json::to_string(t).ok());
        let effective_until = updates.effective_until;
        let structured_data = updates
            .structured_data
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let metadata = updates
            .metadata
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        let affected = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut sets = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(t) = title {
                sets.push("title = ?");
                params.push(Box::new(t));
            }
            if let Some(c) = content {
                sets.push("content = ?");
                params.push(Box::new(c));
            }
            if let Some(s) = summary {
                sets.push("summary = ?");
                params.push(Box::new(s));
            }
            if let Some(p) = priority {
                sets.push("priority = ?");
                params.push(Box::new(p));
            }
            if let Some(s) = status {
                sets.push("status = ?");
                params.push(Box::new(s));
            }
            if let Some(t) = tags {
                sets.push("tags = ?");
                params.push(Box::new(t));
            }
            if let Some(dt) = effective_until {
                let dt_str = dt
                    .format(time::macros::format_description!(
                        "[year]-[month]-[day] [hour]:[minute]:[second]"
                    ))
                    .unwrap_or_default();
                sets.push("effective_until = ?");
                params.push(Box::new(dt_str));
            }
            if let Some(sd) = structured_data {
                sets.push("structured_data = ?");
                params.push(Box::new(sd));
            }
            if let Some(md) = metadata {
                sets.push("metadata = ?");
                params.push(Box::new(md));
            }

            if sets.is_empty() {
                return Ok::<usize, crate::error::Error>(0);
            }

            let sql = format!(
                "UPDATE context_notes SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                sets.join(", ")
            );
            params.push(Box::new(note_id));
            let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

            let affected = conn.execute(&sql, param_refs.as_slice())?;
            Ok::<usize, crate::error::Error>(affected)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(affected)
    }

    /// Delete a note
    ///
    /// # Arguments
    /// * `note_id` - The note ID to delete
    ///
    /// # Returns
    /// Number of rows affected
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn delete_note(&self, note_id: i64) -> Result<usize> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let affected = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let affected = conn.execute(
                "DELETE FROM context_notes WHERE id = ?1",
                [&note_id.to_string()],
            )?;

            Ok::<usize, crate::error::Error>(affected)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(affected)
    }

    /// List notes with pagination
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
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let limit_str = limit.to_string();
        let offset_str = offset.to_string();

        let notes = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let mut stmt = conn.prepare(
                "SELECT id, note_type, title, content, summary, author, priority, status,
                        created_at, updated_at, effective_from, effective_until,
                        tags, structured_data, metadata
                 FROM context_notes
                 ORDER BY priority DESC, created_at DESC
                 LIMIT ?1 OFFSET ?2",
            )?;

            let notes = stmt
                .query_map([&limit_str, &offset_str], ContextNote::from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<ContextNote>, crate::error::Error>(notes)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(notes)
    }

    /// Count notes by status
    ///
    /// # Arguments
    /// * `status` - Optional status filter
    ///
    /// # Returns
    /// Count of notes
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn count_notes(&self, status: Option<NoteStatus>) -> Result<i64> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();
        let status_str = status.map(|s| s.as_str().to_string());

        let count = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;

            let (sql, params): (&str, Vec<String>) = if let Some(s) = status_str {
                (
                    "SELECT COUNT(*) FROM context_notes WHERE status = ?1",
                    vec![s],
                )
            } else {
                ("SELECT COUNT(*) FROM context_notes", vec![])
            };

            let count: i64 = if params.is_empty() {
                conn.query_row(sql, [], |row| row.get(0))?
            } else {
                conn.query_row(sql, [&params[0]], |row| row.get(0))?
            };

            Ok::<i64, crate::error::Error>(count)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(count)
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
    /// Compute cutoff date strings for pruning.
    ///
    /// Returns `(archive_cutoff, delete_cutoff)` where delete_cutoff is 2× max_age_days.
    fn compute_prune_cutoffs(max_age_days: u32) -> (String, String) {
        let now = time::OffsetDateTime::now_utc();
        let cutoff = now - time::Duration::days(i64::from(max_age_days));
        let cutoff_str = cutoff
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| cutoff.to_string());
        let delete_cutoff = now - time::Duration::days(i64::from(max_age_days) * 2);
        let delete_cutoff_str = delete_cutoff
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|_| delete_cutoff.to_string());
        (cutoff_str, delete_cutoff_str)
    }

    /// Prune context notes older than `max_age_days`.
    ///
    /// Returns `(archived_count, deleted_count)`.
    pub async fn prune_notes(&self, max_age_days: u32, dry_run: bool) -> Result<(usize, usize)> {
        self.prune_notes_capped(max_age_days, None, dry_run).await
    }

    /// Prune context notes older than `max_age_days`, and additionally cap the
    /// number of retained active notes at `max_notes`.
    ///
    /// When `max_notes` is `Some(n)` and more than `n` active notes remain after
    /// the age-based pass, the lowest-ranked surplus notes are archived. Rank is
    /// the same order used by search and listing: `priority DESC, updated_at DESC`,
    /// so the highest-priority and most recently touched notes are retained.
    ///
    /// Returns `(archived_count, deleted_count)`. Archiving never deletes: a
    /// capped note keeps its history and can still be read by id.
    pub async fn prune_notes_capped(
        &self,
        max_age_days: u32,
        max_notes: Option<usize>,
        dry_run: bool,
    ) -> Result<(usize, usize)> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let result = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let (cutoff_str, delete_cutoff_str) = Self::compute_prune_cutoffs(max_age_days);

            if dry_run {
                // Count notes that would be archived by the age pass.
                let aged: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM context_notes
                     WHERE updated_at < ?1 AND status = 'active'",
                    [&cutoff_str],
                    |row| row.get(0),
                )?;

                // Count notes that would additionally be archived by the cap:
                // active notes that survive the age pass, beyond `max_notes`.
                let capped: i64 = match max_notes {
                    Some(limit) => {
                        let surviving: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM context_notes
                             WHERE status = 'active' AND updated_at >= ?1",
                            [&cutoff_str],
                            |row| row.get(0),
                        )?;
                        (surviving - i64::try_from(limit).unwrap_or(i64::MAX)).max(0)
                    }
                    None => 0,
                };

                Ok::<(usize, usize), crate::error::Error>((
                    usize::try_from(aged + capped).unwrap_or(0),
                    0,
                ))
            } else {
                // Archive old active notes
                let mut archived = conn.execute(
                    "UPDATE context_notes
                     SET status = 'archived', updated_at = datetime('now')
                     WHERE updated_at < ?1 AND status = 'active'",
                    [&cutoff_str],
                )?;

                // Cap the retained active notes, archiving the lowest-ranked surplus.
                if let Some(limit) = max_notes {
                    archived += conn.execute(
                        "UPDATE context_notes
                         SET status = 'archived', updated_at = datetime('now')
                         WHERE id IN (
                             SELECT id FROM context_notes
                             WHERE status = 'active'
                             ORDER BY priority DESC, updated_at DESC
                             LIMIT -1 OFFSET ?1
                         )",
                        [&i64::try_from(limit).unwrap_or(i64::MAX)],
                    )?;
                }

                // Delete very old archived notes (older than 2x max_age)
                let deleted = conn.execute(
                    "DELETE FROM context_notes
                     WHERE updated_at < ?1 AND status = 'archived'",
                    [&delete_cutoff_str],
                )?;

                Ok((archived, deleted))
            }
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(result)
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
    pub async fn get_prune_statistics(&self, max_age_days: u32) -> Result<PruneStatistics> {
        let pool = self.database.conn_pool();
        let pool = pool.clone();

        let stats = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let (cutoff_str, delete_cutoff_str) = Self::compute_prune_cutoffs(max_age_days);

            // Total notes
            let total: i64 =
                conn.query_row("SELECT COUNT(*) FROM context_notes", [], |row| row.get(0))?;

            // Active notes older than max_age_days
            let old_active: i64 = conn.query_row(
                "SELECT COUNT(*) FROM context_notes WHERE updated_at < ?1 AND status = 'active'",
                [&cutoff_str],
                |row| row.get(0),
            )?;

            // Archived notes older than 2x max_age_days (eligible for deletion)
            let old_archived: i64 = conn.query_row(
                "SELECT COUNT(*) FROM context_notes WHERE updated_at < ?1 AND status = 'archived'",
                [&delete_cutoff_str],
                |row| row.get(0),
            )?;

            // Notes by status
            let active: i64 = conn.query_row(
                "SELECT COUNT(*) FROM context_notes WHERE status = 'active'",
                [],
                |row| row.get(0),
            )?;

            let archived: i64 = conn.query_row(
                "SELECT COUNT(*) FROM context_notes WHERE status = 'archived'",
                [],
                |row| row.get(0),
            )?;

            let superseded: i64 = conn.query_row(
                "SELECT COUNT(*) FROM context_notes WHERE status = 'superseded'",
                [],
                |row| row.get(0),
            )?;

            Ok::<PruneStatistics, crate::error::Error>(PruneStatistics {
                total_notes: usize::try_from(total).unwrap_or(0),
                active_notes: usize::try_from(active).unwrap_or(0),
                archived_notes: usize::try_from(archived).unwrap_or(0),
                superseded_notes: usize::try_from(superseded).unwrap_or(0),
                old_active_notes: usize::try_from(old_active).unwrap_or(0),
                old_archived_notes: usize::try_from(old_archived).unwrap_or(0),
                max_age_days,
            })
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        Ok(stats)
    }

    /// Get the configuration
    #[must_use]
    pub const fn config(&self) -> &ContextConfig {
        &self.config
    }
}

/// Statistics for pruning operations
#[derive(Debug, Clone)]
pub struct PruneStatistics {
    /// Total number of notes
    pub total_notes: usize,
    /// Number of active notes
    pub active_notes: usize,
    /// Number of archived notes
    pub archived_notes: usize,
    /// Number of superseded notes
    pub superseded_notes: usize,
    /// Number of old active notes (eligible for archiving)
    pub old_active_notes: usize,
    /// Number of old archived notes (eligible for deletion)
    pub old_archived_notes: usize,
    /// Maximum age in days used for calculation
    pub max_age_days: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_updates() {
        let updates = NoteUpdates::new().with_title("New Title").with_priority(10);

        assert!(updates.has_updates());
        assert_eq!(updates.title, Some("New Title".to_string()));
        assert_eq!(updates.priority, Some(10));
    }
}
