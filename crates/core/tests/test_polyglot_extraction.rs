//! Polyglot extraction: Solidity interface conformance, SQL table/view + FK
//! extraction, the scripts embedded in Svelte and Vue components, GraphQL type
//! references, and GraphQL operations.

use std::collections::HashSet;
use std::path::Path;

use devscriptor_core::entities::{Entity, RelationshipType};
use devscriptor_core::parser::parse_content_sync;

fn entities(content: &str, lang: &str, file: &str) -> Vec<(String, String, String)> {
    let p = parse_content_sync(content, lang, Path::new(file)).expect("parse");
    p.entities
        .iter()
        .map(|e| {
            (
                e.name().to_string(),
                e.entity_type().to_string(),
                e.language().to_string(),
            )
        })
        .collect()
}

fn hierarchy_edges(content: &str, lang: &str, file: &str) -> HashSet<(String, String, String)> {
    let p = parse_content_sync(content, lang, Path::new(file)).expect("parse");
    let name_of = |id| {
        p.entities
            .iter()
            .find(|e| e.id() == id)
            .map(|e| e.name().to_string())
    };
    p.relationships
        .iter()
        .filter_map(|r| {
            let ty = r.relationship_type.as_str();
            Some((name_of(r.source_id)?, ty.to_string(), name_of(r.target_id)?))
        })
        .collect()
}

// ---------------------------------------------------------------- Solidity ----

#[test]
fn solidity_interface_is_implements_not_inherits() {
    let src = "\
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;
interface IERC20 { function transfer(address to, uint256 v) external returns (bool); }
contract Base { address public owner; }
contract Token is Base, IERC20 {
    function transfer(address to, uint256 v) external returns (bool) { return true; }
}
";
    let ents = entities(src, "solidity", "Token.sol");
    // IERC20 must be classified as an interface, not a class.
    assert!(
        ents.iter()
            .any(|(n, t, _)| n == "IERC20" && t == "interface"),
        "IERC20 should be an interface, got: {ents:?}"
    );

    let edges = hierarchy_edges(src, "solidity", "Token.sol");
    assert!(
        edges.contains(&("Token".into(), "inherits".into(), "Base".into())),
        "Token should inherit contract Base: {edges:?}"
    );
    assert!(
        edges.contains(&("Token".into(), "implements".into(), "IERC20".into())),
        "Token should implement interface IERC20: {edges:?}"
    );
}

// --------------------------------------------------------------------- SQL ----

#[test]
fn sql_tables_views_and_foreign_keys() {
    let src = "\
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL
);
CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER REFERENCES users(id)
);
CREATE VIEW active AS SELECT * FROM users;
";
    let ents = entities(src, "sql", "schema.sql");
    // Tables and the view must be extracted, typed `table` (not `class`/`struct`).
    for want in ["users", "posts", "active"] {
        assert!(
            ents.iter().any(|(n, t, _)| n == want && t == "table"),
            "SQL entity '{want}' should be a `table`: {ents:?}"
        );
    }
    // Columns are folded into their table's body — not standalone `struct`s.
    assert!(
        !ents.iter().any(|(_, t, _)| t == "struct"),
        "SQL columns should not surface as struct entities: {ents:?}"
    );
    assert!(
        !ents.iter().any(|(n, _, _)| n == "email" || n == "user_id"),
        "column names should not be standalone entities: {ents:?}"
    );

    // Foreign key posts.user_id -> users is a `uses` relationship.
    let edges = hierarchy_edges(src, "sql", "schema.sql");
    assert!(
        edges.contains(&("posts".into(), "uses".into(), "users".into())),
        "posts should `uses` users via FK: {edges:?}"
    );
}

// ------------------------------------------------------------------ Svelte ----

#[test]
fn svelte_embedded_script_is_extracted() {
    let src = "\
<script>
  export let on = false;
  function toggle() { on = !on; }
  function reset() { on = false; }
</script>
<button on:click={toggle}>{on ? \"ON\" : \"OFF\"}</button>
";
    let ents = entities(src, "svelte", "Toggle.svelte");
    // The functions inside <script> are visible, attributed to svelte.
    assert!(
        ents.iter()
            .any(|(n, t, l)| n == "toggle" && t == "function" && l == "svelte"),
        "svelte <script> function `toggle` should be extracted as svelte: {ents:?}"
    );
    assert!(
        ents.iter().any(|(n, _, _)| n == "reset"),
        "svelte <script> function `reset` should be extracted: {ents:?}"
    );
}

/// `<script lang="ts">` is TypeScript: its interfaces and typed functions
/// are indexed, which a JavaScript parse of it left as errors.
#[test]
fn svelte_typescript_script_is_parsed_as_typescript() {
    let src = "\
<script lang=\"ts\">
  interface Props { label: string }
  type Mode = 'on' | 'off';
  export let props: Props;
  function toggle(mode: Mode): Mode { return mode === 'on' ? 'off' : 'on'; }
</script>
<button>{props.label}</button>
";
    let ents = entities(src, "svelte", "Toggle.svelte");
    for (name, ty) in [
        ("Props", "interface"),
        ("Mode", "type_alias"),
        ("toggle", "function"),
    ] {
        assert!(
            ents.iter()
                .any(|(n, t, l)| n == name && t == ty && l == "svelte"),
            "`{name}` should be a svelte {ty}: {ents:?}"
        );
    }
}

// --------------------------------------------------------------------- Vue ----

/// Vue's grammar, like Svelte's, leaves a `<script>` body as raw text: both
/// blocks of a component are indexed, `<script setup lang="ts">` as
/// TypeScript.
#[test]
fn vue_scripts_are_extracted() {
    let src = "\
<template>
  <button @click=\"reset\">{{ props.label }}</button>
</template>

<script>
function double(x) { return x * 2; }
export default { name: 'Toggle' };
</script>

<script setup lang=\"ts\">
import { ref } from 'vue';
interface Props { label: string }
const props = defineProps<Props>();
const on = ref(false);
function reset(): void { on.value = false; }
</script>
";
    let ents = entities(src, "vue", "Toggle.vue");
    for (name, ty) in [
        ("double", "function"),
        ("reset", "function"),
        ("Props", "interface"),
    ] {
        assert!(
            ents.iter()
                .any(|(n, t, l)| n == name && t == ty && l == "vue"),
            "`{name}` should be a vue {ty}: {ents:?}"
        );
    }
}

/// A Vue or Svelte file is one component, named after the file and spanning
/// it, and its template's elements are not entities, nor its `<script>` a
/// module of the same name.
#[test]
fn a_component_file_is_one_component() {
    let vue = "\
<template>
  <div class=\"order-table\">
    <table><tr v-for=\"order in orders\"><td>{{ order.id }}</td></tr></table>
    <button @click=\"go(page + 1)\">Next</button>
  </div>
</template>

<script setup>
const page = ref(1);
async function go(next) { page.value = next; }
</script>
";
    let svelte = "\
<script>
  export let orders = [];
  function total() { return orders.length; }
</script>

<section>
  <h2>{total()}</h2>
  <ul>{#each orders as order}<li>{order.id}</li>{/each}</ul>
</section>
";
    for (src, lang, file, function) in [
        (vue, "vue", "OrderTable.vue", "go"),
        (svelte, "svelte", "RevenueChart.svelte", "total"),
    ] {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let found: Vec<(&str, String)> = parsed
            .entities
            .iter()
            .map(|e| (e.name(), e.entity_type().to_string()))
            .collect();
        let components: Vec<_> = parsed
            .entities
            .iter()
            .filter(|e| e.entity_type() == "component")
            .collect();
        assert_eq!(components.len(), 1, "{lang}: {found:?}");
        let component = components[0];
        assert_eq!(component.name(), file.split('.').next().unwrap());
        assert_eq!(component.location().start_line, 1, "{lang}");
        assert!(
            component.location().end_line as usize >= src.lines().count(),
            "{lang}: the component ends at line {}",
            component.location().end_line
        );
        assert!(
            !found.iter().any(|(_, ty)| ty == "module"),
            "{lang}: {found:?}"
        );
        assert!(
            found.contains(&(function, "function".to_string())),
            "{lang}: {found:?}"
        );
    }
}

/// A JSX element renders a component; it is not one, and each `<Input />` is
/// not one more `Input` beside the function that defines it. What an element
/// renders is a call.
#[test]
fn jsx_elements_are_calls_not_components() {
    let src = "\
export function Input({ label }: { label: string }) {
  return <label>{label}<input /></label>;
}

export function Form() {
  return (
    <form>
      <Input label=\"Name\" />
      <Input label=\"Email\" />
    </form>
  );
}
";
    let found = entities(src, "tsx", "Form.tsx");
    let named: Vec<(&str, &str)> = found
        .iter()
        .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
        .collect();
    assert_eq!(named, [("Input", "function"), ("Form", "function")]);
    let edges = hierarchy_edges(src, "tsx", "Form.tsx");
    assert!(
        edges.contains(&("Form".into(), "calls".into(), "Input".into())),
        "{edges:?}"
    );
}

/// A union is an entity where it is defined. A TypeScript union type
/// (`Order | null`) joins types defined elsewhere and is no `union Order`; the
/// cases of an F# union are no unions either.
#[test]
fn a_union_type_defines_no_union() {
    let ts = "\
type Shape = Circle | Square;
function f(x: Order | null): string | undefined { return undefined; }
";
    let found = entities(ts, "typescript", "shape.ts");
    let named: Vec<(&str, &str)> = found
        .iter()
        .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
        .collect();
    assert_eq!(named, [("Shape", "type_alias"), ("f", "function")]);

    let fs = "module Shapes\n\ntype Shape =\n    | Circle of float\n    | Square of float\n";
    let found = entities(fs, "fsharp", "Shapes.fs");
    assert!(
        !found
            .iter()
            .any(|(name, _, _)| name == "Circle" || name == "Square"),
        "{found:?}"
    );

    for (src, lang, file, name) in [
        (
            "union Value { int i; float f; };\n",
            "c",
            "value.c",
            "Value",
        ),
        ("union Bits { i: u32, f: f32 }\n", "rust", "bits.rs", "Bits"),
        (
            "union SearchResult = User | Order\n",
            "graphql",
            "schema.graphql",
            "SearchResult",
        ),
    ] {
        let found = entities(src, lang, file);
        assert!(
            found.contains(&(name.into(), "union".into(), lang.into())),
            "{lang}: {found:?}"
        );
    }
}

/// A C-family `struct`, `union`, `enum` or `class` without a body names a type
/// it does not define — a variable's or a parameter's type, a field pointing
/// to its own struct, a forward declaration — and is no second entity of that
/// name beside the definition.
#[test]
fn a_c_type_is_an_entity_where_it_is_defined() {
    let src = "\
struct Point { int x; int y; };
struct Point origin;
union Value { int i; float f; };
union Value v;
enum Color { RED, GREEN };
enum Color paint;
void move(struct Point *p, union Value *v, enum Color c) {}
struct Opaque;
typedef struct Node { struct Node *next; } Node;
";
    for (lang, file) in [
        ("c", "shapes.c"),
        ("cpp", "shapes.cpp"),
        ("cuda", "shapes.cu"),
    ] {
        let found = entities(src, lang, file);
        let named: Vec<(&str, &str)> = found
            .iter()
            .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("Point", "struct"),
                ("Value", "union"),
                ("Color", "enum"),
                ("move", "function"),
                ("Node", "type_alias"),
                ("Node", "struct"),
            ],
            "{lang}"
        );
    }

    let cpp = "class Engine;\nclass Car { Engine *engine; };\n";
    let found = entities(cpp, "cpp", "car.cpp");
    assert!(
        !found.iter().any(|(name, _, _)| name == "Engine"),
        "{found:?}"
    );
    assert!(found.contains(&("Car".into(), "class".into(), "cpp".into())));
}

/// A Go type declaration is what its type is, not a struct for every
/// `type_spec`: `type Reader interface {…}` is an interface, and `type ID
/// string` and `type Handler func(int)` are type aliases.
#[test]
fn a_go_type_is_what_it_declares() {
    let src = "\
package shapes

type Reader interface {
\tRead(p []byte) (int, error)
}

type Point struct {
\tX int
}

type ID string

type Handler func(int)

type (
\tPair   struct{ A, B int }
\tCloser interface{ Close() error }
)

type Alias = Point
";
    let found = entities(src, "go", "shapes.go");
    let named: Vec<(&str, &str)> = found
        .iter()
        .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("Reader", "interface"),
            ("Point", "struct"),
            ("ID", "type_alias"),
            ("Handler", "type_alias"),
            ("Pair", "struct"),
            ("Closer", "interface"),
            ("Alias", "type_alias"),
        ]
    );
}

/// F# types are entities of their own kinds, with their members as functions
/// of them, as are namespaces and file modules. Each definition sits one level
/// under its `type_definition`, which is where its name is looked for; the
/// cases of a union or an enum are no entities of their own.
#[test]
fn fsharp_types_and_their_members_are_indexed() {
    let src = "\
namespace Shop.Orders

type Customer = { Name: string; Age: int }

type Shape =
    | Circle of float
    | Square of float
    member this.Area = 1.0

type Color =
    | Red = 0
    | Green = 1

type Id = int
type Marker = Marker

type IPricing =
    abstract Price: int -> int

[<AbstractClass>]
type Discount =
    abstract Percent: int

type Base() =
    member _.Id = 0

type Pricing(rate: int) =
    inherit Base()
    member _.Rate = rate
    interface IPricing with
        member _.Price(units) = units * rate

exception OutOfStock of string

type Handler = delegate of int -> unit
";
    let parsed = parse_content_sync(src, "fsharp", Path::new("Orders.fs")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    for (name, ty) in [
        ("Shop.Orders", "namespace"),
        ("Customer", "struct"),
        ("Shape", "union"),
        ("Color", "enum"),
        ("Id", "type_alias"),
        ("Marker", "union"),
        ("IPricing", "interface"),
        ("Discount", "class"),
        ("Pricing", "class"),
        ("OutOfStock", "class"),
        ("Handler", "type_alias"),
    ] {
        assert!(
            found.contains(&(name, ty.to_string())),
            "{name} ({ty}): {found:?}"
        );
    }
    assert!(
        !found
            .iter()
            .any(|(name, _)| matches!(*name, "Circle" | "Square" | "Red" | "Green")),
        "{found:?}"
    );

    let members: Vec<(&str, Option<&str>)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some((f.name.as_str(), f.parent_class.as_deref())),
            _ => None,
        })
        .collect();
    for (member, owner) in [
        ("Area", "Shape"),
        ("Price", "IPricing"),
        ("Rate", "Pricing"),
        ("Price", "Pricing"),
    ] {
        assert!(
            members.contains(&(member, Some(owner))),
            "{owner}.{member}: {members:?}"
        );
    }

    let edges = hierarchy_edges(src, "fsharp", "Orders.fs");
    for edge in [
        ("Pricing", "inherits", "Base"),
        ("Pricing", "implements", "IPricing"),
    ] {
        assert!(
            edges.contains(&(edge.0.into(), edge.1.into(), edge.2.into())),
            "{edge:?}: {edges:?}"
        );
    }
}

/// A method names the type it is a member of, in every language: the type
/// holding the body it is in, not that body.
#[test]
fn a_method_names_its_type() {
    let cases = [
        (
            "java",
            "Cart.java",
            "class Cart {\n    int total() { return 0; }\n}\n",
            "total",
            "Cart",
        ),
        (
            "kotlin",
            "Cart.kt",
            "class Cart {\n    companion object {\n        fun empty(): Cart = Cart()\n    }\n}\n",
            "empty",
            "Cart",
        ),
        (
            "c_sharp",
            "Cart.cs",
            "class Cart {\n    int Total() { return 0; }\n}\n",
            "Total",
            "Cart",
        ),
        (
            "typescript",
            "priced.ts",
            "interface Priced { price(): number; }\n",
            "price",
            "Priced",
        ),
        (
            "typescript",
            "cart.ts",
            "class Cart { total = () => 0; }\n",
            "total",
            "Cart",
        ),
        (
            "python",
            "cart.py",
            "class Cart:\n    def total(self):\n        return 0\n",
            "total",
            "Cart",
        ),
        (
            "ruby",
            "cart.rb",
            "class Cart\n  def self.empty; end\nend\n",
            "empty",
            "Cart",
        ),
        (
            "php",
            "cart.php",
            "<?php\nclass Cart {\n    function total() { return 0; }\n}\n",
            "total",
            "Cart",
        ),
        (
            "scala",
            "Cart.scala",
            "class Cart {\n  def total(): Int = 0\n}\n",
            "total",
            "Cart",
        ),
        (
            "swift",
            "Cart.swift",
            "class Cart {\n    func total() -> Int { 0 }\n}\n",
            "total",
            "Cart",
        ),
        (
            "rust",
            "cart.rs",
            "struct Cart;\nimpl Priced for Cart {\n    fn price(&self) -> i32 { 0 }\n}\n",
            "price",
            "Cart",
        ),
        (
            "go",
            "cart.go",
            "package shop\n\ntype Cart struct{}\n\nfunc (c *Cart) Total() int { return 0 }\n",
            "Total",
            "Cart",
        ),
        (
            "cpp",
            "cart.cpp",
            "class Cart {\npublic:\n    int total();\n};\nint Cart::total() { return 0; }\n",
            "total",
            "Cart",
        ),
        (
            "cpp",
            "order.cpp",
            "namespace shop { int Order::count() const { return 1; } }\n",
            "count",
            "Order",
        ),
    ];
    for (lang, file, src, method, owner) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let owners: Vec<Option<&str>> = parsed
            .entities
            .iter()
            .filter_map(|e| match e {
                Entity::Function(f) if f.name == method => Some(f.parent_class.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(owners, [Some(owner)], "{lang}: {method}");
    }

    // A function in a method is a member of no type.
    let src = "\
class Cart {
  total() {
    function helper() { return 1; }
    return helper();
  }
}
";
    let parsed = parse_content_sync(src, "javascript", Path::new("cart.js")).expect("parse");
    let owners: Vec<(&str, Option<&str>)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some((f.name.as_str(), f.parent_class.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(owners, [("total", Some("Cart")), ("helper", None)]);
}

/// A Swift type is what its declaration says: the grammar parses a class,
/// actor, struct, enum and extension alike, and each is indexed as what it is,
/// an extension not a second class of its type's name. Initializers and a
/// protocol's requirements are its functions, and a `let` is a constant, as a
/// `val` or `final` is elsewhere: `static let rate` is indexed.
#[test]
fn swift_declarations_are_what_they_declare() {
    let src = "\
let maxItems = 10
var counter = 0
let (low, high) = (0, 9)

struct Money {
    static let rate = 3
    let cents: Int
    var label: String = \"\"
    var doubled: Int { cents * 2 }
    init(cents: Int) { self.cents = cents }
}

class Cart {
    lazy var cache = [Int]()
    deinit { }
    subscript(index: Int) -> Int { index }
}

enum Status {
    case pending, shipped
    static let all: [Status] = []
}

actor Ledger {
    var balance = 0
}

protocol Pricing {
    var rate: Int { get }
    func price(units: Int) -> Int
}

extension Money: Pricing {
    func price(units: Int) -> Int { units * Money.rate }
}
";
    let parsed = parse_content_sync(src, "swift", Path::new("Shop.swift")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    for (name, ty) in [
        ("Money", "struct"),
        ("Cart", "class"),
        ("Status", "enum"),
        ("Ledger", "class"),
        ("Pricing", "interface"),
        ("Money", "extension"),
        ("maxItems", "constant"),
        ("rate", "constant"),
        ("cents", "constant"),
        ("all", "constant"),
    ] {
        assert!(
            found.contains(&(name, ty.to_string())),
            "{name} ({ty}): {found:?}"
        );
    }
    assert!(
        !found.iter().any(|(name, ty)| ty == "protocol"
            || name.starts_with('(')
            || matches!(*name, "counter" | "label" | "doubled" | "cache" | "balance")),
        "{found:?}"
    );
    assert_eq!(
        found.iter().filter(|(name, _)| *name == "Money").count(),
        2,
        "the struct and its extension: {found:?}"
    );

    let functions: Vec<(&str, Option<&str>)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some((f.name.as_str(), f.parent_class.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        functions,
        [
            ("init", Some("Money")),
            ("deinit", Some("Cart")),
            ("subscript", Some("Cart")),
            ("price", Some("Pricing")),
            ("price", Some("Money")),
        ]
    );
}

/// Entities on one line are told apart by column. By lines alone each of two
/// entities on the same line would hold the other: a one-line type and its
/// field would each be the other's member, and a call in the first of two
/// one-line functions would be made by the second — here, by the function it
/// calls.
#[test]
fn entities_on_one_line_hold_what_their_columns_hold() {
    let edges = hierarchy_edges("interface Node { id: ID! }\n", "graphql", "schema.graphql");
    let members: Vec<_> = edges
        .iter()
        .filter(|(_, ty, _)| ty == "has_method")
        .collect();
    assert_eq!(
        members,
        [&(
            "Node".to_string(),
            "has_method".to_string(),
            "id".to_string()
        )]
    );

    let src = "function first() { return second(); } function second() { return 1; }\n";
    let edges = hierarchy_edges(src, "javascript", "pair.js");
    let calls: Vec<_> = edges.iter().filter(|(_, ty, _)| ty == "calls").collect();
    assert_eq!(
        calls,
        [&(
            "first".to_string(),
            "calls".to_string(),
            "second".to_string()
        )]
    );
}

/// A class names what it inherits and implements in every language, from the
/// clauses and leaves its grammar has: C++, CUDA and C# lists, PHP's and
/// GraphQL's `name`s, Ruby's `constant`s and an Objective-C class's superclass
/// field and protocol list.
#[test]
fn a_class_names_its_supertypes_in_every_language() {
    let cases = [
        (
            "cpp",
            "cart.cpp",
            "class Base {};\nclass Cart : public Base {};\n",
            &[("Cart", "inherits", "Base")][..],
        ),
        (
            "cuda",
            "cart.cu",
            "class Base {};\nclass Cart : public Base {};\n",
            &[("Cart", "inherits", "Base")],
        ),
        (
            "c_sharp",
            "Cart.cs",
            "interface IPriced {}\nclass Base {}\nclass Cart : Base, IPriced {}\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "IPriced"),
            ],
        ),
        (
            "php",
            "cart.php",
            "<?php\ninterface Priced {}\nclass Base {}\nclass Cart extends \\Shop\\Base implements Priced {}\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
        (
            "ruby",
            "cart.rb",
            "class Base\nend\nclass Cart < Base\nend\n",
            &[("Cart", "inherits", "Base")],
        ),
        (
            "objc",
            "Cart.m",
            "@protocol Priced\n@end\n@interface Base : NSObject\n@end\n@interface Cart : Base <Priced>\n@end\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
        (
            "graphql",
            "schema.graphql",
            "interface Node { id: ID! }\ntype Order implements Node { id: ID! }\n",
            &[("Order", "implements", "Node")],
        ),
        (
            "java",
            "Cart.java",
            "interface Priced {}\nclass Base {}\nclass Cart extends Base implements Priced {}\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
        (
            "python",
            "cart.py",
            "class Base:\n    pass\nclass Cart(Base):\n    pass\n",
            &[("Cart", "inherits", "Base")],
        ),
        (
            "kotlin",
            "Cart.kt",
            "interface Priced\nopen class Base\nclass Cart : Base(), Priced\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
        (
            "swift",
            "Cart.swift",
            "protocol Priced {}\nclass Base {}\nclass Cart: Base, Priced {}\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
        (
            "typescript",
            "cart.ts",
            "interface Priced {}\nclass Base {}\nclass Cart extends Base implements Priced {}\n",
            &[
                ("Cart", "inherits", "Base"),
                ("Cart", "implements", "Priced"),
            ],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let edges = hierarchy_edges(src, lang, file);
        let found: Vec<(&str, &str, &str)> = edges
            .iter()
            .filter(|(_, ty, _)| ty == "inherits" || ty == "implements")
            .map(|(from, ty, to)| (from.as_str(), ty.as_str(), to.as_str()))
            .collect();
        let mut found = found;
        found.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(found, expected, "{lang}");
    }

    // A qualified supertype is named by its last segment, not by each.
    let parsed = parse_content_sync(
        "<?php\nclass Cart extends \\Shop\\Base {}\n",
        "php",
        Path::new("cart.php"),
    )
    .expect("parse");
    let named: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Inherits)
        .map(|r| r.target_name.as_str())
        .collect();
    assert_eq!(named, ["Base"]);
}

/// VB.NET is read by the kinds its grammar has, not by statement kinds it does
/// not have (`function_statement`, `class_statement`, …): its methods,
/// constructors and constants are indexed, a `Structure` is a struct and an
/// `Imports` no type alias; and `Inherits` and `Implements` are read from the
/// lines after `Class`, where VB.NET puts them, though the grammar reads them
/// only on the `Class` line itself.
#[test]
fn vb_net_declarations_are_indexed() {
    let src = "\
Imports System.Text

Namespace Shop
    Public Interface IPriced
        Function Price() As Integer
    End Interface

    Public Class Base
    End Class

    Public Class Cart
        Inherits Base
        Implements IPriced

        Private Const Limit As Integer = 10
        Private items As Integer

        Public Sub New()
        End Sub

        Public Function Price() As Integer Implements IPriced.Price
            Return Total() * 2
        End Function

        Private Function Total() As Integer
            Return items
        End Function
    End Class

    Public Structure Point
        Public X As Integer
    End Structure

    Public Enum Status
        Open
        Closed
    End Enum

    Public Module Helpers
        Public Sub Log(message As String)
        End Sub
    End Module
End Namespace
";
    let parsed = parse_content_sync(src, "vb_net", Path::new("Shop.vb")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    for (name, ty) in [
        ("Shop", "namespace"),
        ("IPriced", "interface"),
        ("Cart", "class"),
        ("Point", "struct"),
        ("Status", "enum"),
        ("Helpers", "module"),
        ("Limit", "constant"),
    ] {
        assert!(
            found.contains(&(name, ty.to_string())),
            "{name} ({ty}): {found:?}"
        );
    }
    assert!(
        !found
            .iter()
            .any(|(name, ty)| ty == "type_alias" || matches!(*name, "X" | "items" | "Inherits")),
        "{found:?}"
    );

    let functions: Vec<(&str, Option<&str>)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some((f.name.as_str(), f.parent_class.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        functions,
        [
            ("Price", Some("IPriced")),
            ("New", Some("Cart")),
            ("Price", Some("Cart")),
            ("Total", Some("Cart")),
            ("Log", None),
        ]
    );

    let edges = hierarchy_edges(src, "vb_net", "Shop.vb");
    for edge in [
        ("Cart", "inherits", "Base"),
        ("Cart", "implements", "IPriced"),
        ("Price", "calls", "Total"),
    ] {
        assert!(
            edges.contains(&(edge.0.into(), edge.1.into(), edge.2.into())),
            "{edge:?}: {edges:?}"
        );
    }
}

/// One capital letter is not a constant's name: `X` names a coordinate far
/// more often, and a one-letter constant is declared as one.
#[test]
fn one_capital_letter_names_no_constant() {
    let constants = |src: &str, lang: &str, file: &str| -> Vec<String> {
        entities(src, lang, file)
            .into_iter()
            .filter(|(_, ty, _)| ty == "constant")
            .map(|(name, _, _)| name)
            .collect()
    };
    assert_eq!(
        constants("X = 1\nMAX = 2\n", "python", "limits.py"),
        ["MAX"]
    );
    assert_eq!(
        constants(
            "struct Point { public int X; public const int N = 3; }\n",
            "c_sharp",
            "Point.cs"
        ),
        ["N"]
    );
}

/// A Rust trait's required methods are functions of the trait, as an
/// interface's are; the grammar calls each a `function_signature_item`.
#[test]
fn a_rust_trait_declares_its_methods() {
    let src = "\
trait Priced {
    fn price(&self) -> i32;
    fn label(&self) -> String { String::new() }
}
struct Cart;
impl Priced for Cart {
    fn price(&self) -> i32 { 0 }
}
";
    let parsed = parse_content_sync(src, "rust", Path::new("cart.rs")).expect("parse");
    let functions: Vec<(&str, Option<&str>)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) => Some((f.name.as_str(), f.parent_class.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        functions,
        [
            ("price", Some("Priced")),
            ("label", Some("Priced")),
            ("price", Some("Cart")),
        ]
    );
}

/// An interface or trait inherits what it extends; a struct or an enum
/// implements the interfaces it names.
#[test]
fn interfaces_inherit_and_enums_implement() {
    let cases = [
        (
            "java",
            "Shapes.java",
            "interface A {}\ninterface B extends A {}\nenum E implements A { X }\n",
            &[("B", "inherits", "A"), ("E", "implements", "A")][..],
        ),
        (
            "typescript",
            "shapes.ts",
            "interface A {}\ninterface B extends A {}\n",
            &[("B", "inherits", "A")],
        ),
        (
            "c_sharp",
            "Shapes.cs",
            "enum E : byte { X }\ninterface IA {}\ninterface IB : IA {}\nstruct S : IA {}\n",
            &[("IB", "inherits", "IA"), ("S", "implements", "IA")],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let edges = hierarchy_edges(src, lang, file);
        let mut found: Vec<(&str, &str, &str)> = edges
            .iter()
            .filter(|(_, ty, _)| ty == "inherits" || ty == "implements")
            .map(|(from, ty, to)| (from.as_str(), ty.as_str(), to.as_str()))
            .collect();
        found.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(found, expected, "{lang}");
    }
}

/// PHP traits and enums are what they are, not classes; its constants are
/// named by their `const_element`; and its static, nullsafe and `new` calls
/// are calls, as `$x->m()` is.
#[test]
fn php_declarations_and_calls() {
    let src = "\
<?php
const VERSION = \"1.0\";
interface Priced { public function price(): int; }
trait Logs { public function log(string $m): void {} }
enum Status: string implements Priced {
    case Open = 'open';
    public function price(): int { return 0; }
}
class Cart implements Priced {
    use Logs;
    const MAX = 10;
    public function price(): int { return $this->total() + Cart::fee() + $this?->rate(); }
    private function total(): int { return 1; }
    private function rate(): int { return 1; }
    public static function fee(): int { $c = new Cart(); return 2; }
}
";
    let found = entities(src, "php", "cart.php");
    for (name, ty) in [
        ("Logs", "trait"),
        ("Status", "enum"),
        ("Cart", "class"),
        ("VERSION", "constant"),
        ("MAX", "constant"),
    ] {
        assert!(
            found.contains(&(name.into(), ty.into(), "php".into())),
            "{name} ({ty}): {found:?}"
        );
    }
    let edges = hierarchy_edges(src, "php", "cart.php");
    for edge in [
        ("price", "calls", "total"),
        ("price", "calls", "fee"),
        ("price", "calls", "rate"),
        ("fee", "calls", "Cart"),
        ("Status", "implements", "Priced"),
    ] {
        assert!(
            edges.contains(&(edge.0.into(), edge.1.into(), edge.2.into())),
            "{edge:?}: {edges:?}"
        );
    }
}

/// A Kotlin `enum class` is an enum, not a class with an enum for each entry;
/// a secondary constructor is a function of its class; and a Kotlin file's
/// imports are read, which the grammar calls `import`, not Java's
/// `import_declaration`.
#[test]
fn kotlin_enums_constructors_and_imports() {
    let src = "\
package shop

import com.shop.model.Cart
import com.shop.util.*
import com.shop.model.Order as O

enum class Color { RED, GREEN }

class Basket(val id: Int) {
    constructor(name: String) : this(0)
}
";
    let parsed = parse_content_sync(src, "kotlin", Path::new("Basket.kt")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    assert_eq!(
        found,
        [
            ("Color", "enum".to_string()),
            ("Basket", "class".to_string()),
            ("constructor", "function".to_string()),
        ]
    );
    let imports: Vec<(&str, &str, Option<&str>, bool)> = parsed
        .imports
        .iter()
        .map(|i| {
            (
                i.local_name.as_str(),
                i.source_module.as_str(),
                i.original_name.as_deref(),
                i.is_wildcard,
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            ("Cart", "com.shop.model.Cart", None, false),
            ("*", "com.shop.util", None, true),
            ("O", "com.shop.model.Order", Some("Order"), false),
        ]
    );
}

/// An extension is an entity where one is declared, and records what it
/// extends. Matched by the word in their kind, SQL's `DROP EXTENSION` would be
/// an extension, a GraphQL `extend type` a second class of its type's name and
/// an `extend union` a second union, and an Objective-C category one more
/// class.
#[test]
fn an_extension_is_declared_and_names_what_it_extends() {
    let extensions = |src: &str, lang: &str, file: &str| {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let found: Vec<(String, Option<String>)> = parsed
            .entities
            .iter()
            .filter_map(|e| match e {
                Entity::Extension(x) => Some((x.name.clone(), x.extended_type.clone())),
                _ => None,
            })
            .collect();
        (parsed, found)
    };
    let some = |s: &str| Some(s.to_string());

    let schema = "type Order { id: ID! }\nextend type Order { total: Int }\n\
                  union Result = Order\nextend union Result = Order\n";
    let (parsed, found) = extensions(schema, "graphql", "schema.graphql");
    assert_eq!(
        found,
        [
            ("Order".to_string(), some("Order")),
            ("Result".to_string(), some("Result"))
        ]
    );
    let kinds_of = |name: &str| -> Vec<String> {
        parsed
            .entities
            .iter()
            .filter(|e| e.name() == name)
            .map(|e| e.entity_type().to_string())
            .collect()
    };
    assert_eq!(kinds_of("Order"), ["class", "extension"]);
    assert_eq!(kinds_of("Result"), ["union", "extension"]);
    assert!(parsed.entities.iter().any(
        |e| matches!(e, Entity::Function(f) if f.name == "total" && f.parent_class.as_deref() == Some("Order"))
    ));

    let objc = "@interface Cart : NSObject\n@end\n@interface Cart (Pricing)\n- (int)price;\n@end\n\
                @interface Cart ()\n@end\n";
    let (parsed, found) = extensions(objc, "objc", "Cart.h");
    assert_eq!(
        found,
        [
            ("Cart(Pricing)".to_string(), some("Cart")),
            ("Cart".to_string(), some("Cart"))
        ]
    );
    assert_eq!(
        parsed
            .entities
            .iter()
            .filter(|e| e.entity_type() == "class")
            .count(),
        1
    );

    let sql = "CREATE EXTENSION IF NOT EXISTS pg_trgm;\nDROP EXTENSION IF EXISTS hstore;\n";
    assert_eq!(
        extensions(sql, "sql", "schema.sql").1,
        [("pg_trgm".to_string(), None)]
    );

    let dart = "extension Shout on String {\n  String shout() => toUpperCase();\n}\n";
    assert_eq!(
        extensions(dart, "dart", "shout.dart").1,
        [("Shout".to_string(), some("String"))]
    );
}

/// An Objective-C enum declared with `NS_ENUM` or `NS_OPTIONS`, as Apple's
/// frameworks declare theirs, is an enum named by the macro: the grammar
/// leaves the declaration as errors, and the `typedef` it makes is no type
/// alias named after the first enumerator. Objective-C's includes and
/// `@import`s, and CUDA's includes, are a file's imports, as C's are.
#[test]
fn objc_macro_enums_and_includes() {
    let src = "\
#import <Foundation/Foundation.h>
@import UIKit;

typedef NS_ENUM(NSInteger, Status) {
    StatusOpen,
    StatusClosed = 2,
};
typedef NS_OPTIONS(NSUInteger, Flags) { FlagA = 1 << 0, FlagB = 1 << 1 };
";
    let parsed = parse_content_sync(src, "objc", Path::new("Status.h")).expect("parse");
    let enums: Vec<(&str, Option<&str>, serde_json::Value)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Enum(x) => Some((
                x.name.as_str(),
                x.backed_type.as_deref(),
                x.variants.clone().unwrap_or_default(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        enums,
        [
            (
                "Status",
                Some("NSInteger"),
                serde_json::json!(["StatusOpen", "StatusClosed"])
            ),
            (
                "Flags",
                Some("NSUInteger"),
                serde_json::json!(["FlagA", "FlagB"])
            ),
        ]
    );
    assert!(
        !parsed.entities.iter().any(|e| e.name() == "StatusOpen"),
        "{:?}",
        parsed.entities
    );
    let imports: Vec<&str> = parsed
        .imports
        .iter()
        .map(|i| i.source_module.as_str())
        .collect();
    assert_eq!(imports, ["Foundation/Foundation.h", "UIKit"]);

    let cuda = "#include \"kernels.h\"\n__global__ void scale(float *x) { x[0] *= 2; }\n";
    let parsed = parse_content_sync(cuda, "cuda", Path::new("scale.cu")).expect("parse");
    let imports: Vec<&str> = parsed
        .imports
        .iter()
        .map(|i| i.source_module.as_str())
        .collect();
    assert_eq!(imports, ["kernels.h"]);
}

/// A Ruby constant is an assignment to a capitalized name, which is in the
/// assignment's `left`; a keyword is no entity, though Ruby's `module` keyword
/// shares a module's kind; and a Ruby call is a call, not an import too, which
/// would make a `uses` reference of each part of every call.
#[test]
fn ruby_constants_modules_and_calls() {
    let src = "\
require 'json'

module Shop
  RATE = 3
  Default = 5
  class Cart < Base
    MAX = 10
    def total
      items.sum(&:price) + helper(1)
    end
    def helper(x)
      count = x * RATE
      count
    end
  end
end
";
    let parsed = parse_content_sync(src, "ruby", Path::new("cart.rb")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    let expected: Vec<(&str, String)> = [
        ("Shop", "module"),
        ("RATE", "constant"),
        ("Default", "constant"),
        ("Cart", "class"),
        ("MAX", "constant"),
        ("total", "function"),
        ("helper", "function"),
    ]
    .into_iter()
    .map(|(name, ty)| (name, ty.to_string()))
    .collect();
    assert_eq!(found, expected);
    assert!(
        parsed.refs.iter().all(|r| r.kind != RelationshipType::Uses),
        "{:?}",
        parsed.refs
    );

    // A TypeScript `module` keyword is no module either.
    let found = entities(
        "module Shop { export const RATE = 3; }\n",
        "typescript",
        "shop.ts",
    );
    assert!(
        !found.iter().any(|(name, _, _)| name == "shop"),
        "{found:?}"
    );

    // An import names what it imports (a qualified name its last segment), not
    // its keyword and punctuation.
    let src = "namespace Shop {\n    using System.Text;\n    class Cart {}\n}\n";
    let parsed = parse_content_sync(src, "c_sharp", Path::new("Cart.cs")).expect("parse");
    let used: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Uses)
        .map(|r| r.target_name.as_str())
        .collect();
    assert_eq!(used, ["Text"]);
}

/// Solidity is read by its grammar's kinds. A call is the call expression, not
/// its arguments (`pay` calls neither `v` nor `sender`); a `constant` or
/// `immutable` state variable is a constant, at file level too, and a plain
/// one (`total`) is not; `receive()` and `fallback()` are functions; and a
/// Solidity file's imports are read.
#[test]
fn solidity_constants_calls_and_imports() {
    let src = "\
pragma solidity ^0.8.0;
import \"./IERC20.sol\";
import {Ownable as Owned, Pausable} from \"./Ownable.sol\";

uint256 constant FEE = 3;

contract Shop is Owned {
    uint256 public constant MAX = 10;
    uint256 public immutable start;
    uint256 total;
    function pay(uint256 v) external { total += fee(v); IERC20(token).transfer(msg.sender, v); }
    function fee(uint256 v) internal pure returns (uint256) { return v * FEE; }
    receive() external payable {}
    fallback() external payable {}
}
";
    let parsed = parse_content_sync(src, "solidity", Path::new("Shop.sol")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    let expected: Vec<(&str, String)> = [
        ("FEE", "constant"),
        ("Shop", "class"),
        ("MAX", "constant"),
        ("start", "constant"),
        ("pay", "function"),
        ("fee", "function"),
        ("receive", "function"),
        ("fallback", "function"),
    ]
    .into_iter()
    .map(|(name, ty)| (name, ty.to_string()))
    .collect();
    assert_eq!(found, expected);

    let mut called: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Calls)
        .map(|r| r.target_name.as_str())
        .collect();
    called.sort_unstable();
    assert_eq!(called, ["IERC20", "fee", "transfer"]);

    let imports: Vec<(&str, &str, Option<&str>, bool)> = parsed
        .imports
        .iter()
        .map(|i| {
            (
                i.local_name.as_str(),
                i.source_module.as_str(),
                i.original_name.as_deref(),
                i.is_wildcard,
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            ("*", "./IERC20.sol", None, true),
            ("Owned", "./Ownable.sol", Some("Ownable"), false),
            ("Pausable", "./Ownable.sol", None, false),
        ]
    );
}

/// A JavaScript or TypeScript class expression is a class, named by its own
/// name or by what it is bound to (the grammar's kind for it is `class`, not
/// `class_expression`), and its methods are its members.
/// A class field is a constant as a field is elsewhere — `static MAX`,
/// `static readonly LIMIT` — unless its value is a function, which it is.
#[test]
fn js_class_expressions_and_constant_fields() {
    let src = "\
const Cart = class {
  total() { return 0; }
};
module.exports = class Named {};
class Order {
  count = 0;
  static MAX = 5;
  static HANDLER = () => 1;
}
";
    for (lang, file) in [("javascript", "cart.js"), ("typescript", "cart.ts")] {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let found: Vec<(&str, String)> = parsed
            .entities
            .iter()
            .map(|e| (e.name(), e.entity_type().to_string()))
            .collect();
        let expected: Vec<(&str, String)> = [
            ("Cart", "class"),
            ("total", "function"),
            ("Named", "class"),
            ("Order", "class"),
            ("MAX", "constant"),
            ("HANDLER", "function"),
        ]
        .into_iter()
        .map(|(name, ty)| (name, ty.to_string()))
        .collect();
        assert_eq!(found, expected, "{lang}");
        assert!(
            parsed.entities.iter().any(|e| matches!(
                e,
                Entity::Function(f) if f.name == "total" && f.parent_class.as_deref() == Some("Cart")
            )),
            "{lang}"
        );
    }
    let ts = "class Order {\n  private count: number = 0;\n  static readonly LIMIT = 3;\n}\n";
    let found = entities(ts, "typescript", "order.ts");
    assert!(
        found.contains(&("LIMIT".into(), "constant".into(), "typescript".into()))
            && !found.iter().any(|(name, _, _)| name == "count"),
        "{found:?}"
    );
}

/// Kinds the configuration names as the grammars have them: Java's
/// `new Order()` is a call, as C#'s is; a C# `record struct` is a struct; and
/// a SQL `CREATE TYPE … AS ENUM` is an enum, a function used in a view a call,
/// and a `SET` statement no constant named after its value.
#[test]
fn java_creations_csharp_record_structs_and_sql_types() {
    let java = "class Cart {\n    void fill() { Order o = new Order(); }\n}\nclass Order {}\n";
    let edges = hierarchy_edges(java, "java", "Cart.java");
    assert!(
        edges.contains(&("fill".into(), "calls".into(), "Order".into())),
        "{edges:?}"
    );

    let cs = "record struct Point(int X, int Y);\nrecord Money(int Cents);\n";
    let found = entities(cs, "c_sharp", "Point.cs");
    assert_eq!(
        found
            .iter()
            .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
            .collect::<Vec<_>>(),
        [("Point", "struct"), ("Money", "class")]
    );

    let sql = "\
SET search_path = public;
CREATE TYPE status AS ENUM ('open', 'closed');
CREATE FUNCTION fee(v int) RETURNS int AS $$ SELECT v * 3 $$ LANGUAGE sql;
CREATE VIEW totals AS SELECT fee(total) AS f FROM orders;
";
    let parsed = parse_content_sync(sql, "sql", Path::new("schema.sql")).expect("parse");
    let found: Vec<(&str, String)> = parsed
        .entities
        .iter()
        .map(|e| (e.name(), e.entity_type().to_string()))
        .collect();
    assert_eq!(
        found,
        [
            ("status", "enum".to_string()),
            ("fee", "function".to_string()),
            ("totals", "table".to_string()),
        ]
    );
    let labels = parsed.entities.iter().find_map(|e| match e {
        Entity::Enum(x) => x.variants.clone(),
        _ => None,
    });
    assert_eq!(labels, Some(serde_json::json!(["open", "closed"])));
    let edges = hierarchy_edges(sql, "sql", "schema.sql");
    assert!(
        edges.contains(&("totals".into(), "calls".into(), "fee".into())),
        "{edges:?}"
    );
}

/// A component's scripts make the calls and imports they make. Their
/// relationships are read in the script's language, not the component's,
/// whose kinds are its document's; and the file's imports are read from its
/// scripts, not from the document, where a script is opaque text.
#[test]
fn component_scripts_make_their_calls_and_imports() {
    let vue = "\
<template><button @click=\"go(1)\">Go</button></template>
<script setup>
import { fetchOrders } from '../api/orders.js';
async function go(page) { return fetchOrders({ page }); }
</script>
";
    let svelte = "\
<script>
  import { sumRevenue } from './lib/revenue.js';
  function total(orders) { return sumRevenue(orders); }
</script>
<p>{total([])}</p>
";
    for (src, lang, file, caller, callee, module) in [
        (
            vue,
            "vue",
            "OrderTable.vue",
            "go",
            "fetchOrders",
            "../api/orders.js",
        ),
        (
            svelte,
            "svelte",
            "Revenue.svelte",
            "total",
            "sumRevenue",
            "./lib/revenue.js",
        ),
    ] {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let calls: Vec<(String, &str)> = parsed
            .refs
            .iter()
            .filter(|r| r.kind == RelationshipType::Calls)
            .map(|r| {
                let source = parsed
                    .entities
                    .iter()
                    .find(|e| e.id() == r.source_id)
                    .map(|e| e.name().to_string())
                    .unwrap_or_default();
                (source, r.target_name.as_str())
            })
            .collect();
        assert!(
            calls.contains(&(caller.to_string(), callee)),
            "{lang}: {calls:?}"
        );
        assert!(
            parsed.imports.iter().any(|i| i.source_module == module),
            "{lang}: {:?}",
            parsed.imports
        );
    }
}

/// A JavaScript or TypeScript import gives the names it imports, not only its
/// module's file name. A specifier's `name` is the export, its `alias` the
/// local name. `import * as money` binds one name, the module: it is no
/// wildcard, which brings every export in unqualified.
#[test]
fn a_js_import_gives_its_names() {
    let src = "\
import { fetchOrders, saveOrder as save } from '../api/orders.js';
import Cart from './cart.js';
import * as money from './money.js';
import 'polyfill';
";
    for (lang, file) in [("javascript", "app.js"), ("typescript", "app.ts")] {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let imports: Vec<(&str, &str, Option<&str>, bool)> = parsed
            .imports
            .iter()
            .map(|i| {
                (
                    i.local_name.as_str(),
                    i.source_module.as_str(),
                    i.original_name.as_deref(),
                    i.is_wildcard,
                )
            })
            .collect();
        assert_eq!(
            imports,
            [
                (
                    "fetchOrders",
                    "../api/orders.js",
                    Some("fetchOrders"),
                    false
                ),
                ("save", "../api/orders.js", Some("saveOrder"), false),
                ("Cart", "./cart.js", Some("Cart"), false),
                ("money", "./money.js", None, false),
                ("polyfill", "polyfill", None, false),
            ],
            "{lang}"
        );
    }
}

/// A Rust `use` and a Python import give each name they bring in, with the
/// module it comes from: a Rust `use` each name, not one import named by its
/// whole path, lists and `as` clauses included; a Python `from` import its
/// module, not its last name; and an `import os, sys` both `os` and `sys`.
#[test]
fn rust_and_python_imports_give_each_name() {
    let imports =
        |src: &str, lang: &str, file: &str| -> Vec<(String, String, Option<String>, bool)> {
            parse_content_sync(src, lang, Path::new(file))
                .expect("parse")
                .imports
                .into_iter()
                .map(|i| {
                    (
                        i.local_name,
                        i.source_module,
                        i.original_name,
                        i.is_wildcard,
                    )
                })
                .collect()
        };
    let item = |local: &str, module: &str, name: Option<&str>, wildcard: bool| {
        (
            local.to_string(),
            module.to_string(),
            name.map(str::to_string),
            wildcard,
        )
    };
    let rust = "\
use crate::shipping::shipping_cents;
use crate::discount::{coupon, volume as vol, rules::{self, cap}};
use std::collections::*;
";
    assert_eq!(
        imports(rust, "rust", "totals.rs"),
        [
            item(
                "shipping_cents",
                "crate::shipping",
                Some("shipping_cents"),
                false
            ),
            item("coupon", "crate::discount", Some("coupon"), false),
            item("vol", "crate::discount", Some("volume"), false),
            item("rules", "crate::discount", Some("rules"), false),
            item("cap", "crate::discount::rules", Some("cap"), false),
            item("*", "std::collections", None, true),
        ]
    );
    let python = "\
import os, sys as system
from app.dispatch import dispatch_date, ships_same_day as same
from m import *
";
    assert_eq!(
        imports(python, "python", "main.py"),
        [
            item("os", "os", None, false),
            item("system", "sys", None, false),
            item(
                "dispatch_date",
                "app.dispatch",
                Some("dispatch_date"),
                false
            ),
            item("same", "app.dispatch", Some("ships_same_day"), false),
            item("*", "m", None, true),
        ]
    );
}

/// A Go import gives each package it imports, under the name it is called by:
/// its alias, else its last segment but for a major version, from a single
/// import and a grouped one alike.
#[test]
fn go_imports_give_each_package() {
    let src = "package main\n\nimport \"os\"\nimport (\n\t\"fmt\"\n\th \"net/http\"\n\
               \t. \"strings\"\n\t_ \"github.com/lib/pq\"\n\t\"github.com/acme/shop/v2\"\n\
               \t\"gopkg.in/yaml.v3\"\n)\n";
    let imports: Vec<(String, String, bool)> = parse_content_sync(src, "go", Path::new("main.go"))
        .expect("parse")
        .imports
        .into_iter()
        .map(|i| (i.local_name, i.source_module, i.is_wildcard))
        .collect();
    let import =
        |local: &str, path: &str, wildcard: bool| (local.to_string(), path.to_string(), wildcard);
    assert_eq!(
        imports,
        [
            import("os", "os", false),
            import("fmt", "fmt", false),
            import("h", "net/http", false),
            import("*", "strings", true),
            import("_", "github.com/lib/pq", false),
            import("shop", "github.com/acme/shop/v2", false),
            import("yaml", "gopkg.in/yaml.v3", false),
        ]
    );
}

/// A Python import uses each name it brings in and the module a `from`
/// import takes them from: every name of `import os.path, numpy as np`, by the
/// name it imports (`os`, `numpy`), the name an alias stands for (`sub` of
/// `sub as s`), and a relative module by its name (`pkg` of `.pkg`), never by
/// its dots.
#[test]
fn a_python_import_uses_each_name_it_brings_in() {
    let src = "\
import os.path, numpy as np
from .pkg import sub as s, other
from . import helpers
from m import *
";
    let parsed = parse_content_sync(src, "python", Path::new("app.py")).expect("parse");
    let mut used: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Uses)
        .map(|r| r.target_name.as_str())
        .collect();
    used.sort_unstable();
    assert_eq!(used, ["helpers", "m", "numpy", "os", "other", "pkg", "sub"]);
}

/// A qualified call is of a member of the type its qualifier names: `self`
/// and `this` the caller's own type, a receiver named after its type
/// (`cart`) that type — not the first definition of its name in the file,
/// whoever's member.
#[test]
fn a_qualified_call_is_of_the_type_its_qualifier_names() {
    let cases = [
        (
            "python",
            "shop.py",
            "\
class Order:
    def total(self):
        return 2

class Cart:
    def total(self):
        return 1

    def checkout(self):
        return self.total()

def report(cart):
    return cart.total()
",
        ),
        (
            "typescript",
            "shop.ts",
            "\
class Order {
  total(): number { return 2; }
}
class Cart {
  total(): number { return 1; }
  checkout(): number { return this.total(); }
}
function report(cart: Cart): number { return cart.total(); }
",
        ),
    ];
    for (lang, file, src) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let entity = |id| parsed.entities.iter().find(|e| e.id() == id);
        let mut calls: Vec<(String, Option<String>)> = parsed
            .relationships
            .iter()
            .filter(|r| r.relationship_type == RelationshipType::Calls)
            .filter_map(|r| {
                let source = entity(r.source_id)?.name().to_string();
                let owner = match entity(r.target_id)? {
                    Entity::Function(f) => f.parent_class.clone(),
                    _ => None,
                };
                Some((source, owner))
            })
            .collect();
        calls.sort();
        let cart = Some("Cart".to_string());
        assert_eq!(
            calls,
            [
                ("checkout".to_string(), cart.clone()),
                ("report".to_string(), cart.clone()),
            ],
            "{lang}"
        );
    }
}

/// A call through a path — Rust's `Money::new()`, C++'s `std::abs(x)`,
/// PHP's `Tax::rate()` — is of a member of the type the path names or a
/// function of the module it names, and of nothing else: a path names a type
/// or a module, never a value, so `Vec::new()` is no call of the file's own
/// `Cart::new`, `std::abs` none of its `abs` and `Carbon::now()` none of its
/// `now`. Rust's
/// `self::` and `super::` name modules, and PHP's `parent::` a supertype the
/// file does not tell: a call through one is of any definition of its name.
#[test]
fn a_call_through_a_path_is_of_what_the_path_names() {
    /// A call: its caller, its callee and the type the callee is a member of.
    type Call = (&'static str, &'static str, Option<&'static str>);
    let cases: [(&str, &str, &str, &[Call]); 3] = [
        (
            "rust",
            "lib.rs",
            "mod money {\n    pub fn round(c: i64) -> i64 { c }\n}\n\
             fn helper() -> i64 { 0 }\n\
             mod inner {\n    pub fn run() -> i64 { super::helper() }\n}\n\
             struct Cart;\nstruct Order;\nstruct Cache<T>(T);\n\
             impl<T> Cache<T> {\n    fn create(t: T) -> Self { Cache(t) }\n}\n\
             impl Order {\n    fn new() -> Self { Order }\n}\n\
             impl Cart {\n    fn new() -> Self { Cart }\n    \
             fn total(&self) -> i64 { money::round(1) + Self::fee() + self::helper() }\n    \
             fn fee() -> i64 { 0 }\n}\n\
             fn build() -> Vec<u8> { let _ = Cart::new(); let _ = Cache::<u8>::create(1); Vec::<u8>::new() }\n",
            &[
                ("build", "create", Some("Cache")),
                ("build", "new", Some("Cart")),
                // A tuple struct's constructor.
                ("create", "Cache", None),
                ("run", "helper", None),
                ("total", "fee", Some("Cart")),
                ("total", "helper", None),
                ("total", "round", None),
            ],
        ),
        (
            "cpp",
            "geo.cpp",
            "namespace geo {\nint distance(int a) { return a; }\n}\n\
             int abs(int x) { return x; }\n\
             int length() { return geo::distance(1) + std::abs(2); }\n",
            &[("length", "distance", None)],
        ),
        (
            "php",
            "tax.php",
            "<?php\nclass Tax {\n    public static function rate() { return 1; }\n}\n\
             class Duty extends Tax {\n    public static function rate() { return 2; }\n    \
             public static function levy() { return parent::base(); }\n}\n\
             class Base {\n    public static function base() { return 3; }\n}\n\
             function now() { return 0; }\n\
             function total() { return Tax::rate() + Carbon::now(); }\n",
            &[
                ("levy", "base", Some("Base")),
                ("total", "rate", Some("Tax")),
            ],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let entity = |id| parsed.entities.iter().find(|e| e.id() == id);
        let mut calls: Vec<(String, String, Option<String>)> = parsed
            .relationships
            .iter()
            .filter(|r| r.relationship_type == RelationshipType::Calls)
            .filter_map(|r| {
                let target = entity(r.target_id)?;
                let owner = match target {
                    Entity::Function(f) => f.parent_class.clone(),
                    _ => None,
                };
                Some((
                    entity(r.source_id)?.name().to_string(),
                    target.name().to_string(),
                    owner,
                ))
            })
            .collect();
        calls.sort();
        let expected: Vec<(String, String, Option<String>)> = expected
            .iter()
            .map(|(source, target, owner)| {
                (
                    (*source).to_string(),
                    (*target).to_string(),
                    owner.map(str::to_string),
                )
            })
            .collect();
        assert_eq!(calls, expected, "{lang}");
    }
}

/// A call through a value of a type the index cannot tell is not of whatever
/// shares its name. A standard type's method (`items.len()`, `xs.push(x)`)
/// is the standard type's, unless the value is named after a type that has
/// it — by its name or last word (`guard` for `StaleGuard`, `queue` for
/// `TaskQueue`) — or is the caller itself; and a value an import binds
/// (`json.dumps`) is the module's, which is another file.
#[test]
fn a_call_through_a_value_is_not_of_whatever_shares_its_name() {
    type Call = (&'static str, &'static str, Option<&'static str>);
    let cases: [(&str, &str, &str, &[Call]); 3] = [
        (
            "rust",
            "guard.rs",
            "struct StaleGuard;\nimpl StaleGuard {\n    fn len(&self) -> usize { 0 }\n    \
             fn is_stale(&self) -> bool { self.len() > 0 }\n}\n\
             fn count(items: &Vec<u8>) -> usize { items.len() }\n\
             fn check(guard: &StaleGuard) -> usize { guard.len() }\n",
            &[
                ("check", "len", Some("StaleGuard")),
                ("is_stale", "len", Some("StaleGuard")),
            ],
        ),
        (
            "python",
            "tasks.py",
            "import json\n\nclass TaskQueue:\n    def append(self, task):\n        pass\n\n\
             def dumps(value):\n    return str(value)\n\n\
             def enqueue(queue, names, task):\n    names.append(task)\n    queue.append(task)\n    \
             return json.dumps(task)\n",
            &[("enqueue", "append", Some("TaskQueue"))],
        ),
        (
            "javascript",
            "stack.js",
            "class Stack {\n  push(x) { return x; }\n}\n\
             function fill(items, stack) {\n  items.push(1);\n  return stack.push(2);\n}\n",
            &[("fill", "push", Some("Stack"))],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let entity = |id| parsed.entities.iter().find(|e| e.id() == id);
        let mut calls: Vec<(String, String, Option<String>)> = parsed
            .relationships
            .iter()
            .filter(|r| r.relationship_type == RelationshipType::Calls)
            .filter_map(|r| {
                let target = entity(r.target_id)?;
                let owner = match target {
                    Entity::Function(f) => f.parent_class.clone(),
                    _ => None,
                };
                Some((
                    entity(r.source_id)?.name().to_string(),
                    target.name().to_string(),
                    owner,
                ))
            })
            .collect();
        calls.sort();
        let expected: Vec<(String, String, Option<String>)> = expected
            .iter()
            .map(|(source, target, owner)| {
                (
                    (*source).to_string(),
                    (*target).to_string(),
                    owner.map(str::to_string),
                )
            })
            .collect();
        assert_eq!(calls, expected, "{lang}");
    }
}

/// A method names the interface or trait it implements a member of where
/// its definition says so — Rust's `impl Display for`, C#'s
/// `IDisposable.Dispose`, F#'s `interface … with`, VB.NET's `Implements`
/// clause — and no other method does. A VB.NET class's `Implements`, which the
/// grammar splits from its types, names them.
#[test]
fn a_method_names_the_supertype_it_implements() {
    let cases = [
        (
            "rust",
            "cents.rs",
            "struct Cents;\nimpl std::fmt::Display for Cents {\n    \
             fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }\n}\n\
             impl<T> From<T> for Cents {\n    fn from(_: T) -> Self { Cents }\n}\n\
             impl Cents {\n    fn new() -> Self { Cents }\n}\n",
            vec![
                ("fmt", Some("Display")),
                ("from", Some("From")),
                ("new", None),
            ],
        ),
        (
            "c_sharp",
            "Res.cs",
            "class Res : IDisposable {\n    void IDisposable.Dispose() {}\n    public void Close() {}\n}\n",
            vec![("Dispose", Some("IDisposable")), ("Close", None)],
        ),
        (
            "fsharp",
            "Res.fs",
            "module M\ntype Res() =\n    member this.Close() = ()\n    \
             interface System.IDisposable with\n        member this.Dispose() = ()\n",
            vec![("Close", None), ("Dispose", Some("IDisposable"))],
        ),
        (
            "vb_net",
            "Res.vb",
            "Public Class Res\n    Implements IDisposable\n    \
             Public Sub Dispose() Implements IDisposable.Dispose\n    End Sub\n    \
             Public Sub Close()\n    End Sub\nEnd Class\n",
            vec![("Dispose", Some("IDisposable")), ("Close", None)],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let implements: Vec<(String, Option<String>)> = parsed
            .entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Function(f) => Some((f.name.clone(), f.implements.clone())),
                _ => None,
            })
            .collect();
        let expected: Vec<(String, Option<String>)> = expected
            .into_iter()
            .map(|(name, implements)| (name.to_string(), implements.map(str::to_string)))
            .collect();
        assert_eq!(implements, expected, "{lang}");
    }
    let src = "Public Class B\n    Inherits Base\n    Implements IDisposable, ICloneable\n\n    \
               Public Sub Close()\n    End Sub\nEnd Class\n\
               Public Class A\n    Implements IComparable\n    Public Sub Open()\n    End Sub\nEnd Class\n";
    let parsed = parse_content_sync(src, "vb_net", Path::new("Res.vb")).expect("parse");
    let mut supertypes: Vec<(String, String)> = parsed
        .refs
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                RelationshipType::Inherits | RelationshipType::Implements
            )
        })
        .filter_map(|r| {
            let source = parsed.entities.iter().find(|e| e.id() == r.source_id)?;
            Some((source.name().to_string(), r.target_name.clone()))
        })
        .collect();
    supertypes.sort();
    let supertype = |ty: &str, of: &str| (ty.to_string(), of.to_string());
    assert_eq!(
        supertypes,
        [
            supertype("A", "IComparable"),
            supertype("B", "Base"),
            supertype("B", "ICloneable"),
            supertype("B", "IDisposable"),
        ]
    );
}

/// A type names its methods, fields and supertypes, read from the type's body
/// and supertype clauses, not the type node's own children, where most
/// grammars put none: its fields, typed where they are declared with a type,
/// and its supertypes by name, without the clause's keyword (`Runnable`, not
/// `implements Runnable`).
#[test]
fn a_type_names_its_methods_fields_and_supertypes() {
    fn structure(src: &str, lang: &str, file: &str, name: &str) -> serde_json::Value {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let entity = parsed
            .entities
            .iter()
            .find(|e| e.name() == name && e.entity_type() != "function")
            .unwrap_or_else(|| panic!("{lang}: no {name}"));
        match entity {
            Entity::Class(c) => serde_json::json!({
                "methods": c.methods, "fields": c.fields,
                "extends": c.extends, "implements": c.implements,
            }),
            Entity::Struct(s) => serde_json::json!({ "methods": s.methods, "fields": s.fields }),
            Entity::Interface(i) => serde_json::json!({
                "methods": i.methods, "properties": i.properties, "extends": i.extends,
            }),
            other => panic!("{lang}: {name} is a {}", other.entity_type()),
        }
    }
    let cases: [(&str, &str, &str, &str, serde_json::Value); 13] = [
        (
            "rust",
            "cart.rs",
            "struct Cart { items: Vec<u32>, total: u64 }\nimpl Cart {\n    fn add(&mut self) {}\n}\n\
             trait Store { type Item; fn get(&self) -> Self::Item; }\n",
            "Cart",
            serde_json::json!({
                "methods": ["add"],
                "fields": [{"name": "items", "type": "Vec<u32>"}, {"name": "total", "type": "u64"}],
            }),
        ),
        (
            "rust",
            "store.rs",
            "trait Store { type Item; fn get(&self) -> Self::Item; }\n",
            "Store",
            serde_json::json!({ "methods": ["get"], "properties": null, "extends": null }),
        ),
        (
            "java",
            "Order.java",
            "class Order extends Base implements Runnable, Comparable<Order> {\n    \
             private int a, b;\n    public void run() {}\n}\n",
            "Order",
            serde_json::json!({
                "methods": ["run"],
                "fields": [{"name": "a", "type": "int"}, {"name": "b", "type": "int"}],
                "extends": "Base",
                "implements": "Runnable, Comparable",
            }),
        ),
        (
            "python",
            "order.py",
            "class Order(Base, Mixin):\n    \"\"\"An order.\"\"\"\n    count: int = 0\n    \
             label = 'x'\n\n    def total(self):\n        return 1\n",
            "Order",
            serde_json::json!({
                "methods": ["total"],
                "fields": [{"name": "count", "type": "int"}, {"name": "label"}],
                "extends": "Base, Mixin",
                "implements": null,
            }),
        ),
        (
            "go",
            "shape.go",
            "package shape\n\ntype Shape interface {\n\tArea() float64\n}\n\n\
             type Circle struct {\n\tName, Label string\n\tRadius float64\n}\n\n\
             func (c Circle) Area() float64 { return c.Radius }\n",
            "Circle",
            serde_json::json!({
                "methods": ["Area"],
                "fields": [
                    {"name": "Name", "type": "string"},
                    {"name": "Label", "type": "string"},
                    {"name": "Radius", "type": "float64"},
                ],
            }),
        ),
        (
            "go",
            "shape.go",
            "package shape\n\ntype Shape interface {\n\tArea() float64\n}\n",
            "Shape",
            serde_json::json!({ "methods": ["Area"], "properties": null, "extends": null }),
        ),
        (
            "typescript",
            "person.ts",
            "interface Person extends Named {\n  name: string;\n  greet(): void;\n}\n",
            "Person",
            serde_json::json!({
                "methods": ["greet"],
                "properties": [{"name": "name", "type": "string"}],
                "extends": ["Named"],
            }),
        ),
        (
            "kotlin",
            "Point.kt",
            "data class Point(val x: Int, val y: Int) {\n    fun norm(): Int = x\n}\n",
            "Point",
            serde_json::json!({
                "methods": ["norm"],
                "fields": [{"name": "x", "type": "Int"}, {"name": "y", "type": "Int"}],
                "extends": null,
                "implements": null,
            }),
        ),
        (
            "vb_net",
            "Point.vb",
            "Public Class Point\n    Inherits Base\n    Implements IComparable\n    \
             Public X As Double\n    \
             Public Shared Operator +(p1 As Point, p2 As Point) As Point\n        \
             Return New Point(p1.X + p2.X)\n    End Operator\n    \
             Public Sub Move()\n    End Sub\nEnd Class\n",
            "Point",
            serde_json::json!({
                "methods": ["Move"],
                "fields": [{"name": "X"}],
                "extends": "Base",
                "implements": "IComparable",
            }),
        ),
        (
            "cpp",
            "circle.cpp",
            "class Circle : public Shape {\n    double radius;\npublic:\n    double area();\n};\n\
             double Circle::area() { return radius; }\n",
            "Circle",
            serde_json::json!({
                "methods": ["area"],
                "fields": [{"name": "radius", "type": "double"}],
                "extends": "Shape",
                "implements": null,
            }),
        ),
        (
            "dart",
            "user.dart",
            "class User {\n  final int id;\n  User(this.id);\n  String label() => '$id';\n}\n",
            "User",
            serde_json::json!({
                "methods": ["User", "label"],
                "fields": [{"name": "id", "type": "int"}],
                "extends": null,
                "implements": null,
            }),
        ),
        (
            "ruby",
            "point.rb",
            "class Point\n  attr_accessor :x, :y\n\n  def norm\n    x\n  end\nend\n",
            "Point",
            serde_json::json!({
                "methods": ["norm"],
                "fields": [{"name": "x"}, {"name": "y"}],
                "extends": null,
                "implements": null,
            }),
        ),
        (
            "solidity",
            "shop.sol",
            "contract Shop {\n    struct Order {\n        uint id;\n        address buyer;\n    }\n}\n",
            "Order",
            serde_json::json!({
                "methods": null,
                "fields": [{"name": "id", "type": "uint"}, {"name": "buyer", "type": "address"}],
            }),
        ),
    ];
    for (lang, file, src, name, expected) in cases {
        assert_eq!(structure(src, lang, file, name), expected, "{lang} {name}");
    }
}

/// A nested function names the function it is defined in, past a callback
/// with no name of its own, so that two `inner`s of different functions are
/// two identities, whose ids do not move with their order.
#[test]
fn a_nested_function_names_the_function_it_is_in() {
    let src = "def first():\n    def inner():\n        return 1\n    return inner\n\n\
               def second():\n    def inner():\n        return 2\n    return inner\n";
    let parsed = parse_content_sync(src, "python", Path::new("nested.py")).expect("parse");
    let inners: Vec<(Option<String>, devscriptor_core::entities::EntityId)> = parsed
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Function(f) if f.name == "inner" => Some((f.parent_function.clone(), f.id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        inners
            .iter()
            .map(|(parent, _)| parent.as_deref())
            .collect::<Vec<_>>(),
        [Some("first"), Some("second")]
    );
    assert_ne!(inners[0].1, inners[1].1);
    let src = "function render(items) {\n  items.forEach(function (item) {\n    \
               function label() { return item; }\n    label();\n  });\n}\n";
    let parsed = parse_content_sync(src, "javascript", Path::new("render.js")).expect("parse");
    let label = parsed
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Function(f) if f.name == "label" => Some(f.parent_function.clone()),
            _ => None,
        })
        .expect("label");
    assert_eq!(label.as_deref(), Some("render"));
}

/// A type's references are to the types its fields are written with, not to
/// the fields: C#'s `Total` and Go's `Name` are no types the class uses.
#[test]
fn a_type_uses_its_fields_types_not_their_names() {
    let src = "class Cart {\n    public decimal Total { get; set; }\n    public Money Price;\n}\n";
    let parsed = parse_content_sync(src, "c_sharp", Path::new("Cart.cs")).expect("parse");
    let used: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::Uses)
        .map(|r| r.target_name.as_str())
        .collect();
    assert_eq!(used, ["Money"]);
}

/// A declaration's access and modifiers are read from its whole header, as
/// words, and the annotations inside it are its decorators: a Java or C#
/// method under its annotation has its access, Swift's `func openDoor()` is not
/// `open`, Solidity's and TypeScript's are read, and a Java, Kotlin, C# or Dart
/// method has its decorators.
#[test]
fn a_declaration_has_its_access_modifiers_and_annotations() {
    /// A function, and the access, modifiers and decorators it is to have.
    struct Case {
        lang: &'static str,
        file: &'static str,
        src: &'static str,
        function: &'static str,
        access: Option<&'static str>,
        modifiers: Option<&'static str>,
        decorators: &'static [&'static str],
    }
    let cases = [
        Case {
            lang: "java",
            file: "C.java",
            src: "class C {\n    @GetMapping(\"/x\")\n    public List<String> list() { return null; }\n}\n",
            function: "list",
            access: Some("public"),
            modifiers: None,
            decorators: &["GetMapping"],
        },
        Case {
            lang: "c_sharp",
            file: "C.cs",
            src: "class C {\n    [HttpGet]\n    public int Get() { return 0; }\n}\n",
            function: "Get",
            access: Some("public"),
            modifiers: None,
            decorators: &["HttpGet"],
        },
        Case {
            lang: "kotlin",
            file: "C.kt",
            src: "class C : B() {\n    @Test\n    fun works() {}\n}\n",
            function: "works",
            access: None,
            modifiers: None,
            decorators: &["Test"],
        },
        Case {
            lang: "kotlin",
            file: "D.kt",
            src: "class D : B() {\n    override fun toString(): String = \"\"\n}\n",
            function: "toString",
            access: None,
            modifiers: Some("override"),
            decorators: &[],
        },
        Case {
            lang: "dart",
            file: "a.dart",
            src: "class A extends B {\n  @override\n  int build() => 0;\n}\n",
            function: "build",
            access: None,
            modifiers: None,
            decorators: &["override"],
        },
        Case {
            lang: "swift",
            file: "Door.swift",
            src: "class Door {\n    func openDoor() {}\n}\n",
            function: "openDoor",
            access: None,
            modifiers: None,
            decorators: &[],
        },
        Case {
            lang: "solidity",
            file: "C.sol",
            src: "contract C {\n    function pay(uint v) external payable {}\n}\n",
            function: "pay",
            access: Some("external"),
            modifiers: Some("payable"),
            decorators: &[],
        },
        Case {
            lang: "typescript",
            file: "list.ts",
            src: "export function list() { return 1; }\n",
            function: "list",
            access: Some("export"),
            modifiers: None,
            decorators: &[],
        },
    ];
    for case in cases {
        let parsed = parse_content_sync(case.src, case.lang, Path::new(case.file)).expect("parse");
        let Some(found) = parsed.entities.iter().find_map(|e| match e {
            Entity::Function(f) if f.name == case.function => Some(f),
            _ => None,
        }) else {
            panic!("{}: no function {}", case.lang, case.function);
        };
        let what = format!("{} {}", case.lang, case.function);
        assert_eq!(found.access_modifiers.as_deref(), case.access, "{what}");
        assert_eq!(found.modifiers.as_deref(), case.modifiers, "{what}");
        assert_eq!(found.decorators, case.decorators, "{what}");
    }
}

/// A function or constant used as a value is referenced by what uses it —
/// a handler registered, a mapper, a callback, a method reference, a
/// constant passed — and a variable is no definition's reference: a
/// parameter, a local and a field of the caller's own object link nothing,
/// though the field is recorded as read. A registered handler is reached by
/// what registers it, as a function called by name is by its callers.
#[test]
fn a_function_passed_as_a_value_is_referenced() {
    let cases = [
        (
            "go",
            "routes.go",
            "package api\n\nfunc handleCheckout(w int) {}\ntype S struct{}\nfunc (s *S) handleIndex(w int) {}\n\
             func (s *S) routes(mux *Mux) {\n\tmux.HandleFunc(\"/checkout\", handleCheckout)\n\tmux.HandleFunc(\"/\", s.handleIndex)\n}\n",
            &[("routes", "handleCheckout"), ("routes", "handleIndex")][..],
        ),
        (
            "javascript",
            "list.js",
            "function format(x) { return x; }\nconst total = 3;\n\
             export function list(items) { return items.map(format).concat(total); }\n",
            &[("list", "format"), ("list", "total")],
        ),
        (
            "python",
            "panel.py",
            "class Panel:\n    def on_save(self):\n        pass\n    def wire(self, bus):\n        bus.register(self.on_save)\n",
            &[("wire", "on_save")],
        ),
        (
            "java",
            "Job.java",
            "class Job {\n    void work() {}\n    void start(Executor e) { e.submit(this::work); }\n}\n",
            &[("start", "work")],
        ),
        (
            "kotlin",
            "show.kt",
            "fun format(x: Int) = x\nfun show(xs: List<Int>) = xs.map(::format)\n",
            &[("show", "format")],
        ),
        // Through a qualifier, only the member of the type it names: not the
        // file's `MAX`, which `Other.MAX` and `o.MAX` are not.
        (
            "kotlin",
            "limits.kt",
            "const val MAX = 3\n\nobject Limits {\n    const val MIN = 1\n}\n\n\
             fun f(o: Other): Int = Other.MAX + Limits.MIN + o.MAX\n",
            &[("f", "MIN")],
        ),
        (
            "rust",
            "lines.rs",
            "struct Line;\nimpl Line { fn amount_cents(&self) -> u64 { 0 } }\n\
             fn total(lines: &[Line]) -> u64 { lines.iter().map(Line::amount_cents).sum() }\n",
            &[("total", "amount_cents")],
        ),
    ];
    for (lang, file, src, expected) in cases {
        let edges = hierarchy_edges(src, lang, file);
        let mut found: Vec<(&str, &str)> = edges
            .iter()
            .filter(|(_, ty, _)| ty == "references")
            .map(|(from, _, to)| (from.as_str(), to.as_str()))
            .collect();
        found.sort_unstable();
        assert_eq!(found, expected, "{lang}");
    }

    let src = "\
class Panel:
    def on_save(self):
        pass

    def wire(self, bus, timeout):
        size = 3
        bus.register(self.on_save, timeout, size, self.color)
";
    let parsed = parse_content_sync(src, "python", Path::new("panel.py")).expect("parse");
    let referenced: Vec<&str> = parsed
        .refs
        .iter()
        .filter(|r| r.kind == RelationshipType::References)
        .map(|r| r.target_name.as_str())
        .collect();
    assert_eq!(referenced, ["on_save", "color"]);
}

/// A Java lambda and a Go function literal are named by what they are bound
/// to, or not indexed: a lambda took its parameter's name (`r` of
/// `r -> r.isEmpty()`) and a Go literal its return type's (`int`). Of several
/// names bound at once, each literal takes its own.
#[test]
fn lambdas_are_named_by_their_binding_or_not_at_all() {
    let java = "class A {\n    void f(List<String> xs) {\n        xs.removeIf(r -> r.isEmpty());\n\
                Function<Integer, Integer> twice = x -> x * 2;\n    }\n}\n";
    let go = "package a\n\nfunc f() {\n\tgo func(r int) {}(1)\n\
              \ttwice := func(x int) int { return x * 2 }\n\t_ = twice\n\
              \tinc, dec := func(x int) int { return x + 1 }, func(x int) int { return x - 1 }\n\
              \t_, _ = inc, dec\n}\n\n\
              var sq, cube = func(x int) int { return x * x }, func(x int) int { return x * x * x }\n";
    for (src, lang, file, expected) in [
        (java, "java", "A.java", &["f", "twice"][..]),
        (
            go,
            "go",
            "a.go",
            &["f", "twice", "inc", "dec", "sq", "cube"][..],
        ),
    ] {
        let functions: Vec<String> = entities(src, lang, file)
            .into_iter()
            .filter(|(_, ty, _)| ty == "function")
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(functions, expected, "{lang}");
    }
}

/// A decorator, annotation or attribute is stored by its name, in every
/// language: Rust's without their `#[` and with their whole path (`allow`,
/// `tokio::test`), PHP's, and a TypeScript class's behind the `export` before
/// it.
#[test]
fn decorators_are_stored_by_name() {
    let cases: [(&str, &str, &str, &str, &[&str]); 4] = [
        (
            "#[allow(dead_code)]\n/// Documented.\n#[tokio::test]\nasync fn checks() {}\n",
            "rust",
            "lib.rs",
            "checks",
            &["allow", "tokio::test"],
        ),
        (
            "<?php\nclass P {\n    #[Route('/x'), Cache]\n    #[\\Override]\n    public function index() {}\n}\n",
            "php",
            "P.php",
            "index",
            &["Route", "Cache", "Override"],
        ),
        (
            "@Component({ selector: 'x' })\nexport class Widget {}\n",
            "typescript",
            "widget.ts",
            "Widget",
            &["Component"],
        ),
        (
            "@functools.lru_cache(maxsize=1)\ndef cached():\n    pass\n",
            "python",
            "cache.py",
            "cached",
            &["functools.lru_cache"],
        ),
    ];
    for (src, lang, file, name, expected) in cases {
        let parsed = parse_content_sync(src, lang, Path::new(file)).expect("parse");
        let decorators = parsed
            .entities
            .iter()
            .find_map(|entity| match entity {
                Entity::Function(function) if function.name == name => {
                    Some(function.decorators.clone())
                }
                Entity::Class(class) if class.name == name => Some(class.decorators.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{lang}: no {name}"));
        assert_eq!(decorators, expected, "{lang}");
    }
}

/// A call names its callee whole, as its definition does: a Ruby method's `?`
/// or `!`, a JavaScript private method's `#`, an F# name's prime. A call of
/// `same_day?` is no call of `same_day`, nor `this.#total()` of `total` or
/// `f'` of `f`.
#[test]
fn a_call_keeps_its_callee_name_whole() {
    let ruby = "\
module DispatchWindow
  def self.same_day?(t)
    true
  end
  def self.save!
  end
  def self.check(t)
    same_day?(t) && save!
  end
end
";
    let javascript =
        "class Cart {\n  #total() { return 1; }\n  check() { return this.#total(); }\n}\n";
    let fsharp = "module M\nlet f' x = x + 1\nlet check y = f' y\n";
    for (src, lang, file, callees) in [
        (
            ruby,
            "ruby",
            "dispatch_window.rb",
            &["same_day?", "save!"][..],
        ),
        (javascript, "javascript", "cart.js", &["#total"][..]),
        (fsharp, "fsharp", "M.fs", &["f'"][..]),
    ] {
        let edges = hierarchy_edges(src, lang, file);
        for callee in callees {
            assert!(
                edges.contains(&("check".into(), "calls".into(), (*callee).into())),
                "{lang} {callee}: {edges:?}"
            );
        }
    }
}

/// A script in a language no grammar here parses is left out, not read as
/// JavaScript.
#[test]
fn a_script_in_another_language_is_left_out() {
    let src = "<script lang=\"coffee\">\nsquare = (x) -> x * x\n</script>\n<p>hi</p>\n";
    for (lang, file) in [("svelte", "Square.svelte"), ("vue", "Square.vue")] {
        let ents = entities(src, lang, file);
        assert!(
            !ents.iter().any(|(n, _, _)| n == "square"),
            "{lang}: {ents:?}"
        );
    }
}

/// A GraphQL type is defined once however often it is used: the references in
/// field types (`subtotal: Cents!`) make no type-alias entities of their own.
#[test]
fn graphql_type_references_are_not_definitions() {
    let schema = "\"\"\"Money in integer cents.\"\"\"\nscalar Cents\n\n\
                  type Order {\n  id: ID!\n  subtotal: Cents!\n  total: Cents!\n}\n\n\
                  input CartLineInput {\n  unitPrice: Cents!\n}\n";
    let found = entities(schema, "graphql", "schema.graphql");
    let aliases: Vec<&str> = found
        .iter()
        .filter(|(_, ty, _)| ty == "type_alias")
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(aliases, ["Cents"], "{found:?}");
    for (name, ty) in [
        ("Order", "class"),
        ("CartLineInput", "struct"),
        ("subtotal", "function"),
    ] {
        assert!(
            found.iter().any(|(n, t, _)| n == name && t == ty),
            "{name} ({ty}) missing: {found:?}"
        );
    }
}

/// A query document defines its named operations and fragments. The fields,
/// directives and arguments it selects are uses of the schema, not functions,
/// macros and constants. An anonymous operation's selections are the
/// document's module's.
#[test]
fn graphql_operations_are_definitions_and_selections_are_not() {
    let document = "query GetOrder($id: ID!) {\n  order(id: $id) {\n    id\n    \
                    total @include(if: true)\n  }\n}\n\n\
                    fragment OrderParts on Order {\n  id\n}\n\n\
                    {\n  recentOrders {\n    id\n  }\n}\n";
    let found = entities(document, "graphql", "queries.graphql");
    let mut defined: Vec<(&str, &str)> = found
        .iter()
        .map(|(name, ty, _)| (name.as_str(), ty.as_str()))
        .collect();
    defined.sort_unstable();
    assert_eq!(
        defined,
        [
            ("GetOrder", "function"),
            ("OrderParts", "function"),
            ("queries", "module")
        ],
        "{found:?}"
    );
}

/// A selection calls the schema field it selects, and a directive use the
/// directive's definition.
#[test]
fn graphql_selections_call_their_definitions() {
    let document = "directive @audited on FIELD\n\n\
                    type Query {\n  order(id: ID!): Order\n}\n\n\
                    type Order {\n  id: ID!\n  total: Int\n}\n\n\
                    query GetOrder($id: ID!) {\n  order(id: $id) {\n    id\n    \
                    total @audited\n  }\n}\n";
    let edges = hierarchy_edges(document, "graphql", "orders.graphql");
    let calls = RelationshipType::Calls.as_str();
    for target in ["order", "id", "total", "audited"] {
        assert!(
            edges.contains(&("GetOrder".into(), calls.into(), target.into())),
            "GetOrder → {target} missing: {edges:?}"
        );
    }
}

/// Each import is on the line that names it: in an import of several names
/// over several lines, each name's own line, where a language server reports
/// it. `get_references` gives the import of a name among the uses the index
/// records, with that line.
/// A language, a file, its source, and its imports' local names and lines.
type ImportLines = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, u32)],
);

#[test]
fn each_import_is_on_the_line_that_names_it() {
    let cases: [ImportLines; 13] = [
        (
            "python",
            "a.py",
            "import os\nfrom shop.money import (\n    cents,\n    usd as dollars,\n)\n",
            &[("os", 1), ("cents", 3), ("dollars", 4)],
        ),
        (
            "rust",
            "a.rs",
            "use crate::money::{\n    cents,\n    usd as dollars,\n};\nuse std::fmt;\n",
            &[("cents", 2), ("dollars", 3), ("fmt", 5)],
        ),
        (
            "go",
            "a.go",
            "package main\n\nimport (\n\t\"fmt\"\n\th \"net/http\"\n)\n",
            &[("fmt", 4), ("h", 5)],
        ),
        (
            "typescript",
            "a.ts",
            "import {\n  a,\n  b as c,\n} from './m';\nimport * as ns from './n';\nimport D from './d';\n",
            &[("a", 2), ("c", 3), ("ns", 5), ("D", 6)],
        ),
        (
            "javascript",
            "a.js",
            "const x = 1;\nconst fs = require('fs');\n",
            &[("fs", 2)],
        ),
        (
            "java",
            "A.java",
            "package a;\n\nimport java.util.List;\nimport java.util.*;\n",
            &[("List", 3), ("*", 4)],
        ),
        (
            "kotlin",
            "A.kt",
            "package a\n\nimport a.b.C\nimport a.b.D as E\n",
            &[("C", 3), ("E", 4)],
        ),
        (
            "solidity",
            "A.sol",
            "pragma solidity ^0.8.0;\nimport {\n  A,\n  B as C\n} from \"./x.sol\";\n",
            &[("A", 3), ("C", 4)],
        ),
        (
            "swift",
            "A.swift",
            "import Foundation\n\nimport UIKit\n",
            &[("Foundation", 1), ("UIKit", 3)],
        ),
        (
            "ruby",
            "a.rb",
            "require 'json'\n\nrequire_relative 'money'\n",
            &[("json", 1), ("money", 3)],
        ),
        (
            "dart",
            "a.dart",
            "import 'package:a/b.dart';\n\nimport 'c.dart' as c;\nimport 'd.dart'\n    show d1, d2;\nexport 'e.dart';\npart 'f.dart';\n",
            &[("*", 1), ("c", 3), ("d1", 5), ("d2", 5), ("f.dart", 7)],
        ),
        (
            "c",
            "a.c",
            "#include <stdio.h>\n\n#include \"money.h\"\n",
            &[("stdio.h", 1), ("money.h", 3)],
        ),
        (
            "php",
            "a.php",
            "<?php\nnamespace App;\n\nuse App\\Money\\Cents;\nuse App\\{\n    A,\n    B as C\n};\nrequire 'boot.php';\n",
            &[("Cents", 4), ("A", 6), ("C", 7), ("boot.php", 9)],
        ),
    ];
    for (language, path, src, expected) in cases {
        let parsed = parse_content_sync(src, language, Path::new(path)).expect("parse");
        let found: Vec<(&str, u32)> = parsed
            .imports
            .iter()
            .map(|i| (i.local_name.as_str(), i.line))
            .collect();
        assert_eq!(found, expected, "{language}");
    }
}
