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

mod calls;
mod hierarchy;
mod imports;
mod nesting;
#[cfg(test)]
mod tests;
mod value_uses;

use calls::*;
use hierarchy::*;
pub(crate) use hierarchy::{
    Supertype, base_type_name, collect_supertypes, extended_kind, vb_heritage_statement,
};
use imports::*;
pub(crate) use nesting::generate_reverse_relationships;
use nesting::*;
use value_uses::*;

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
    let mut line_index = Extents::new(entities.iter().chain(&module));

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
        line_index = Extents::new(entities);
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

/// The entities' extents, sorted by where they start, linked so that the
/// innermost one holding a place is found without walking every extent
/// before it.
struct Extents {
    sorted: Vec<Extent>,
    /// For each extent, the nearest one before it that ends no earlier, if
    /// any: every extent between the two ends before it does. Where extents
    /// nest, as the nodes they come from do, it is the one around it.
    enclosing: Vec<Option<usize>>,
}

impl Extents {
    fn new<'e>(entities: impl IntoIterator<Item = &'e Entity>) -> Self {
        let mut sorted: Vec<Extent> = entities
            .into_iter()
            .map(|e| {
                let loc = e.location();
                (
                    (loc.start_line, loc.start_column),
                    (loc.end_line, loc.end_column),
                    e.id(),
                )
            })
            .collect();
        sorted.sort_by_key(|(start, _, _)| *start);
        Self::new_sorted(sorted)
    }

    /// The index of extents already sorted by where they start.
    fn new_sorted(sorted: Vec<Extent>) -> Self {
        let mut enclosing = Vec::with_capacity(sorted.len());
        // The extents that may yet enclose one, the latest last; each ends
        // no later than the one before it.
        let mut open: Vec<usize> = Vec::new();
        for (at, (_, end, _)) in sorted.iter().enumerate() {
            while open.last().is_some_and(|&last| sorted[last].1 < *end) {
                open.pop();
            }
            enclosing.push(open.last().copied());
            open.push(at);
        }
        Self { sorted, enclosing }
    }

    /// The innermost of the extents that `holds` accepts, all of which start
    /// at or before `from` and end at or after `reach`: of them, the one
    /// starting last, and of those starting there, the one ending first.
    /// Lines alone cannot tell two entities on one line apart: a call there
    /// would be the first's whichever it is in.
    fn innermost(
        &self,
        from: Position,
        reach: Position,
        holds: impl Fn(&Extent) -> bool,
    ) -> Option<Uuid> {
        let mut at = self
            .sorted
            .partition_point(|(start, _, _)| *start <= from)
            .checked_sub(1);
        // The last to start of those that hold. One that does not hold and
        // ends at or before `reach` is passed with every extent back to the
        // one enclosing it: they all end before it does, so none holds.
        let last = loop {
            let i = at?;
            let extent = &self.sorted[i];
            if holds(extent) {
                break i;
            }
            at = if extent.1 <= reach {
                self.enclosing[i]
            } else {
                i.checked_sub(1)
            };
        };
        let start = self.sorted[last].0;
        self.sorted[..=last]
            .iter()
            .rev()
            .take_while(|extent| extent.0 == start)
            .filter(|extent| holds(extent))
            .min_by_key(|extent| extent.1)
            .map(|(_, _, id)| *id)
    }
}

/// The entity a node is in: the innermost one whose extent holds where the
/// node starts.
fn find_containing_entity(node: Node, line_index: &Extents) -> Option<Uuid> {
    let at = (
        start_line(node),
        u32::try_from(node.start_position().column).unwrap_or(0),
    );
    line_index.innermost(at, at, |(start, end, _)| *start <= at && at < *end)
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
    line_index: &Extents,
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
    line_index: &Extents,
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
