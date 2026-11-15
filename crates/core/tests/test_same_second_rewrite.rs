//! A file written again within the second its indexed version was written,
//! at the same size, is re-indexed all the same.
//!
//! The manifest records a file's size and modification time in whole seconds,
//! so such a write leaves both as they were. A row read within
//! `RACY_WINDOW_MS` of its file's modification time does not vouch for the
//! file: its file is hashed again, and re-parsed if the content changed. Two
//! `git checkout`s a fraction of a second apart do exactly this to a file
//! whose branches differ in content of the same length.
//!
//! Each run also tells who follows the files on disk what it found created,
//! changed or deleted (`Database::on_file_changes`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use devscriptor_core::database::operations::{RACY_WINDOW_MS, settled};
use devscriptor_core::database::{DatabaseConfig, FileChange, FileChangeKind};
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

fn pipeline(db: &Path) -> CodebasePipeline {
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
        incremental: true,
        enable_cross_file: true,
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        max_concurrency: 2,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config).expect("pipeline")
}

/// A project and its index.
struct Indexed {
    _tree: TempDir,
    _index: TempDir,
    root: std::path::PathBuf,
    db: std::path::PathBuf,
}

impl Indexed {
    fn new(files: &[(&str, &str)]) -> Self {
        let tree = TempDir::new().expect("tempdir for the tree");
        let root = dunce::canonicalize(tree.path()).expect("canonical root");
        for (name, text) in files {
            std::fs::write(root.join(name), text).expect("write");
        }
        let index = TempDir::new().expect("tempdir for the index");
        let db = index.path().join("index.db");
        Self {
            _tree: tree,
            _index: index,
            root,
            db,
        }
    }

    fn names(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(&self.db).expect("open the index");
        let mut stmt = conn
            .prepare("SELECT name FROM entities WHERE type = 'function' ORDER BY name")
            .expect("prepare");
        stmt.query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("names")
    }
}

/// Write `text` to `path` and give it back the modification time it had: a
/// second write within the same second as the first.
fn rewrite_in_the_same_second(path: &Path, text: &str) {
    let before = std::fs::metadata(path).expect("metadata");
    let modified = before.modified().expect("mtime");
    assert_eq!(
        before.len(),
        text.len() as u64,
        "the rewrite keeps the size"
    );
    std::fs::write(path, text).expect("rewrite");
    set_mtime(path, modified);
}

fn set_mtime(path: &Path, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(time)
        .expect("set the modification time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reindex_of_the_path_finds_the_rewrite() {
    let indexed = Indexed::new(&[("racy.py", "def alpha_v1():\n    return 1\n")]);
    let pipeline = pipeline(&indexed.db);
    let file = indexed.root.join("racy.py");
    pipeline
        .reindex_paths(&indexed.root, std::slice::from_ref(&file))
        .await
        .expect("index the file");
    assert_eq!(indexed.names(), ["alpha_v1"]);

    rewrite_in_the_same_second(&file, "def alpha_v2():\n    return 1\n");
    let result = pipeline
        .reindex_paths(&indexed.root, std::slice::from_ref(&file))
        .await
        .expect("re-index the file");
    assert_eq!(result.files_parsed, 1, "{result:?}");
    assert_eq!(indexed.names(), ["alpha_v2"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incremental_scan_finds_the_rewrite() {
    let indexed = Indexed::new(&[("racy.py", "def alpha_v1():\n    return 1\n")]);
    pipeline(&indexed.db)
        .analyze_directory(&indexed.root)
        .await
        .expect("scan");
    assert_eq!(indexed.names(), ["alpha_v1"]);

    rewrite_in_the_same_second(
        &indexed.root.join("racy.py"),
        "def alpha_v2():\n    return 1\n",
    );
    pipeline(&indexed.db)
        .analyze_directory(&indexed.root)
        .await
        .expect("scan again");
    assert_eq!(indexed.names(), ["alpha_v2"]);
}

/// A file read long after its last write vouches for itself, and its row is
/// settled; one read just after a write is reported unsettled, for a check
/// once the window has passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_read_long_after_the_write_are_settled() {
    let indexed = Indexed::new(&[
        ("old.py", "def old():\n    return 1\n"),
        ("new.py", "def new():\n    return 2\n"),
    ]);
    set_mtime(
        &indexed.root.join("old.py"),
        SystemTime::now() - Duration::from_secs(3600),
    );
    let result = pipeline(&indexed.db)
        .analyze_directory(&indexed.root)
        .await
        .expect("scan");
    assert_eq!(result.unsettled, [indexed.root.join("new.py")]);

    // The rule itself, at the edge of the window.
    assert!(!settled(100, 100_000 + RACY_WINDOW_MS - 1));
    assert!(settled(100, 100_000 + RACY_WINDOW_MS));
}

/// Each run announces what it found on disk: a file new to the index is
/// created, one with other content changed, one gone deleted. A file touched
/// but unchanged is not announced, and neither is a fresh index's every file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_run_announces_the_changes_it_found() {
    let indexed = Indexed::new(&[
        ("edited.py", "def edited():\n    return 1\n"),
        ("gone.py", "def gone():\n    return 1\n"),
        ("touched.py", "def touched():\n    return 1\n"),
    ]);
    let pipeline = Arc::new(pipeline(&indexed.db));
    let heard: Arc<parking_lot::Mutex<Vec<FileChange>>> = Arc::default();
    let listener = Arc::clone(&heard);
    pipeline
        .db()
        .on_file_changes(move |changes| listener.lock().extend_from_slice(changes));
    pipeline
        .analyze_directory(&indexed.root)
        .await
        .expect("scan");
    assert!(heard.lock().is_empty(), "a fresh index: {:?}", heard.lock());

    let path = |name: &str| indexed.root.join(name);
    std::fs::write(path("edited.py"), "def edited():\n    return 2 + 2\n").expect("edit");
    std::fs::remove_file(path("gone.py")).expect("delete");
    std::fs::write(path("added.py"), "def added():\n    return 3\n").expect("add");
    set_mtime(
        &path("touched.py"),
        SystemTime::now() + Duration::from_secs(60),
    );
    let all: Vec<_> = ["edited.py", "gone.py", "added.py", "touched.py"]
        .iter()
        .map(|name| path(name))
        .collect();
    pipeline
        .reindex_paths(&indexed.root, &all)
        .await
        .expect("re-index the paths");

    let mut changes: Vec<(String, FileChangeKind)> = heard
        .lock()
        .iter()
        .map(|change| (change.path.clone(), change.kind))
        .collect();
    changes.sort_by(|(a, _), (b, _)| a.cmp(b));
    let at = |name: &str| path(name).to_string_lossy().into_owned();
    assert_eq!(
        changes,
        [
            (at("added.py"), FileChangeKind::Created),
            (at("edited.py"), FileChangeKind::Changed),
            (at("gone.py"), FileChangeKind::Deleted),
        ]
    );

    // A full re-scan announces what changed since, and only that.
    heard.lock().clear();
    std::fs::write(path("added.py"), "def added():\n    return 4 + 4\n").expect("edit");
    pipeline
        .analyze_directory(&indexed.root)
        .await
        .expect("scan again");
    let heard = heard.lock();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].path, at("added.py"));
    assert_eq!(heard[0].kind, FileChangeKind::Changed);
}

/// A database whose manifest predates `read_at_ms` gains the column, and
/// keeps its rows: they count as unsettled, so their files are hashed once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manifest_without_read_times_gains_them() {
    let indexed = Indexed::new(&[("a.py", "def a():\n    return 1\n")]);
    pipeline(&indexed.db)
        .analyze_directory(&indexed.root)
        .await
        .expect("scan");
    {
        let conn = rusqlite::Connection::open(&indexed.db).expect("open");
        conn.execute_batch("ALTER TABLE scan_manifest DROP COLUMN read_at_ms")
            .expect("drop the column");
    }
    let pipeline = pipeline(&indexed.db);
    let rows = pipeline.db().load_scan_manifest().await.expect("manifest");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].read_at_ms, 0);
    assert!(!rows[0].settled());

    let result = pipeline
        .analyze_directory(&indexed.root)
        .await
        .expect("scan again");
    assert_eq!(
        (result.files_parsed, result.files_skipped),
        (0, 1),
        "{result:?}"
    );
}
