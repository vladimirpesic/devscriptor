//! Edges derived from stored references, resolved from the database alone.
//!
//! A reference (`symbol_refs`) names what its source entity calls, uses or
//! extends. Resolving it against the entities of that name and the imports of
//! the source's file (`file_imports`) yields its forward edges, and each forward
//! edge brings its reverse row. An edge records the lines of the references it
//! was resolved from (`"lines"`): a caller that calls a function twice makes
//! one `calls` edge, on both lines. Deriving the edges of a set of sources
//! again replaces exactly the edges derived from them, whatever else changed in
//! the index since they were made.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use uuid::Uuid;

use crate::database::{Database, StoredRef};
use crate::entities::{EntityId, Relationship, RelationshipType, SymbolRef};
use crate::error::Result;
use crate::parser::cross_file::is_common_builtin;
use crate::parser::relationship_extraction::generate_reverse_relationships;
use crate::parser::symbol_table::{CallScope, SymbolTable};

/// How many sources [`rebuild_edges_for_sources`] derives per transaction.
const CHUNK: usize = 4096;

/// How many references [`rebuild_edges_for_sources`] resolves before it
/// writes their edges: the edges of a group are held in memory until written.
const REFS_PER_WRITE: usize = 32_768;

/// How many definitions [`rebuild_edges_for_sources`] keeps from one chunk to
/// the next before it starts over. The chunks of one project name many of the
/// same definitions (`build`, `new`, `toString`, …), which would otherwise be
/// read again for every chunk; this bounds what is kept.
const KEPT_DEFINITIONS: usize = 500_000;

/// The entity a reference is made in, as resolution sees it.
#[derive(Debug, Clone, Copy)]
pub struct RefSource<'a> {
    /// The entity's id.
    pub id: EntityId,
    /// Its name, which the edges to its supertypes record (`child`); not
    /// needed for any other reference.
    pub name: Option<&'a str>,
    /// Its kind (`class`, `interface`, …), which decides whether a supertype
    /// it names is one it extends or one it implements; not needed for any
    /// other reference.
    pub kind: Option<&'a str>,
    /// The file it is defined in, as stored.
    pub file_path: &'a Path,
    /// That file's language.
    pub language: &'a str,
    /// The type it is a member of, which a `self` or `this` call names.
    pub owner: Option<&'a str>,
}

/// The forward relationships `reference`, made in `source`, resolves to in
/// `table`: one per definition ranked best, at most `fanout`, each on the
/// reference's line (`"lines"`) and marked ambiguous when several tie (see
/// [`SymbolTable::resolve_symbol`]).
///
/// A call resolves across files only when `cross_file_calls`; otherwise, and
/// when it calls a common built-in name (`print`, `len`, `log`, …) the file
/// does not import, it links only a definition in the caller's own file. Type uses and supertypes
/// prefer type definitions; a supertype named where a class extends
/// (`Inherits`) yields `Implements` when it resolves to an interface or trait
/// and the class is none ([`extended_kind`]). A reference never links its own
/// source.
///
/// [`extended_kind`]: crate::parser::relationship_extraction::extended_kind
#[must_use]
pub fn resolve_reference(
    table: &SymbolTable,
    source: &RefSource<'_>,
    reference: &SymbolRef,
    cross_file_calls: bool,
    fanout: usize,
) -> Vec<Relationship> {
    let name = reference.target_name.as_str();
    let file = source.file_path;
    let (resolution, metadata) = match reference.kind {
        RelationshipType::Calls => {
            // A name a built-in of the caller's language has (`print`, `fmt`) is
            // the built-in, unless the file imports a definition of it. Go's
            // `new` is no Rust built-in: read for every language, it made a
            // Rust `Order::new()` a call no other file could answer.
            let builtin =
                is_common_builtin(name, source.language) && !table.imports_name(file, name);
            // A qualified call is of a member of the type its qualifier names,
            // and one through a path of nothing else.
            let scope = CallScope::of(
                reference.qualifier.as_deref(),
                source.owner,
                name,
                source.language,
            );
            let resolution = if !cross_file_calls || builtin {
                table.resolve_local(name, file, fanout)
            } else {
                table.resolve_call_in(name, scope, file, source.language, fanout)
            };
            (resolution, serde_json::json!({ "called_name": name }))
        }
        RelationshipType::Uses => (
            table.resolve_type_in(name, file, source.language, fanout),
            serde_json::json!({ "type_reference": name }),
        ),
        RelationshipType::Inherits | RelationshipType::Implements => (
            table.resolve_type_in(name, file, source.language, fanout),
            serde_json::json!({ "child": source.name, "parent": name }),
        ),
        // A name used as a value — a constant or property read
        // (`ReeferLimits.MAX_CELSIUS`), a function passed
        // (`HandleFunc("/x", handleCheckout)`): only a constant or a function
        // is one, a member of the type its qualifier names first.
        RelationshipType::References => {
            let scope = CallScope::of(
                reference.qualifier.as_deref(),
                source.owner,
                name,
                source.language,
            );
            (
                table.resolve_value_in(name, scope, file, source.language, fanout),
                serde_json::json!({ "used_name": name }),
            )
        }
        _ => return Vec::new(),
    };
    let mut metadata = metadata;
    metadata["lines"] = serde_json::json!([reference.line]);
    let now = OffsetDateTime::now_utc();
    resolution
        .entity_ids()
        .iter()
        .filter(|&&target_id| target_id != source.id)
        .map(|&target_id| {
            let relationship_type = match reference.kind {
                RelationshipType::Inherits => {
                    let kind = |id| table.get_by_id(id).map(|e| e.entity_type.as_str());
                    crate::parser::relationship_extraction::extended_kind(
                        source.kind.or_else(|| kind(source.id)).unwrap_or_default(),
                        kind(target_id).unwrap_or_default(),
                    )
                }
                kind => kind,
            };
            let mut relationship = Relationship {
                id: Uuid::new_v4(),
                source_id: source.id,
                target_id,
                relationship_type,
                metadata: Some(metadata.clone()),
                created_at: now,
            };
            relationship.mark_ambiguous(resolution.candidates());
            relationship
        })
        .collect()
}

/// What [`rebuild_edges_for_sources`] resolves references against, and how.
#[derive(Debug, Clone, Copy)]
pub struct Resolution<'a> {
    /// The scan root: only entities stored under it are definitions a
    /// reference resolves to, as in a scan of it. The database may hold
    /// other projects, and a reference in this one must not link into them.
    pub root: &'a Path,
    /// Whether calls resolve across files (`CrossFileMode::Deep`); otherwise
    /// a call links only a definition in its own file.
    pub cross_file_calls: bool,
    /// How many of several equally ranked definitions a reference is linked
    /// to, at least one.
    pub fanout: usize,
}

/// Derive the edges of `source_ids` again from their stored references,
/// resolved against the entities and imports in the database with the
/// ranking of [`resolve_reference`].
///
/// The forward edges of each source, but for the nesting of members in their
/// types (`has_method`, `method_of`, which come from the file), and the
/// reverse rows of those, are replaced; a source with no references keeps
/// none. Sources are taken a chunk at a time, and a chunk's edges are written
/// in groups of sources with a bounded number of references, one transaction
/// each, which replaces every edge of its sources. The definitions and imports
/// read for a chunk are kept for the next ones, up to 500,000 definitions.
/// Returns the rows inserted and, of those, how many join entities of
/// different files.
///
/// # Errors
/// Returns an error if a database read or write fails.
pub async fn rebuild_edges_for_sources(
    db: &Database,
    source_ids: &[EntityId],
    resolution: Resolution<'_>,
) -> Result<(usize, usize)> {
    derive_edges(db, source_ids, resolution, Write::Replace).await
}

/// [`rebuild_edges_for_sources`] for entities just stored, which have no
/// edges derived from their references yet, nor reverse rows of those: the
/// edges are only inserted, with nothing to look up and delete first. A fresh
/// index derives its edges this way.
///
/// # Errors
/// Returns an error if a database read or write fails.
pub async fn derive_edges_for_new_sources(
    db: &Database,
    source_ids: &[EntityId],
    resolution: Resolution<'_>,
) -> Result<(usize, usize)> {
    derive_edges(db, source_ids, resolution, Write::Insert).await
}

/// How [`derive_edges`] stores a chunk's edges.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// In place of the sources' derived edges.
    Replace,
    /// Next to nothing: the sources have none yet.
    Insert,
}

/// [`rebuild_edges_for_sources`] and [`derive_edges_for_new_sources`].
async fn derive_edges(
    db: &Database,
    source_ids: &[EntityId],
    resolution: Resolution<'_>,
    write: Write,
) -> Result<(usize, usize)> {
    let (mut inserted, mut cross_file) = (0, 0);
    // The definitions of every name read so far, and the imports of every
    // file: all a name has under the root, or all a file has.
    let mut table = SymbolTable::new();
    let mut names_read: HashSet<String> = HashSet::new();
    let mut files_read: HashSet<String> = HashSet::new();
    // Where the time goes, and how many definitions were read.
    let mut timing = Timing::default();
    for chunk in source_ids.chunks(CHUNK) {
        if table.len() > KEPT_DEFINITIONS {
            table = SymbolTable::new();
            names_read.clear();
            files_read.clear();
        }
        let started = Instant::now();
        let refs = db.get_symbol_refs_for_sources(chunk).await?;
        timing.references += started.elapsed();
        let started = Instant::now();
        // The names of the sources whose references name their supertypes,
        // which the edges to those record, and their kinds, which decide
        // whether they extend or implement them: the definitions read for a
        // chunk hold a source only when something names it, and an interface
        // nobody named would be taken for implementing the one it extends.
        let supertyped: Vec<EntityId> = refs
            .iter()
            .filter(|stored| {
                matches!(
                    stored.reference.kind,
                    RelationshipType::Inherits | RelationshipType::Implements
                )
            })
            .map(|stored| stored.reference.source_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let names_of: HashMap<EntityId, (String, String)> = db
            .get_entity_names_by_ids(&supertyped)
            .await?
            .into_iter()
            .map(|source| (source.id, (source.name, source.entity_type)))
            .collect();
        // The types of the sources that make qualified calls or uses, which
        // their `self` and `this` name (`self.total()`, `self.MAX`).
        let qualifying: Vec<EntityId> = refs
            .iter()
            .filter(|stored| {
                matches!(
                    stored.reference.kind,
                    RelationshipType::Calls | RelationshipType::References
                ) && stored.reference.qualifier.is_some()
            })
            .map(|stored| stored.reference.source_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let owners_of: HashMap<EntityId, String> = db
            .get_member_owners(&qualifying)
            .await?
            .into_iter()
            .collect();
        let files: Vec<String> = refs
            .iter()
            .map(|stored| stored.file_path.as_str())
            .filter(|file| !files_read.contains(*file))
            .collect::<HashSet<_>>()
            .into_iter()
            .map(str::to_string)
            .collect();
        let imports = db.get_file_imports(&files).await?;
        files_read.extend(files);

        // Every entity a reference, or an import it may resolve through,
        // could name — and the module or type a call's path names, which
        // decides what the call may be of — that no chunk before read.
        let mut names: HashSet<String> = refs
            .iter()
            .flat_map(|stored| {
                let path = CallScope::of(
                    stored.reference.qualifier.as_deref(),
                    None,
                    &stored.reference.target_name,
                    &stored.language,
                )
                .path();
                std::iter::once(stored.reference.target_name.as_str()).chain(path)
            })
            .filter(|name| !names_read.contains(*name))
            .map(str::to_string)
            .collect();
        names.extend(
            imports
                .values()
                .flatten()
                .map(|import| import.original_name.as_ref().unwrap_or(&import.local_name))
                .filter(|name| !names_read.contains(*name))
                .cloned(),
        );
        let names: Vec<String> = names.into_iter().collect();
        for entry in db.get_symbol_entries_named(&names, resolution.root).await? {
            timing.definitions_read += 1;
            table.add_entry(entry);
        }
        names_read.extend(names);
        for (file, file_imports) in imports {
            table.add_imports(Path::new(&file), file_imports);
        }
        timing.definitions += started.elapsed();

        // The chunk's sources in groups whose references stay under
        // `REFS_PER_WRITE`, each resolved and written on its own: a chunk of
        // test files can make ten times the references of another, and every
        // edge of a group is held until it is written. A group holds every
        // reference of its sources, so their edges are replaced at once.
        let mut by_source: HashMap<EntityId, Vec<&StoredRef>> = HashMap::new();
        for stored in &refs {
            by_source
                .entry(stored.reference.source_id)
                .or_default()
                .push(stored);
        }
        let mut group: Vec<EntityId> = Vec::new();
        let mut group_refs: Vec<&StoredRef> = Vec::new();
        for (index, &source) in chunk.iter().enumerate() {
            group.push(source);
            if let Some(source_refs) = by_source.get(&source) {
                group_refs.extend(source_refs);
            }
            if group_refs.len() < REFS_PER_WRITE && index + 1 < chunk.len() {
                continue;
            }
            let started = Instant::now();
            let mut edges = Vec::new();
            for stored in &group_refs {
                let header = names_of.get(&stored.reference.source_id);
                let source = RefSource {
                    id: stored.reference.source_id,
                    name: header.map(|(name, _)| name.as_str()),
                    kind: header.map(|(_, kind)| kind.as_str()),
                    file_path: Path::new(&stored.file_path),
                    language: &stored.language,
                    owner: owners_of
                        .get(&stored.reference.source_id)
                        .map(String::as_str),
                };
                edges.extend(resolve_reference(
                    &table,
                    &source,
                    &stored.reference,
                    resolution.cross_file_calls,
                    resolution.fanout,
                ));
            }
            let mut edges = merge_lines(edges);
            let reverse = generate_reverse_relationships(&edges);
            edges.extend(reverse);
            // A source's file is its references'; a target's, its definition's.
            let file_of: HashMap<EntityId, &str> = group_refs
                .iter()
                .map(|stored| (stored.reference.source_id, stored.file_path.as_str()))
                .collect();
            let file = |id: EntityId| {
                file_of
                    .get(&id)
                    .copied()
                    .or_else(|| table.get_by_id(id).and_then(|e| e.file_path.to_str()))
            };
            let crosses: Vec<bool> = edges
                .iter()
                .map(|edge| file(edge.source_id) != file(edge.target_id))
                .collect();
            timing.resolving += started.elapsed();

            let started = Instant::now();
            let written = match write {
                Write::Replace => db.replace_edges_of_sources(&group, edges).await?,
                Write::Insert => db.insert_relationships(edges).await?,
            };
            timing.writing += started.elapsed();
            for (crosses, _) in crosses.iter().zip(written).filter(|(_, written)| *written) {
                inserted += 1;
                if *crosses {
                    cross_file += 1;
                }
            }
            group.clear();
            group_refs.clear();
        }
    }
    tracing::debug!(
        "Derived the edges of {} sources: references read in {:.1}s, {} definitions in {:.1}s, \
         resolved in {:.1}s, written in {:.1}s",
        source_ids.len(),
        timing.references.as_secs_f64(),
        timing.definitions_read,
        timing.definitions.as_secs_f64(),
        timing.resolving.as_secs_f64(),
        timing.writing.as_secs_f64(),
    );
    Ok((inserted, cross_file))
}

/// One edge for each source, target and kind among `edges`, on the lines of
/// all of them (`"lines"`, in order): the references a source makes to one
/// definition yield one edge, which the database would otherwise keep with
/// the first reference's line alone. The edge is ambiguous only when each of
/// those references left the definition one of several.
fn merge_lines(edges: Vec<Relationship>) -> Vec<Relationship> {
    let lines = |edge: &Relationship| -> Vec<u64> {
        edge.metadata
            .as_ref()
            .and_then(|metadata| metadata["lines"].as_array())
            .map(|lines| lines.iter().filter_map(serde_json::Value::as_u64).collect())
            .unwrap_or_default()
    };
    let mut merged: Vec<Relationship> = Vec::with_capacity(edges.len());
    let mut at: HashMap<(EntityId, EntityId, RelationshipType), usize> = HashMap::new();
    for edge in edges {
        let key = (edge.source_id, edge.target_id, edge.relationship_type);
        let Some(&index) = at.get(&key) else {
            at.insert(key, merged.len());
            merged.push(edge);
            continue;
        };
        let kept = &mut merged[index];
        let mut all = lines(kept);
        all.extend(lines(&edge));
        all.sort_unstable();
        all.dedup();
        let certain = !edge.is_ambiguous();
        if let Some(fields) = kept.metadata.as_mut().and_then(|m| m.as_object_mut()) {
            fields.insert("lines".to_string(), serde_json::json!(all));
            if certain {
                fields.remove("ambiguous");
                fields.remove("candidates");
            }
        }
    }
    merged
}

/// Where [`rebuild_edges_for_sources`] spent its time.
#[derive(Debug, Default)]
struct Timing {
    /// Reading the sources' references.
    references: Duration,
    /// Reading the names of supertyped sources, their files' imports and the
    /// definitions named.
    definitions: Duration,
    /// How many definitions were read.
    definitions_read: usize,
    /// Resolving the references.
    resolving: Duration,
    /// Replacing the sources' edges.
    writing: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(source: EntityId, target: EntityId, line: u32, candidates: usize) -> Relationship {
        let mut edge = Relationship {
            id: Uuid::new_v4(),
            source_id: source,
            target_id: target,
            relationship_type: RelationshipType::Calls,
            metadata: Some(serde_json::json!({"called_name": "add", "lines": [line]})),
            created_at: OffsetDateTime::now_utc(),
        };
        edge.mark_ambiguous(candidates);
        edge
    }

    /// The calls a source makes to one definition make one edge, on all their
    /// lines; it is ambiguous only when each call left the definition one of
    /// several. Calls to another definition make an edge of their own.
    #[test]
    fn the_references_to_one_definition_make_one_edge_on_all_their_lines() {
        let (caller, add, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let merged = merge_lines(vec![
            call(caller, add, 9, 2),
            call(caller, other, 9, 2),
            call(caller, add, 4, 2),
        ]);
        assert_eq!(merged.len(), 2);
        let lines = |edge: &Relationship| edge.metadata.as_ref().unwrap()["lines"].clone();
        assert_eq!(lines(&merged[0]), serde_json::json!([4, 9]));
        assert!(merged[0].is_ambiguous(), "every call was ambiguous");
        assert_eq!(lines(&merged[1]), serde_json::json!([9]));

        let merged = merge_lines(vec![call(caller, add, 9, 2), call(caller, add, 4, 1)]);
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].is_ambiguous(), "one call was certain");
        assert!(
            merged[0]
                .metadata
                .as_ref()
                .unwrap()
                .get("candidates")
                .is_none()
        );
    }
}
