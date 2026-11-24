//! Classes, structs, enums, interfaces and traits, with the fields, methods
//! and supertypes they declare.

use super::*;

/// Whether a Kotlin `class_declaration` node is actually an interface.
///
/// Kotlin reuses `class_declaration` for both, distinguished only by an
/// `interface` keyword child in place of `class`.
pub(super) fn kotlin_node_is_interface(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == "interface")
}

/// A Kotlin `enum class` is a `class_declaration` whose modifiers hold `enum`;
/// read as a class, it would be one, and each of its entries an enum of its own.
pub(super) fn kotlin_node_is_enum(node: Node) -> bool {
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
pub(super) fn type_body(node: Node) -> Node {
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
pub(super) fn field_kinds(language: &str) -> &'static [&'static str] {
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
pub(super) fn declared_fields(
    node: Node,
    content: &str,
    language: &str,
) -> Option<serde_json::Value> {
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
pub(super) fn collect_kinds<'a>(
    node: Node<'a>,
    kinds: &[&str],
    depth: usize,
    out: &mut Vec<Node<'a>>,
) {
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
pub(super) fn declares_a_function(member: Node) -> bool {
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
pub(super) fn declared_names(node: Node, content: &str) -> Vec<String> {
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
pub(super) fn leaf_name(node: Node, content: &str) -> Option<String> {
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
pub(super) fn declared_type(node: Node, content: &str) -> Option<String> {
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
pub(super) fn declared_supertypes(node: Node, content: &str) -> (Vec<String>, Vec<String>) {
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
pub(super) fn extract_class(
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
pub(super) fn extract_swift_type(
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
pub(super) fn objc_enum_macro_arguments<'a>(
    node: Node,
    content: &'a str,
) -> Option<(&'a str, &'a str)> {
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
pub(super) fn objc_macro_enum(
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
pub(super) fn sql_enum_labels(node: Node, content: &str) -> Option<Vec<String>> {
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
pub(super) fn objc_category(
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
pub(super) fn extract_go_type_spec(
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

pub(super) fn extract_struct(
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
pub(super) fn extract_enum(
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
pub(super) fn extract_interface(
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
pub(super) fn extract_trait(
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
