//! What every extractor reads from a node: its text, body and location, the
//! decorators before it, and the name a function expression is bound to.

use super::*;

/// Maximum body text to capture per entity (1 MB). Larger bodies are truncated
/// to avoid excessive memory allocation during extraction.
pub(super) const MAX_ENTITY_BODY_BYTES: usize = 1_048_576;

/// Extract node text as a `String`, capping at [`MAX_ENTITY_BODY_BYTES`].
/// Prevents excessive cloning when a node spans a very large source region.
pub(super) fn extract_body_text(node: Node, content: &str) -> String {
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
pub(super) fn node_text<'a>(node: Node, content: &'a str) -> &'a str {
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
pub(super) fn get_source_location(node: Node) -> SourceLocation {
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
pub(super) fn collect_preceding_decorators(node: Node, content: &str) -> Vec<String> {
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
pub(super) fn is_js_function_expression(kind: &str) -> bool {
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
pub(super) fn js_function_name(node: Node, content: &str) -> Option<String> {
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
pub(super) fn bound_lambda_name(node: Node, content: &str) -> Option<String> {
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
