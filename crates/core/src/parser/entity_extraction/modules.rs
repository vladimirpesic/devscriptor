//! Modules, namespaces and type aliases.

use super::*;

/// Extract a module entity
pub(super) fn extract_module(
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
pub(super) fn module_entity(
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
pub(super) fn extract_namespace(
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
pub(super) fn extract_type_alias(
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
