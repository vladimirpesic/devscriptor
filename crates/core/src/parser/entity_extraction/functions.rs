//! Functions and methods: their parameters, return type, modifiers and
//! annotations.

use super::*;

/// Extract a function entity
pub(super) fn extract_function(
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
pub(super) fn extract_parameters(node: Node, content: &str, language: &str) -> Option<String> {
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
pub(super) fn extract_return_type(node: Node, content: &str, language: &str) -> Option<String> {
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
pub(super) fn line_has_modifier(line: &str, token: &str) -> bool {
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
pub(super) fn extract_modifiers(
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
pub(super) fn declaration_header<'a>(node: Node, content: &'a str, language: &str) -> &'a str {
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
pub(super) fn collect_inner_annotations(node: Node, content: &str) -> Vec<String> {
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
pub(super) fn annotation_name(node: Node, content: &str) -> Option<String> {
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
