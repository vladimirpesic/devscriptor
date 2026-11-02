//! Dead code is what nothing reaches: no call, reference, use or supertype,
//! and no runtime, framework or test runner either. What it is reported with
//! says how sure that is: a private entity 0.9, one its language exports 0.5,
//! and one whose name is written outside its definition 0.3.

use devscriptor_core::analysis::DeadCodeDetector;
use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

/// Scan `files`, a project in the directory `home` of a fresh temporary one,
/// into a fresh index and return the findings of the dead code under `scope`
/// as `(name, confidence)`, sorted.
async fn dead_code(home: &str, files: &[(&str, &str)], scope: &str) -> Vec<(String, f64)> {
    let temporary = TempDir::new().expect("tempdir for the project");
    let project = temporary.path().join(home);
    for (file, source) in files {
        let path = project.join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, source).expect("write");
    }
    let root = dunce::canonicalize(project).expect("canonical root");
    let index = TempDir::new().expect("tempdir for the index");
    let config = DatabaseConfig {
        path: index.path().join("index.db").to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    };
    CodebasePipeline::new(PipelineConfig {
        database: config.clone(),
        enable_cross_file: true,
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
    .analyze_directory(&root)
    .await
    .expect("scan the project");
    let database = std::sync::Arc::new(Database::new(config).expect("open the index"));
    let report = DeadCodeDetector::new(database)
        .within(root.join(scope))
        .analyze()
        .await
        .expect("analyze");
    let mut found: Vec<(String, f64)> = report
        .dead_functions
        .iter()
        .chain(report.dead_classes.iter())
        .map(|entity| (entity.name.clone(), entity.confidence))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// What a test runner, a framework or the runtime calls is no dead code; a
/// private helper nothing calls is, surely; an exported one less surely; one
/// whose name another file writes — a script running it as a program, here —
/// least; a script's own function its top level calls is reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_code_is_what_nothing_reaches() {
    let files = [
        // Go: tests, and a handler registered by value.
        (
            "orders/totals_test.go",
            "package orders\n\nfunc TestTotal(t *testing.T) {}\n",
        ),
        (
            "orders/api.go",
            "package orders\n\nfunc handleCheckout(w int) {}\n\
             func Register(mux *Mux) { mux.HandleFunc(\"/checkout\", handleCheckout) }\n",
        ),
        // Python: a route, a test, a dunder, a private helper nobody calls.
        (
            "service/dispatching.py",
            "@app.get(\"/dispatch\")\ndef dispatch():\n    return 1\n\n\
             class Order:\n    def __str__(self):\n        return \"order\"\n\n\
             def _unused_helper():\n    return 2\n",
        ),
        (
            "tests/test_views.py",
            "def test_dispatch():\n    assert True\n",
        ),
        // A view a URL configuration names, which this project does not hold,
        // and a getter, read as a property.
        (
            "catalog/views.py",
            "def product_detail(request):\n    return 3\n",
        ),
        (
            "web/cart.js",
            "export class Cart {\n  get total() { return 0; }\n}\n",
        ),
        // Kotlin: an override, which its base type calls; Java: a class used
        // only through a static method.
        (
            "ui/Panel.kt",
            "class Panel : View() {\n    override fun render() {}\n}\n",
        ),
        (
            "src/Money.java",
            "class Money {\n    public static int round(int cents) { return cents; }\n}\n",
        ),
        (
            "src/Totals.java",
            "class Totals {\n    int total() { return Money.round(5); }\n}\n",
        ),
        // Rust: an exported function nobody calls, and a binary's entry,
        // which a release script runs: a program, not a call of the function.
        // The script's own function, which its top level calls, is reached.
        ("lib/src/lib.rs", "pub fn unused_api() -> u32 { 1 }\n"),
        ("lib/src/report.rs", "pub fn export_report() -> u32 { 2 }\n"),
        (
            "scripts/retention.sh",
            "prune_old_backups() {\n  echo pruning\n}\n",
        ),
        (
            "scripts/backup.sh",
            "#!/bin/sh\n. ./retention.sh\nprune_old_backups\n./target/release/export_report --all\n",
        ),
    ];
    let found = dead_code(".", &files, ".").await;
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Totals",
            "_unused_helper",
            "export_report",
            "total",
            "unused_api"
        ],
        "{found:?}"
    );
    let confidence = |name: &str| {
        found
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, confidence)| *confidence)
    };
    assert_eq!(confidence("_unused_helper"), Some(0.9));
    assert_eq!(confidence("unused_api"), Some(0.5));
    assert_eq!(confidence("export_report"), Some(0.3));

    // Scoped to one directory, only its entities are reported; what uses
    // them, and where their names are written, is looked for everywhere.
    let found = dead_code(".", &files, "lib").await;
    assert_eq!(
        found,
        [
            ("export_report".to_string(), 0.3),
            ("unused_api".to_string(), 0.5)
        ]
    );
}

/// A decorator, annotation or attribute registers what it marks — a route, a
/// test — unless it only describes it: `@dataclass`, `lru_cache`, `allow`,
/// read by the last segment of its name, are no registration. And a test is
/// named as one, not merely begun with its letters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decorator_registers_unless_it_only_describes() {
    let files = [
        (
            "billing/ledger.py",
            "import functools\nfrom dataclasses import dataclass\n\n\
             @dataclass\nclass _Entry:\n    cents: int\n\n\
             @functools.lru_cache(maxsize=1)\ndef _rates():\n    return {}\n\n\
             def testimonials():\n    return []\n",
        ),
        (
            "billing/src/lib.rs",
            "#[allow(dead_code)]\nfn quiet() {}\n\n#[tokio::test]\nasync fn checks() {}\n",
        ),
        (
            "billing/Refunds.php",
            "<?php\nclass Refunds {\n    #[Route('/refunds')]\n    \
             public function index() {}\n    private function audit() {}\n}\n",
        ),
    ];
    let found = dead_code(".", &files, ".").await;
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["_Entry", "_rates", "audit", "quiet", "testimonials"],
        "{found:?}"
    );
}

/// A project's own directories say what its files are; the directories above
/// it do not: a project in `…/test/app/` holds no tests and no framework's
/// role files for being there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_project_s_own_directories_say_what_its_files_are() {
    let files = [
        ("shop/pricing.py", "def unused_discount():\n    return 0\n"),
        (
            "tests/test_pricing.py",
            "def helper_for_tests():\n    return 1\n",
        ),
    ];
    let found = dead_code("test/app", &files, ".").await;
    assert_eq!(found, [("unused_discount".to_string(), 0.5)]);
}

/// A member is reached through the type it overrides or implements: a call
/// of `Shape.area` may run `Circle.area`. One that implements a member of a
/// type the index does not hold is reached by the code that calls it through
/// that type (Rust's `fmt` of `impl Display`, `drop` of `impl Drop`); one
/// that may — a Python method of a standard library's handler — scores 0.3,
/// unless its language marks every implementation, as Rust's does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_is_reached_through_the_type_it_implements() {
    let files = [
        (
            "shapes/Shape.java",
            "interface Shape {\n    double area();\n}\n",
        ),
        (
            "shapes/Circle.java",
            "class Circle implements Shape {\n    public double area() { return 1; }\n    \
             public double radius() { return 2; }\n}\n",
        ),
        (
            "shapes/Report.java",
            "public class Report {\n    public static void main(String[] args) {\n        \
             Shape shape = new Circle();\n        System.out.println(shape.area());\n    }\n}\n",
        ),
        (
            "money/src/cents.rs",
            "pub struct Cents(i64);\n\
             impl std::fmt::Display for Cents {\n    \
             fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, \"{}\", self.0) }\n}\n\
             impl Drop for Cents {\n    fn drop(&mut self) {}\n}\n\
             impl Cents {\n    fn unused_inner(&self) -> i64 { self.0 }\n}\n",
        ),
        (
            "web/serving.py",
            "from http.server import BaseHTTPRequestHandler, HTTPServer\n\n\
             class Ping(BaseHTTPRequestHandler):\n    def do_GET(self):\n        return 1\n\n\
             def main():\n    HTTPServer(('', 8000), Ping).serve_forever()\n",
        ),
    ];
    let found = dead_code(".", &files, ".").await;
    assert_eq!(
        found,
        [
            ("Ping".to_string(), 0.3),
            ("do_GET".to_string(), 0.3),
            ("radius".to_string(), 0.5),
            ("unused_inner".to_string(), 0.9),
        ]
    );
}

/// A method a language's runtime calls through a protocol no supertype names
/// is no dead code: Go's `String` and `MarshalJSON`, met by a type rather
/// than declared; Ruby's `to_s` and `<=>`; JavaScript's `toJSON`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_protocol_method_is_reached_by_its_runtime() {
    let files = [
        (
            "money/cents.go",
            "package money\n\ntype Cents int\n\n\
             func (c Cents) String() string { return \"\" }\n\
             func (c Cents) MarshalJSON() ([]byte, error) { return nil, nil }\n\
             func (c Cents) rounded() Cents { return c }\n",
        ),
        (
            "money/cents.rb",
            "class Cents\n  def to_s\n    ''\n  end\n\n  def <=>(other)\n    0\n  end\n\n  \
             def rounded\n    self\n  end\nend\n",
        ),
        (
            "money/cents.js",
            "export class Cents {\n  toJSON() { return 0; }\n  rounded() { return this; }\n}\n",
        ),
    ];
    let found = dead_code(".", &files, ".").await;
    let names: Vec<&str> = found.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["rounded", "rounded", "rounded"], "{found:?}");
}
