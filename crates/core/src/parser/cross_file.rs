//! What cross-file resolution needs besides the symbol table: the analysis
//! mode, the result of resolving a name, external symbols and the common
//! built-in names.
//!
//! References are stored with their entities at parse time (`symbol_refs`),
//! and every edge between entities is derived from them by
//! [`crate::graph::resolve`], against the definitions in the database.

use crate::entities::EntityId;
use crate::parser::symbol_table::SymbolEntry;
use std::collections::HashSet;
use std::sync::LazyLock;

/// External symbol information (standard library, built-ins)
#[derive(Debug, Clone)]
pub struct ExternalSymbol {
    /// Symbol name
    pub name: String,
    /// Symbol type (function, class, module, etc.)
    pub symbol_type: String,
    /// Language
    pub language: String,
    /// Documentation/URL
    pub documentation: Option<String>,
}

/// Result of symbol resolution
#[derive(Debug, Clone)]
pub enum SymbolResolution {
    /// Resolved to an entity
    Entity(EntityId),
    /// Several definitions are equally good: the first of them, in a fixed
    /// order, up to the fan-out cap.
    Ambiguous {
        /// The candidates, best first.
        ids: Vec<EntityId>,
        /// How many definitions tied, before the cap.
        total: usize,
    },
    /// External symbol (standard library)
    External {
        /// Module or namespace containing the symbol
        module: String,
        /// Symbol name
        name: String,
    },
    /// Could not be resolved
    Unresolved,
}

impl SymbolResolution {
    /// The resolution to `candidates`, as returned with their untruncated
    /// count by the symbol table; `None` when there are none.
    pub(crate) fn of_candidates(candidates: (Vec<&SymbolEntry>, usize)) -> Option<Self> {
        let (entries, total) = candidates;
        match entries.as_slice() {
            [] => None,
            [only] if total == 1 => Some(Self::Entity(only.id)),
            _ => Some(Self::Ambiguous {
                ids: entries.iter().map(|entry| entry.id).collect(),
                total,
            }),
        }
    }

    /// Check if resolved (either entity or external)
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        !matches!(self, SymbolResolution::Unresolved)
    }

    /// Get entity ID if resolved to entity: for an ambiguous name, the first
    /// candidate.
    #[must_use]
    pub fn entity_id(&self) -> Option<EntityId> {
        self.entity_ids().first().copied()
    }

    /// Every entity it resolved to, best first: one, or the candidates of an
    /// ambiguous name. Empty for an external or unresolved symbol.
    #[must_use]
    pub fn entity_ids(&self) -> &[EntityId] {
        match self {
            SymbolResolution::Entity(id) => std::slice::from_ref(id),
            SymbolResolution::Ambiguous { ids, .. } => ids,
            SymbolResolution::External { .. } | SymbolResolution::Unresolved => &[],
        }
    }

    /// How many definitions the name could mean: 1 when resolved to an
    /// entity, the untruncated count when ambiguous, 0 otherwise.
    #[must_use]
    pub fn candidates(&self) -> usize {
        match self {
            SymbolResolution::Entity(_) => 1,
            SymbolResolution::Ambiguous { total, .. } => *total,
            SymbolResolution::External { .. } | SymbolResolution::Unresolved => 0,
        }
    }
}

/// Cross-file analysis mode: which stored references resolve across files.
/// A reference that does not still links a definition in its own file.
///
/// | Mode | Type uses, supertypes | Calls |
/// |------|-----------------------|-------|
/// | None | No | No |
/// | Light | Yes | No |
/// | Deep | Yes | Yes |
/// | Auto | Adaptive | Adaptive |
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrossFileMode {
    /// Skip cross-file analysis entirely
    None,
    /// Resolve every reference but calls across files: `Uses`, `Inherits`,
    /// `Implements`
    Light,
    /// Resolve every reference, calls included
    Deep,
    /// `Light` for a project of more than [`CrossFileMode::LIGHT_ABOVE`]
    /// entities, `Deep` otherwise
    Auto,
}

impl CrossFileMode {
    /// The entity count above which `Auto` stands for `Light`.
    pub const LIGHT_ABOVE: usize = 500_000;

    /// The mode this one stands for in a project of `entities` entities:
    /// `Auto` becomes `Light` or `Deep`, any other mode is itself.
    #[must_use]
    pub const fn effective(self, entities: usize) -> Self {
        match self {
            Self::Auto if entities > Self::LIGHT_ABOVE => Self::Light,
            Self::Auto => Self::Deep,
            other => other,
        }
    }
}

/// Static built-in sets, created once at first access.
static PYTHON_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
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
    ])
});

static JS_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
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
        "document",
        "window",
        "fetch",
        "require",
        "module",
        "exports",
    ])
});

static RUST_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "println", "eprintln", "format", "vec", "Box", "Option", "Result", "String", "Vec",
        "HashMap", "BTreeMap", "Rc", "Arc",
    ])
});

static GO_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "fmt", "Println", "Printf", "Sprint", "len", "cap", "make", "new",
    ])
});

static JAVA_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "System", "out", "println", "String", "Object", "Math", "Integer",
    ])
});

static C_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "printf", "scanf", "malloc", "free", "sizeof", "strlen", "strcpy",
    ])
});

static RUBY_BUILTINS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["puts", "print", "p", "require", "include", "extend"]));

static PHP_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "echo", "print", "isset", "unset", "array", "count", "strlen",
    ])
});

/// SQL's aggregate, window and common scalar functions, lower-cased: SQL
/// names are case-blind, `COUNT` as much as `count`.
static SQL_BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "row_number",
        "rank",
        "dense_rank",
        "ntile",
        "lag",
        "lead",
        "first_value",
        "last_value",
        "coalesce",
        "nullif",
        "cast",
        "lower",
        "upper",
        "length",
        "substring",
        "trim",
        "round",
        "abs",
        "now",
        "concat",
        "extract",
        "date_trunc",
        "greatest",
        "least",
        "string_agg",
        "array_agg",
        "json_agg",
    ])
});

static COMMON_BUILTINS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["assert", "panic", "unwrap", "expect"]));

/// Check if a name is a common built-in function of `language`. An empty
/// `language`, or one with no list of its own, is checked against every list.
pub fn is_common_builtin(name: &str, language: &str) -> bool {
    if language.eq_ignore_ascii_case("python") {
        PYTHON_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("javascript")
        || language.eq_ignore_ascii_case("typescript")
        || language.eq_ignore_ascii_case("tsx")
        || language.eq_ignore_ascii_case("vue")
        || language.eq_ignore_ascii_case("svelte")
    {
        JS_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("rust") {
        RUST_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("go") {
        GO_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("java") {
        JAVA_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("c")
        || language.eq_ignore_ascii_case("cpp")
        || language.eq_ignore_ascii_case("c++")
        || language.eq_ignore_ascii_case("cuda")
        || language.eq_ignore_ascii_case("objc")
        || language.eq_ignore_ascii_case("objective_c")
    {
        C_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("ruby") {
        RUBY_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("php") {
        PHP_BUILTINS.contains(name)
    } else if language.eq_ignore_ascii_case("sql") {
        SQL_BUILTINS.contains(name.to_ascii_lowercase().as_str())
    } else {
        // Any language, or one with no list of its own: every list.
        PYTHON_BUILTINS.contains(name)
            || JS_BUILTINS.contains(name)
            || RUST_BUILTINS.contains(name)
            || GO_BUILTINS.contains(name)
            || JAVA_BUILTINS.contains(name)
            || C_BUILTINS.contains(name)
            || RUBY_BUILTINS.contains(name)
            || PHP_BUILTINS.contains(name)
            || COMMON_BUILTINS.contains(name)
    }
}
