//! The freshness check answers from the index go through
//! ([`devscriptor_mcp::freshness::ensure_fresh`]): what it re-indexes, what it
//! reports, and what it leaves alone; answers computed again when a file they
//! read was re-indexed meanwhile ([`devscriptor_mcp::freshness::fresh_answer`]);
//! and `get_relationships`, whose answer names entities rather than files.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use devscriptor_mcp::freshness::{FreshnessReport, MAX_FILES, ensure_fresh, fresh_answer};
use devscriptor_mcp::{CoreModule, DevscriptorServerState, ToolResult};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A project scanned through `scan_codebase` into an index of its own, served
/// with a budget no re-index here exceeds.
struct Scanned {
    module: CoreModule,
    root: PathBuf,
    dir: TempDir,
}

impl Scanned {
    async fn new(files: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().join("project");
        for (path, content) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
            std::fs::write(path, content).expect("write");
        }
        let root = dunce::canonicalize(root).expect("canonical root");
        let state = DevscriptorServerState::with_db_path(dir.path().join("index.db"))
            .expect("server state")
            .with_freshness_budget(Duration::from_secs(60));
        let scanned = Self {
            module: CoreModule::new(Arc::new(state)),
            root,
            dir,
        };
        scanned
            .call("scan_codebase", json!({"path": scanned.root}))
            .await;
        scanned
    }

    fn state(&self) -> &DevscriptorServerState {
        self.module.state()
    }

    /// The path of `name` in the project, as the index stores it.
    fn file(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }

    async fn call(&self, tool: &str, arguments: Value) -> Value {
        match self.module.handle_tool(tool, Some(arguments)).await {
            Ok(ToolResult::Success(answer)) => answer,
            other => panic!("{tool} failed: {other:?}"),
        }
    }

    async fn manifest_mtime(&self, path: &str) -> u64 {
        self.state()
            .database()
            .get_scan_manifest_entries(&[path.to_string()])
            .await
            .expect("manifest")
            .first()
            .expect("a manifest row")
            .mtime_secs
    }
}

fn set_mtime(path: &str, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(time)
        .expect("set the modification time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_touched_but_unchanged_is_neither_refreshed_nor_stale() {
    let scanned = Scanned::new(&[("a.py", "def a():\n    return 1\n")]).await;
    let file = scanned.file("a.py");
    let later = SystemTime::now() + Duration::from_secs(120);
    set_mtime(&file, later);

    let report = ensure_fresh(scanned.state(), std::slice::from_ref(&file)).await;
    assert_eq!(report, FreshnessReport::default());
    // The manifest has the new time, so the next check takes the fast path.
    let seconds = later
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    assert_eq!(scanned.manifest_mtime(&file).await, seconds);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn past_the_file_limit_changed_files_are_stale() {
    let names: Vec<String> = (0..MAX_FILES + 2).map(|i| format!("f{i:02}.py")).collect();
    let contents: Vec<String> = (0..names.len())
        .map(|i| format!("def f{i:02}():\n    return {i}\n"))
        .collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .zip(&contents)
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    let scanned = Scanned::new(&files).await;
    let paths: Vec<String> = names.iter().map(|name| scanned.file(name)).collect();
    for (path, content) in paths.iter().zip(&contents) {
        std::fs::write(path, format!("{content}\n\ndef added():\n    return 0\n")).expect("edit");
    }

    let report = ensure_fresh(scanned.state(), &paths).await;
    assert_eq!(report.refreshed, paths[..MAX_FILES]);
    assert_eq!(report.stale, paths[MAX_FILES..]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_outside_every_scanned_root_is_never_reindexed() {
    let scanned = Scanned::new(&[("a.py", "def a():\n    return 1\n")]).await;
    // A file the index holds, but under no scanned root: indexed before
    // scans recorded their roots, say.
    let other = scanned.dir.path().join("other");
    std::fs::create_dir_all(&other).expect("mkdir");
    let other = dunce::canonicalize(other).expect("canonical");
    let file = other.join("old.py");
    std::fs::write(&file, "def old_name():\n    return 1\n").expect("write");
    CodebasePipeline::with_database(
        PipelineConfig {
            database: DatabaseConfig {
                path: scanned
                    .dir
                    .path()
                    .join("index.db")
                    .to_string_lossy()
                    .into_owned(),
                ..DatabaseConfig::default()
            },
            incremental: true,
            skip_embeddings: true,
            ..PipelineConfig::default()
        },
        scanned.state().database(),
    )
    .expect("pipeline")
    .reindex_paths(&other, std::slice::from_ref(&file))
    .await
    .expect("index the file");

    std::fs::write(&file, "def new_name():\n    return 22\n").expect("edit");
    let path = file.to_string_lossy().into_owned();
    let report = ensure_fresh(scanned.state(), std::slice::from_ref(&path)).await;
    assert_eq!(report.stale, [path]);
    assert!(report.refreshed.is_empty());
    let found = scanned
        .call("search_functions", json!({"query": "old_name"}))
        .await;
    assert_eq!(
        found["returned"], 1,
        "the old version is still indexed: {found}"
    );
    assert_eq!(found["stale_files"], json!([file]), "{found}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relationships_follow_the_callers_on_disk() {
    let scanned = Scanned::new(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "caller.py",
            "from lib import target\n\n\ndef caller():\n    return target()\n",
        ),
    ])
    .await;
    let mentions_caller = |answer: &Value| {
        answer["relationships"]
            .as_array()
            .expect("a list of relationships")
            .iter()
            .any(|edge| edge["source"] == "caller" || edge["target"] == "caller")
    };

    let answer = scanned
        .call("get_relationships", json!({"entity_name": "target"}))
        .await;
    assert!(mentions_caller(&answer), "caller calls target: {answer}");

    // The call goes; the answer about `target` follows the caller's file,
    // which it does not name.
    let caller = scanned.file("caller.py");
    std::fs::write(&caller, "def caller():\n    return 0\n").expect("edit");
    let answer = scanned
        .call("get_relationships", json!({"entity_name": "target"}))
        .await;
    assert!(
        !mentions_caller(&answer),
        "the removed call is gone: {answer}"
    );
    assert_eq!(answer["refreshed_files"], json!([caller]), "{answer}");
}

/// The index stores absolute paths only: a relative one names nothing to
/// check, and neither does a path that is neither indexed nor on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_and_unknown_paths_are_ignored() {
    let scanned = Scanned::new(&[("a.py", "def a():\n    return 1\n")]).await;
    std::fs::write(scanned.file("a.py"), "def a():\n    return 2\n\n").expect("edit");
    let missing = scanned.file("never_existed.py");
    let report = ensure_fresh(
        scanned.state(),
        &["a.py".to_string(), missing, String::new()],
    )
    .await;
    assert_eq!(report, FreshnessReport::default());
}

/// An answer that read a file just before another re-index of the file
/// committed is read again. The answer's own check comes after that re-index
/// and finds the file current, so without the rerun the answer would keep the
/// code the re-index replaced, as several agents asking one server at once
/// would see.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_read_during_a_reindex_of_its_file_is_read_again() {
    let scanned = Scanned::new(&[("a.py", "def a():\n    return 1\n")]).await;
    let file = scanned.file("a.py");
    std::fs::write(&file, "def a():\n    return 22\n").expect("edit");
    let state = scanned.state();
    let root = scanned.root.clone();

    let mut reads = 0;
    let (body, report) = fresh_answer(state, || {
        reads += 1;
        let first = reads == 1;
        let (file, root) = (file.clone(), root.clone());
        async move {
            let entity = state
                .database()
                .get_entities_by_exact_name("a", 10)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
                .ok_or("a is indexed")?;
            if first {
                // Another answer's check, or the watcher, re-indexes the file
                // after this answer read it and before its own check.
                state
                    .structural_pipeline()
                    .await
                    .map_err(|e| e.to_string())?
                    .reindex_paths(&root, &[PathBuf::from(&file)])
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok::<_, String>((entity.body().to_string(), vec![file]))
        }
    })
    .await
    .expect("an answer");
    assert_eq!(reads, 2, "read again after the other re-index");
    assert!(body.contains("return 22"), "{body}");
    assert_eq!(
        report,
        FreshnessReport::default(),
        "it re-indexed nothing itself"
    );
}

/// A file an answer names only once it is computed again is checked too: the
/// new answer can rank in files the first did not name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_first_named_by_a_rerun_are_checked_too() {
    let scanned = Scanned::new(&[
        ("a.py", "def a():\n    return 1\n"),
        ("b.py", "def b():\n    return 1\n"),
    ])
    .await;
    let (a, b) = (scanned.file("a.py"), scanned.file("b.py"));
    std::fs::write(&a, "def a():\n    return 2\n\n").expect("edit a.py");
    std::fs::write(&b, "def b():\n    return 2\n\n").expect("edit b.py");

    let mut reads = 0;
    let (_, report) = fresh_answer(scanned.state(), || {
        reads += 1;
        let files = if reads == 1 {
            vec![a.clone()]
        } else {
            vec![a.clone(), b.clone()]
        };
        async move { Ok::<_, ()>(((), files)) }
    })
    .await
    .expect("an answer");
    assert_eq!(report.refreshed, [a, b]);
    assert!(report.stale.is_empty(), "{report:?}");
    assert_eq!(reads, 3, "once for a.py, once more for b.py");
}
