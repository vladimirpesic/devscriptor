//! A name that several definitions share resolves to each of them, in an order
//! that does not depend on where the tree is checked out.
//!
//! The entity id cannot decide between tied candidates: it hashes the absolute
//! path, so two checkouts of one tree would link a call to different
//! overloads. Picking one would be arbitrary anyway; dead-code and impact
//! analysis need every possible target, and need to know the edge is a guess.
//! A name in a type position is no guess when a constructor shares it with its
//! class.

use std::collections::BTreeSet;
use std::path::Path;

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::entities::RelationshipType;
use devscriptor_core::parser::ParserConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

/// Overloaded methods, each pair called from another file.
const OVERLOADED: [&str; 6] = ["add", "sub", "mul", "div", "neg", "shift"];

/// `src/Calc.java` declares an `int` and a `long` overload of every name in
/// [`OVERLOADED`], the `int` one first, plus `twice`, which calls `add` in
/// the same file. `src/Main.java` calls every overloaded name once.
fn overloads() -> Vec<(&'static str, String)> {
    let mut calc = String::from("public class Calc {\n");
    for name in OVERLOADED {
        calc.push_str(&format!("public int {name}(int a) {{\nreturn a;\n}}\n"));
        calc.push_str(&format!("public long {name}(long a) {{\nreturn a;\n}}\n"));
    }
    calc.push_str("public int twice(int a) {\nreturn add(a) + add(a);\n}\n}\n");

    let mut main = String::from("public class Main {\npublic void run(Calc calc) {\n");
    for name in OVERLOADED {
        main.push_str(&format!("calc.{name}(1);\n"));
    }
    main.push_str("}\n}\n");

    vec![("src/Calc.java", calc), ("src/Main.java", main)]
}

/// An edge by relative path, name and line, comparable across checkouts:
/// `(source, kind, target, ambiguous candidates)`, where a symbol is
/// `(file, name, start line)` and the last field is `None` for an edge that
/// is not ambiguous.
type Symbol = (String, String, u32);
type Edge = (Symbol, String, Symbol, Option<u64>);

/// Write `files` under `root`, scan them into a new index with
/// `ambiguous_fanout`, and return the index's edges.
async fn scan(
    root: &Path,
    files: Vec<(&'static str, String)>,
    ambiguous_fanout: usize,
) -> BTreeSet<Edge> {
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        std::fs::write(path, content).expect("write fixture");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
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
        enable_cross_file: true,
        use_gitignore: false,
        include_extensions: vec!["java".to_string()],
        skip_embeddings: true,
        ambiguous_fanout,
        ..PipelineConfig::default()
    };
    CodebasePipeline::new(config)
        .expect("pipeline")
        .analyze_directory(root)
        .await
        .expect("scan");
    edges(&db, root)
}

fn edges(db: &Path, root: &Path) -> BTreeSet<Edge> {
    // The index stores its paths under the root's real path.
    let root = &dunce::canonicalize(root).expect("the root exists");
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.file_path, s.name, s.start_line, r.relationship_type, \
                    t.file_path, t.name, t.start_line, r.metadata \
             FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id",
        )
        .expect("prepare");
    // With `/` between folders, as the expectations are written, on Windows too.
    let relative = |path: String| {
        Path::new(&path)
            .strip_prefix(root)
            .expect("a path under the root")
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    stmt.query_map([], |row| {
        let metadata: Option<String> = row.get(7)?;
        let candidates = metadata
            .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
            .filter(|m| m["ambiguous"] == true)
            .map(|m| m["candidates"].as_u64().expect("a candidate count"));
        Ok((
            (relative(row.get(0)?), row.get(1)?, row.get(2)?),
            row.get(3)?,
            (relative(row.get(4)?), row.get(5)?, row.get(6)?),
            candidates,
        ))
    })
    .expect("query edges")
    .collect::<Result<_, _>>()
    .expect("read edges")
}

/// The edges of `kind` from the method `source` to methods named `target`.
fn calls<'a>(
    edges: &'a BTreeSet<Edge>,
    kind: RelationshipType,
    source: &'a str,
    target: &'a str,
) -> Vec<&'a Edge> {
    edges
        .iter()
        .filter(|(from, k, to, _)| from.1 == source && k == kind.as_str() && to.1 == target)
        .collect()
}

/// The `int` overload of the `index`-th name of [`OVERLOADED`] starts on
/// line `2 + 6 * index`, the `long` one three lines later.
fn overload_line(index: u32, long: bool) -> u32 {
    2 + 6 * index + if long { 3 } else { 0 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_overload_is_linked_and_marked_ambiguous() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let edges = scan(tree.path(), overloads(), 3).await;

    // From another file, through the cross-file pass.
    for (index, name) in (0..).zip(OVERLOADED) {
        let found = calls(&edges, RelationshipType::Calls, "run", name);
        let targets: Vec<(u32, Option<u64>)> = found.iter().map(|e| (e.2.2, e.3)).collect();
        assert_eq!(
            targets,
            vec![
                (overload_line(index, false), Some(2)),
                (overload_line(index, true), Some(2)),
            ],
            "run → {name}"
        );
        // The reverse rows carry the mark too.
        let callers: Vec<Option<u64>> = edges
            .iter()
            .filter(|(from, k, to, _)| {
                from.1 == name && k == RelationshipType::CalledBy.as_str() && to.1 == "run"
            })
            .map(|e| e.3)
            .collect();
        assert_eq!(callers, vec![Some(2), Some(2)], "{name} ← run");
    }

    // Within the file, through the per-file pass.
    let found = calls(&edges, RelationshipType::Calls, "twice", "add");
    let targets: Vec<(u32, Option<u64>)> = found.iter().map(|e| (e.2.2, e.3)).collect();
    assert_eq!(
        targets,
        vec![
            (overload_line(0, false), Some(2)),
            (overload_line(0, true), Some(2)),
        ],
        "twice → add"
    );

    // An edge with one possible target is not marked.
    let defined: Vec<&Edge> = edges
        .iter()
        .filter(|(_, k, _, _)| k == RelationshipType::HasMethod.as_str())
        .collect();
    assert!(!defined.is_empty());
    assert!(defined.iter().all(|e| e.3.is_none()), "{defined:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_checkouts_of_one_tree_resolve_alike() {
    let tree = TempDir::new().expect("tempdir for the trees");
    for fanout in [3, 1] {
        let first = scan(
            &tree.path().join(format!("alpha-{fanout}")),
            overloads(),
            fanout,
        )
        .await;
        let second = scan(
            &tree.path().join(format!("checkout-zeta-{fanout}")),
            overloads(),
            fanout,
        )
        .await;
        assert!(!first.is_empty());
        assert_eq!(first, second, "fanout {fanout}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fanout_of_one_links_the_first_candidate() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let edges = scan(tree.path(), overloads(), 1).await;

    for (index, name) in (0..).zip(OVERLOADED) {
        let found = calls(&edges, RelationshipType::Calls, "run", name);
        let targets: Vec<(u32, Option<u64>)> = found.iter().map(|e| (e.2.2, e.3)).collect();
        assert_eq!(
            targets,
            vec![(overload_line(index, false), Some(2))],
            "run → {name}"
        );
    }
    let found = calls(&edges, RelationshipType::Calls, "twice", "add");
    let targets: Vec<(u32, Option<u64>)> = found.iter().map(|e| (e.2.2, e.3)).collect();
    assert_eq!(targets, vec![(overload_line(0, false), Some(2))]);
}

/// A constructor is named after its class. Where a name is a type (a
/// supertype, a parameter type) the class is meant, so the constructor does
/// not make it ambiguous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_type_name_shared_with_a_constructor_means_the_type() {
    let files = vec![
        (
            "src/Shapes.java",
            "public class Base {\npublic Base() {\n}\n}\n\
             class Square extends Base {\npublic Square() {\n}\n}\n"
                .to_string(),
        ),
        (
            "src/Calc.java",
            "public class Calc {\npublic Calc() {\n}\n}\n".to_string(),
        ),
        (
            "src/Main.java",
            "public class Main {\npublic void run(Calc calc) {\n}\n}\n".to_string(),
        ),
    ];
    let tree = TempDir::new().expect("tempdir for the tree");
    let edges = scan(tree.path(), files, 3).await;

    let inherits: Vec<&Edge> = edges
        .iter()
        .filter(|(from, k, _, _)| from.1 == "Square" && k == RelationshipType::Inherits.as_str())
        .collect();
    assert_eq!(
        inherits,
        vec![&(
            ("src/Shapes.java".to_string(), "Square".to_string(), 5),
            RelationshipType::Inherits.as_str().to_string(),
            ("src/Shapes.java".to_string(), "Base".to_string(), 1),
            None,
        )]
    );

    let uses = calls(&edges, RelationshipType::Uses, "run", "Calc");
    assert_eq!(
        uses,
        vec![&(
            ("src/Main.java".to_string(), "run".to_string(), 2),
            RelationshipType::Uses.as_str().to_string(),
            ("src/Calc.java".to_string(), "Calc".to_string(), 1),
            None,
        )]
    );
}
