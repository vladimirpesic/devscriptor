//! The type hierarchy: what a type extends and implements, in each language's
//! syntax, and SQL's foreign keys.

use super::*;

/// Extract inheritance (`inherits`) and interface-conformance (`implements`)
/// relationships from a class/struct/interface node, across all supported
/// grammars. Whether an edge is `Inherits` or `Implements` is decided by
/// resolving the supertype entity: an interface/trait/protocol parent yields
/// `Implements`, anything else yields `Inherits`. This keeps Kotlin/Swift `:`
/// (which serve both roles) correct without grammar-specific guessing. A type
/// in an `implements` list is implemented whatever its kind: Dart and
/// TypeScript let a class implement a class.
pub(super) fn extract_class_hierarchy_relationships(
    node: Node,
    content: &str,
    names: &LocalNames,
    out: &mut Extraction,
) {
    // An F# type keeps its name in its `type_name`; an Objective-C class and
    // a GraphQL type in their first identifier or `name`, with no field.
    let class_name = node
        .child_by_field_name("name")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|child| child.kind() == "type_name")?
                .child_by_field_name("type_name")
        })
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|child| matches!(child.kind(), "identifier" | "name"))
        })
        .map(|n| node_text(n, content).to_string())
        .unwrap_or_default();
    if class_name.is_empty() {
        return;
    }
    // The class this node declares, not another one of the same name.
    let Some(class) = names.defined_at(&class_name, start_line(node)) else {
        return;
    };
    for supertype in collect_supertypes(node, content) {
        if is_linked_supertype(&class_name, &supertype.name) {
            out.refs.push(SymbolRef {
                source_id: class.id(),
                kind: supertype_kind(supertype.implemented),
                target_name: supertype.name.clone(),
                qualifier: None,
                line: start_line(node),
            });
        }
        out.relationships.extend(build_hierarchy_rels(
            (std::slice::from_ref(&class), 1),
            &class_name,
            &supertype.name,
            supertype.implemented,
            names,
        ));
    }
}

/// Rust `impl Trait for Type` → `Type implements Trait`. `impl_item` is not a
/// class node, so it is dispatched separately. Inherent impls (`impl Type {}`,
/// no `trait` field) yield no relationship.
pub(super) fn extract_rust_impl_relationship(
    node: Node,
    content: &str,
    names: &LocalNames,
    out: &mut Extraction,
) {
    let (Some(trait_node), Some(type_node)) = (
        node.child_by_field_name("trait"),
        node.child_by_field_name("type"),
    ) else {
        return;
    };
    let trait_name = base_type_name(trait_node, content);
    let type_name = base_type_name(type_node, content);
    // The implementing type is defined in this file, or the impl names no
    // entity of it.
    let implementors = names.type_candidates(&type_name);
    if is_linked_supertype(&type_name, &trait_name) {
        out.refs
            .extend(implementors.0.iter().map(|implementor| SymbolRef {
                source_id: implementor.id(),
                kind: RelationshipType::Implements,
                target_name: trait_name.clone(),
                qualifier: None,
                line: start_line(node),
            }));
    }
    // Each implementor is a reference of its own, resolved on its own.
    for implementor in implementors.0 {
        out.relationships.extend(build_hierarchy_rels(
            (std::slice::from_ref(implementor), 1),
            &type_name,
            &trait_name,
            true,
            names,
        ));
    }
}

/// SQL foreign keys. A `create_table` whose `column_definition`s carry
/// `REFERENCES other_table(...)` yields a `uses` relationship from this table to
/// each referenced table. Both the owning table name and each referenced name
/// are wrapped as `object_reference → name: identifier`.
pub(super) fn extract_sql_fk_relationships(
    node: Node,
    content: &str,
    names: &LocalNames,
    out: &mut Extraction,
) {
    let Some(table) = sql_object_ref_name(node, content) else {
        return;
    };
    // The table this statement creates, not another one of the same name.
    let Some(source) = names.defined_at(&table, start_line(node)) else {
        return;
    };
    let mut cursor = node.walk();
    for defs in node.children(&mut cursor) {
        if defs.kind() != "column_definitions" {
            continue;
        }
        let mut dc = defs.walk();
        for col in defs.children(&mut dc) {
            if col.kind() != "column_definition" {
                continue;
            }
            // A referenced table is the `object_reference` following `REFERENCES`.
            let mut seen_ref = false;
            let mut cc = col.walk();
            for part in col.children(&mut cc) {
                match part.kind() {
                    "keyword_references" => seen_ref = true,
                    "object_reference" if seen_ref => {
                        seen_ref = false;
                        if let Some(target) = sql_object_ref_name(part, content) {
                            if target != table {
                                out.refs.push(SymbolRef {
                                    source_id: source.id(),
                                    kind: RelationshipType::Uses,
                                    target_name: target.clone(),
                                    qualifier: None,
                                    line: start_line(part),
                                });
                            }
                            out.relationships
                                .extend(build_uses_rels(source, &table, &target, names));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Name of a SQL `object_reference` — its `name` field identifier. When given a
/// wrapper node (e.g. `create_table`), returns the name of its first
/// `object_reference` child (the table/view being defined).
pub(super) fn sql_object_ref_name(node: Node, content: &str) -> Option<String> {
    let target = if node.kind() == "object_reference" {
        node
    } else {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|c| c.kind() == "object_reference")?
    };
    let name = target.child_by_field_name("name")?;
    let s = node_text(name, content).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// `uses` relationships from `source`, named `from`, to the definitions of
/// `to` in this file, its type definitions when it has any, one per
/// definition (see [`LocalNames::type_candidates`]);
/// none to `source` itself.
pub(super) fn build_uses_rels(
    source: &Entity,
    from: &str,
    to: &str,
    names: &LocalNames,
) -> Vec<Relationship> {
    // A name a table uses, like any `uses` reference, prefers type
    // definitions: what resolving the reference across files does too.
    let (targets, candidates) = names.type_candidates(to);
    let now = OffsetDateTime::now_utc();
    targets
        .iter()
        .filter(|target| target.id() != source.id())
        .map(|target| {
            let mut relationship = Relationship {
                id: Uuid::new_v4(),
                source_id: source.id(),
                target_id: target.id(),
                relationship_type: RelationshipType::Uses,
                metadata: Some(serde_json::json!({ "from": from, "to": to })),
                created_at: now,
            };
            relationship.mark_ambiguous(candidates);
            relationship
        })
        .collect()
}

/// Whether a type `child_name` naming `parent_name` as a supertype is linked
/// to it: not for an empty name, nor for the universal roots every class
/// would otherwise point at (`object`, `Object`, `Exception`, `Any`,
/// `AnyObject`).
pub(super) fn is_linked_supertype(child_name: &str, parent_name: &str) -> bool {
    !parent_name.is_empty()
        && !child_name.is_empty()
        && !matches!(
            parent_name,
            "object" | "Object" | "Exception" | "Any" | "AnyObject"
        )
}

/// The kind of a supertype reference: `Implements` in an `implements` list,
/// else `Inherits`, which yields `Implements` when the supertype resolves to
/// an interface, trait or protocol.
pub(super) fn supertype_kind(implemented: bool) -> RelationshipType {
    if implemented {
        RelationshipType::Implements
    } else {
        RelationshipType::Inherits
    }
}

/// What a type that names `parent` where it extends does to it: a class,
/// struct or enum implements an interface or trait, and inherits anything
/// else; an interface or trait inherits what it extends, interfaces included.
pub(crate) fn extended_kind(child: &str, parent: &str) -> RelationshipType {
    let is_interface = |kind: &str| matches!(kind, "interface" | "trait");
    if is_interface(parent) && !is_interface(child) {
        RelationshipType::Implements
    } else {
        RelationshipType::Inherits
    }
}

/// Resolve a supertype `parent_name` named by the type `child_name` to
/// relationships, choosing `Implements` when the parent is an
/// interface/trait/protocol and `Inherits` otherwise: one per pair of the
/// `children` (definitions of the child, with how many the file has) and the
/// parent's definitions in this file (see [`LocalNames::type_candidates`]).
/// None for unknown/self/`Object` parents.
pub(super) fn build_hierarchy_rels(
    children: (&[&Entity], usize),
    child_name: &str,
    parent_name: &str,
    implemented: bool,
    names: &LocalNames,
) -> Vec<Relationship> {
    if !is_linked_supertype(child_name, parent_name) {
        return Vec::new();
    }
    let (children, child_candidates) = children;
    let (parents, parent_candidates) = names.type_candidates(parent_name);
    let now = OffsetDateTime::now_utc();
    let mut rels = Vec::new();
    for child in children {
        for parent in parents {
            if parent.id() == child.id() {
                continue; // never emit a self-referential edge
            }
            let relationship_type = if implemented {
                RelationshipType::Implements
            } else {
                extended_kind(child.entity_type(), parent.entity_type())
            };
            let mut relationship = Relationship {
                id: Uuid::new_v4(),
                source_id: child.id(),
                target_id: parent.id(),
                relationship_type,
                metadata: Some(serde_json::json!({ "child": child_name, "parent": parent_name })),
                created_at: now,
            };
            relationship.mark_ambiguous(child_candidates * parent_candidates);
            rels.push(relationship);
        }
    }
    rels
}

/// A supertype a class names, and whether it is named in an `implements` list.
pub(crate) struct Supertype {
    /// Its name, its last segment without type arguments.
    pub(crate) name: String,
    /// Whether it is named in an `implements` list rather than extended.
    pub(crate) implemented: bool,
}

/// Collect supertype names from a class-like node across grammars, handling the
/// different shapes each tree-sitter grammar uses for `extends`/`implements`/`:`.
pub(crate) fn collect_supertypes(class_node: Node, content: &str) -> Vec<Supertype> {
    let mut out = Vec::new();
    // An Objective-C class names its superclass in a field of its own.
    if class_node.kind() == "class_interface"
        && let Some(superclass) = class_node.child_by_field_name("superclass")
    {
        push_supertypes(superclass, content, false, &mut out);
    }
    let mut cursor = class_node.walk();
    let children: Vec<Node> = class_node.children(&mut cursor).collect();
    for (at, &child) in children.iter().enumerate() {
        if let Some(clause) = vb_heritage_statement(child, children.get(at + 1).copied(), content) {
            out.extend(clause);
            continue;
        }
        match child.kind() {
            // A Java or TypeScript interface's `extends`.
            "extends_interfaces" | "extends_type_clause" => {
                push_supertypes(child, content, false, &mut out);
            }
            // Java `super_interfaces`, TS and VB `implements_clause`, Dart
            // `interfaces`, PHP `class_interface_clause`, GraphQL
            // `implements_interfaces`, an Objective-C class's protocols: the
            // `implements` lists.
            "super_interfaces"
            | "implements_clause"
            | "interfaces"
            | "class_interface_clause"
            | "implements_interfaces" => push_supertypes(child, content, true, &mut out),
            "parameterized_arguments" if class_node.kind() == "class_interface" => {
                push_supertypes(child, content, true, &mut out);
            }
            // Java, Dart and Ruby `superclass` (Dart's with its `mixins`); PHP
            // `base_clause`; C++ `base_class_clause`; C# `base_list`; VB
            // `inherits_clause`; Swift `inheritance_specifier`/
            // `type_inheritance_clause`; grammars that expose `extends_clause`
            // directly. Where a list does not say which it names, the type
            // named decides between `inherits` and `implements`.
            "superclass"
            | "base_clause"
            | "base_class_clause"
            | "base_list"
            | "inherits_clause"
            | "extends_clause"
            | "inheritance_specifier"
            | "type_inheritance_clause" => push_supertypes(child, content, false, &mut out),
            // JS/TS nest extends/implements inside `class_heritage`.
            "class_heritage" => {
                let mut c2 = child.walk();
                for h in child.children(&mut c2) {
                    push_supertypes(h, content, h.kind() == "implements_clause", &mut out);
                }
            }
            // Kotlin `class X : Super(args), Iface` — under delegation specifiers.
            "delegation_specifiers" | "delegation_specifier" => {
                push_supertypes(child, content, false, &mut out);
            }
            // F# `inherit Base(args)`, and `interface IFoo with …` among the
            // type's members: the type each names, not its arguments or members.
            "class_inherits_decl" => push_fsharp_supertype(child, content, false, &mut out),
            "type_extension_elements" => {
                let mut c2 = child.walk();
                for element in child.children(&mut c2) {
                    if element.kind() == "interface_implementation" {
                        push_fsharp_supertype(element, content, true, &mut out);
                    }
                }
            }
            // Python `class X(Base1, Base2)` — an `argument_list` of the bases
            // (direct child of the class, unlike a nested constructor arg-list).
            "argument_list" | "superclasses" => {
                let mut c2 = child.walk();
                for a in child.children(&mut c2) {
                    if matches!(a.kind(), "identifier" | "attribute" | "dotted_name") {
                        let name = node_text(a, content).trim().to_string();
                        if !name.is_empty() {
                            out.push(Supertype {
                                name,
                                implemented: false,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The supertypes of a VB.NET `Inherits` or `Implements` statement. The
/// grammar reads the clauses only on the `Class` line itself, where VB.NET
/// does not put them, and parses the statement on the line below as a field
/// declaration with an error in it, or as the keyword alone — an error or a
/// field of its own — with the types the field `next` declares; so it is read
/// from its text: the keyword, then the types, each named by its last
/// segment, without type arguments. Both words are reserved, so no field is
/// named either. An `Implements` the grammar splits in two is read from both
/// parts.
pub(crate) fn vb_heritage_statement(
    node: Node,
    next: Option<Node>,
    content: &str,
) -> Option<Vec<Supertype>> {
    if !matches!(node.kind(), "field_declaration" | "ERROR") {
        return None;
    }
    let text = node_text(node, content).trim();
    let (keyword, types) = match text.split_once(char::is_whitespace) {
        Some(statement) => statement,
        None => (text, node_text(next?, content).trim()),
    };
    let implemented = if keyword.eq_ignore_ascii_case("Inherits") {
        false
    } else if keyword.eq_ignore_ascii_case("Implements") {
        true
    } else {
        return None;
    };
    Some(
        types
            .split(',')
            .filter_map(|ty| {
                let ty = ty.split('(').next().unwrap_or(ty).trim();
                let name = ty.rsplit('.').next().unwrap_or(ty).trim();
                (!name.is_empty()).then(|| Supertype {
                    name: name.to_string(),
                    implemented,
                })
            })
            .collect(),
    )
}

/// The supertype an F# `inherit` or `interface … with` names: its type, the
/// first part of it whose kind is a type.
pub(super) fn push_fsharp_supertype(
    clause: Node,
    content: &str,
    implemented: bool,
    out: &mut Vec<Supertype>,
) {
    let mut cursor = clause.walk();
    let name = clause
        .named_children(&mut cursor)
        .find(|part| part.kind().ends_with("_type"))
        .map(|ty| base_type_name(ty, content))
        .unwrap_or_default();
    if !name.is_empty() {
        out.push(Supertype { name, implemented });
    }
}

/// The supertypes a heritage clause names.
pub(super) fn push_supertypes(
    clause: Node,
    content: &str,
    implemented: bool,
    out: &mut Vec<Supertype>,
) {
    let mut names = Vec::new();
    collect_type_names(clause, content, &mut names);
    out.extend(
        names
            .into_iter()
            .map(|name| Supertype { name, implemented }),
    );
}

/// Recursively collect type-name leaves from a heritage clause, skipping value
/// and type-argument subtrees so constructor call arguments (Kotlin
/// `: Super(x)`) and generic parameters are not mistaken for supertypes.
pub(super) fn collect_type_names(node: Node, content: &str, out: &mut Vec<String>) {
    match node.kind() {
        "value_arguments"
        | "arguments"
        | "argument_list"
        | "type_arguments"
        | "type_parameters"
        | "type_argument_list"
        | "template_argument_list"
        | "generic_arguments" => {}
        // A qualified name — `App\Base`, `Shop::Base`, `com.shop.Base` —
        // names the type its last segment does.
        "qualified_name"
        | "qualified_identifier"
        | "scope_resolution"
        | "scoped_type_identifier"
        | "scoped_identifier"
        | "relative_name" => {
            let last = node
                .child_by_field_name("name")
                .or_else(|| node.named_child(node.named_child_count().saturating_sub(1) as u32));
            if let Some(last) = last {
                collect_type_names(last, content, out);
            }
        }
        // PHP's and GraphQL's names are `name`s, Ruby's `constant`s.
        "type_identifier" | "identifier" | "name" | "constant" => {
            let s = node_text(node, content).trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_type_names(child, content, out);
            }
        }
    }
}

/// First type-name leaf under a node (e.g. the base of a generic/scoped type).
pub(crate) fn base_type_name(node: Node, content: &str) -> String {
    let mut names = Vec::new();
    collect_type_names(node, content, &mut names);
    names.into_iter().next().unwrap_or_default()
}
