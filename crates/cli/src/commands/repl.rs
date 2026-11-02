//! REPL command implementation

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use devscriptor_core::database::Database;
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};
use devscriptor_core::search::{CodebaseSearcher, SearchOptions};
use devscriptor_core::utils::validation::PathValidationOptions;
use tokio::io::AsyncBufReadExt;

use crate::commands::common::validate_path;

/// Start REPL mode.
///
/// Uses Tokio's async stdin to avoid pinning an async runtime worker,
/// and awaits database operations directly instead of nesting
/// `block_on` inside `spawn_blocking` (which can deadlock when the
/// blocking thread pool is exhausted).
/// Accepts a shared `Arc<Database>`, so the session uses the CLI's one
/// connection pool.
pub async fn start(database: Arc<Database>) -> Result<()> {
    let db_path = database.path().to_string();
    println!("\n{}", "═".repeat(60));
    println!(" Devscriptor REPL");
    println!("{}", "═".repeat(60));
    println!("\nDatabase: {}", db_path);
    println!("\nCommands:");
    println!("  help              - Show this help message");
    println!("  scan <path>       - Scan a directory");
    println!("  search <query>    - Search for symbols");
    println!("  analyze           - Run analysis");
    println!("  stats             - Show statistics");
    println!("  quit              - Exit REPL");
    println!();

    // An embedding service, so semantic queries search the stored vectors. It
    // loads the model at the first semantic query, and keeps it for the rest of
    // the session.
    let service = EmbeddingService::new(EmbeddingConfig::default());
    let searcher = CodebaseSearcher::with_embedding_service(database.conn_pool().clone(), service);

    run_repl_loop(&PathBuf::from(db_path), &database, &searcher).await
}

/// Async REPL loop using Tokio's async I/O.
///
/// Reads from `tokio::io::stdin` and awaits database/search operations
/// directly, eliminating the `block_on` inside `spawn_blocking`
/// anti-pattern.
async fn run_repl_loop(database: &Path, db: &Database, searcher: &CodebaseSearcher) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut input = String::new();

    loop {
        print!("> ");
        std::io::stdout().flush()?;

        input.clear();
        if reader.read_line(&mut input).await? == 0 {
            // EOF
            break;
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        match parts[0] {
            "help" => {
                println!("\nCommands:");
                println!("  help              - Show this help message");
                println!("  scan <path>       - Scan a directory");
                println!("  search <query>    - Search for symbols");
                println!("  analyze           - Run analysis");
                println!("  stats             - Show statistics");
                println!("  quit              - Exit REPL");
            }
            "scan" => {
                if parts.len() > 1 {
                    match validate_path(parts[1], &PathValidationOptions::must_be_directory()) {
                        Ok(validated) => {
                            println!("Scanning: {}", validated.display());
                            println!("  (Use the CLI 'scan' command for full scanning)");
                        }
                        Err(e) => {
                            println!("Invalid path: {e}");
                        }
                    }
                } else {
                    println!("Usage: scan <path>");
                }
            }
            "search" => {
                if parts.len() > 1 {
                    let query = parts[1..].join(" ");
                    match searcher
                        .search_entities(&query, SearchOptions::default())
                        .await
                    {
                        Ok(results) => {
                            println!("Found {} results", results.len());
                            for (i, r) in results.iter().take(10).enumerate() {
                                println!(
                                    "  {}. {} ({})",
                                    i + 1,
                                    r.entity.name(),
                                    r.entity.entity_type()
                                );
                            }
                        }
                        Err(e) => println!("Search error: {e}"),
                    }
                } else {
                    println!("Usage: search <query>");
                }
            }
            "analyze" => {
                println!("Running analysis...");
                println!("  (Use the CLI 'analyze' command for full analysis)");
            }
            // `get_stats` returns (file_count, entity_count, relationship_count).
            "stats" => match db.get_stats().await {
                Ok((file_count, entity_count, relationship_count)) => {
                    println!("Database: {}", database.display());
                    println!("  Files:         {file_count}");
                    println!("  Entities:      {entity_count}");
                    println!("  Relationships: {relationship_count}");
                }
                Err(e) => println!("Stats error: {e}"),
            },
            "quit" | "exit" => {
                println!("Goodbye!");
                break;
            }
            _ => {
                println!("Unknown command: {}", parts[0]);
                println!("Type 'help' for available commands.");
            }
        }
    }

    Ok(())
}
