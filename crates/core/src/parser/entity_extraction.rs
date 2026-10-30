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
    let members: Vec<(String, (u32, u32), String)> = entities
        .iter()
        .filter_map(|entity| match entity {
            Entity::Function(f) => Some((
                f.parent_class.clone()?,
                (f.location.start_line, f.location.end_line),
                f.name.clone(),
            )),
            _ => None,
        })
        .collect();
    for entity in entities.iter_mut() {
        let location = entity.location();
        let (name, methods, inside) = match entity {
            Entity::Class(c) => (c.name.as_str(), &mut c.methods, false),
            Entity::Struct(s) => (s.name.as_str(), &mut s.methods, false),
            Entity::Enum(e) => (e.name.as_str(), &mut e.methods, false),
            Entity::Interface(i) => (i.name.as_str(), &mut i.methods, false),
            Entity::Trait(t) => (t.name.as_str(), &mut t.methods, false),
            Entity::Component(c) => (c.name.as_str(), &mut c.methods, false),
            Entity::Extension(e) => match e.extended_type.as_deref() {
                Some(extended) => (extended, &mut e.methods, true),
                None => continue,
            },
            _ => continue,
        };
        let mut names: Vec<String> = Vec::new();
        for (owner, (start, end), method) in &members {
            let placed = !inside || (location.start_line <= *start && *end <= location.end_line);
            if owner == name && placed && !names.contains(method) {
                names.push(method.clone());
            }
        }
        // A Go interface's methods are no functions; it names them itself.
        if !names.is_empty() {
            *methods = Some(names);
        }
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

/// Extract the name from a node using various strategies
fn extract_node_name(node: Node, content: &str, language: &str) -> Option<String> {
    // HTML markup has no `name` field anywhere, so every element fell through to
    // `extract_component`'s file-stem fallback — which is right for Vue and
    // Svelte, where a file *is* one component, but in HTML it named every
    // element after the page. A five-element document produced five entities all
    // called `index`, colliding in search and in the graph.
    if language == "html"
        && let Some(tag) = html_element_name(node, content)
    {
        return Some(tag);
    }

    // A JavaScript or TypeScript class expression is named by its own name,
    // else by what it is bound to (`const Cart = class { … }`).
    if matches!(language, "javascript" | "typescript" | "tsx") && node.kind() == "class" {
        return node
            .child_by_field_name("name")
            .map(|name| node_text(name, content).trim().to_string())
            .or_else(|| js_function_name(node, content));
    }

    // A Solidity `receive()` or `fallback()` is named by its keyword.
    if node.kind() == "fallback_receive_definition" {
        return node
            .child(0)
            .map(|keyword| node_text(keyword, content).trim().to_string());
    }

    // A Kotlin secondary constructor is named by its keyword, as Kotlin
    // declares it.
    if language == "kotlin" && node.kind() == "secondary_constructor" {
        return Some("constructor".to_string());
    }

    // A Kotlin property names its one binding in its `variable_declaration`;
    // a destructuring `val (a, b)` binds several and stays unnamed.
    if language == "kotlin" && node.kind() == "property_declaration" {
        let mut cursor = node.walk();
        let declaration = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "variable_declaration")?;
        let mut cursor = declaration.walk();
        return declaration
            .named_children(&mut cursor)
            .find(|child| child.kind() == "identifier")
            .map(|name| node_text(name, content).trim().to_string());
    }

    // A Swift deinitializer or subscript is named by its keyword; a
    // subscript's `name` field holds a type.
    if language == "swift" {
        match node.kind() {
            "deinit_declaration" => return Some("deinit".to_string()),
            "subscript_declaration" => return Some("subscript".to_string()),
            _ => {}
        }
    }

    // If the node *is* an identifier — reached by recursing through a `declarator`
    // chain, e.g. the nested `identifier` inside a C/C++ `function_declarator` —
    // return its own text. Without this, C-style free functions (whose name is
    // buried under `function_definition → function_declarator → identifier`)
    // resolve to no name and are dropped entirely by `extract_function`.
    if matches!(
        node.kind(),
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "variable_identifier"
            | "property_identifier"
    ) {
        let name = node_text(node, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try "name" field first (most common)
    if let Some(name_node) = node.child_by_field_name("name") {
        // An F# member's is `this.Area`: the instance, then the member. A Swift
        // property's is a pattern, binding one name or a tuple of them.
        let name_node = match name_node.kind() {
            "property_or_ident" => {
                Some(name_node.child_by_field_name("method").unwrap_or(name_node))
            }
            "pattern" => name_node.child_by_field_name("bound_identifier"),
            _ => Some(name_node),
        };
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    // Try "identifier" field
    if let Some(ident_node) = node.child_by_field_name("identifier") {
        let name = node_text(ident_node, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try "declarator" field (C-style). A C/C++/CUDA `function_definition` nests
    // its name under `declarator: function_declarator → declarator: identifier`
    // (and one more `*_declarator` level for pointer/reference returns), so
    // recurse — but *fall through* to the sibling strategies below when the
    // recursion is fruitless, never short-circuiting to `None`.
    if let Some(decl_node) = node.child_by_field_name("declarator")
        && let Some(name) = extract_node_name(decl_node, content, language)
    {
        return Some(name);
    }

    // A PHP constant names itself in its first `const_element`.
    if node.kind() == "const_declaration" {
        let mut cursor = node.walk();
        let element = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "const_element");
        if let Some(element) = element {
            let mut cursor = element.walk();
            if let Some(name) = element
                .named_children(&mut cursor)
                .find(|part| part.kind() == "name")
            {
                return Some(node_text(name, content).trim().to_string());
            }
        }
    }

    // A C# field names itself in the first declarator of its
    // `variable_declaration`, a VB.NET field in its first declarator.
    if node.kind() == "field_declaration"
        && let Some(declarator) = first_variable_declarator(node)
        && let Some(name) = declarator.child_by_field_name("name")
    {
        let name = node_text(name, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try to find first identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "identifier"
            || kind == "type_identifier"
            || kind == "variable_identifier"
            || kind == "property_identifier"
            || kind == "field_identifier"
            || kind == "string"
        {
            let name = node_text(child, content).trim().to_string();
            if !name.is_empty() && !name.starts_with('"') {
                return Some(name.trim_matches('"').to_string());
            }
        }
    }

    // Language-specific name extraction
    extract_language_specific_name(node, content, language)
}

// ---------------------------------------------------------------------------
// Language-specific name extractors — registered in a static dispatch table
// ---------------------------------------------------------------------------

/// Handler signature for language-specific name extraction.
type NameExtractor = fn(Node, &str) -> Option<String>;

/// Lazily-built HashMap from canonical language name → extractor function.
/// New languages should register their extractor here instead of adding
/// another `match` arm.
static LANGUAGE_NAME_EXTRACTORS: LazyLock<HashMap<&'static str, NameExtractor>> =
    LazyLock::new(|| {
        let mut m = HashMap::<&'static str, NameExtractor>::new();
        m.insert("bash", extract_bash_name);
        m.insert("sh", extract_bash_name);
        m.insert("vb_net", extract_vb_net_name);
        m.insert("fsharp", extract_fsharp_name);
        m.insert("graphql", extract_graphql_name);
        m.insert("sql", extract_sql_name);
        m.insert("dart", extract_dart_name);
        m
    });

// ---------------------------------------------------------------------------
// Per-language extractor implementations
// ---------------------------------------------------------------------------

fn extract_bash_name(node: Node, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "word" || kind == "identifier" || kind == "function_name" {
            let text = node_text(child, content).trim().to_string();
            if !text.is_empty() && text != "function" && !text.starts_with('(') {
                return Some(text);
            }
        }
    }
    None
}

/// SQL `CREATE TABLE/VIEW/…` wraps the object's name one level down:
/// `create_table → object_reference → name: identifier`. The generic strategies
/// look only at direct `name`/`identifier` children, so without this the table
/// name is never found and the table entity is dropped — only its
/// `column_definition` children survive (misleadingly, as `struct`s).
fn extract_sql_name(node: Node, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "object_reference" {
            if let Some(name) = child.child_by_field_name("name") {
                let n = node_text(name, content).trim().to_string();
                if !n.is_empty() {
                    return Some(n);
                }
            }
            let mut inner = child.walk();
            for gc in child.children(&mut inner) {
                if gc.kind() == "identifier" {
                    let n = node_text(gc, content).trim().to_string();
                    if !n.is_empty() {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

/// The first `variable_declarator` of a field: its child, or its
/// `variable_declaration`'s.
fn first_variable_declarator(field: Node) -> Option<Node> {
    let mut cursor = field.walk();
    let children: Vec<Node> = field.named_children(&mut cursor).collect();
    children.iter().find_map(|child| match child.kind() {
        "variable_declarator" => Some(*child),
        "variable_declaration" => {
            let mut inner = child.walk();
            child
                .named_children(&mut inner)
                .find(|part| part.kind() == "variable_declarator")
        }
        _ => None,
    })
}

/// VB.NET names a constructor `New`.
fn extract_vb_net_name(node: Node, _content: &str) -> Option<String> {
    (node.kind() == "constructor_declaration").then(|| "New".to_string())
}

/// F# keeps a type's name in its definition's `type_name` (`Result` of
/// `Result<'T, 'E>`), an exception's in its `exception_name`, a module's or
/// namespace's in its `long_identifier` or `identifier`, and a function's or
/// value's in its `function_declaration_left`.
fn extract_fsharp_name(node: Node, content: &str) -> Option<String> {
    let text = |part: Node| {
        let name = node_text(part, content).trim().to_string();
        (!name.is_empty()).then_some(name)
    };
    let mut cursor = node.walk();
    match node.kind() {
        "exception_definition" => text(node.child_by_field_name("exception_name")?),
        "named_module" | "module_defn" | "namespace" => node
            .children(&mut cursor)
            .find(|child| matches!(child.kind(), "long_identifier" | "identifier"))
            .and_then(text),
        "function_or_value_defn" => {
            for child in node.children(&mut cursor) {
                if child.kind() == "function_declaration_left" {
                    let mut inner_cursor = child.walk();
                    for inner in child.children(&mut inner_cursor) {
                        if inner.kind() == "identifier" {
                            return text(inner);
                        }
                    }
                }
                if child.kind() == "identifier" {
                    return text(child);
                }
            }
            None
        }
        _ => {
            let type_name = node
                .children(&mut cursor)
                .find(|child| child.kind() == "type_name")?;
            text(type_name.child_by_field_name("type_name")?)
        }
    }
}

/// An F# type with no constructor, no base type and only abstract members is
/// an interface (`type IGreeter = abstract Greet: string -> string`): F#
/// infers it, with no keyword to read, unless `[<Class>]` or
/// `[<AbstractClass>]` says otherwise.
fn fsharp_type_is_interface(node: Node, content: &str) -> bool {
    if node.kind() != "anon_type_defn" {
        return false;
    }
    let declared_a_class = node.parent().is_some_and(|definition| {
        let mut cursor = definition.walk();
        definition.children(&mut cursor).any(|child| {
            child.kind() == "attributes" && node_text(child, content).contains("Class")
        })
    });
    if declared_a_class {
        return false;
    }
    let mut members = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "primary_constr_args" | "class_inherits_decl" | "interface_implementation" => {
                return false;
            }
            "member_defn" => members.push(child),
            "type_extension_elements" => {
                let mut elements = child.walk();
                for element in child.named_children(&mut elements) {
                    if element.kind() != "member_defn" {
                        return false;
                    }
                    members.push(element);
                }
            }
            _ => {}
        }
    }
    !members.is_empty()
        && members.iter().all(|member| {
            let mut parts = member.walk();
            member
                .named_children(&mut parts)
                .any(|part| part.kind() == "member_signature")
        })
}

/// `type Id = int` parses as a union of one case, `int`, though it abbreviates
/// `int`: a lone case with no fields and no `|` before it, named other than the
/// type itself (`type Marker = Marker` is a union).
fn fsharp_union_abbreviates(node: Node, content: &str) -> bool {
    let mut cursor = node.walk();
    let Some(cases) = node
        .children(&mut cursor)
        .find(|child| child.kind() == "union_type_cases")
    else {
        return false;
    };
    let mut cursor = cases.walk();
    let [case] = cases.children(&mut cursor).collect::<Vec<_>>()[..] else {
        return false;
    };
    let mut cursor = case.walk();
    let [identifier] = case.children(&mut cursor).collect::<Vec<_>>()[..] else {
        return false;
    };
    identifier.kind() == "identifier"
        && extract_fsharp_name(node, content).as_deref()
            != Some(node_text(identifier, content).trim())
}

/// The type an F# member belongs to: the nearest type definition above it,
/// past its `member_defn`, the block of members and an `interface … with`. A
/// member of an object expression (`{ new IDisposable with … }`) belongs to
/// no named type.
fn fsharp_member_owner(node: Node) -> Option<Node> {
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        match candidate.kind() {
            "anon_type_defn"
            | "record_type_defn"
            | "union_type_defn"
            | "interface_type_defn"
            | "exception_definition"
            | "type_extension" => return Some(candidate),
            "object_expression" | "function_or_value_defn" | "type_definition" => return None,
            _ => ancestor = candidate.parent(),
        }
    }
    None
}

/// Dart keeps a declaration's name in its signature: a function, getter,
/// setter or method `*_declaration` in its `signature` field — a method's
/// wrapped once more in a `method_signature` — a local function in its
/// `function_signature`, and a member without a body in the signature its
/// `declaration` holds. A `declaration` of fields holds none and gets no name:
/// its fields are entities of their own.
fn extract_dart_name(node: Node, content: &str) -> Option<String> {
    match node.kind() {
        "function_signature" | "getter_signature" | "setter_signature" => {
            let name = node.child_by_field_name("name")?;
            Some(node_text(name, content).to_string())
        }
        // `Point`, or `Point.fromJson` for a named constructor: the `name`
        // field is each part and the dot between them.
        "constructor_signature"
        | "constant_constructor_signature"
        | "factory_constructor_signature"
        | "redirecting_factory_constructor_signature" => {
            let mut cursor = node.walk();
            let name: String = node
                .children_by_field_name("name", &mut cursor)
                .map(|part| node_text(part, content))
                .collect();
            (!name.is_empty()).then_some(name)
        }
        "operator_signature" => {
            let operator = node.child_by_field_name("operator")?;
            Some(format!("operator {}", node_text(operator, content)))
        }
        // `class Mixed = Base with Mixin;` names the class in its
        // `mixin_application_class`.
        "class_declaration" => {
            let application = named_child_of_kind(node, "mixin_application_class")?;
            let name = named_child_of_kind(application, "identifier")?;
            Some(node_text(name, content).to_string())
        }
        // `library shop.cart;`
        "library_name" => {
            let name = named_child_of_kind(node, "dotted_identifier_list")?;
            Some(node_text(name, content).to_string())
        }
        _ => {
            let signature = node.child_by_field_name("signature").or_else(|| {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .find(|child| child.kind().ends_with("signature"))
            })?;
            extract_dart_name(signature, content)
        }
    }
}

/// The name of the type a function is a member of: the nearest class, struct,
/// interface, trait or enum around it, past the body that holds its members
/// (`class_body`, `declaration_list`, …); the type a Rust `impl` is for; a Go
/// method's receiver type; the class a C++ method defined outside it names
/// (`Cart::total`). A function inside another function, or at the top of a
/// module, is a member of no type.
fn member_owner_name(
    node: Node,
    content: &str,
    language: &str,
    config: &LanguageNodeConfig,
) -> Option<String> {
    let owner = match language {
        "dart" => {
            return dart_member_owner(node).and_then(|o| extract_node_name(o, content, language));
        }
        "fsharp" => {
            return fsharp_member_owner(node).and_then(|o| extract_node_name(o, content, language));
        }
        "go" => {
            let receiver = node.child_by_field_name("receiver")?;
            let mut cursor = receiver.walk();
            let parameter = receiver
                .named_children(&mut cursor)
                .find(|part| part.kind() == "parameter_declaration")?;
            let ty = parameter.child_by_field_name("type")?;
            return Some(base_type_name(ty, content)).filter(|name| !name.is_empty());
        }
        "cpp" | "cuda" => qualified_definition_scope(node, content),
        _ => None,
    };
    if owner.is_some() {
        return owner;
    }
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        let kind = candidate.kind();
        if matches_node_type(kind, &config.function_nodes) {
            return None;
        }
        if kind == "impl_item" {
            let ty = candidate.child_by_field_name("type")?;
            return Some(base_type_name(ty, content)).filter(|name| !name.is_empty());
        }
        // A type with no name (a Kotlin `companion object`) is passed for the
        // one around it.
        if is_type_node(candidate, language, config)
            && let Some(name) = extract_node_name(candidate, content, language)
        {
            return Some(name);
        }
        ancestor = candidate.parent();
    }
    None
}

/// Whether `node` declares a type whose members the functions inside it are.
fn is_type_node(node: Node, language: &str, config: &LanguageNodeConfig) -> bool {
    let kind = node.kind();
    [
        &config.class_nodes,
        &config.struct_nodes,
        &config.interface_nodes,
        &config.trait_nodes,
        &config.enum_nodes,
    ]
    .iter()
    .any(|kinds| matches_node_type(kind, kinds))
        || matches!(kind, "class_definition" | "class_declaration" | "impl_item")
        || defines_an_extension(node, language)
}

/// The function a function is defined in, rather than in a type or at a
/// file's top: a closure's, a nested `def`'s — passed over an unnamed one,
/// a callback, for the one around it. It is part of the identity, so two
/// nested functions of one name, and parameters in different functions, are
/// told apart, with ids that do not move with their order.
fn enclosing_function_name(
    node: Node,
    content: &str,
    language: &str,
    config: &LanguageNodeConfig,
) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        if matches_node_type(candidate.kind(), &config.function_nodes)
            && let Some(name) = function_name(candidate, content, language)
        {
            return Some(name);
        }
        if is_type_node(candidate, language, config) {
            return None;
        }
        ancestor = candidate.parent();
    }
    None
}

/// The name a function node is indexed by: a JavaScript function
/// expression's or a Java lambda's binding, else its own.
fn function_name(node: Node, content: &str, language: &str) -> Option<String> {
    if matches!(language, "javascript" | "typescript" | "tsx")
        && is_js_function_expression(node.kind())
    {
        js_function_name(node, content)
    } else if matches!(
        (language, node.kind()),
        ("java", "lambda_expression") | ("go", "func_literal")
    ) {
        bound_lambda_name(node, content)
    } else {
        extract_node_name(node, content, language)
    }
    .filter(|name| !name.is_empty())
}

/// The interface or trait a method implements a member of, where its
/// definition says so (see [`Function::implements`](crate::entities::Function::implements)):
/// Rust's `impl Display for Order`, C#'s `void IDisposable.Dispose()`, F#'s
/// `interface IDisposable with`, VB.NET's `Implements IDisposable.Dispose` —
/// which the grammar does not parse, and reads as an error. By its last
/// segment, without type arguments: `fmt::Display` is `Display`, `From<T>`
/// `From`.
fn implemented_supertype(node: Node, content: &str, language: &str) -> Option<String> {
    let last_segment = |text: &str| -> String {
        let text = text.split('<').next().unwrap_or(text).trim();
        text.rsplit(['.', ':']).next().unwrap_or(text).to_string()
    };
    let name = match language {
        "rust" => {
            let block = node
                .parent()
                .filter(|block| block.kind() == "declaration_list")?;
            let implementation = block.parent().filter(|item| item.kind() == "impl_item")?;
            base_type_name(implementation.child_by_field_name("trait")?, content)
        }
        "c_sharp" => {
            let mut cursor = node.walk();
            let specifier = node
                .children(&mut cursor)
                .find(|child| child.kind() == "explicit_interface_specifier")?;
            base_type_name(specifier, content)
        }
        "fsharp" => {
            let implementation = std::iter::successors(node.parent(), Node::parent)
                .take(3)
                .find(|ancestor| ancestor.kind() == "interface_implementation")?;
            let mut cursor = implementation.walk();
            let ty = implementation
                .named_children(&mut cursor)
                .find(|child| child.kind().ends_with("type"))?;
            last_segment(node_text(ty, content))
        }
        "vb_net" => {
            let mut cursor = node.walk();
            let clause = node.children(&mut cursor).find_map(|child| {
                let text = node_text(child, content).trim();
                text.get(..10)
                    .filter(|keyword| keyword.eq_ignore_ascii_case("implements"))
                    .map(|_| text[10..].trim())
            })?;
            let member = clause.split(',').next().unwrap_or(clause).trim();
            last_segment(member.rsplit_once('.')?.0)
        }
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

/// The class a C++ method defined outside it names: `Cart` in
/// `int Cart::total() {…}`, `Order` in `shop::Order::count`. A namespace
/// qualifier reads the same, but a free function is defined inside its
/// `namespace { … }` far more often than with one.
fn qualified_definition_scope(node: Node, content: &str) -> Option<String> {
    let mut declarator = node.child_by_field_name("declarator")?;
    while declarator.kind() != "function_declarator" {
        declarator = declarator.child_by_field_name("declarator")?;
    }
    let mut name = declarator.child_by_field_name("declarator")?;
    let mut scope = None;
    while name.kind() == "qualified_identifier" {
        scope = name.child_by_field_name("scope");
        name = name.child_by_field_name("name")?;
    }
    let scope = scope?;
    let scope = scope.child_by_field_name("name").unwrap_or(scope);
    Some(node_text(scope, content).trim().to_string()).filter(|s| !s.is_empty())
}

/// The type a Dart member belongs to. A member is a `class_member` in the body
/// of its class, mixin, enum, extension or extension type, so the type is
/// three levels up.
fn dart_member_owner(node: Node) -> Option<Node> {
    let member = node.parent().filter(|p| p.kind() == "class_member")?;
    let body = member.parent().filter(|p| p.kind().ends_with("_body"))?;
    body.parent()
}

/// The first named child of `node` of kind `kind`.
fn named_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn extract_graphql_name(node: Node, content: &str) -> Option<String> {
    // The grammar has no fields: a definition's name is a `name` child, and a
    // fragment's sits one level down, in `fragment_name`. An anonymous
    // operation (`{ … }`) has neither, so it yields no entity.
    let name = named_child_of_kind(node, "name").or_else(|| {
        named_child_of_kind(node, "fragment_name")
            .and_then(|fragment| named_child_of_kind(fragment, "name"))
    })?;
    let name_text = node_text(name, content).trim();
    (!name_text.is_empty()).then(|| name_text.to_string())
}

/// Look up a language-specific name extractor for the given language.
///
/// Falls back to a no-op when the language is not registered.
fn extract_language_specific_name(node: Node, content: &str, language: &str) -> Option<String> {
    LANGUAGE_NAME_EXTRACTORS
        .get(language)
        .and_then(|extractor| extractor(node, content))
}

/// Get the text content of a node
///
/// Maximum body text to capture per entity (1 MB). Larger bodies are truncated
/// to avoid excessive memory allocation during extraction.
const MAX_ENTITY_BODY_BYTES: usize = 1_048_576;

/// Extract node text as a `String`, capping at [`MAX_ENTITY_BODY_BYTES`].
/// Prevents excessive cloning when a node spans a very large source region.
fn extract_body_text(node: Node, content: &str) -> String {
    let text = node_text(node, content);
    if text.len() > MAX_ENTITY_BODY_BYTES {
        // Split on a char boundary, not a byte index, so a cap inside a multi-byte
        // sequence makes no invalid UTF-8.
        let split = text
            .char_indices()
            .take_while(|(i, _)| *i <= MAX_ENTITY_BODY_BYTES)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let mut truncated = text[..split].to_string();
        truncated.push('\n');
        truncated.push_str(TRUNCATION_MARKER);
        truncated
    } else {
        text.to_string()
    }
}

/// Returns the text slice for the given node's byte range.
/// If the byte range is invalid (e.g., tree-sitter produced invalid indices),
/// logs a warning and returns an empty string.
fn node_text<'a>(node: Node, content: &'a str) -> &'a str {
    let range = node.start_byte()..node.end_byte();
    if range.start > content.len() || range.end > content.len() || range.start > range.end {
        tracing::warn!(
            "Invalid byte range from tree-sitter: {:?} for content length {}",
            range,
            content.len()
        );
        ""
    } else {
        &content[range]
    }
}

/// Get source location from a node
fn get_source_location(node: Node) -> SourceLocation {
    let start = node.start_position();
    let end = node.end_position();

    SourceLocation {
        start_line: u32::try_from(start.row).unwrap_or(0).saturating_add(1),
        start_column: u32::try_from(start.column).unwrap_or(0),
        end_line: u32::try_from(end.row).unwrap_or(0).saturating_add(1),
        end_column: u32::try_from(end.column).unwrap_or(0),
    }
}

/// Collect decorator / annotation names from the node's consecutive preceding siblings.
///
/// Decorators appear as preceding siblings of the decorated node in most
/// tree-sitter grammars (Python `decorator`, TypeScript `decorator`, Java
/// `annotation`, C# `attribute`, Kotlin `annotation`, Rust `attribute_item`).
/// We walk backwards past comments, and past the `export` and `default` a
/// TypeScript `@Component export class` puts between a class and its
/// decorators, stopping at the first other sibling. Names are returned
/// outermost first (the order they appear in source), each as
/// [`annotation_name`] reads it, without its sigils and arguments:
/// `#[allow(dead_code)]` is `allow`, `#[tokio::test]` `tokio::test`.
fn collect_preceding_decorators(node: Node, content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        let kind = sib.kind();
        if kind.contains("decorator")
            || kind.contains("annotation")
            || kind == "attribute"
            || kind == "attribute_item"
        {
            names.extend(annotation_name(sib, content));
        } else if !(kind.contains("comment") || matches!(kind, "export" | "default")) {
            break;
        }
        sibling = sib.prev_sibling();
    }
    names.reverse();
    names
}

/// A JavaScript or TypeScript function written as an expression, which has
/// no name of its own or not the one callers use.
fn is_js_function_expression(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

/// The name a JavaScript or TypeScript function expression is called by: the
/// name it is bound to (`const total = () => …`, `{ total: () => … }`,
/// `exports.total = function () …`, a class field `total = () => …`), else
/// its own (`function total() {}` as an argument). A callback bound to no name
/// has none, and is not indexed; it is never named after a parameter.
fn js_function_name(node: Node, content: &str) -> Option<String> {
    let parent = node.parent()?;
    let bound = match parent.kind() {
        "variable_declarator" => parent
            .child_by_field_name("name")
            .filter(|name| name.kind() == "identifier"),
        "pair" => parent.child_by_field_name("key"),
        "assignment_expression" => parent
            .child_by_field_name("left")
            .map(|left| left.child_by_field_name("property").unwrap_or(left)),
        "public_field_definition" | "field_definition" => parent
            .child_by_field_name("name")
            .or_else(|| parent.child_by_field_name("property")),
        _ => None,
    };
    bound
        .or_else(|| node.child_by_field_name("name"))
        .map(|name| {
            node_text(name, content)
                .trim()
                .trim_matches(|c| c == '"' || c == '\'' || c == '`')
                .to_string()
        })
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
}

/// The name a Java lambda or a Go function literal is bound to —
/// `Function<…> twice = x -> …`, `twice := func(x int) int {…}` — or none: a
/// lambda passed as an argument is a callback, not indexed. Named the way
/// other functions are, a lambda took its parameter's name (`r` of
/// `r -> r.isEmpty()`) and a Go literal its return type's (`int`).
fn bound_lambda_name(node: Node, content: &str) -> Option<String> {
    let parent = node.parent()?;
    let name = match parent.kind() {
        "variable_declarator" => parent.child_by_field_name("name")?,
        // `twice := func…`, `var twice = func…`, `a, b := func…, func…`: the
        // name in the literal's place among the names its value list binds.
        "expression_list" => {
            let place = {
                let mut values = parent.walk();
                parent
                    .named_children(&mut values)
                    .position(|value| value.id() == node.id())?
            };
            let declaration = parent.parent()?;
            let mut names = declaration.walk();
            match declaration.kind() {
                "short_var_declaration" | "assignment_statement" => {
                    let left = declaration.child_by_field_name("left")?;
                    left.named_child(u32::try_from(place).ok()?)?
                }
                "var_spec" => declaration
                    .children_by_field_name("name", &mut names)
                    .nth(place)?,
                _ => return None,
            }
        }
        _ => return None,
    };
    let text = node_text(name, content).trim();
    (!text.is_empty() && text.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| text.to_string())
}

/// Extract a function entity
fn extract_function(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let Some(name) = function_name(node, content, language) else {
        return Ok(None);
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    let parent_class = member_owner_name(node, content, language, config);
    let parent_function = enclosing_function_name(node, content, language, config);
    let implements = implemented_supertype(node, content, language);

    // Try to extract parameters
    let parameters = extract_parameters(node, content, language);

    // Try to extract return type
    let return_type = extract_return_type(node, content, language);

    // Extract modifiers and access modifiers
    let (modifiers_vec, access_mods_vec) = extract_modifiers(node, content, language);
    let modifiers = modifiers_vec.map(|v| v.join(", "));
    let access_modifiers = access_mods_vec.map(|v| v.join(", "));

    let mut decorators = collect_preceding_decorators(node, content);
    decorators.extend(collect_inner_annotations(node, content));

    let function = Function {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        parent_function,
        parent_class,
        parameters,
        return_type,
        modifiers,
        access_modifiers,
        docstring: None,
        decorators,
        implements,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Function(function)))
}

/// Extract parameters from a function node
fn extract_parameters(node: Node, content: &str, language: &str) -> Option<String> {
    // Try "parameters" field
    if let Some(params) = node.child_by_field_name("parameters") {
        return Some(node_text(params, content).to_string());
    }

    // Try "arguments" field
    if let Some(args) = node.child_by_field_name("arguments") {
        return Some(node_text(args, content).to_string());
    }

    // Try "formal_parameters" field
    if let Some(formal) = node.child_by_field_name("formal_parameters") {
        return Some(node_text(formal, content).to_string());
    }

    // Try "signature" field and extract from there
    if let Some(sig) = node.child_by_field_name("signature") {
        return extract_parameters(sig, content, language);
    }

    // Language-specific extraction
    match language {
        "python" => {
            // Python parameters are in a parameters node
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "parameters" {
                    return Some(node_text(child, content).to_string());
                }
            }
        }
        "c" | "cpp" | "cuda" => {
            // C-style: look for parameter_list
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "parameter_list" || child.kind() == "parameter_declaration" {
                    return Some(node_text(child, content).to_string());
                }
            }
        }
        _ => {}
    }

    None
}

/// Extract return type from a function node
fn extract_return_type(node: Node, content: &str, language: &str) -> Option<String> {
    // Try "return_type" field
    if let Some(ret_type) = node.child_by_field_name("return_type") {
        return Some(node_text(ret_type, content).to_string());
    }

    // Try "type" field
    if let Some(type_node) = node.child_by_field_name("type") {
        return Some(node_text(type_node, content).to_string());
    }

    // Try "result" field
    if let Some(result) = node.child_by_field_name("result") {
        return Some(node_text(result, content).to_string());
    }

    // Language-specific extraction
    match language {
        "rust" => {
            // Rust return type comes after ->
            let text = node_text(node, content);
            if let Some(arrow_pos) = text.find("->") {
                let after_arrow = &text[arrow_pos + 2..];
                // Find the opening brace or semicolon
                if let Some(end_pos) = after_arrow.find(['{', ';', '\n']) {
                    let ret_type = after_arrow[..end_pos].trim();
                    if !ret_type.is_empty() {
                        return Some(ret_type.to_string());
                    }
                }
            }
        }
        "go" => {
            // Go return types are in the signature
            if let Some(sig) = node.child_by_field_name("signature")
                && let Some(result) = sig.child_by_field_name("result")
            {
                return Some(node_text(result, content).to_string());
            }
        }
        _ => {}
    }

    None
}

/// Check whether `token` appears as a standalone word in `line`.
/// Multi-word tokens (e.g. "protected internal") fall back to substring
/// matching; single-word tokens use exact whitespace-delimited matching
/// to avoid false positives inside string literals or identifiers.
fn line_has_modifier(line: &str, token: &str) -> bool {
    if token.contains(' ') {
        line.contains(token)
    } else {
        // Tokenize on punctuation that separates keywords from identifiers
        // to avoid false positives like "publication" matching "public"
        line.split(|c: char| {
            c.is_whitespace()
                || c == '('
                || c == ')'
                || c == '{'
                || c == '<'
                || c == '>'
                || c == ';'
                || c == ','
        })
        .any(|word| word == token)
    }
}

/// The modifiers and the access of a declaration, read from its header (see
/// [`declaration_header`]) as words. Read from the first line alone, a method
/// under its annotation (`@GetMapping`, `[HttpGet]`) would have no access at
/// all, and read as substrings, Swift's `func openDoor()` would be `open`. A
/// JavaScript or TypeScript declaration under `export` has that access.
fn extract_modifiers(
    node: Node,
    content: &str,
    language: &str,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let header = declaration_header(node, content, language).to_lowercase();
    let (access, others): (&[&str], &[&str]) = match language {
        "java" => (
            &["public", "private", "protected"],
            &[
                "static",
                "final",
                "abstract",
                "synchronized",
                "native",
                "default",
            ],
        ),
        "kotlin" => (
            &["public", "private", "protected", "internal"],
            &[
                "override", "open", "abstract", "suspend", "inline", "operator", "external",
            ],
        ),
        "c_sharp" => (
            &[
                "public",
                "private",
                "protected",
                "internal",
                "protected internal",
                "private protected",
            ],
            &[
                "static", "readonly", "const", "abstract", "virtual", "override", "sealed",
                "unsafe", "async", "partial", "extern",
            ],
        ),
        "rust" => (&["pub"], &["async", "const", "unsafe", "extern"]),
        "cpp" | "cuda" => (
            &[],
            &[
                "static",
                "const",
                "virtual",
                "inline",
                "explicit",
                "constexpr",
                "noexcept",
                "override",
                "final",
            ],
        ),
        "c" | "objc" | "objective_c" => (&[], &["static", "inline", "extern"]),
        "swift" => (
            &["public", "private", "internal", "open", "fileprivate"],
            &[
                "static", "class", "final", "override", "mutating", "throws", "async",
            ],
        ),
        "vb_net" => (
            &[
                "public",
                "private",
                "protected",
                "friend",
                "protected friend",
            ],
            &[
                "shared",
                "overridable",
                "notoverridable",
                "mustoverride",
                "overrides",
                "overloads",
                "shadows",
            ],
        ),
        "php" => (
            &["public", "private", "protected"],
            &["static", "abstract", "final"],
        ),
        "typescript" | "tsx" => (
            &["public", "private", "protected"],
            &[
                "static", "async", "readonly", "abstract", "override", "get", "set",
            ],
        ),
        "javascript" => (&[], &["static", "async", "get", "set"]),
        "scala" => (
            &["private", "protected"],
            &["override", "final", "implicit", "lazy", "abstract"],
        ),
        "solidity" => (
            &["public", "private", "internal", "external"],
            &["view", "pure", "payable", "virtual", "override"],
        ),
        "dart" => (
            &[],
            &[
                "static", "abstract", "external", "factory", "async", "get", "set",
            ],
        ),
        "fsharp" => (
            &["public", "private", "internal"],
            &["static", "override", "abstract", "default", "inline"],
        ),
        "python" => (&[], &["async"]),
        _ => (&[], &[]),
    };
    let present = |tokens: &[&str]| -> Vec<String> {
        tokens
            .iter()
            .filter(|token| line_has_modifier(&header, token))
            .map(|token| (*token).to_string())
            .collect()
    };
    let mut access_mods = present(access);
    let mut modifiers = present(others);

    if language == "python" && node_text(node, content).contains("@property") {
        modifiers.push("property".to_string());
    }
    if matches!(language, "javascript" | "typescript" | "tsx")
        && node.parent().is_some_and(|parent| {
            parent.kind() == "export_statement"
                || parent
                    .parent()
                    .is_some_and(|grandparent| grandparent.kind() == "export_statement")
        })
    {
        access_mods.push("export".to_string());
    }

    (
        (!modifiers.is_empty()).then_some(modifiers),
        (!access_mods.is_empty()).then_some(access_mods),
    )
}

/// A declaration's header: its text from its start to its name — its
/// annotations, keywords and return type — or, where the keywords follow the
/// name (Solidity's `function pay() external`), to its body; its first line
/// when it has neither.
fn declaration_header<'a>(node: Node, content: &'a str, language: &str) -> &'a str {
    let until = if language == "solidity" {
        node.child_by_field_name("body")
    } else {
        node.child_by_field_name("name")
            .or_else(|| node.child_by_field_name("declarator"))
    };
    match until {
        Some(part) if part.start_byte() > node.start_byte() => content
            .get(node.start_byte()..part.start_byte())
            .unwrap_or(""),
        _ => node_text(node, content).lines().next().unwrap_or(""),
    }
}

/// The annotations a declaration holds among its own parts rather than before
/// it: Java's and Kotlin's in its `modifiers` (`@GetMapping`, `@Test`), C#'s
/// attribute lists (`[HttpGet]`), PHP's attribute groups (`#[Route]`),
/// Swift's attributes, Dart's (`@override`). Without them, no Java, Kotlin,
/// C#, PHP or Dart method would have a decorator.
fn collect_inner_annotations(node: Node, content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "modifiers" | "attribute_list" | "attributes" => {
                let mut parts = child.walk();
                for part in child.children(&mut parts) {
                    match part.kind() {
                        "marker_annotation" | "annotation" | "attribute" => {
                            names.extend(annotation_name(part, content));
                        }
                        // PHP's `#[Route('/x'), Cache]`.
                        "attribute_group" => {
                            let mut attributes = part.walk();
                            names.extend(
                                part.children(&mut attributes)
                                    .filter(|attribute| attribute.kind() == "attribute")
                                    .filter_map(|attribute| annotation_name(attribute, content)),
                            );
                        }
                        _ => {}
                    }
                }
            }
            // Dart's annotations are the declaration's own children.
            "attribute" | "decorator" | "annotation" | "marker_annotation" => {
                names.extend(annotation_name(child, content));
            }
            _ => {}
        }
    }
    names
}

/// The name of an annotation, decorator or attribute — `app.route`,
/// `tokio::test`, `Override` — its `name`, else its text, without `@`, `#[`,
/// `[`, PHP's leading `\` and its arguments.
fn annotation_name(node: Node, content: &str) -> Option<String> {
    let text = node
        .child_by_field_name("name")
        .map(|name| node_text(name, content))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| node_text(node, content));
    let text = text
        .trim()
        .trim_start_matches(['@', '#', '[', '\\'])
        .trim_end_matches(']');
    let name = text.split('(').next().unwrap_or(text).trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether a Kotlin `class_declaration` node is actually an interface.
///
/// Kotlin reuses `class_declaration` for both, distinguished only by an
/// `interface` keyword child in place of `class`.
fn kotlin_node_is_interface(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == "interface")
}

/// A Kotlin `enum class` is a `class_declaration` whose modifiers hold `enum`;
/// read as a class, it would be one, and each of its entries an enum of its own.
fn kotlin_node_is_enum(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind() == "modifiers")
        .any(|modifiers| {
            let mut inner = modifiers.walk();
            modifiers.children(&mut inner).any(|modifier| {
                let mut words = modifier.walk();
                modifier.kind() == "class_modifier"
                    && modifier
                        .children(&mut words)
                        .any(|word| word.kind() == "enum")
            })
        })
}

/// The node a type declares its members in: its `body`, a Go struct's
/// `field_declaration_list`, or the body-like child a grammar gives no field
/// (Kotlin's `class_body`, F#'s `record_fields`); the type node itself where
/// its members are its own children (VB.NET, Objective-C).
fn type_body(node: Node) -> Node {
    if let Some(body) = node.child_by_field_name("body") {
        return body;
    }
    if let Some(ty) = node.child_by_field_name("type") {
        let mut cursor = ty.walk();
        if let Some(list) = ty
            .named_children(&mut cursor)
            .find(|child| child.kind() == "field_declaration_list")
        {
            return list;
        }
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| {
            child.kind().ends_with("body")
                || matches!(
                    child.kind(),
                    "declaration_list" | "field_declaration_list" | "record_fields"
                )
        })
        .unwrap_or(node)
}

/// The kinds of node a type's body declares a field with, in `language`.
fn field_kinds(language: &str) -> &'static [&'static str] {
    match language {
        // `count: int = 0` in a class body: an assignment in a statement.
        "python" => &["expression_statement"],
        "javascript" => &["field_definition"],
        "typescript" | "tsx" => &["public_field_definition", "property_signature"],
        "java" | "c" | "cpp" | "cuda" | "rust" | "go" => &["field_declaration"],
        "c_sharp" | "vb_net" => &["field_declaration", "property_declaration"],
        "php" | "swift" | "kotlin" | "objc" => &["property_declaration"],
        "dart" => &["declaration"],
        "scala" => &["val_definition", "var_definition"],
        "fsharp" => &["record_field"],
        "solidity" => &["state_variable_declaration", "struct_member"],
        _ => &[],
    }
}

/// The fields a type declares — `[{"name": "balance", "type": "Money"}]`,
/// the type where one is written — in its body, and in Kotlin's and Scala's
/// primary constructors (`class Point(val x: Int)`, a case class's every
/// parameter). Read from the type node's own children, where most grammars
/// put none, no type would have any, and no field a type.
fn declared_fields(node: Node, content: &str, language: &str) -> Option<serde_json::Value> {
    let kinds = field_kinds(language);
    let body = type_body(node);
    let mut cursor = body.walk();
    // Dart wraps each member in a `class_member`.
    let members: Vec<Node> = body
        .named_children(&mut cursor)
        .flat_map(|member| {
            let mut inner = member.walk();
            let parts: Vec<Node> = if member.kind() == "class_member" {
                member.named_children(&mut inner).collect()
            } else {
                vec![member]
            };
            parts
        })
        .collect();
    let mut fields = Vec::new();
    let push = |member: Node, fields: &mut Vec<serde_json::Value>| {
        let ty = declared_type(member, content);
        for name in declared_names(member, content) {
            fields.push(match &ty {
                Some(ty) => serde_json::json!({ "name": name, "type": ty }),
                None => serde_json::json!({ "name": name }),
            });
        }
    };
    let mut after_keyword = false;
    for (at, &member) in members.iter().enumerate() {
        // A VB.NET `Inherits` or `Implements` statement, which the grammar
        // reads as fields — the keyword and its types, or the keyword alone
        // and its types the next — names the class's supertypes.
        if std::mem::take(&mut after_keyword) {
            continue;
        }
        if language == "vb_net"
            && crate::parser::relationship_extraction::vb_heritage_statement(
                member,
                members.get(at + 1).copied(),
                content,
            )
            .is_some()
        {
            after_keyword = !node_text(member, content)
                .trim()
                .contains(char::is_whitespace);
            continue;
        }
        // A member the grammar could not read, or one beside such, is none:
        // VB.NET's operators leave errors and fields made of their parts.
        let unreadable = member.has_error()
            || [member.prev_named_sibling(), member.next_named_sibling()]
                .into_iter()
                .flatten()
                .any(|sibling| sibling.is_error());
        if kinds.contains(&member.kind()) && !declares_a_function(member) && !unreadable {
            push(member, &mut fields);
        }
        // Ruby's `attr_accessor :name, :email` declares its attributes.
        if language == "ruby" && member.kind() == "call" {
            let accessor = member
                .child_by_field_name("method")
                .map(|method| node_text(method, content));
            if matches!(
                accessor,
                Some("attr_accessor" | "attr_reader" | "attr_writer")
            ) && let Some(arguments) = member.child_by_field_name("arguments")
            {
                let mut cursor = arguments.walk();
                for symbol in arguments.named_children(&mut cursor) {
                    let name = node_text(symbol, content).trim().trim_start_matches(':');
                    if symbol.kind() == "simple_symbol" && !name.is_empty() {
                        fields.push(serde_json::json!({ "name": name }));
                    }
                }
            }
        }
    }
    if matches!(language, "kotlin" | "scala") {
        let case_class = node_text(node, content).trim_start().starts_with("case ");
        let mut parameters = Vec::new();
        collect_kinds(node, &["class_parameter"], 2, &mut parameters);
        for parameter in parameters {
            let text = node_text(parameter, content).trim_start();
            let words: Vec<&str> = text.split_whitespace().collect();
            if case_class
                || words
                    .iter()
                    .take(3)
                    .any(|word| matches!(*word, "val" | "var"))
            {
                push(parameter, &mut fields);
            }
        }
    }
    (!fields.is_empty()).then_some(serde_json::Value::Array(fields))
}

/// The nodes of `kinds` under `node`, at most `depth` levels down.
fn collect_kinds<'a>(node: Node<'a>, kinds: &[&str], depth: usize, out: &mut Vec<Node<'a>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(child);
        } else if depth > 0 {
            collect_kinds(child, kinds, depth - 1, out);
        }
    }
}

/// Whether a member declaration declares a function: C++'s `int total();`
/// in its class is a `field_declaration` too.
fn declares_a_function(member: Node) -> bool {
    let mut cursor = member.walk();
    let declarators: Vec<Node> = member
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    declarators.into_iter().any(|declarator| {
        std::iter::successors(Some(declarator), |part| {
            part.child_by_field_name("declarator")
        })
        .any(|part| part.kind() == "function_declarator")
    })
}

/// The names a declaration declares: by the field each grammar names them in
/// (`name`, `declarator`, `property`, `pattern`, `left`) — every one, as in
/// `double x, y;` — else in the declaration-like parts a grammar gives no
/// field (C#'s `variable_declaration`, PHP's `property_element`, Kotlin's
/// `variable_declaration`, Dart's `initialized_identifier`), else its first
/// identifier (F#'s `record_field`).
fn declared_names(node: Node, content: &str) -> Vec<String> {
    let mut names = Vec::new();
    for field in ["name", "declarator", "property", "pattern", "left"] {
        let mut cursor = node.walk();
        let parts: Vec<Node> = node.children_by_field_name(field, &mut cursor).collect();
        for part in parts {
            names.extend(leaf_name(part, content));
        }
    }
    if !names.is_empty() {
        return names;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    for child in &children {
        if matches!(
            child.kind(),
            "variable_declaration"
                | "variable_declarator"
                | "property_element"
                | "initialized_identifier_list"
                | "initialized_identifier"
                | "struct_declaration"
                | "struct_declarator"
                | "assignment"
        ) {
            names.extend(declared_names(*child, content));
        }
    }
    if names.is_empty()
        && let Some(identifier) = children
            .iter()
            .find(|child| matches!(child.kind(), "identifier" | "simple_identifier"))
    {
        names.extend(leaf_name(*identifier, content));
    }
    names
}

/// The name a declarator or pattern comes down to: its `name`,
/// `declarator` or `bound_identifier` part's, or, a name itself, its text —
/// without PHP's `$`.
fn leaf_name(node: Node, content: &str) -> Option<String> {
    for field in ["name", "declarator", "bound_identifier"] {
        if let Some(part) = node.child_by_field_name(field) {
            return leaf_name(part, content);
        }
    }
    let kind = node.kind();
    let named = kind.ends_with("identifier") || matches!(kind, "name" | "variable_name");
    if named && node.named_child_count() == 0 {
        let text = node_text(node, content).trim().trim_start_matches('$');
        return (!text.is_empty()).then(|| text.to_string());
    }
    let mut cursor = node.walk();
    let first = node
        .named_children(&mut cursor)
        .find(|child| child.kind().ends_with("identifier") || child.kind() == "name")?;
    leaf_name(first, content)
}

/// The type a declaration writes for what it declares: its `type`, or that of
/// the declaration-like part holding it (C#'s `variable_declaration`), or a
/// type-like child a grammar gives no field (Swift's `type_annotation`,
/// Dart's `type`, F#'s `simple_type`) — without the `:` before it.
fn declared_type(node: Node, content: &str) -> Option<String> {
    fn typed(node: Node<'_>) -> Option<Node<'_>> {
        node.child_by_field_name("type").or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find(|child| {
                matches!(
                    child.kind(),
                    "type_annotation" | "type" | "user_type" | "simple_type" | "type_identifier"
                )
            })
        })
    }
    let ty = typed(node).or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter(|child| {
                matches!(
                    child.kind(),
                    "variable_declaration" | "struct_declaration" | "assignment"
                )
            })
            .find_map(typed)
    })?;
    let text = node_text(ty, content).trim().trim_start_matches(':').trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The names of the supertypes a type declares: those it extends, and those
/// an `implements` list names — read as the references to them are.
fn declared_supertypes(node: Node, content: &str) -> (Vec<String>, Vec<String>) {
    let (implemented, extended): (Vec<_>, Vec<_>) =
        crate::parser::relationship_extraction::collect_supertypes(node, content)
            .into_iter()
            .partition(|supertype| supertype.implemented);
    let names = |supertypes: Vec<crate::parser::relationship_extraction::Supertype>| {
        let mut names: Vec<String> = Vec::new();
        for supertype in supertypes {
            if !names.contains(&supertype.name) {
                names.push(supertype.name);
            }
        }
        names
    };
    (names(extended), names(implemented))
}

/// Extract a class entity
fn extract_class(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // The types it extends and implements, as its references name them, not the
    // clause's text, keyword and all (`implements Runnable`).
    let (extended, implemented) = declared_supertypes(node, content);
    let listed = |names: Vec<String>| (!names.is_empty()).then(|| names.join(", "));

    // Extract access modifiers
    let (_, access_mods_vec) = extract_modifiers(node, content, language);
    let access_modifiers = access_mods_vec.map(|v| v.join(", "));

    let mut decorators = collect_preceding_decorators(node, content);
    decorators.extend(collect_inner_annotations(node, content));

    // Its methods are attached once the file's functions are extracted
    // (`attach_methods`).
    let class = Class {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        fields: declared_fields(node, content, language),
        methods: None,
        extends: listed(extended),
        implements: listed(implemented),
        constants: None,
        access_modifiers,
        docstring: None,
        decorators,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Class(class)))
}

/// Extract a struct entity
/// A Swift `class_declaration` declares what its `declaration_kind` says: a
/// class or an actor, a struct, an enum, or an extension of a type declared
/// elsewhere.
fn extract_swift_type(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    match node
        .child_by_field_name("declaration_kind")
        .map(|kind| kind.kind())
    {
        Some("struct") => extract_struct(node, content, language, file_path, now),
        Some("enum") => extract_enum(node, content, language, file_path, now),
        Some("extension") => extract_extension(node, content, language, file_path, now),
        _ => extract_class(node, content, language, file_path, now),
    }
}

/// The arguments of an `NS_ENUM`-family macro — `(NSInteger, Status)` of
/// `NS_ENUM(NSInteger, Status)` — when `node` is a `macro_type_specifier` of
/// one.
fn objc_enum_macro_arguments<'a>(node: Node, content: &'a str) -> Option<(&'a str, &'a str)> {
    if node.kind() != "macro_type_specifier" {
        return None;
    }
    let text = node_text(node, content);
    let (name, arguments) = text.split_once('(')?;
    if !matches!(
        name.trim(),
        "NS_ENUM"
            | "NS_OPTIONS"
            | "NS_CLOSED_ENUM"
            | "NS_ERROR_ENUM"
            | "CF_ENUM"
            | "CF_OPTIONS"
            | "CF_CLOSED_ENUM"
    ) {
        return None;
    }
    let (backing, name) = arguments.trim_end_matches(')').split_once(',')?;
    Some((backing.trim(), name.trim()))
}

/// An Objective-C enum declared the way Apple's frameworks declare theirs,
/// `typedef NS_ENUM(NSInteger, Status) { StatusOpen, StatusClosed };`. The
/// grammar parses the macro and leaves the rest as errors — the `typedef` it
/// makes is named after the first enumerator — so the enum is read from the
/// macro's arguments, its name and backing type, and its enumerators from the
/// braces that follow it.
fn objc_macro_enum(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Option<Entity> {
    let (backing, name) = objc_enum_macro_arguments(node, content)?;
    let after = &content[node.end_byte()..];
    let open = after.find(|c: char| !c.is_whitespace())?;
    let (start, end, variants) = if after[open..].starts_with('{') {
        let close = open + after[open..].find('}')?;
        let variants: Vec<String> = after[open + 1..close]
            .split(',')
            .filter_map(|entry| {
                let entry = entry.split('=').next().unwrap_or(entry).trim();
                let named =
                    !entry.is_empty() && entry.chars().all(|c| c.is_alphanumeric() || c == '_');
                named.then(|| entry.to_string())
            })
            .collect();
        (node.start_byte(), node.end_byte() + close + 1, variants)
    } else {
        (node.start_byte(), node.end_byte(), Vec::new())
    };
    let position = |byte: usize| -> (u32, u32) {
        let before = &content[..byte];
        let line = before.matches('\n').count();
        let column = byte - before.rfind('\n').map_or(0, |newline| newline + 1);
        (
            u32::try_from(line).unwrap_or(0).saturating_add(1),
            u32::try_from(column).unwrap_or(0),
        )
    };
    let ((start_line, start_column), (end_line, end_column)) = (position(start), position(end));
    Some(Entity::Enum(Enum {
        id: Uuid::new_v4(),
        name: name.to_string(),
        file_path: file_path.to_string(),
        body: content[start..end].to_string(),
        location: SourceLocation {
            start_line,
            start_column,
            end_line,
            end_column,
        },
        language: language.to_string(),
        variants: Some(serde_json::json!(variants)),
        parent_class: None,
        backed_type: Some(backing.to_string()),
        implements: None,
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    }))
}

/// The labels of a SQL `CREATE TYPE status AS ENUM ('open', 'closed')`, when
/// the type is an enum; any other `CREATE TYPE` is a type alias.
fn sql_enum_labels(node: Node, content: &str) -> Option<Vec<String>> {
    let mut cursor = node.walk();
    let elements = node
        .children(&mut cursor)
        .find(|child| child.kind() == "enum_elements")?;
    let mut cursor = elements.walk();
    Some(
        elements
            .named_children(&mut cursor)
            .filter(|label| label.kind() == "literal")
            .map(|label| {
                node_text(label, content)
                    .trim()
                    .trim_matches('\'')
                    .to_string()
            })
            .collect(),
    )
}

/// An Objective-C category, `@interface Cart (Pricing)`, or class extension,
/// `@interface Cart ()`, and their `@implementation`s: an extension of the
/// class, named `Cart(Pricing)` (or `Cart`), not one more class `Cart`. Its
/// methods are the class's.
fn objc_category(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Option<Entity> {
    let mut cursor = node.walk();
    let is_category = node.children(&mut cursor).any(|child| child.kind() == "(");
    if !is_category {
        return None;
    }
    let class = extract_node_name(node, content, language)?;
    let name = match node.child_by_field_name("category") {
        Some(category) => format!("{class}({})", node_text(category, content).trim()),
        None => class.clone(),
    };
    Some(Entity::Extension(Extension {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body: extract_body_text(node, content),
        location: get_source_location(node),
        language: language.to_string(),
        extended_type: Some(class),
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    }))
}

/// A Go `type_spec` declares what its `type` is: `type Point struct {…}` a
/// struct, `type Reader interface {…}` an interface, and `type ID string` or
/// `type Handler func(int)` a type of its own over another, indexed as a type
/// alias.
fn extract_go_type_spec(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    match node.child_by_field_name("type").map(|ty| ty.kind()) {
        Some("struct_type") => extract_struct(node, content, language, file_path, now),
        Some("interface_type") => {
            extract_interface(node, content, language, file_path, config, now)
        }
        _ => extract_type_alias(node, content, language, file_path, now),
    }
}

fn extract_struct(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Its methods are attached once the file's functions are extracted.
    let struct_entity = Struct {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        fields: declared_fields(node, content, language),
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Struct(struct_entity)))
}

/// Extract an enum entity
fn extract_enum(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Extract variants: children of the enum, or of the body that most
    // grammars put them in (Dart and Java `enum_body`, Rust
    // `enum_variant_list`, C `enumerator_list`).
    let variants: Vec<String> = {
        let mut variants = Vec::new();
        for holder in std::iter::once(node).chain(node.child_by_field_name("body")) {
            let mut cursor = holder.walk();
            for child in holder.children(&mut cursor) {
                let kind = child.kind();
                if (kind.contains("variant")
                    || kind.contains("constant")
                    || kind.contains("enumerator")
                    || kind == "enum_variant")
                    && let Some(variant_name) = extract_node_name(child, content, language)
                {
                    variants.push(variant_name);
                }
            }
        }
        variants
    };

    let enum_entity = Enum {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        variants: Some(serde_json::json!(variants)),
        parent_class: None,
        backed_type: None,
        implements: {
            let (_, implemented) = declared_supertypes(node, content);
            (!implemented.is_empty()).then(|| implemented.join(", "))
        },
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Enum(enum_entity)))
}

/// Extract an interface entity
fn extract_interface(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    _config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // The interfaces it extends, as its references name them.
    let (mut extends, implemented) = declared_supertypes(node, content);
    extends.extend(implemented);

    // Its methods are attached once the file's functions are extracted; a Go
    // interface's `method_elem`s are none, so it names them itself.
    let mut elements = Vec::new();
    collect_kinds(node, &["method_elem"], 2, &mut elements);
    let declared: Vec<String> = elements
        .into_iter()
        .filter_map(|element| element.child_by_field_name("name"))
        .map(|name| node_text(name, content).to_string())
        .collect();
    let interface = Interface {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        methods: (!declared.is_empty()).then_some(declared),
        properties: declared_fields(node, content, language),
        extends: (!extends.is_empty()).then_some(extends),
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Interface(interface)))
}

/// Extract a trait entity: a PHP `trait` or a Dart `mixin`. A Rust trait,
/// a Scala trait and an Objective-C protocol are interfaces.
fn extract_trait(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Its methods are attached once the file's functions are extracted.
    let trait_entity = Trait {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Trait(trait_entity)))
}

/// Extract a module entity
fn extract_module(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => Some(name),
        _ => {
            // For module nodes without name, use file name
            std::path::Path::new(file_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        }
    };

    let name = match name {
        Some(name) => name,
        None => return Ok(None),
    };
    Ok(Some(module_entity(
        node, name, content, language, file_path, now,
    )))
}

/// The module a file's code outside every entity is: named after the file
/// (its stem) and spanning `root`, as the module a Python file's root is.
/// `None` for a path without a stem.
pub(crate) fn file_module(
    root: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Option<Entity> {
    let name = std::path::Path::new(file_path)
        .file_stem()?
        .to_string_lossy()
        .to_string();
    Some(module_entity(root, name, content, language, file_path, now))
}

/// The module entity `node` is, named `name`, with the imports among its
/// children.
fn module_entity(
    node: Node,
    name: String,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Entity {
    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Extract imports/exports
    let config = get_language_config(language);
    let imports: Vec<String> = {
        let mut cursor = node.walk();
        let mut imports = Vec::new();
        for child in node.children(&mut cursor) {
            if matches_node_type(child.kind(), &config.import_nodes) {
                imports.push(node_text(child, content).to_string());
            }
        }
        imports
    };

    Entity::Module(Module {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        exports: None,
        imports: if imports.is_empty() {
            None
        } else {
            Some(imports)
        },
        created_at: now,
        updated_at: now,
    })
}

/// Extract a namespace entity
fn extract_namespace(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // For C# and other languages that use namespaces
    let namespace = Namespace {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Namespace(namespace)))
}

/// Extract a type alias entity
fn extract_type_alias(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Try to extract the aliased type
    let aliased_type = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("value"))
        .map(|t| node_text(t, content).to_string())
        .unwrap_or_else(|| body.clone());

    let type_alias = TypeAlias {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body: body.clone(),
        location,
        language: language.to_string(),
        aliased_type,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::TypeAlias(type_alias)))
}

/// Returns `true` when the identifier `name` looks like a module-level constant
/// for languages that use generic `assignment`/`declaration` nodes for constants.
///
/// Accepted: ALL_CAPS, ALL_CAPS_WITH_NUMBERS, SCREAMING_SNAKE_CASE.
/// This is the community convention in Python, Ruby, C defines, and C++ consts.
/// One capital letter is not: `X` and `Y` name coordinates, `T` a type and
/// `N` a bound far more often, and a one-letter constant is declared with
/// `const`, `final`, `val` or `let`, which says so.
fn is_constant_name(name: &str) -> bool {
    if name.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return false;
    }
    // All non-digit, non-underscore chars must be uppercase.
    name.chars()
        .filter(|c| !c.is_ascii_digit() && *c != '_')
        .all(|c| c.is_uppercase())
}

/// Returns `true` when `node` lives inside a function/method/lambda/closure body
/// (i.e. it is a function-local declaration). Walks up the ancestor chain and
/// stops at the nearest scope boundary: a function-like body yields `true`, while
/// a type body (class/struct/interface/enum/trait/object/namespace) or a module
/// root yields `false`. It keeps ordinary locals from being labeled
/// `Constant` entities while admitting genuine module- and type-level
/// constants (e.g. a class-level `MAX_RETRIES`).
fn is_function_local(node: Node) -> bool {
    fn is_function_scope(kind: &str) -> bool {
        kind.contains("function")
            || kind.contains("method")
            || kind.contains("lambda")
            || kind.contains("closure")
            || kind.contains("arrow")
            || matches!(
                kind,
                "block" | "statement_block" | "compound_statement" | "do_block"
            )
    }
    fn is_type_or_module_boundary(kind: &str) -> bool {
        kind.contains("class")
            || kind.contains("struct")
            || kind.contains("interface")
            || kind.contains("enum")
            || kind.contains("trait")
            || kind.contains("object")
            || kind.contains("namespace")
            || matches!(
                kind,
                "module"
                    | "source_file"
                    | "program"
                    | "translation_unit"
                    | "compilation_unit"
                    | "chunk"    // Lua
                    | "document" // Generic
            )
    }
    let mut current = node.parent();
    while let Some(p) = current {
        let kind = p.kind();
        if is_function_scope(kind) {
            return true;
        }
        if is_type_or_module_boundary(kind) {
            return false;
        }
        current = p.parent();
    }
    false // reached AST root without entering a function body
}

/// Returns `true` when the declaration carries a leading immutability modifier
/// (`const`, `final`, `readonly`, `val`, `constexpr`, `constant`, `lazy`).
///
/// Only the tokens *before* the first `=` are inspected so an initializer value
/// (e.g. `= some_const_fn()`) never produces a false positive. This lets genuine
/// constants with lower/mixed-case names (C# `const int MaxSize`, Kotlin
/// `const val timeout`, Scala `val pi`) be admitted without an UPPER_CASE name.
/// Whether this declaration binds its name for good. Swift says so with `let`,
/// where a `var` — a `lazy var` too — binds a variable; elsewhere it is a
/// keyword such as `const` or `val` ([`has_immutability_keyword`]).
fn is_immutable_binding(node: Node, content: &str, language: &str) -> bool {
    if language == "swift" {
        let mut cursor = node.walk();
        return node.children(&mut cursor).any(|child| {
            child.kind() == "value_binding_pattern"
                && child
                    .child_by_field_name("mutability")
                    .is_some_and(|mutability| mutability.kind() == "let")
        });
    }
    has_immutability_keyword(node, content)
}

fn has_immutability_keyword(node: Node, content: &str) -> bool {
    let text = node_text(node, content);
    let prefix = text.split('=').next().unwrap_or(text);
    prefix
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|tok| {
            matches!(
                tok.to_ascii_lowercase().as_str(),
                "const"
                    | "final"
                    | "readonly"
                    | "val"
                    | "constexpr"
                    | "constant"
                    | "immutable"
                    | "lazy"
            )
        })
}

/// Whether this node's constant is already emitted by its declaration sibling.
///
/// C, C++ and CUDA list both `declaration` and the `init_declarator` nested
/// inside it as constant node kinds, so a single `constexpr int X = 1;` matched
/// at both levels and produced two `Constant` entities for one constant — with
/// different column spans, so nothing downstream collapsed them.
///
/// The choice is made per declaration rather than by always preferring one
/// kind, because neither is correct alone. The `declaration` carries the type
/// and the full source text, but takes its name from only the *first*
/// declarator — so `const int A = 1, B = 2;` would silently lose `B`. The
/// `init_declarator` names every constant correctly but knows nothing of the
/// declared type.
///
/// One declarator: keep the declaration, which is the richer entity. Several:
/// keep the declarators, which are the complete set.
fn is_redundant_c_declarator(node: Node) -> bool {
    match node.kind() {
        "init_declarator" => node
            .parent()
            .filter(|parent| parent.kind() == "declaration")
            .is_some_and(|parent| count_init_declarators(parent) == 1),
        "declaration" => count_init_declarators(node) > 1,
        // Other grammars have no `init_declarator`, so a `declaration` there
        // counts zero and is never skipped.
        _ => false,
    }
}

/// How many declarators one C-family declaration introduces.
fn count_init_declarators(declaration: Node) -> usize {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .filter(|child| child.kind() == "init_declarator")
        .count()
}

/// Extract a constant entity.
///
/// For languages that use generic node types (`assignment`, `declaration`,
/// `init_declarator`) we apply two guards to avoid emitting every local
/// variable as a Constant entity:
///
/// 1. **Scope guard** — the node must be a direct child of the file root
///    (module/program/source_file/translation_unit).
/// 2. **Name guard** — for Python and Ruby (where even module-level assignments
///    are generic), the identifier must be UPPER_CASE / SCREAMING_SNAKE_CASE.
///
/// Languages that use dedicated constant node kinds (`const_item`,
/// `const_declaration`, `lexical_declaration const`, …) bypass both guards
/// since the grammar already disambiguates them.
fn extract_constant(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    // Several languages list *generic* node kinds in `constant_nodes` — kinds
    // that also denote ordinary locals, fields, or mutable properties (e.g. JS
    // `lexical_declaration` for `const`/`let`, Java/C#/VB `field_declaration`,
    // Scala `val_definition`, Bash `variable_assignment`, C/C++ `declaration`).
    // Emitting a Constant for each would bloat the entity table, add FTS noise,
    // and mislabel locals. For these kinds we apply two guards; languages with
    // dedicated const kinds (`const_item`, `const_declaration`, …) bypass them
    // since the grammar already disambiguates a constant from a variable.
    // C-family grammars nest a declarator inside its declaration and list both
    // kinds as constant nodes, so one `constexpr int X = 1;` matched twice and
    // produced two entities for one constant.
    if is_redundant_c_declarator(node) {
        return Ok(None);
    }
    if node.kind() == "variable_declarator" {
        return Ok(module_constant(node, content, language, file_path, now));
    }
    if language == "ruby" && node.kind() == "assignment" {
        let owner = member_owner_name(node, content, language, config);
        return Ok(ruby_constant(node, content, file_path, owner, now));
    }

    let generic_kind = matches!(
        node.kind(),
        "assignment"
            | "lexical_declaration"
            | "declaration"
            | "init_declarator"
            | "field_declaration"
            | "property_declaration"
            | "val_definition"
            | "val_declaration"
            | "lazy_val"
            | "variable_assignment"
            | "declaration_command"
            | "state_variable_declaration"
            | "field_definition"
            | "public_field_definition"
    );

    // A class field whose value is a function or a class is that, indexed as
    // what it is (`static HANDLER = () => …`).
    if matches!(node.kind(), "field_definition" | "public_field_definition")
        && node
            .child_by_field_name("value")
            .is_some_and(|value| is_js_function_expression(value.kind()) || value.kind() == "class")
    {
        return Ok(None);
    }

    // Guard 1 (scope): a function-local declaration is never a top-level constant.
    if generic_kind && is_function_local(node) {
        return Ok(None);
    }

    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    // Guard 2 (constness): for generic kinds require an immutability signal —
    // either an UPPER_CASE / SCREAMING_SNAKE name (Python `MAX`, Java `LIMIT`)
    // or a leading immutability keyword (`const`/`final`/`readonly`/`val`). This
    // drops mutable locals/fields (`let x`, `var y`, plain fields) while keeping
    // genuine constants regardless of language-specific naming conventions.
    if generic_kind && !is_constant_name(&name) && !is_immutable_binding(node, content, language) {
        return Ok(None);
    }

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // Try to extract value
    let value = node
        .child_by_field_name("value")
        .map(|v| node_text(v, content).to_string());

    // Try to extract type
    let constant_type = node
        .child_by_field_name("type")
        .map(|t| node_text(t, content).to_string());

    let constant = Constant {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        value,
        constant_type,
        parent_class: member_owner_name(node, content, language, config),
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Constant(constant)))
}

/// A Ruby constant: an assignment to a `constant` — a capitalized name, so
/// `Default = 5` as much as `MAX = 10` — or to one in a scope
/// (`Shop::RATE = 3`). An assignment to any other name is a variable. The
/// name is in the assignment's `left`.
fn ruby_constant(
    assignment: Node,
    content: &str,
    file_path: &str,
    owner: Option<String>,
    now: OffsetDateTime,
) -> Option<Entity> {
    let left = assignment.child_by_field_name("left")?;
    let name = match left.kind() {
        "constant" => left,
        "scope_resolution" => left
            .child_by_field_name("name")
            .filter(|name| name.kind() == "constant")?,
        _ => return None,
    };
    Some(Entity::Constant(Constant {
        id: Uuid::new_v4(),
        name: node_text(name, content).trim().to_string(),
        file_path: file_path.to_string(),
        body: extract_body_text(assignment, content),
        location: get_source_location(assignment),
        language: "ruby".to_string(),
        value: assignment
            .child_by_field_name("right")
            .map(|value| node_text(value, content).to_string()),
        constant_type: None,
        parent_class: owner,
        docstring: None,
        created_at: now,
        updated_at: now,
    }))
}

/// A JavaScript or TypeScript module constant: one declarator of a `const`
/// declaration at the top of a module, exported or not, whose value is not a
/// function or class (those are extracted as what they are). `let`, `var`,
/// locals and destructuring patterns make none.
fn module_constant(
    declarator: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Option<Entity> {
    let declaration = declarator.parent()?;
    if declaration.kind() != "lexical_declaration"
        || declaration
            .child(0)
            .map(|keyword| node_text(keyword, content))
            != Some("const")
    {
        return None;
    }
    let statement = match declaration.parent()? {
        export if export.kind() == "export_statement" => export,
        _ => declaration,
    };
    if statement.parent()?.kind() != "program" {
        return None;
    }
    let name = declarator.child_by_field_name("name")?;
    if name.kind() != "identifier" {
        return None;
    }
    let value = declarator.child_by_field_name("value");
    if value.is_some_and(|value| {
        matches!(
            value.kind(),
            "arrow_function"
                | "function_expression"
                | "function"
                | "generator_function"
                | "class"
                | "class_expression"
        )
    }) {
        return None;
    }
    let constant_type = declarator.child_by_field_name("type").map(|annotation| {
        node_text(annotation, content)
            .trim_start_matches(':')
            .trim()
            .to_string()
    });
    Some(Entity::Constant(Constant {
        id: Uuid::new_v4(),
        name: node_text(name, content).to_string(),
        file_path: file_path.to_string(),
        body: extract_body_text(statement, content),
        location: get_source_location(statement),
        language: language.to_string(),
        value: value.map(|value| node_text(value, content).to_string()),
        constant_type,
        parent_class: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    }))
}

/// Extract a macro entity
fn extract_macro(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    // Try to get name from extract_node_name
    let name_result = extract_node_name(node, content, language);

    let name_opt: Option<String> = if let Some(name) = name_result {
        if !name.is_empty() {
            Some(name)
        } else {
            // For macros without name field, try to extract from first identifier
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|c| c.kind() == "identifier")
                .map(|c| node_text(c, content).to_string())
        }
    } else {
        // For macros without name field, try to extract from first identifier
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|c| c.kind() == "identifier")
            .map(|c| node_text(c, content).to_string())
    };

    let name = match name_opt {
        Some(name) => name,
        None => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    let macro_entity = Macro {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Macro(macro_entity)))
}

/// Extract a component entity (for Vue, Svelte, etc.)
fn extract_component(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    config: &LanguageNodeConfig,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    // A self-closing element is parsed as an `element` wrapping a
    // `self_closing_tag` of the *same* extent, and both kinds are component
    // nodes — so `<meta charset="utf-8" />` would be emitted twice at
    // identical spans, which nothing downstream could tell apart.
    if covers_the_same_source_as_parent(node, config) {
        return Ok(None);
    }

    // A Vue or Svelte file is one component, named after its file as both
    // frameworks name it; an HTML element is named for itself
    // (`html_element_name`).
    let name = match extract_node_name(node, content, language).or_else(|| {
        std::path::Path::new(file_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
    }) {
        Some(n) => n,
        None => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    // The attributes written on the node itself are its props.
    let properties: Vec<String> = {
        let mut cursor = node.walk();
        let mut props = Vec::new();
        for child in node.children(&mut cursor) {
            if matches_node_type(child.kind(), &config.property_nodes)
                && let Some(prop_name) = extract_node_name(child, content, language)
            {
                props.push(prop_name);
            }
        }
        props
    };

    // Convert properties to serde_json::Value
    let props_value = if properties.is_empty() {
        None
    } else {
        Some(serde_json::json!(properties))
    };

    let component = Component {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        props: props_value,
        methods: None,
        template: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Component(component)))
}

/// Extract a union/discriminated union entity
fn extract_union(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    let union_entity = Union {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        members: Vec::new(),
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Union(union_entity)))
}

/// Extract a contract entity (Solidity)
fn extract_contract(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    let contract = Contract {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        functions: None,
        events: None,
        modifiers: None,
        state_variables: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Contract(contract)))
}

/// Extract an extension entity (Swift, Kotlin)
fn extract_extension(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);
    // What it extends: a Dart extension's `on` type; the others are named
    // after it. A SQL extension extends no type.
    let extended_type = match language {
        "dart" => node
            .child_by_field_name("class")
            .map(|ty| node_text(ty, content).trim().to_string()),
        "sql" => None,
        _ => Some(name.clone()),
    };

    let extension = Extension {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        extended_type,
        methods: None,
        docstring: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Extension(extension)))
}

/// Extract a decorator/annotation entity (Python, Java, TS)
fn extract_decorator(
    node: Node,
    content: &str,
    language: &str,
    file_path: &str,
    now: OffsetDateTime,
) -> Result<Option<Entity>> {
    let name = match extract_node_name(node, content, language) {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let body = extract_body_text(node, content);
    let location = get_source_location(node);

    let decorator = Decorator {
        id: Uuid::new_v4(),
        name,
        file_path: file_path.to_string(),
        body,
        location,
        language: language.to_string(),
        decorator_type: None,
        target: None,
        parameters: None,
        created_at: now,
        updated_at: now,
    };

    Ok(Some(Entity::Decorator(decorator)))
}

/// The name of an HTML element: its tag, qualified by `id` when it has one.
///
/// `main id="app"` becomes `main#app`, which is what a person would use to refer
/// to it and is distinct from the other elements on the page. Returns `None` for
/// non-element nodes so the ordinary name resolution still applies.
fn html_element_name(node: Node, content: &str) -> Option<String> {
    if !matches!(
        node.kind(),
        "element" | "void_element" | "self_closing_tag" | "start_tag"
    ) {
        return None;
    }
    // `element` wraps a `start_tag` / `self_closing_tag`; the others carry the
    // tag name directly.
    let tag_holder = if node.kind() == "element" {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|c| matches!(c.kind(), "start_tag" | "self_closing_tag"))?
    } else {
        node
    };

    let mut cursor = tag_holder.walk();
    let children: Vec<Node> = tag_holder.children(&mut cursor).collect();

    let tag = children
        .iter()
        .find(|c| c.kind() == "tag_name")
        .map(|c| node_text(*c, content).trim().to_string())
        .filter(|t| !t.is_empty())?;

    let id = children
        .iter()
        .filter(|c| c.kind() == "attribute")
        .find_map(|attr| html_attribute(*attr, content, "id"));

    Some(match id {
        Some(id) => format!("{tag}#{id}"),
        None => tag,
    })
}

/// The value of one named attribute on an HTML `attribute` node.
fn html_attribute(attr: Node, content: &str, wanted: &str) -> Option<String> {
    let mut cursor = attr.walk();
    let children: Vec<Node> = attr.children(&mut cursor).collect();
    let name = children.iter().find(|c| c.kind() == "attribute_name")?;
    if !node_text(*name, content)
        .trim()
        .eq_ignore_ascii_case(wanted)
    {
        return None;
    }
    let value = children.iter().find(|c| {
        matches!(
            c.kind(),
            "attribute_value" | "quoted_attribute_value" | "unquoted_attribute_value"
        )
    })?;
    let text = node_text(*value, content).trim().trim_matches(['"', '\'']);
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether this node re-wraps exactly the source its parent already covers.
///
/// Some grammars nest a node inside another of the same extent — HTML parses
/// `<meta />` as an `element` containing a `self_closing_tag` with identical
/// start and end bytes. When both kinds are extracted, one piece of source
/// yields two entities that differ in nothing an index can use to separate
/// them. The outer node wins: it is the one that carries the element's children.
fn covers_the_same_source_as_parent(node: Node, config: &LanguageNodeConfig) -> bool {
    node.parent().is_some_and(|parent| {
        matches_node_type(parent.kind(), &config.component_nodes)
            && parent.start_byte() == node.start_byte()
            && parent.end_byte() == node.end_byte()
    })
}

#[cfg(test)]
mod tests {
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
        let entities =
            extract_entities(&tree, content, "typescript", "test.ts").unwrap_or_default();
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
}
