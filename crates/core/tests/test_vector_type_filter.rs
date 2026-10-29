//! A type-filtered vector search must return the type, however rare it is near
//! the query.
//!
//! The vec0 table cannot filter by entity type inside its KNN, so the type
//! condition applies to the neighbours the KNN returns. Asked for only
//! `limit` neighbours, a type ranked behind closer entities of other types
//! would come back short or empty; the search takes the widest KNN instead,
//! and compares every vector of the type when that is still short.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    embeddings::EMBEDDING_DIMENSION,
    entities::{Entity, EntityId},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
};
use std::fmt::Write as _;
use std::sync::Arc;
use tempfile::TempDir;

/// A Python file with `functions` functions and two classes, scanned.
async fn indexed(functions: usize) -> (TempDir, Arc<Database>, Vec<Entity>) {
    let dir = TempDir::new().expect("tempdir");
    let mut source = String::new();
    for i in 0..functions {
        writeln!(source, "def f{i}():\n    return {i}\n").unwrap();
    }
    source.push_str("class Near:\n    pass\n\n\nclass Far:\n    pass\n");
    let file = dir.path().join("module.py");
    std::fs::write(&file, source).expect("write");

    let config = PipelineConfig {
        database: DatabaseConfig {
            path: dir.path().join("index.db").to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        incremental: false,
        enable_cross_file: false,
        use_gitignore: false,
        include_extensions: vec!["py".to_string()],
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    let db = Arc::new(Database::new(config.database.clone()).expect("db"));
    CodebasePipeline::with_database(config, Arc::clone(&db))
        .expect("pipeline")
        .analyze_directory(dir.path())
        .await
        .expect("scan");
    let entities = db
        // Under the file's real path, which the scan stores.
        .get_entities_by_file(&dunce::canonicalize(&file).unwrap().to_string_lossy())
        .await
        .expect("entities");
    (dir, db, entities)
}

/// A unit vector along `axis`, tilted towards axis 0 by `toward_query`.
fn vector(axis: usize, toward_query: f32) -> Vec<f32> {
    let mut v = vec![0.0; EMBEDDING_DIMENSION];
    v[axis] = 1.0;
    v[0] += toward_query;
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / norm).collect()
}

/// Store vectors that put every function nearer the query (axis 0) than either
/// class, and `Near` nearer than `Far`.
async fn store_vectors(db: &Database, entities: &[Entity]) {
    let mut items: Vec<(EntityId, Vec<f32>)> = Vec::new();
    for (i, entity) in entities.iter().enumerate() {
        let v = match entity.name() {
            "Near" => vector(1, 0.5),
            "Far" => vector(2, 0.1),
            _ => vector(3 + i % (EMBEDDING_DIMENSION - 3), 4.0),
        };
        items.push((entity.id(), v));
    }
    db.upsert_embeddings_batch(&items).await.expect("vectors");
}

fn names(results: &[(Entity, f32)]) -> Vec<&str> {
    results.iter().map(|(e, _)| e.name()).collect()
}

/// The classes rank behind every function: asking for two classes must not
/// return the two nearest neighbours, both functions, filtered to nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_type_behind_closer_neighbours_is_still_found() {
    let (_dir, db, entities) = indexed(20).await;
    store_vectors(&db, &entities).await;

    let results = db
        .vector_search_filtered(&vector(0, 0.0), 2, Some("class"))
        .await
        .expect("search");

    assert_eq!(names(&results), ["Near", "Far"]);
    assert!(results[0].1 < results[1].1, "ordered by distance");
}

/// Beyond the 4096 neighbours one KNN query may return, the classes are found
/// by the exact scan, which scores with the same cosine distance as the KNN.
#[tokio::test(flavor = "multi_thread")]
async fn a_type_beyond_the_knn_limit_is_found_by_the_exact_scan() {
    let (_dir, db, entities) = indexed(4200).await;
    store_vectors(&db, &entities).await;
    let query = vector(0, 0.0);

    let classes = db
        .vector_search_filtered(&query, 2, Some("class"))
        .await
        .expect("search");
    assert_eq!(names(&classes), ["Near", "Far"]);

    // The same distance the KNN reports for an entity it does return.
    let near = &classes[0];
    let expected = 1.0 - vector(1, 0.5)[0];
    assert!(
        (near.1 - expected).abs() < 1e-4,
        "exact-scan distance {} differs from the cosine distance {expected}",
        near.1
    );
    let functions = db
        .vector_search_filtered(&query, 1, Some("function"))
        .await
        .expect("search");
    let f = &functions[0];
    assert!((f.1 - (1.0 - vector(3, 4.0)[0])).abs() < 1e-4);
}
