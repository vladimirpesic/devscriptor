//! Value uses: a name read as a value — a constant, a function passed on, a
//! type named — recorded as a use of what it names, and the syntax that tells
//! a use from a declaration, a binding or a member.

use super::*;

/// The member and path kinds, by grammar, each with the fields its name may be
/// in, looked for in order: a kind with none (Kotlin's `navigation_expression`,
/// PHP's `class_constant_access_expression`, F#'s `long_identifier`) names its
/// last named child.
pub(super) const MEMBER_ACCESSES: &[(&str, &[&str])] = &[
    ("attribute", &["attribute"]),
    ("member_expression", &["property"]),
    ("field_expression", &["field"]),
    ("selector_expression", &["field"]),
    ("field_access", &["field"]),
    ("navigation_expression", &["suffix"]),
    ("member_access_expression", &["name"]),
    ("member_access", &["member"]),
    ("scoped_identifier", &["name"]),
    ("qualified_identifier", &["name"]),
    ("scope_resolution", &["name"]),
    ("scoped_property_access_expression", &["name"]),
    ("class_constant_access_expression", &[]),
    ("long_identifier", &[]),
];

/// The kinds of [`MEMBER_ACCESSES`] whose object is a path — a type or a
/// module, never a value — whose qualifier keeps its `::`, as a call's does
/// ([`call_receiver`]).
pub(super) const PATH_ACCESSES: &[&str] = &[
    "scoped_identifier",
    "qualified_identifier",
    "scope_resolution",
    "scoped_property_access_expression",
    "class_constant_access_expression",
];

/// The kinds of a bare name, by grammar: most grammars' `identifier`, Swift's
/// and Kotlin's `simple_identifier`, Ruby's `constant`, PHP's `name`, the
/// shell's `variable_name` (in `$MAX`) and JavaScript's
/// `shorthand_property_identifier` (`{ total }`).
pub(super) const NAME_KINDS: &[&str] = &[
    "identifier",
    "simple_identifier",
    "constant",
    "name",
    "variable_name",
    "shorthand_property_identifier",
];

/// The fields a name is declared, bound or labelled in, rather than used: a
/// definition's name, a declarator, a binding pattern, an alias, a label, a
/// parameter, a type. An assignment's or a loop's `left` binds too
/// ([`declares`]); a binary expression's is a use.
pub(super) const DECLARING_FIELDS: &[&str] = &[
    "name",
    "declarator",
    "pattern",
    "alias",
    "label",
    "parameter",
    "parameters",
    "type",
    "return_type",
    "returns",
];

/// The kinds a name is a label in, which no field says: C#'s `name:`, Swift's
/// argument labels, Dart's named arguments, a loop's label.
pub(super) const LABEL_KINDS: &[&str] = &[
    "name_colon",
    "value_argument_label",
    "label",
    "labeled_statement",
    "break_statement",
    "continue_statement",
    "goto_statement",
];

/// What names a package, a namespace or an attribute, never a value: Kotlin's
/// `package_header`, Java's `package_declaration`, Scala's `package_clause`,
/// PHP's `namespace_name`, C#'s `qualified_name` (of a namespace or a type),
/// Dart's `library_name`, Rust's `#[…]`.
pub(super) const NAMING_KINDS: &[&str] = &[
    "package_header",
    "package_declaration",
    "package_clause",
    "namespace_name",
    "qualified_name",
    "library_name",
    "attribute_item",
    "inner_attribute_item",
];

/// The clauses a type names its supertypes in, which are recorded as such
/// (`inherits`, `implements`): TypeScript's `extends Base`, C#'s base list,
/// Ruby's and PHP's superclass, Python's (`class Square(Shape)`, its
/// `superclasses`).
pub(super) const SUPERTYPE_CLAUSES: &[&str] = &[
    "extends_clause",
    "implements_clause",
    "class_heritage",
    "superclass",
    "super_interfaces",
    "extends_interfaces",
    "base_list",
    "base_clause",
    "class_interface_clause",
    "inheritance_specifier",
    "delegation_specifier",
];

/// Names that are a language's own values, never a definition's: literals,
/// the receiver, Kotlin's implicit `it`.
pub(super) const VALUE_KEYWORDS: &[&str] = &[
    "true",
    "false",
    "null",
    "nil",
    "None",
    "True",
    "False",
    "undefined",
    "self",
    "this",
    "super",
    "Self",
    "it",
    "_",
    "NaN",
    "Infinity",
    "arguments",
];

/// A name the code uses as a value, not calling it: what it names, what it is
/// a member of (`ReeferLimits` of `ReeferLimits.MAX_CELSIUS`, `Self::` of
/// `Self::MIN`), and its line.
pub(super) struct ValueUse {
    name: String,
    qualifier: Option<String>,
    line: u32,
}

/// The names a file's code uses as values, and what [`value_use`] reads them
/// with: the names each function binds, read once per function.
pub(super) struct ValueUses<'n, 'a> {
    content: &'n str,
    language: &'n str,
    config: &'n crate::parser::language_config::LanguageNodeConfig,
    names: &'n LocalNames<'a>,
    bound: std::collections::HashMap<usize, std::collections::HashSet<String>>,
}

/// Record the names `tree`'s code uses as values: the constants and
/// properties it reads or writes (`ReeferLimits.MAX_CELSIUS`,
/// `config.DocCutoff`, `self.color`, a bare `MAX_RETRIES`), and the functions
/// it passes (`HandleFunc("/x", handleCheckout)`, `items.map(format)`,
/// `run(this::work)`). Each is a reference of the entity it is made in, linked
/// to the file's constants and functions of its name. With only calls and
/// types recorded, the index would hold no use of a constant a server missed,
/// and the files reading one would not be opened before the server is asked.
///
/// A name is no use where it is declared, bound or called: a definition, a
/// parameter or local of an enclosing function, a callee, a type, an import,
/// a label, or what an import binds (`config` of `config.DocCutoff`, whose
/// import is recorded). A name the language has — `true`, `self`, a
/// built-in — is none either.
pub(super) fn record_value_uses(
    tree: &Tree,
    content: &str,
    language: &str,
    config: &crate::parser::language_config::LanguageNodeConfig,
    names: &LocalNames,
    line_index: &Extents,
    out: &mut Extraction,
) {
    // Markup and query languages name nothing the way code does; a
    // component's scripts are read in their own language.
    if matches!(language, "html" | "sql" | "graphql" | "vue" | "svelte") {
        return;
    }
    let mut uses = ValueUses {
        content,
        language,
        config,
        names,
        bound: std::collections::HashMap::new(),
    };
    let mut seen = std::collections::HashSet::<(Uuid, String, Option<String>, u32)>::new();
    let now = OffsetDateTime::now_utc();
    let mut stack = vec![(tree.root_node(), 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_TRAVERSE_DEPTH {
            continue;
        }
        // What an import names is recorded as the import (`file_imports`);
        // the shell's `$(...)`, listed with its imports, is a command.
        if (language != "bash" && matches_node_type(node.kind(), &config.import_nodes))
            || NAMING_KINDS.contains(&node.kind())
        {
            continue;
        }
        if let Some(found) = uses.value_use(node)
            && let Some(source_id) = find_containing_entity(node, line_index)
            && seen.insert((
                source_id,
                found.name.clone(),
                found.qualifier.clone(),
                found.line,
            ))
        {
            link_value_use(source_id, &found, names, now, out);
            out.refs.push(SymbolRef {
                source_id,
                kind: RelationshipType::References,
                target_name: found.name,
                qualifier: found.qualifier,
                line: found.line,
            });
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
    }
}

/// Link the use `found` makes from `source_id` to the file's constants and
/// functions of its name ([`LocalNames::value_candidates`]).
pub(super) fn link_value_use(
    source_id: Uuid,
    found: &ValueUse,
    names: &LocalNames,
    now: OffsetDateTime,
    out: &mut Extraction,
) {
    let scope = CallScope::of(
        found.qualifier.as_deref(),
        names.owner_of(source_id),
        &found.name,
        names.language,
    );
    let (targets, candidates) = names.value_candidates(&found.name, scope);
    out.relationships.extend(targets.iter().map(|target| {
        let mut relationship = Relationship {
            id: Uuid::new_v4(),
            source_id,
            target_id: target.id(),
            relationship_type: RelationshipType::References,
            metadata: Some(serde_json::json!({ "used_name": found.name })),
            created_at: now,
        };
        relationship.mark_ambiguous(candidates);
        relationship
    }));
}

impl ValueUses<'_, '_> {
    /// The value use `node` is, if any: a member or path not called, by its
    /// name and what it is of, or a bare name in an expression that no
    /// enclosing function binds.
    fn value_use(&mut self, node: Node) -> Option<ValueUse> {
        if let Some(name) = member_name(node) {
            let text = node_text(name, self.content).trim().trim_start_matches('$');
            if text.is_empty()
                || VALUE_KEYWORDS.contains(&text)
                || is_callee(node, self.config)
                || in_type_position(node)
                || declares(node)
            {
                return None;
            }
            return Some(ValueUse {
                name: text.to_string(),
                qualifier: qualifier_of(node, name, self.content),
                line: start_line(name),
            });
        }
        let kind = node.kind();
        if !NAME_KINDS.contains(&kind) || node.named_child_count() > 0 {
            return None;
        }
        let parent = node.parent()?;
        // A member's own name is the member's use. A name before a
        // parenthesized group in a Rust macro's arguments is a call, recorded
        // as one ([`extract_token_tree_calls`]).
        if is_member_name(node)
            || (parent.kind() == "token_tree"
                && node.next_sibling().is_some_and(|next| {
                    next.kind() == "token_tree" && node_text(next, self.content).starts_with('(')
                }))
        {
            return None;
        }
        let text = node_text(node, self.content).trim();
        let line = start_line(node);
        // A built-in's name is the built-in, unless the file defines it
        // (`xs.map(::format)` of its own `format`).
        // What a whole-module import binds is the object of its members
        // (`config.DocCutoff`, `fmt.Println`): the import is recorded. A
        // name an import binds read on its own (`LIMIT`) is a use of it.
        let module_object = member_name(parent).is_some() && self.names.bound.contains(text);
        // A parameter's own name, but not its default value.
        let parameter = parent.kind().contains("parameter")
            && !in_field(node, "value")
            && !in_field(node, "default_value");
        if text.is_empty()
            || module_object
            || parameter
            || in_binding_pattern(node)
            || in_supertypes(node)
            || VALUE_KEYWORDS.contains(&text)
            || (crate::parser::cross_file::is_common_builtin(text, self.language)
                && !self.names.by_name.contains_key(text))
            || self.names.defines_on(text, line)
            || LABEL_KINDS.contains(&parent.kind())
            || is_callee(node, self.config)
            || in_type_position(node)
            || declares(node)
            || self.is_bound(node, text)
        {
            return None;
        }
        Some(ValueUse {
            name: text.to_string(),
            qualifier: None,
            line,
        })
    }

    /// Whether a function around `node` binds `name`: a parameter or local of
    /// its own or, for a closure, of the function it is in.
    fn is_bound(&mut self, node: Node, name: &str) -> bool {
        let mut ancestor = node.parent();
        while let Some(function) = ancestor {
            if matches_node_type(function.kind(), &self.config.function_nodes) {
                let content = self.content;
                if self
                    .bound
                    .entry(function.id())
                    .or_insert_with(|| locally_bound_names(function, content))
                    .contains(name)
                {
                    return true;
                }
            }
            ancestor = function.parent();
        }
        false
    }
}

/// The name a member or path names, when `node` is one ([`MEMBER_ACCESSES`]),
/// down to the name itself: Swift's is the `suffix` of its
/// `navigation_suffix`, PHP's `Limits::$count` the `name` of its variable.
pub(super) fn member_name(node: Node) -> Option<Node> {
    let &(_, fields) = MEMBER_ACCESSES
        .iter()
        .find(|(kind, _)| *kind == node.kind())?;
    let count = u32::try_from(node.named_child_count()).ok()?;
    if count < 2 {
        return None;
    }
    let name = fields
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| node.named_child(count - 1))?;
    name_leaf(name)
}

/// The name a member's name node is: itself when a leaf name, else the one
/// name inside it.
pub(super) fn name_leaf(node: Node) -> Option<Node> {
    /// `navigation_suffix` → `simple_identifier`, `variable_name` → `name`.
    const MAX_DESCENT: usize = 3;
    let mut node = node;
    for _ in 0..MAX_DESCENT {
        if node.named_child_count() == 0 {
            return (node.kind().ends_with("identifier") || NAME_KINDS.contains(&node.kind()))
                .then_some(node);
        }
        node = node
            .child_by_field_name("suffix")
            .or_else(|| node.child_by_field_name("name"))
            .or_else(|| {
                (node.named_child_count() == 1)
                    .then(|| node.named_child(0))
                    .flatten()
            })?;
    }
    None
}

/// What the member `node`, named `name`, is of: the text before the name,
/// without its `.`, `?.`, `->` or `::`, when short and on one line; a path's
/// with its `::`.
pub(super) fn qualifier_of(node: Node, name: Node, content: &str) -> Option<String> {
    /// Longer objects are chained calls or literals, not a name.
    const MAX_LEN: usize = 64;
    let text = content.get(node.start_byte()..name.start_byte())?;
    let text = text
        .trim_end()
        .trim_end_matches(['.', ':', '>', '-', '?', '$'])
        .trim();
    if text.is_empty() || text.len() > MAX_LEN || text.contains('\n') {
        return None;
    }
    Some(if PATH_ACCESSES.contains(&node.kind()) {
        format!("{text}::")
    } else {
        text.to_string()
    })
}

/// Whether `node` is the name of a member or path around it, however its
/// grammar wraps the name (Swift's `navigation_suffix`).
pub(super) fn is_member_name(node: Node) -> bool {
    /// The name, its wrapper, the member.
    const MAX_ASCENT: usize = 3;
    let mut ancestor = node.parent();
    for _ in 0..MAX_ASCENT {
        let Some(member) = ancestor else {
            return false;
        };
        if member_name(member) == Some(node) {
            return true;
        }
        ancestor = member.parent();
    }
    false
}

/// Whether `node` is in `field` of its parent.
///
/// Asked of the field rather than of the node's place among its siblings: a
/// parent without the field says so at once, where finding the node's place
/// walks every sibling before it, which for each name in a module of many
/// definitions made a file's extraction quadratic.
pub(super) fn in_field(node: Node, field: &str) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.child_by_field_name(field) {
        None => false,
        Some(first) if first == node => true,
        Some(_) => {
            let mut cursor = parent.walk();
            parent
                .children_by_field_name(field, &mut cursor)
                .any(|child| child == node)
        }
    }
}

/// `node`'s children, each with the field it is in, read in one walk: asking
/// each child's field by its place walks the children before it again.
pub(super) fn children_and_fields<'t>(node: Node<'t>) -> Vec<(Node<'t>, Option<&'t str>)> {
    let mut cursor = node.walk();
    let mut children = Vec::new();
    if cursor.goto_first_child() {
        loop {
            children.push((cursor.node(), cursor.field_name()));
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    children
}

/// The first of `fields` that `node` is in, if any ([`in_field`]).
pub(super) fn field_among<'f>(
    node: Node,
    fields: impl IntoIterator<Item = &'f str>,
) -> Option<&'f str> {
    fields.into_iter().find(|field| in_field(node, field))
}

/// Whether `node` is bound by a pattern around it — a destructuring, a
/// match arm, F#'s `let MAX = 3` (`identifier_pattern`) — or named on the
/// left of F#'s function declaration. A constant matched against (C#'s
/// `constant_pattern`) is a use.
pub(super) fn in_binding_pattern(node: Node) -> bool {
    /// The name, its wrappers, the pattern.
    const MAX_ASCENT: usize = 3;
    let mut ancestor = node.parent();
    for _ in 0..MAX_ASCENT {
        let Some(around) = ancestor else {
            return false;
        };
        let kind = around.kind();
        if (kind.ends_with("pattern") && kind != "constant_pattern")
            || kind.contains("declaration_left")
        {
            return true;
        }
        ancestor = around.parent();
    }
    false
}

/// Whether `node` names a supertype of the type around it
/// ([`SUPERTYPE_CLAUSES`]).
pub(super) fn in_supertypes(node: Node) -> bool {
    /// The name, its wrappers, the clause.
    const MAX_ASCENT: usize = 3;
    let mut ancestor = node.parent();
    for _ in 0..MAX_ASCENT {
        let Some(around) = ancestor else {
            return false;
        };
        if SUPERTYPE_CLAUSES.contains(&around.kind()) || in_field(around, "superclasses") {
            return true;
        }
        ancestor = around.parent();
    }
    false
}

/// Whether a node of `kind` is a loop binding the names on its left: Python's
/// `for x in`, a comprehension's, Java's `for (Item i : items)`, C++'s range
/// `for`, Go's `range`, Rust's `for x in`.
pub(super) fn is_loop(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "for_in_clause"
            | "enhanced_for_statement"
            | "for_range_loop"
            | "foreach_statement"
            | "for_expression"
            | "range_clause"
    )
}

/// The node a value stands for in its parent: `node`, or what wraps it alone
/// (Solidity's and VB's `expression`, F#'s `long_identifier_or_op`).
pub(super) fn unwrapped(node: Node) -> Node {
    let mut node = node;
    while let Some(parent) = node.parent() {
        if parent.named_child_count() == 1
            && matches!(parent.kind(), "expression" | "long_identifier_or_op")
        {
            node = parent;
        } else {
            break;
        }
    }
    node
}

/// Whether `node` is what a call calls, its callee read as
/// [`extract_called_function_name`] reads it: a call, recorded as one.
pub(super) fn is_callee(
    node: Node,
    config: &crate::parser::language_config::LanguageNodeConfig,
) -> bool {
    let node = unwrapped(node);
    node.parent().is_some_and(|call| {
        matches_node_type(call.kind(), &config.call_nodes)
            && ["name", "method", "function", "constructor", "macro"]
                .iter()
                .find_map(|field| call.child_by_field_name(field))
                .or_else(|| call.named_child(0))
                == Some(node)
    })
}

/// Whether `node` declares, binds or labels a name where it is
/// ([`DECLARING_FIELDS`]); an assignment's or a loop's `left` binds a name, as
/// a loop's `variable` or `item` does, and writes a member, which is a use.
pub(super) fn declares(node: Node) -> bool {
    let node = unwrapped(node);
    // The fields that bind only in some parents, then those that always do.
    let fields = ["left", "variable", "item", "key"]
        .into_iter()
        .chain(DECLARING_FIELDS.iter().copied());
    let (Some(parent), Some(field)) = (node.parent(), field_among(node, fields)) else {
        return false;
    };
    let parent_kind = parent.kind();
    match field {
        // An assignment's target, a loop's variable (the shell's `for v in`).
        "left" | "variable" | "item" => {
            let binds = parent_kind.contains("assignment")
                || parent_kind.contains("declaration")
                || is_loop(parent_kind);
            binds && member_name(node).is_none()
        }
        // An annotation's argument name (Java's `@Size(max = …)`); a
        // dictionary's key (a `pair`'s) may be a constant.
        "key" => parent_kind != "pair",
        _ => true,
    }
}

/// Whether `node` is in a type: its parent or grandparent is one
/// (`user_type`, `type_annotation`, `type_arguments`), which a signature's
/// types are recorded from.
pub(super) fn in_type_position(node: Node) -> bool {
    let is_type = |kind: &str| {
        kind == "type"
            || kind.ends_with("_type")
            || kind == "type_annotation"
            || (kind.starts_with("type_")
                && !kind.contains("expression")
                && !kind.contains("declaration")
                && !kind.contains("definition")
                && !kind.contains("alias"))
    };
    let parent = node.parent();
    parent.is_some_and(|parent| is_type(parent.kind()))
        || parent
            .and_then(|parent| parent.parent())
            .is_some_and(|grandparent| is_type(grandparent.kind()))
}
