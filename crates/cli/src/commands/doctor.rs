//! `devscriptor doctor` — Health check command
//!
//! Validates that the environment is ready for scanning:
//! - Embedding model availability
//! - The project's configuration file
//! - Database integrity
//! - System resources (RAM / disk)
//! - Tree-sitter grammar presence

use anyhow::Result;
use devscriptor_core::config::ProjectConfig;
use devscriptor_core::database::Database;
use std::path::Path;

use crate::commands::common::database_config;

/// Health check result for a single component
#[derive(Debug, Clone)]
struct Check {
    name: &'static str,
    status: Status,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warning,
    Error,
}

/// Run all health checks and print a human-readable report. `config` is the
/// file `--config` names; without it, `devscriptor.yaml` in the working
/// directory is checked, if there is one.
pub async fn execute(
    db_path: std::path::PathBuf,
    config: Option<std::path::PathBuf>,
) -> Result<()> {
    println!("\n🏥 Devscriptor Health Check\n");

    let disk_space = check_disk_space(&db_path).await;
    let (project_check, project) = check_project_config(config.as_deref());
    let mut checks = vec![check_embedding_model().await, project_check];
    checks.extend(check_database(&db_path, &project).await);
    checks.extend([
        disk_space,
        check_memory().await,
        check_treesitter_grammars().await,
    ]);

    let mut _ok = 0usize;
    let mut warnings = 0usize;
    let mut errors = 0usize;

    for check in &checks {
        let icon = match check.status {
            Status::Ok => "✅",
            Status::Warning => "⚠️",
            Status::Error => "❌",
        };
        println!("{icon}  {:<25} {}", check.name, check.message);
        match check.status {
            Status::Ok => _ok += 1,
            Status::Warning => warnings += 1,
            Status::Error => errors += 1,
        }
    }

    println!();
    if errors > 0 {
        println!("❌  {} error(s), {} warning(s) found.", errors, warnings);
        println!("   Some features may not work correctly.");
    } else if warnings > 0 {
        println!(
            "⚠️  {} warning(s) found — devscriptor should still work.",
            warnings
        );
    } else {
        println!("✅  All checks passed — devscriptor is ready!");
    }
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

async fn check_embedding_model() -> Check {
    use devscriptor_core::embeddings::model_fetch;

    let cache_dir = match model_fetch::model_cache_dir() {
        Ok(d) => d,
        Err(_) => {
            return Check {
                name: "Embedding model",
                status: Status::Warning,
                message: "Could not determine cache directory".to_string(),
            };
        }
    };

    if model_fetch::model_files_present(&cache_dir) {
        Check {
            name: "Embedding model",
            status: Status::Ok,
            message: format!("Ready at {}", cache_dir.display()),
        }
    } else if cache_dir.exists() {
        Check {
            name: "Embedding model",
            status: Status::Warning,
            message: format!(
                "Incomplete model cache at {} — missing files will be re-fetched on first use",
                cache_dir.display()
            ),
        }
    } else {
        Check {
            name: "Embedding model",
            status: Status::Warning,
            message: format!(
                "Not downloaded ({}) — will be fetched (Hugging Face, mirror fallback) on first scan/embed",
                cache_dir.display()
            ),
        }
    }
}

/// The project's configuration file: whether there is one, and whether its
/// settings can be applied. Reported, not fatal: a file with errors leaves
/// the defaults to the other checks.
fn check_project_config(explicit: Option<&Path>) -> (Check, ProjectConfig) {
    let found = std::env::current_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| ProjectConfig::find(explicit, &dir).map_err(|e| e.to_string()));
    match found {
        Ok(None) => (
            Check {
                name: "Project config",
                status: Status::Ok,
                message: "No devscriptor.yaml here — the defaults apply".to_string(),
            },
            ProjectConfig::default(),
        ),
        Ok(Some(located)) => {
            let warnings = located.config.check().warnings;
            let check = match warnings.first() {
                None => Check {
                    name: "Project config",
                    status: Status::Ok,
                    message: format!("{} — valid", located.path.display()),
                },
                Some(first) => Check {
                    name: "Project config",
                    status: Status::Warning,
                    message: format!(
                        "{}: {first}{}",
                        located.path.display(),
                        match warnings.len() {
                            1 => String::new(),
                            n => format!(" (and {} more; `devscriptor validate`)", n - 1),
                        }
                    ),
                },
            };
            (check, located.config)
        }
        Err(e) => (
            Check {
                name: "Project config",
                status: Status::Error,
                message: format!("{e} — the defaults apply until it is fixed"),
            },
            ProjectConfig::default(),
        ),
    }
}

/// The database at `db_path` (the global `--database`), and whether its
/// entities have the vectors semantic search needs. A database that does not
/// exist is reported, not created.
async fn check_database(db_path: &Path, project: &ProjectConfig) -> Vec<Check> {
    if !db_path.exists() {
        return vec![Check {
            name: "Database",
            status: Status::Warning,
            message: format!(
                "None at {} — run `devscriptor scan <project>`, or pass --database",
                db_path.display()
            ),
        }];
    }
    let db = match Database::new(database_config(db_path, project)) {
        Ok(d) => d,
        Err(e) => {
            return vec![Check {
                name: "Database",
                status: Status::Error,
                message: format!("Failed to open database: {e}"),
            }];
        }
    };

    // `get_stats` returns (file_count, entity_count, relationship_count).
    let database = match db.get_stats().await {
        Ok((file_count, entity_count, relationship_count)) => Check {
            name: "Database",
            status: Status::Ok,
            message: format!(
                "Open OK — {entity_count} entities, {relationship_count} relationships, {file_count} files"
            ),
        },
        Err(e) => Check {
            name: "Database",
            status: Status::Warning,
            message: format!("Open OK but stats query failed: {e}"),
        },
    };
    vec![database, check_embeddings(&db, project).await]
}

/// Whether the entities `devscriptor embed` would embed (the project's
/// embedding selection) have their vectors: without them semantic search
/// falls back to keywords for those entities.
async fn check_embeddings(db: &Database, project: &ProjectConfig) -> Check {
    match db
        .get_unembedded_entity_ids(false, project.embedding_selection())
        .await
    {
        Ok((pending, _)) if pending.is_empty() => Check {
            name: "Embeddings",
            status: Status::Ok,
            message: "Every entity to embed has its vector — semantic search is ready".to_string(),
        },
        Ok((pending, _)) => Check {
            name: "Embeddings",
            status: Status::Warning,
            message: format!(
                "{} entities have no vector yet — run `devscriptor embed` for full semantic search",
                pending.len()
            ),
        },
        Err(e) => Check {
            name: "Embeddings",
            status: Status::Warning,
            message: format!("Could not count the entities without a vector: {e}"),
        },
    }
}

async fn check_disk_space(db_path: &Path) -> Check {
    // The free space that matters is on the filesystem holding the database:
    // its directory, made absolute, or the nearest ancestor that exists.
    let dir = std::path::absolute(db_path)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let dir = dir
        .ancestors()
        .find_map(|p| dunce::canonicalize(p).ok())
        .unwrap_or(dir);

    let disks = sysinfo::Disks::new_with_refreshed_list();
    // The deepest mount point containing the directory; `/` contains every
    // path, so the first match in list order would usually be the wrong disk.
    let best_match = disks
        .list()
        .iter()
        .filter(|disk| dir.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())
        .map(|disk| {
            (
                disk.mount_point(),
                disk.available_space(),
                disk.total_space(),
            )
        });

    if let Some((mount, avail, total)) = best_match {
        let avail_gb = avail / 1024 / 1024 / 1024;
        // From bytes: whole gigabytes rounded small disks to 0% or 100%.
        let pct = avail
            .saturating_mul(100)
            .checked_div(total)
            .map_or(0, |avail_pct| 100u64.saturating_sub(avail_pct));

        let status = if avail_gb < 1 {
            Status::Error
        } else {
            Status::Ok
        };
        return Check {
            name: "Disk space",
            status,
            message: format!("{avail_gb} GB free on {} ({}% used)", mount.display(), pct),
        };
    }

    Check {
        name: "Disk space",
        status: Status::Warning,
        message: "Could not determine disk usage".to_string(),
    }
}

async fn check_memory() -> Check {
    // `new()` loads nothing; `new_all()` also read every process and CPU.
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    let total_gb = sys.total_memory() / 1024 / 1024 / 1024;
    let avail_gb = sys.available_memory() / 1024 / 1024 / 1024;

    if total_gb < 4 {
        Check {
            name: "Memory",
            status: Status::Warning,
            message: format!(
                "{} GB total RAM ({} GB available) — embedding throughput will be limited",
                total_gb, avail_gb
            ),
        }
    } else {
        Check {
            name: "Memory",
            status: Status::Ok,
            message: format!("{} GB total ({} GB available)", total_gb, avail_gb),
        }
    }
}

async fn check_treesitter_grammars() -> Check {
    // The core crate bundles grammars at compile time; we just verify that
    // the parser module can be instantiated.
    use devscriptor_core::parser::{ParserConfig, TreeSitterParser};

    match TreeSitterParser::new(ParserConfig::default()) {
        Ok(_) => Check {
            name: "Tree-sitter grammars",
            status: Status::Ok,
            message: "Parser initialised successfully".to_string(),
        },
        Err(e) => Check {
            name: "Tree-sitter grammars",
            status: Status::Error,
            message: format!("Parser failed to initialise: {e}"),
        },
    }
}
