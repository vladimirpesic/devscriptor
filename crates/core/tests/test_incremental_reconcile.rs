//! Incremental reconciliation: re-parsing a file must drop the entities that
//! vanished — and their embedding vectors — while keeping the surviving entities
//! and their vectors intact (the cleanup half of reliable incremental embedding,
//! built on content-stable entity ids).

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::EmbeddingSelection;
use devscriptor_core::parser::parse_content_sync;
use std::path::Path;

const DIM: usize = 768;

fn unit_vec() -> Vec<f32> {
    let v = 1.0 / (DIM as f32).sqrt();
    vec![v; DIM]
}

fn vector_count(db: &Database) -> i64 {
    db.conn_pool()
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM entity_vectors_rowids", [], |r| {
            r.get(0)
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn reconcile_drops_vanished_entities_and_their_vectors() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("recon.db");
    let db = Database::new(DatabaseConfig {
        path: db_path.to_string_lossy().to_string(),
        enable_vector: true,
        ..DatabaseConfig::default()
    })
    .expect("db");

    // Initial parse: a file with two functions → two entities with stable ids.
    let pf = parse_content_sync(
        "fn alpha() -> u8 { 1 }\nfn beta() -> u8 { 2 }\n",
        "rust",
        Path::new("lib.rs"),
    )
    .expect("parse");
    let alpha = pf
        .entities
        .iter()
        .find(|e| e.name() == "alpha")
        .map(|e| e.id())
        .expect("alpha");
    let beta = pf
        .entities
        .iter()
        .find(|e| e.name() == "beta")
        .map(|e| e.id())
        .expect("beta");
    assert_ne!(alpha, beta);

    db.batch_insert_parsed_files(&[pf.to_record()], None)
        .await
        .expect("insert");

    // Embed both.
    db.upsert_embeddings_batch(&[(alpha, unit_vec()), (beta, unit_vec())])
        .await
        .expect("embed");
    let (_, total) = db
        .get_unembedded_entity_ids(false, EmbeddingSelection::Standard)
        .await
        .unwrap();
    assert_eq!(total, 2, "two entities indexed");
    assert_eq!(vector_count(&db), 2, "both embedded");

    // Re-parse where `beta` was removed: reconcile keeping only `alpha`.
    db.reconcile_file_entities(&[("lib.rs".to_string(), vec![alpha])])
        .await
        .expect("reconcile");

    // `beta` (and its vector) gone; `alpha` (and its vector) intact.
    let (unembedded, total) = db
        .get_unembedded_entity_ids(false, EmbeddingSelection::Standard)
        .await
        .unwrap();
    assert_eq!(total, 1, "beta entity removed");
    assert!(
        unembedded.is_empty(),
        "alpha keeps its vector (no re-embed needed)"
    );
    assert_eq!(vector_count(&db), 1, "beta's vector cleaned up — no orphan");

    // The surviving entity is alpha.
    let conn = db.conn_pool().get().unwrap();
    let name: String = conn
        .query_row("SELECT name FROM entities", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "alpha");
}
