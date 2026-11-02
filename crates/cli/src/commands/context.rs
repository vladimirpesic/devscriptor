//! Context graph command implementation

use std::sync::Arc;

use anyhow::Result;

use devscriptor_core::context_graph::{
    ContextConfig, ContextGraphManager, ContextNote, EntityLinkSpec, PruneStatistics,
};
use devscriptor_core::database::Database;

/// Commands for managing the context graph
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ContextCommands {
    /// Add a new context note
    Add {
        /// Note title
        #[arg(short, long)]
        title: String,
        /// Note content
        #[arg(short, long)]
        content: String,
        /// Note type: adr, standard, optimization, failure, practice,
        /// reflection, general, file_description, or modification
        #[arg(short, long, default_value = "general")]
        note_type: String,
        /// Related entity UUIDs (comma-separated)
        #[arg(long, value_delimiter = ',')]
        entities: Vec<String>,
        /// Entity type for linked entities (function, class, etc.)
        #[arg(long, default_value = "function")]
        entity_type: String,
    },
    /// List all context notes
    List {
        /// Maximum number of notes
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Offset for pagination
        #[arg(short, long, default_value = "0")]
        offset: usize,
    },
    /// Search context notes
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show a specific note by ID
    Show {
        /// Note ID
        id: i64,
    },
    /// Delete a note by ID
    Delete {
        /// Note ID
        id: i64,
    },
    /// Prune old notes
    Prune {
        /// Maximum age in days
        #[arg(short, long, default_value = "90")]
        max_age_days: u32,
        /// Preview only (dry run)
        #[arg(long)]
        dry_run: bool,
    },
    /// Get context related to an entity type
    Entity {
        /// Entity type (function, class, etc.)
        entity_type: String,
    },
    /// Get context related to a concept
    Concept {
        /// Concept name
        concept: String,
    },
}

/// Execute context graph commands
pub async fn execute(db: Arc<Database>, command: ContextCommands) -> Result<()> {
    let config = ContextConfig::default();
    let manager = ContextGraphManager::new(db, config);

    match command {
        ContextCommands::Add {
            title,
            content,
            note_type,
            entities,
            entity_type,
        } => {
            let nt = devscriptor_core::context_graph::NoteType::from_str(&note_type)
                .unwrap_or(devscriptor_core::context_graph::NoteType::General);
            let note = ContextNote::new(nt, title, content);
            let entity_links: Option<Vec<EntityLinkSpec>> = if entities.is_empty() {
                None
            } else {
                Some(
                    entities
                        .into_iter()
                        .map(|e| EntityLinkSpec::new(&entity_type).with_entity_id(e))
                        .collect(),
                )
            };
            let result = manager.add_note(note, entity_links, None).await?;
            println!(
                "Created note #{}: {} ({})",
                result.note_id, result.title, result.status
            );
            if !result.entity_links.is_empty() {
                println!("  Linked to {} entities", result.entity_links.len());
            }
        }
        ContextCommands::List { limit, offset } => {
            let notes = manager.list_notes(limit, offset).await?;
            if notes.is_empty() {
                println!("No context notes found.");
            } else {
                println!("Context Notes ({} total shown):", notes.len());
                for note in notes {
                    println!(
                        "  #{} [{}] {} — {}",
                        note.id.unwrap_or(0),
                        note.note_type.as_str(),
                        note.title,
                        note.content.chars().take(60).collect::<String>()
                    );
                }
            }
        }
        ContextCommands::Search { query, limit } => {
            let results = manager.search_notes(&query, limit).await?;
            if results.is_empty() {
                println!("No notes matching '{}'.", query);
            } else {
                println!("Search results for '{}':", query);
                for r in results {
                    println!(
                        "  #{} [{}] {} (score: {:.2})",
                        r.note.id.unwrap_or(0),
                        r.note.note_type.as_str(),
                        r.note.title,
                        r.score
                    );
                }
            }
        }
        ContextCommands::Show { id } => match manager.get_note(id).await {
            Ok(note) => {
                println!("Note #{} [{}]", id, note.note_type.as_str());
                println!("Title: {}", note.title);
                println!("Content: {}", note.content);
                if let Some(created) = note.created_at {
                    println!("Created: {}", created);
                }
                if let Some(updated) = note.updated_at {
                    println!("Updated: {}", updated);
                }
            }
            Err(e) => {
                println!("Note #{} not found: {}", id, e);
            }
        },
        ContextCommands::Delete { id } => {
            let result = manager.delete_note(id).await?;
            println!(
                "Note #{} {} ({} links removed)",
                result.note_id, result.status, result.links_deleted
            );
        }
        ContextCommands::Prune {
            max_age_days,
            dry_run,
        } => {
            if dry_run {
                let stats: PruneStatistics = manager.get_prune_statistics(max_age_days).await?;
                println!(
                    "Dry run: {} old active + {} old archived notes eligible for pruning (older than {} days)",
                    stats.old_active_notes, stats.old_archived_notes, max_age_days
                );
            } else {
                let (archived, deleted) = manager.prune_notes(max_age_days, false).await?;
                println!(
                    "Pruned {} archived, {} deleted notes (older than {} days)",
                    archived, deleted, max_age_days
                );
            }
        }
        ContextCommands::Entity { entity_type } => {
            let results = manager.get_context_for_entity(&entity_type).await?;
            if results.is_empty() {
                println!("No context notes for entity type '{}'.", entity_type);
            } else {
                println!("Context for '{}' entities:", entity_type);
                for r in results {
                    println!(
                        "  #{} {} (score: {:.2})",
                        r.note.id.unwrap_or(0),
                        r.note.title,
                        r.score
                    );
                }
            }
        }
        ContextCommands::Concept { concept } => {
            let results = manager.get_context_for_concept(&concept).await?;
            if results.is_empty() {
                println!("No context notes for concept '{}'.", concept);
            } else {
                println!("Context for '{}':", concept);
                for r in results {
                    println!(
                        "  #{} {} (score: {:.2})",
                        r.note.id.unwrap_or(0),
                        r.note.title,
                        r.score
                    );
                }
            }
        }
    }

    Ok(())
}
