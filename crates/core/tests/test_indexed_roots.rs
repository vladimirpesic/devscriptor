//! Every scan records its root, once, so a server started inside a scanned
//! directory can find the root to watch.

use std::path::{Component, Path, PathBuf};

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

async fn scan(db: &Path, root: &Path, incremental: bool) {
    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        incremental,
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config)
        .expect("pipeline")
        .analyze_directory(root)
        .await
        .expect("scan");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_scanned_root_is_recorded_once() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = dunce::canonicalize(tree.path()).expect("canonical root");
    std::fs::create_dir_all(root.join("service")).expect("mkdir");
    std::fs::write(root.join("service/app.py"), "def a():\n    return 1\n").expect("write");
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");

    scan(&db, &root.join("service"), false).await;
    scan(&db, &root, false).await;
    scan(&db, &root, true).await;
    // A relative spelling of a scanned root is the same root: where it has
    // one, which a root on another Windows drive does not.
    if let Some(relative) = relative_to_cwd(&root.join("service")) {
        scan(&db, &relative, true).await;
    }

    let database = devscriptor_core::database::Database::new(DatabaseConfig {
        path: db.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .expect("open the index");
    assert_eq!(
        database.indexed_roots().await.expect("read the roots"),
        vec![root.clone(), root.join("service")]
    );
}

/// `path` spelled relative to the current directory: `..` up to what the
/// two have in common, then the rest of `path`. A path on another Windows
/// drive than the current directory has no such spelling.
fn relative_to_cwd(path: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let (cwd, path): (Vec<_>, Vec<_>) = (cwd.components().collect(), path.components().collect());
    let root = |parts: &[Component]| {
        parts
            .iter()
            .take_while(|part| !matches!(part, Component::Normal(_)))
            .count()
    };
    if root(&cwd) != root(&path) || cwd[..root(&cwd)] != path[..root(&path)] {
        return None;
    }
    let common = cwd.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut relative: PathBuf =
        std::iter::repeat_n(Component::ParentDir, cwd.len() - common).collect();
    relative.extend(&path[common..]);
    Some(relative)
}
