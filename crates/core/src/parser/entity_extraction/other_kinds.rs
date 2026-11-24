//! Macros, components, unions, contracts, extensions, decorators and HTML
//! elements.

use super::*;

/// Extract a macro entity
pub(super) fn extract_macro(
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
pub(super) fn extract_component(
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
pub(super) fn extract_union(
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
pub(super) fn extract_contract(
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
pub(super) fn extract_extension(
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
pub(super) fn extract_decorator(
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
pub(super) fn html_element_name(node: Node, content: &str) -> Option<String> {
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
pub(super) fn html_attribute(attr: Node, content: &str, wanted: &str) -> Option<String> {
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
pub(super) fn covers_the_same_source_as_parent(node: Node, config: &LanguageNodeConfig) -> bool {
    node.parent().is_some_and(|parent| {
        matches_node_type(parent.kind(), &config.component_nodes)
            && parent.start_byte() == node.start_byte()
            && parent.end_byte() == node.end_byte()
    })
}
