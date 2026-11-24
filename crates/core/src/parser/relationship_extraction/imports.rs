//! Imports: what a file imports, recorded as a relationship of the entity
//! that imports it.

use super::*;

/// Record each name an import statement brings into its containing entity as
/// a `uses` reference, and link it to the definitions of it in this file, its
/// type definitions when it has any (see [`LocalNames::type_candidates`]).
pub(super) fn extract_import_relationship(
    node: Node,
    content: &str,
    language: &str,
    names: &LocalNames,
    line_index: &Extents,
    out: &mut Extraction,
) {
    let Some(source_id) = find_containing_entity(node, line_index) else {
        return;
    };

    let node_type = node.kind();
    let now = OffsetDateTime::now_utc();
    let line = start_line(node);
    // The importing entity uses `name`: a reference, linked here to each
    // definition of `name` in this file.
    let mut link = |name: &str, metadata: serde_json::Value| {
        out.refs.push(SymbolRef {
            source_id,
            kind: RelationshipType::Uses,
            target_name: name.to_string(),
            qualifier: None,
            line,
        });
        let (targets, candidates) = names.type_candidates(name);
        for target in targets {
            let mut relationship = Relationship {
                id: Uuid::new_v4(),
                source_id,
                target_id: target.id(),
                relationship_type: RelationshipType::Uses,
                metadata: Some(metadata.clone()),
                created_at: now,
            };
            relationship.mark_ambiguous(candidates);
            out.relationships.push(relationship);
        }
    };

    // --- Python: `import a.b, c as d` uses `a` and `c`; `from m import x, y
    // as z` uses `x` and `y`, and the module `m` they come from. Each name of
    // an import is a `name` of its own, an aliased one under the alias
    // (`import numpy as np` uses `numpy`), and a relative module
    // (`from .. import x`) is not a name of dots. ---
    if language == "python" && matches!(node_type, "import_statement" | "import_from_statement") {
        let module = node
            .child_by_field_name("module_name")
            .map(|module| node_text(module, content).trim_start_matches('.'))
            .filter(|module| !module.is_empty());
        if let Some(module) = module {
            link(
                module.rsplit('.').next().unwrap_or(module),
                serde_json::json!({
                    "import_type": "from_import",
                    "module": module,
                }),
            );
        }
        let mut cursor = node.walk();
        for name in node.children_by_field_name("name", &mut cursor) {
            let Some(name) = (if name.kind() == "aliased_import" {
                name.child_by_field_name("name")
            } else {
                Some(name)
            }) else {
                continue;
            };
            let imported = node_text(name, content);
            if node_type == "import_statement" {
                link(
                    imported.split('.').next().unwrap_or(imported),
                    serde_json::json!({
                        "import_type": "import",
                        "module": imported,
                    }),
                );
            } else {
                link(
                    imported,
                    serde_json::json!({
                        "import_type": "from_import",
                        "module": module,
                        "imported_name": imported,
                    }),
                );
            }
        }
        return;
    }

    // --- JS/TS import_statement ---
    if node_type == "import_statement" && matches!(language, "javascript" | "typescript" | "tsx") {
        let mut imported_names = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let kind = child.kind();
            if kind == "import_clause" || kind == "named_imports" || kind == "namespace_import" {
                let mut inner = child.walk();
                for grandchild in child.children(&mut inner) {
                    if grandchild.kind() == "identifier" {
                        imported_names.push(node_text(grandchild, content));
                    }
                }
            }
            if kind == "string" {
                let module_path = node_text(child, content);
                let clean_path = module_path.trim_matches(|c| c == '\'' || c == '"');
                let base = clean_path.split('/').next_back().unwrap_or(clean_path);
                link(
                    base,
                    serde_json::json!({
                        "import_type": "es_module",
                        "module": clean_path,
                    }),
                );
            }
        }
        for name in imported_names {
            link(
                name,
                serde_json::json!({
                    "import_type": "es_module",
                    "imported_name": name,
                }),
            );
        }
        return;
    }

    // --- Java import_declaration ---
    if node_type == "import_declaration" {
        if let Some(dotted) = node.child_by_field_name("name") {
            let full = node_text(dotted, content);
            let base = full.split('.').next_back().unwrap_or(full);
            link(
                base,
                serde_json::json!({
                    "import_type": "java_import",
                    "class": full,
                }),
            );
        }
        return;
    }

    // --- Rust use_declaration ---
    if node_type == "use_declaration" || node_type == "use" || node_type == "extern_crate" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let name = extract_identifier_name(child, content);
            if !name.is_empty() && name != "use" && name != "extern" && name != "crate" {
                link(
                    &name,
                    serde_json::json!({
                        "import_type": "rust_use",
                        "name": name,
                    }),
                );
            }
        }
        return;
    }

    // --- Generic fallback: the names the import's named parts give; its
    // keyword and punctuation (`using`, `open`, `;`) are no names ---
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let name = extract_identifier_name(child, content);
        if !name.is_empty() {
            link(
                &name,
                serde_json::json!({
                    "import_type": "generic",
                    "name": name,
                    "node_type": node_type,
                }),
            );
        }
    }
}

// =============================================================================
// HIERARCHICAL RELATIONSHIPS
// =============================================================================
