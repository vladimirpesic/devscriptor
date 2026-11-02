//! Embedding generation and storage for entities
//!
//! This module provides functionality to generate embeddings for entities
//! and store them in the vector database.

use crate::database::connection::ConnectionPool;
use crate::database::vector;
use crate::embeddings::EmbeddingService;
use crate::entities::Entity;
use crate::error::{Error, Result};

/// Configuration for embedding generation
#[derive(Debug, Clone)]
pub struct EmbeddingGenerationConfig {
    /// Enable embedding generation
    pub enabled: bool,
    /// Generate embeddings for functions
    pub embed_functions: bool,
    /// Generate embeddings for classes
    pub embed_classes: bool,
    /// Generate embeddings for other entity types
    pub embed_other: bool,
    /// Maximum batch size for embedding generation
    pub batch_size: usize,
}

impl Default for EmbeddingGenerationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            embed_functions: true,
            embed_classes: true,
            embed_other: true,
            batch_size: 16,
        }
    }
}

/// Generate embeddings for entities and store them in the database
///
/// # Errors
/// Returns an error if embedding generation or database operations fail
pub async fn generate_and_store_embeddings(
    pool: &ConnectionPool,
    embedding_service: &EmbeddingService,
    entities: &[Entity],
    config: &EmbeddingGenerationConfig,
) -> Result<usize> {
    if !config.enabled {
        return Ok(0);
    }

    // Filter entities based on configuration
    let entities_to_embed: Vec<&Entity> = entities
        .iter()
        .filter(|e| match e.entity_type() {
            "function" => config.embed_functions,
            "class" | "table" => config.embed_classes,
            _ => config.embed_other,
        })
        .collect();

    if entities_to_embed.is_empty() {
        return Ok(0);
    }

    let mut embedded_count = 0;
    let mut failures = Vec::new();

    // Process in batches to avoid memory issues
    for chunk in entities_to_embed.chunks(config.batch_size) {
        // Prepare texts for embedding generation
        let texts: Vec<String> = chunk.iter().map(|e| entity_to_text(e)).collect();

        // Generate embeddings
        let embeddings = tokio::task::spawn_blocking({
            let embedding_service = embedding_service.clone();
            move || embedding_service.generate_embeddings_batch(&texts)
        })
        .await
        .map_err(|e| Error::Embedding(format!("Embedding task failed: {e}")))??;

        // Store embeddings in the database
        for (entity, embedding) in chunk.iter().zip(embeddings.iter()) {
            let entity_id = entity.id();
            match vector::upsert_embedding(pool, entity_id, embedding).await {
                Ok(()) => embedded_count += 1,
                Err(e) => {
                    tracing::warn!("Failed to store embedding for entity {}: {}", entity_id, e);
                    failures.push((entity_id, e.to_string()));
                }
            }
        }
    }

    tracing::info!("Generated and stored {} embeddings", embedded_count);

    if !failures.is_empty() {
        return Err(Error::Embedding(format!(
            "Failed to store {} of {} embeddings. First failure: entity {} — {}",
            failures.len(),
            entities_to_embed.len(),
            failures[0].0,
            failures[0].1
        )));
    }

    Ok(embedded_count)
}

/// Maximum number of characters fed to the embedding model per entity.
///
/// The tokenizer truncates at `max_length = 512` (see `TractEmbedder::load`), so
/// only the first ~512 tokens of any input are ever used. At ~4 characters per
/// code token that is ≈2048 characters — feeding more only wastes tokenizer
/// work and memory. Capping the *input string* here also bounds the
/// `embed_texts` the pipeline holds between flushes, which matters on large
/// repositories.
pub const MAX_EMBED_TEXT_CHARS: usize = 2048;

/// Truncate `text` to at most [`MAX_EMBED_TEXT_CHARS`] characters on a UTF-8
/// boundary (never panics on multi-byte input).
fn truncate_for_embedding(text: String) -> String {
    if text.len() <= MAX_EMBED_TEXT_CHARS {
        // `len()` is bytes; if byte length already fits, char length does too.
        return text;
    }
    // Find the largest char boundary ≤ MAX_EMBED_TEXT_CHARS bytes.
    let mut end = MAX_EMBED_TEXT_CHARS;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Convert an entity to text for embedding generation.
///
/// The returned text is truncated to [`MAX_EMBED_TEXT_CHARS`] so that batched
/// ONNX inference has a bounded, predictable memory footprint regardless of how
/// large individual code bodies are.
pub fn entity_to_text(entity: &Entity) -> String {
    let text = match entity {
        Entity::Function(f) => {
            let mut text = format!("function {}\n", f.name);
            if let Some(params) = &f.parameters {
                text.push_str(&format!("parameters: {}\n", params));
            }
            if let Some(return_type) = &f.return_type {
                text.push_str(&format!("returns: {}\n", return_type));
            }
            if let Some(doc) = &f.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&f.body);
            text
        }
        Entity::Class(c) => {
            let mut text = format!("class {}\n", c.name);
            if let Some(extends) = &c.extends {
                text.push_str(&format!("extends: {}\n", extends));
            }
            if let Some(doc) = &c.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&c.body);
            text
        }
        // SQL table/view. The body carries the `CREATE TABLE …` text (including
        // columns), so column names stay searchable through the table.
        Entity::Table(t) => {
            let mut text = format!("table {}\n", t.name);
            if let Some(doc) = &t.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&t.body);
            text
        }
        Entity::Struct(s) => {
            let mut text = format!("struct {}\n", s.name);
            if let Some(doc) = &s.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&s.body);
            text
        }
        Entity::Enum(e) => {
            let mut text = format!("enum {}\n", e.name);
            if let Some(doc) = &e.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&e.body);
            text
        }
        Entity::Interface(i) => {
            let mut text = format!("interface {}\n", i.name);
            if let Some(doc) = &i.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&i.body);
            text
        }
        Entity::Trait(t) => {
            let mut text = format!("trait {}\n", t.name);
            if let Some(doc) = &t.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&t.body);
            text
        }
        Entity::Module(m) => {
            let mut text = format!("module {}\n", m.name);
            text.push_str(&m.body);
            text
        }
        Entity::Namespace(n) => {
            let mut text = format!("namespace {}\n", n.name);
            text.push_str(&n.body);
            text
        }
        Entity::Constant(c) => {
            let mut text = format!("constant {}\n", c.name);
            if let Some(doc) = &c.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&c.body);
            text
        }
        Entity::TypeAlias(t) => {
            let mut text = format!("type alias {} = {}\n", t.name, t.aliased_type);
            if let Some(doc) = &t.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&t.body);
            text
        }
        Entity::Union(u) => {
            let mut text = format!("union {}\n", u.name);
            if let Some(doc) = &u.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&u.body);
            text
        }
        Entity::Macro(m) => {
            let mut text = format!("macro {}\n", m.name);
            if let Some(doc) = &m.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&m.body);
            text
        }
        Entity::Component(c) => {
            let mut text = format!("component {}\n", c.name);
            if let Some(doc) = &c.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&c.body);
            text
        }
        Entity::Contract(c) => {
            let mut text = format!("contract {}\n", c.name);
            if let Some(doc) = &c.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&c.body);
            text
        }
        Entity::Extension(e) => {
            let mut text = format!("extension {}\n", e.name);
            if let Some(extended) = &e.extended_type {
                text.push_str(&format!("extends: {}\n", extended));
            }
            if let Some(doc) = &e.docstring {
                text.push_str(&format!("documentation: {}\n", doc));
            }
            text.push_str(&e.body);
            text
        }
        Entity::Decorator(d) => {
            let mut text = format!("decorator {}\n", d.name);
            text.push_str(&d.body);
            text
        }
        // The heading is the question a reader would ask; the body is the
        // answer. Both go in, with the file so a section is anchored to the
        // document it came from.
        Entity::Section(d) => {
            let mut text = format!("section {} in {}\n", d.name, d.file_path);
            text.push_str(&d.body);
            text
        }
        Entity::Index(i) => {
            let mut text = format!("index {}\n", i.name);
            text.push_str(&i.body);
            text
        }
    };
    truncate_for_embedding(text)
}

/// Split a code identifier into space-separated lower-case tokens for FTS5
/// indexing.  Handles:
/// - `snake_case`    → "snake case"
/// - `camelCase`     → "camel case"
/// - `PascalCase`    → "pascal case"
/// - `SCREAMING`     → "screaming"
/// - `dot.notation`  → "dot notation"
/// - `kebab-case`    → "kebab case"
///
/// The result is stored in the `name_tokens` FTS5 column so that substring
/// queries like `userBy` match `getUserById` via the token "get user by id".
pub fn split_identifier_tokens(name: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    // First split on non-alphanumeric separators, then at the case changes
    // that start a word: `camel|Case`, and `HTTP|Server`, where the last
    // capital of a run begins the next word. A run of capitals with no
    // lower-case after it is one word (`SCREAMING`, `parse|JSON`). Splitting
    // at every capital turned `SCREAMING` into "s c r e a m i n g".
    for word in name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let chars: Vec<char> = word.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let (prev, cur) = (chars[i - 1], chars[i]);
            let next_is_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            let starts_word = cur.is_uppercase()
                && (prev.is_lowercase()
                    || prev.is_numeric()
                    || (prev.is_uppercase() && next_is_lower));
            if starts_word {
                tokens.push(chars[start..i].iter().collect::<String>().to_lowercase());
                start = i;
            }
        }
        tokens.push(chars[start..].iter().collect::<String>().to_lowercase());
    }

    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_split_into_their_words() {
        for (name, tokens) in [
            ("getUserById", "get user by id"),
            ("snake_case", "snake case"),
            ("PascalCase", "pascal case"),
            ("SCREAMING", "screaming"),
            ("FREE_SHIPPING_THRESHOLD", "free shipping threshold"),
            ("HTTPServer", "http server"),
            ("parseJSON", "parse json"),
            ("XMLHttpRequest", "xml http request"),
            ("dot.notation", "dot notation"),
            ("kebab-case", "kebab case"),
            ("sha256Digest", "sha256 digest"),
            ("x", "x"),
        ] {
            assert_eq!(split_identifier_tokens(name), tokens, "{name}");
        }
    }
}
