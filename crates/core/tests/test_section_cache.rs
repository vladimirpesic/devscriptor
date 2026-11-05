//! A Markdown section's embedded text names its file relative to the scanned
//! root, so a copy of a project checked out elsewhere finds its sections'
//! vectors in the embedding cache instead of embedding them again.

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    embeddings::{EmbeddingService, MockEmbeddingProvider},
    parser::ParserConfig,
    pipeline::{CodebasePipeline, PipelineConfig},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;

const GUIDE: &str = "# Install\n\nRun the installer, then sign in with your account.\n\n\
                     ## Upgrade\n\nRun the installer again; your settings stay as they were.\n";

fn database_config(path: &Path) -> DatabaseConfig {
    DatabaseConfig {
        path: path.to_string_lossy().into_owned(),
        cache_size: 512,
        ..DatabaseConfig::default()
    }
}

fn pipeline(path: &Path, db: &Arc<Database>) -> CodebasePipeline {
    let config = PipelineConfig {
        database: database_config(path),
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        use_gitignore: false,
        include_extensions: vec!["md".to_string()],
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    let svc = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));
    CodebasePipeline::with_database_and_service(config, Arc::clone(db), Some(svc))
        .expect("pipeline")
}

/// A project named `name` in `parent`, holding `docs/guide.md`.
fn project(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join("docs")).expect("create the project");
    std::fs::write(root.join("docs").join("guide.md"), GUIDE).expect("write the guide");
    dunce::canonicalize(root).expect("canonical root")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_elsewhere_finds_its_sections_in_the_cache() {
    let dir = TempDir::new().expect("tempdir");
    let index = dir.path().join("index.db");
    let db = Arc::new(Database::new(database_config(&index)).expect("open the index"));

    let first = pipeline(&index, &db)
        .analyze_directory(&project(dir.path(), "one"))
        .await
        .expect("scan the project");
    assert!(
        first.embedding_count > 0,
        "the guide's sections are embedded"
    );
    assert_eq!(first.embedding_cache_hits, 0, "nothing is cached yet");

    let copy = project(dir.path(), "copy");
    let second = pipeline(&index, &db)
        .analyze_directory(&copy)
        .await
        .expect("scan the copy");
    assert_eq!(second.embedding_count, first.embedding_count);
    assert_eq!(
        second.embedding_cache_hits, second.embedding_count,
        "every section of the copy comes from the cache"
    );

    // `devscriptor embed` builds the same texts as the scan.
    let file = copy.join("docs").join("guide.md");
    let ids: Vec<_> = db
        .get_entities_by_file(&file.to_string_lossy())
        .await
        .expect("the copy's entities")
        .iter()
        .filter(|entity| entity.entity_type() == "section")
        .map(|entity| entity.id())
        .collect();
    let hashes: Vec<String> = db
        .get_entity_texts_for_ids(&ids)
        .await
        .expect("texts")
        .iter()
        .map(|(_, text)| format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes())))
        .collect();
    assert_eq!(hashes.len(), second.embedding_count);
    assert_eq!(
        db.lookup_embedding_cache(&hashes)
            .await
            .expect("cache lookup")
            .len(),
        hashes.len(),
        "the texts `embed` builds are the ones cached"
    );
}
