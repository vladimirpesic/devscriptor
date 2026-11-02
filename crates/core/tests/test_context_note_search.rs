//! Context-note search ranks the notes themselves.
//!
//! Notes are embedded in `note_vectors` from their own text. Scored by the
//! similarity of a code entity a note links to instead, a note without links
//! would never be found, a note linked to many entities would come back many
//! times, and hybrid search would count each copy, so the note that matches a
//! query best could rank last.
//!
//! The mock provider's vectors carry no meaning; a query identical to a note's
//! embedded text (its title, a blank line, its content) is the one query whose
//! nearest note is known.

use std::collections::HashSet;
use std::sync::Arc;

use devscriptor_core::context_graph::{
    ContextConfig, ContextGraphManager, ContextNote, ContextSearchResult, EntityLinkSpec, NoteType,
    SearchFilters, SearchType,
};
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::{EmbeddingService, MockEmbeddingProvider};
use tempfile::TempDir;

const NOTES: [(&str, &str); 4] = [
    (
        "Money is integer cents everywhere",
        "Every component keeps money as an integer number of cents.",
    ),
    (
        "Pricing rules are duplicated",
        "Rules 1 to 6 are implemented in pricing-rust and orders-go alike.",
    ),
    (
        "Wire format",
        "REST uses snake_case JSON; GraphQL uses camelCase fields.",
    ),
    (
        "Release audit fixes",
        "The audit fixed thirty deviations from the business rules.",
    ),
];

struct Store {
    manager: ContextGraphManager,
    path: std::path::PathBuf,
    _dir: TempDir,
}

fn store() -> Store {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.db");
    let database = Arc::new(
        Database::new(DatabaseConfig {
            path: path.to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        })
        .unwrap(),
    );
    let manager = ContextGraphManager::with_embedding_service(
        database,
        ContextConfig::default(),
        EmbeddingService::with_provider(MockEmbeddingProvider::new()),
    );
    Store {
        manager,
        path,
        _dir: dir,
    }
}

/// The text the store embeds a note from that has no summary.
fn embedded(title: &str, content: &str) -> String {
    format!("{title}\n\n{content}")
}

async fn add_all(store: &Store) -> Vec<i64> {
    let mut ids = Vec::new();
    for (i, (title, content)) in NOTES.iter().enumerate() {
        // The first note has no entity link; the others link to the same
        // function, which a search by linked code would take for a match of all.
        let links = (i > 0).then(|| {
            vec![EntityLinkSpec {
                entity_name: Some("calculate_total".to_string()),
                ..EntityLinkSpec::new("function")
            }]
        });
        let created = store
            .manager
            .add_note(
                ContextNote::new(NoteType::Standard, *title, *content),
                links,
                None,
            )
            .await
            .unwrap();
        ids.push(created.note_id);
    }
    ids
}

async fn search(store: &Store, query: &str, kind: SearchType) -> Vec<ContextSearchResult> {
    store
        .manager
        .search_notes_with_type(query, 10, kind, &SearchFilters::new())
        .await
        .unwrap()
}

fn ids(results: &[ContextSearchResult]) -> Vec<i64> {
    results.iter().map(|r| r.note.id.unwrap()).collect()
}

/// A connection of the test's own. Opening the store registered sqlite-vec
/// for every connection of the process, so it reads the vector table.
fn connect(store: &Store) -> rusqlite::Connection {
    rusqlite::Connection::open(&store.path).unwrap()
}

fn vector_rows(store: &Store) -> Vec<(i64, String)> {
    let conn = connect(store);
    let mut stmt = conn
        .prepare("SELECT rowid, text_hash FROM note_vectors ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn the_note_whose_text_is_the_query_ranks_first() {
    let store = store();
    let notes = add_all(&store).await;
    let (title, content) = NOTES[0];
    let query = embedded(title, content);

    let semantic = search(&store, &query, SearchType::Semantic).await;
    assert_eq!(ids(&semantic)[0], notes[0], "{semantic:?}");
    assert!(semantic[0].score > 0.99, "{}", semantic[0].score);
    assert!(!semantic[0].fallback);

    let hybrid = search(&store, &query, SearchType::Hybrid).await;
    assert_eq!(ids(&hybrid)[0], notes[0]);
    assert_eq!(hybrid[0].score, 1.0, "the best fused match scores 1.0");
    assert!(hybrid.windows(2).all(|w| w[0].score >= w[1].score));
}

#[tokio::test]
async fn each_note_is_found_once() {
    let store = store();
    let notes = add_all(&store).await;
    for kind in [SearchType::Semantic, SearchType::Hybrid] {
        let found = ids(&search(&store, "how money is stored", kind).await);
        let unique: HashSet<_> = found.iter().collect();
        assert_eq!(
            unique.len(),
            found.len(),
            "{kind:?} repeats a note: {found:?}"
        );
        assert_eq!(
            unique.len(),
            notes.len(),
            "{kind:?} finds every note: {found:?}"
        );
    }
}

#[tokio::test]
async fn vectors_follow_edits_and_deletions() {
    let store = store();
    let notes = add_all(&store).await;
    search(&store, "anything", SearchType::Semantic).await;
    let before = vector_rows(&store);
    assert_eq!(before.iter().map(|(id, _)| *id).collect::<Vec<_>>(), notes);

    // An edit made past the manager is noticed too: by the text's hash.
    let new_content = "Money is kept as integer cents, never as a float.";
    connect(&store)
        .execute(
            "UPDATE context_notes SET content = ?1 WHERE id = ?2",
            rusqlite::params![new_content, notes[0]],
        )
        .unwrap();
    store.manager.delete_note(notes[3]).await.unwrap();

    let found = search(
        &store,
        &embedded(NOTES[0].0, new_content),
        SearchType::Semantic,
    )
    .await;
    assert_eq!(
        ids(&found)[0],
        notes[0],
        "the edited text is what is searched"
    );
    assert!(
        !ids(&found).contains(&notes[3]),
        "a deleted note is not found"
    );
    let after = vector_rows(&store);
    assert_eq!(
        after.len(),
        3,
        "the deleted note's vector is gone: {after:?}"
    );
    assert_ne!(
        after[0].1, before[0].1,
        "the edited note was embedded again"
    );
    assert_eq!(after[1], before[1], "an unchanged note keeps its vector");
}

#[tokio::test]
async fn keyword_scores_rank_the_matches() {
    let store = store();
    add_all(&store).await;
    let found = search(&store, "integer cents money json", SearchType::Keyword).await;
    assert_eq!(found.len(), 2, "the money note and the wire-format one");
    assert_eq!(found[0].note.title, NOTES[0].0);
    assert_eq!(found[0].score, 1.0);
    assert!(
        found.iter().skip(1).all(|r| r.score < 1.0),
        "a weaker match scores less: {:?}",
        found.iter().map(|r| r.score).collect::<Vec<_>>()
    );
}
