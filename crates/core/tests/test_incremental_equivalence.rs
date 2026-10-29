//! A re-index must leave exactly the graph that a fresh scan of the same tree
//! builds.
//!
//! A re-index keeps the edges into an edited file from unchanged files,
//! resolves its references against the whole index rather than the files of
//! the run, and removes the edges of a call the edit deleted, so its graph
//! equals a fresh scan's in incremental runs, full re-scans and re-indexes of
//! given paths alike.
//!
//! Each scenario edits the tree, re-indexes the existing database, and compares
//! its entities and edges with those of a fresh scan into a new database. The
//! scenarios run in order on one database, once incrementally, once as full
//! re-scans and once as re-indexes of just the paths the edit touched (what
//! the watcher does), and every scenario is checked even after one has failed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::graph::resolve::{Resolution, rebuild_edges_for_sources};
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::parser::symbol_table::DEFAULT_AMBIGUOUS_FANOUT;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use filetime::FileTime;
use tempfile::TempDir;

/// An entity keyed as any scan of the same tree keys it: path relative to the
/// root, type, name, first and last line.
type EntityKey = (String, String, String, u32, u32);

/// A relationship by its endpoints and kind.
type EdgeKey = (EntityKey, String, EntityKey);

#[derive(Debug, PartialEq, Eq)]
struct Graph {
    files: BTreeSet<String>,
    entities: BTreeSet<EntityKey>,
    edges: BTreeSet<EdgeKey>,
}

/// Python and Rust files with calls and type uses across files, a second caller
/// of the same callee, a class inheriting a class from another file, and a
/// method.
const FIXTURE: &[(&str, &str)] = &[
    (
        "app/geometry.py",
        "\"\"\"Shapes and scaling.\"\"\"\n\n\nclass Shape:\n    def area(self):\n        return 0\n\n\n\
         def scale(value):\n    return value * 2\n",
    ),
    (
        "app/report.py",
        "\"\"\"Reports built on geometry.\"\"\"\nfrom geometry import Shape, scale\n\n\n\
         class Square(Shape):\n    def area(self):\n        return scale(3)\n\n\n\
         def summary():\n    return scale(1) + total()\n\n\n\
         def total():\n    return 7\n",
    ),
    (
        "app/caller.py",
        "\"\"\"A second caller of scale.\"\"\"\nfrom geometry import scale\n\n\n\
         def second_caller():\n    return scale(5)\n",
    ),
    (
        "core/money.rs",
        "//! Money in integer cents.\n\npub struct Money {\n    pub cents: u64,\n}\n\n\
         pub fn add(a: Money, b: Money) -> Money {\n    Money {\n        cents: a.cents + b.cents,\n    }\n}\n",
    ),
    (
        "core/ops.rs",
        "//! Operations on money.\n\npub fn double(m: Money) -> Money {\n    add(m, Money { cents: m.cents })\n}\n",
    ),
];

fn write_fixture(root: &Path) {
    for (path, content) in FIXTURE {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("fixture files live in a directory"))
            .expect("create fixture directory");
        std::fs::write(path, content).expect("write fixture file");
    }
}

/// Rewrite `path` with `edit` and give it a modification time no earlier scan
/// can have recorded, so the incremental fast path (same size and mtime) never
/// mistakes the edit for an unchanged file.
fn edit(root: &Path, path: &str, edit: impl FnOnce(String) -> String, stamp: i64) {
    let path = root.join(path);
    let before = std::fs::read_to_string(&path).expect("read file to edit");
    let after = edit(before.clone());
    assert_ne!(
        before,
        after,
        "the edit of {} changed nothing",
        path.display()
    );
    std::fs::write(&path, after).expect("write edited file");
    stamp_file(&path, stamp);
}

fn create(root: &Path, path: &str, content: &str, stamp: i64) {
    let path = root.join(path);
    std::fs::write(&path, content).expect("write new file");
    stamp_file(&path, stamp);
}

fn stamp_file(path: &Path, stamp: i64) {
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_secs();
    let base = i64::try_from(base).expect("seconds since the epoch fit an i64");
    filetime::set_file_mtime(path, FileTime::from_unix_time(base + 60 * stamp, 0))
        .expect("set mtime");
}

fn replace(from: &'static str, to: &'static str) -> impl FnOnce(String) -> String {
    move |s: String| {
        assert!(s.contains(from), "`{from}` not found");
        s.replacen(from, to, 1)
    }
}

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
        enable_cross_file: true,
        use_gitignore: false,
        include_extensions: vec!["py".to_string(), "rs".to_string()],
        skip_embeddings: true,
        max_concurrency: 2,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config).expect("pipeline")
}

async fn reindex(db: &Path, root: &Path, incremental: bool) {
    pipeline(db, incremental)
        .analyze_directory(root)
        .await
        .expect("re-index");
}

/// Scan `root` into a new database and return its graph.
async fn fresh(root: &Path) -> Graph {
    let dir = TempDir::new().expect("tempdir for the fresh index");
    let db = dir.path().join("fresh.db");
    pipeline(&db, false)
        .analyze_directory(root)
        .await
        .expect("fresh scan");
    graph(&db, root)
}

/// Every file, entity and edge in the database at `db`, keyed relative to
/// `root`.
fn graph(db: &Path, root: &Path) -> Graph {
    let root = dunce::canonicalize(root).expect("canonical root");
    let relative = |path: &str| -> String {
        let path = PathBuf::from(path);
        path.strip_prefix(&root)
            .map_or_else(|_| path.clone(), Path::to_path_buf)
            .to_string_lossy()
            .into_owned()
    };
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the index");

    let mut file_rows = conn
        .prepare("SELECT path FROM files")
        .expect("prepare file query");
    let files = file_rows
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query files")
        .map(|path| relative(&path.expect("file row")))
        .collect();

    let mut entity_rows = conn
        .prepare("SELECT id, type, name, file_path, start_line, end_line FROM entities")
        .expect("prepare entity query");
    let mut keys = std::collections::HashMap::new();
    let mut entities = BTreeSet::new();
    let rows = entity_rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, u32>(5)?,
            ))
        })
        .expect("query entities");
    for row in rows {
        let (id, kind, name, path, start, end) = row.expect("entity row");
        let key = (relative(&path), kind, name, start, end);
        entities.insert(key.clone());
        keys.insert(id, key);
    }

    let mut edge_rows = conn
        .prepare("SELECT source_id, relationship_type, target_id FROM relationships")
        .expect("prepare edge query");
    let rows = edge_rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("query edges");
    let mut edges = BTreeSet::new();
    for row in rows {
        let (source, kind, target) = row.expect("edge row");
        let source = keys.get(&source).expect("edge source is an entity").clone();
        let target = keys.get(&target).expect("edge target is an entity").clone();
        edges.insert((source, kind, target));
    }
    Graph {
        files,
        entities,
        edges,
    }
}

/// What `got` lacks and what it has extra, compared with `want`, or `None`
/// when the graphs are equal.
fn difference(scenario: &str, got: &Graph, want: &Graph) -> Option<String> {
    if got == want {
        return None;
    }
    let mut report = format!("scenario \"{scenario}\":");
    for f in want.files.difference(&got.files) {
        report.push_str(&format!("\n    missing file   {f}"));
    }
    for f in got.files.difference(&want.files) {
        report.push_str(&format!("\n    extra file     {f}"));
    }
    for e in want.entities.difference(&got.entities) {
        report.push_str(&format!("\n    missing entity {e:?}"));
    }
    for e in got.entities.difference(&want.entities) {
        report.push_str(&format!("\n    extra entity   {e:?}"));
    }
    for e in want.edges.difference(&got.edges) {
        report.push_str(&format!("\n    missing edge   {e:?}"));
    }
    for e in got.edges.difference(&want.edges) {
        report.push_str(&format!("\n    extra edge     {e:?}"));
    }
    Some(report)
}

type Scenario = (&'static str, fn(&Path));

/// The edits, applied in order to one tree.
const SCENARIOS: &[Scenario] = &[
    ("a comment added to each callee's file", |root| {
        edit(root, "app/geometry.py", |s| s + "\n# a comment\n", 1);
        edit(root, "core/money.rs", |s| s + "\n// a comment\n", 1);
    }),
    ("a call removed from a caller", |root| {
        edit(
            root,
            "app/report.py",
            replace("return scale(1) + total()", "return total()"),
            2,
        );
    }),
    (
        "a call added from an edited file into an unchanged file",
        |root| {
            edit(
                root,
                "app/caller.py",
                |s| s + "\n\ndef third_caller():\n    return scale(9)\n",
                3,
            );
            edit(
                root,
                "core/ops.rs",
                |s| s + "\npub fn triple(m: Money) -> Money {\n    add(m, double(m))\n}\n",
                3,
            );
        },
    ),
    ("a function another file calls renamed", |root| {
        edit(
            root,
            "app/geometry.py",
            replace("def scale(value):", "def resize(value):"),
            4,
        );
    }),
    ("a file deleted", |root| {
        std::fs::remove_file(root.join("app/caller.py")).expect("delete a file");
    }),
    ("a new file calling existing functions", |root| {
        create(
            root,
            "app/extra.py",
            "from report import total\n\n\ndef extra():\n    return total() + total()\n",
            6,
        );
    }),
    ("a second definition of a name another file calls", |root| {
        edit(
            root,
            "core/money.rs",
            |s| {
                s + "\nimpl Money {\n    pub fn add(self, other: Money) -> Money {\n        \
                     Money {\n            cents: self.cents + other.cents,\n        }\n    }\n}\n"
            },
            7,
        );
    }),
];

/// Apply every scenario to one tree, re-index after each, and compare with a
/// fresh scan.
async fn run_sequence(incremental: bool) {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    write_fixture(root);
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    reindex(&db, root, false).await;

    let initial = graph(&db, root);
    assert!(
        initial
            .edges
            .iter()
            .any(|(source, kind, target)| kind == "calls" && source.0 != target.0),
        "the fixture must produce cross-file calls: {:#?}",
        initial.edges
    );

    let mut failures = Vec::new();
    for (scenario, apply) in SCENARIOS {
        apply(root);
        reindex(&db, root, incremental).await;
        let want = fresh(root).await;
        if let Some(report) = difference(scenario, &graph(&db, root), &want) {
            failures.push(report);
        }
    }
    let mode = if incremental { "incremental" } else { "full" };
    assert!(
        failures.is_empty(),
        "{} {mode} re-index(es) differ from a fresh scan:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incremental_reindex_matches_a_fresh_scan() {
    run_sequence(true).await;
}

/// Every file under `root` with its content, to tell which paths an edit
/// touched.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read a directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let content = std::fs::read(&path).expect("read a file");
                files.insert(path, content);
            }
        }
    }
    files
}

/// The paths created, changed or deleted between two snapshots.
fn touched(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The scenarios once more, each followed by a re-index of only the paths it
/// touched, through one pipeline, so the entities it keeps between runs are
/// exercised too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reindex_of_the_touched_paths_matches_a_fresh_scan() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    write_fixture(root);
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    reindex(&db, root, false).await;
    let watcher = pipeline(&db, true);

    let mut failures = Vec::new();
    for (scenario, apply) in SCENARIOS {
        let before = snapshot(root);
        apply(root);
        let paths = touched(&before, &snapshot(root));
        assert!(!paths.is_empty(), "{scenario} touched nothing");
        watcher
            .reindex_paths(root, &paths)
            .await
            .expect("re-index the touched paths");
        let want = fresh(root).await;
        if let Some(report) = difference(scenario, &graph(&db, root), &want) {
            failures.push(report);
        }
    }
    assert!(
        failures.is_empty(),
        "{} path re-index(es) differ from a fresh scan:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Paths a scan of the root would not index are not indexed by a re-index of
/// paths either: a dependency directory, a hidden one, another extension.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reindex_of_paths_skips_what_a_scan_skips() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    write_fixture(root);
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    reindex(&db, root, false).await;
    let before = graph(&db, root);

    let skipped = [
        "node_modules/pkg/vendored.py",
        ".cache/generated.py",
        "app/notes.txt",
    ];
    for path in skipped {
        let dir = root.join(path).parent().expect("a directory").to_path_buf();
        std::fs::create_dir_all(dir).expect("mkdir");
        create(root, path, "def vendored():\n    return 1\n", 7);
    }
    let result = pipeline(&db, true)
        .reindex_paths(
            root,
            &skipped
                .iter()
                .map(|path| root.join(path))
                .collect::<Vec<_>>(),
        )
        .await
        .expect("re-index the paths");
    assert_eq!(result.files_parsed, 0);
    assert_eq!(graph(&db, root), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_reindex_matches_a_fresh_scan() {
    run_sequence(false).await;
}

/// How a test re-indexes the tree after an edit.
#[derive(Debug, Clone, Copy)]
enum Reindex {
    /// An incremental scan of the tree.
    Incremental,
    /// A full scan of the tree.
    Full,
    /// A re-index of the paths the edit touched, as the watcher does.
    Paths,
}

/// Apply `apply` to the tree at `root` and re-index the database at `db` the
/// way `how` says.
async fn edit_and_reindex(db: &Path, root: &Path, how: Reindex, apply: impl FnOnce(&Path)) {
    reindex_after(pipeline, db, root, how, apply).await;
}

/// [`edit_and_reindex`] through the pipelines `make` builds.
async fn reindex_after(
    make: fn(&Path, bool) -> CodebasePipeline,
    db: &Path,
    root: &Path,
    how: Reindex,
    apply: impl FnOnce(&Path),
) {
    let before = snapshot(root);
    apply(root);
    let pipeline = make(db, !matches!(how, Reindex::Full));
    match how {
        Reindex::Incremental | Reindex::Full => {
            pipeline.analyze_directory(root).await.expect("re-index");
        }
        Reindex::Paths => {
            pipeline
                .reindex_paths(root, &touched(&before, &snapshot(root)))
                .await
                .expect("re-index the touched paths");
        }
    }
}

/// Whether `graph` has a call edge from the entity named `caller` to the
/// one named `callee`.
fn calls(graph: &Graph, caller: &str, callee: &str) -> bool {
    graph
        .edges
        .iter()
        .any(|(source, kind, target)| kind == "calls" && source.2 == caller && target.2 == callee)
}

/// A definition added to one file must be linked from unchanged files that
/// already called it by name, directly or through an import alias: a fresh
/// scan links them, so a re-index in any mode must too. Re-resolving the
/// callers of a name another file added needs stored references.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_definition_is_linked_from_unchanged_callers() {
    let mut failures = Vec::new();
    for how in [Reindex::Incremental, Reindex::Full, Reindex::Paths] {
        let tree = TempDir::new().expect("tempdir for the tree");
        let root = tree.path();
        write_fixture(root);
        edit(
            root,
            "app/report.py",
            replace(
                "return scale(1) + total()",
                "return scale(1) + total() + helper()",
            ),
            1,
        );
        create(
            root,
            "app/assist.py",
            "from geometry import helper as aid\n\n\ndef assisted():\n    return aid()\n",
            1,
        );
        let index = TempDir::new().expect("tempdir for the index");
        let db = index.path().join("index.db");
        reindex(&db, root, false).await;
        assert!(!calls(&graph(&db, root), "summary", "helper"));

        edit_and_reindex(&db, root, how, |root| {
            edit(
                root,
                "app/geometry.py",
                |s| s + "\n\ndef helper():\n    return 1\n",
                2,
            );
        })
        .await;

        let want = fresh(root).await;
        assert!(
            calls(&want, "summary", "helper") && calls(&want, "assisted", "helper"),
            "a fresh scan links both callers: {:#?}",
            want.edges
        );
        if let Some(report) = difference(
            &format!("a definition of an unresolved name added ({how:?})"),
            &graph(&db, root),
            &want,
        ) {
            failures.push(report);
        }
    }
    assert!(
        failures.is_empty(),
        "{} re-index(es) differ from a fresh scan:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Entities whose file lies under `root`, as `graph` keys them: relative paths.
fn entities_under(graph: &Graph, root: &Path) -> BTreeSet<EntityKey> {
    let root = dunce::canonicalize(root).expect("canonical root");
    graph
        .entities
        .iter()
        .filter(|(path, ..)| !Path::new(path).is_absolute() || Path::new(path).starts_with(&root))
        .cloned()
        .collect()
}

/// One database can hold several projects. Re-indexing one of them, in either
/// mode, must not touch the others, nor take their files for removed ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reindex_leaves_other_projects_in_the_database_alone() {
    let first = TempDir::new().expect("tempdir for the first project");
    let second = TempDir::new().expect("tempdir for the second project");
    write_fixture(first.path());
    write_fixture(second.path());
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    reindex(&db, first.path(), false).await;
    reindex(&db, second.path(), false).await;
    let before = graph(&db, second.path());
    let second_entities = entities_under(&before, second.path());
    assert!(!second_entities.is_empty(), "the second project is indexed");

    for incremental in [true, false] {
        reindex(&db, first.path(), incremental).await;
        assert_eq!(
            graph(&db, second.path()),
            before,
            "re-indexing the unchanged first project (incremental: {incremental}) changed the index"
        );
    }

    std::fs::remove_file(first.path().join("app/caller.py")).expect("delete a file");
    reindex(&db, first.path(), true).await;
    let after = graph(&db, second.path());
    assert_eq!(
        entities_under(&after, second.path()),
        second_entities,
        "deleting a file of the first project changed the second"
    );
    assert!(
        !after
            .entities
            .iter()
            .any(|(path, ..)| path.ends_with("app/caller.py") && Path::new(path).is_absolute()),
        "the deleted file of the first project is gone"
    );
}

/// The edges of `graph` (keyed relative to one project's root) that join an
/// entity of that project to an entity outside it.
fn edges_leaving(graph: &Graph) -> Vec<&EdgeKey> {
    graph
        .edges
        .iter()
        .filter(|(source, _, target)| {
            Path::new(&source.0).is_absolute() != Path::new(&target.0).is_absolute()
        })
        .collect()
}

/// A reference resolves within its own project, as a scan of the project
/// resolves it, whichever project of a shared database is re-indexed: one
/// project's call to a name only the other defines stays unresolved when
/// either is re-indexed, in any mode, even when the other's re-index changes
/// that name's definitions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reindex_resolves_references_within_their_project() {
    for how in [Reindex::Incremental, Reindex::Full, Reindex::Paths] {
        let first = TempDir::new().expect("tempdir for the first project");
        let second = TempDir::new().expect("tempdir for the second project");
        create(
            first.path(),
            "main.py",
            "def run():\n    return shared()\n",
            0,
        );
        create(
            second.path(),
            "lib.py",
            "def shared():\n    return 1\n\n\ndef user():\n    return shared()\n",
            0,
        );
        let index = TempDir::new().expect("tempdir for the index");
        let db = index.path().join("index.db");
        reindex(&db, first.path(), false).await;
        reindex(&db, second.path(), false).await;

        edit_and_reindex(&db, first.path(), how, |root| {
            edit(root, "main.py", |s| s + "\n# edited\n", 1);
        })
        .await;
        edit_and_reindex(&db, second.path(), how, |root| {
            edit(
                root,
                "lib.py",
                |s| s + "\n\ndef shared():\n    return 2\n",
                1,
            );
        })
        .await;

        let graph = graph(&db, first.path());
        assert!(
            edges_leaving(&graph).is_empty(),
            "{how:?}: edges between the projects: {:#?}",
            edges_leaving(&graph)
        );
        assert!(
            calls(&graph, "user", "shared"),
            "{how:?}: the second project's own call is linked"
        );
        assert!(
            !calls(&graph, "run", "shared"),
            "{how:?}: the first project's call stays unresolved"
        );
    }
}

/// Every ambiguous edge in the database at `db`, keyed as [`graph`] keys
/// edges, with how many candidates tied.
fn ambiguous_edges(db: &Path, root: &Path) -> BTreeMap<EdgeKey, u64> {
    let root = dunce::canonicalize(root).expect("canonical root");
    let relative = |path: String| -> String {
        let path = PathBuf::from(path);
        path.strip_prefix(&root)
            .map_or_else(|_| path.clone(), Path::to_path_buf)
            .to_string_lossy()
            .into_owned()
    };
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.file_path, s.type, s.name, s.start_line, s.end_line, r.relationship_type, \
                    t.file_path, t.type, t.name, t.start_line, t.end_line, r.metadata \
             FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| {
            Ok((
                (
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                ),
                row.get::<_, String>(5)?,
                (
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?,
                ),
                row.get::<_, Option<String>>(11)?,
            ))
        })
        .expect("query edges");
    let mut ambiguous = BTreeMap::new();
    for row in rows {
        let (source, kind, target, metadata) = row.expect("edge row");
        let candidates = metadata
            .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
            .filter(|m| m["ambiguous"] == true)
            .and_then(|m| m["candidates"].as_u64());
        if let Some(candidates) = candidates {
            let key =
                |(path, entity_type, name, start, end): (String, String, String, u32, u32)| {
                    (relative(path), entity_type, name, start, end)
                };
            ambiguous.insert((key(source), kind, key(target)), candidates);
        }
    }
    ambiguous
}

/// The golden corpus: a file in each language the parser knows.
fn golden_corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_files")
}

/// A pipeline over the database at `db` that indexes every language.
fn corpus_pipeline(db: &Path, incremental: bool) -> CodebasePipeline {
    CodebasePipeline::new(PipelineConfig {
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
        enable_cross_file: true,
        use_gitignore: false,
        skip_embeddings: true,
        max_concurrency: 2,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
}

/// The graph and the ambiguous edges of a fresh scan of `root`, every
/// language included.
async fn fresh_corpus(root: &Path) -> (Graph, BTreeMap<EdgeKey, u64>) {
    let dir = TempDir::new().expect("tempdir for the fresh index");
    let db = dir.path().join("fresh.db");
    corpus_pipeline(&db, false)
        .analyze_directory(root)
        .await
        .expect("fresh scan");
    (graph(&db, root), ambiguous_edges(&db, root))
}

/// The golden corpus, indexed while a third of its files at a time goes and
/// comes back: after each change an incremental re-index, of the tree or of
/// the paths that changed, leaves the graph a fresh scan of the tree builds,
/// ambiguity included. Across its languages, a file that goes takes the
/// definitions references elsewhere resolved to, and one that comes back
/// gives them a definition again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_incremental_reindex_of_the_golden_corpus_matches_a_fresh_scan() {
    let corpus = golden_corpus();
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(&corpus)
        .expect("read the corpus")
        .map(|entry| entry.expect("a corpus entry").file_name())
        .collect();
    names.sort();
    let third = |k: usize| -> Vec<std::ffi::OsString> {
        names
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 3 == k)
            .map(|(_, name)| name.clone())
            .collect()
    };
    // (step, files, whether they come back)
    let steps = [
        ("the first third comes back", third(0), true),
        ("the second third goes", third(1), false),
        ("the second third comes back", third(1), true),
    ];

    let mut failures = Vec::new();
    for how in [Reindex::Incremental, Reindex::Paths] {
        let tree = TempDir::new().expect("tempdir for the tree");
        let root = tree.path();
        for name in &names {
            std::fs::copy(corpus.join(name), root.join(name)).expect("copy a corpus file");
        }
        for name in third(0) {
            std::fs::remove_file(root.join(name)).expect("remove a corpus file");
        }
        let index = TempDir::new().expect("tempdir for the index");
        let db = index.path().join("index.db");
        corpus_pipeline(&db, false)
            .analyze_directory(root)
            .await
            .expect("scan the corpus");

        for (step, files, back) in &steps {
            reindex_after(corpus_pipeline, &db, root, how, |root| {
                for name in files {
                    if *back {
                        std::fs::copy(corpus.join(name), root.join(name))
                            .expect("bring a corpus file back");
                    } else {
                        std::fs::remove_file(root.join(name)).expect("remove a corpus file");
                    }
                }
            })
            .await;
            let (want, want_ambiguous) = fresh_corpus(root).await;
            assert!(
                want.edges.len() > 100,
                "the corpus has edges to compare: {}",
                want.edges.len()
            );
            let scenario = format!("{step} ({how:?})");
            if let Some(report) = difference(&scenario, &graph(&db, root), &want) {
                failures.push(report);
            }
            if ambiguous_edges(&db, root) != want_ambiguous {
                failures.push(format!("scenario \"{scenario}\": other ambiguous edges"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} re-index(es) of the golden corpus differ from a fresh scan:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The ids of every entity in the database at `db`.
fn entity_ids(db: &Path) -> Vec<devscriptor_core::entities::EntityId> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the index");
    let mut stmt = conn.prepare("SELECT id FROM entities").expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query ids")
        .map(|id| uuid::Uuid::parse_str(&id.expect("id row")).expect("a uuid"))
        .collect()
}

/// More entities than one chunk of a derivation holds, and more references
/// than one write of its edges: modules whose functions each call eight
/// functions of the next module, a `helper` each defines, and `shared_*`
/// functions a fifth of them define, so calls to those are ambiguous.
fn write_large_tree(root: &Path, modules: usize, functions: usize) {
    for module in 0..modules {
        let next = (module + 1) % modules;
        let mut source = String::from("def helper():\n    return 0\n\n\n");
        for function in 0..functions {
            let calls: Vec<String> = (0..8)
                .map(|offset| format!("f{next}_{}()", (function + offset) % functions))
                .collect();
            source.push_str(&format!(
                "def f{module}_{function}():\n    return {} + helper() + shared_{}()\n\n\n",
                calls.join(" + "),
                function % 7
            ));
        }
        if module % 5 == 0 {
            for shared in 0..7 {
                source.push_str(&format!(
                    "def shared_{shared}():\n    return {shared}\n\n\n"
                ));
            }
        }
        std::fs::write(root.join(format!("module_{module:03}.py")), source)
            .expect("write a module");
    }
}

/// A scan derives its edges a chunk of entities at a time, keeps the
/// definitions one chunk read for the next, and writes a chunk's edges in
/// groups of sources. Deriving every entity's edges again, in separate calls
/// that each read their definitions afresh and write one group, and then in one
/// call over all of them, gives the same graph each time, ambiguity included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edges_derived_in_separate_calls_equal_a_scans() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = tree.path();
    write_large_tree(root, 40, 130);
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    reindex(&db, root, false).await;
    let ids = entity_ids(&db);
    assert!(ids.len() > 5000, "more than one chunk: {}", ids.len());
    let scanned = (graph(&db, root), ambiguous_edges(&db, root));
    // Every function calls eight of the next module's and its own `helper`;
    // `shared_*` is its own module's where it defines one (8 of the 40
    // modules), else three of the eight definitions.
    let calls = scanned
        .0
        .edges
        .iter()
        .filter(|(_, kind, _)| kind == "calls");
    assert_eq!(calls.count(), 40 * 130 * 9 + 8 * 130 + 32 * 130 * 3);
    assert!(
        !scanned.1.is_empty(),
        "the tree has ambiguous calls to compare"
    );

    let database = Database::new(DatabaseConfig {
        path: db.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .expect("open the index");
    let canonical = dunce::canonicalize(root).expect("canonical root");
    let resolution = Resolution {
        root: &canonical,
        cross_file_calls: true,
        fanout: DEFAULT_AMBIGUOUS_FANOUT,
    };
    for slice in ids.chunks(1000) {
        rebuild_edges_for_sources(&database, slice, resolution)
            .await
            .expect("derive the edges again");
    }
    if let Some(report) = difference(
        "edges derived in separate calls",
        &graph(&db, root),
        &scanned.0,
    ) {
        panic!("{report}");
    }
    assert_eq!(ambiguous_edges(&db, root), scanned.1);

    rebuild_edges_for_sources(&database, &ids, resolution)
        .await
        .expect("derive every edge again in one call");
    drop(database);
    if let Some(report) = difference("edges derived in one call", &graph(&db, root), &scanned.0) {
        panic!("{report}");
    }
    assert_eq!(ambiguous_edges(&db, root), scanned.1);
}
