//! The keyword half of the hybrid retriever must actually contribute.
//!
//! `retrieve` fuses a BM25 channel over FTS5 with a dense vector channel, and
//! the BM25 half is documented as first-class rather than a fallback. That
//! matters beyond ranking: it is the reason an entity kind can be left out of
//! embedding at all. If BM25 contributes nothing, then having no vector means
//! being absent from semantic search entirely.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
    search::CodebaseSearcher,
};
use std::sync::Arc;
use tempfile::TempDir;

async fn indexed_project() -> (TempDir, Arc<Database>) {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("thresholds.py"),
        "MAX_SAFE_CELSIUS_CENTI = 8500\n\n\ndef is_unsafe(centi):\n    \
         return centi > MAX_SAFE_CELSIUS_CENTI\n",
    )
    .expect("write");

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
    (dir, db)
}

/// With no embedding service wired up there is no dense channel at all, so
/// every result must come from BM25. An empty result set here means the keyword
/// half contributes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_keyword_channel_returns_results_on_its_own() {
    let (_dir, db) = indexed_project().await;
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    let results = searcher
        .semantic_search("MAX_SAFE_CELSIUS_CENTI", "", 10)
        .await
        .expect("search");

    let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"MAX_SAFE_CELSIUS_CENTI"),
        "the constant matches in FTS but never reached the caller: {names:?}"
    );
}

/// The entity kinds that carry no vector are exactly the ones this channel has
/// to reach, so filtering by such a type must still return them.
#[tokio::test(flavor = "multi_thread")]
async fn the_keyword_channel_honours_an_entity_type_filter() {
    let (_dir, db) = indexed_project().await;
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    let results = searcher
        .semantic_search("MAX_SAFE_CELSIUS_CENTI", "constant", 10)
        .await
        .expect("search");

    assert!(
        !results.is_empty(),
        "a type-filtered keyword search returned nothing"
    );
    assert!(
        results.iter().all(|r| r.entity_type == "constant"),
        "the filter leaked other types: {:?}",
        results.iter().map(|r| &r.entity_type).collect::<Vec<_>>()
    );
}

/// The real condition: a dense channel *is* active, and the entity the caller
/// wants has no vector. This is what happens to any kind excluded from
/// embedding, and it is the case the "BM25 is first-class" claim has to cover.
#[tokio::test(flavor = "multi_thread")]
async fn a_vectorless_entity_survives_fusion_with_an_active_dense_channel() {
    use devscriptor_core::embeddings::{EmbeddingService, MockEmbeddingProvider};

    let (_dir, db) = indexed_project().await;

    // Embed only the function, leaving the constant without a vector — exactly
    // the shape the embedding policy would produce.
    let svc = EmbeddingService::with_provider(MockEmbeddingProvider::new());
    let entities = db
        .get_entities_by_exact_name("is_unsafe", 10)
        .await
        .expect("entities");
    let function = entities
        .iter()
        .find(|e| e.entity_type() == "function")
        .expect("a function was indexed");
    let text = devscriptor_core::database::embeddings::entity_to_text(function);
    let vector = svc.generate_embedding(&text).expect("embed");
    db.upsert_embeddings_batch(&[(function.id(), vector)])
        .await
        .expect("store vector");

    let searcher = CodebaseSearcher::with_embedding_service(db.conn_pool().clone(), svc);
    let results = searcher
        .semantic_search("MAX_SAFE_CELSIUS_CENTI", "", 10)
        .await
        .expect("search");

    let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"MAX_SAFE_CELSIUS_CENTI"),
        "an entity with no vector vanished once the dense channel was active: {names:?}"
    );
}
