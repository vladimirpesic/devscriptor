//! `inherits` (subclassing) and `implements` (interface/trait/protocol
//! conformance) relationships are extracted across every grammar: Rust
//! `impl Trait`, TypeScript `extends`/`implements`, Kotlin `:`, Swift `:`, Java
//! `extends` and the rest, not just Python and Java.

use std::collections::HashSet;
use std::path::Path;

use devscriptor_core::parser::parse_content_sync;

/// Parse `content` as `language` and return the set of hierarchy edges as
/// `(child_name, rel_type, parent_name)` where `rel_type` is "inherits" or
/// "implements". Reverse edges (`inherited_by`/`implemented_by`) are ignored.
fn hierarchy_edges(content: &str, language: &str, file: &str) -> HashSet<(String, String, String)> {
    let parsed = parse_content_sync(content, language, Path::new(file))
        .unwrap_or_else(|e| panic!("parse failed for {language}: {e}"));

    let name_of = |id| {
        parsed
            .entities
            .iter()
            .find(|e| e.id() == id)
            .map(|e| e.name().to_string())
    };

    parsed
        .relationships
        .iter()
        .filter_map(|r| {
            let ty = r.relationship_type.as_str();
            if ty != "inherits" && ty != "implements" {
                return None;
            }
            Some((name_of(r.source_id)?, ty.to_string(), name_of(r.target_id)?))
        })
        .collect()
}

fn assert_edge(edges: &HashSet<(String, String, String)>, child: &str, ty: &str, parent: &str) {
    let want = (child.to_string(), ty.to_string(), parent.to_string());
    assert!(
        edges.contains(&want),
        "expected {child} --{ty}--> {parent}, got: {edges:?}"
    );
}

#[test]
fn rust_impl_trait_yields_implements() {
    let src = r"
trait Shape { fn area(&self) -> f64; }
struct Circle { r: f64 }
impl Shape for Circle { fn area(&self) -> f64 { 3.14 * self.r * self.r } }
";
    let edges = hierarchy_edges(src, "rust", "shapes.rs");
    assert_edge(&edges, "Circle", "implements", "Shape");
}

#[test]
fn typescript_extends_and_implements() {
    let src = r"
interface Drawable { draw(): void; }
class Base {}
class Widget extends Base implements Drawable { draw(): void {} }
";
    let edges = hierarchy_edges(src, "typescript", "widget.ts");
    assert_edge(&edges, "Widget", "inherits", "Base");
    assert_edge(&edges, "Widget", "implements", "Drawable");
}

#[test]
fn kotlin_supertype_list() {
    let src = "\
interface Clickable {
    fun click()
}
open class View {
}
class Button : View(), Clickable {
    fun click() {}
}
";
    let edges = hierarchy_edges(src, "kotlin", "Button.kt");
    assert_edge(&edges, "Button", "inherits", "View");
    assert_edge(&edges, "Button", "implements", "Clickable");
}

#[test]
fn swift_inheritance_clause() {
    let src = r"
protocol Animal { func sound() }
class Base {}
class Dog: Base, Animal { func sound() {} }
";
    let edges = hierarchy_edges(src, "swift", "Dog.swift");
    assert_edge(&edges, "Dog", "inherits", "Base");
    assert_edge(&edges, "Dog", "implements", "Animal");
}

#[test]
fn java_extends_and_implements() {
    let src = r"
interface Task { void run(); }
class Base {}
class Worker extends Base implements Task { public void run() {} }
";
    let edges = hierarchy_edges(src, "java", "Worker.java");
    assert_edge(&edges, "Worker", "inherits", "Base");
    assert_edge(&edges, "Worker", "implements", "Task");
}

#[test]
fn python_subclass_inherits() {
    let src = r"
class Animal:
    pass

class Dog(Animal):
    pass
";
    let edges = hierarchy_edges(src, "python", "dog.py");
    assert_edge(&edges, "Dog", "inherits", "Animal");
}

/// Dart: `extends` inherits, `with` mixes in a mixin (a trait), and every type
/// after `implements` is implemented — a class included, since Dart classes
/// are implicit interfaces.
#[test]
fn dart_extends_with_and_implements() {
    let src = r"
mixin Walker {}
abstract class Repo { void save(); }
class Shape {}
class Point extends Shape with Walker implements Repo {
  void save() {}
}
";
    let edges = hierarchy_edges(src, "dart", "point.dart");
    assert_edge(&edges, "Point", "inherits", "Shape");
    assert_edge(&edges, "Point", "implements", "Walker");
    assert_edge(&edges, "Point", "implements", "Repo");
}

/// TypeScript lets a class implement a class; the edge says so.
#[test]
fn typescript_implements_a_class() {
    let src = r"
class Base { run(): void {} }
class Fake implements Base { run(): void {} }
";
    let edges = hierarchy_edges(src, "typescript", "fake.ts");
    assert_edge(&edges, "Fake", "implements", "Base");
}
