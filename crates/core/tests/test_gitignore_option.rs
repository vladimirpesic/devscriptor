//! `PipelineConfig::use_gitignore` decides whether a scan applies git's ignore
//! files: with it off, a file a `.gitignore` excludes is indexed too.

use std::path::Path;
use std::sync::Arc;

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::{EmbeddingService, MockEmbeddingProvider};
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

/// A git project whose `.gitignore` excludes `generated.py`.
fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    // `.gitignore` applies inside a git repository only.
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "generated.py\n").unwrap();
    std::fs::write(dir.path().join("main.py"), "def main():\n    return 1\n").unwrap();
    std::fs::write(
        dir.path().join("generated.py"),
        "def generated():\n    return 2\n",
    )
    .unwrap();
    dir
}

/// Which of `main.py` and `generated.py` a full scan of `root` indexes, with
/// `use_gitignore` as given.
async fn indexed(root: &Path, use_gitignore: bool) -> (bool, bool) {
    let store = TempDir::new().unwrap();
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: store.path().join("index.db").to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        },
        parser: ParserConfig {
            enable_cache: false,
            ..ParserConfig::default()
        },
        use_gitignore,
        max_concurrency: 1,
        ..PipelineConfig::default()
    };
    let db = Arc::new(Database::new(config.database.clone()).unwrap());
    let service = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));
    let pipeline =
        CodebasePipeline::with_database_and_service(config, Arc::clone(&db), Some(service))
            .unwrap();
    pipeline.analyze_directory(root).await.unwrap();
    let is_indexed = |name: &str| {
        let path = root.join(name).to_string_lossy().into_owned();
        let db = Arc::clone(&db);
        async move { db.file_is_indexed(&path).await.unwrap() }
    };
    (
        is_indexed("main.py").await,
        is_indexed("generated.py").await,
    )
}

#[tokio::test]
async fn ignore_files_apply_only_with_use_gitignore() {
    let project = project();
    let root = dunce::canonicalize(project.path()).unwrap();
    assert_eq!(indexed(&root, true).await, (true, false));
    assert_eq!(indexed(&root, false).await, (true, true));
}
