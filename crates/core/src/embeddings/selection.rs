//! Which extracted entities receive a semantic embedding.
//!
//! Embedding is the most expensive stage of analysis, and not every entity is a
//! meaningful semantic-search target. Constants, decorators-of-noise, usage and
//! metrics records, and trivial undocumented accessors add inference cost and
//! *dilute* search precision. They remain fully discoverable through the
//! lexical paths (`name`, `name_tokens`, FTS5), and search already falls back to
//! keyword results for entities without a vector — so skipping them is lossless
//! for real-world retrieval while cutting embedding volume.

use crate::entities::Entity;

/// How aggressively to prune entities before embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingSelection {
    /// Embed every named entity: the largest cost, and the noisiest semantic
    /// index.
    All,
    /// Embed substantive, named code units; skip non-semantic noise. Default.
    #[default]
    Standard,
    /// `Standard` minus the entities whose body is under 96 bytes — the leanest
    /// index for very large corpora that still want transformer-quality vectors.
    Significant,
}

/// Minimum body length (bytes) for an entity to be embedded under
/// [`EmbeddingSelection::Significant`]. Below this a callable is almost always a
/// one-line accessor/forwarder whose name already carries its meaning.
const MIN_SIGNIFICANT_BODY: usize = 96;

impl EmbeddingSelection {
    /// Parse from `DEVSCRIPTOR_EMBEDDING_SELECTION` (`all` | `standard` |
    /// `significant`), falling back to `default` when unset or unrecognised.
    #[must_use]
    pub fn from_env_or(default: Self) -> Self {
        match std::env::var("DEVSCRIPTOR_EMBEDDING_SELECTION")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("all") => Self::All,
            Some("standard") => Self::Standard,
            Some("significant") => Self::Significant,
            _ => default,
        }
    }

    /// Stable lowercase identifier (config, logs).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Standard => "standard",
            Self::Significant => "significant",
        }
    }
}

/// Decide whether `entity` should be embedded under the given `selection`.
#[must_use]
pub fn should_embed(entity: &Entity, selection: EmbeddingSelection) -> bool {
    // A nameless entity cannot be a useful semantic target and never embeds.
    if entity.name().trim().is_empty() {
        return false;
    }
    match selection {
        EmbeddingSelection::All => true,
        EmbeddingSelection::Standard => is_semantic_unit(entity),
        EmbeddingSelection::Significant => {
            is_semantic_unit(entity) && entity.body().trim().len() >= MIN_SIGNIFICANT_BODY
        }
    }
}

/// Entity type strings that `is_semantic_unit` accepts.
///
/// The same policy as the match below, expressed as strings so the embedding
/// pass can filter in SQL without deserialising every row. Kept in step by
/// `the_type_list_matches_the_policy`.
pub const SEMANTIC_ENTITY_TYPES: &[&str] = &[
    "function",
    "class",
    "table",
    "struct",
    "enum",
    "interface",
    "trait",
    "module",
    "namespace",
    "type_alias",
    "union",
    "macro",
    "component",
    "contract",
    "extension",
    "section",
];

/// The whitespace `str::trim` removes, as SQLite's `trim(X, Y)` takes it.
const SQL_WHITESPACE: &str = "' ' || char(9, 10, 11, 12, 13)";

/// `selection` as an SQL condition over a row of `entities` aliased `e`: the
/// same policy as [`should_embed`], for the passes that pick the entities to
/// embed in SQL (`devscriptor embed`, `generate_embeddings`) without
/// deserialising every row. A body is measured in bytes, as there.
#[must_use]
pub fn sql_condition(selection: EmbeddingSelection) -> String {
    let named = format!("trim(e.name, {SQL_WHITESPACE}) <> ''");
    let types = SEMANTIC_ENTITY_TYPES
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",");
    match selection {
        EmbeddingSelection::All => named,
        EmbeddingSelection::Standard => format!("{named} AND e.type IN ({types})"),
        EmbeddingSelection::Significant => format!(
            "{named} AND e.type IN ({types}) \
             AND length(CAST(trim(e.body, {SQL_WHITESPACE}) AS BLOB)) >= {MIN_SIGNIFICANT_BODY}"
        ),
    }
}

/// Whether an entity of this stored type is worth embedding.
#[must_use]
pub fn is_semantic_type(entity_type: &str) -> bool {
    SEMANTIC_ENTITY_TYPES.contains(&entity_type)
}

/// `true` for entity kinds that represent a self-contained unit of code or type
/// worth retrieving semantically. The match is exhaustive so a new [`Entity`]
/// variant forces an explicit decision here.
fn is_semantic_unit(entity: &Entity) -> bool {
    match entity {
        // Substantive, named code/type units — the targets of semantic search.
        Entity::Function(_)
        | Entity::Class(_)
        | Entity::Table(_)
        | Entity::Struct(_)
        | Entity::Enum(_)
        | Entity::Interface(_)
        | Entity::Trait(_)
        | Entity::Module(_)
        | Entity::Namespace(_)
        | Entity::TypeAlias(_)
        | Entity::Union(_)
        | Entity::Macro(_)
        | Entity::Component(_)
        | Entity::Contract(_)
        | Entity::Extension(_)
        // A documentation section is the one prose unit worth retrieving by
        // meaning: it answers "why is it this way?", which no code entity can.
        // Docstrings stay out below — they restate the code they sit on, so
        // embedding them returns the same result twice.
        | Entity::Section(_) => true,

        // Lexical-only / non-semantic noise — found by name/FTS, not vectors.
        Entity::Constant(_)
        | Entity::Decorator(_)
        // An index is a performance artifact, not a unit of meaning: its text
        // is a table name and a column list, near-identical to every other
        // index, so vectors would cluster them together and say nothing. Found
        // by name and by the FTS body match instead.
        | Entity::Index(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Constant, Function, SourceLocation};
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn func(name: &str, body: &str) -> Entity {
        let now = OffsetDateTime::now_utc();
        Entity::Function(Function {
            id: Uuid::new_v4(),
            name: name.to_string(),
            file_path: "test.rs".to_string(),
            body: body.to_string(),
            location: SourceLocation::new(1, 0, 2, 0),
            language: "rust".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: None,
            return_type: None,
            modifiers: None,
            access_modifiers: None,
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: now,
            updated_at: now,
        })
    }

    fn constant(name: &str) -> Entity {
        let now = OffsetDateTime::now_utc();
        Entity::Constant(Constant {
            id: Uuid::new_v4(),
            name: name.to_string(),
            file_path: "test.rs".to_string(),
            body: "const X = 1".to_string(),
            location: SourceLocation::new(1, 0, 1, 0),
            language: "rust".to_string(),
            value: None,
            constant_type: None,
            parent_class: None,
            docstring: None,
            created_at: now,
            updated_at: now,
        })
    }

    #[test]
    fn all_embeds_everything_named() {
        assert!(should_embed(&func("f", ""), EmbeddingSelection::All));
        assert!(should_embed(&constant("K"), EmbeddingSelection::All));
    }

    #[test]
    fn standard_skips_noise_keeps_code_units() {
        assert!(should_embed(
            &func("compute", "x + y"),
            EmbeddingSelection::Standard
        ));
        assert!(!should_embed(
            &constant("MAX"),
            EmbeddingSelection::Standard
        ));
    }

    #[test]
    fn significant_skips_trivial_bodies() {
        // Comfortably exceeds MIN_SIGNIFICANT_BODY (96 bytes).
        let big = "let total = items.iter().map(|i| i.weight * i.count).sum::<u64>(); \
                   validate(total)?; persist(total).await?; Ok(total)";
        assert!(big.len() >= MIN_SIGNIFICANT_BODY);
        assert!(should_embed(
            &func("real", big),
            EmbeddingSelection::Significant
        ));
        assert!(!should_embed(
            &func("get", "x"),
            EmbeddingSelection::Significant
        ));
    }

    #[test]
    fn nameless_never_embeds() {
        assert!(!should_embed(&func("", "body"), EmbeddingSelection::All));
    }

    /// The SQL-facing list and the policy match must agree, or the embedding
    /// pass embeds a different set than the policy describes.
    #[test]
    fn the_type_list_matches_the_policy() {
        for entity in [func("f", "body"), constant("K")] {
            assert_eq!(
                is_semantic_unit(&entity),
                is_semantic_type(entity.entity_type()),
                "policy and type list disagree about {}",
                entity.entity_type()
            );
        }
    }
}
