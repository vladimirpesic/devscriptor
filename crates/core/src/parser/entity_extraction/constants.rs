//! Constants: which bindings are constants in each language, and the entity
//! each is.

use super::*;

/// Returns `true` when the identifier `name` looks like a module-level constant
/// for languages that use generic `assignment`/`declaration` nodes for constants.
///
/// Accepted: ALL_CAPS, ALL_CAPS_WITH_NUMBERS, SCREAMING_SNAKE_CASE.
/// This is the community convention in Python, Ruby, C defines, and C++ consts.
/// One capital letter is not: `X` and `Y` name coordinates, `T` a type and
/// `N` a bound far more often, and a one-letter constant is declared with
/// `const`, `final`, `val` or `let`, which says so.
pub(super) fn is_constant_name(name: &str) -> bool {
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
pub(super) fn is_function_local(node: Node) -> bool {
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
pub(super) fn is_immutable_binding(node: Node, content: &str, language: &str) -> bool {
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

pub(super) fn has_immutability_keyword(node: Node, content: &str) -> bool {
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
pub(super) fn is_redundant_c_declarator(node: Node) -> bool {
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
pub(super) fn count_init_declarators(declaration: Node) -> usize {
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
pub(super) fn extract_constant(
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
pub(super) fn ruby_constant(
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
pub(super) fn module_constant(
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
