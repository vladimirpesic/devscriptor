//! Extraction records every reference an entity makes — calls, type uses,
//! supertypes, imported names — whether or not it resolves in its file: the
//! edges into other files are derived from these references, and a name
//! defined only later must still find the reference that names it.

use std::collections::BTreeSet;
use std::path::Path;

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::entities::RelationshipType;
use devscriptor_core::parser::parse_content_sync;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

/// A reference as `(source "type name", kind, target, qualifier)`.
type Ref = (String, &'static str, String, Option<String>);

/// The references in `content`, parsed as `language`.
fn refs(language: &str, path: &str, content: &str) -> BTreeSet<Ref> {
    let parsed = parse_content_sync(content, language, Path::new(path)).expect("parse");
    let source = |id| {
        parsed
            .entities
            .iter()
            .find(|e| e.id() == id)
            .map(|e| format!("{} {}", e.entity_type(), e.name()))
            .expect("a reference's source is an entity of the file")
    };
    parsed
        .refs
        .iter()
        .map(|r| {
            (
                source(r.source_id),
                r.kind.as_str(),
                r.target_name.clone(),
                r.qualifier.clone(),
            )
        })
        .collect()
}

fn expected(refs: &[(&str, &'static str, &str, Option<&str>)]) -> BTreeSet<Ref> {
    refs.iter()
        .map(|(source, kind, target, qualifier)| {
            (
                (*source).to_string(),
                *kind,
                (*target).to_string(),
                qualifier.map(str::to_string),
            )
        })
        .collect()
}

#[test]
fn rust_references() {
    let found = refs(
        "rust",
        "src/pay.rs",
        "use crate::money::Money;\n\npub struct Wallet {\n    balance: Money,\n}\n\n\
         pub trait Pay {\n    fn pay(&self) -> Money;\n}\n\nimpl Pay for Wallet {\n    \
         fn pay(&self) -> Money {\n        let total = add(self.balance, fee());\n        \
         missing_helper(total)\n    }\n}\n\nfn fee() -> Money {\n    Money::zero()\n}\n",
    );
    assert_eq!(
        found,
        expected(&[
            ("struct Wallet", "implements", "Pay", None),
            // The type of its field `balance`.
            ("struct Wallet", "uses", "Money", None),
            // Resolved in the file, and not.
            ("function pay", "calls", "fee", None),
            ("function pay", "calls", "add", None),
            ("function pay", "calls", "missing_helper", None),
            // A field it reads, through `self`.
            ("function pay", "references", "balance", Some("self")),
            // A path keeps its `::`: it names a type or a module, not a value.
            ("function fee", "calls", "zero", Some("Money::")),
            ("function pay", "uses", "Money", None),
            ("function fee", "uses", "Money", None),
        ])
    );
}

/// A call inside a macro's arguments is a reference too, though tree-sitter
/// reads macro arguments as tokens, not expressions; a function a macro
/// defines is not a call.
#[test]
fn rust_references_inside_macro_arguments() {
    let found = refs(
        "rust",
        "src/report.rs",
        "fn total(x: u32) -> u32 {\n    x\n}\n\n\
         fn report(items: Vec<u32>) -> String {\n    \
         assert_eq!(total(1), 1);\n    \
         let v = vec![total(2), helper()];\n    \
         define! { fn defined_inside() {} }\n    \
         format!(\"{} {}\", total(v.len() as u32), Money::zero())\n}\n",
    );
    for reference in expected(&[
        ("function report", "calls", "total", None),
        ("function report", "calls", "helper", None),
        ("function report", "calls", "len", Some("v")),
        ("function report", "calls", "zero", Some("Money::")),
    ]) {
        assert!(found.contains(&reference), "{reference:?} in {found:?}");
    }
    assert!(
        !found
            .iter()
            .any(|(_, _, target, _)| target == "defined_inside"),
        "{found:?}"
    );
    for called in ["total", "helper"] {
        assert!(
            !found
                .iter()
                .any(|(_, kind, target, _)| *kind == "references" && target == called),
            "a call in a macro is no value: {found:?}"
        );
    }

    // The call resolves in the file, as one outside a macro does.
    let parsed = parse_content_sync(
        "fn total(x: u32) -> u32 {\n    x\n}\n\n\
         fn report() -> String {\n    format!(\"{}\", total(1))\n}\n",
        "rust",
        Path::new("src/report.rs"),
    )
    .expect("parse");
    let id = |name: &str| {
        parsed
            .entities
            .iter()
            .find(|e| e.name() == name)
            .map(|e| e.id())
            .unwrap()
    };
    assert!(
        parsed
            .relationships
            .iter()
            .any(|r| r.source_id == id("report")
                && r.target_id == id("total")
                && r.relationship_type == RelationshipType::Calls),
        "report calls total"
    );
}

#[test]
fn python_references() {
    let found = refs(
        "python",
        "app/report.py",
        "from geometry import Shape, scale\nimport math\n\n\nclass Square(Shape):\n    \
         def area(self):\n        return scale(self.side) * math.sqrt(4)\n\n\n\
         def report(items: list) -> Summary:\n    total = helper(items)\n    \
         return Square().area()\n",
    );
    assert_eq!(
        found,
        expected(&[
            // The module imports these names.
            ("module report", "uses", "geometry", None),
            ("module report", "uses", "Shape", None),
            ("module report", "uses", "scale", None),
            ("module report", "uses", "math", None),
            ("class Square", "inherits", "Shape", None),
            ("function area", "calls", "scale", None),
            ("function area", "calls", "sqrt", Some("math")),
            // An attribute it reads; a supertype is no read.
            ("function area", "references", "side", Some("self")),
            ("function report", "calls", "helper", None),
            ("function report", "calls", "Square", None),
            ("function report", "calls", "area", Some("Square()")),
            ("function report", "uses", "Summary", None),
        ])
    );
}

#[test]
fn typescript_references() {
    let found = refs(
        "typescript",
        "src/circle.ts",
        "import { Base } from \"./base\";\n\ninterface Shape {\n  area(): number;\n}\n\n\
         export class Circle extends Base implements Shape {\n  area(): number {\n    \
         return Math.PI * square(this.r);\n  }\n}\n\n\
         export function total(items: Circle[]): Report {\n  \
         return format(items.map((c) => c.area()));\n}\n",
    );
    assert_eq!(
        found,
        expected(&[
            ("class Circle", "inherits", "Base", None),
            ("class Circle", "implements", "Shape", None),
            ("function area", "calls", "square", None),
            // A constant and a field it reads.
            ("function area", "references", "PI", Some("Math")),
            ("function area", "references", "r", Some("this")),
            ("function total", "calls", "format", None),
            ("function total", "calls", "map", Some("items")),
            // From the arrow function, which is not an entity of its own.
            ("function total", "calls", "area", Some("c")),
            ("function total", "uses", "Report", None),
            ("function total", "uses", "Circle", None),
        ])
    );
}

#[test]
fn java_references() {
    let found = refs(
        "java",
        "src/Main.java",
        "import java.util.List;\n\npublic class Main extends Base implements Runnable {\n    \
         public void run() {\n        Calc calc = new Calc();\n        calc.add(1);\n        \
         helper();\n        System.out.println(\"x\");\n    }\n}\n",
    );
    assert_eq!(
        found,
        expected(&[
            ("class Main", "inherits", "Base", None),
            ("class Main", "implements", "Runnable", None),
            // The constructor a `new` invokes, named after its class.
            ("function run", "calls", "Calc", None),
            // The method a call names, not the object it is called on.
            ("function run", "calls", "add", Some("calc")),
            ("function run", "calls", "helper", None),
            ("function run", "calls", "println", Some("System.out")),
            // The field the call is made on.
            ("function run", "references", "out", Some("System")),
        ])
    );
}

#[test]
fn go_references() {
    let found = refs(
        "go",
        "shop/cart.go",
        "package shop\n\nimport \"fmt\"\n\ntype Cart struct {\n\tItems []Item\n}\n\n\
         func (c *Cart) Total() Money {\n\tsum := c.sumItems()\n\tfmt.Println(sum)\n\t\
         return Convert(sum)\n}\n",
    );
    assert_eq!(
        found,
        expected(&[
            ("function Total", "calls", "sumItems", Some("c")),
            ("function Total", "calls", "Println", Some("fmt")),
            ("function Total", "calls", "Convert", None),
            ("function Total", "uses", "Money", None),
            // The type of its field `Items`.
            ("struct Cart", "uses", "Item", None),
        ])
    );
}

/// Every forward edge a fresh scan stores comes from a stored reference of
/// its source to its target's name: the references are a complete record of
/// what the edges were derived from. Nesting (`has_method`, `method_of`)
/// does not come from references, and reverse rows mirror forward ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_forward_edge_of_a_scan_has_its_reference() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_files");
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(&corpus)
    .await
    .expect("scan the corpus");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, s.file_path, r.relationship_type, t.name, \
                    EXISTS (SELECT 1 FROM symbol_refs x \
                            WHERE x.source_id = r.source_id AND x.target_name = t.name \
                              AND (x.kind = r.relationship_type \
                                   OR (r.relationship_type = 'implements' AND x.kind = 'inherits'))) \
             FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id",
        )
        .expect("prepare");
    let edges: Vec<(String, String, String, String, bool)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query edges")
        .collect::<Result<_, _>>()
        .expect("read edges");

    let forward: Vec<_> = edges
        .iter()
        .filter(|(_, _, kind, _, _)| {
            let kind = RelationshipType::parse(kind).expect("a known kind");
            !kind.is_reverse() && !kind.is_nesting()
        })
        .collect();
    assert!(
        forward.len() > 100,
        "the corpus yields edges to check: {}",
        forward.len()
    );
    let unmatched: Vec<_> = forward.iter().filter(|edge| !edge.4).collect();
    assert!(
        unmatched.is_empty(),
        "{} of {} forward edges have no reference, e.g. {:?}",
        unmatched.len(),
        forward.len(),
        &unmatched[..unmatched.len().min(10)]
    );

    // An edge to a supertype records the name of the type that extends it.
    let mut stmt = conn
        .prepare(
            "SELECT s.name, r.metadata FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             WHERE r.relationship_type IN ('inherits', 'implements')",
        )
        .expect("prepare");
    let supertypes: Vec<(String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query supertype edges")
        .collect::<Result<_, _>>()
        .expect("read supertype edges");
    assert!(!supertypes.is_empty(), "the corpus extends types");
    for (name, metadata) in &supertypes {
        let metadata: serde_json::Value =
            serde_json::from_str(metadata.as_deref().unwrap_or("null")).expect("JSON metadata");
        assert_eq!(metadata["child"], name.as_str(), "{metadata}");
    }
}

/// An interface that extends one in another file inherits it, as one in the
/// same file does; a class implements the interfaces it names. A supertype
/// that is an interface is not an `implements` whatever extends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interface_inherits_the_interface_it_extends_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        ("Named.java", "interface Named { String name(); }\n"),
        (
            "Priced.java",
            "interface Priced extends Named { int price(); }\n",
        ),
        (
            "Cart.java",
            "class Cart implements Priced {\n    public String name() { return \"cart\"; }\n    public int price() { return 0; }\n}\n",
        ),
    ] {
        std::fs::write(project.path().join(file), source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, r.relationship_type, t.name FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type IN ('inherits', 'implements')",
        )
        .expect("prepare");
    let mut edges: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query supertype edges")
        .collect::<Result<_, _>>()
        .expect("read supertype edges");
    edges.sort();
    let edge =
        |from: &str, kind: &str, to: &str| (from.to_string(), kind.to_string(), to.to_string());
    assert_eq!(
        edges,
        [
            edge("Cart", "implements", "Priced"),
            edge("Priced", "inherits", "Named")
        ]
    );
}

/// A call of an imported name resolves to the module the import names, not to
/// any file defining the name: the names a JavaScript or TypeScript import
/// gives are read, and a module named by path (`./b.js`) matches that file,
/// not every JavaScript file by its extension.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_of_an_imported_name_resolves_to_the_module_it_names() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        ("a.js", "export function fmt() { return 1; }\n"),
        ("b.js", "export function fmt() { return 2; }\n"),
        (
            "c.js",
            "import { fmt } from './b.js';\nexport function show() { return fmt(); }\n",
        ),
        (
            "d.ts",
            "import { fmt as f } from './a';\nexport function display() { return f(); }\n",
        ),
    ] {
        std::fs::write(project.path().join(file), source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'calls'",
        )
        .expect("prepare");
    let mut calls: Vec<(String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(1)?;
            let name = Path::new(&file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((row.get(0)?, name))
        })
        .expect("query calls")
        .collect::<Result<_, _>>()
        .expect("read calls");
    calls.sort();
    assert_eq!(
        calls,
        [
            ("display".to_string(), "a.js".to_string()),
            ("show".to_string(), "b.js".to_string()),
        ]
    );
}

/// A name a Rust `use` or a Python `from` import brings in resolves to the
/// module it names, beside another module defining the same name: each name a
/// statement imports is its own reference, not the whole path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_used_or_imported_name_resolves_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    let files = [
        (
            "pricing/src/shipping.rs",
            "pub fn shipping_cents(x: u64) -> u64 { x }\n",
        ),
        (
            "pricing/src/legacy.rs",
            "pub fn shipping_cents(x: u64) -> u64 { x + 1 }\n",
        ),
        (
            "pricing/src/totals.rs",
            "use crate::shipping::shipping_cents;\npub fn price() -> u64 { shipping_cents(1) }\n",
        ),
        (
            "app/dispatch.py",
            "def ships_same_day(t):\n    return True\n",
        ),
        (
            "app/legacy.py",
            "def ships_same_day(t):\n    return False\n",
        ),
        (
            "tests/test_dispatch.py",
            "from app.dispatch import dispatch_date, ships_same_day\n\n\
             def test_morning():\n    assert ships_same_day(9)\n",
        ),
    ];
    for (file, source) in files {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'calls'",
        )
        .expect("prepare");
    let mut calls: Vec<(String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(1)?;
            let name = Path::new(&file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((row.get(0)?, name))
        })
        .expect("query calls")
        .collect::<Result<_, _>>()
        .expect("read calls");
    calls.sort();
    assert_eq!(
        calls,
        [
            ("price".to_string(), "shipping.rs".to_string()),
            ("test_morning".to_string(), "dispatch.py".to_string()),
        ]
    );
}

/// `Order::new()` in another file resolves to `Order`'s `new`, not to the
/// nearest or first `new`: resolution reads the qualifier, and `new` is Go's
/// built-in only in Go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_qualified_call_resolves_to_the_type_it_names_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        (
            "src/line.rs",
            "pub struct Line;\nimpl Line {\n    pub fn new() -> Self { Line }\n}\n",
        ),
        (
            "src/order.rs",
            "pub struct Order;\nimpl Order {\n    pub fn new() -> Self { Order }\n}\n",
        ),
        (
            "src/checkout.rs",
            "use crate::order::Order;\npub fn open() -> Order { Order::new() }\n",
        ),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'calls'",
        )
        .expect("prepare");
    let calls: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(2)?;
            let name = Path::new(&file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((row.get(0)?, row.get(1)?, name))
        })
        .expect("query calls")
        .collect::<Result<_, _>>()
        .expect("read calls");
    assert_eq!(
        calls,
        [(
            "open".to_string(),
            "new".to_string(),
            "order.rs".to_string()
        )]
    );
}

/// A call through a path is of what the path names across files too: a
/// function of the module it names (`config::load()` of `config.rs`,
/// `geo::area` of the file holding `namespace geo`), and nothing when the
/// path names no module or type the index holds — `std::abs`,
/// `serde_json::from_str`, `tokio::spawn` and `Vec::new()` are no calls of
/// whichever `from_str`, `spawn` and `new` the project defines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_through_a_path_is_of_nothing_it_does_not_name_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        ("src/config.rs", "pub fn load() -> u32 { 1 }\n"),
        (
            "src/settings.rs",
            "pub struct Settings;\nimpl Settings {\n    pub fn from_str(s: &str) -> Self { Settings }\n    \
             pub fn new() -> Self { Settings }\n}\n",
        ),
        ("src/tasks.rs", "pub fn spawn() {}\n"),
        // A C++ namespace spans files, whichever their names.
        (
            "lib/shapes.cpp",
            "namespace geo {\nint area(int s) { return s; }\n}\nint abs(int x) { return x; }\n",
        ),
        (
            "lib/run.cpp",
            "int main() { return geo::area(2) + std::abs(-1); }\n",
        ),
        (
            "src/main.rs",
            "fn main() {\n    let _ = config::load();\n    let _: u32 = serde_json::from_str(\"1\").unwrap();\n    \
             tokio::spawn(async {});\n    let _: Vec<u8> = Vec::new();\n}\n",
        ),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'calls'",
        )
        .expect("prepare");
    let mut calls: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(2)?;
            let name = Path::new(&file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((row.get(0)?, row.get(1)?, name))
        })
        .expect("query calls")
        .collect::<Result<_, _>>()
        .expect("read calls");
    calls.sort();
    let call = |source: &str, target: &str, file: &str| {
        (source.to_string(), target.to_string(), file.to_string())
    };
    assert_eq!(
        calls,
        [
            call("main", "area", "shapes.cpp"),
            call("main", "load", "config.rs")
        ]
    );
}

/// A call through a value an import binds is of the module the import
/// names, across files: Go's `billing.Price()` of its package's file, a
/// Python module's function of that module (`from . import utils`, of the
/// importing file's package); and of nothing when the index holds no such
/// module — `fmt.Println`, `json.dumps`, `re.compile` are no calls of
/// whichever `Println`, `dumps` and `compile` the project defines, the last in
/// a file merely under `reports/`, whose path holds `re`. A standard type's method
/// through any other value (`lines.append`) is the standard type's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_through_an_imported_name_is_of_its_module_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        (
            "billing/price.go",
            "package billing\n\nfunc Price() int { return 1 }\n",
        ),
        (
            "logging/print.go",
            "package logging\n\nfunc Println(s string) {}\n",
        ),
        (
            "main.go",
            "package main\n\nimport (\n\t\"fmt\"\n\t\"github.com/acme/shop/billing\"\n)\n\n\
             func main() { fmt.Println(billing.Price()) }\n",
        ),
        ("app/utils.py", "def slugify(text):\n    return text\n"),
        (
            "app/serializers.py",
            "def dumps(value):\n    return value\n",
        ),
        (
            "app/reports/compiler.py",
            "def compile(source):\n    return source\n",
        ),
        (
            "app/tasks.py",
            "class TaskQueue:\n    def append(self, task):\n        pass\n",
        ),
        (
            "app/views.py",
            "import json\nimport re\nfrom . import utils\n\n\
             def show(text):\n    lines = []\n    lines.append(text)\n    \
             return utils.slugify(json.dumps(re.compile(text)))\n",
        ),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'calls'",
        )
        .expect("prepare");
    let mut calls: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(2)?;
            let name = Path::new(&file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((row.get(0)?, row.get(1)?, name))
        })
        .expect("query calls")
        .collect::<Result<_, _>>()
        .expect("read calls");
    calls.sort();
    let call = |source: &str, target: &str, file: &str| {
        (source.to_string(), target.to_string(), file.to_string())
    };
    assert_eq!(
        calls,
        [
            call("main", "Price", "price.go"),
            call("show", "slugify", "utils.py")
        ]
    );
}

/// A Go handler defined in one file of its package and registered in another
/// is referenced by the registration; a function of the same name in another
/// directory is not, nor one named like a variable passed — a passed name
/// resolves beside its caller, or through an import.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_function_passed_as_a_value_is_referenced_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        (
            "api/handlers.go",
            "package api\n\nfunc handleCheckout(w int) {}\n",
        ),
        (
            "api/server.go",
            "package api\n\nfunc routes(mux *Mux, total int) {\n\
             \tmux.HandleFunc(\"/checkout\", handleCheckout)\n\tmux.Limit(total)\n}\n",
        ),
        (
            "legacy/handlers.go",
            "package legacy\n\nfunc handleCheckout(w int) {}\nfunc total() int { return 0 }\n",
        ),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'references'",
        )
        .expect("prepare");
    let references: Vec<(String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(1)?;
            let parts: Vec<&str> = file.rsplit(['/', '\\']).take(2).collect();
            Ok((row.get(0)?, format!("{}/{}", parts[1], parts[0])))
        })
        .expect("query references")
        .collect::<Result<_, _>>()
        .expect("read references");
    assert_eq!(
        references,
        [("routes".to_string(), "api/handlers.go".to_string())]
    );
}

/// Code outside every entity is its file's module's — named after the file,
/// as a Python file's module is: the callbacks a test file hands its test
/// runner, the code at the top of a script. Without it a JavaScript test file
/// would have no entity at all, and none of its calls would be recorded, so
/// nothing it tests would be called from a test.
#[test]
fn code_outside_every_entity_is_its_files_module() {
    let src = "import { test } from 'node:test';\n\
               import { buildOrdersQuery } from '../src/api/orders.js';\n\
               \n\
               test('page size is sent', () => {\n  buildOrdersQuery({ size: 50 });\n});\n\
               \n\
               test('the endpoint is the order list', () => {\n  buildOrdersQuery();\n});\n";
    let parsed =
        parse_content_sync(src, "javascript", Path::new("test/orders.test.js")).expect("parse");
    let module = parsed
        .entities
        .iter()
        .find(|e| e.entity_type() == "module")
        .expect("a module for the file's code");
    assert_eq!(module.name(), "orders.test");
    let lines: Vec<u32> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Calls && r.target_name == "buildOrdersQuery")
        .map(|r| {
            assert_eq!(r.source_id, module.id(), "{r:?}");
            r.line
        })
        .collect();
    assert_eq!(lines, [5, 9]);

    // A file whose code is all in its entities gets no module, and its imports
    // are not recorded; nothing is linked to a module it does not have.
    let src = "import { BASE } from './base.js';\nexport function fetchOrders() {\n  return get(BASE);\n}\n";
    let parsed =
        parse_content_sync(src, "javascript", Path::new("src/api/orders.js")).expect("parse");
    let ids: Vec<_> = parsed.entities.iter().map(|e| e.id()).collect();
    assert!(
        parsed
            .relationships
            .iter()
            .all(|r| ids.contains(&r.source_id) && ids.contains(&r.target_id)),
        "{:?}",
        parsed.relationships
    );
    let found = refs(
        "javascript",
        "src/api/orders.js",
        "import { BASE } from './base.js';\nexport function fetchOrders() {\n  return get(BASE);\n}\n",
    );
    assert_eq!(
        found,
        expected(&[
            ("function fetchOrders", "calls", "get", None),
            ("function fetchOrders", "references", "BASE", None),
        ])
    );
    // Code calling only its language's built-ins links nothing, and makes no
    // module: a Ruby file's `require`, a SQL report's `COUNT`.
    for (language, path, src) in [
        ("ruby", "lib/boot.rb", "require 'json'\nputs 'ready'\n"),
        (
            "sql",
            "reports/totals.sql",
            "SELECT COUNT(*), SUM(total) FROM orders;\n",
        ),
    ] {
        let parsed = parse_content_sync(src, language, Path::new(path)).expect("parse");
        assert!(
            parsed.entities.iter().all(|e| e.entity_type() != "module"),
            "{path}: {:?}",
            parsed.entities.iter().map(|e| e.name()).collect::<Vec<_>>()
        );
    }
}

/// The code at the top of a component's script — most of a `<script setup>` —
/// is the component's, which spans the file: `onMounted(() => refresh())` is a
/// call of the component's.
#[test]
fn a_component_scripts_top_level_code_is_the_components() {
    let src = "<script setup>\n\
               import { onMounted } from 'vue';\n\
               import { fetchOrders } from './api/orders.js';\n\
               function refresh() {\n  return fetchOrders();\n}\n\
               onMounted(() => refresh());\n\
               </script>\n\
               <template><p>orders</p></template>\n";
    let parsed = parse_content_sync(src, "vue", Path::new("src/OrderList.vue")).expect("parse");
    assert!(
        parsed.entities.iter().all(|e| e.entity_type() != "module"),
        "the component is the file's"
    );
    let found = refs("vue", "src/OrderList.vue", src);
    for call in [
        ("component OrderList", "onMounted"),
        ("component OrderList", "refresh"),
        ("function refresh", "fetchOrders"),
    ] {
        assert!(
            found.contains(&(call.0.to_string(), "calls", call.1.to_string(), None)),
            "{call:?} in {found:?}"
        );
    }
}

/// The use sites of a name under a folder: each line that refers to it, by
/// the name or by the alias an import binds to it, each line an import names
/// it on — in a multi-line import, its own — and none outside the folder:
/// it on — in a multi-line import, its own — and none outside the folder:
/// what `get_references` adds when a language server cannot see a file, such
/// as a Svelte component's import of a dashboard's `formatUsd`, which the
/// server cannot read in `.svelte`. And the calls alone, each with what it is
/// made in, which `get_incoming_calls` adds: a Vue component's caller among
/// them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_use_sites_of_a_name_under_a_folder() {
    let project = TempDir::new().expect("tempdir for the project");
    let app = project.path().join("app");
    let other = project.path().join("other");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    for (file, source) in [
        (
            app.join("validate.js"),
            "export function windowProblems(w) {\n  return [];\n}\n",
        ),
        (
            app.join("form.js"),
            "import { windowProblems } from './validate.js';\nexport function check(w) {\n  return windowProblems(w);\n}\n",
        ),
        (
            app.join("board.js"),
            "import { windowProblems as wp } from './validate.js';\nexport function board(w) {\n  const a = 1;\n  return wp(w);\n}\n",
        ),
        (
            app.join("multi.js"),
            "import {\n  check,\n  windowProblems,\n} from './validate.js';\nexport const n = windowProblems(1);\n",
        ),
        (
            app.join("Use.kt"),
            "import app.validate.windowProblems\n\nfun use() = windowProblems(1)\n",
        ),
        (
            app.join("Card.svelte"),
            "<script>\n  import { windowProblems } from './validate.js';\n  export let w;\n</script>\n\n<p>{windowProblems(w)}</p>\n",
        ),
        (
            other.join("use.js"),
            "export function elsewhere(w) {\n  return windowProblems(w);\n}\n",
        ),
    ] {
        std::fs::write(file, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db_path = index.path().join("index.db");
    let config = DatabaseConfig {
        path: db_path.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    };
    CodebasePipeline::new(PipelineConfig {
        database: config.clone(),
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let db = devscriptor_core::database::Database::new(config).expect("open the index");
    // The folder by its real path, which the index stores.
    let app = dunce::canonicalize(&app).unwrap();
    let sites = db
        .get_reference_sites(&app, &["windowProblems".to_string()])
        .await
        .expect("query");
    let sites: Vec<(String, u32, String)> = sites
        .into_iter()
        .map(|(file, line, language)| {
            let name = Path::new(&file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, line, language)
        })
        .collect();
    let site =
        |file: &str, line: u32, language: &str| (file.to_string(), line, language.to_string());
    assert_eq!(
        sites,
        [
            site("Card.svelte", 2, "svelte"),
            site("Card.svelte", 6, "svelte"),
            site("Use.kt", 1, "kotlin"),
            site("Use.kt", 3, "kotlin"),
            site("board.js", 1, "javascript"),
            site("board.js", 4, "javascript"),
            site("form.js", 1, "javascript"),
            site("form.js", 3, "javascript"),
            site("multi.js", 3, "javascript"),
            site("multi.js", 5, "javascript"),
        ],
        "the imports and the calls, by the name and by the import's alias, not the one outside app/"
    );

    let calls: Vec<(String, u32, String)> = db
        .get_call_sites(&app, &["windowProblems".to_string()])
        .await
        .expect("query")
        .into_iter()
        .map(|(file, line, _, caller)| {
            let name = Path::new(&file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, line, caller)
        })
        .collect();
    let call = |file: &str, line: u32, caller: &str| (file.to_string(), line, caller.to_string());
    assert_eq!(
        calls,
        [
            call("Card.svelte", 6, "Card"),
            call("Use.kt", 3, "use"),
            call("board.js", 4, "board"),
            call("form.js", 3, "check"),
            call("multi.js", 5, "n"),
        ],
        "each call with what it is made in, through an alias too; no import"
    );
}

/// The calls in a component's markup are the component's, as those at the
/// top of its script are: Svelte's `{…}` in text, attributes, `{@html …}`
/// and block tags, and Vue's `{{ … }}` and directive values. A function used
/// only in the markup — a dashboard's `formatUsd` — has its callers there,
/// which `get_references` and the dead-code analysis count as uses of it.
#[test]
fn markup_calls_are_the_components() {
    let calls = |language: &str, path: &str, content: &str| -> BTreeSet<(String, String)> {
        refs(language, path, content)
            .into_iter()
            .filter(|(_, kind, _, _)| *kind == "calls")
            .map(|(source, _, target, _)| (source, target))
            .collect()
    };
    let svelte = calls(
        "svelte",
        "src/RevenueCard.svelte",
        "<script>\n  import { formatUsd } from './money.js';\n  export let cents;\n</script>\n\n\
         <p class=\"amount\">{formatUsd(cents)}</p>\n{#each rows(cents) as item}\n  \
         <Card value={total(item)} on:click={() => pick(item)} />\n{/each}\n{@html render(cents)}\n",
    );
    for target in ["formatUsd", "rows", "total", "pick", "render"] {
        assert!(
            svelte
                .iter()
                .any(|(source, called)| source.starts_with("component") && called == target),
            "svelte markup call of {target}: {svelte:?}"
        );
    }
    let vue = calls(
        "vue",
        "src/BerthWindowForm.vue",
        "<script setup>\nimport { windowProblems } from '../validate.js';\n</script>\n\n<template>\n  \
         <p>{{ formatUsd(cents) }}</p>\n  <button :disabled=\"problems(a).length > 0\" \
         @click=\"submit(x)\" v-for=\"p in list(y)\">Go</button>\n</template>\n",
    );
    for target in ["formatUsd", "problems", "submit", "list"] {
        assert!(
            vue.iter()
                .any(|(source, called)| source.starts_with("component") && called == target),
            "vue markup call of {target}: {vue:?}"
        );
    }

    // The binding of a keyed `{#each}` is no expression: `row, i (row.id)`
    // read as one calls `i`.
    let keyed = calls(
        "svelte",
        "src/Rows.svelte",
        "<script>\n  export let rows;\n</script>\n{#each rows as row, i (row.id)}\n  <p>{label(row)}</p>\n{/each}\n",
    );
    assert!(
        keyed.iter().any(|(_, called)| called == "label"),
        "{keyed:?}"
    );
    assert!(!keyed.iter().any(|(_, called)| called == "i"), "{keyed:?}");

    // Each expression is a statement of its own: side by side, `{label}`
    // and `{(fmt(x))}` read as one are a call of `label`.
    let side_by_side = calls(
        "svelte",
        "src/Label.svelte",
        "<script>\n  export let label, x;\n</script>\n<p>{label}{(fmt(x))}</p>\n",
    );
    assert!(
        side_by_side.iter().any(|(_, called)| called == "fmt"),
        "{side_by_side:?}"
    );
    assert!(
        !side_by_side.iter().any(|(_, called)| called == "label"),
        "{side_by_side:?}"
    );

    // Each at its own line: the markup is read where it stands.
    let content = "<script>\n  let cents = 5;\n</script>\n\n<p>{formatUsd(cents)}</p>\n";
    let parsed = parse_content_sync(content, "svelte", Path::new("a.svelte")).expect("parse");
    let line = parsed
        .refs
        .iter()
        .find(|r| r.target_name == "formatUsd")
        .map(|r| r.line);
    assert_eq!(line, Some(5));
}

/// What each grammar's code uses as a value is recorded — a constant read
/// through what it is a member of and on its own, a property read, a name
/// imported and read — and what it binds, calls, types or names is not: a
/// parameter, a local, a loop's or a lambda's variable, a callee, a type, a
/// label, a package, the module an import binds as the object of its
/// members, a constant's own declaration, a member's name on its own. With
/// only calls and types recorded, the index would hold no read of a constant a
/// server missed.
#[test]
fn the_names_code_uses_as_values_are_recorded_in_every_grammar() {
    /// A grammar, a file of it, its source, the uses recorded (each name with
    /// what it is a member of) and the names not recorded.
    type Case<'a> = (
        &'a str,
        &'a str,
        &'a str,
        &'a [(&'a str, Option<&'a str>)],
        &'a [&'a str],
    );
    let cases: [Case; 18] = [
        (
            "python",
            "m.py",
            "import config\nfrom limits import LIMIT\nMAX = 3\ncount = 0\n\ndef f(o):\n    x = Limits.MAX + MAX\n    \
             for item in o.items:\n        g(item, config.CUT, LIMIT)\n    return o.size\n",
            &[
                ("MAX", Some("Limits")),
                ("MAX", None),
                ("size", Some("o")),
                ("CUT", Some("config")),
                ("LIMIT", None),
            ],
            &["o", "x", "g", "item", "config", "count"],
        ),
        (
            "javascript",
            "m.js",
            "const MAX = 3;\nconst [first, second] = pairs;\nfunction f(o) {\n  const x = Limits.MAX + MAX;\n  o.items.forEach(i => g(i, x, first));\n  return o.size;\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "i", "second"],
        ),
        (
            "typescript",
            "m.ts",
            "const MAX = 3;\nfunction f(o: Order): number {\n  const x: number = Limits.MAX + MAX;\n  g(x);\n  return o.size;\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "Order"],
        ),
        (
            "go",
            "m.go",
            "package p\n\nimport \"booking/internal/config\"\n\nconst MAX = 3\n\nfunc f(o T) int {\n\tx := Limits.MAX + MAX\n\tg(x, config.DocCutoff)\n\treturn o.size\n}\n",
            &[
                ("MAX", Some("Limits")),
                ("MAX", None),
                ("size", Some("o")),
                ("DocCutoff", Some("config")),
            ],
            &["o", "x", "g", "T", "config"],
        ),
        (
            "rust",
            "m.rs",
            "const MAX: u32 = 3;\n\nfn f(o: &T) -> u32 {\n    let x = Limits::MAX + MAX;\n    g(x);\n    o.size\n}\n",
            &[
                ("MAX", Some("Limits::")),
                ("MAX", None),
                ("size", Some("o")),
            ],
            &["o", "x", "g", "T"],
        ),
        (
            "java",
            "A.java",
            "package com.acme;\n\nclass A {\n    static final int MAX = 3;\n\n    int f(T o) {\n        int x = Limits.MAX + MAX;\n        g(x);\n        return o.size;\n    }\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "T", "com", "acme"],
        ),
        (
            "kotlin",
            "m.kt",
            "object L { const val MAX = 3 }\n\nclass P(val rate: Int)\n\nfun f(o: T): Int {\n    val x = Limits.MAX + MAX\n    o.items.forEach { g(it, x) }\n    return o.size\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "T", "it", "rate"],
        ),
        (
            "swift",
            "m.swift",
            "let MAX = 3\n\nfunc f(o: T) -> Int {\n    let x = Limits.MAX + MAX\n    g(value: x)\n    return o.size\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "T", "value"],
        ),
        (
            "c_sharp",
            "A.cs",
            "namespace App.Billing\n{\n    class A {\n        const int MAX = 3;\n\n        int F(T o) {\n            var x = Limits.MAX + MAX;\n            G(x);\n            return o.size;\n        }\n    }\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "G", "T", "App", "Billing"],
        ),
        (
            "cpp",
            "m.cpp",
            "const int MAX = 3;\n\nint f(T o) {\n    int x = Limits::MAX + MAX;\n    g(x);\n    return o.size;\n}\n",
            &[
                ("MAX", Some("Limits::")),
                ("MAX", None),
                ("size", Some("o")),
            ],
            &["o", "x", "g", "T"],
        ),
        (
            "php",
            "m.php",
            "<?php\nconst MAX = 3;\n\nfunction f($o) {\n    $x = Limits::MAX + MAX;\n    g($x);\n    return $o->size;\n}\n",
            &[
                ("MAX", Some("Limits::")),
                ("MAX", None),
                ("size", Some("$o")),
            ],
            &["o", "x", "g"],
        ),
        (
            "ruby",
            "m.rb",
            "MAX = 3\n\ndef f(o)\n  x = Limits::MAX + MAX\n  g(x)\n  o.size\nend\n",
            // Ruby reads an attribute by calling it.
            &[("MAX", Some("Limits::")), ("MAX", None)],
            &["o", "x", "g", "size"],
        ),
        (
            "scala",
            "m.scala",
            "object A {\n  val MAX = 3\n\n  def f(o: T): Int = {\n    val x = Limits.MAX + MAX\n    g(x)\n    o.size\n  }\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("size", Some("o"))],
            &["o", "x", "g", "T"],
        ),
        (
            "dart",
            "m.dart",
            "const maxRate = 3;\n\nint f(T o) {\n  var x = Limits.maxRate + maxRate;\n  g(x);\n  return o.size;\n}\n",
            &[
                ("maxRate", Some("Limits")),
                ("maxRate", None),
                ("size", Some("o")),
            ],
            &["o", "x", "g", "T"],
        ),
        (
            "bash",
            "m.sh",
            "readonly MAX=3\n\nf() {\n  local x=$(( MAX + 1 ))\n  for v in 1 2; do\n    g \"$x\" \"$v\"\n  done\n}\n",
            &[("MAX", None)],
            &["x", "g", "v"],
        ),
        (
            "solidity",
            "m.sol",
            "contract C {\n    uint constant MAX = 3;\n\n    function f() public returns (uint) {\n        uint x = Limits.MAX + MAX;\n        g(x);\n        return x;\n    }\n}\n",
            &[("MAX", Some("Limits")), ("MAX", None)],
            &["x", "g"],
        ),
        (
            "fsharp",
            "m.fs",
            "module M\n\nlet MAX = 3\n\nlet f o =\n    let x = Limits.MAX + MAX\n    g x\n    o.Size\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("Size", Some("o"))],
            &["o", "x", "g", "f"],
        ),
        (
            "vb_net",
            "m.vb",
            "Module M\n    Const MAX As Integer = 3\n\n    Function F(o As T) As Integer\n        Dim x = Limits.MAX + MAX\n        G(x)\n        Return o.Size\n    End Function\nEnd Module\n",
            &[("MAX", Some("Limits")), ("MAX", None), ("Size", Some("o"))],
            &["o", "x", "G", "T"],
        ),
    ];
    for (language, file, source, recorded, not_recorded) in cases {
        let parsed = parse_content_sync(source, language, Path::new(file)).expect("parse");
        let uses: Vec<(String, Option<String>)> = parsed
            .refs
            .iter()
            .filter(|r| r.kind == RelationshipType::References)
            .map(|r| (r.target_name.clone(), r.qualifier.clone()))
            .collect();
        // A constant's declaration is no use of it, nor a member's name a
        // name read on its own.
        let constant = recorded[0].0;
        let declared = source
            .lines()
            // Each declares it before it uses it.
            .position(|line| line.contains(constant))
            .map(|index| u32::try_from(index + 1).unwrap())
            .unwrap();
        assert!(
            !parsed
                .refs
                .iter()
                .any(|r| r.kind == RelationshipType::References
                    && r.target_name == constant
                    && r.line == declared),
            "{language}: the declaration of {constant} is no use of it"
        );
        for (name, qualifier) in recorded {
            if qualifier.is_some() {
                assert!(
                    !uses
                        .iter()
                        .any(|(n, q)| n == name && q.is_none() && *name != constant),
                    "{language}: the member {name} is no name read on its own: {uses:?}"
                );
            }
        }
        for (name, qualifier) in recorded {
            assert!(
                uses.iter()
                    .any(|(n, q)| n == name && q.as_deref() == *qualifier),
                "{language}: {name} through {qualifier:?} in {uses:?}"
            );
        }
        for name in not_recorded {
            assert!(
                !uses.iter().any(|(n, _)| n == name),
                "{language}: {name} is no use: {uses:?}"
            );
        }
    }
}

/// A constant read in another file is referenced by what reads it: a
/// Kotlin object's constant through the object (`ReeferLimits.MAX_CELSIUS`,
/// not another object's of the same name beside it), a C# class's through
/// the class from another namespace's directory (not another class's), a Go
/// package's through its import (`config.DocCutoff`, not another package's),
/// a Python module's by the name imported, and a Java class's own through
/// `this` (not another class's of the file). A member of a value of no known
/// type (`gauge.MAX_CELSIUS`) is none, nor a name of another directory. The reads are use sites of the
/// name, and the files reading it files that use it: what `get_references`
/// adds for a server that missed them, and opens before asking it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_constant_read_is_referenced_across_files() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        (
            "reefer/ReeferLimits.kt",
            "package reefer\n\nobject ReeferLimits {\n    const val MAX_CELSIUS = 25.0\n}\n",
        ),
        (
            "reefer/OtherLimits.kt",
            "package reefer\n\nobject OtherLimits {\n    const val MAX_CELSIUS = 99.0\n}\n",
        ),
        (
            "reefer/SetPoint.kt",
            "package reefer\n\nfun validate(c: Double, gauge: Gauge): Boolean {\n    return c > ReeferLimits.MAX_CELSIUS || c > gauge.MAX_CELSIUS\n}\n",
        ),
        (
            "internal/config/defaults.go",
            "package config\n\nconst DocCutoff = 24\n",
        ),
        (
            "internal/instructions/cutoff.go",
            "package instructions\n\nimport \"booking/internal/config\"\n\n\
             func accepted(hours int) bool {\n\treturn hours < config.DocCutoff\n}\n",
        ),
        (
            "legacy/defaults.go",
            "package legacy\n\nconst DocCutoff = 1\n",
        ),
        (
            "cs/Config/Limits.cs",
            "namespace App.Config\n{\n    public static class Limits\n    {\n        \
             public const int MAX = 3;\n    }\n}\n",
        ),
        (
            "cs/Other/Rates.cs",
            "namespace App.Other\n{\n    public static class Rates\n    {\n        \
             public const int MAX = 9;\n    }\n}\n",
        ),
        (
            "cs/Billing/Bill.cs",
            "using App.Config;\n\nnamespace App.Billing\n{\n    class Bill\n    {\n        \
             int Total()\n        {\n            return Limits.MAX;\n        }\n    }\n}\n",
        ),
        (
            "java/Two.java",
            "class Beta {\n    static final int MAX = 2;\n}\n\nclass Alpha {\n    \
             static final int MAX = 1;\n\n    int own() {\n        return this.MAX;\n    }\n}\n",
        ),
        ("py/limits.py", "MAX = 3\n"),
        (
            "py/use.py",
            "from limits import MAX\n\n\ndef allowed(n):\n    return n < MAX and n < RATE\n",
        ),
        // A name used as a value is looked for beside it or through an import:
        // `RATE` of `use.py` is no constant of another directory.
        ("other/rates.py", "RATE = 5\n"),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db_path = index.path().join("index.db");
    let config = DatabaseConfig {
        path: db_path.to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    };
    CodebasePipeline::new(PipelineConfig {
        database: config.clone(),
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db_path).expect("open the index");
    let own: Vec<u32> = conn
        .prepare(
            "SELECT t.start_line FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'references' AND s.name = 'own'",
        )
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("read");
    assert_eq!(own, [6], "Alpha's own MAX, not Beta's");
    let mut stmt = conn
        .prepare(
            "SELECT s.name, t.name, t.file_path FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type = 'references'",
        )
        .expect("prepare");
    let mut references: Vec<(String, String, String)> = stmt
        .query_map([], |row| {
            let file: String = row.get(2)?;
            let parts: Vec<&str> = file.rsplit(['/', '\\']).take(2).collect();
            Ok((
                row.get(0)?,
                row.get(1)?,
                format!("{}/{}", parts[1], parts[0]),
            ))
        })
        .expect("query references")
        .collect::<Result<_, _>>()
        .expect("read references");
    references.sort();
    let reference = |source: &str, target: &str, file: &str| {
        (source.to_string(), target.to_string(), file.to_string())
    };
    assert_eq!(
        references,
        [
            reference("Total", "MAX", "Config/Limits.cs"),
            reference("accepted", "DocCutoff", "config/defaults.go"),
            reference("allowed", "MAX", "py/limits.py"),
            reference("own", "MAX", "java/Two.java"),
            reference("validate", "MAX_CELSIUS", "reefer/ReeferLimits.kt"),
        ]
    );

    let db = devscriptor_core::database::Database::new(config).expect("open the index");
    // The folder by its real path, which the index stores.
    let reefer = dunce::canonicalize(project.path().join("reefer")).unwrap();
    let sites = db
        .get_reference_sites(&reefer, &["MAX_CELSIUS".to_string()])
        .await
        .expect("query the use sites");
    assert!(
        sites
            .iter()
            .any(|(file, line, _)| Path::new(file).ends_with("reefer/SetPoint.kt") && *line == 4),
        "{sites:?}"
    );
    let readers = db
        .get_sources_referring_to(&reefer, &["MAX_CELSIUS".to_string()])
        .await
        .expect("query the files using it");
    assert!(
        readers
            .iter()
            .any(|(_, file)| Path::new(file).ends_with("reefer/SetPoint.kt")),
        "{readers:?}"
    );
}

/// A loop's variable is bound where the loop names it, and read where it is
/// expanded: the top of a shell script reads `v` on its second line, not on
/// the first. A destructuring at the top of a script binds its names, which
/// it reads after.
#[test]
fn a_loop_binds_its_variable_and_the_body_reads_it() {
    let parsed = parse_content_sync(
        "const [first, second] = pairs();\nconsole.log(first, second);\n",
        "javascript",
        Path::new("setup.test.js"),
    )
    .expect("parse");
    let reads: Vec<(&str, u32)> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::References)
        .map(|r| (r.target_name.as_str(), r.line))
        .collect();
    assert_eq!(reads, [("first", 2), ("second", 2)]);

    let parsed = parse_content_sync(
        "for v in 1 2; do\n  deploy \"$v\"\ndone\n",
        "bash",
        Path::new("deploy.sh"),
    )
    .expect("parse");
    let reads: Vec<(&str, u32)> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::References)
        .map(|r| (r.target_name.as_str(), r.line))
        .collect();
    assert_eq!(reads, [("v", 2)]);
}

/// An interface that nothing names inherits the interface it extends in
/// another file: resolution reads the extending type's own kind, which the
/// definitions the chunk's references name hold only when something refers to
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interface_nothing_names_inherits_the_interface_it_extends() {
    let project = TempDir::new().expect("tempdir for the project");
    for (file, source) in [
        (
            "tool/toolContract.ts",
            "export interface AgentTool {\n  run(): void;\n}\n",
        ),
        (
            "tools/bash.ts",
            "import { AgentTool } from '../tool/toolContract';\n\n\
             export interface IBashTool extends AgentTool {\n  command: string;\n}\n",
        ),
    ] {
        let path = project.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(project.path())
    .await
    .expect("scan the project");

    let conn = rusqlite::Connection::open(&db).expect("open the index");
    let edges: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT s.name, r.relationship_type, t.name FROM relationships r \
             JOIN entities s ON s.id = r.source_id \
             JOIN entities t ON t.id = r.target_id \
             WHERE r.relationship_type IN ('inherits', 'implements')",
        )
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("read");
    assert_eq!(
        edges,
        [(
            "IBashTool".to_string(),
            "inherits".to_string(),
            "AgentTool".to_string()
        )]
    );
}
