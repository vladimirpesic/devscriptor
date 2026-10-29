//! Relationship extraction from tree-sitter parse trees
//!
//! This module implements a two-pass approach for extracting relationships:
//! 1. First pass: Extract entities and build spatial index (node position -> entity ID)
//! 2. Second pass: Traverse tree, find containing entity for each call, extract relationships

use crate::entities::{Entity, Relationship, RelationshipType, SymbolRef};
use crate::error::Result;
use crate::parser::cross_file::is_common_builtin;
use crate::parser::import_extraction::ImportInfo;
use crate::parser::language_config::{get_language_config, matches_node_type};
use crate::parser::symbol_table::{CallScope, names_module};
use std::path::Path;
use time::OffsetDateTime;
use tree_sitter::{Node, Tree};
use uuid::Uuid;

/// What a file's parse tree yields besides its entities.
#[derive(Debug, Default)]
pub struct Extraction {
    /// Relationships between the file's own entities: the references that
    /// resolve within the file, the nesting of members in their types, and
    /// the reverse of each.
    pub relationships: Vec<Relationship>,
    /// Every reference the file's entities make, resolved or not: calls,
    /// supertypes, the tables a foreign key names, and imported names. The
    /// types their signatures name come from the entities themselves
    /// ([`super::symbol_table::type_refs`]).
    pub refs: Vec<SymbolRef>,
    /// The module the file's code outside every entity is, when that code
    /// calls something other than its language's built-ins and no entity
    /// spans the file: the code at the top of a
    /// script, the callbacks a test file hands its test runner. Named after
    /// the file and spanning it, as a Python file's module is, it is the
    /// source of that code's references, which would otherwise have none.
    pub module: Option<Entity>,
}

/// Extract relationships between entities from a parse tree, and the
/// references they come from.
///
/// This function uses a two-pass approach:
/// 1. Build a spatial index mapping node byte ranges to entity IDs
/// 2. Traverse the tree, finding which entity contains each call site
///
/// # Arguments
///
/// * `tree` - The parse tree from tree-sitter
/// * `content` - The source code content
/// * `entities` - The entities extracted from the file
/// * `imports` - The file's imports, whose names a call's value may be
/// * `ambiguous_fanout` - How many of several same-named definitions in the
///   file a reference is linked to, earliest first (0 counts as 1). Each of
///   those edges is marked ambiguous.
/// * `language` - The language the tree is in
/// * `file_path` - The file's path, which names its module
///
/// Code outside every entity is the file's module's ([`Extraction::module`]):
/// when no entity spans the file, one is made for it, and kept if that code
/// calls something other than its language's built-ins; an import alone does
/// not make one.
///
/// # Errors
///
/// Returns an error if relationship extraction fails
pub fn extract_relationships(
    tree: &Tree,
    content: &str,
    entities: &[Entity],
    imports: &[ImportInfo],
    ambiguous_fanout: usize,
    language: &str,
    file_path: &str,
) -> Result<Extraction> {
    let mut out = Extraction::default();
    let config = get_language_config(language);
    let names = LocalNames::new(entities, imports, ambiguous_fanout, language, file_path);
    let module = provisional_module(tree, content, entities, language, file_path);
    let mut line_index = build_line_index(entities);
    if let Some(module) = &module {
        let location = module.location();
        line_index.push((
            (location.start_line, location.start_column),
            (location.end_line, location.end_column),
            module.id(),
        ));
        line_index.sort_by_key(|(start, _, _)| *start);
    }

    let mut cursor = tree.root_node().walk();

    // Pass 2: Traverse tree and extract relationships with proper source tracking
    traverse_and_extract_relationships(
        &mut cursor,
        content,
        language,
        config,
        &names,
        &line_index,
        &mut out,
        0,
    );

    // The module stays if its code calls something other than its language's
    // built-ins (`require`, `puts`, SQL's `COUNT`), which link nothing; what
    // an import alone gave it goes with it.
    let module = module.filter(|module| {
        let id = module.id();
        let calls = out.refs.iter().any(|r| {
            r.source_id == id
                && r.kind != RelationshipType::Uses
                && !is_common_builtin(&r.target_name, language)
        });
        if !calls {
            out.refs.retain(|r| r.source_id != id);
            out.relationships
                .retain(|r| r.source_id != id && r.target_id != id);
        }
        calls
    });
    if module.is_none() {
        line_index = build_line_index(entities);
    }
    record_value_uses(
        tree,
        content,
        language,
        config,
        &names,
        &line_index,
        &mut out,
    );
    let mut id_map = build_id_map(entities);
    if let Some(module) = &module {
        id_map.insert(module.id(), module.name());
    }

    // Extract hierarchical relationships (HasMethod / MethodOf) from entity
    // nesting: a kept module holds the file's top-level entities, as a Python
    // file's module does.
    let hierarchical = extract_hierarchical_relationships(entities, &line_index, &id_map);
    out.relationships.extend(hierarchical);

    // Generate reverse relationships for non-bidirectional types only.
    // HasMethod / MethodOf are already created as a bidirectional pair above,
    // so we skip them here to avoid duplicates.
    let reverse = generate_reverse_relationships(&out.relationships);
    out.relationships.extend(reverse);

    // Drop self-referential edges (source == target) at the source, so the
    // pipeline's validation never has to reject-and-warn about them. They arise
    // from recursive calls and from name collisions where a node resolves back to
    // its own containing entity.
    out.relationships.retain(|r| r.source_id != r.target_id);
    out.module = module;

    Ok(out)
}

/// The module that code outside every entity of the file would be the
/// source of ([`entity_extraction::file_module`]), unless an entity spans the
/// file already — a Python file's module, a Vue or Svelte component — which is
/// then that code's source.
///
/// [`entity_extraction::file_module`]: crate::parser::entity_extraction::file_module
fn provisional_module(
    tree: &Tree,
    content: &str,
    entities: &[Entity],
    language: &str,
    file_path: &str,
) -> Option<Entity> {
    let root = tree.root_node();
    let column = |column: usize| u32::try_from(column).unwrap_or(0);
    let start = (start_line(root), column(root.start_position().column));
    let end = (
        u32::try_from(root.end_position().row)
            .unwrap_or(0)
            .saturating_add(1),
        column(root.end_position().column),
    );
    let spans = |entity: &Entity| {
        let location = entity.location();
        (location.start_line, location.start_column) <= start
            && end <= (location.end_line, location.end_column)
    };
    if entities.iter().any(spans) {
        return None;
    }
    crate::parser::entity_extraction::file_module(
        root,
        content,
        language,
        file_path,
        OffsetDateTime::now_utc(),
    )
}

/// The file's entities by name, for resolving the names its code references.
struct LocalNames<'a> {
    /// Each name's definitions, earliest first; extraction order between
    /// definitions that start on the same line.
    by_name: std::collections::HashMap<&'a str, Vec<&'a Entity>>,
    /// The type definitions among them, in the same order.
    types_by_name: std::collections::HashMap<&'a str, Vec<&'a Entity>>,
    /// How many of several definitions a reference is linked to (at least 1).
    fanout: usize,
    /// Each entity by id: the source of a reference, whose type a `self` or
    /// `this` call names.
    by_id: std::collections::HashMap<Uuid, &'a Entity>,
    /// The file's language, which decides what a qualifier says.
    language: &'a str,
    /// The file's path, which a call's path may name as its module.
    file: &'a Path,
    /// The names the file's imports bind (`json`, `np`, `os` of `os.path`):
    /// a call through one is of another file's module.
    bound: std::collections::HashSet<&'a str>,
}

impl<'a> LocalNames<'a> {
    fn new(
        entities: &'a [Entity],
        imports: &'a [ImportInfo],
        fanout: usize,
        language: &'a str,
        file: &'a str,
    ) -> Self {
        let mut by_name: std::collections::HashMap<&str, Vec<&Entity>> =
            std::collections::HashMap::new();
        for entity in entities {
            by_name.entry(entity.name()).or_default().push(entity);
        }
        for definitions in by_name.values_mut() {
            definitions.sort_by_key(|entity| entity.location().start_line);
        }
        let types_by_name = by_name
            .iter()
            .filter_map(|(&name, definitions)| {
                let types: Vec<&Entity> = definitions
                    .iter()
                    .copied()
                    .filter(|entity| entity.is_type_definition())
                    .collect();
                (!types.is_empty()).then_some((name, types))
            })
            .collect();
        Self {
            by_name,
            types_by_name,
            fanout: fanout.max(1),
            by_id: entities
                .iter()
                .map(|entity| (entity.id(), entity))
                .collect(),
            language,
            file: Path::new(file),
            bound: imports
                .iter()
                .filter(|import| !import.is_wildcard)
                .filter_map(|import| import.local_name.split('.').next())
                .collect(),
        }
    }

    /// The definitions a reference to `name` is linked to, at most the
    /// fan-out, earliest first, with how many the file has. More than one
    /// makes each resulting edge ambiguous; picking the first would be
    /// arbitrary.
    fn candidates(&self, name: &str) -> (&[&'a Entity], usize) {
        self.capped(self.by_name.get(name).map_or(&[], Vec::as_slice))
    }

    /// The type the entity `id` is a member of.
    fn owner_of(&self, id: Uuid) -> Option<&'a str> {
        entity_owner(self.by_id.get(&id)?)
    }

    /// Whether the file defines `name` on `line`: a declaration whose name is
    /// in no field of its own (Kotlin's `const val MAX`, PHP's `const MAX`)
    /// is no use of it.
    fn defines_on(&self, name: &str, line: u32) -> bool {
        self.by_name.get(name).is_some_and(|definitions| {
            definitions
                .iter()
                .any(|entity| entity.location().start_line == line)
        })
    }

    /// [`Self::candidates`] for a call in `scope` (see [`CallScope`]): through
    /// a value named after a type, that type's members when the file has
    /// any, so `cart.total()` links `Cart`'s `total` and not `Order`'s too,
    /// and nothing else for a standard type's method (`items.len()`); through
    /// a value an import binds (`json.dumps`), nothing of this file; through a
    /// path, only what the path names — the type's members, or what is of no
    /// type when the file is the module the path names or holds one of its
    /// name — so `Vec::new()` links no `new` of the file.
    fn member_candidates(&self, name: &str, scope: CallScope<'_>) -> (Vec<&'a Entity>, usize) {
        let definitions = self.by_name.get(name).map_or(&[][..], Vec::as_slice);
        let owner = entity_owner;
        if scope.head().is_some_and(|head| self.bound.contains(head)) {
            return (Vec::new(), 0);
        }
        let members = || -> Vec<&'a Entity> {
            definitions
                .iter()
                .copied()
                .filter(|entity| scope.names_owner(owner(entity)))
                .collect()
        };
        let chosen: Vec<&'a Entity> = match scope {
            CallScope::Any => definitions.to_vec(),
            CallScope::Receiver { .. } => {
                let members = members();
                if members.is_empty() {
                    definitions.to_vec()
                } else {
                    members
                }
            }
            CallScope::Standard { .. } => members(),
            CallScope::Path(path) => {
                let module_here = names_module(self.file, path)
                    || self.by_name.get(path).is_some_and(|named| {
                        named
                            .iter()
                            .any(|entity| matches!(entity.entity_type(), "module" | "namespace"))
                    });
                definitions
                    .iter()
                    .copied()
                    .filter(|entity| {
                        scope.names_owner(owner(entity)) || (module_here && owner(entity).is_none())
                    })
                    .collect()
            }
        };
        let (linked, total) = self.capped(&chosen);
        (linked.to_vec(), total)
    }

    /// The file's constants and functions a use of `name` as a value in
    /// `scope` is of: unqualified, any of them; through a value or a path,
    /// only the members of the type it names — `Limits.MAX` is no other
    /// `MAX` of the file, and `o.size` none — or, through a path to this
    /// file's module, what is of no type; through what an import binds, none.
    fn value_candidates(&self, name: &str, scope: CallScope<'_>) -> (Vec<&'a Entity>, usize) {
        if scope.head().is_some_and(|head| self.bound.contains(head)) {
            return (Vec::new(), 0);
        }
        let module_here = scope.path().is_some_and(|path| {
            names_module(self.file, path)
                || self.by_name.get(path).is_some_and(|named| {
                    named
                        .iter()
                        .any(|entity| matches!(entity.entity_type(), "module" | "namespace"))
                })
        });
        let chosen: Vec<&'a Entity> = self
            .by_name
            .get(name)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .copied()
            .filter(|entity| matches!(entity, Entity::Function(_) | Entity::Constant(_)))
            .filter(|entity| {
                scope == CallScope::Any
                    || scope.names_owner(entity_owner(entity))
                    || (module_here && entity_owner(entity).is_none())
            })
            .collect();
        let (linked, total) = self.capped(&chosen);
        (linked.to_vec(), total)
    }

    /// [`Self::candidates`] for a name in a type position (a supertype, the
    /// type of an `impl`): only the type definitions of the name when it has
    /// any, so a constructor named after its class is not a candidate.
    fn type_candidates(&self, name: &str) -> (&[&'a Entity], usize) {
        match self.types_by_name.get(name) {
            Some(types) => self.capped(types),
            None => self.candidates(name),
        }
    }

    /// The first fan-out of `definitions`, and how many there are.
    fn capped<'m>(&self, definitions: &'m [&'a Entity]) -> (&'m [&'a Entity], usize) {
        let linked = definitions.len().min(self.fanout);
        (&definitions[..linked], definitions.len())
    }

    /// The definition of `name` whose lines hold `line` (the innermost when
    /// they nest): the entity a declaration on that line defines. Falls back
    /// to the earliest definition.
    fn defined_at(&self, name: &str, line: u32) -> Option<&'a Entity> {
        let definitions = self.by_name.get(name)?;
        definitions
            .iter()
            .filter(|entity| {
                let location = entity.location();
                location.start_line <= line && line <= location.end_line
            })
            .min_by_key(|entity| {
                let location = entity.location();
                location.end_line.saturating_sub(location.start_line)
            })
            .or_else(|| definitions.first())
            .copied()
    }
}

/// The type `entity` is a member of: a method's class, a constant's
/// (`ReeferLimits` of `ReeferLimits.MAX_CELSIUS`).
fn entity_owner(entity: &Entity) -> Option<&str> {
    match entity {
        Entity::Function(f) => f.parent_class.as_deref(),
        Entity::Constant(c) => c.parent_class.as_deref(),
        _ => None,
    }
}

/// The 1-based line `node` starts on.
fn start_line(node: Node) -> u32 {
    u32::try_from(node.start_position().row)
        .unwrap_or(0)
        .saturating_add(1)
}

/// Build a map for fast entity lookup by ID (O(1) instead of O(n))
fn build_id_map(entities: &[Entity]) -> std::collections::HashMap<Uuid, &str> {
    let mut map = std::collections::HashMap::new();
    for entity in entities {
        map.insert(entity.id(), entity.name());
    }
    map
}

/// A position in a file: its line (1-based) and column.
type Position = (u32, u32);

/// Where an entity starts and ends, and which it is.
type Extent = (Position, Position, Uuid);

/// The entities' extents, sorted by where they start.
fn build_line_index(entities: &[Entity]) -> Vec<Extent> {
    let mut index: Vec<Extent> = entities
        .iter()
        .map(|e| {
            let loc = e.location();
            (
                (loc.start_line, loc.start_column),
                (loc.end_line, loc.end_column),
                e.id(),
            )
        })
        .collect();
    index.sort_by_key(|(start, _, _)| *start);
    index
}

/// The innermost of the extents that `holds` accepts, all of which start at
/// or before `from`: of them, the one starting last, and of those starting
/// there, the one ending first. Lines alone cannot tell two entities on one
/// line apart: a call there would be the first's whichever it is in.
fn innermost_extent_holding(
    line_index: &[Extent],
    from: Position,
    holds: impl Fn(&Extent) -> bool,
) -> Option<Uuid> {
    let candidates = &line_index[..line_index.partition_point(|(start, _, _)| *start <= from)];
    let mut innermost: Option<&Extent> = None;
    for extent in candidates.iter().rev() {
        // What starts before the innermost holder found holds it, if anything.
        if innermost.is_some_and(|(start, _, _)| extent.0 < *start) {
            break;
        }
        if holds(extent) && innermost.is_none_or(|(_, end, _)| extent.1 < *end) {
            innermost = Some(extent);
        }
    }
    innermost.map(|(_, _, id)| *id)
}

/// The entity a node is in: the innermost one whose extent holds where the
/// node starts.
fn find_containing_entity(node: Node, line_index: &[Extent]) -> Option<Uuid> {
    let at = (
        start_line(node),
        u32::try_from(node.start_position().column).unwrap_or(0),
    );
    innermost_extent_holding(line_index, at, |(start, end, _)| *start <= at && at < *end)
}

/// Recursively traverse the tree and extract relationships
/// Maximum AST nesting depth for relationship traversal (mirrors entity extraction).
const MAX_TRAVERSE_DEPTH: usize = 128;

#[allow(clippy::too_many_arguments)]
fn traverse_and_extract_relationships(
    cursor: &mut tree_sitter::TreeCursor,
    content: &str,
    language: &str,
    config: &crate::parser::language_config::LanguageNodeConfig,
    names: &LocalNames,
    line_index: &[Extent],
    out: &mut Extraction,
    depth: usize,
) {
    if depth > MAX_TRAVERSE_DEPTH {
        return;
    }

    let node = cursor.node();

    // Extract relationships from this node
    try_extract_relationship(node, content, language, config, names, line_index, out);

    // Recursively process children
    if cursor.goto_first_child() {
        loop {
            traverse_and_extract_relationships(
                cursor,
                content,
                language,
                config,
                names,
                line_index,
                out,
                depth + 1,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Try to extract relationships from a node
fn try_extract_relationship(
    node: Node,
    content: &str,
    language: &str,
    config: &crate::parser::language_config::LanguageNodeConfig,
    names: &LocalNames,
    line_index: &[Extent],
    out: &mut Extraction,
) {
    let node_type = node.kind();

    // Check for function call relationships
    if matches_node_type(node_type, &config.call_nodes) {
        extract_call_relationships(node, content, language, names, line_index, out);
    }
    // A Rust macro's arguments are a token tree, not expressions: a call in
    // `format!("{}", total(x))` is a name followed by a parenthesized group.
    if language == "rust" && node_type == "token_tree" {
        extract_token_tree_calls(node, content, names, line_index, out);
    }

    // Inheritance (`inherits`) + interface conformance (`implements`) from
    // every type definition, across all grammars: an interface's or trait's
    // supertypes, an enum's interfaces and a struct's bases too.
    if [
        &config.class_nodes,
        &config.struct_nodes,
        &config.interface_nodes,
        &config.trait_nodes,
        &config.enum_nodes,
    ]
    .iter()
    .any(|kinds| matches_node_type(node_type, kinds))
        // `extend type Order implements Node` adds a supertype too.
        || (language == "graphql" && node_type.ends_with("_type_extension"))
    {
        extract_class_hierarchy_relationships(node, content, names, out);
    }
    // Rust `impl Trait for Type` is an `impl_item`, not a class node.
    if node_type == "impl_item" {
        extract_rust_impl_relationship(node, content, names, out);
    }
    // SQL foreign keys: `col ... REFERENCES other(...)` in a `create_table` is a
    // `uses` edge from this table to the referenced one.
    if language == "sql" && node_type == "create_table" {
        extract_sql_fk_relationships(node, content, names, out);
    }

    // Check for import/use relationships
    if matches_node_type(node_type, &config.import_nodes) {
        extract_import_relationship(node, content, language, names, line_index, out);
    }
}

/// The member and path kinds, by grammar, each with the fields its name may be
/// in, looked for in order: a kind with none (Kotlin's `navigation_expression`,
/// PHP's `class_constant_access_expression`, F#'s `long_identifier`) names its
/// last named child.
const MEMBER_ACCESSES: &[(&str, &[&str])] = &[
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
const PATH_ACCESSES: &[&str] = &[
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
const NAME_KINDS: &[&str] = &[
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
const DECLARING_FIELDS: &[&str] = &[
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
const LABEL_KINDS: &[&str] = &[
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
const NAMING_KINDS: &[&str] = &[
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
const SUPERTYPE_CLAUSES: &[&str] = &[
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
const VALUE_KEYWORDS: &[&str] = &[
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
struct ValueUse {
    name: String,
    qualifier: Option<String>,
    line: u32,
}

/// The names a file's code uses as values, and what [`value_use`] reads them
/// with: the names each function binds, read once per function.
struct ValueUses<'n, 'a> {
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
fn record_value_uses(
    tree: &Tree,
    content: &str,
    language: &str,
    config: &crate::parser::language_config::LanguageNodeConfig,
    names: &LocalNames,
    line_index: &[Extent],
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
fn link_value_use(
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
            && !field_of(node).is_some_and(|field| matches!(field, "value" | "default_value"));
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
fn member_name(node: Node) -> Option<Node> {
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
fn name_leaf(node: Node) -> Option<Node> {
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
fn qualifier_of(node: Node, name: Node, content: &str) -> Option<String> {
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
fn is_member_name(node: Node) -> bool {
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

/// The field `node` is in its parent, if any.
fn field_of<'t>(node: Node<'t>) -> Option<&'t str> {
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    let index = parent
        .children(&mut cursor)
        .position(|child| child == node)?;
    parent.field_name_for_child(u32::try_from(index).ok()?)
}

/// Whether `node` is bound by a pattern around it — a destructuring, a
/// match arm, F#'s `let MAX = 3` (`identifier_pattern`) — or named on the
/// left of F#'s function declaration. A constant matched against (C#'s
/// `constant_pattern`) is a use.
fn in_binding_pattern(node: Node) -> bool {
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
fn in_supertypes(node: Node) -> bool {
    /// The name, its wrappers, the clause.
    const MAX_ASCENT: usize = 3;
    let mut ancestor = node.parent();
    for _ in 0..MAX_ASCENT {
        let Some(around) = ancestor else {
            return false;
        };
        if SUPERTYPE_CLAUSES.contains(&around.kind()) || field_of(around) == Some("superclasses") {
            return true;
        }
        ancestor = around.parent();
    }
    false
}

/// Whether a node of `kind` is a loop binding the names on its left: Python's
/// `for x in`, a comprehension's, Java's `for (Item i : items)`, C++'s range
/// `for`, Go's `range`, Rust's `for x in`.
fn is_loop(kind: &str) -> bool {
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
fn unwrapped(node: Node) -> Node {
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
fn is_callee(node: Node, config: &crate::parser::language_config::LanguageNodeConfig) -> bool {
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
fn declares(node: Node) -> bool {
    let node = unwrapped(node);
    let (Some(parent), Some(field)) = (node.parent(), field_of(node)) else {
        return false;
    };
    let parent_kind = parent.kind();
    // An assignment's target, a loop's variable (the shell's `for v in`).
    if matches!(field, "left" | "variable" | "item") {
        let binds = parent_kind.contains("assignment")
            || parent_kind.contains("declaration")
            || is_loop(parent_kind);
        return binds && member_name(node).is_none();
    }
    // An annotation's argument name (Java's `@Size(max = …)`); a dictionary's
    // key (a `pair`'s) may be a constant.
    if field == "key" {
        return parent_kind != "pair";
    }
    DECLARING_FIELDS.contains(&field)
}

/// Whether `node` is in a type: its parent or grandparent is one
/// (`user_type`, `type_annotation`, `type_arguments`), which a signature's
/// types are recorded from.
fn in_type_position(node: Node) -> bool {
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

/// Record the call a call expression makes from its containing entity, and
/// link it to each definition of the called name in this file (see
/// [`LocalNames::candidates`]).
fn extract_call_relationships(
    node: Node,
    content: &str,
    language: &str,
    names: &LocalNames,
    line_index: &[Extent],
    out: &mut Extraction,
) {
    // Find the containing entity (source of the call)
    let Some(source_id) = find_containing_entity(node, line_index) else {
        return; // No containing entity found
    };

    // Try to find the function name being called
    let called_name = if language == "graphql" {
        graphql_called_name(node, content)
    } else {
        extract_called_function_name(node, content)
    };
    if called_name.is_empty() {
        return;
    }
    let qualifier = call_receiver(node, content);
    record_call(node, source_id, called_name, qualifier, names, out);
}

/// The fields of a declaration that hold what it is given rather than what it
/// binds: a value, a default, a type, a body.
const VALUE_FIELDS: &[&str] = &[
    "value",
    "right",
    "default_value",
    "initializer",
    "type",
    "body",
    "condition",
    "arguments",
    "function",
];

/// The names a function binds itself: its parameters and those of the
/// closures in it, and what it declares, assigns, loops over, catches or
/// matches in its body. Used as a value, one is a variable of its own, not a
/// constant or function anywhere.
fn locally_bound_names(function: Node, content: &str) -> std::collections::HashSet<String> {
    fn identifiers(node: Node, content: &str, out: &mut std::collections::HashSet<String>) {
        // A member assigned (`self.total = 0`) is written, not bound.
        if member_name(node).is_some() {
            return;
        }
        let kind = node.kind();
        if kind.ends_with("identifier")
            || kind.ends_with("identifier_pattern")
            || kind == "variable_name"
        {
            let name = node_text(node, content).trim().trim_start_matches('$');
            if !name.is_empty() {
                out.insert(name.to_string());
            }
            return;
        }
        // What a declaration is given — its value, type or body — binds
        // nothing: `int x = Limits.MAX + MAX` binds `x` alone.
        let mut cursor = node.walk();
        for (index, child) in node.children(&mut cursor).enumerate() {
            let field = u32::try_from(index)
                .ok()
                .and_then(|index| node.field_name_for_child(index));
            if child.is_named() && !field.is_some_and(|field| VALUE_FIELDS.contains(&field)) {
                identifiers(child, content, out);
            }
        }
    }
    /// Kotlin's `variable_declaration` names its variable in no field: its
    /// first name that is no type.
    fn declared_name(node: Node) -> Option<Node> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|child| {
            matches!(child.kind(), "identifier" | "simple_identifier") && field_of(*child).is_none()
        })
    }
    /// Kotlin's declarations name their variable in a child, not a field.
    fn declared_variable(node: Node) -> Option<Node> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "variable_declaration" | "multi_variable_declaration"
            )
        })
    }
    fn declarations(node: Node, content: &str, out: &mut std::collections::HashSet<String>) {
        let kind = node.kind();
        let bound = match kind {
            "variable_declarator"
            | "init_declarator"
            | "let_declaration"
            | "const_declaration"
            | "var_declaration"
            | "property_declaration"
            | "local_variable_declaration"
            | "variable_assignment"
            | "variable_declaration"
            | "initialized_variable_definition"
            | "dim_statement"
            | "catch_declaration" => node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("pattern"))
                .or_else(|| node.child_by_field_name("declarator"))
                .or_else(|| declared_variable(node))
                .or_else(|| declared_name(node)),
            "short_var_declaration" | "assignment" | "assignment_expression" | "var_spec" => node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("name")),
            "as_pattern" => node.child_by_field_name("alias"),
            // F#'s `let x = …` and `let f o = …` inside a function.
            "value_declaration_left" | "function_declaration_left" => Some(node),
            _ if is_loop(kind) => node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("item"))
                .or_else(|| node.child_by_field_name("variable"))
                .or_else(|| node.child_by_field_name("name"))
                .or_else(|| node.child_by_field_name("declarator"))
                .or_else(|| declared_variable(node)),
            _ => None,
        };
        if let Some(bound) = bound {
            identifiers(bound, content, out);
        }
        // A pattern binds what it names (`if let Some(x)`, a match arm, a
        // destructuring); a closure's, a catch's or a lambda's parameters
        // are its own.
        if let Some(pattern) = node.child_by_field_name("pattern") {
            identifiers(pattern, content, out);
        }
        if kind.contains("parameter") || kind == "argument_patterns" {
            identifiers(node, content, out);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            declarations(child, content, out);
        }
    }
    let mut names = std::collections::HashSet::new();
    let mut cursor = function.walk();
    for part in function.named_children(&mut cursor) {
        if part.kind().contains("parameter") {
            identifiers(part, content, &mut names);
        }
    }
    for field in ["parameters", "parameter"] {
        if let Some(parameters) = function.child_by_field_name(field) {
            identifiers(parameters, content, &mut names);
        }
    }
    declarations(function, content, &mut names);
    names
}

/// The calls written in a Rust token tree (a macro's arguments): a name
/// followed by a parenthesized group, its qualifier the path segment or
/// receiver before it (`Money` in `Money::zero()`, `v` in `v.len()`). A macro
/// invoked inside (`vec![…]`) is no call here: its own token tree is read in
/// turn. A function defined inside (`fn helper()`) is no call either.
fn extract_token_tree_calls(
    node: Node,
    content: &str,
    names: &LocalNames,
    line_index: &[Extent],
    out: &mut Extraction,
) {
    let mut cursor = node.walk();
    let tokens: Vec<Node> = node.children(&mut cursor).collect();
    for (i, token) in tokens.iter().enumerate() {
        let is_call = token.kind() == "identifier"
            && tokens.get(i + 1).is_some_and(|next| {
                next.kind() == "token_tree" && node_text(*next, content).starts_with('(')
            })
            && !(i > 0 && tokens[i - 1].kind() == "fn");
        if !is_call {
            continue;
        }
        let Some(source_id) = find_containing_entity(*token, line_index) else {
            continue;
        };
        // `Money::new(…)`'s path keeps its `::`, as [`call_receiver`]'s does.
        let qualifier = (i >= 2 && matches!(tokens[i - 1].kind(), "." | "::"))
            .then(|| tokens[i - 2])
            .filter(|before| matches!(before.kind(), "identifier" | "self"))
            .map(|before| {
                let separator = if tokens[i - 1].kind() == "::" {
                    "::"
                } else {
                    ""
                };
                format!("{}{separator}", node_text(before, content))
            });
        let name = node_text(*token, content).to_string();
        record_call(*token, source_id, name, qualifier, names, out);
    }
}

/// Record the call `node` makes from `source_id` to `called_name`, and link
/// it to each definition of the name in this file.
fn record_call(
    node: Node,
    source_id: Uuid,
    called_name: String,
    qualifier: Option<String>,
    names: &LocalNames,
    out: &mut Extraction,
) {
    // A qualified call is of a member of the type its qualifier names, and
    // one through a path of nothing else.
    let scope = CallScope::of(
        qualifier.as_deref(),
        names.owner_of(source_id),
        &called_name,
        names.language,
    );
    let (targets, candidates) = names.member_candidates(&called_name, scope);
    out.refs.push(SymbolRef {
        source_id,
        kind: RelationshipType::Calls,
        target_name: called_name.clone(),
        qualifier,
        line: start_line(node),
    });

    let now = OffsetDateTime::now_utc();
    let source_location = format!(
        "{}:{}-{}:{}",
        node.start_position().row + 1,
        node.start_position().column,
        node.end_position().row + 1,
        node.end_position().column
    );
    out.relationships.extend(targets.iter().map(|target| {
        let mut relationship = Relationship {
            id: Uuid::new_v4(),
            source_id,
            target_id: target.id(),
            relationship_type: RelationshipType::Calls,
            metadata: Some(serde_json::json!({
                "called_name": called_name,
                "source_location": source_location,
            })),
            created_at: now,
        };
        relationship.mark_ambiguous(candidates);
        relationship
    }));
}

/// What a method call is made on, when it is a short expression on one line:
/// `orders` in `orders.total()`; a path ends with its `::` — `Money::` in
/// `Money::new()`, `std::` in C++'s `std::abs(x)`, `Tax::` in PHP's
/// `Tax::rate()` — as it names a type or a module, never a value (see
/// [`CallScope`]).
fn call_receiver(node: Node, content: &str) -> Option<String> {
    /// Longer receivers are chained calls or literals, not a name.
    const MAX_LEN: usize = 64;
    let function = node.child_by_field_name("function");
    [function, Some(node)]
        .into_iter()
        .flatten()
        .find_map(|callee| {
            ["object", "value", "operand", "path", "scope"]
                .iter()
                .find_map(|&field| Some((field, callee.child_by_field_name(field)?)))
        })
        .map(|(field, receiver)| {
            let text = node_text(receiver, content).trim();
            if matches!(field, "path" | "scope") {
                format!("{text}::")
            } else {
                text.to_string()
            }
        })
        .filter(|text| !text.is_empty() && text.len() <= MAX_LEN && !text.contains('\n'))
}

/// The name a GraphQL selection (`order(id: 1)`, `mine: order`) or directive
/// use (`@include(if: true)`) refers to: its `name` child. The first child is
/// an alias or the `@` token, which the generic rule would take.
fn graphql_called_name(node: Node, content: &str) -> String {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == "name")
        .map(|name| node_text(name, content).trim().to_string())
        .unwrap_or_default()
}

/// The name a call expression calls: the last name of its callee, so that
/// `calc.add(1)`, `pkg.Func()`, `Money::new()` and `self.repo.save()` call
/// `add`, `Func`, `new` and `save`. A call that names its method itself
/// (Java `method_invocation`, PHP member calls: `name`; Ruby `call`:
/// `method`) is read there; otherwise the callee expression is.
fn extract_called_function_name(node: Node, content: &str) -> String {
    ["name", "method", "function", "constructor", "macro"]
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| node.named_child(0))
        .map(|callee| callee_name(callee, content))
        .unwrap_or_default()
}

/// The last name in a callee expression: down its name-like fields (a
/// member's `property`, `attribute`, `field` or `name`), else its last named
/// child (Kotlin and Swift navigation suffixes), to an identifier; failing
/// that, the last identifier in its text before any type arguments.
fn callee_name(node: Node, content: &str) -> String {
    /// Deep enough for any callee; bounds a pathological tree.
    const MAX_DESCENT: usize = 16;
    let mut node = node;
    for _ in 0..MAX_DESCENT {
        if node.named_child_count() == 0 {
            break;
        }
        let next = [
            "property",
            "attribute",
            "field",
            "name",
            "method",
            "function",
        ]
        .iter()
        .find_map(|field| node.child_by_field_name(field))
        .or_else(|| {
            u32::try_from(node.named_child_count() - 1)
                .ok()
                .and_then(|last| node.named_child(last))
        });
        match next {
            Some(next) => node = next,
            None => break,
        }
    }
    // A name is the whole of its leaf: Ruby's `same_day?` and `save!` lost
    // their `?` and `!` to the split below, JavaScript's `#total` its `#` and
    // F#'s `f'` its prime, and called nothing named so.
    if node.named_child_count() == 0 && node.kind().ends_with("identifier") {
        let name = node_text(node, content).trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let text = node_text(node, content);
    let text = text.split(['<', '(']).next().unwrap_or(text);
    text.rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        })
        .unwrap_or_default()
        .to_string()
}

/// Extract identifier name from a node
fn extract_identifier_name(node: Node, content: &str) -> String {
    let node_type = node.kind();

    // Direct identifier
    if node_type == "identifier" || node_type == "IDENTIFIER" {
        return node_text(node, content).to_string();
    }

    // Try child with name field
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, content).to_string();
    }

    // For member expressions (e.g., obj.method), get the method name
    if node_type == "member_expression" || node_type == "attribute" {
        if let Some(prop_node) = node.child_by_field_name("property") {
            return extract_identifier_name(prop_node, content);
        }
        if let Some(attr_node) = node.child_by_field_name("attribute") {
            return node_text(attr_node, content).to_string();
        }
    }

    // Default: return the node text
    node_text(node, content).to_string()
}

/// Extract node text with bounds checking
fn node_text<'a>(node: Node, content: &'a str) -> &'a str {
    let start = node.start_byte();
    let end = node.end_byte();
    if start <= end && end <= content.len() {
        &content[start..end]
    } else {
        ""
    }
}

/// Extract inheritance (`inherits`) and interface-conformance (`implements`)
/// relationships from a class/struct/interface node, across all supported
/// grammars. Whether an edge is `Inherits` or `Implements` is decided by
/// resolving the supertype entity: an interface/trait/protocol parent yields
/// `Implements`, anything else yields `Inherits`. This keeps Kotlin/Swift `:`
/// (which serve both roles) correct without grammar-specific guessing. A type
/// in an `implements` list is implemented whatever its kind: Dart and
/// TypeScript let a class implement a class.
fn extract_class_hierarchy_relationships(
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
fn extract_rust_impl_relationship(
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
fn extract_sql_fk_relationships(
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
fn sql_object_ref_name(node: Node, content: &str) -> Option<String> {
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
fn build_uses_rels(source: &Entity, from: &str, to: &str, names: &LocalNames) -> Vec<Relationship> {
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
fn is_linked_supertype(child_name: &str, parent_name: &str) -> bool {
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
fn supertype_kind(implemented: bool) -> RelationshipType {
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
fn build_hierarchy_rels(
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
fn push_fsharp_supertype(clause: Node, content: &str, implemented: bool, out: &mut Vec<Supertype>) {
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
fn push_supertypes(clause: Node, content: &str, implemented: bool, out: &mut Vec<Supertype>) {
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
fn collect_type_names(node: Node, content: &str, out: &mut Vec<String>) {
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

/// Record each name an import statement brings into its containing entity as
/// a `uses` reference, and link it to the definitions of it in this file, its
/// type definitions when it has any (see [`LocalNames::type_candidates`]).
fn extract_import_relationship(
    node: Node,
    content: &str,
    language: &str,
    names: &LocalNames,
    line_index: &[Extent],
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

/// Extract hierarchical relationships (HasMethod / MethodOf) between entities
/// that are nested within each other (e.g., methods inside a class).
fn extract_hierarchical_relationships(
    entities: &[Entity],
    line_index: &[Extent],
    id_map: &std::collections::HashMap<Uuid, &str>,
) -> Vec<Relationship> {
    let mut relationships = Vec::new();
    let now = OffsetDateTime::now_utc();

    // For each entity, check if it is contained within a larger entity
    for entity in entities {
        let loc = entity.location();
        let entity_id = entity.id();

        // Skip if entity spans the whole file or is invalid
        if loc.start_line == 0 || loc.end_line < loc.start_line {
            continue;
        }

        // The innermost entity whose extent holds this one's and is larger: two
        // entities on the same lines would each hold the other by lines alone, so
        // a field and its one-line type would each be the other's member.
        let (start, end) = (
            (loc.start_line, loc.start_column),
            (loc.end_line, loc.end_column),
        );
        let parent_id = innermost_extent_holding(line_index, start, |(c_start, c_end, _)| {
            *c_start <= start && end <= *c_end && (*c_start, *c_end) != (start, end)
        });

        if let Some(parent) = parent_id {
            let parent_name = id_map.get(&parent).copied().unwrap_or("");
            // Create HasMethod: parent -> child
            relationships.push(Relationship {
                id: Uuid::new_v4(),
                source_id: parent,
                target_id: entity_id,
                relationship_type: RelationshipType::HasMethod,
                metadata: Some(serde_json::json!({
                    "container_name": parent_name,
                    "contained_name": entity.name(),
                    "contained_type": entity.entity_type(),
                })),
                created_at: now,
            });

            // Create MethodOf: child -> parent
            relationships.push(Relationship {
                id: Uuid::new_v4(),
                source_id: entity_id,
                target_id: parent,
                relationship_type: RelationshipType::MethodOf,
                metadata: Some(serde_json::json!({
                    "member_name": entity.name(),
                    "parent_name": parent_name,
                })),
                created_at: now,
            });
        }
    }

    relationships
}

// =============================================================================
// REVERSE RELATIONSHIPS
// =============================================================================

/// Generate reverse relationships for every forward relationship.
///
/// Forward -> Reverse mapping:
///   Calls       -> CalledBy
///   Inherits    -> InheritedBy
///   Implements  -> ImplementedBy
///   Uses        -> UsedBy
///   References  -> ReferencedBy
///   Overrides   -> OverriddenBy
///
/// NOTE: HasMethod / MethodOf are intentionally skipped here because
/// `extract_hierarchical_relationships()` already creates them as a
/// bidirectional pair, so reversing them again would produce duplicates.
pub(crate) fn generate_reverse_relationships(relationships: &[Relationship]) -> Vec<Relationship> {
    let mut reverse = Vec::with_capacity(relationships.len());
    let now = OffsetDateTime::now_utc();

    for rel in relationships {
        if let Some(reverse_type) = reverse_of(&rel.relationship_type) {
            reverse.push(Relationship {
                id: Uuid::new_v4(),
                source_id: rel.target_id,
                target_id: rel.source_id,
                relationship_type: reverse_type,
                metadata: rel.metadata.clone(),
                created_at: now,
            });
        }
    }

    reverse
}

/// Map a forward relationship type to its reverse.
///
/// HasMethod and MethodOf are intentionally excluded from this mapping
/// because they are already created as a bidirectional pair by
/// `extract_hierarchical_relationships()`, so reversing them again
/// would produce duplicate relationships.
pub(crate) fn reverse_of(rel_type: &RelationshipType) -> Option<RelationshipType> {
    match rel_type {
        RelationshipType::Calls => Some(RelationshipType::CalledBy),
        RelationshipType::Inherits => Some(RelationshipType::InheritedBy),
        RelationshipType::Implements => Some(RelationshipType::ImplementedBy),
        RelationshipType::Uses => Some(RelationshipType::UsedBy),
        RelationshipType::References => Some(RelationshipType::ReferencedBy),
        RelationshipType::Overrides => Some(RelationshipType::OverriddenBy),
        // HasMethod / MethodOf are already bidirectional — don't reverse again
        RelationshipType::HasMethod | RelationshipType::MethodOf => None,
        // Reverse types themselves don't get another reverse
        RelationshipType::CalledBy
        | RelationshipType::InheritedBy
        | RelationshipType::ImplementedBy
        | RelationshipType::UsedBy
        | RelationshipType::ReferencedBy
        | RelationshipType::OverriddenBy
        | RelationshipType::RelatedTo => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forward kind and its reverse sit on opposite sides of `is_reverse`,
    /// which tells a file's own edges from the edges other files have into it
    /// when the edges of a re-parsed file are replaced.
    #[test]
    fn every_reverse_pair_is_classified_by_is_reverse() {
        let mut pairs = 0;
        for kind in RelationshipType::ALL {
            if let Some(reverse) = reverse_of(&kind) {
                assert!(!kind.is_reverse(), "{kind:?} is a forward kind");
                assert!(
                    reverse.is_reverse(),
                    "{reverse:?} is the reverse of {kind:?}"
                );
                pairs += 1;
            }
        }
        assert_eq!(pairs, 6, "every reverse kind has its forward kind");
        assert_eq!(
            RelationshipType::ALL
                .iter()
                .filter(|k| k.is_reverse())
                .count(),
            pairs
        );
        // HasMethod and MethodOf are created as a pair from one file's
        // nesting; both are that file's own edges.
        assert!(!RelationshipType::HasMethod.is_reverse());
        assert!(!RelationshipType::MethodOf.is_reverse());
    }

    /// `ALL` lists each kind once, and each round-trips through its string.
    #[test]
    fn all_lists_each_kind_once() {
        let unique: std::collections::HashSet<_> = RelationshipType::ALL.iter().collect();
        assert_eq!(unique.len(), RelationshipType::ALL.len());
        for kind in RelationshipType::ALL {
            assert_eq!(RelationshipType::parse(kind.as_str()).unwrap(), kind);
        }
    }
}

#[cfg(all(test, feature = "lang-javascript"))]
mod module_tests {
    use super::*;
    use crate::parser::entity_extraction::extract_entities;

    /// The edges of `src`, read as JavaScript at `path`, and the file's
    /// entities with its module, if one is kept.
    fn extracted(src: &str, path: &str) -> (Vec<Entity>, Extraction) {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .expect("the JavaScript grammar");
        let tree = parser.parse(src, None).expect("a tree");
        let entities = extract_entities(&tree, src, "javascript", path).expect("entities");
        let extraction = extract_relationships(&tree, src, &entities, &[], 1, "javascript", path)
            .expect("edges");
        (entities, extraction)
    }

    /// Every edge joins the file's entities, or the module kept for its code:
    /// a module made for the file and not kept — its code only imports — is
    /// no member's container either.
    #[test]
    fn every_edge_joins_entities_the_file_keeps() {
        for (src, kept) in [
            (
                "import { BASE } from './base.js';\nfunction fetchOrders() {\n  return get(BASE);\n}\n",
                false,
            ),
            ("function main() {\n  return 0;\n}\nmain();\n", true),
        ] {
            let (entities, extraction) = extracted(src, "src/orders.js");
            assert_eq!(extraction.module.is_some(), kept, "{src}");
            let ids: Vec<Uuid> = entities
                .iter()
                .chain(&extraction.module)
                .map(Entity::id)
                .collect();
            for edge in &extraction.relationships {
                assert!(
                    ids.contains(&edge.source_id) && ids.contains(&edge.target_id),
                    "{src}: {edge:?}"
                );
            }
        }
    }
}
