//! Entity extraction from tree-sitter parse trees

use crate::Result;
use crate::entities::{
    Class, Component, Constant, Contract, Decorator, Entity, Enum, Extension, Function, Interface,
    Macro, Module, Namespace, SourceLocation, Struct, Trait, TypeAlias, Union,
};
use crate::parser::TRUNCATION_MARKER;
use crate::parser::language_config::{LanguageNodeConfig, get_language_config, matches_node_type};
use crate::parser::relationship_extraction::base_type_name;
use std::collections::HashMap;
use std::sync::LazyLock;
use time::OffsetDateTime;
use tree_sitter::Node;
use uuid::Uuid;

mod constants;
mod functions;
mod modules;
mod names;
mod nodes;
mod other_kinds;
#[cfg(test)]
mod tests;
mod types;

use constants::*;
use functions::*;
pub(crate) use modules::file_module;
use modules::*;
use names::*;
use nodes::*;
use other_kinds::*;
use types::*;

/// Extract all entities from a parse tree
pub fn extract_entities(
    tree: &tree_sitter::Tree,
    content: &str,
    language: &str,
    file_path: &str,
) -> Result<Vec<Entity>> {
    let root = tree.root_node();
    let mut entities = Vec::new();
    let config = get_language_config(language);
    let now = OffsetDateTime::now_utc();

    traverse_and_extract(
        root,
        content,
        language,
        file_path,
        &mut entities,
        config,
        now,
        0,
    );
    attach_methods(&mut entities);

    Ok(entities)
}

/// Each type's methods: the names of the file's functions that are its
/// members, by its name, wherever defined — a Rust `impl`, a Go receiver, a
/// C++ `int Cart::total()` — and, for an extension, inside it, not among the
/// type node's own children, where no grammar puts them.
fn attach_methods(entities: &mut [Entity]) {
    /// A member function: its first and last lines, and its name.
    type Member<'e> = ((u32, u32), &'e str);
    // The functions that are members, by their type's name, in the order they
    // start.
    let mut members: std::collections::HashMap<&str, Vec<Member>> =
        std::collections::HashMap::new();
    for entity in entities.iter() {
        if let Entity::Function(f) = entity
            && let Some(owner) = f.parent_class.as_deref()
        {
            members.entry(owner).or_default().push((
                (f.location.start_line, f.location.end_line),
                f.name.as_str(),
            ));
        }
    }
    for list in members.values_mut() {
        list.sort_by_key(|((start, _), _)| *start);
    }
    let mut attached = Vec::new();
    for (at, entity) in entities.iter().enumerate() {
        let location = entity.location();
        let (name, inside) = match entity {
            Entity::Class(c) => (c.name.as_str(), false),
            Entity::Struct(s) => (s.name.as_str(), false),
            Entity::Enum(e) => (e.name.as_str(), false),
            Entity::Interface(i) => (i.name.as_str(), false),
            Entity::Trait(t) => (t.name.as_str(), false),
            Entity::Component(c) => (c.name.as_str(), false),
            Entity::Extension(e) => match e.extended_type.as_deref() {
                Some(extended) => (extended, true),
                None => continue,
            },
            _ => continue,
        };
        let Some(list) = members.get(name) else {
            continue;
        };
        // An extension's are those within it: from the first that starts in
        // it, while they start in it.
        let list = if inside {
            let first = list.partition_point(|((start, _), _)| *start < location.start_line);
            let past = first
                + list[first..].partition_point(|((start, _), _)| *start <= location.end_line);
            &list[first..past]
        } else {
            &list[..]
        };
        let mut seen = std::collections::HashSet::new();
        let names: Vec<String> = list
            .iter()
            .filter(|((_, end), _)| !inside || *end <= location.end_line)
            .filter(|(_, method)| seen.insert(*method))
            .map(|(_, method)| (*method).to_string())
            .collect();
        // A Go interface's methods are no functions; it names them itself.
        if !names.is_empty() {
            attached.push((at, names));
        }
    }
    for (at, names) in attached {
        let methods = match &mut entities[at] {
            Entity::Class(c) => &mut c.methods,
            Entity::Struct(s) => &mut s.methods,
            Entity::Enum(e) => &mut e.methods,
            Entity::Interface(i) => &mut i.methods,
            Entity::Trait(t) => &mut t.methods,
            Entity::Component(c) => &mut c.methods,
            Entity::Extension(e) => &mut e.methods,
            _ => continue,
        };
        *methods = Some(names);
    }
}

/// Maximum AST nesting depth for entity traversal.
///
/// Tree-sitter grammars for well-formed source rarely exceed depth ~40.
/// A hard cap at 128 prevents stack overflow on pathologically deep or
/// machine-generated files while still covering all real-world code.
const MAX_TRAVERSE_DEPTH: usize = 128;

/// Recursively traverse the tree and extract entities.
///
/// `depth` is incremented on each recursive call and the traversal stops at
/// [`MAX_TRAVERSE_DEPTH`] to guard against stack overflow on very deep ASTs.
#[allow(clippy::too_many_arguments)]
fn traverse_and_extract(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    entities: &mut Vec<Entity>,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
    depth: usize,
) {
    if depth > MAX_TRAVERSE_DEPTH {
        return;
    }

    // Try to extract an entity from this node. A keyword or punctuation token
    // is none, though some share their node's kind: Ruby's `module` keyword
    // is a `module` too, and taken as one without a name it would be a module
    // named after the file, beside every module the file declares.
    if node.is_named()
        && let Ok(Some(entity)) =
            try_extract_entity(node, content, language, file_path, config, now)
    {
        entities.push(entity);
    }

    // Recursively traverse children (depth already checked at entry)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_and_extract(
            child,
            content,
            language,
            file_path,
            entities,
            config,
            now,
            depth + 1,
        );
    }
}

/// Try to extract an entity from a node
fn try_extract_entity(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let node_type = node.kind();

    // `struct Point origin;`, `void move(struct Point *p)` and the forward
    // declaration `struct Point;` name a C-family type without defining it,
    // and are no entity: each would be one more `Point` beside the definition.
    if names_a_type_without_defining_it(node) {
        return Ok(None);
    }

    if matches!(language, "objc" | "objective_c") {
        if node_type == "macro_type_specifier" {
            return Ok(objc_macro_enum(node, content, language, file_path, now));
        }
        // The `typedef` around one names its first enumerator by mistake.
        if node_type == "type_definition"
            && node
                .named_child(0)
                .is_some_and(|ty| objc_enum_macro_arguments(ty, content).is_some())
        {
            return Ok(None);
        }
    }

    // Function nodes, methods included: every method is a Function entity of
    // its own, with its body, location, vector and place in the call graph, and
    // `extract_function` gives it its `parent_class` from the parent node.
    if matches_node_type(node_type, &config.function_nodes) {
        return extract_function(node, content, language, file_path, config, now);
    }

    // Check class nodes
    if matches_node_type(node_type, &config.class_nodes) {
        // Kotlin has no distinct interface node: an interface is a
        // `class_declaration` carrying an `interface` keyword instead of
        // `class`. Left to the class branch it would be mislabelled a class,
        // which also breaks `implements` vs `inherits` classification down the
        // line. Route it to the interface extractor.
        if language == "kotlin" && kotlin_node_is_interface(node) {
            return extract_interface(node, content, language, file_path, config, now);
        }
        if language == "kotlin" && kotlin_node_is_enum(node) {
            return extract_enum(node, content, language, file_path, now);
        }
        if language == "fsharp" && fsharp_type_is_interface(node, content) {
            return extract_interface(node, content, language, file_path, config, now);
        }
        if language == "swift" {
            return extract_swift_type(node, content, language, file_path, now);
        }
        if matches!(language, "objc" | "objective_c")
            && let Some(category) = objc_category(node, content, language, file_path, now)
        {
            return Ok(Some(category));
        }
        // A C# `record struct` is a struct, as a `struct` is.
        if language == "c_sharp" && node_type == "record_declaration" {
            let mut cursor = node.walk();
            if node
                .children(&mut cursor)
                .any(|child| child.kind() == "struct")
            {
                return extract_struct(node, content, language, file_path, now);
            }
        }
        // SQL `CREATE TABLE/VIEW/…` parses through the class branch but is a
        // relational table, not a class. Re-tag it so it embeds and reads as a
        // `table` (better semantic recall) and stays out of code-clone detection.
        if language == "sql" {
            // `CREATE INDEX` also parses through the class branch, but an index
            // is not a relational container — typing it `table` would answer
            // "which tables exist?" with index names.
            let is_index = node.kind() == "create_index";
            return Ok(
                extract_class(node, content, language, file_path, now)?.map(|e| match e {
                    Entity::Class(c) if is_index => Entity::Index(c),
                    Entity::Class(c) => Entity::Table(c),
                    other => other,
                }),
            );
        }
        return extract_class(node, content, language, file_path, now);
    }

    // Check struct nodes
    if matches_node_type(node_type, &config.struct_nodes) {
        if language == "go" {
            return extract_go_type_spec(node, content, language, file_path, config, now);
        }
        return extract_struct(node, content, language, file_path, now);
    }

    // Check enum nodes
    if matches_node_type(node_type, &config.enum_nodes) {
        return extract_enum(node, content, language, file_path, now);
    }

    // Check interface nodes
    if matches_node_type(node_type, &config.interface_nodes) {
        return extract_interface(node, content, language, file_path, config, now);
    }

    // Check trait nodes
    if matches_node_type(node_type, &config.trait_nodes) {
        return extract_trait(node, content, language, file_path, now);
    }

    // Check module nodes
    if matches_node_type(node_type, &config.module_nodes) {
        return extract_module(node, content, language, file_path, now);
    }

    // Check namespace nodes
    if matches_node_type(node_type, &config.namespace_nodes) {
        return extract_namespace(node, content, language, file_path, now);
    }

    // Check type alias nodes
    if matches_node_type(node_type, &config.type_alias_nodes) {
        if language == "sql"
            && let Some(labels) = sql_enum_labels(node, content)
        {
            return Ok(
                extract_enum(node, content, language, file_path, now)?.map(|entity| match entity {
                    Entity::Enum(mut e) => {
                        e.variants = Some(serde_json::json!(labels));
                        Entity::Enum(e)
                    }
                    other => other,
                }),
            );
        }
        return extract_type_alias(node, content, language, file_path, now);
    }

    // Check constant nodes
    if matches_node_type(node_type, &config.constant_nodes) {
        return extract_constant(node, content, language, file_path, config, now);
    }

    // Check macro nodes
    if matches_node_type(node_type, &config.macro_nodes) {
        return extract_macro(node, content, language, file_path, now);
    }

    // Check component nodes (for framework languages like Vue, Svelte)
    if matches_node_type(node_type, &config.component_nodes) {
        return extract_component(node, content, language, file_path, config, now);
    }

    if defines_a_union(node) {
        if language == "fsharp" && fsharp_union_abbreviates(node, content) {
            return extract_type_alias(node, content, language, file_path, now);
        }
        return extract_union(node, content, language, file_path, now);
    }

    // Check for contract declarations (Solidity)
    if node_type.contains("contract") {
        return extract_contract(node, content, language, file_path, now);
    }

    if defines_an_extension(node, language) {
        return extract_extension(node, content, language, file_path, now);
    }

    // Check for decorators/annotations (Python, Java, TS)
    if node_type.contains("decorator")
        || node_type.contains("annotation")
        || node_type == "attribute"
    {
        return extract_decorator(node, content, language, file_path, now);
    }

    Ok(None)
}

/// Whether this node is a C-family `struct`, `union`, `enum` or `class` that
/// has no body: it names the type — as a variable's or a parameter's type, in a
/// field pointing to its own struct, in a forward declaration — and defines it
/// only where it has one.
fn names_a_type_without_defining_it(node: Node) -> bool {
    matches!(
        node.kind(),
        "struct_specifier" | "union_specifier" | "enum_specifier" | "class_specifier"
    ) && node.child_by_field_name("body").is_none()
}

/// Whether this node defines a union: a C-family `union` with a body, a Rust
/// `union`, an F# discriminated union and a GraphQL `union` (an `extend union`
/// is an extension of one). A TypeScript, PHP or Python union *type*
/// (`Order | null`) joins types defined elsewhere, and an F# union's cases and
/// fields are parts of one: matched as unions by the word in their kind, each
/// `Order | null` would be a `union Order`, and each F# case a union of its own.
fn defines_a_union(node: Node) -> bool {
    matches!(
        node.kind(),
        "union_specifier" | "union_item" | "union_type_defn" | "union_type_definition"
    )
}

/// Whether this node declares an extension of a type defined elsewhere — a
/// Dart `extension … on`, an F# type extension (`type System.String with …`),
/// a GraphQL `extend type|interface|enum|input|scalar|union` — or, in SQL,
/// installs one (`CREATE EXTENSION`). A Swift extension is a
/// `class_declaration` and an Objective-C category a `class_interface`
/// (`extract_swift_type`, `objc_category`). Matched by the word in their kind,
/// C's `__extension__` expressions and SQL's `DROP EXTENSION` would be
/// extensions too; and a GraphQL `extend type` is no second class of its
/// type's name.
fn defines_an_extension(node: Node, language: &str) -> bool {
    match language {
        "dart" => node.kind() == "extension_declaration",
        "fsharp" => node.kind() == "type_extension",
        "graphql" => matches!(
            node.kind(),
            "object_type_extension"
                | "interface_type_extension"
                | "enum_type_extension"
                | "input_object_type_extension"
                | "scalar_type_extension"
                | "union_type_extension"
        ),
        "sql" => node.kind() == "create_extension",
        _ => false,
    }
}
