use std::collections::HashMap;
use tree_sitter::Node;

/// Information about an extracted import
#[derive(Debug, Clone, PartialEq)]
pub struct ImportInfo {
    /// The local name used in the code
    pub local_name: String,
    /// The source module/path
    pub source_module: String,
    /// The original name (for aliased imports)
    pub original_name: Option<String>,
    /// Whether it brings every name the module exports in unqualified, as
    /// `from x import *` and `use x::*` do; its `local_name` is then `*`.
    /// `import * as x` binds one name, the module, and is none.
    pub is_wildcard: bool,
    /// The line it is named on, 1-based: in an import of several names over
    /// several lines, each name's own, where a language server reports it.
    /// 0 for one read back from an index that recorded no lines (before
    /// schema version 7), until its full rescan.
    pub line: u32,
}

/// The 1-based line `node` starts on.
fn line_of(node: Node) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}

/// Extract imports from a parsed AST.
///
/// `node` should be the tree root (e.g. `tree.root_node()`). The function
/// traverses the tree recursively and calls the per-language parser on each
/// node that looks like an import statement.
///
/// The root node is always `module`/`source_file`/`program`, never an import
/// statement, so every node of the tree is visited.
pub fn extract_imports(root: Node, content: &str, language: &str) -> Vec<ImportInfo> {
    let mut results = Vec::new();
    traverse_for_imports(root, content, language, &mut results, 0);
    results
}

/// Recursively traverse the AST and extract import nodes.
///
/// Depth is bounded at 50 to guard against pathological/malformed trees.
fn traverse_for_imports<'a>(
    node: Node<'a>,
    content: &str,
    language: &str,
    out: &mut Vec<ImportInfo>,
    depth: usize,
) {
    if depth > 50 {
        return;
    }

    // Try to parse this node as an import statement.
    let before = out.len();
    extract_single_node(node, content, language, out);
    let extracted = out.len() > before;

    // If we extracted something (or the node is definitely an import kind),
    // don't descend into its children — the per-language parsers already walk
    // the import node's subtree internally.
    if extracted || is_import_kind(node.kind(), language) {
        return;
    }

    // Recurse into children.
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            traverse_for_imports(cursor.node(), content, language, out, depth + 1);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Returns true when the node kind is an import statement for the given language.
fn is_import_kind(kind: &str, language: &str) -> bool {
    match language {
        "python" => matches!(kind, "import_statement" | "import_from_statement"),
        "javascript" | "typescript" | "tsx" => {
            matches!(kind, "import_statement" | "import_declaration")
        }
        "rust" => matches!(kind, "use_declaration"),
        "go" => kind == "import_declaration",
        "java" => matches!(kind, "import_declaration"),
        "kotlin" => matches!(kind, "import"),
        "solidity" => matches!(kind, "import_directive"),
        "c" | "cpp" | "c++" | "cuda" => matches!(kind, "preproc_include"),
        "objc" | "objective_c" => matches!(kind, "preproc_include" | "module_import"),
        "c_sharp" | "csharp" | "c#" => matches!(kind, "using_directive"),
        "php" => matches!(
            kind,
            "require_expression"
                | "require_once_expression"
                | "include_expression"
                | "include_once_expression"
                | "use_declaration"
        ),
        "ruby" => matches!(kind, "call" | "require"),
        "swift" => matches!(kind, "import_declaration"),
        "dart" => matches!(kind, "import_or_export"),
        _ => false,
    }
}

/// Try to extract import information from a single node; push to `out` if successful.
fn extract_single_node(node: Node, content: &str, language: &str, out: &mut Vec<ImportInfo>) {
    match language {
        "python" => {
            out.extend(parse_python_import(node, content));
        }
        "javascript" | "typescript" | "tsx" => {
            out.extend(parse_js_import(node, content));
        }
        "rust" => {
            out.extend(parse_rust_import(node, content));
        }
        "go" => {
            out.extend(parse_go_import(node, content));
        }
        "java" => {
            out.extend(parse_java_import(node, content));
        }
        "kotlin" => {
            if let Some(import) = parse_kotlin_import(node, content) {
                out.push(import);
            }
        }
        "solidity" => {
            out.extend(parse_solidity_import(node, content));
        }
        "c" | "cpp" | "c++" | "cuda" | "objc" | "objective_c" => {
            if let Some(import) = parse_c_import(node, content) {
                out.push(import);
            }
        }
        "c_sharp" | "csharp" | "c#" => {
            out.extend(parse_csharp_import(node, content));
        }
        "php" => {
            out.extend(parse_php_import(node, content));
        }
        "ruby" => {
            out.extend(parse_ruby_import(node, content));
        }
        "swift" => {
            out.extend(parse_swift_import(node, content));
        }
        "dart" => {
            out.extend(parse_dart_import(node, content));
        }
        _ => {}
    }
}

/// Parse a Kotlin `import`: `import a.b.C`, `import a.b.*` and
/// `import a.b.C as D`. Kotlin's grammar calls it `import`, not Java's
/// `import_declaration`.
fn parse_kotlin_import(node: Node, content: &str) -> Option<ImportInfo> {
    if node.kind() != "import" || !node.is_named() {
        return None;
    }
    let text = |part: Node| part.utf8_text(content.as_bytes()).ok().map(str::to_string);
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    let path = text(
        *children
            .iter()
            .find(|c| c.kind() == "qualified_identifier")?,
    )?;
    if children.iter().any(|c| c.kind() == "*") {
        return Some(ImportInfo {
            local_name: "*".to_string(),
            source_module: path,
            original_name: None,
            is_wildcard: true,
            line: line_of(node),
        });
    }
    let imported = path.rsplit('.').next().unwrap_or(&path).to_string();
    let alias = children
        .iter()
        .skip_while(|c| c.kind() != "as")
        .find(|c| c.kind() == "identifier")
        .and_then(|alias| text(*alias));
    Some(ImportInfo {
        local_name: alias.clone().unwrap_or_else(|| imported.clone()),
        source_module: path,
        original_name: alias.map(|_| imported),
        is_wildcard: false,
        line: line_of(node),
    })
}

/// Parse a Solidity `import`: `import "./A.sol";`, every name the file
/// declares; `import {B as C, D} from "./B.sol";`, the names given; and
/// `import * as E from "./E.sol";` or `import "./F.sol" as G;`, the file as
/// one name, as JavaScript's `import * as E` is.
fn parse_solidity_import(node: Node, content: &str) -> Vec<ImportInfo> {
    if node.kind() != "import_directive" {
        return Vec::new();
    }
    let text = |part: Node| {
        part.utf8_text(content.as_bytes())
            .unwrap_or_default()
            .to_string()
    };
    let Some(source) = node.child_by_field_name("source") else {
        return Vec::new();
    };
    let source = text(source).trim_matches(['"', '\'']).to_string();
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    if !children.iter().any(|child| child.kind() == "{") {
        let (local_name, is_wildcard) = match node.child_by_field_name("alias") {
            Some(alias) => (text(alias), true),
            None => ("*".to_string(), true),
        };
        return vec![ImportInfo {
            local_name,
            source_module: source,
            original_name: None,
            is_wildcard,
            line: line_of(node),
        }];
    }
    let mut imports = Vec::new();
    let mut i = 0;
    while i < children.len() {
        if children[i].kind() == "identifier" {
            let name = text(children[i]);
            let alias = (children.get(i + 1).map(Node::kind) == Some("as"))
                .then(|| children.get(i + 2))
                .flatten()
                .filter(|alias| alias.kind() == "identifier");
            imports.push(match alias {
                Some(alias) => {
                    i += 2;
                    ImportInfo {
                        local_name: text(*alias),
                        source_module: source.clone(),
                        original_name: Some(name),
                        is_wildcard: false,
                        line: line_of(*alias),
                    }
                }
                None => ImportInfo {
                    local_name: name,
                    source_module: source.clone(),
                    original_name: None,
                    is_wildcard: false,
                    line: line_of(children[i]),
                },
            });
        }
        i += 1;
    }
    imports
}

/// Parse a Python import: `import a.b` and `import a.b as c`, one per module
/// named; `from m import x, y as z`, one per name, from `m`; and
/// `from m import *`. A `from` import's names are `dotted_name`s as its
/// module is, so the module is the `module_name` field, not the first or last
/// of them, and `import os, sys` gives both.
fn parse_python_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let text = |part: Node| {
        part.utf8_text(content.as_bytes())
            .unwrap_or_default()
            .to_string()
    };
    // A name, the alias it is bound to, if any, and its line.
    let named = |part: Node| match part.kind() {
        "aliased_import" => Some((
            text(part.child_by_field_name("name")?),
            part.child_by_field_name("alias").map(text),
            line_of(part),
        )),
        "dotted_name" => Some((text(part), None, line_of(part))),
        _ => None,
    };
    let mut cursor = node.walk();
    match node.kind() {
        "import_from_statement" => {
            let module = node
                .child_by_field_name("module_name")
                .map(text)
                .unwrap_or_default();
            let mut children = node.walk();
            if node
                .children(&mut children)
                .any(|child| child.kind() == "wildcard_import")
            {
                return vec![ImportInfo {
                    local_name: "*".to_string(),
                    source_module: module,
                    original_name: None,
                    is_wildcard: true,
                    line: line_of(node),
                }];
            }
            node.children_by_field_name("name", &mut cursor)
                .filter_map(named)
                .map(|(name, alias, line)| ImportInfo {
                    local_name: alias.unwrap_or_else(|| name.clone()),
                    source_module: module.clone(),
                    original_name: Some(name),
                    is_wildcard: false,
                    line,
                })
                .collect()
        }
        "import_statement" => node
            .children_by_field_name("name", &mut cursor)
            .filter_map(named)
            .map(|(module, alias, line)| ImportInfo {
                local_name: alias.unwrap_or_else(|| module.clone()),
                source_module: module,
                original_name: None,
                is_wildcard: false,
                line,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse JavaScript/TypeScript imports — returns all named imports from a single statement
fn parse_js_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let node_type = node.kind();
    let mut results = Vec::new();

    if node_type == "import_statement" {
        // Get source module
        let Some(source) = node
            .child_by_field_name("source")
            .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        else {
            return results;
        };
        let source = source.trim_matches('"').trim_matches('\'');

        // The names it gives, from its clause — a child, not a field: a default
        // import (`import Cart from …`), named ones (`import { a, b as c } from …`,
        // whose `name` is the export and `alias` the local name) and a namespace
        // (`import * as ns from …`).
        let mut cursor = node.walk();
        let clause = node
            .children(&mut cursor)
            .find(|child| child.kind() == "import_clause");
        if let Some(clause) = clause {
            let text = |part: Node| part.utf8_text(content.as_bytes()).ok().map(str::to_string);
            let mut cursor = clause.walk();
            for part in clause.named_children(&mut cursor) {
                match part.kind() {
                    "identifier" => {
                        if let Some(name) = text(part) {
                            results.push(ImportInfo {
                                local_name: name.clone(),
                                source_module: source.to_string(),
                                original_name: Some(name),
                                is_wildcard: false,
                                line: line_of(part),
                            });
                        }
                    }
                    "named_imports" => {
                        let mut specifiers = part.walk();
                        for specifier in part.named_children(&mut specifiers) {
                            let Some(imported) =
                                specifier.child_by_field_name("name").and_then(text)
                            else {
                                continue;
                            };
                            let local = specifier
                                .child_by_field_name("alias")
                                .and_then(text)
                                .unwrap_or_else(|| imported.clone());
                            results.push(ImportInfo {
                                local_name: local,
                                source_module: source.to_string(),
                                original_name: Some(imported),
                                is_wildcard: false,
                                line: line_of(specifier),
                            });
                        }
                    }
                    "namespace_import" => {
                        let mut names = part.walk();
                        let name = part
                            .named_children(&mut names)
                            .find(|name| name.kind() == "identifier")
                            .and_then(text);
                        // `import * as api` binds one name, the module.
                        if let Some(name) = name {
                            results.push(ImportInfo {
                                local_name: name,
                                source_module: source.to_string(),
                                original_name: None,
                                is_wildcard: false,
                                line: line_of(part),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Module import without clause: import 'polyfill'
        if results.is_empty() {
            results.push(ImportInfo {
                local_name: source.split('/').next_back().unwrap_or(source).to_string(),
                source_module: source.to_string(),
                original_name: None,
                is_wildcard: false,
                line: line_of(node),
            });
        }

        return results;
    }

    // require() call
    if node_type == "call_expression"
        && let Some(func) = node.child_by_field_name("function")
        && func.utf8_text(content.as_bytes()).ok() == Some("require")
        && let Some(args) = node.child_by_field_name("arguments")
    {
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            if (child.kind() == "string" || child.kind() == "string_fragment")
                && let Ok(source) = child.utf8_text(content.as_bytes())
            {
                let source = source.trim_matches('"').trim_matches('\'');
                results.push(ImportInfo {
                    local_name: source.split('/').next_back().unwrap_or(source).to_string(),
                    source_module: source.to_string(),
                    original_name: None,
                    is_wildcard: false,
                    line: line_of(child),
                });
            }
        }
    }

    results
}

/// Parse a Rust `use`: each name it brings in, with the module it comes from
/// — `use a::b::c` (`c` from `a::b`), `use a::{b, c::d as e}` (`b` from `a`,
/// and `e`, which is `d`, from `a::c`), `use a::b::{self}` (`b` from `a`) and
/// `use a::*`, each name by itself: one import named by its whole path, `{…}`
/// lists and `as` clauses included, would be looked up by the resolver as a
/// name, and no `use`d function would resolve.
fn parse_rust_import(node: Node, content: &str) -> Vec<ImportInfo> {
    if !matches!(node.kind(), "use_declaration" | "use_statement") {
        return Vec::new();
    }
    let argument = node.child_by_field_name("argument").or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| child.kind() == "use_wildcard")
    });
    let mut imports = Vec::new();
    if let Some(argument) = argument {
        collect_rust_use(argument, "", content, &mut imports);
    }
    imports
}

/// The imports of one part of a Rust use tree, under `prefix`.
fn collect_rust_use(node: Node, prefix: &str, content: &str, out: &mut Vec<ImportInfo>) {
    let text = |part: Node| part.utf8_text(content.as_bytes()).unwrap_or_default();
    let join = |path: &str| {
        if prefix.is_empty() {
            path.to_string()
        } else {
            format!("{prefix}::{path}")
        }
    };
    match node.kind() {
        "scoped_identifier" | "identifier" | "crate" | "self" | "super" => {
            push_rust_item(&join(text(node)), None, line_of(node), out);
        }
        "use_as_clause" => {
            if let Some(path) = node.child_by_field_name("path") {
                let alias = node
                    .child_by_field_name("alias")
                    .map(|a| text(a).to_string());
                push_rust_item(&join(text(path)), alias, line_of(node), out);
            }
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map_or_else(|| prefix.to_string(), |path| join(text(path)));
            if let Some(list) = node.child_by_field_name("list") {
                collect_rust_use(list, &path, content, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for item in node.named_children(&mut cursor) {
                collect_rust_use(item, prefix, content, out);
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let module = node
                .named_children(&mut cursor)
                .next()
                .map_or_else(|| prefix.to_string(), |path| join(text(path)));
            out.push(ImportInfo {
                local_name: "*".to_string(),
                source_module: module,
                original_name: None,
                is_wildcard: true,
                line: line_of(node),
            });
        }
        _ => {}
    }
}

/// One Rust import of the item `path` names, on `line`, bound to `alias` or
/// its own name; `a::b::self` is `b`, from `a`.
fn push_rust_item(path: &str, alias: Option<String>, line: u32, out: &mut Vec<ImportInfo>) {
    let split = |path: &str| -> (String, String) {
        match path.rsplit_once("::") {
            Some((module, item)) => (module.to_string(), item.to_string()),
            None => (String::new(), path.to_string()),
        }
    };
    let (mut module, mut item) = split(path);
    if item == "self" {
        (module, item) = split(&module);
    }
    out.push(ImportInfo {
        local_name: alias.unwrap_or_else(|| item.clone()),
        source_module: module,
        original_name: Some(item),
        is_wildcard: false,
        line,
    });
}

/// Parse a Go `import` declaration: each of its specs, `import "fmt"` or
/// one of `import ( … )`, under the name the package is called by — its
/// alias (`h "net/http"`), else its package's name, the path's last segment
/// but for a major version (`github.com/acme/shop/v2` is `shop`,
/// `gopkg.in/yaml.v3` is `yaml`). A dot import brings every name in
/// unqualified; a blank one (`_ "github.com/lib/pq"`) none. A spec is a
/// child of the declaration or, in a group, of its `import_spec_list`.
fn parse_go_import(node: Node, content: &str) -> Vec<ImportInfo> {
    if node.kind() != "import_declaration" {
        return Vec::new();
    }
    let text = |part: Node| part.utf8_text(content.as_bytes()).ok();
    let mut specs = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => specs.push(child),
            "import_spec_list" => {
                let mut inner = child.walk();
                specs.extend(
                    child
                        .named_children(&mut inner)
                        .filter(|spec| spec.kind() == "import_spec"),
                );
            }
            _ => {}
        }
    }
    specs
        .into_iter()
        .filter_map(|spec| {
            let path = text(spec.child_by_field_name("path")?)?
                .trim_matches(|c| c == '"' || c == '`')
                .to_string();
            if path.is_empty() {
                return None;
            }
            let alias = spec.child_by_field_name("name");
            let (local_name, is_wildcard) = match alias.map(|alias| alias.kind()) {
                Some("dot") => ("*".to_string(), true),
                Some(_) => (text(alias?)?.to_string(), false),
                None => (go_package_name(&path).to_string(), false),
            };
            Some(ImportInfo {
                local_name,
                source_module: path.clone(),
                original_name: Some(path),
                is_wildcard,
                line: line_of(spec),
            })
        })
        .collect()
}

/// The name a Go package imported by `path` is called by, by the module
/// conventions: its last segment, without a major version — the segment
/// `v2` of `github.com/acme/shop/v2`, the suffix `.v3` of `gopkg.in/yaml.v3`.
pub(crate) fn go_package_name(path: &str) -> &str {
    let is_version = |text: &str| {
        text.strip_prefix('v')
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    };
    let mut segments = path.rsplit('/');
    let last = segments.next().unwrap_or(path);
    let last = if is_version(last) {
        segments.next().unwrap_or(last)
    } else {
        last
    };
    match last.rsplit_once('.') {
        Some((name, version)) if is_version(version) => name,
        _ => last,
    }
}

/// Parse Java/Kotlin imports
fn parse_java_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let node_type = node.kind();
    let mut results = Vec::new();

    if node_type == "import_declaration" {
        let mut cursor = node.walk();
        let mut path = String::new();
        let _is_static = false;
        let mut is_wildcard = false;

        for child in node.children(&mut cursor) {
            match child.kind() {
                "static" => {
                    // static import marker — no distinct handling needed
                }
                "asterisk" => {
                    is_wildcard = true;
                }
                "scoped_identifier" | "identifier" => {
                    path = child
                        .utf8_text(content.as_bytes())
                        .ok()
                        .unwrap_or_default()
                        .to_string();
                }
                _ => {}
            }
        }

        if !path.is_empty() {
            let local_name = if is_wildcard {
                "*".to_string()
            } else {
                path.split('.').next_back().unwrap_or(&path).to_string()
            };

            results.push(ImportInfo {
                local_name,
                source_module: path,
                original_name: None,
                is_wildcard,
                line: line_of(node),
            });
        }
    }

    results
}

/// Parse C/C++ includes
fn parse_c_import(node: Node, content: &str) -> Option<ImportInfo> {
    let node_type = node.kind();

    // Objective-C's `@import UIKit.UIView;` is named by its path, as an include
    // is: the `path` is the module's names and the dots between them.
    if node_type == "module_import" {
        let mut cursor = node.walk();
        let path: String = node
            .children_by_field_name("path", &mut cursor)
            .filter_map(|part| part.utf8_text(content.as_bytes()).ok())
            .collect();
        if path.is_empty() {
            return None;
        }
        return Some(ImportInfo {
            local_name: path.clone(),
            source_module: path,
            original_name: None,
            is_wildcard: false,
            line: line_of(node),
        });
    }

    if node_type == "preproc_include" || node_type == "include" {
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.kind() == "string_literal" || child.kind() == "system_lib_string" {
                let path = child.utf8_text(content.as_bytes()).ok()?.to_string();
                let clean_path = path.trim_matches('"').trim_matches('<').trim_matches('>');

                return Some(ImportInfo {
                    local_name: clean_path.to_string(),
                    source_module: clean_path.to_string(),
                    original_name: None,
                    is_wildcard: false,
                    line: line_of(node),
                });
            }
        }
    }

    None
}

/// Parse C# imports
fn parse_csharp_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let node_type = node.kind();
    let mut results = Vec::new();

    if node_type == "using_statement" || node_type == "using_directive" {
        let mut cursor = node.walk();
        let mut name = String::new();
        let mut alias = String::new();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" | "qualified_name" => {
                    if name.is_empty() {
                        name = child
                            .utf8_text(content.as_bytes())
                            .ok()
                            .unwrap_or_default()
                            .to_string();
                    } else {
                        alias = child
                            .utf8_text(content.as_bytes())
                            .ok()
                            .unwrap_or_default()
                            .to_string();
                    }
                }
                "name_equals" => {
                    // Alias assignment
                    let mut alias_cursor = child.walk();
                    for alias_child in child.children(&mut alias_cursor) {
                        if alias_child.kind() == "identifier" {
                            alias = alias_child
                                .utf8_text(content.as_bytes())
                                .ok()
                                .unwrap_or_default()
                                .to_string();
                        }
                    }
                }
                _ => {}
            }
        }

        if !name.is_empty() {
            results.push(ImportInfo {
                local_name: if alias.is_empty() {
                    name.split('.').next_back().unwrap_or(&name).to_string()
                } else {
                    alias
                },
                source_module: name.clone(),
                original_name: Some(name),
                is_wildcard: false,
                line: line_of(node),
            });
        }
    }

    results
}

/// Parse PHP imports: `use App\Http\Pagination;` and `use App\{A, B as C};`
/// (each name of the group, under its prefix), each bound to its alias or
/// its last name, as Kotlin's are; a trait's `use` in a class; and the file an
/// `include` or `require` names. The grammar keeps the names of a namespace
/// `use` in `namespace_use_clause`s, not among the declaration's own children,
/// and a `require`'s file in its child, not an `argument` field.
fn parse_php_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let text = |part: Node| {
        part.utf8_text(content.as_bytes())
            .unwrap_or_default()
            .to_string()
    };
    let mut results = Vec::new();
    match node.kind() {
        "namespace_use_declaration" => {
            let mut cursor = node.walk();
            let prefix = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "namespace_name")
                .map(text);
            let mut cursor = node.walk();
            let mut clauses: Vec<Node> = node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "namespace_use_clause")
                .collect();
            if let Some(group) = node.child_by_field_name("body") {
                let mut cursor = group.walk();
                clauses.extend(
                    group
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() == "namespace_use_clause"),
                );
            }
            for clause in clauses {
                let alias = clause.child_by_field_name("alias");
                let mut cursor = clause.walk();
                let Some(name) = clause.named_children(&mut cursor).find(|child| {
                    Some(*child) != alias && matches!(child.kind(), "qualified_name" | "name")
                }) else {
                    continue;
                };
                let path = match &prefix {
                    Some(prefix) => format!("{prefix}\\{}", text(name)),
                    None => text(name),
                };
                let imported = path.rsplit('\\').next().unwrap_or(&path).to_string();
                results.push(ImportInfo {
                    local_name: alias.map_or_else(|| imported.clone(), text),
                    original_name: alias.map(|_| imported),
                    source_module: path,
                    is_wildcard: false,
                    line: line_of(clause),
                });
            }
        }
        "use_declaration" => {
            let mut cursor = node.walk();
            let trait_name = node
                .children(&mut cursor)
                .filter(|child| matches!(child.kind(), "name" | "qualified_name"))
                .last()
                .map(text);
            if let Some(path) = trait_name {
                results.push(ImportInfo {
                    local_name: path.rsplit('\\').next().unwrap_or(&path).to_string(),
                    source_module: path.clone(),
                    original_name: Some(path),
                    is_wildcard: false,
                    line: line_of(node),
                });
            }
        }
        "include_expression"
        | "include_once_expression"
        | "require_expression"
        | "require_once_expression" => {
            let file = node
                .named_child(0)
                .filter(|child| matches!(child.kind(), "string" | "encapsed_string"));
            if let Some(file) = file {
                let file = text(file);
                let clean = file.trim_matches(['"', '\'']);
                results.push(ImportInfo {
                    local_name: clean.split('/').next_back().unwrap_or(clean).to_string(),
                    source_module: clean.to_string(),
                    original_name: Some(clean.to_string()),
                    is_wildcard: false,
                    line: line_of(node),
                });
            }
        }
        _ => {}
    }
    results
}

/// Parse Ruby imports (`require`, `require_relative`, `include`, `extend`)
fn parse_ruby_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let node_type = node.kind();
    let mut results = Vec::new();

    if node_type == "call" {
        let mut cursor = node.walk();
        let mut method_name = String::new();
        let mut arg_str = String::new();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" | "method_name" => {
                    method_name = child
                        .utf8_text(content.as_bytes())
                        .ok()
                        .unwrap_or_default()
                        .to_string();
                }
                "argument_list" => {
                    let mut arg_cursor = child.walk();
                    for arg_child in child.children(&mut arg_cursor) {
                        if (arg_child.kind() == "string" || arg_child.kind() == "string_content")
                            && let Ok(s) = arg_child.utf8_text(content.as_bytes())
                        {
                            arg_str = s.trim_matches('"').trim_matches('\'').to_string();
                        }
                    }
                }
                _ => {}
            }
        }

        if matches!(
            method_name.as_str(),
            "require" | "require_relative" | "load"
        ) && !arg_str.is_empty()
        {
            results.push(ImportInfo {
                local_name: arg_str
                    .split('/')
                    .next_back()
                    .unwrap_or(&arg_str)
                    .to_string(),
                source_module: arg_str.clone(),
                original_name: Some(arg_str),
                is_wildcard: false,
                line: line_of(node),
            });
        }
    }

    results
}

/// Parse Swift imports (`import` statements)
fn parse_swift_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let node_type = node.kind();
    let mut results = Vec::new();

    if node_type == "import_declaration" {
        let mut cursor = node.walk();
        let mut path = String::new();
        let alias = String::new();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" | "simple_identifier" | "dotted_name" => {
                    path = child
                        .utf8_text(content.as_bytes())
                        .ok()
                        .unwrap_or_default()
                        .to_string();
                }
                "type_identifier" => {
                    // Swift `import Module.Type` — keep the module part
                    let type_name = child
                        .utf8_text(content.as_bytes())
                        .ok()
                        .unwrap_or_default()
                        .to_string();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&type_name);
                }
                _ => {}
            }
        }

        if !path.is_empty() {
            results.push(ImportInfo {
                local_name: if alias.is_empty() {
                    path.split('.').next_back().unwrap_or(&path).to_string()
                } else {
                    alias
                },
                source_module: path.clone(),
                original_name: Some(path),
                is_wildcard: false,
                line: line_of(node),
            });
        }
    }

    results
}

/// Parse a Dart `import` and `part`: `import 'u.dart';` brings in every
/// public name of `u.dart` unqualified, `import 'u.dart' show a, b;` the names
/// it shows, and `import 'u.dart' as u;` binds `u`; `part 'p.dart';` makes
/// `p.dart` part of this library. The grammar keeps an import's URI and prefix
/// in an `import_specification` under the directive, not among the
/// directive's own children. An `export` binds nothing in its file, as a
/// JavaScript re-export does not.
fn parse_dart_import(node: Node, content: &str) -> Vec<ImportInfo> {
    let text = |part: Node| {
        part.utf8_text(content.as_bytes())
            .unwrap_or_default()
            .trim_matches(['"', '\''])
            .to_string()
    };
    let mut results = Vec::new();
    match node.kind() {
        "import_or_export" => {
            let mut cursor = node.walk();
            let specifications = node
                .named_children(&mut cursor)
                .filter(|directive| directive.kind() == "library_import")
                .filter_map(|import| {
                    let mut cursor = import.walk();
                    import
                        .named_children(&mut cursor)
                        .find(|part| part.kind() == "import_specification")
                });
            for specification in specifications {
                let Some(uri) = specification.child_by_field_name("uri").map(text) else {
                    continue;
                };
                if let Some(prefix) = specification.child_by_field_name("alias") {
                    results.push(ImportInfo {
                        local_name: text(prefix),
                        source_module: uri,
                        original_name: None,
                        is_wildcard: false,
                        line: line_of(prefix),
                    });
                    continue;
                }
                let mut cursor = specification.walk();
                let shown: Vec<Node> = specification
                    .named_children(&mut cursor)
                    .filter(|combinator| {
                        combinator.kind() == "combinator"
                            && text(*combinator).trim_start().starts_with("show")
                    })
                    .flat_map(|combinator| {
                        let mut cursor = combinator.walk();
                        combinator
                            .named_children(&mut cursor)
                            .filter(|name| name.kind() == "identifier")
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if shown.is_empty() {
                    results.push(ImportInfo {
                        local_name: "*".to_string(),
                        source_module: uri,
                        original_name: None,
                        is_wildcard: true,
                        line: line_of(specification),
                    });
                } else {
                    results.extend(shown.into_iter().map(|name| ImportInfo {
                        local_name: text(name),
                        source_module: uri.clone(),
                        original_name: Some(text(name)),
                        is_wildcard: false,
                        line: line_of(name),
                    }));
                }
            }
        }
        "part_directive" | "part_of_directive" => {
            if let Some(uri) = node.child_by_field_name("uri") {
                let clean = text(uri);
                results.push(ImportInfo {
                    local_name: clean.split('/').next_back().unwrap_or(&clean).to_string(),
                    source_module: clean.clone(),
                    original_name: Some(clean),
                    is_wildcard: false,
                    line: line_of(node),
                });
            }
        }
        _ => {}
    }
    results
}

/// Build a dependency graph from import information
pub fn build_import_graph(imports: &[ImportInfo]) -> HashMap<String, Vec<String>> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for import in imports {
        graph
            .entry(import.local_name.clone())
            .or_default()
            .push(import.source_module.clone());
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_info_creation() {
        let import = ImportInfo {
            local_name: "React".to_string(),
            source_module: "react".to_string(),
            original_name: Some("React".to_string()),
            is_wildcard: false,
            line: 1,
        };

        assert_eq!(import.local_name, "React");
        assert_eq!(import.source_module, "react");
        assert!(!import.is_wildcard);
    }

    #[test]
    fn test_import_graph_building() {
        let imports = vec![
            ImportInfo {
                local_name: "React".to_string(),
                source_module: "react".to_string(),
                original_name: None,
                is_wildcard: false,
                line: 1,
            },
            ImportInfo {
                local_name: "useState".to_string(),
                source_module: "react".to_string(),
                original_name: None,
                is_wildcard: false,
                line: 1,
            },
        ];

        let graph = build_import_graph(&imports);
        assert_eq!(graph.get("React").unwrap().len(), 1);
        assert_eq!(graph.get("useState").unwrap().len(), 1);
    }
}
