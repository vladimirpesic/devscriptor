//! Search command implementations

use std::sync::Arc;

use anyhow::Result;
use devscriptor_core::database::Database;
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService};
use devscriptor_core::entities::Entity;
use devscriptor_core::search::{CodebaseSearcher, SearchOptions, SemanticResult};
use devscriptor_core::utils::number_lines;

use crate::commands::common::{OutputFormat, SearchType};

/// Create a searcher from an existing database handle.
///
/// It always has an `EmbeddingService`, so semantic (vector) search queries the
/// stored embeddings. The service loads the model at the first semantic query,
/// and unloads it after the CLI query completes.
fn open_searcher(database: &Database) -> CodebaseSearcher {
    let service = EmbeddingService::new(EmbeddingConfig::default());
    CodebaseSearcher::with_embedding_service(database.conn_pool().clone(), service)
}

/// Extract a short preview of the entity body (first three lines, at most 120
/// bytes, cut at a character boundary).
fn body_preview(body: &str) -> String {
    let preview: String = body.lines().take(3).collect::<Vec<_>>().join("\n");
    if preview.len() > 120 {
        format!("{}...", &preview[..preview.floor_char_boundary(120)])
    } else {
        preview
    }
}

/// Print `preview`, which starts at line `first` of its file, under its
/// result: each line indented and numbered as in the file.
fn print_preview(preview: &str, first: u32) {
    if preview.is_empty() {
        println!("     <no preview>");
    }
    for line in number_lines(preview, first).lines() {
        println!("     {line}");
    }
    println!();
}

/// Print a list of entities with name, file path, line number, and body preview.
fn print_entities(entities: &[Entity]) {
    if entities.is_empty() {
        println!("No results found.");
        return;
    }
    println!("Found {} result(s):\n", entities.len());
    for (i, entity) in entities.iter().enumerate() {
        let loc = entity.location();
        println!(
            "  {}. {} ({})\n     File: {}:{}",
            i + 1,
            entity.name(),
            entity.entity_type(),
            entity.file_path(),
            loc.start_line,
        );
        print_preview(&body_preview(entity.body()), loc.start_line);
    }
}

/// Print a list of semantic search results.
fn print_semantic_results(results: &[SemanticResult]) {
    if results.is_empty() {
        println!("No results found.");
        return;
    }
    let fallback = results.first().map(|r| r.fallback).unwrap_or(false);
    if fallback {
        println!(
            "Found {} result(s) (⚠ keyword fallback — no embedding model available):\n",
            results.len()
        );
    } else {
        println!("Found {} result(s):\n", results.len());
    }
    for (i, r) in results.iter().enumerate() {
        println!(
            "  {}. {} ({}) — relevance: {:.2}{}\n     File: {}:{}",
            i + 1,
            r.name,
            r.entity_type,
            r.similarity,
            if r.fallback { " [fallback]" } else { "" },
            r.file_path,
            r.start_line,
        );
        print_preview(r.body_preview.as_deref().unwrap_or_default(), r.start_line);
    }
}

/// One search hit in `--output json`.
///
/// `name`, `type`, `file`, `line` and `score` are what the VS Code extension
/// reads; `score` is present for semantic results only.
#[derive(Debug, serde::Serialize)]
struct JsonHit {
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    file: String,
    line: u32,
    end_line: u32,
    language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f64>,
    /// Semantic results that came from the keyword fallback (no model).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    fallback: bool,
}

impl From<&Entity> for JsonHit {
    fn from(entity: &Entity) -> Self {
        let location = entity.location();
        Self {
            name: entity.name().to_string(),
            entity_type: entity.entity_type().to_string(),
            file: entity.file_path().to_string(),
            line: location.start_line,
            end_line: location.end_line,
            language: entity.language().to_string(),
            score: None,
            fallback: false,
        }
    }
}

impl From<&SemanticResult> for JsonHit {
    fn from(result: &SemanticResult) -> Self {
        Self {
            name: result.name.clone(),
            entity_type: result.entity_type.clone(),
            file: result.file_path.clone(),
            line: result.start_line,
            end_line: result.end_line,
            language: result.language.clone(),
            score: Some(result.similarity),
            fallback: result.fallback,
        }
    }
}

/// Print hits as one JSON array on stdout.
fn print_json(hits: &[JsonHit]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(hits)?);
    Ok(())
}

/// Execute search command
pub async fn execute(
    query: String,
    database: Arc<Database>,
    search_type: SearchType,
    limit: usize,
    output: OutputFormat,
) -> Result<()> {
    let searcher = open_searcher(&database);
    let json = matches!(output, OutputFormat::Json);

    let entities: Vec<Entity> = match search_type {
        SearchType::Function => {
            searcher
                .search_functions(&query, true, Some(limit), None)
                .await?
        }
        SearchType::Class => {
            searcher
                .search_classes(&query, true, Some(limit), None)
                .await?
        }
        SearchType::Symbol => {
            let options = SearchOptions {
                fuzzy: true,
                limit: Some(limit),
                ..SearchOptions::default()
            };
            let results = searcher.search_entities(&query, options).await?;
            results.into_iter().map(|r| r.entity).collect()
        }
        SearchType::Semantic => {
            let results = searcher.find_similar(&query, limit).await?;
            if json {
                return print_json(&results.iter().map(JsonHit::from).collect::<Vec<_>>());
            }
            print_semantic_results(&results);
            return Ok(());
        }
    };

    if json {
        print_json(&entities.iter().map(JsonHit::from).collect::<Vec<_>>())
    } else {
        print_entities(&entities);
        Ok(())
    }
}

/// Search for functions
pub async fn search_function(
    name: String,
    database: Arc<Database>,
    fuzzy: bool,
    limit: usize,
) -> Result<()> {
    let searcher = open_searcher(&database);
    let results = searcher
        .search_functions(&name, fuzzy, Some(limit), None)
        .await?;

    print_entities(&results);
    Ok(())
}

/// Search for classes
pub async fn search_class(
    name: String,
    database: Arc<Database>,
    fuzzy: bool,
    limit: usize,
) -> Result<()> {
    let searcher = open_searcher(&database);
    let results = searcher
        .search_classes(&name, fuzzy, Some(limit), None)
        .await?;

    print_entities(&results);
    Ok(())
}

/// Find similar code
pub async fn find_similar(query: String, database: Arc<Database>, limit: usize) -> Result<()> {
    let searcher = open_searcher(&database);
    let results = searcher.find_similar(&query, limit).await?;

    print_semantic_results(&results);
    Ok(())
}

/// Search by pattern
pub async fn search_pattern(
    pattern: String,
    database: Arc<Database>,
    entity_type: Option<String>,
    language: Option<String>,
) -> Result<()> {
    let searcher = open_searcher(&database);

    let options = SearchOptions {
        fuzzy: true,
        entity_type,
        language,
        ..SearchOptions::default()
    };

    let results = searcher.search_entities(&pattern, options).await?;
    let entities: Vec<Entity> = results.into_iter().map(|r| r.entity).collect();

    if entities.is_empty() {
        println!("No results found for pattern: {pattern}");
    } else {
        print_entities(&entities);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview is cut at 120 bytes, at a character boundary: inside `é` it would panic.
    #[test]
    fn body_preview_cuts_at_a_character_boundary() {
        let body = format!("{}é tail", "x".repeat(119));
        let preview = body_preview(&body);
        assert_eq!(preview, format!("{}...", "x".repeat(119)));
    }

    /// The keys the VS Code extension reads, and `score` only when there is one.
    #[test]
    fn a_semantic_hit_serializes_the_keys_the_extension_reads() {
        let hit = JsonHit {
            name: "apply_discount".into(),
            entity_type: "function".into(),
            file: "/p/pricing.py".into(),
            line: 4,
            end_line: 5,
            language: "python".into(),
            score: Some(1.0),
            fallback: false,
        };
        let value = serde_json::to_value(&hit).unwrap();
        assert_eq!(value["name"], "apply_discount");
        assert_eq!(value["type"], "function");
        assert_eq!(value["file"], "/p/pricing.py");
        assert_eq!(value["line"], 4);
        assert_eq!(value["score"], 1.0);
        assert!(value.get("fallback").is_none());

        let keyword = JsonHit { score: None, ..hit };
        assert!(
            serde_json::to_value(&keyword)
                .unwrap()
                .get("score")
                .is_none()
        );
    }
}
