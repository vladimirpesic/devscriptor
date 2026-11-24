use super::*;

fn create_test_tree(content: &str, language: &str) -> Option<tree_sitter::Tree> {
    let ts_language = crate::parser::language_loader::get_language(language).ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_language).ok()?;
    parser.parse(content, None)
}

#[test]
fn test_extract_node_name() {
    let content = "def hello(): pass";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping test: Python language not available");
        return;
    };
    let root = tree.root_node();
    let func = root.child(0).unwrap();
    assert_eq!(
        extract_node_name(func, content, "python"),
        Some("hello".to_string())
    );
}

#[test]
fn test_extract_function() {
    let content = "def hello():\n    pass";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping test: Python language not available");
        return;
    };
    let func = tree.root_node().child(0).unwrap();
    let config = get_language_config("python");

    let now = OffsetDateTime::now_utc();
    let result = extract_function(func, content, "python", "test.py", config, now).unwrap();
    assert!(result.is_some());

    if let Some(Entity::Function(f)) = result {
        assert_eq!(f.name, "hello");
        assert_eq!(f.language, "python");
        assert_eq!(f.file_path, "test.py");
    } else {
        panic!("Expected Function entity");
    }
}

/// C/C++/CUDA free (non-member) functions nest their name under
/// `function_definition → function_declarator → identifier`, which
/// `extract_node_name` reads: C files are almost all free functions.
#[test]
fn test_extract_c_family_free_function() {
    let content = "int add(int a, int b) {\n    return a + b;\n}";
    for lang in ["c", "cpp", "cuda"] {
        let Some(tree) = create_test_tree(content, lang) else {
            eprintln!("Skipping test: {lang} language not available");
            continue;
        };
        let func = tree.root_node().child(0).unwrap();
        assert_eq!(
            func.kind(),
            "function_definition",
            "{lang}: unexpected root child"
        );
        assert_eq!(
            extract_node_name(func, content, lang),
            Some("add".to_string()),
            "{lang}: free-function name must extract from the nested declarator"
        );

        let config = get_language_config(lang);
        let now = OffsetDateTime::now_utc();
        let result = extract_function(func, content, lang, "t.c", config, now).unwrap();
        match result {
            Some(Entity::Function(f)) => assert_eq!(f.name, "add", "{lang}: wrong name"),
            other => panic!("{lang}: expected Function 'add', got {other:?}"),
        }
    }
}

#[test]
fn test_extract_class() {
    let content = "class MyClass:\n    pass";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping test: Python language not available");
        return;
    };
    let class_node = tree.root_node().child(0).unwrap();

    let now = OffsetDateTime::now_utc();
    let result = extract_class(class_node, content, "python", "test.py", now).unwrap();
    assert!(result.is_some());

    if let Some(Entity::Class(c)) = result {
        assert_eq!(c.name, "MyClass");
    } else {
        panic!("Expected Class entity");
    }
}

#[test]
fn test_node_text() {
    let content = "def hello(): pass";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping test: Python language not available");
        return;
    };
    let root = tree.root_node();
    let func = root.child(0).unwrap();
    assert_eq!(node_text(func, content), content);
}

#[test]
fn test_get_source_location() {
    let content = "def hello():\n    pass";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping test: Python language not available");
        return;
    };
    let func = tree.root_node().child(0).unwrap();
    let loc = get_source_location(func);
    assert_eq!(loc.start_line, 1);
    assert_eq!(loc.start_column, 0);
}

// ── Constant over-extraction guard ────────────────────────────────────────

fn print_tree(node: tree_sitter::Node, content: &str, indent: usize) {
    let prefix = " ".repeat(indent * 2);
    let kind = node.kind();
    let field = if let Some(parent) = node.parent() {
        let mut c = parent.walk();
        let mut field_name = None;
        for (i, ch) in parent.children(&mut c).enumerate() {
            if ch.id() == node.id() {
                field_name = parent.field_name_for_child(i as u32).map(|s| s.to_string());
                break;
            }
        }
        field_name.map(|f| format!(" [{f}]")).unwrap_or_default()
    } else {
        String::new()
    };
    let text = if node.child_count() == 0 {
        format!(" = {:?}", &content[node.start_byte()..node.end_byte()])
    } else {
        String::new()
    };
    eprintln!("{prefix}({kind}){field}{text}");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        print_tree(child, content, indent + 1);
    }
}

#[test]
fn debug_python_tree_structure() {
    let content = "MAX = 100\n";
    let Some(tree) = create_test_tree(content, "python") else {
        return;
    };
    print_tree(tree.root_node(), content, 0);
    let entities = extract_entities(&tree, content, "python", "test.py").unwrap_or_default();
    eprintln!("entities: {}", entities.len());
    for e in &entities {
        eprintln!("  {:?}", std::mem::discriminant(e));
    }
}

/// The entities of a Dart file as `(type, name, owning type)`, sorted;
/// `None` when the Dart grammar is not built in.
fn dart_entities(content: &str) -> Option<Vec<(&'static str, String, Option<String>)>> {
    let Some(tree) = create_test_tree(content, "dart") else {
        eprintln!("Skipping: Dart not available");
        return None;
    };
    let mut entities: Vec<_> = extract_entities(&tree, content, "dart", "lib/cart.dart")
        .unwrap()
        .iter()
        .map(|entity| {
            let owner = match entity {
                Entity::Function(f) => f.parent_class.clone(),
                _ => None,
            };
            (entity.entity_type(), entity.name().to_string(), owner)
        })
        .collect();
    entities.sort();
    Some(entities)
}

fn free(kind: &'static str, name: &str) -> (&'static str, String, Option<String>) {
    (kind, name.to_string(), None)
}

fn member(name: &str, owner: &str) -> (&'static str, String, Option<String>) {
    ("function", name.to_string(), Some(owner.to_string()))
}

/// The Dart configuration names the node kinds of the Dart grammar in use:
/// a file of two classes — the cart model of a Flutter app — yields its
/// classes, getters and constructors.
#[test]
fn dart_classes_and_their_members_are_extracted() {
    let content = "\
class CartLine {
  final String sku;
  final int quantity;
  const CartLine(this.sku, this.quantity);
  int get totalCents => quantity * 100;
}

class CartModel {
  static const int bulkThreshold = 12;
  final List<CartLine> lines;
  CartModel(this.lines);
  String? get bulkHint {
    final missing = bulkThreshold - lines.length;
    return missing > 0 ? 'Add $missing more' : null;
  }
}
";
    let Some(entities) = dart_entities(content) else {
        return;
    };
    let mut expected = vec![
        free("class", "CartLine"),
        member("CartLine", "CartLine"),
        member("totalCents", "CartLine"),
        free("class", "CartModel"),
        free("constant", "bulkThreshold"),
        member("CartModel", "CartModel"),
        member("bulkHint", "CartModel"),
    ];
    expected.sort();
    assert_eq!(entities, expected);

    // A member is its whole declaration, body included.
    let tree = create_test_tree(content, "dart").unwrap();
    let entities = extract_entities(&tree, content, "dart", "lib/cart.dart").unwrap();
    let hint = entities.iter().find(|e| e.name() == "bulkHint").unwrap();
    assert!(
        hint.body().contains("'Add $missing more'"),
        "{}",
        hint.body()
    );
}

/// Every kind of Dart declaration, named as Dart names it: a named
/// constructor `Point.zero`, an operator `operator +`, a library by its
/// dotted name. An extension type's name and representation are not
/// extensions of their own, and neither fields nor locals are entities.
#[test]
fn dart_declarations_are_named_as_dart_names_them() {
    let content = "\
library shapes.geometry;
typedef IntList = List<int>;
typedef int Compare(Object a, Object b);
const int maxItems = 10;
var counter = 0;
int get answer => 42;
set answer(int v) {}
external void nativeCall();
int twice(int x) {
  int helper(int y) => y * 2;
  return helper(x);
}
mixin Walker {
  void walk() {}
}
abstract class Shape {
  double area();
}
class Point extends Shape with Walker {
  final int x, y;
  Point(this.x, this.y) : assert(x >= 0);
  Point.zero() : this(0, 0);
  factory Point.fromJson(Map<String, int> j) => Point(j['x']!, j['y']!);
  factory Point.origin() = Point.zero;
  Point operator +(Point o) => Point(x + o.x, y + o.y);
  set label(String v) {}
  static Point parse(String s) => Point.zero();
  double area() => 0;
}
class Mixed = Shape with Walker;
enum Color { red, green }
extension StringX on String {
  bool get isBlank => trim().isEmpty;
}
extension type UserId(int value) {
  bool get valid => value > 0;
}
";
    let Some(entities) = dart_entities(content) else {
        return;
    };
    let mut expected = vec![
        free("module", "shapes.geometry"),
        free("type_alias", "IntList"),
        free("type_alias", "Compare"),
        free("constant", "maxItems"),
        free("function", "answer"),
        free("function", "answer"),
        free("function", "nativeCall"),
        free("function", "twice"),
        free("function", "helper"),
        free("trait", "Walker"),
        member("walk", "Walker"),
        free("class", "Shape"),
        member("area", "Shape"),
        free("class", "Point"),
        member("Point", "Point"),
        member("Point.zero", "Point"),
        member("Point.fromJson", "Point"),
        member("Point.origin", "Point"),
        member("operator +", "Point"),
        member("label", "Point"),
        member("parse", "Point"),
        member("area", "Point"),
        free("class", "Mixed"),
        free("enum", "Color"),
        free("extension", "StringX"),
        member("isBlank", "StringX"),
        free("class", "UserId"),
        member("valid", "UserId"),
    ];
    expected.sort();
    assert_eq!(entities, expected);
}

/// An enum's variants are read from its body, where most grammars keep
/// them, not from the enum node's own children: Dart, Java and Rust alike.
#[test]
fn enum_variants_are_read_from_the_body() {
    for (language, content, variants) in [
        ("dart", "enum Color { red, green }", ["red", "green"]),
        ("java", "enum Color { RED, GREEN }", ["RED", "GREEN"]),
        ("rust", "enum Color { Red, Green }", ["Red", "Green"]),
    ] {
        let Some(tree) = create_test_tree(content, language) else {
            eprintln!("Skipping: {language} not available");
            continue;
        };
        let entities = extract_entities(&tree, content, language, "color").unwrap();
        let Some(Entity::Enum(color)) = entities.iter().find(|e| e.name() == "Color") else {
            panic!("{language}: no enum Color in {entities:?}");
        };
        assert_eq!(
            color.variants,
            Some(serde_json::json!(variants)),
            "{language}"
        );
    }
}

fn count_constants(entities: &[Entity]) -> usize {
    entities
        .iter()
        .filter(|e| matches!(e, Entity::Constant(_)))
        .count()
}

/// Python: local assignments inside a function must NOT become Constants;
/// only UPPER_CASE module-level assignments do.
#[test]
fn test_python_local_assignments_are_not_constants() {
    let content = "\
MAX = 100
def foo():
    x = 1
    y = 2
    z = 3
    return x + y + z
";
    let Some(tree) = create_test_tree(content, "python") else {
        eprintln!("Skipping: Python not available");
        return;
    };
    let entities = extract_entities(&tree, content, "python", "test.py").unwrap_or_default();
    let constants = count_constants(&entities);
    assert_eq!(constants, 1, "expected only MAX, got {constants}");
}

/// The names of the constants extracted from `content` in `language`.
fn constant_names(content: &str, language: &str, file: &str) -> Option<Vec<String>> {
    let tree = create_test_tree(content, language)?;
    let entities = extract_entities(&tree, content, language, file).unwrap_or_default();
    Some(
        entities
            .iter()
            .filter_map(|e| match e {
                Entity::Constant(c) => Some(c.name.clone()),
                _ => None,
            })
            .collect(),
    )
}

/// A Kotlin `const val` or `val`, at the top level or in an `object` or a
/// companion object, is a constant, as a Scala `val` is; a `var`, and a
/// local in a function, are not. Each is named, so `ReeferLimits.MAX_CELSIUS`
/// can be looked up by name.
#[test]
fn kotlin_const_vals_are_constants() {
    let content = "\
const val TOP_LIMIT = 10

object ReeferLimits {
    const val MIN_CELSIUS = -30.0
    const val MAX_CELSIUS = 30.0
    val label = \"reefer\"
}

class Gate {
    companion object {
        const val DOORS = 2
    }
    var open = false
    fun check(): Int {
        val local = 1
        return local
    }
}
";
    let Some(names) = constant_names(content, "kotlin", "Limits.kt") else {
        eprintln!("Skipping: Kotlin not available");
        return;
    };
    assert_eq!(
        names,
        ["TOP_LIMIT", "MIN_CELSIUS", "MAX_CELSIUS", "label", "DOORS"]
    );
}

/// A JavaScript module's `const` declarations are its constants; the
/// locals of a function are not.
#[test]
fn javascript_module_constants_are_extracted() {
    let content = "\
const MAX = 100;
function foo() {
  const x = 1;
  const y = 2;
  return x + y;
}
";
    let Some(names) = constant_names(content, "javascript", "test.js") else {
        eprintln!("Skipping: JavaScript not available");
        return;
    };
    assert_eq!(names, ["MAX"]);
}

/// Exported or not, each declarator of a top-level `const` is a constant,
/// with its value and type; functions, classes, `let`, `var`, patterns and
/// block-local declarations are not.
#[test]
fn typescript_module_constants_are_extracted() {
    let content = "\
export const LIMIT: number = 5;
const A = 1, B = 'two';
const handler = () => 1;
const Widget = class {};
let counter = 0;
var legacy = 1;
const { left, right } = pair;
if (ready) {
  const LOCAL = 3;
}
const tools = { sum: (a: number, b: number) => a + b };
module.exports.total = function () { return 1; };
[1, 2].map(x => x * 2);
class Store { reset = () => 0; }
";
    let Some(tree) = create_test_tree(content, "typescript") else {
        eprintln!("Skipping: TypeScript not available");
        return;
    };
    let entities = extract_entities(&tree, content, "typescript", "test.ts").unwrap_or_default();
    let constants: Vec<&Constant> = entities
        .iter()
        .filter_map(|e| match e {
            Entity::Constant(c) => Some(c),
            _ => None,
        })
        .collect();
    let names: Vec<&str> = constants.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["LIMIT", "A", "B", "tools"]);
    assert_eq!(constants[0].constant_type.as_deref(), Some("number"));
    assert_eq!(constants[0].value.as_deref(), Some("5"));
    assert_eq!(constants[2].value.as_deref(), Some("'two'"));
    let functions: Vec<&str> = entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect();
    // Functions are named by what they are bound to; a callback bound to
    // nothing is not indexed, and never named after its parameter.
    for bound in ["handler", "sum", "total", "reset"] {
        assert!(functions.contains(&bound), "{bound} in {functions:?}");
    }
    assert!(!functions.contains(&"x"), "{functions:?}");
}
