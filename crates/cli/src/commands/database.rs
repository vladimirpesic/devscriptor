//! Database management commands

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use devscriptor_core::database::Database;
use devscriptor_core::utils::validation::PathValidationOptions;

use crate::commands::common::validate_path;

/// Show database status (with optional project path)
pub async fn status(database: Arc<Database>, project_path: Option<PathBuf>) -> Result<()> {
    // The path the database itself was opened at.
    let db_path = PathBuf::from(database.path());
    // Validate database path for dangerous patterns
    let _ = validate_path(&db_path, &PathValidationOptions::default())?;
    if let Some(ref p) = project_path {
        let _ = validate_path(p, &PathValidationOptions::must_be_directory())?;
    }
    let stats = database.get_stats().await?;

    println!("\n{}", "═".repeat(60));
    if project_path.is_some() {
        println!(" Database Statistics");
    } else {
        println!(" Database Status");
    }
    println!("{}", "═".repeat(60));
    if let Some(ref p) = project_path {
        println!("\n📁 Project: {}", p.display());
    }
    println!("📁 Database: {}", db_path.display());

    // get_stats returns (file_count, entity_count, relationship_count)
    let (files, entities, relationships) = stats;

    println!("\n📊 Statistics:");
    println!("  • Total files: {files}");
    println!("  • Total entities: {entities}");
    println!("  • Total relationships: {relationships}");

    Ok(())
}

/// RAII wrapper for a cross-platform advisory file lock.
///
/// Acquires an exclusive lock on creation and releases it on drop.
struct AdvisoryLock {
    file: std::fs::File,
}

impl AdvisoryLock {
    fn acquire(path: &std::path::Path) -> anyhow::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("Failed to open lock file {}", path.display()))?;

        file.lock().with_context(
            || "Failed to acquire advisory lock; another process may be using the database",
        )?;

        Ok(Self { file })
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Clean database atomically and safely.
///
/// # Security
/// - Validates the path before any filesystem access.
/// - Removes the database path from the global connection-pool cache so no
///   new connections can be opened while we are deleting the file.
/// - Acquires a POSIX advisory lock to prevent other Devscriptor processes
///   from opening the database during cleanup.
/// - Uses atomic rename-to-trash instead of direct deletion, eliminating the
///   TOCTOU window between "check exists" and "remove file".
pub async fn clean(database: PathBuf) -> Result<()> {
    // Validate database path for dangerous patterns
    let _ = validate_path(&database, &PathValidationOptions::default())?;

    let db_path_str = database.to_string_lossy().to_string();

    // Remove the path from the global pool cache so no new connections
    // can be opened for this file while we are deleting it.
    devscriptor_core::database::connection::ConnectionPool::remove_from_cache(&db_path_str);

    // Acquire an advisory lock to prevent other processes from opening
    // the database while we clean.  The lock file is a sibling of the DB.
    let lock_file_path = database.with_extension("db.lock");
    let _lock = AdvisoryLock::acquire(&lock_file_path)
        .context("Failed to acquire cleanup lock; another process may be using the database")?;

    // Atomically rename database file(s) to a temporary trash directory
    // before deletion.  This eliminates the TOCTOU race between "exists"
    // and "remove" — any lingering connections see the file vanish.
    //
    // The trash directory must sit on the **same filesystem** as the database:
    // `rename(2)` across devices fails with `EXDEV` ("Invalid cross-device link").
    // In `$TMPDIR` it would break `clean` for every database outside `/tmp`,
    // which is the normal case — the MCP server's default lives under
    // `~/.local/share/devscriptor/`. Placing it beside the database keeps the
    // rename atomic, which is the entire point of the dance.
    let trash_parent = database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let trash_dir = tempfile::tempdir_in(trash_parent).with_context(|| {
        format!(
            "Failed to create temporary directory in {} for atomic cleanup",
            trash_parent.display()
        )
    })?;
    let trash_db = trash_dir.path().join("trash.db");

    if database.exists() {
        tokio::fs::rename(&database, &trash_db)
            .await
            .with_context(|| {
                format!("Failed to move database to trash at {}", trash_db.display())
            })?;
    }

    // Move WAL and SHM files to trash as well.
    // SQLite appends `-wal` and `-shm` suffixes to the database filename,
    // NOT a new extension. For `data.sqlite3`, the WAL file is
    // `data.sqlite3-wal`, NOT `data.db-wal`.
    //
    // A failure here is not fatal — the main database is already gone, so a
    // stray WAL cannot resurrect it — but it must not pass silently.
    let wal_path = PathBuf::from(format!("{db_path_str}-wal"));
    let shm_path = PathBuf::from(format!("{db_path_str}-shm"));
    for (path, name) in [(&wal_path, "trash.db-wal"), (&shm_path, "trash.db-shm")] {
        if path.exists()
            && let Err(e) = tokio::fs::rename(path, trash_dir.path().join(name)).await
        {
            tracing::warn!("Failed to remove {}: {e}", path.display());
        }
    }

    // trash_dir is automatically deleted when it goes out of scope.
    // Release the lock and remove the lock file so it doesn't accumulate.
    // The lock must be released before removal on Windows, where open
    // handles prevent deletion.
    drop(_lock);
    let _ = tokio::fs::remove_file(&lock_file_path).await;

    println!("✅ Database cleared successfully");

    Ok(())
}

/// Output database statistics as JSON, `{"files":N,"entities":N,"relationships":N}`
/// on stdout, for scripts and the VS Code extension (`stats --database X`).
pub async fn execute_stats(
    db_path: PathBuf,
    project: &devscriptor_core::config::ProjectConfig,
) -> Result<()> {
    let db = Database::new(crate::commands::common::database_config(&db_path, project))
        .context("Failed to open database")?;

    let (file_count, entity_count, relationship_count) =
        db.get_stats().await.context("Failed to query stats")?;

    println!(
        "{{\"files\":{file_count},\"entities\":{entity_count},\"relationships\":{relationship_count}}}"
    );
    Ok(())
}
