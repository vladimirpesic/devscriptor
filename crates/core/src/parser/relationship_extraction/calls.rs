//! Calls: what a call expression calls, from which entity, and with what
//! receiver, and the names a function binds itself.

use super::*;

/// Record the call a call expression makes from its containing entity, and
/// link it to each definition of the called name in this file (see
/// [`LocalNames::candidates`]).
pub(super) fn extract_call_relationships(
    node: Node,
    content: &str,
    language: &str,
    names: &LocalNames,
    line_index: &Extents,
    out: &mut Extraction,
) {
    // Find the containing entity (source of the call)
    let Some(source_id) = find_containing_entity(node, line_index) else {
        return; // No containing entity found
    };

    // Try to find the function name being called
    let called_name = if language == "graphql" {
        graphql_called_name(node, content)
    } else {
        extract_called_function_name(node, content)
    };
    if called_name.is_empty() {
        return;
    }
    let qualifier = call_receiver(node, content);
    record_call(node, source_id, called_name, qualifier, names, out);
}

/// The fields of a declaration that hold what it is given rather than what it
/// binds: a value, a default, a type, a body.
pub(super) const VALUE_FIELDS: &[&str] = &[
    "value",
    "right",
    "default_value",
    "initializer",
    "type",
    "body",
    "condition",
    "arguments",
    "function",
];

/// The names a function binds itself: its parameters and those of the
/// closures in it, and what it declares, assigns, loops over, catches or
/// matches in its body. Used as a value, one is a variable of its own, not a
/// constant or function anywhere.
pub(super) fn locally_bound_names(
    function: Node,
    content: &str,
) -> std::collections::HashSet<String> {
    fn identifiers(node: Node, content: &str, out: &mut std::collections::HashSet<String>) {
        // A member assigned (`self.total = 0`) is written, not bound.
        if member_name(node).is_some() {
            return;
        }
        let kind = node.kind();
        if kind.ends_with("identifier")
            || kind.ends_with("identifier_pattern")
            || kind == "variable_name"
        {
            let name = node_text(node, content).trim().trim_start_matches('$');
            if !name.is_empty() {
                out.insert(name.to_string());
            }
            return;
        }
        // What a declaration is given — its value, type or body — binds
        // nothing: `int x = Limits.MAX + MAX` binds `x` alone.
        for (child, field) in children_and_fields(node) {
            if child.is_named() && !field.is_some_and(|field| VALUE_FIELDS.contains(&field)) {
                identifiers(child, content, out);
            }
        }
    }
    /// Kotlin's `variable_declaration` names its variable in no field: its
    /// first name that is no type.
    fn declared_name(node: Node) -> Option<Node> {
        children_and_fields(node)
            .into_iter()
            .find(|(child, field)| {
                matches!(child.kind(), "identifier" | "simple_identifier") && field.is_none()
            })
            .map(|(child, _)| child)
    }
    /// Kotlin's declarations name their variable in a child, not a field.
    fn declared_variable(node: Node) -> Option<Node> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "variable_declaration" | "multi_variable_declaration"
            )
        })
    }
    fn declarations(node: Node, content: &str, out: &mut std::collections::HashSet<String>) {
        let kind = node.kind();
        let bound = match kind {
            "variable_declarator"
            | "init_declarator"
            | "let_declaration"
            | "const_declaration"
            | "var_declaration"
            | "property_declaration"
            | "local_variable_declaration"
            | "variable_assignment"
            | "variable_declaration"
            | "initialized_variable_definition"
            | "dim_statement"
            | "catch_declaration" => node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("pattern"))
                .or_else(|| node.child_by_field_name("declarator"))
                .or_else(|| declared_variable(node))
                .or_else(|| declared_name(node)),
            "short_var_declaration" | "assignment" | "assignment_expression" | "var_spec" => node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("name")),
            "as_pattern" => node.child_by_field_name("alias"),
            // F#'s `let x = …` and `let f o = …` inside a function.
            "value_declaration_left" | "function_declaration_left" => Some(node),
            _ if is_loop(kind) => node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("item"))
                .or_else(|| node.child_by_field_name("variable"))
                .or_else(|| node.child_by_field_name("name"))
                .or_else(|| node.child_by_field_name("declarator"))
                .or_else(|| declared_variable(node)),
            _ => None,
        };
        if let Some(bound) = bound {
            identifiers(bound, content, out);
        }
        // A pattern binds what it names (`if let Some(x)`, a match arm, a
        // destructuring); a closure's, a catch's or a lambda's parameters
        // are its own.
        if let Some(pattern) = node.child_by_field_name("pattern") {
            identifiers(pattern, content, out);
        }
        if kind.contains("parameter") || kind == "argument_patterns" {
            identifiers(node, content, out);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            declarations(child, content, out);
        }
    }
    let mut names = std::collections::HashSet::new();
    let mut cursor = function.walk();
    for part in function.named_children(&mut cursor) {
        if part.kind().contains("parameter") {
            identifiers(part, content, &mut names);
        }
    }
    for field in ["parameters", "parameter"] {
        if let Some(parameters) = function.child_by_field_name(field) {
            identifiers(parameters, content, &mut names);
        }
    }
    declarations(function, content, &mut names);
    names
}

/// The calls written in a Rust token tree (a macro's arguments): a name
/// followed by a parenthesized group, its qualifier the path segment or
/// receiver before it (`Money` in `Money::zero()`, `v` in `v.len()`). A macro
/// invoked inside (`vec![…]`) is no call here: its own token tree is read in
/// turn. A function defined inside (`fn helper()`) is no call either.
pub(super) fn extract_token_tree_calls(
    node: Node,
    content: &str,
    names: &LocalNames,
    line_index: &Extents,
    out: &mut Extraction,
) {
    let mut cursor = node.walk();
    let tokens: Vec<Node> = node.children(&mut cursor).collect();
    for (i, token) in tokens.iter().enumerate() {
        let is_call = token.kind() == "identifier"
            && tokens.get(i + 1).is_some_and(|next| {
                next.kind() == "token_tree" && node_text(*next, content).starts_with('(')
            })
            && !(i > 0 && tokens[i - 1].kind() == "fn");
        if !is_call {
            continue;
        }
        let Some(source_id) = find_containing_entity(*token, line_index) else {
            continue;
        };
        // `Money::new(…)`'s path keeps its `::`, as [`call_receiver`]'s does.
        let qualifier = (i >= 2 && matches!(tokens[i - 1].kind(), "." | "::"))
            .then(|| tokens[i - 2])
            .filter(|before| matches!(before.kind(), "identifier" | "self"))
            .map(|before| {
                let separator = if tokens[i - 1].kind() == "::" {
                    "::"
                } else {
                    ""
                };
                format!("{}{separator}", node_text(before, content))
            });
        let name = node_text(*token, content).to_string();
        record_call(*token, source_id, name, qualifier, names, out);
    }
}

/// Record the call `node` makes from `source_id` to `called_name`, and link
/// it to each definition of the name in this file.
pub(super) fn record_call(
    node: Node,
    source_id: Uuid,
    called_name: String,
    qualifier: Option<String>,
    names: &LocalNames,
    out: &mut Extraction,
) {
    // A qualified call is of a member of the type its qualifier names, and
    // one through a path of nothing else.
    let scope = CallScope::of(
        qualifier.as_deref(),
        names.owner_of(source_id),
        &called_name,
        names.language,
    );
    let (targets, candidates) = names.member_candidates(&called_name, scope);
    out.refs.push(SymbolRef {
        source_id,
        kind: RelationshipType::Calls,
        target_name: called_name.clone(),
        qualifier,
        line: start_line(node),
    });

    let now = OffsetDateTime::now_utc();
    let source_location = format!(
        "{}:{}-{}:{}",
        node.start_position().row + 1,
        node.start_position().column,
        node.end_position().row + 1,
        node.end_position().column
    );
    out.relationships.extend(targets.iter().map(|target| {
        let mut relationship = Relationship {
            id: Uuid::new_v4(),
            source_id,
            target_id: target.id(),
            relationship_type: RelationshipType::Calls,
            metadata: Some(serde_json::json!({
                "called_name": called_name,
                "source_location": source_location,
            })),
            created_at: now,
        };
        relationship.mark_ambiguous(candidates);
        relationship
    }));
}

/// What a method call is made on, when it is a short expression on one line:
/// `orders` in `orders.total()`; a path ends with its `::` — `Money::` in
/// `Money::new()`, `std::` in C++'s `std::abs(x)`, `Tax::` in PHP's
/// `Tax::rate()` — as it names a type or a module, never a value (see
/// [`CallScope`]).
pub(super) fn call_receiver(node: Node, content: &str) -> Option<String> {
    /// Longer receivers are chained calls or literals, not a name.
    const MAX_LEN: usize = 64;
    let function = node.child_by_field_name("function");
    [function, Some(node)]
        .into_iter()
        .flatten()
        .find_map(|callee| {
            ["object", "value", "operand", "path", "scope"]
                .iter()
                .find_map(|&field| Some((field, callee.child_by_field_name(field)?)))
        })
        .map(|(field, receiver)| {
            let text = node_text(receiver, content).trim();
            if matches!(field, "path" | "scope") {
                format!("{text}::")
            } else {
                text.to_string()
            }
        })
        .filter(|text| !text.is_empty() && text.len() <= MAX_LEN && !text.contains('\n'))
}

/// The name a GraphQL selection (`order(id: 1)`, `mine: order`) or directive
/// use (`@include(if: true)`) refers to: its `name` child. The first child is
/// an alias or the `@` token, which the generic rule would take.
pub(super) fn graphql_called_name(node: Node, content: &str) -> String {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "name")
        .map(|name| node_text(name, content).trim().to_string())
        .unwrap_or_default()
}

/// The name a call expression calls: the last name of its callee, so that
/// `calc.add(1)`, `pkg.Func()`, `Money::new()` and `self.repo.save()` call
/// `add`, `Func`, `new` and `save`. A call that names its method itself
/// (Java `method_invocation`, PHP member calls: `name`; Ruby `call`:
/// `method`) is read there; otherwise the callee expression is.
pub(super) fn extract_called_function_name(node: Node, content: &str) -> String {
    ["name", "method", "function", "constructor", "macro"]
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| node.named_child(0))
        .map(|callee| callee_name(callee, content))
        .unwrap_or_default()
}

/// The last name in a callee expression: down its name-like fields (a
/// member's `property`, `attribute`, `field` or `name`), else its last named
/// child (Kotlin and Swift navigation suffixes), to an identifier; failing
/// that, the last identifier in its text before any type arguments.
pub(super) fn callee_name(node: Node, content: &str) -> String {
    /// Deep enough for any callee; bounds a pathological tree.
    const MAX_DESCENT: usize = 16;
    let mut node = node;
    for _ in 0..MAX_DESCENT {
        if node.named_child_count() == 0 {
            break;
        }
        let next = [
            "property",
            "attribute",
            "field",
            "name",
            "method",
            "function",
        ]
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| {
            u32::try_from(node.named_child_count() - 1)
                .ok()
                .and_then(|last| node.named_child(last))
        });
        match next {
            Some(next) => node = next,
            None => break,
        }
    }
    // A name is the whole of its leaf: Ruby's `same_day?` and `save!` lost
    // their `?` and `!` to the split below, JavaScript's `#total` its `#` and
    // F#'s `f'` its prime, and called nothing named so.
    if node.named_child_count() == 0 && node.kind().ends_with("identifier") {
        let name = node_text(node, content).trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let text = node_text(node, content);
    let text = text.split(['<', '(']).next().unwrap_or(text);
    text.rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        })
        .unwrap_or_default()
        .to_string()
}

/// Extract identifier name from a node
pub(super) fn extract_identifier_name(node: Node, content: &str) -> String {
    let node_type = node.kind();

    // Direct identifier
    if node_type == "identifier" || node_type == "IDENTIFIER" {
        return node_text(node, content).to_string();
    }

    // Try child with name field
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, content).to_string();
    }

    // For member expressions (e.g., obj.method), get the method name
    if node_type == "member_expression" || node_type == "attribute" {
        if let Some(prop_node) = node.child_by_field_name("property") {
            return extract_identifier_name(prop_node, content);
        }
        if let Some(attr_node) = node.child_by_field_name("attribute") {
            return node_text(attr_node, content).to_string();
        }
    }

    // Default: return the node text
    node_text(node, content).to_string()
}

/// Extract node text with bounds checking
pub(super) fn node_text<'a>(node: Node, content: &'a str) -> &'a str {
    let start = node.start_byte();
    let end = node.end_byte();
    if start <= end && end <= content.len() {
        &content[start..end]
    } else {
        ""
    }
}
