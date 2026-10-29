//! Lightweight symbol table for name resolution.
//!
//! It stores only the metadata resolution ranks definitions by — name, id,
//! file, start line, language and entity type — never bodies, and the imports
//! of the files whose references it resolves. Edges are derived a chunk of
//! sources at a time ([`crate::graph::resolve`]), each chunk with a table of
//! just the definitions its references name.

use crate::database::embeddings::split_identifier_tokens;
use crate::entities::{Entity, EntityId, RelationshipType, SymbolRef, is_type_definition};
use crate::parser::cross_file::{ExternalSymbol, SymbolResolution};
use crate::parser::import_extraction::{ImportInfo, go_package_name};
use crate::parser::standard_methods::is_standard_method;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static TYPE_NAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"([A-Z][a-zA-Z0-9_]*)(?:\s*[<\[][^>\]]*[>\]])?")
        .expect("TYPE_NAME_RE is a valid regex")
});

/// How many of several equally good definitions of a name a reference is
/// linked to unless configured otherwise. Each of those edges is marked
/// ambiguous (see [`Relationship::mark_ambiguous`](crate::entities::Relationship::mark_ambiguous)).
pub const DEFAULT_AMBIGUOUS_FANOUT: usize = 3;

/// Lightweight metadata for a single entity, sufficient for cross-file
/// symbol resolution without holding the entity body in RAM.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// Unique entity identifier
    pub id: EntityId,
    /// Entity name (used for resolution)
    pub name: String,
    /// File where the entity is defined
    pub file_path: PathBuf,
    /// Line the definition starts on (1-based); orders same-named
    /// definitions of one file.
    pub start_line: u32,
    /// Programming language
    pub language: String,
    /// Entity type string (function, class, etc.)
    pub entity_type: String,
    /// The type a function or constant is a member of (its `parent_class`):
    /// the one a qualified call's or use's qualifier names (`Order::new`,
    /// `cart.total()`, `ReeferLimits.MAX_CELSIUS`).
    pub owner: Option<String>,
}

impl SymbolEntry {
    /// Build a [`SymbolEntry`] from a full [`Entity`], extracting only the
    /// metadata needed for cross-file analysis.
    pub fn from_entity(entity: &Entity) -> Self {
        Self {
            id: entity.id(),
            name: entity.name().to_string(),
            file_path: PathBuf::from(entity.file_path()),
            start_line: entity.location().start_line,
            language: entity.language().to_string(),
            entity_type: entity.entity_type().to_string(),
            owner: match entity {
                Entity::Function(f) => f.parent_class.clone(),
                Entity::Constant(c) => c.parent_class.clone(),
                _ => None,
            },
        }
    }
}

/// The type a call's qualifier names, whose members it calls: its last
/// segment — `Order` of `crate::order::Order` or `Order::new`, `Vec` of
/// `Vec::<u8>::new`, `cart` of `self.cart` — or, for `self`, `this`, `Self`
/// and `static`, the type the caller is a member of.
pub(crate) fn qualifier_owner<'a>(
    qualifier: &'a str,
    caller_owner: Option<&'a str>,
) -> Option<&'a str> {
    match last_segment(qualifier) {
        "self" | "this" | "Self" | "static" => caller_owner,
        "" => None,
        other => Some(other),
    }
}

/// The last segment of a qualifier, without its type arguments — a path's
/// `::` ends it (`Vec::<u8>::`).
fn last_segment(qualifier: &str) -> &str {
    let mut text = qualifier.trim().trim_end_matches(':');
    if text.ends_with('>') {
        let mut depth = 0usize;
        for (at, c) in text.char_indices().rev() {
            match c {
                '>' => depth += 1,
                '<' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        text = text[..at].trim_end_matches(':');
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    text.rsplit(['.', ':', '\\', '>'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(text)
        .trim_start_matches(['$', '@'])
        .trim()
}

/// What a call's qualifier and name say of the definitions the call may be of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallScope<'a> {
    /// Unqualified, or qualified by what says no more than the name does
    /// (Rust's `crate::`, `super::`, `self::`): any definition of the name.
    Any,
    /// A value (`cart.total()`, `self.total()`), of a type the index cannot
    /// tell: the members of a type it is named after rank first.
    Receiver {
        /// What the value is named after: the qualifier's last segment, or
        /// the caller's own type for `self` and `this`.
        named: &'a str,
        /// The name its expression starts with (`os` of `os.path`, `Agent` of
        /// `Agent::new(x)`): one an import binds makes the call the module's.
        head: &'a str,
    },
    /// A value calling a method the language's standard types have
    /// (`items.len()`, `dir.path()`; see
    /// [`is_standard_method`](super::standard_methods::is_standard_method)):
    /// only a member of a type it is named after, as anything else is the
    /// standard type's, which the index does not hold. Resolved by name, such
    /// calls would be most of those guessed, nearly all wrong.
    Standard {
        /// As [`Self::Receiver`]'s.
        named: &'a str,
        /// As [`Self::Receiver`]'s.
        head: &'a str,
    },
    /// A path (`Order::new`, `money::round`, `std::abs`, `Self::fee`): a
    /// member of the type it names or a function of the module it names, and
    /// nothing else — a path to neither is to code outside the index, so a
    /// `Vec::new()` is no call of whichever `new` the index holds.
    Path(&'a str),
}

impl<'a> CallScope<'a> {
    /// The scope of a call of `name` qualified by `qualifier` — a path when it
    /// ends in `::`, as the parser records one, else a value — made in a
    /// member of `caller_owner`, in `language`.
    #[must_use]
    pub fn of(
        qualifier: Option<&'a str>,
        caller_owner: Option<&'a str>,
        name: &str,
        language: &str,
    ) -> Self {
        let Some(qualifier) = qualifier else {
            return Self::Any;
        };
        if !qualifier.ends_with("::") {
            let Some(named) = qualifier_owner(qualifier, caller_owner) else {
                return Self::Any;
            };
            let head = qualifier
                .trim_start_matches(['$', '@'])
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default();
            return if is_standard_method(name, language) {
                Self::Standard { named, head }
            } else {
                Self::Receiver { named, head }
            };
        }
        match last_segment(qualifier) {
            // Rust's paths from the crate's root, a parent or the module itself.
            "crate" | "super" => Self::Any,
            "self" if language == "rust" => Self::Any,
            // PHP's `parent::`, the supertype the index does not tell.
            "parent" => Self::Any,
            _ => qualifier_owner(qualifier, caller_owner).map_or(Self::Any, Self::Path),
        }
    }

    /// Whether a definition that is a member of `owner` (`None`: of no type)
    /// is a member of the type the qualifier names: the path's, by its name,
    /// or one a value is named after (see [`named_after`]). Such members rank
    /// first.
    pub(crate) fn names_owner(self, owner: Option<&str>) -> bool {
        owner.is_some_and(|owner| match self {
            Self::Any => false,
            Self::Receiver { named, .. } | Self::Standard { named, .. } => {
                named_after(named, owner)
            }
            Self::Path(path) => same_type_name(owner, path),
        })
    }

    /// The name a value's expression starts with, which an import may bind.
    pub(crate) fn head(self) -> Option<&'a str> {
        match self {
            Self::Receiver { head, .. } | Self::Standard { head, .. } => Some(head),
            Self::Any | Self::Path(_) => None,
        }
    }

    /// The module or type a path names, whose definitions resolution must
    /// read as well as the name's.
    #[must_use]
    pub fn path(self) -> Option<&'a str> {
        match self {
            Self::Path(path) => Some(path),
            _ => None,
        }
    }
}

/// Whether `file` is the module `module` names: named after it (`money.rs`,
/// `geo.cpp`), or the `mod.rs`, `index` or `__init__` of a directory named
/// after it.
pub(crate) fn names_module(file: &Path, module: &str) -> bool {
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if matches!(stem, "mod" | "index" | "__init__") {
        file.parent()
            .and_then(|dir| dir.file_name())
            .and_then(|dir| dir.to_str())
            .is_some_and(|dir| same_type_name(dir, module))
    } else {
        same_type_name(stem, module)
    }
}

/// Whether a value named `value` is named after the type `ty`: by its name
/// ([`same_type_name`]: `cart` for `Cart`, `order_service` for
/// `OrderService`), or by its last words (`queue` for `PromptQueue`,
/// `registry` for `ToolRegistry`).
pub(crate) fn named_after(value: &str, ty: &str) -> bool {
    if same_type_name(ty, value) {
        return true;
    }
    let (value, ty) = (split_identifier_tokens(value), split_identifier_tokens(ty));
    !value.is_empty()
        && ty
            .strip_suffix(value.as_str())
            .is_some_and(|before| before.ends_with(' '))
}

/// Whether `owner` and `named` name one type: the same but for case and
/// underscores, as a receiver is commonly named after its type (`cart` for
/// `Cart`, `order_service` for `OrderService`).
pub(crate) fn same_type_name(owner: &str, named: &str) -> bool {
    let normal = |name: &str| -> String {
        name.chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect()
    };
    normal(owner) == normal(named)
}

/// The types `entity`'s signature names — parameter, return and field types —
/// as references of kind `Uses`, on the entity's first line.
pub fn type_refs(entity: &Entity) -> impl Iterator<Item = SymbolRef> + use<> {
    let source_id = entity.id();
    let line = entity.location().start_line;
    extract_type_refs_from_entity(entity)
        .into_iter()
        .map(move |target_name| SymbolRef {
            source_id,
            kind: RelationshipType::Uses,
            target_name,
            qualifier: None,
            line,
        })
}

/// Extract type references from an entity without needing the full body.
/// This mirrors `cross_file::extract_type_references` but operates on the
/// strongly-typed entity struct rather than a regex over body text.
fn extract_type_refs_from_entity(entity: &Entity) -> Vec<String> {
    use crate::entities::*;
    let mut types = Vec::new();

    match entity {
        Entity::Function(f) => {
            if let Some(rt) = &f.return_type {
                types.extend(extract_type_names(rt));
            }
            if let Some(params) = &f.parameters {
                types.extend(extract_type_names(params));
            }
        }
        Entity::Class(c) => types.extend(field_types(c.fields.as_ref())),
        Entity::Struct(s) => types.extend(field_types(s.fields.as_ref())),
        Entity::Interface(i) => types.extend(field_types(i.properties.as_ref())),
        _ => {}
    }

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    types.retain(|t| seen.insert(t.clone()));
    types
}

/// Extract type names from a type signature string.
fn extract_type_names(type_str: &str) -> Vec<String> {
    TYPE_NAME_RE
        .captures_iter(type_str)
        .filter_map(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .filter(|t| !is_primitive_type(t))
        .collect()
}

/// The type names fields' written types hold — `[{"name": "balance",
/// "type": "Money"}]` uses `Money` — and not their names: a capitalized field
/// (C#'s `Total`, Go's `Name`) is no type it uses, nor are VB.NET's `Inherits`
/// and `Implements`, read as fields, nor a Go interface method's name.
fn field_types(fields: Option<&serde_json::Value>) -> Vec<String> {
    fields
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| field.get("type")?.as_str())
        .flat_map(extract_type_names)
        .collect()
}

/// Check if a type name is a primitive that should not create a relationship.
/// Delegates to the shared `utils::type_helpers` list.
fn is_primitive_type(type_name: &str) -> bool {
    crate::utils::type_helpers::is_primitive_type(type_name)
}

/// What a reference names, which decides the definitions that fit it best.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// A call or a plain name: any definition.
    Name,
    /// A type annotation: a type definition before any other definition of the
    /// name.
    Type,
    /// A name used as a value (a constant or property read, a function
    /// passed): only a constant or a function definition.
    Value,
}

/// Lightweight in-memory symbol table for name resolution: the definitions a
/// set of references may resolve to, and the imports of the files they are
/// made in. It holds no entity bodies.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// Primary storage — flat Vec for cache-friendly iteration.
    entries: Vec<SymbolEntry>,
    /// name → indices into `entries` (for global lookup).
    by_name: HashMap<String, Vec<usize>>,
    /// file path → indices into `entries` (for local-preference lookup).
    by_file: HashMap<PathBuf, Vec<usize>>,
    /// EntityId → index into `entries` (for fast ID-based lookup).
    by_id: HashMap<EntityId, usize>,
    /// Import mappings per file.
    imports: HashMap<PathBuf, Vec<ImportInfo>>,
    /// External symbols (standard library, built-ins).
    external_symbols: HashMap<String, ExternalSymbol>,
}

impl SymbolTable {
    /// Create an empty symbol table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve capacity for `n` entries to avoid repeated reallocation.
    pub fn reserve(&mut self, n: usize) {
        self.entries.reserve(n);
        self.by_name.reserve(n);
        self.by_file.reserve(n / 10); // heuristic: ~10 entities per file
        self.by_id.reserve(n);
    }

    /// Add a single entity to the table.
    pub fn add_entity(&mut self, entity: &Entity) {
        let entry = SymbolEntry::from_entity(entity);
        self.add_entry(entry);
    }

    /// Add a pre-built [`SymbolEntry`] to storage and every lookup index.
    pub fn add_entry(&mut self, entry: SymbolEntry) {
        let idx = self.entries.len();
        self.by_name
            .entry(entry.name.clone())
            .or_default()
            .push(idx);
        self.by_file
            .entry(entry.file_path.clone())
            .or_default()
            .push(idx);
        self.by_id.insert(entry.id, idx);
        self.entries.push(entry);
    }

    /// Add import information for a file.
    pub fn add_imports(&mut self, file_path: &Path, imports: Vec<ImportInfo>) {
        self.imports.insert(file_path.to_path_buf(), imports);
    }

    /// Whether `file` imports `name` by that name (`import { fmt } from …`),
    /// not through a wildcard.
    #[must_use]
    pub fn imports_name(&self, file: &Path, name: &str) -> bool {
        self.imports.get(file).is_some_and(|imports| {
            imports
                .iter()
                .any(|i| !i.is_wildcard && i.local_name == name)
        })
    }

    /// Look up an entry by entity ID.
    #[must_use]
    pub fn get_by_id(&self, id: EntityId) -> Option<&SymbolEntry> {
        self.by_id.get(&id).and_then(|&idx| self.entries.get(idx))
    }

    /// Look up an entity by name (any file), the same one every run.
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<&SymbolEntry> {
        let indices = self.by_name.get(name)?;
        let (best, _) = self.best_candidates(
            indices,
            Path::new(""),
            None,
            Reference::Name,
            CallScope::Any,
            1,
        );
        best.first().copied()
    }

    /// The best of several same-named candidates for a reference from
    /// `current_file`, at most `cap` of them, in an order that is the same in
    /// every run and every checkout, together with how many tied before the
    /// cap.
    ///
    /// Entries are appended as files finish parsing, in parallel, so taking
    /// the first match would make the target of an ambiguous call differ from
    /// run to run. Ranked, best first: a member of the type a qualified call names,
    /// a type definition for a [`Reference::Type`], the caller's `language`,
    /// then the most leading directories in common with the caller (its own
    /// package before a distant one). A call through a path has only what the
    /// path names among its candidates (see [`Self::of_path`]), and one of a
    /// standard type's method only the members of a type its value is named
    /// after (see [`CallScope::Standard`]). Every entry of
    /// the best rank is a candidate, ordered by file path, start line and
    /// entity id. File paths within one tree sort the same in every checkout;
    /// the id hashes the absolute path, so it decides only between
    /// definitions that start on the same line.
    fn best_candidates<'a>(
        &'a self,
        indices: &[usize],
        current_file: &Path,
        language: Option<&str>,
        reference: Reference,
        scope: CallScope<'_>,
        cap: usize,
    ) -> (Vec<&'a SymbolEntry>, usize) {
        let dir = current_file.parent().unwrap_or(current_file);
        let rank = |entry: &SymbolEntry| {
            (
                // A member of the type a qualified call names first.
                scope != CallScope::Any && !scope.names_owner(entry.owner.as_deref()),
                reference == Reference::Type && !is_type_definition(&entry.entity_type),
                language.is_some_and(|lang| entry.language != lang),
                std::cmp::Reverse(shared_components(dir, &entry.file_path)),
            )
        };
        let entries = indices
            .iter()
            .filter_map(|&idx| self.entries.get(idx))
            .filter(|entry| {
                reference != Reference::Value
                    || matches!(entry.entity_type.as_str(), "function" | "constant")
            })
            .filter(|entry| match scope {
                CallScope::Path(path) => self.of_path(entry, path),
                CallScope::Standard { .. } => scope.names_owner(entry.owner.as_deref()),
                // A value's member is the type's it is named after: `o.size`
                // is no constant `size` of another type, as a call of `size`
                // through `o` might be the only one.
                CallScope::Receiver { .. } => {
                    reference != Reference::Value || scope.names_owner(entry.owner.as_deref())
                }
                CallScope::Any => true,
            });
        let Some(best) = entries.clone().map(rank).min() else {
            return (Vec::new(), 0);
        };
        let mut candidates: Vec<&SymbolEntry> =
            entries.filter(|entry| rank(entry) == best).collect();
        candidates.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then(a.start_line.cmp(&b.start_line))
                .then(a.id.cmp(&b.id))
        });
        let total = candidates.len();
        candidates.truncate(cap);
        (candidates, total)
    }

    /// The definitions of `name` in `file`, at most `cap`, earliest first;
    /// `None` when the file does not define the name. A file's own
    /// definitions shadow every other.
    fn same_file_candidates(
        &self,
        name: &str,
        file: &Path,
        reference: Reference,
        scope: CallScope<'_>,
        cap: usize,
    ) -> Option<(Vec<&SymbolEntry>, usize)> {
        let indices: Vec<usize> = self
            .by_file
            .get(file)?
            .iter()
            .copied()
            .filter(|&idx| self.entries.get(idx).is_some_and(|e| e.name == name))
            .collect();
        (!indices.is_empty())
            .then(|| self.best_candidates(&indices, file, None, reference, scope, cap))
    }

    /// Whether `entry` is what a call through the path `path` may be of: a
    /// member of the type the path names, or a function of the module it
    /// names — of a file named after it ([`names_module`]), or of one holding
    /// a module or namespace of its name (`mod money { … }`,
    /// `namespace geo { … }`).
    fn of_path(&self, entry: &SymbolEntry, path: &str) -> bool {
        match entry.owner.as_deref() {
            Some(owner) => same_type_name(owner, path),
            None => {
                names_module(&entry.file_path, path)
                    || self.by_name.get(path).is_some_and(|indices| {
                        indices.iter().any(|&idx| {
                            self.entries.get(idx).is_some_and(|module| {
                                matches!(module.entity_type.as_str(), "module" | "namespace")
                                    && module.file_path == entry.file_path
                            })
                        })
                    })
            }
        }
    }

    /// The best definitions of `name` anywhere, at most `cap`, for a
    /// reference from `current_file` in `language` (when not given, the
    /// language of the file's first entry): the caller's language first
    /// (same-language module resolution), then the nearest.
    fn project_candidates(
        &self,
        name: &str,
        current_file: &Path,
        language: Option<&str>,
        reference: Reference,
        scope: CallScope<'_>,
        cap: usize,
    ) -> (Vec<&SymbolEntry>, usize) {
        let caller_lang = language.or_else(|| {
            self.by_file
                .get(current_file)
                .and_then(|ids| ids.first())
                .and_then(|&idx| self.entries.get(idx))
                .map(|e| e.language.as_str())
        });
        let Some(indices) = self.by_name.get(name) else {
            return (Vec::new(), 0);
        };
        // A definition in a language whose code cannot call the caller's is no
        // candidate: a shell script running `export_report` runs a program,
        // not the Rust function of that name ([`language_family`]). A name
        // used as a value is looked for beside the caller only — a Go
        // package's other files — through an import, or as a member of the
        // type its qualifier names: a name that is a variable shares it with
        // constants and functions anywhere.
        let family = caller_lang.map(language_family);
        let dir = current_file.parent();
        let indices: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&idx| {
                self.entries.get(idx).is_some_and(|entry| {
                    family.is_none_or(|family| language_family(&entry.language) == family)
                        && (reference != Reference::Value
                            || entry.file_path.parent() == dir
                            || scope.names_owner(entry.owner.as_deref()))
                })
            })
            .collect();
        self.best_candidates(&indices, current_file, caller_lang, reference, scope, cap)
    }

    /// Look up entities by name with file preference (tries current file
    /// first), the same one every run.
    #[must_use]
    pub fn find_prefer_local(&self, name: &str, current_file: &Path) -> Option<&SymbolEntry> {
        let (best, _) = self
            .same_file_candidates(name, current_file, Reference::Name, CallScope::Any, 1)
            .unwrap_or_else(|| {
                self.project_candidates(
                    name,
                    current_file,
                    None,
                    Reference::Name,
                    CallScope::Any,
                    1,
                )
            });
        best.first().copied()
    }

    /// The best definitions of `name`, at most `cap`, preferring those of the
    /// module `module` names ([`Self::in_module`]); the best anywhere when the
    /// module defines none.
    fn find_in_module(
        &self,
        name: &str,
        module: &str,
        current_file: &Path,
        reference: Reference,
        scope: CallScope<'_>,
        cap: usize,
    ) -> (Vec<&SymbolEntry>, usize) {
        let in_module = self.in_module(name, module, current_file);
        if !in_module.is_empty() {
            return self.best_candidates(&in_module, current_file, None, reference, scope, cap);
        }

        // Fall back to the best match globally.
        let Some(indices) = self.by_name.get(name) else {
            return (Vec::new(), 0);
        };
        self.best_candidates(indices, current_file, None, reference, scope, cap)
    }

    /// The definitions of `name` in the module `module` names, imported by
    /// `current_file`: for a path next to it (`./b.js`, `../api/orders`), the
    /// file the path names, with or without its extension, or its
    /// directory's `index`; for Python's `.` and `..`, the files of its
    /// package or of the package's parent; for any other, a file or directory
    /// named after the module's last segment ([`module_stem`]). A path's last
    /// dotted segment is its extension, `js`, which every JavaScript file
    /// holds; and a file's path merely holding the segment made `re` and `os`
    /// modules of most files.
    fn in_module(&self, name: &str, module: &str, current_file: &Path) -> Vec<usize> {
        let Some(indices) = self.by_name.get(name) else {
            return Vec::new();
        };
        let named_file = module_file(module, current_file);
        let package = (!module.is_empty() && module.chars().all(|c| c == '.'))
            .then(|| {
                // `.` is the importing file's package, each further dot a parent.
                (1..module.len()).try_fold(current_file.parent()?, |dir, _| dir.parent())
            })
            .flatten();
        let stem = module_stem(module);
        indices
            .iter()
            .copied()
            .filter(|&idx| {
                self.entries.get(idx).is_some_and(|entry| {
                    if let Some(file) = &named_file {
                        return is_the_module_file(&entry.file_path, file);
                    }
                    if let Some(dir) = package {
                        return entry.file_path.parent() == Some(dir);
                    }
                    !stem.is_empty()
                        && (names_module(&entry.file_path, stem)
                            || entry
                                .file_path
                                .parent()
                                .and_then(Path::file_name)
                                .and_then(|dir| dir.to_str())
                                .is_some_and(|dir| same_type_name(dir, stem)))
                })
            })
            .collect()
    }

    /// The import of `file` that binds `name`, unless a wildcard: `import
    /// json` binds `json`, `import os.path` binds `os`, `import numpy as np`
    /// binds `np`.
    fn binding_import(&self, file: &Path, name: &str) -> Option<&ImportInfo> {
        self.imports
            .get(file)?
            .iter()
            .find(|import| !import.is_wildcard && import.local_name.split('.').next() == Some(name))
    }

    /// Resolve a symbol considering imports and local scope.
    ///
    /// Resolution order (most-specific first):
    /// 1. Same file (local definitions)
    /// 2. Explicitly imported symbol — resolved to the `source_module` first,
    ///    then globally if no module-local match exists.
    /// 3. Project-wide, the caller's language and nearest directories first.
    /// 4. Language external symbols / common built-ins.
    ///
    /// When several definitions tie at the first step that finds any, the
    /// result is [`SymbolResolution::Ambiguous`] with at most `fanout` of them
    /// (at least one), ordered by file path, start line and entity id. Any
    /// kind of definition fits; for a name in a type position use
    /// [`Self::resolve_type`].
    #[must_use]
    pub fn resolve_symbol(
        &self,
        name: &str,
        current_file: &Path,
        fanout: usize,
    ) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            None,
            Reference::Name,
            CallScope::Any,
            fanout,
        )
    }

    /// [`Self::resolve_symbol`] for a reference made in `language`, which the
    /// project-wide step ranks first, whatever else the table holds of
    /// `current_file`.
    #[must_use]
    pub fn resolve_symbol_in(
        &self,
        name: &str,
        current_file: &Path,
        language: &str,
        fanout: usize,
    ) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            Some(language),
            Reference::Name,
            CallScope::Any,
            fanout,
        )
    }

    /// [`Self::resolve_symbol_in`] for a call in `scope` (see [`CallScope`]):
    /// at each step a member of the type its qualifier names ranks above any
    /// other definition of the name, so `self.total()` finds the caller's
    /// own, not whichever is nearest; and through a path, only what the path
    /// names is one, so `Order::new` finds the `Order`'s and `Vec::new` none.
    #[must_use]
    pub fn resolve_call_in(
        &self,
        name: &str,
        scope: CallScope<'_>,
        current_file: &Path,
        language: &str,
        fanout: usize,
    ) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            Some(language),
            Reference::Name,
            scope,
            fanout,
        )
    }

    /// [`Self::resolve_call_in`] for a name used as a value, not called — a
    /// constant or property read (`ReeferLimits.MAX_CELSIUS`), a function
    /// passed (`HandleFunc("/x", handleCheckout)`): the constants and
    /// functions among the definitions it resolves to, a member of the type
    /// its qualifier names first.
    #[must_use]
    pub fn resolve_value_in(
        &self,
        name: &str,
        scope: CallScope<'_>,
        current_file: &Path,
        language: &str,
        fanout: usize,
    ) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            Some(language),
            Reference::Value,
            scope,
            fanout,
        )
    }

    /// [`Self::resolve_symbol`] for a name in a type position (a parameter,
    /// return or field type): at each step a type definition (see
    /// [`is_type_definition`]) ranks above any other definition of the name,
    /// so a constructor named after its class does not tie with the class.
    #[must_use]
    pub fn resolve_type(&self, name: &str, current_file: &Path, fanout: usize) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            None,
            Reference::Type,
            CallScope::Any,
            fanout,
        )
    }

    /// [`Self::resolve_type`] for a reference made in `language` (see
    /// [`Self::resolve_symbol_in`]).
    #[must_use]
    pub fn resolve_type_in(
        &self,
        name: &str,
        current_file: &Path,
        language: &str,
        fanout: usize,
    ) -> SymbolResolution {
        self.resolve(
            name,
            current_file,
            Some(language),
            Reference::Type,
            CallScope::Any,
            fanout,
        )
    }

    /// The definitions of `name` in `current_file` alone, at most `fanout`,
    /// earliest first: where a call to a common built-in name may link.
    #[must_use]
    pub fn resolve_local(
        &self,
        name: &str,
        current_file: &Path,
        fanout: usize,
    ) -> SymbolResolution {
        self.same_file_candidates(
            name,
            current_file,
            Reference::Name,
            CallScope::Any,
            fanout.max(1),
        )
        .and_then(SymbolResolution::of_candidates)
        .unwrap_or(SymbolResolution::Unresolved)
    }

    fn resolve(
        &self,
        name: &str,
        current_file: &Path,
        language: Option<&str>,
        reference: Reference,
        scope: CallScope<'_>,
        fanout: usize,
    ) -> SymbolResolution {
        let cap = fanout.max(1);

        // 0. A value an import binds (`json.dumps`, `fmt.Println`, `api.get`)
        //    is the module's: its definitions of the name, or none the index
        //    holds — never a same-named definition anywhere else.
        if let Some(import) = scope
            .head()
            .and_then(|head| self.binding_import(current_file, head))
        {
            // A value of the module (`config.DocCutoff`) is any of its
            // definitions of the name, of no type as a package's constant is.
            let ranking = match scope {
                _ if reference == Reference::Value => CallScope::Any,
                CallScope::Standard { named, head } => CallScope::Receiver { named, head },
                other => other,
            };
            let in_module = self.in_module(name, &import.source_module, current_file);
            let found =
                self.best_candidates(&in_module, current_file, None, reference, ranking, cap);
            return SymbolResolution::of_candidates(found).unwrap_or_else(|| {
                SymbolResolution::External {
                    module: import.source_module.clone(),
                    name: name.to_string(),
                }
            });
        }

        // 1. Same-file definitions always win — a locally-defined `foo` shadows
        //    any import or project-wide entity named `foo`.
        if let Some(local) = self.same_file_candidates(name, current_file, reference, scope, cap)
            && let Some(resolution) = SymbolResolution::of_candidates(local)
        {
            return resolution;
        }

        // 2. Explicitly imported symbol — resolve `from foo import bar` to
        //    `bar` in module `foo` rather than the first `bar` globally.
        if let Some(imports) = self.imports.get(current_file) {
            for import in imports {
                if import.local_name == name {
                    let target_name = import.original_name.as_ref().unwrap_or(&import.local_name);
                    let found = self.find_in_module(
                        target_name,
                        &import.source_module,
                        current_file,
                        reference,
                        scope,
                        cap,
                    );
                    if let Some(resolution) = SymbolResolution::of_candidates(found) {
                        return resolution;
                    }
                    return SymbolResolution::External {
                        module: import.source_module.clone(),
                        name: target_name.to_string(),
                    };
                }
            }
        }

        // 3. Project-wide search with same-language preference.
        if let Some(resolution) = SymbolResolution::of_candidates(self.project_candidates(
            name,
            current_file,
            language,
            reference,
            scope,
            cap,
        )) {
            return resolution;
        }

        // 4. Language-specific external symbols.
        if self.external_symbols.contains_key(name) {
            return SymbolResolution::External {
                module: "__builtin__".to_string(),
                name: name.to_string(),
            };
        }

        // 5. Common built-ins (language-agnostic).
        if is_common_builtin(name) {
            return SymbolResolution::External {
                module: "__builtin__".to_string(),
                name: name.to_string(),
            };
        }

        SymbolResolution::Unresolved
    }

    /// Initialize external symbols for a language.
    pub fn initialize_external_symbols(&mut self, language: &str) {
        let symbols = get_language_external_symbols(language);
        for symbol in symbols {
            self.external_symbols.insert(symbol.name.clone(), symbol);
        }
    }

    /// Get all entries.
    #[must_use]
    pub fn all_entries(&self) -> &[SymbolEntry] {
        &self.entries
    }

    /// Get entries for a specific file.
    #[must_use]
    pub fn entries_for_file(&self, file_path: &Path) -> Vec<&SymbolEntry> {
        self.by_file
            .get(file_path)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|&idx| self.entries.get(idx))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get total entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Check if a name is a common built-in function across languages.
fn is_common_builtin(name: &str) -> bool {
    let builtins: std::collections::HashSet<&str> = [
        "print",
        "len",
        "range",
        "str",
        "int",
        "float",
        "list",
        "dict",
        "set",
        "tuple",
        "map",
        "filter",
        "zip",
        "enumerate",
        "sum",
        "min",
        "max",
        "abs",
        "round",
        "open",
        "input",
        "isinstance",
        "hasattr",
        "getattr",
        "setattr",
        "console",
        "log",
        "warn",
        "error",
        "JSON",
        "Math",
        "Object",
        "Array",
        "String",
        "Number",
        "Boolean",
        "Date",
        "Promise",
        "setTimeout",
        "setInterval",
        "fetch",
        "require",
        "module",
        "exports",
        "println",
        "eprintln",
        "format",
        "vec",
        "Box",
        "Option",
        "Result",
        "String",
        "Vec",
        "HashMap",
        "BTreeMap",
        "Rc",
        "Arc",
        "fmt",
        "Println",
        "Printf",
        "Sprint",
        "len",
        "cap",
        "make",
        "new",
        "System",
        "out",
        "println",
        "Object",
        "Math",
        "Integer",
        "printf",
        "scanf",
        "malloc",
        "free",
        "sizeof",
        "strlen",
        "strcpy",
        "puts",
        "p",
        "include",
        "extend",
        "echo",
        "isset",
        "unset",
        "array",
        "count",
    ]
    .iter()
    .copied()
    .collect();

    builtins.contains(name)
}

/// Get external symbols for a language.
fn get_language_external_symbols(language: &str) -> Vec<ExternalSymbol> {
    let mut symbols = Vec::new();

    if language.eq_ignore_ascii_case("python") {
        let python_builtins = [
            "print",
            "len",
            "range",
            "str",
            "int",
            "float",
            "list",
            "dict",
            "set",
            "tuple",
            "map",
            "filter",
            "zip",
            "enumerate",
            "sum",
            "min",
            "max",
            "abs",
            "round",
            "open",
            "input",
            "isinstance",
            "hasattr",
            "getattr",
            "setattr",
            "dir",
            "type",
            "id",
            "repr",
            "sorted",
            "reversed",
            "any",
            "all",
            "eval",
            "exec",
            "compile",
        ];
        for name in python_builtins {
            symbols.push(ExternalSymbol {
                name: name.to_string(),
                symbol_type: "builtin_function".to_string(),
                language: "python".to_string(),
                documentation: Some(format!(
                    "https://docs.python.org/3/library/functions.html#{name}"
                )),
            });
        }
    } else if language.eq_ignore_ascii_case("javascript")
        || language.eq_ignore_ascii_case("typescript")
    {
        let js_globals = [
            "console",
            "JSON",
            "Math",
            "Object",
            "Array",
            "String",
            "Number",
            "Boolean",
            "Date",
            "Promise",
            "RegExp",
            "Error",
            "Map",
            "Set",
            "WeakMap",
            "WeakSet",
            "Symbol",
            "Proxy",
            "setTimeout",
            "setInterval",
            "clearTimeout",
            "clearInterval",
            "fetch",
            "alert",
            "confirm",
            "prompt",
            "document",
            "window",
        ];
        for name in js_globals {
            symbols.push(ExternalSymbol {
                name: name.to_string(),
                symbol_type: "global_object".to_string(),
                language: "javascript".to_string(),
                documentation: Some(format!(
                    "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/{name}"
                )),
            });
        }
    } else if language.eq_ignore_ascii_case("rust") {
        let rust_std = [
            "println",
            "eprintln",
            "format",
            "panic",
            "assert",
            "assert_eq",
            "vec",
            "String",
            "Box",
            "Rc",
            "Arc",
            "Option",
            "Result",
            "Some",
            "None",
            "Ok",
            "Err",
            "Drop",
            "Clone",
            "Copy",
        ];
        for name in rust_std {
            symbols.push(ExternalSymbol {
                name: name.to_string(),
                symbol_type: "std_macro_or_type".to_string(),
                language: "rust".to_string(),
                documentation: Some(format!("https://doc.rust-lang.org/std/?search={name}")),
            });
        }
    }

    symbols
}

/// The number of leading path components `a` and `b` share.
fn shared_components(a: &Path, b: &Path) -> usize {
    a.components()
        .zip(b.components())
        .take_while(|(x, y)| x == y)
        .count()
}

/// The file a module named by a relative path is, lexically: `./b.js` or
/// `../api/orders` against the directory of the importing file, `.` and `..`
/// resolved. `None` for a module named any other way.
fn module_file(module: &str, importing_file: &Path) -> Option<std::path::PathBuf> {
    if !(module.starts_with("./") || module.starts_with("../")) {
        return None;
    }
    let mut file = importing_file.parent()?.to_path_buf();
    for part in Path::new(module).components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                file.pop();
            }
            other => file.push(other.as_os_str()),
        }
    }
    Some(file)
}

/// The last segment of a module's name, which names its file or directory:
/// `dispatch` of `app.dispatch` and `.dispatch`, `shipping` of
/// `crate::shipping`, `Models` of `App\\Models`; of a path, its package's name
/// ([`go_package_name`]: `billing` of `github.com/acme/shop/billing`, `shop`
/// of `…/shop/v2`).
fn module_stem(module: &str) -> &str {
    if module.contains('/') {
        return go_package_name(module);
    }
    module
        .rsplit(['.', ':', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
}

/// Whether `candidate` is the file `module` names: the same path, or the same
/// without the extension the import left off, or the `index` of the directory
/// it names.
fn is_the_module_file(candidate: &Path, module: &Path) -> bool {
    if candidate == module || candidate.with_extension("") == module {
        return true;
    }
    if module.extension().is_some() && candidate.with_extension("") == module.with_extension("") {
        return true;
    }
    candidate.parent() == Some(module) && candidate.file_stem().is_some_and(|stem| stem == "index")
}

/// The languages `language`'s code calls directly, named by one of them: a
/// JVM's (Java, Kotlin, Scala), .NET's (C#, F#, VB.NET), C and those built on
/// it (C++, CUDA, Objective-C, and Swift through Objective-C), JavaScript's
/// dialects and the components written in them; any other language alone.
/// One language calls another's code only through a process, a network or a
/// foreign-function declaration of its own, which no name resolves.
#[must_use]
pub fn language_family(language: &str) -> &str {
    match language {
        "java" | "kotlin" | "scala" => "jvm",
        "c_sharp" | "fsharp" | "vb_net" => "dotnet",
        "c" | "cpp" | "cuda" | "objc" | "objective_c" | "swift" => "c",
        "javascript" | "typescript" | "tsx" | "vue" | "svelte" => "javascript",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Function, SourceLocation};
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn make_function(name: &str, file: &str, return_type: Option<&str>) -> Entity {
        let now = OffsetDateTime::now_utc();
        Entity::Function(Function {
            id: EntityId::from(Uuid::new_v4()),
            name: name.to_string(),
            file_path: file.to_string(),
            body: "def foo(): pass".to_string(),
            location: SourceLocation::new(1, 0, 2, 0),
            language: "python".to_string(),
            parent_function: None,
            parent_class: None,
            parameters: None,
            return_type: return_type.map(|s| s.to_string()),
            modifiers: None,
            access_modifiers: None,
            docstring: None,
            decorators: vec![],
            implements: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// `make_function` in `language`.
    fn in_language(mut entity: Entity, language: &str) -> Entity {
        if let Entity::Function(function) = &mut entity {
            function.language = language.to_string();
        }
        entity
    }

    /// A call resolves to a definition in a language whose code can call the
    /// caller's, and to no other: a shell script's `export_report` is no call
    /// of the Rust function, while Kotlin calls Java's and TypeScript
    /// JavaScript's.
    #[test]
    fn a_call_resolves_within_the_callers_language_family() {
        let mut table = SymbolTable::new();
        for (name, file, language) in [
            ("export_report", "lib/src/report.rs", "rust"),
            ("points_for", "src/Loyalty.java", "java"),
            ("format_cents", "web/money.js", "javascript"),
        ] {
            table.add_entity(&in_language(make_function(name, file, None), language));
        }
        let resolves = |name: &str, file: &str, language: &str| {
            !matches!(
                table.resolve_call_in(name, CallScope::Any, Path::new(file), language, 1),
                SymbolResolution::Unresolved | SymbolResolution::External { .. }
            )
        };
        assert!(!resolves("export_report", "scripts/run.sh", "bash"));
        assert!(!resolves("export_report", "tools/report.py", "python"));
        assert!(resolves("export_report", "lib/src/main.rs", "rust"));
        assert!(resolves("points_for", "src/Charge.kt", "kotlin"));
        assert!(!resolves("points_for", "lib/points.dart", "dart"));
        assert!(resolves("format_cents", "web/cart.ts", "typescript"));
        assert!(resolves("format_cents", "web/Cart.vue", "vue"));

        // A function passed as a value is looked for beside the caller only:
        // an argument that is a variable shares its name with functions
        // anywhere.
        table.add_entity(&in_language(
            make_function("handle_checkout", "api/handlers.go", None),
            "go",
        ));
        let passed = |file: &str| {
            !matches!(
                table.resolve_value_in("handle_checkout", CallScope::Any, Path::new(file), "go", 1),
                SymbolResolution::Unresolved | SymbolResolution::External { .. }
            )
        };
        assert!(passed("api/routes.go"));
        assert!(!passed("cmd/main.go"));
    }

    /// `make_function`, defined on `line`.
    fn at_line(mut entity: Entity, line: u32) -> Entity {
        if let Entity::Function(function) = &mut entity {
            function.location = SourceLocation::new(line, 0, line + 1, 0);
        }
        entity
    }

    /// Entries arrive in whatever order parallel parsing finishes. What an
    /// ambiguous name resolves to must not depend on it, and should be the
    /// nearest definitions.
    #[test]
    fn an_ambiguous_name_resolves_the_same_way_in_any_insertion_order() {
        let files = [
            "src/b/util.py",
            "src/a/util.py",
            "lib/util.py",
            "src/a/z/util.py",
        ];
        let entities: Vec<Entity> = files
            .iter()
            .map(|f| make_function("json", f, None))
            .collect();
        let caller = make_function("caller", "src/a/main.py", None);

        let mut resolved = Vec::new();
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1]] {
            let mut table = SymbolTable::new();
            table.add_entity(&caller);
            for i in order {
                table.add_entity(&entities[i]);
            }
            match table.resolve_symbol("json", Path::new("src/a/main.py"), 3) {
                SymbolResolution::Ambiguous { ids, total } => resolved.push((ids, total)),
                other => panic!("not ambiguous: {other:?}"),
            }
        }

        assert!(resolved.windows(2).all(|w| w[0] == w[1]), "{resolved:?}");
        // `src/a/util.py` and `src/a/z/util.py` share `src/a` with the caller,
        // more than the others; they tie, in path order.
        assert_eq!(resolved[0], (vec![entities[1].id(), entities[3].id()], 2));
    }

    /// Definitions that tie are ordered by file and line, never by their id
    /// first: the id hashes the absolute path, so it differs between two
    /// checkouts of one tree.
    #[test]
    fn overloads_are_candidates_in_line_order() {
        let first = at_line(make_function("add", "src/calc.py", None), 3);
        let second = at_line(make_function("add", "src/calc.py", None), 10);
        let caller = make_function("caller", "src/main.py", None);
        let mut table = SymbolTable::new();
        table.add_entity(&caller);
        table.add_entity(&second);
        table.add_entity(&first);

        for file in ["src/main.py", "src/calc.py"] {
            match table.resolve_symbol("add", Path::new(file), 3) {
                SymbolResolution::Ambiguous { ids, total } => {
                    assert_eq!(ids, vec![first.id(), second.id()], "from {file}");
                    assert_eq!(total, 2);
                }
                other => panic!("not ambiguous from {file}: {other:?}"),
            }
        }
        // A fan-out of 1 keeps the first and still reports the tie.
        match table.resolve_symbol("add", Path::new("src/main.py"), 1) {
            SymbolResolution::Ambiguous { ids, total } => {
                assert_eq!(ids, vec![first.id()]);
                assert_eq!(total, 2);
            }
            other => panic!("not ambiguous: {other:?}"),
        }
        assert_eq!(table.find_by_name("add").unwrap().id, first.id());
        assert_eq!(
            table
                .find_prefer_local("add", Path::new("src/main.py"))
                .unwrap()
                .id,
            first.id()
        );
    }

    /// A target-only entry is found by every lookup, but its own references
    /// are not resolved: it is not a source.
    #[test]
    fn test_symbol_entry_from_entity() {
        let f = make_function("my_func", "test.py", Some("MyClass"));
        let entry = SymbolEntry::from_entity(&f);
        assert_eq!(entry.name, "my_func");
        assert_eq!(entry.entity_type, "function");
    }

    /// A constant of a type is its member: used through a value named after
    /// the type, it is that type's, not a same-named constant of another type
    /// beside it, and through a value named after no type, none.
    #[test]
    fn a_constant_used_through_its_type_is_the_types() {
        let constant = |owner: &str, file: &str| {
            let now = OffsetDateTime::now_utc();
            Entity::Constant(crate::entities::Constant {
                id: EntityId::from(Uuid::new_v4()),
                name: "MAX_CELSIUS".to_string(),
                file_path: file.to_string(),
                body: "const val MAX_CELSIUS = 25.0".to_string(),
                location: SourceLocation::new(4, 0, 4, 30),
                language: "kotlin".to_string(),
                value: Some("25.0".to_string()),
                constant_type: None,
                parent_class: Some(owner.to_string()),
                docstring: None,
                created_at: now,
                updated_at: now,
            })
        };
        let reefer = constant("ReeferLimits", "reefer/ReeferLimits.kt");
        let other = constant("OtherLimits", "reefer/OtherLimits.kt");
        assert_eq!(
            SymbolEntry::from_entity(&reefer).owner.as_deref(),
            Some("ReeferLimits")
        );
        let mut table = SymbolTable::new();
        table.add_entity(&other);
        table.add_entity(&reefer);
        let through = |qualifier: &str| {
            table
                .resolve_value_in(
                    "MAX_CELSIUS",
                    CallScope::of(Some(qualifier), None, "MAX_CELSIUS", "kotlin"),
                    Path::new("reefer/SetPoint.kt"),
                    "kotlin",
                    3,
                )
                .entity_ids()
                .to_vec()
        };
        assert_eq!(through("ReeferLimits"), [reefer.id()]);
        assert_eq!(through("OtherLimits"), [other.id()]);
        assert!(through("setPoint").is_empty(), "a value of no known type");
    }

    /// The types a signature names are references of kind `Uses`.
    #[test]
    fn a_signature_type_is_a_uses_reference() {
        let f = make_function("my_func", "test.py", Some("MyClass"));
        let refs: Vec<SymbolRef> = type_refs(&f).collect();
        assert_eq!(
            refs.iter()
                .map(|r| (r.source_id, r.kind, r.target_name.as_str()))
                .collect::<Vec<_>>(),
            [(f.id(), RelationshipType::Uses, "MyClass")]
        );
    }

    #[test]
    fn test_symbol_table_add_and_resolve() {
        let mut table = SymbolTable::new();
        let f = make_function("helper", "a.py", None);
        table.add_entity(&f);

        let resolved = table.resolve_symbol("helper", Path::new("b.py"), 3);
        assert!(matches!(resolved, SymbolResolution::Entity(_)));
    }

    #[test]
    fn test_symbol_table_local_preference() {
        let mut table = SymbolTable::new();
        let f1 = make_function("foo", "a.py", None);
        let f2 = make_function("foo", "b.py", None);
        table.add_entity(&f1);
        table.add_entity(&f2);

        let resolved = table.resolve_symbol("foo", Path::new("b.py"), 3);
        match resolved {
            SymbolResolution::Entity(id) => {
                let entry = table.get_by_id(id).unwrap();
                assert_eq!(entry.file_path, PathBuf::from("b.py"));
            }
            _ => panic!("Expected Entity resolution"),
        }
    }

    #[test]
    fn test_symbol_table_import_resolution() {
        let mut table = SymbolTable::new();
        let f = make_function("real_name", "lib.py", None);
        table.add_entity(&f);
        table.add_imports(
            Path::new("main.py"),
            vec![ImportInfo {
                local_name: "alias".to_string(),
                source_module: "lib".to_string(),
                original_name: Some("real_name".to_string()),
                is_wildcard: false,
                line: 1,
            }],
        );

        let resolved = table.resolve_symbol("alias", Path::new("main.py"), 3);
        assert!(resolved.is_resolved());
    }

    /// When two files both define `bar`, `from foo import bar` must resolve to
    /// the one in `foo.py`, not the one in `other.py`.
    #[test]
    fn test_symbol_table_import_prefers_source_module() {
        let mut table = SymbolTable::new();
        // Two files both define `bar`
        let foo_bar = make_function("bar", "src/foo.py", None);
        let other_bar = make_function("bar", "src/other.py", None);
        table.add_entity(&foo_bar);
        table.add_entity(&other_bar);

        table.add_imports(
            Path::new("main.py"),
            vec![ImportInfo {
                local_name: "bar".to_string(),
                source_module: "foo".to_string(),
                original_name: None,
                is_wildcard: false,
                line: 1,
            }],
        );

        let resolved = table.resolve_symbol("bar", Path::new("main.py"), 3);
        match resolved {
            SymbolResolution::Entity(id) => {
                let entry = table.get_by_id(id).unwrap();
                assert!(
                    entry.file_path.to_string_lossy().contains("foo"),
                    "Expected resolution to foo.py, got {:?}",
                    entry.file_path
                );
            }
            _ => panic!("Expected Entity resolution, got {resolved:?}"),
        }
    }

    /// A locally-defined symbol must shadow an import with the same name.
    #[test]
    fn test_symbol_table_local_shadows_import() {
        let mut table = SymbolTable::new();
        let local_foo = make_function("foo", "app.py", None);
        let other_foo = make_function("foo", "lib.py", None);
        table.add_entity(&local_foo);
        table.add_entity(&other_foo);

        table.add_imports(
            Path::new("app.py"),
            vec![ImportInfo {
                local_name: "foo".to_string(),
                source_module: "lib".to_string(),
                original_name: None,
                is_wildcard: false,
                line: 1,
            }],
        );

        // `foo` is defined locally in app.py AND imported from lib — local wins.
        let resolved = table.resolve_symbol("foo", Path::new("app.py"), 3);
        match resolved {
            SymbolResolution::Entity(id) => {
                let entry = table.get_by_id(id).unwrap();
                assert_eq!(
                    entry.file_path,
                    PathBuf::from("app.py"),
                    "Local definition must shadow import"
                );
            }
            _ => panic!("Expected Entity resolution, got {resolved:?}"),
        }
    }

    #[test]
    fn test_symbol_table_memory_size() {
        // Sanity check: SymbolEntry should be much smaller than Entity
        let f = make_function("test", "t.py", Some("List[str]"));
        let entry = SymbolEntry::from_entity(&f);
        let entry_size = std::mem::size_of_val(&entry);
        let entity_size = std::mem::size_of_val(&f);
        // SymbolEntry should be significantly smaller (exact sizes vary by
        // platform, but the body string dominates the Entity size).
        assert!(
            entry_size < entity_size,
            "SymbolEntry ({entry_size}) should be smaller than Entity ({entity_size})"
        );
    }
}
