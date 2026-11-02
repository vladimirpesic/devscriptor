//! Re-indexing drops an entity's vector exactly when its body changed.
//!
//! A vector embeds an entity's body: unchanged code in a changed file keeps its
//! vector, and is not embedded again, while an edited entity loses its vector,
//! which would describe code that no longer exists.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::MockEmbeddingProvider;
use devscriptor_core::embeddings::provider::EmbeddingProvider;
use devscriptor_core::entities::EntityId;
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use filetime::FileTime;
use tempfile::TempDir;

const SOURCE: &str = "def a():\n    return 1\n\n\ndef b():\n    return 2\n\n\n\
                      class C:\n    def m(self):\n        return 3\n";

fn pipeline(db: &Path, incremental: bool) -> CodebasePipeline {
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            cache_size: 512,
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        incremental,
        use_gitignore: false,
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config).expect("pipeline")
}

fn database(db: &Path) -> Database {
    Database::new(DatabaseConfig {
        path: db.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .expect("open the index")
}

/// Every entity of the index by id: name and body.
fn entities(db: &Database) -> HashMap<String, (String, String)> {
    let conn = db.conn_pool().get().expect("connection");
    let mut stmt = conn
        .prepare("SELECT id, name, body FROM entities")
        .expect("prepare");
    stmt.query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .expect("query entities")
        .collect::<rusqlite::Result<_>>()
        .expect("entity rows")
}

/// The ids of the entities that have a vector.
fn ids_with_vectors(db: &Database) -> HashSet<String> {
    let conn = db.conn_pool().get().expect("connection");
    let mut stmt = conn
        .prepare("SELECT entity_id FROM entity_vectors")
        .expect("prepare");
    stmt.query_map([], |row| row.get(0))
        .expect("query vectors")
        .collect::<rusqlite::Result<_>>()
        .expect("vector rows")
}

fn id_named(entities: &HashMap<String, (String, String)>, name: &str) -> String {
    entities
        .iter()
        .find(|(_, (n, _))| n == name)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("no entity named {name}"))
}

async fn run(incremental: bool) {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    let file = root.join("app.py");
    std::fs::write(&file, SOURCE).expect("write the source");
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    pipeline(&db, false)
        .analyze_directory(root)
        .await
        .expect("scan");

    // A vector for every entity.
    let store = database(&db);
    let before = entities(&store);
    let provider = MockEmbeddingProvider::new();
    let vectors: Vec<(EntityId, Vec<f32>)> = before
        .iter()
        .map(|(id, (_, body))| {
            (
                id.parse().expect("entity ids are UUIDs"),
                provider.generate_embedding(body).expect("mock embedding"),
            )
        })
        .collect();
    store
        .upsert_embeddings_batch(&vectors)
        .await
        .expect("store vectors");
    assert_eq!(ids_with_vectors(&store).len(), before.len());
    drop(store);

    // Change the body of `a`, and add a comment after the last definition.
    std::fs::write(
        &file,
        SOURCE.replacen("return 1", "return 10", 1) + "\n# a comment\n",
    )
    .expect("edit the source");
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_secs()
        + 60;
    filetime::set_file_mtime(
        &file,
        FileTime::from_unix_time(i64::try_from(later).expect("fits"), 0),
    )
    .expect("set mtime");
    pipeline(&db, incremental)
        .analyze_directory(root)
        .await
        .expect("re-index");

    let store = database(&db);
    let after = entities(&store);
    let with_vectors = ids_with_vectors(&store);
    assert!(
        !with_vectors.contains(&id_named(&after, "a")),
        "`a` changed and lost its vector"
    );
    assert!(
        with_vectors.contains(&id_named(&after, "b")),
        "`b` is unchanged and keeps its vector"
    );
    for (id, (name, body)) in &after {
        let unchanged = before.get(id).is_some_and(|(_, old)| old == body);
        assert_eq!(
            with_vectors.contains(id),
            unchanged,
            "{name}: a vector must remain exactly when the body is unchanged"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incremental_reindex_drops_only_the_vectors_of_changed_bodies() {
    run(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_reindex_drops_only_the_vectors_of_changed_bodies() {
    run(false).await;
}
