//! The name of the entity a node defines, read as each language writes it, and
//! the type a member belongs to.

use super::*;

/// Extract the name from a node using various strategies
pub(super) fn extract_node_name(node: Node, content: &str, language: &str) -> Option<String> {
    // HTML markup has no `name` field anywhere, so every element fell through to
    // `extract_component`'s file-stem fallback — which is right for Vue and
    // Svelte, where a file *is* one component, but in HTML it named every
    // element after the page. A five-element document produced five entities all
    // called `index`, colliding in search and in the graph.
    if language == "html"
        && let Some(tag) = html_element_name(node, content)
    {
        return Some(tag);
    }

    // A JavaScript or TypeScript class expression is named by its own name,
    // else by what it is bound to (`const Cart = class { … }`).
    if matches!(language, "javascript" | "typescript" | "tsx") && node.kind() == "class" {
        return node
            .child_by_field_name("name")
            .map(|name| node_text(name, content).trim().to_string())
            .or_else(|| js_function_name(node, content));
    }

    // A Solidity `receive()` or `fallback()` is named by its keyword.
    if node.kind() == "fallback_receive_definition" {
        return node
            .child(0)
            .map(|keyword| node_text(keyword, content).trim().to_string());
    }

    // A Kotlin secondary constructor is named by its keyword, as Kotlin
    // declares it.
    if language == "kotlin" && node.kind() == "secondary_constructor" {
        return Some("constructor".to_string());
    }

    // A Kotlin property names its one binding in its `variable_declaration`;
    // a destructuring `val (a, b)` binds several and stays unnamed.
    if language == "kotlin" && node.kind() == "property_declaration" {
        let mut cursor = node.walk();
        let declaration = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "variable_declaration")?;
        let mut cursor = declaration.walk();
        return declaration
            .named_children(&mut cursor)
            .find(|child| child.kind() == "identifier")
            .map(|name| node_text(name, content).trim().to_string());
    }

    // A Swift deinitializer or subscript is named by its keyword; a
    // subscript's `name` field holds a type.
    if language == "swift" {
        match node.kind() {
            "deinit_declaration" => return Some("deinit".to_string()),
            "subscript_declaration" => return Some("subscript".to_string()),
            _ => {}
        }
    }

    // If the node *is* an identifier — reached by recursing through a `declarator`
    // chain, e.g. the nested `identifier` inside a C/C++ `function_declarator` —
    // return its own text. Without this, C-style free functions (whose name is
    // buried under `function_definition → function_declarator → identifier`)
    // resolve to no name and are dropped entirely by `extract_function`.
    if matches!(
        node.kind(),
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "variable_identifier"
            | "property_identifier"
    ) {
        let name = node_text(node, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try "name" field first (most common)
    if let Some(name_node) = node.child_by_field_name("name") {
        // An F# member's is `this.Area`: the instance, then the member. A Swift
        // property's is a pattern, binding one name or a tuple of them.
        let name_node = match name_node.kind() {
            "property_or_ident" => {
                Some(name_node.child_by_field_name("method").unwrap_or(name_node))
            }
            "pattern" => name_node.child_by_field_name("bound_identifier"),
            _ => Some(name_node),
        };
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    // Try "identifier" field
    if let Some(ident_node) = node.child_by_field_name("identifier") {
        let name = node_text(ident_node, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try "declarator" field (C-style). A C/C++/CUDA `function_definition` nests
    // its name under `declarator: function_declarator → declarator: identifier`
    // (and one more `*_declarator` level for pointer/reference returns), so
    // recurse — but *fall through* to the sibling strategies below when the
    // recursion is fruitless, never short-circuiting to `None`.
    if let Some(decl_node) = node.child_by_field_name("declarator")
        && let Some(name) = extract_node_name(decl_node, content, language)
    {
        return Some(name);
    }

    // A PHP constant names itself in its first `const_element`.
    if node.kind() == "const_declaration" {
        let mut cursor = node.walk();
        let element = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "const_element");
        if let Some(element) = element {
            let mut cursor = element.walk();
            if let Some(name) = element
                .named_children(&mut cursor)
                .find(|part| part.kind() == "name")
            {
                return Some(node_text(name, content).trim().to_string());
            }
        }
    }

    // A C# field names itself in the first declarator of its
    // `variable_declaration`, a VB.NET field in its first declarator.
    if node.kind() == "field_declaration"
        && let Some(declarator) = first_variable_declarator(node)
        && let Some(name) = declarator.child_by_field_name("name")
    {
        let name = node_text(name, content).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Try to find first identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "identifier"
            || kind == "type_identifier"
            || kind == "variable_identifier"
            || kind == "property_identifier"
            || kind == "field_identifier"
            || kind == "string"
        {
            let name = node_text(child, content).trim().to_string();
            if !name.is_empty() && !name.starts_with('"') {
                return Some(name.trim_matches('"').to_string());
            }
        }
    }

    // Language-specific name extraction
    extract_language_specific_name(node, content, language)
}

// ---------------------------------------------------------------------------
// Language-specific name extractors — registered in a static dispatch table
// ---------------------------------------------------------------------------

/// Handler signature for language-specific name extraction.
pub(super) type NameExtractor = fn(Node, &str) -> Option<String>;

/// Lazily-built HashMap from canonical language name → extractor function.
/// New languages should register their extractor here instead of adding
/// another `match` arm.
pub(super) static LANGUAGE_NAME_EXTRACTORS: LazyLock<HashMap<&'static str, NameExtractor>> =
    LazyLock::new(|| {
        let mut m = HashMap::<&'static str, NameExtractor>::new();
        m.insert("bash", extract_bash_name);
        m.insert("sh", extract_bash_name);
        m.insert("vb_net", extract_vb_net_name);
        m.insert("fsharp", extract_fsharp_name);
        m.insert("graphql", extract_graphql_name);
        m.insert("sql", extract_sql_name);
        m.insert("dart", extract_dart_name);
        m
    });

// ---------------------------------------------------------------------------
// Per-language extractor implementations
// ---------------------------------------------------------------------------

pub(super) fn extract_bash_name(node: Node, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "word" || kind == "identifier" || kind == "function_name" {
            let text = node_text(child, content).trim().to_string();
            if !text.is_empty() && text != "function" && !text.starts_with('(') {
                return Some(text);
            }
        }
    }
    None
}

/// SQL `CREATE TABLE/VIEW/…` wraps the object's name one level down:
/// `create_table → object_reference → name: identifier`. The generic strategies
/// look only at direct `name`/`identifier` children, so without this the table
/// name is never found and the table entity is dropped — only its
/// `column_definition` children survive (misleadingly, as `struct`s).
pub(super) fn extract_sql_name(node: Node, content: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "object_reference" {
            if let Some(name) = child.child_by_field_name("name") {
                let n = node_text(name, content).trim().to_string();
                if !n.is_empty() {
                    return Some(n);
                }
            }
            let mut inner = child.walk();
            for gc in child.children(&mut inner) {
                if gc.kind() == "identifier" {
                    let n = node_text(gc, content).trim().to_string();
                    if !n.is_empty() {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
}

/// The first `variable_declarator` of a field: its child, or its
/// `variable_declaration`'s.
pub(super) fn first_variable_declarator(field: Node) -> Option<Node> {
    let mut cursor = field.walk();
    let children: Vec<Node> = field.named_children(&mut cursor).collect();
    children.iter().find_map(|child| match child.kind() {
        "variable_declarator" => Some(*child),
        "variable_declaration" => {
            let mut inner = child.walk();
            child
                .named_children(&mut inner)
                .find(|part| part.kind() == "variable_declarator")
        }
        _ => None,
    })
}

/// VB.NET names a constructor `New`.
pub(super) fn extract_vb_net_name(node: Node, _content: &str) -> Option<String> {
    (node.kind() == "constructor_declaration").then(|| "New".to_string())
}

/// F# keeps a type's name in its definition's `type_name` (`Result` of
/// `Result<'T, 'E>`), an exception's in its `exception_name`, a module's or
/// namespace's in its `long_identifier` or `identifier`, and a function's or
/// value's in its `function_declaration_left`.
pub(super) fn extract_fsharp_name(node: Node, content: &str) -> Option<String> {
    let text = |part: Node| {
        let name = node_text(part, content).trim().to_string();
        (!name.is_empty()).then_some(name)
    };
    let mut cursor = node.walk();
    match node.kind() {
        "exception_definition" => text(node.child_by_field_name("exception_name")?),
        "named_module" | "module_defn" | "namespace" => node
            .children(&mut cursor)
            .find(|child| matches!(child.kind(), "long_identifier" | "identifier"))
            .and_then(text),
        "function_or_value_defn" => {
            for child in node.children(&mut cursor) {
                if child.kind() == "function_declaration_left" {
                    let mut inner_cursor = child.walk();
                    for inner in child.children(&mut inner_cursor) {
                        if inner.kind() == "identifier" {
                            return text(inner);
                        }
                    }
                }
                if child.kind() == "identifier" {
                    return text(child);
                }
            }
            None
        }
        _ => {
            let type_name = node
                .children(&mut cursor)
                .find(|child| child.kind() == "type_name")?;
            text(type_name.child_by_field_name("type_name")?)
        }
    }
}

/// An F# type with no constructor, no base type and only abstract members is
/// an interface (`type IGreeter = abstract Greet: string -> string`): F#
/// infers it, with no keyword to read, unless `[<Class>]` or
/// `[<AbstractClass>]` says otherwise.
pub(super) fn fsharp_type_is_interface(node: Node, content: &str) -> bool {
    if node.kind() != "anon_type_defn" {
        return false;
    }
    let declared_a_class = node.parent().is_some_and(|definition| {
        let mut cursor = definition.walk();
        definition.children(&mut cursor).any(|child| {
            child.kind() == "attributes" && node_text(child, content).contains("Class")
        })
    });
    if declared_a_class {
        return false;
    }
    let mut members = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "primary_constr_args" | "class_inherits_decl" | "interface_implementation" => {
                return false;
            }
            "member_defn" => members.push(child),
            "type_extension_elements" => {
                let mut elements = child.walk();
                for element in child.named_children(&mut elements) {
                    if element.kind() != "member_defn" {
                        return false;
                    }
                    members.push(element);
                }
            }
            _ => {}
        }
    }
    !members.is_empty()
        && members.iter().all(|member| {
            let mut parts = member.walk();
            member
                .named_children(&mut parts)
                .any(|part| part.kind() == "member_signature")
        })
}

/// `type Id = int` parses as a union of one case, `int`, though it abbreviates
/// `int`: a lone case with no fields and no `|` before it, named other than the
/// type itself (`type Marker = Marker` is a union).
pub(super) fn fsharp_union_abbreviates(node: Node, content: &str) -> bool {
    let mut cursor = node.walk();
    let Some(cases) = node
        .children(&mut cursor)
        .find(|child| child.kind() == "union_type_cases")
    else {
        return false;
    };
    let mut cursor = cases.walk();
    let [case] = cases.children(&mut cursor).collect::<Vec<_>>()[..] else {
        return false;
    };
    let mut cursor = case.walk();
    let [identifier] = case.children(&mut cursor).collect::<Vec<_>>()[..] else {
        return false;
    };
    identifier.kind() == "identifier"
        && extract_fsharp_name(node, content).as_deref()
            != Some(node_text(identifier, content).trim())
}

/// The type an F# member belongs to: the nearest type definition above it,
/// past its `member_defn`, the block of members and an `interface … with`. A
/// member of an object expression (`{ new IDisposable with … }`) belongs to
/// no named type.
pub(super) fn fsharp_member_owner(node: Node) -> Option<Node> {
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        match candidate.kind() {
            "anon_type_defn"
            | "record_type_defn"
            | "union_type_defn"
            | "interface_type_defn"
            | "exception_definition"
            | "type_extension" => return Some(candidate),
            "object_expression" | "function_or_value_defn" | "type_definition" => return None,
            _ => ancestor = candidate.parent(),
        }
    }
    None
}

/// Dart keeps a declaration's name in its signature: a function, getter,
/// setter or method `*_declaration` in its `signature` field — a method's
/// wrapped once more in a `method_signature` — a local function in its
/// `function_signature`, and a member without a body in the signature its
/// `declaration` holds. A `declaration` of fields holds none and gets no name:
/// its fields are entities of their own.
pub(super) fn extract_dart_name(node: Node, content: &str) -> Option<String> {
    match node.kind() {
        "function_signature" | "getter_signature" | "setter_signature" => {
            let name = node.child_by_field_name("name")?;
            Some(node_text(name, content).to_string())
        }
        // `Point`, or `Point.fromJson` for a named constructor: the `name`
        // field is each part and the dot between them.
        "constructor_signature"
        | "constant_constructor_signature"
        | "factory_constructor_signature"
        | "redirecting_factory_constructor_signature" => {
            let mut cursor = node.walk();
            let name: String = node
                .children_by_field_name("name", &mut cursor)
                .map(|part| node_text(part, content))
                .collect();
            (!name.is_empty()).then_some(name)
        }
        "operator_signature" => {
            let operator = node.child_by_field_name("operator")?;
            Some(format!("operator {}", node_text(operator, content)))
        }
        // `class Mixed = Base with Mixin;` names the class in its
        // `mixin_application_class`.
        "class_declaration" => {
            let application = named_child_of_kind(node, "mixin_application_class")?;
            let name = named_child_of_kind(application, "identifier")?;
            Some(node_text(name, content).to_string())
        }
        // `library shop.cart;`
        "library_name" => {
            let name = named_child_of_kind(node, "dotted_identifier_list")?;
            Some(node_text(name, content).to_string())
        }
        _ => {
            let signature = node.child_by_field_name("signature").or_else(|| {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .find(|child| child.kind().ends_with("signature"))
            })?;
            extract_dart_name(signature, content)
        }
    }
}

/// The name of the type a function is a member of: the nearest class, struct,
/// interface, trait or enum around it, past the body that holds its members
/// (`class_body`, `declaration_list`, …); the type a Rust `impl` is for; a Go
/// method's receiver type; the class a C++ method defined outside it names
/// (`Cart::total`). A function inside another function, or at the top of a
/// module, is a member of no type.
pub(super) fn member_owner_name(
    node: Node,
    content: &str,
    language: &str,
    config: &LanguageNodeConfig,
) -> Option<String> {
    let owner = match language {
        "dart" => {
            return dart_member_owner(node).and_then(|o| extract_node_name(o, content, language));
        }
        "fsharp" => {
            return fsharp_member_owner(node).and_then(|o| extract_node_name(o, content, language));
        }
        "go" => {
            let receiver = node.child_by_field_name("receiver")?;
            let mut cursor = receiver.walk();
            let parameter = receiver
                .named_children(&mut cursor)
                .find(|part| part.kind() == "parameter_declaration")?;
            let ty = parameter.child_by_field_name("type")?;
            return Some(base_type_name(ty, content)).filter(|name| !name.is_empty());
        }
        "cpp" | "cuda" => qualified_definition_scope(node, content),
        _ => None,
    };
    if owner.is_some() {
        return owner;
    }
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        let kind = candidate.kind();
        if matches_node_type(kind, &config.function_nodes) {
            return None;
        }
        if kind == "impl_item" {
            let ty = candidate.child_by_field_name("type")?;
            return Some(base_type_name(ty, content)).filter(|name| !name.is_empty());
        }
        // A type with no name (a Kotlin `companion object`) is passed for the
        // one around it.
        if is_type_node(candidate, language, config)
            && let Some(name) = extract_node_name(candidate, content, language)
        {
            return Some(name);
        }
        ancestor = candidate.parent();
    }
    None
}

/// Whether `node` declares a type whose members the functions inside it are.
pub(super) fn is_type_node(node: Node, language: &str, config: &LanguageNodeConfig) -> bool {
    let kind = node.kind();
    [
        &config.class_nodes,
        &config.struct_nodes,
        &config.interface_nodes,
        &config.trait_nodes,
        &config.enum_nodes,
    ]
    .iter()
    .any(|kinds| matches_node_type(kind, kinds))
        || matches!(kind, "class_definition" | "class_declaration" | "impl_item")
        || defines_an_extension(node, language)
}

/// The function a function is defined in, rather than in a type or at a
/// file's top: a closure's, a nested `def`'s — passed over an unnamed one,
/// a callback, for the one around it. It is part of the identity, so two
/// nested functions of one name, and parameters in different functions, are
/// told apart, with ids that do not move with their order.
pub(super) fn enclosing_function_name(
    node: Node,
    content: &str,
    language: &str,
    config: &LanguageNodeConfig,
) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(candidate) = ancestor {
        if matches_node_type(candidate.kind(), &config.function_nodes)
            && let Some(name) = function_name(candidate, content, language)
        {
            return Some(name);
        }
        if is_type_node(candidate, language, config) {
            return None;
        }
        ancestor = candidate.parent();
    }
    None
}

/// The name a function node is indexed by: a JavaScript function
/// expression's or a Java lambda's binding, else its own.
pub(super) fn function_name(node: Node, content: &str, language: &str) -> Option<String> {
    if matches!(language, "javascript" | "typescript" | "tsx")
        && is_js_function_expression(node.kind())
    {
        js_function_name(node, content)
    } else if matches!(
        (language, node.kind()),
        ("java", "lambda_expression") | ("go", "func_literal")
    ) {
        bound_lambda_name(node, content)
    } else {
        extract_node_name(node, content, language)
    }
    .filter(|name| !name.is_empty())
}

/// The interface or trait a method implements a member of, where its
/// definition says so (see [`Function::implements`](crate::entities::Function::implements)):
/// Rust's `impl Display for Order`, C#'s `void IDisposable.Dispose()`, F#'s
/// `interface IDisposable with`, VB.NET's `Implements IDisposable.Dispose` —
/// which the grammar does not parse, and reads as an error. By its last
/// segment, without type arguments: `fmt::Display` is `Display`, `From<T>`
/// `From`.
pub(super) fn implemented_supertype(node: Node, content: &str, language: &str) -> Option<String> {
    let last_segment = |text: &str| -> String {
        let text = text.split('<').next().unwrap_or(text).trim();
        text.rsplit(['.', ':']).next().unwrap_or(text).to_string()
    };
    let name = match language {
        "rust" => {
            let block = node
                .parent()
                .filter(|block| block.kind() == "declaration_list")?;
            let implementation = block.parent().filter(|item| item.kind() == "impl_item")?;
            base_type_name(implementation.child_by_field_name("trait")?, content)
        }
        "c_sharp" => {
            let mut cursor = node.walk();
            let specifier = node
                .children(&mut cursor)
                .find(|child| child.kind() == "explicit_interface_specifier")?;
            base_type_name(specifier, content)
        }
        "fsharp" => {
            let implementation = std::iter::successors(node.parent(), Node::parent)
                .take(3)
                .find(|ancestor| ancestor.kind() == "interface_implementation")?;
            let mut cursor = implementation.walk();
            let ty = implementation
                .named_children(&mut cursor)
                .find(|child| child.kind().ends_with("type"))?;
            last_segment(node_text(ty, content))
        }
        "vb_net" => {
            let mut cursor = node.walk();
            let clause = node.children(&mut cursor).find_map(|child| {
                let text = node_text(child, content).trim();
                text.get(..10)
                    .filter(|keyword| keyword.eq_ignore_ascii_case("implements"))
                    .map(|_| text[10..].trim())
            })?;
            let member = clause.split(',').next().unwrap_or(clause).trim();
            last_segment(member.rsplit_once('.')?.0)
        }
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

/// The class a C++ method defined outside it names: `Cart` in
/// `int Cart::total() {…}`, `Order` in `shop::Order::count`. A namespace
/// qualifier reads the same, but a free function is defined inside its
/// `namespace { … }` far more often than with one.
pub(super) fn qualified_definition_scope(node: Node, content: &str) -> Option<String> {
    let mut declarator = node.child_by_field_name("declarator")?;
    while declarator.kind() != "function_declarator" {
        declarator = declarator.child_by_field_name("declarator")?;
    }
    let mut name = declarator.child_by_field_name("declarator")?;
    let mut scope = None;
    while name.kind() == "qualified_identifier" {
        scope = name.child_by_field_name("scope");
        name = name.child_by_field_name("name")?;
    }
    let scope = scope?;
    let scope = scope.child_by_field_name("name").unwrap_or(scope);
    Some(node_text(scope, content).trim().to_string()).filter(|s| !s.is_empty())
}

/// The type a Dart member belongs to. A member is a `class_member` in the body
/// of its class, mixin, enum, extension or extension type, so the type is
/// three levels up.
pub(super) fn dart_member_owner(node: Node) -> Option<Node> {
    let member = node.parent().filter(|p| p.kind() == "class_member")?;
    let body = member.parent().filter(|p| p.kind().ends_with("_body"))?;
    body.parent()
}

/// The first named child of `node` of kind `kind`.
pub(super) fn named_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

pub(super) fn extract_graphql_name(node: Node, content: &str) -> Option<String> {
    // The grammar has no fields: a definition's name is a `name` child, and a
    // fragment's sits one level down, in `fragment_name`. An anonymous
    // operation (`{ … }`) has neither, so it yields no entity.
    let name = named_child_of_kind(node, "name").or_else(|| {
        named_child_of_kind(node, "fragment_name")
            .and_then(|fragment| named_child_of_kind(fragment, "name"))
    })?;
    let name_text = node_text(name, content).trim();
    (!name_text.is_empty()).then(|| name_text.to_string())
}

/// Look up a language-specific name extractor for the given language.
///
/// Falls back to a no-op when the language is not registered.
pub(super) fn extract_language_specific_name(
    node: Node,
    content: &str,
    language: &str,
) -> Option<String> {
    LANGUAGE_NAME_EXTRACTORS
        .get(language)
        .and_then(|extractor| extractor(node, content))
}
