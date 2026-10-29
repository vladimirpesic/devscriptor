//! The watcher watches only what a scan reads, follows new directories,
//! re-indexes the changed paths together, and polls where events fail.
//!
//! It watches each directory a scan descends into, not the root recursively:
//! a recursive watch adds one for every directory, `target/` and
//! `node_modules/` included, and can exhaust `fs.inotify.max_user_watches`.
//! A change re-indexes the changed paths, not the tree.
//!
//! These tests pass in either mode: where the system's inotify watches are
//! used up, the watcher polls, as designed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig, PipelineResult};
use devscriptor_core::watcher::{CodeWatcher, WatchConfig, WatchMode};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn pipeline(db: &Path) -> Arc<CodebasePipeline> {
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
        include_extensions: vec!["py".to_string(), "rs".to_string(), "js".to_string()],
        skip_embeddings: true,
        max_concurrency: 2,
        ..PipelineConfig::default()
    };
    Arc::new(CodebasePipeline::new(config).expect("pipeline"))
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

/// Whether the index holds an entity named `name`.
fn indexed(db: &Path, name: &str) -> bool {
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM entities WHERE name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .is_ok_and(|count| count > 0)
}

/// Wait up to `limit` for `name` to be indexed.
async fn wait_indexed(db: &Path, name: &str, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if indexed(db, name) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    indexed(db, name)
}

/// A tree, indexed, with a watch session running on it. The re-index results
/// arrive on the returned channel.
struct Watched {
    _tree: TempDir,
    _index: TempDir,
    root: PathBuf,
    db: PathBuf,
    results: mpsc::UnboundedReceiver<PipelineResult>,
    mode: WatchMode,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn watched(config: WatchConfig) -> Watched {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = dunce::canonicalize(tree.path()).expect("canonical root");
    write(&root, "src/app.py", "def existing():\n    return 1\n");
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    let pipeline = pipeline(&db);

    let session = CodeWatcher::new(root.clone(), Arc::clone(&pipeline), config)
        .start()
        .expect("start watching");
    let mode = session.state().mode();
    pipeline.analyze_directory(&root).await.expect("prime");
    let (tx, results) = mpsc::unbounded_channel();
    let task = tokio::spawn(session.run(move |rescan| {
        let _ = tx.send(rescan.result);
    }));
    Watched {
        _tree: tree,
        _index: index,
        root,
        db,
        results,
        mode,
        task,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignored_directories_get_no_watch() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = dunce::canonicalize(tree.path()).expect("canonical root");
    write(&root, "src/app.py", "def a():\n    return 1\n");
    write(&root, "node_modules/x/index.js", "function b() {}\n");
    write(&root, "target/y/gen.rs", "fn c() {}\n");
    write(&root, ".git/objects/pack/x.rs", "fn d() {}\n");
    let index = TempDir::new().expect("tempdir for the index");

    let session = CodeWatcher::new(
        root.clone(),
        pipeline(&index.path().join("index.db")),
        WatchConfig::default(),
    )
    .start()
    .expect("start watching");
    let watched = session.watched_directories();

    assert!(watched.contains(&root), "{watched:?}");
    assert!(watched.contains(&root.join("src")), "{watched:?}");
    for ignored in ["node_modules", "target", ".git"] {
        assert!(
            !watched
                .iter()
                .any(|dir| dir.starts_with(root.join(ignored))),
            "{ignored} is watched: {watched:?}"
        );
    }
    assert_eq!(session.state().watched_directories(), watched.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_directory_created_after_start_is_watched_and_indexed() {
    let watched = watched(WatchConfig {
        debounce: Duration::from_millis(300),
        poll_interval: Duration::from_millis(300),
        ..WatchConfig::default()
    })
    .await;

    write(
        &watched.root,
        "pkg/new.py",
        "def created_later():\n    return 2\n",
    );
    assert!(
        wait_indexed(&watched.db, "created_later", Duration::from_secs(10)).await,
        "a file in a new directory was not indexed ({:?} mode)",
        watched.mode
    );

    // And the new directory is watched: a file written there later is too.
    tokio::time::sleep(Duration::from_millis(500)).await;
    write(
        &watched.root,
        "pkg/later.py",
        "def written_after():\n    return 3\n",
    );
    assert!(
        wait_indexed(&watched.db, "written_after", Duration::from_secs(10)).await,
        "a later file in the new directory was not indexed ({:?} mode)",
        watched.mode
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forced_poll_indexes_a_new_file() {
    let watched = watched(WatchConfig {
        force_poll: true,
        ..WatchConfig::default()
    })
    .await;
    assert_eq!(watched.mode, WatchMode::Poll);

    write(
        &watched.root,
        "src/polled.py",
        "def found_by_polling():\n    return 4\n",
    );
    assert!(wait_indexed(&watched.db, "found_by_polling", Duration::from_secs(5)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_edits_in_one_window_are_reindexed_together() {
    // A debounce longer than the poll interval, so that in polling mode both
    // files land in one window even if a poll falls between the writes.
    let mut watched = watched(WatchConfig {
        debounce: Duration::from_secs(2),
        poll_interval: Duration::from_millis(500),
        ..WatchConfig::default()
    })
    .await;

    write(&watched.root, "src/one.py", "def first():\n    return 1\n");
    write(&watched.root, "src/two.py", "def second():\n    return 2\n");
    let result = tokio::time::timeout(Duration::from_secs(10), watched.results.recv())
        .await
        .expect("a re-index")
        .expect("a result");
    assert_eq!(result.files_parsed, 2, "{result:?}");
    assert!(indexed(&watched.db, "first") && indexed(&watched.db, "second"));

    // Nothing else followed.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(watched.results.try_recv().is_err());
}
