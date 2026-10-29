//! The embedding passes that pick entities in SQL (`devscriptor embed`, the
//! `generate_embeddings` tool) select exactly the entities [`should_embed`]
//! does, under every [`EmbeddingSelection`], so `all` and `significant` act on
//! `embed` as on `scan --embed`.

use std::collections::BTreeSet;
use std::path::Path;

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::{EmbeddingSelection, should_embed};
use devscriptor_core::parser::parse_content_sync;

/// Small and large functions, a constant, a struct and a documented section,
/// so that each selection picks a different set.
const SOURCE: &str = r#"
const LIMIT: u32 = 10;

struct Point { x: i32, y: i32 }

fn get() -> u32 { 1 }

fn total(items: &[u32]) -> u32 {
    let mut sum = 0;
    for item in items {
        sum += item * 2 + 1;
    }
    if sum > LIMIT { sum - LIMIT } else { sum }
}
"#;

#[tokio::test]
async fn the_sql_selection_is_the_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::new(DatabaseConfig {
        path: tmp.path().join("index.db").to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .unwrap();
    let parsed = parse_content_sync(SOURCE, "rust", Path::new("lib.rs")).unwrap();
    db.batch_insert_parsed_files(&[parsed.to_record()], None)
        .await
        .unwrap();

    let mut sets = Vec::new();
    for selection in [
        EmbeddingSelection::All,
        EmbeddingSelection::Standard,
        EmbeddingSelection::Significant,
    ] {
        let expected: BTreeSet<_> = parsed
            .entities
            .iter()
            .filter(|entity| should_embed(entity, selection))
            .map(|entity| entity.id())
            .collect();
        let (ids, total) = db
            .get_unembedded_entity_ids(false, selection)
            .await
            .unwrap();
        assert_eq!(total, parsed.entities.len());
        let selected: BTreeSet<_> = ids.into_iter().collect();
        assert_eq!(selected, expected, "{}", selection.as_str());
        let (forced, _) = db.get_unembedded_entity_ids(true, selection).await.unwrap();
        assert_eq!(
            forced.len(),
            expected.len(),
            "{} forced",
            selection.as_str()
        );
        sets.push(selected);
    }
    // The three policies differ on this file, so the comparison means something.
    assert!(sets[0].len() > sets[1].len(), "all > standard: {sets:?}");
    assert!(
        sets[1].len() > sets[2].len(),
        "standard > significant: {sets:?}"
    );
    assert!(!sets[2].is_empty());
}
