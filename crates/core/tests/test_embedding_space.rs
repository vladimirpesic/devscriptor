//! An index keeps its vectors in one space: the vectors of another model or
//! arithmetic are dropped once when it is opened, together with the embedding
//! cache.

use devscriptor_core::database::schema::EMBEDDING_SPACE_KEY;
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::EMBEDDING_SPACE;
use uuid::Uuid;

fn open(path: &std::path::Path) -> Database {
    Database::new(DatabaseConfig {
        path: path.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .unwrap()
}

fn stamp(path: &std::path::Path) -> Option<String> {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            [EMBEDDING_SPACE_KEY],
            |row| row.get(0),
        )
        .ok()
}

#[tokio::test]
async fn vectors_of_another_space_are_dropped_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.db");
    let hash = "00000000000000aa".to_string();
    let vector = vec![1.0 / (768.0_f32).sqrt(); 768];
    {
        let db = open(&path);
        db.upsert_embeddings_batch(&[(Uuid::from_u128(1), vector.clone())])
            .await
            .unwrap();
        db.store_embedding_cache(&[(hash.clone(), vector.clone())])
            .await
            .unwrap();
    }
    assert_eq!(stamp(&path).as_deref(), Some(EMBEDDING_SPACE));

    // An index of the INT8 arithmetic has no stamp.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM metadata WHERE key = ?1", [EMBEDDING_SPACE_KEY])
        .unwrap();
    {
        let db = open(&path);
        assert!(!db.has_vectors().await.unwrap(), "its vectors are dropped");
        assert!(
            db.lookup_embedding_cache(std::slice::from_ref(&hash))
                .await
                .unwrap()
                .is_empty(),
            "and its cached vectors"
        );
        db.upsert_embeddings_batch(&[(Uuid::from_u128(2), vector)])
            .await
            .unwrap();
    }
    assert_eq!(stamp(&path).as_deref(), Some(EMBEDDING_SPACE));

    // Stamped, its new vectors stay.
    let db = open(&path);
    assert!(db.has_vectors().await.unwrap());
}
