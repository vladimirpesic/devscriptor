//! Dead Code Detection for Devscriptor
//!
//! This module provides functionality to detect unused functions and classes
//! across the codebase by analyzing call relationships and usage patterns.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analysis::api::AnalysisSummary;
use crate::analysis::{Finding, Severity};
use crate::database::Database;
use crate::error::Result;

/// Confidence that a non-public, unreferenced entity is genuinely dead.
///
/// Nothing inside this codebase can reach it, so the only remaining doubt is
/// dynamic dispatch / reflection.
pub const CONFIDENCE_PRIVATE: f64 = 0.9;

/// Confidence that a **public**, unreferenced entity is genuinely dead.
///
/// Lower, because a public symbol may be part of the crate/package's external
/// API and consumed outside the scanned codebase.
pub const CONFIDENCE_PUBLIC: f64 = 0.5;

/// Confidence that an unreferenced entity whose name is written outside its
/// own definition — in another file, or elsewhere in its own — is genuinely
/// dead: the name may reach it in a way no reference records (another
/// language calling it, a template, a program a script runs, a string).
pub const CONFIDENCE_NAMED_ELSEWHERE: f64 = 0.3;

/// Confidence that an unreferenced method of a type extending one the index
/// does not hold is genuinely dead: it may implement a member of that type,
/// which code outside the index calls (a framework's base class, a standard
/// interface).
pub const CONFIDENCE_UNINDEXED_SUPERTYPE: f64 = 0.3;

/// Files larger than this are not scanned for names: generated or vendored
/// code, whose mentions say little.
const MAX_SCANNED_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Information about an unused entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnusedEntity {
    /// Entity name
    pub name: String,
    /// File path
    pub file_path: String,
    /// Line number
    pub line: u32,
    /// Programming language
    pub language: String,
    /// Entity type
    pub entity_type: String,
    /// Reason for being flagged
    pub reason: String,
    /// Confidence in `[0.0, 1.0]` that the entity is genuinely dead.
    ///
    /// Derived from visibility — a public symbol may be used by consumers
    /// outside the scanned codebase, so it scores lower than a private one —
    /// and lower still when its name is written outside its definition. See
    /// [`CONFIDENCE_PRIVATE`], [`CONFIDENCE_PUBLIC`],
    /// [`CONFIDENCE_NAMED_ELSEWHERE`] and [`CONFIDENCE_UNINDEXED_SUPERTYPE`].
    pub confidence: f64,
}

/// Confidence for an unreferenced entity, given its visibility.
#[must_use]
pub const fn confidence_for_visibility(is_public: bool) -> f64 {
    if is_public {
        CONFIDENCE_PUBLIC
    } else {
        CONFIDENCE_PRIVATE
    }
}

/// Comprehensive dead code analysis report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeReport {
    /// Analysis summary
    pub summary: AnalysisSummary,
    /// Unused functions
    pub dead_functions: Vec<UnusedEntity>,
    /// Unused classes
    pub dead_classes: Vec<UnusedEntity>,
    /// Unused files
    pub dead_files: Vec<UnusedEntity>,
    /// Unused imports
    pub unused_imports: Vec<UnusedEntity>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

impl DeadCodeReport {
    /// Create a new empty report
    #[must_use]
    pub fn new() -> Self {
        Self {
            summary: AnalysisSummary::new(),
            dead_functions: Vec::new(),
            dead_classes: Vec::new(),
            dead_files: Vec::new(),
            unused_imports: Vec::new(),
            recommendations: Vec::new(),
        }
    }

    /// Add an unused function
    pub fn add_dead_function(&mut self, entity: UnusedEntity) {
        self.dead_functions.push(entity);
        self.summary.add_finding(Severity::Medium);
    }

    /// Add an unused class
    pub fn add_dead_class(&mut self, entity: UnusedEntity) {
        self.dead_classes.push(entity);
        self.summary.add_finding(Severity::Medium);
    }

    /// Add an unused file
    pub fn add_dead_file(&mut self, entity: UnusedEntity) {
        self.dead_files.push(entity);
        self.summary.add_finding(Severity::Low);
    }

    /// Generate recommendations based on findings
    pub fn generate_recommendations(&mut self) {
        let mut recs = Vec::new();

        if !self.dead_functions.is_empty() {
            recs.push(format!(
                "Consider removing {} unused functions to reduce code maintenance burden",
                self.dead_functions.len()
            ));
        }

        if !self.dead_classes.is_empty() {
            recs.push(format!(
                "Review {} unused classes - they may be candidates for removal",
                self.dead_classes.len()
            ));
        }

        if !self.dead_files.is_empty() {
            recs.push(format!(
                "{} files appear to be unused and could be deleted",
                self.dead_files.len()
            ));
        }

        if recs.is_empty() {
            recs.push("No dead code detected - your codebase is clean!".to_string());
        }

        self.recommendations = recs;
    }

    /// Convert findings to analysis findings
    #[must_use]
    pub fn to_findings(&self) -> Vec<Finding> {
        let mut findings = Vec::new();

        for func in &self.dead_functions {
            findings.push(
                Finding::new(
                    Severity::Medium,
                    format!("Unused function: {}", func.name),
                    &func.file_path,
                    func.line,
                    &func.name,
                    &func.entity_type,
                )
                .with_suggestion("Consider removing this unused function"),
            );
        }

        for cls in &self.dead_classes {
            findings.push(
                Finding::new(
                    Severity::Medium,
                    format!("Unused class: {}", cls.name),
                    &cls.file_path,
                    cls.line,
                    &cls.name,
                    &cls.entity_type,
                )
                .with_suggestion("Consider removing this unused class"),
            );
        }

        findings
    }
}

impl Default for DeadCodeReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Detector for identifying unused code in a codebase
#[derive(Clone)]
pub struct DeadCodeDetector {
    /// Database reference
    database: std::sync::Arc<Database>,
    /// The directory analyzed — the entities under it; the whole index when
    /// `None`.
    root: Option<PathBuf>,
    /// Functions loaded from database
    functions: Vec<EntityInfo>,
    /// Classes loaded from database
    classes: Vec<EntityInfo>,
    /// Call graph: caller -> callees
    call_graph: HashMap<String, HashSet<String>>,
    /// Usage graph: user -> used entities
    usage_graph: HashMap<String, HashSet<String>>,
    /// Entry points: what a runtime, a framework or a test runner calls, and
    /// nothing in the code by name.
    entry_points: HashSet<String>,
    /// Set of all entities that are referenced as a target in any relationship.
    /// Looked up in O(1) by `is_entity_used`.
    used_entities: HashSet<String>,
    /// The members reached through a type they override or implement (see
    /// [`Hierarchy::dispatched`]).
    dispatched: HashSet<String>,
}

/// Entity information for analysis
#[derive(Debug, Clone)]
struct EntityInfo {
    id: String,
    name: String,
    file_path: String,
    /// Its file's path within its project (see [`path_in_project`]).
    path_in_project: PathBuf,
    start_line: u32,
    end_line: u32,
    language: String,
    /// Its access keywords, lower case (`public`, `pub`, `export`, …).
    access: String,
    /// Its other modifiers, lower case (`static`, `override`, …).
    modifiers: String,
    /// Its decorators and annotations, by name (`app.route`, `tokio::test`).
    decorators: Vec<String>,
    /// The type it is a member of.
    owner: Option<String>,
}

/// Decorators, annotations and attributes that say nothing of who calls what
/// they mark — they describe it, tune it or derive code from it — by the last
/// segment of their name (`functools.lru_cache` is `lru_cache`). Any other
/// marks what a framework registers or a runtime calls (`@app.route`,
/// `@Test`, `@GetMapping`, `@pytest.fixture`, `@Bean`, `@objc`), as does a
/// name that is both (Spring's `@Value` injects what Lombok's describes).
const NON_ENTRY_DECORATORS: &[&str] = &[
    // Python
    "staticmethod",
    "classmethod",
    "abstractmethod",
    "lru_cache",
    "cache",
    "wraps",
    "dataclass",
    "total_ordering",
    "unique",
    "final",
    "overload",
    "contextmanager",
    "asynccontextmanager",
    "singledispatch",
    "deprecated",
    // Rust
    "allow",
    "warn",
    "deny",
    "forbid",
    "expect",
    "cfg",
    "cfg_attr",
    "derive",
    "doc",
    "inline",
    "cold",
    "must_use",
    "repr",
    "non_exhaustive",
    "track_caller",
    // Java, Kotlin, Scala, Dart
    "Deprecated",
    "SuppressWarnings",
    "Suppress",
    "SafeVarargs",
    "FunctionalInterface",
    "Nullable",
    "NonNull",
    "NotNull",
    "VisibleForTesting",
    "visibleForTesting",
    "JvmStatic",
    "JvmOverloads",
    "JvmField",
    "JvmName",
    "Throws",
    "throws",
    "tailrec",
    "immutable",
    "mustCallSuper",
    // Lombok
    "Data",
    "Getter",
    "Setter",
    "Builder",
    "ToString",
    "EqualsAndHashCode",
    "NoArgsConstructor",
    "AllArgsConstructor",
    "RequiredArgsConstructor",
    // C#
    "Obsolete",
    "Serializable",
    "Flags",
    "MethodImpl",
    "Pure",
    "DebuggerStepThrough",
    "DebuggerDisplay",
    "DebuggerHidden",
    // Swift
    "discardableResult",
    "available",
    "inlinable",
    "usableFromInline",
    "frozen",
    "MainActor",
    "Sendable",
];

/// Names a runtime calls on its own: Python's dunders aside, constructors and
/// initializers, `main`, operator methods (Scala's `apply`, Kotlin's
/// `invoke`), the methods every object has (`toString`, `equals`), and
/// Solidity's `receive`/`fallback`.
const RUNTIME_NAMES: &[&str] = &[
    "constructor",
    "init",
    "deinit",
    "initialize",
    "__construct",
    "New",
    "main",
    "apply",
    "unapply",
    "invoke",
    "subscript",
    "toString",
    "equals",
    "hashCode",
    "compareTo",
    "Dispose",
    "finalize",
    "receive",
    "fallback",
];

/// The methods a language's runtime or standard library calls through a
/// protocol no supertype names: Go's interfaces are met, not declared
/// (`fmt.Stringer`'s `String`, `io.Reader`'s `Read`, `sort.Interface`'s
/// `Len`, `json.Marshaler`'s `MarshalJSON`, `http.Handler`'s `ServeHTTP`);
/// Ruby's and JavaScript's conversions, comparisons and iteration go by name
/// alone (`to_s`, `<=>`, `each`, `valueOf`, `toJSON`).
fn protocol_methods(language: &str) -> &'static [&'static str] {
    match language {
        "go" => &[
            "As",
            "Close",
            "Error",
            "Format",
            "GoString",
            "Is",
            "Len",
            "Less",
            "MarshalBinary",
            "MarshalJSON",
            "MarshalText",
            "Pop",
            "Push",
            "Read",
            "ReadFrom",
            "Scan",
            "Seek",
            "ServeHTTP",
            "String",
            "Swap",
            "UnmarshalBinary",
            "UnmarshalJSON",
            "UnmarshalText",
            "Unwrap",
            "Value",
            "Write",
            "WriteTo",
        ],
        "ruby" => &[
            "<=>",
            "==",
            "call",
            "coerce",
            "each",
            "eql?",
            "hash",
            "inspect",
            "method_missing",
            "respond_to_missing?",
            "to_ary",
            "to_proc",
            "to_s",
            "to_str",
        ],
        "javascript" | "typescript" | "tsx" | "vue" | "svelte" => &[
            "adoptedCallback",
            "attributeChangedCallback",
            "connectedCallback",
            "disconnectedCallback",
            "handleEvent",
            "toJSON",
            "valueOf",
        ],
        _ => &[],
    }
}

/// The languages in which a method implements a supertype's member only
/// where it says so: Rust's `impl Trait for`, F#'s `interface … with` and
/// `override`, VB.NET's `Implements` and `Overrides`, Kotlin's `override`.
/// Elsewhere — a Java, C#, TypeScript, Swift or Python method — one may
/// implement an interface's member unmarked.
const MARKED_IMPLEMENTATIONS: &[&str] = &["rust", "fsharp", "vb_net", "kotlin"];

/// The parts of a path that say a framework calls what the file holds: its
/// routes, controllers, views, resolvers, jobs and the like.
const ROLE_WORDS: &[&str] = &[
    "main",
    "app",
    "application",
    "server",
    "api",
    "endpoint",
    "endpoints",
    "route",
    "routes",
    "router",
    "routers",
    "handler",
    "handlers",
    "controller",
    "controllers",
    "view",
    "views",
    "resolver",
    "resolvers",
    "mailer",
    "mailers",
    "job",
    "jobs",
    "worker",
    "workers",
    "command",
    "commands",
    "listener",
    "listeners",
    "consumer",
    "consumers",
    "urls",
];

/// The endings of a class name that say a framework instantiates it.
const ROLE_CLASS_SUFFIXES: &[&str] = &[
    "Controller",
    "Resource",
    "Handler",
    "Mailer",
    "Job",
    "Worker",
    "Consumer",
    "Listener",
    "Middleware",
    "Command",
    "Servlet",
    "Activity",
    "Fragment",
    "Test",
    "Tests",
    "Spec",
];

impl EntityInfo {
    /// Whether code outside the analyzed files may use it: what the language
    /// exports — Rust's `pub`, Go's capitalized names, Python's and Dart's
    /// names without `_`, a JavaScript `export` or a class member, a Java, C#,
    /// Swift or VB.NET `public`/`protected`, a C-family function not `static`,
    /// a Solidity `public`/`external` — and in the languages public by default,
    /// anything not `private` or `internal`.
    fn is_exported(&self) -> bool {
        let access = |token: &str| {
            self.access
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word == token)
        };
        match self.language.as_str() {
            "rust" => access("pub"),
            "go" => self.name.chars().next().is_some_and(char::is_uppercase),
            "python" | "dart" => !self.name.starts_with('_'),
            "javascript" | "typescript" | "tsx" | "vue" | "svelte" => {
                access("export")
                    || (self.owner.is_some() && !access("private") && !self.name.starts_with('#'))
            }
            "java" | "c_sharp" | "vb_net" => access("public") || access("protected"),
            "swift" => access("public") || access("open"),
            "c" | "cpp" | "cuda" | "objc" => !self
                .modifiers
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word == "static"),
            "solidity" => access("public") || access("external"),
            _ => !(access("private") || access("internal") || access("fileprivate")),
        }
    }

    /// Whether it is test code, which its test runner calls: in a test file
    /// or directory, or named as a test (`test_total`, `testTotal`, Go's
    /// `TestTotal`). A test annotation (`#[test]`, `@Test`) registers what it
    /// marks as any other does (see [`Self::has_entry_decorator`]).
    fn is_test_code(&self) -> bool {
        let path = &self.path_in_project;
        let in_test_dir = path.components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some("test" | "tests" | "spec" | "specs" | "__tests__" | "testing" | "Tests")
            )
        });
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let test_file = stem.starts_with("test_")
            || stem.ends_with("_test")
            || stem.ends_with("_spec")
            || stem.ends_with(".test")
            || stem.ends_with(".spec")
            || stem.ends_with("Test")
            || stem.ends_with("Tests");
        let test_name = names_a_test(&self.name, "test")
            || (self.language == "go"
                && ["Test", "Benchmark", "Example", "Fuzz"]
                    .iter()
                    .any(|prefix| names_a_test(&self.name, prefix)));
        in_test_dir || test_file || test_name
    }

    /// Whether a decorator or annotation registers it with a framework.
    fn has_entry_decorator(&self) -> bool {
        self.decorators.iter().any(|decorator| {
            let name = decorator.rsplit(['.', ':']).next().unwrap_or(decorator);
            !NON_ENTRY_DECORATORS.contains(&name)
        })
    }

    /// Whether it overrides a member of a type it extends, through which it
    /// is called.
    fn overrides(&self) -> bool {
        self.modifiers
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "override" || word == "overrides")
            || self
                .decorators
                .iter()
                .any(|decorator| decorator.eq_ignore_ascii_case("override"))
    }

    /// Whether the runtime calls it by its name alone.
    fn is_runtime_invoked(&self) -> bool {
        (self.name.starts_with("__") && self.name.ends_with("__") && self.name.len() > 4)
            || RUNTIME_NAMES.contains(&self.name.as_str())
            || protocol_methods(&self.language).contains(&self.name.as_str())
            // A constructor named after its class.
            || self.owner.as_deref() == Some(self.name.as_str())
    }

    /// Whether its language's keywords or naming make it private: `private`,
    /// `fileprivate`, a leading `_`, a `#` field.
    fn is_private(&self) -> bool {
        self.access
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| matches!(word, "private" | "fileprivate"))
            || self.name.starts_with('_')
            || self.name.starts_with('#')
    }

    /// Whether it is a getter or a setter (`get total()`, `int get total`).
    fn is_accessor(&self) -> bool {
        self.modifiers
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "get" || word == "set")
    }

    /// Whether its file's path names a framework role (see [`ROLE_WORDS`]).
    fn in_role_file(&self) -> bool {
        self.path_in_project.components().any(|part| {
            let part = part.as_os_str().to_string_lossy();
            let stem = part.split('.').next().unwrap_or(&part).to_lowercase();
            ROLE_WORDS.contains(&stem.as_str())
        })
    }
}

/// `file`'s path within its project: under the outermost of `roots` — the
/// directories scanned into the index — that holds it, else all of it. A
/// project's own directories say what its files are; those above it do not:
/// read whole, a path under `/home/me/test/` made all of a project test code,
/// and one under `/srv/app/` all of it a framework's.
fn path_in_project(file: &str, roots: &[PathBuf]) -> PathBuf {
    let file = Path::new(file);
    roots
        .iter()
        .filter_map(|root| file.strip_prefix(root).ok())
        .max_by_key(|within| within.components().count())
        .map_or_else(|| file.to_path_buf(), Path::to_path_buf)
}

/// Whether `name` is `prefix`, or `prefix` and then a word of its own — `test`,
/// `test_total`, `testTotal`, `TestTotal`, `Test1` — not a word beginning with
/// its letters (`testimonials`, `Testify`).
fn names_a_test(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_lowercase()))
}

/// The types each type extends or implements, and their members: what a
/// call through a supertype may reach. By name, as references name types.
#[derive(Debug, Default)]
struct Hierarchy {
    /// Each type's direct supertypes, as its references name them — those
    /// the index holds and those it does not (`BaseHTTPRequestHandler`,
    /// `Runnable`, Rust's `Display`).
    supertypes: HashMap<String, Vec<String>>,
    /// The names of the types the index holds.
    types: HashSet<String>,
    /// Every function of a type, or that implements one: its id, name, type
    /// and the type its definition says it implements a member of.
    members: Vec<(String, String, Option<String>, Option<String>)>,
}

impl Hierarchy {
    /// Read the hierarchy of the whole index, as a call anywhere may reach a
    /// member through it.
    ///
    /// # Errors
    /// Returns an error if a database read fails.
    async fn load(database: &Database) -> Result<Self> {
        let pool = database.conn_pool().clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut hierarchy = Self::default();
            let mut stmt = conn.prepare(
                "SELECT e.name, r.target_name FROM symbol_refs r \
                 JOIN entities e ON e.id = r.source_id \
                 WHERE r.kind IN ('inherits', 'implements')",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            for row in rows {
                let (ty, supertype): (String, String) = row?;
                hierarchy.supertypes.entry(ty).or_default().push(supertype);
            }
            let mut stmt = conn.prepare(
                "SELECT DISTINCT name FROM entities \
                 WHERE type IN ('class', 'interface', 'trait', 'struct', 'contract')",
            )?;
            hierarchy.types = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut stmt = conn.prepare(
                "SELECT id, name, json_extract(data, '$.parent_class'), \
                        json_extract(data, '$.implements') \
                 FROM entities WHERE type = 'function' \
                   AND (json_extract(data, '$.parent_class') IS NOT NULL \
                        OR json_extract(data, '$.implements') IS NOT NULL)",
            )?;
            hierarchy.members = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok::<_, crate::error::Error>(hierarchy)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))?
    }

    /// The types `ty` extends or implements, directly or through another,
    /// each once, nearest first; not `ty` itself.
    fn supertypes_of<'a>(&'a self, ty: &'a str) -> Vec<&'a str> {
        let mut seen: HashSet<&str> = HashSet::from([ty]);
        let mut lineage = Vec::new();
        let mut next = vec![ty];
        while let Some(current) = next.pop() {
            for supertype in self.supertypes.get(current).into_iter().flatten() {
                if seen.insert(supertype) {
                    lineage.push(supertype.as_str());
                    next.push(supertype);
                }
            }
        }
        lineage
    }

    /// A type `ty` extends or implements, directly or through another, that
    /// the index does not hold.
    fn unindexed_supertype<'a>(&'a self, ty: &'a str) -> Option<&'a str> {
        self.supertypes_of(ty)
            .into_iter()
            .find(|supertype| !self.types.contains(*supertype))
    }

    /// The ids of the members reached through a type: a member, those it
    /// overrides or implements — of the same name, in the types its own type
    /// extends and the one its definition names — and theirs are one group,
    /// as a call of any may run each, and a group is reached when `reached`
    /// holds any of its members. A member that implements one of a type the
    /// index does not hold is reached too: code outside the index calls it
    /// through that type (Rust's `fmt` of `impl Display`, `drop` of
    /// `impl Drop`).
    fn dispatched(&self, reached: &HashSet<String>) -> HashSet<String> {
        let mut by_type: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for (index, (_, name, owner, _)) in self.members.iter().enumerate() {
            if let Some(owner) = owner {
                by_type.entry((owner, name)).or_default().push(index);
            }
        }
        let mut parent: Vec<usize> = (0..self.members.len()).collect();
        fn root(parent: &mut [usize], mut at: usize) -> usize {
            while parent[at] != at {
                parent[at] = parent[parent[at]];
                at = parent[at];
            }
            at
        }
        let mut reached_roots: HashSet<usize> = HashSet::new();
        for (index, (id, name, owner, implements)) in self.members.iter().enumerate() {
            let mut supertypes: Vec<&str> = owner
                .as_deref()
                .map(|owner| self.supertypes_of(owner))
                .unwrap_or_default();
            if let Some(implemented) = implements.as_deref() {
                supertypes.push(implemented);
                supertypes.extend(self.supertypes_of(implemented));
            }
            for supertype in supertypes {
                for &other in by_type
                    .get(&(supertype, name.as_str()))
                    .into_iter()
                    .flatten()
                {
                    let (a, b) = (root(&mut parent, index), root(&mut parent, other));
                    parent[a] = b;
                }
            }
            let outside = implements
                .as_deref()
                .is_some_and(|implemented| !self.types.contains(implemented));
            if outside || reached.contains(id) {
                reached_roots.insert(index);
            }
        }
        let reached_roots: HashSet<usize> = reached_roots
            .into_iter()
            .map(|index| root(&mut parent, index))
            .collect();
        (0..self.members.len())
            .filter(|&index| reached_roots.contains(&root(&mut parent, index)))
            .map(|index| self.members[index].0.clone())
            .collect()
    }
}

impl DeadCodeDetector {
    /// Create a new dead code detector over the whole index.
    pub fn new(database: std::sync::Arc<Database>) -> Self {
        Self {
            database,
            root: None,
            functions: Vec::new(),
            classes: Vec::new(),
            call_graph: HashMap::new(),
            usage_graph: HashMap::new(),
            entry_points: HashSet::new(),
            used_entities: HashSet::new(),
            dispatched: HashSet::new(),
        }
    }

    /// Analyze only the entities under `root` — canonical, as the index
    /// stores paths, when it exists. What they are used by is looked for
    /// everywhere.
    #[must_use]
    pub fn within(mut self, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        self.root = Some(dunce::canonicalize(&root).unwrap_or(root));
        self
    }

    /// Analyze the codebase to detect dead code
    ///
    /// # Returns
    /// Comprehensive dead code analysis report
    ///
    /// # Errors
    /// Returns an error if database operations fail
    pub async fn analyze(&mut self) -> Result<DeadCodeReport> {
        let start_time = std::time::Instant::now();
        let mut report = DeadCodeReport::new();

        // Load all entities and relationships
        self.load_entities().await?;
        self.build_relationship_graphs().await?;
        self.identify_entry_points();
        // A call of a member may run what overrides or implements it.
        let hierarchy = Hierarchy::load(&self.database).await?;
        let reached: HashSet<String> = self
            .used_entities
            .union(&self.entry_points)
            .cloned()
            .collect();
        self.dispatched = hierarchy.dispatched(&reached);

        // Find unused entities
        let mut dead_functions = self.find_unused_functions(&hierarchy);
        let mut dead_classes = self.find_unused_classes();

        // An unreferenced entity whose name is written outside its own
        // definition may be reached in a way no reference records.
        let mentions = self
            .mentions_outside_definitions(&dead_functions, &dead_classes)
            .await?;
        for (entity, mention) in dead_functions
            .iter_mut()
            .chain(dead_classes.iter_mut())
            .zip(mentions)
        {
            if let Some((file, line)) = mention {
                entity.confidence = entity.confidence.min(CONFIDENCE_NAMED_ELSEWHERE);
                entity.reason = format!(
                    "{}, though its name is written at {file}:{line}: it may be reached in a \
                     way no reference records (another language, a template, a program a \
                     script runs, a string)",
                    entity.reason
                );
            }
        }

        for func in dead_functions {
            report.add_dead_function(func);
        }

        for cls in dead_classes {
            report.add_dead_class(cls);
        }

        // Generate recommendations
        report.generate_recommendations();

        // Update summary
        report.summary.duration_ms = start_time.elapsed().as_millis() as u64;
        report.summary.entities_analyzed = self.functions.len() + self.classes.len();

        Ok(report)
    }

    /// Load the functions and classes analyzed — under the root, when there
    /// is one — with what decides who may use them: their access, modifiers,
    /// decorators, the type they are a member of and their file's path within
    /// its project.
    ///
    /// # Errors
    /// Returns an error if database operations fail
    async fn load_entities(&mut self) -> Result<()> {
        let pool = self.database.conn_pool().clone();
        let range = self
            .root
            .as_deref()
            .map(crate::database::operations::path_range);
        let roots = self.database.indexed_roots().await?;

        let (functions, classes): (Vec<EntityInfo>, Vec<EntityInfo>) =
            tokio::task::spawn_blocking(move || {
                let conn = pool.get()?;
                let scope = if range.is_some() {
                    "AND file_path >= ?1 AND file_path < ?2"
                } else {
                    ""
                };
                let mut stmt = conn.prepare(&format!(
                    "SELECT id, name, file_path, start_line, end_line, language, type,
                            COALESCE(json_extract(data, '$.access_modifiers'), ''),
                            COALESCE(json_extract(data, '$.modifiers'), ''),
                            COALESCE(json_extract(data, '$.decorators'), '[]'),
                            json_extract(data, '$.parent_class')
                     FROM entities WHERE type IN ('function', 'class')
                       -- Declarative/schema/markup languages have no call graph,
                       -- so 'unused function' is meaningless (a GraphQL field, HTML
                       -- element, or SQL column is never 'called').
                       AND language NOT IN ('graphql', 'html', 'css', 'sql') {scope}"
                ))?;

                let mut functions = Vec::new();
                let mut classes = Vec::new();

                let read = |row: &rusqlite::Row| {
                    let entity_type: String = row.get(6)?;
                    let decorators: String = row.get(9)?;
                    let file_path: String = row.get(2)?;
                    let entity = EntityInfo {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        path_in_project: path_in_project(&file_path, &roots),
                        file_path,
                        start_line: row.get(3)?,
                        end_line: row.get(4)?,
                        language: row.get(5)?,
                        access: row.get::<_, String>(7)?.to_lowercase(),
                        modifiers: row.get::<_, String>(8)?.to_lowercase(),
                        decorators: serde_json::from_str(&decorators).unwrap_or_default(),
                        owner: row.get(10)?,
                    };
                    Ok((entity_type, entity))
                };
                let rows: Vec<(String, EntityInfo)> = match &range {
                    Some((low, high)) => stmt
                        .query_map(rusqlite::params![low, high], read)?
                        .collect::<rusqlite::Result<_>>()?,
                    None => stmt.query_map([], read)?.collect::<rusqlite::Result<_>>()?,
                };
                for (ty, entity) in rows {
                    if ty == "class" {
                        classes.push(entity);
                    } else {
                        functions.push(entity);
                    }
                }

                Ok::<_, crate::error::Error>((functions, classes))
            })
            .await
            .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        self.functions = functions;
        self.classes = classes;

        Ok(())
    }

    /// For each of `functions`, then `classes`, where its name is first written
    /// outside its own definition — in another file, or in its own file's other
    /// lines — among all the indexed files, as uses are looked for everywhere:
    /// what a call from another language, a template, a program a script runs, a
    /// string or a configuration leaves of it, which no reference records. A
    /// name is written where it is a word of the file, `?` and `!` included.
    async fn mentions_outside_definitions(
        &self,
        functions: &[UnusedEntity],
        classes: &[UnusedEntity],
    ) -> Result<Vec<Option<(String, u32)>>> {
        // The definitions' spans, by the entity's file and first line.
        let spans: HashMap<(&str, u32), u32> = self
            .functions
            .iter()
            .chain(self.classes.iter())
            .map(|entity| {
                (
                    (entity.file_path.as_str(), entity.start_line),
                    entity.end_line,
                )
            })
            .collect();
        let wanted: Vec<(String, String, u32, u32)> = functions
            .iter()
            .chain(classes.iter())
            .map(|entity| {
                let end = spans
                    .get(&(entity.file_path.as_str(), entity.line))
                    .copied()
                    .unwrap_or(entity.line);
                (
                    entity.name.clone(),
                    entity.file_path.clone(),
                    entity.line,
                    end,
                )
            })
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let pool = self.database.conn_pool().clone();
        tokio::task::spawn_blocking(move || {
            let files: Vec<String> = {
                let conn = pool.get()?;
                let mut stmt = conn.prepare("SELECT path FROM files")?;
                stmt.query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let names: HashSet<&str> = wanted.iter().map(|(name, ..)| name.as_str()).collect();
            // Every place each name is written: (file, line).
            let mut written: HashMap<&str, Vec<(String, u32)>> = HashMap::new();
            for file in &files {
                if !std::fs::metadata(file).is_ok_and(|meta| meta.len() <= MAX_SCANNED_FILE_BYTES) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(file) else {
                    continue;
                };
                for (number, line) in text.lines().enumerate() {
                    let words = line.split(|c: char| {
                        !(c.is_alphanumeric() || matches!(c, '_' | '$' | '?' | '!'))
                    });
                    for word in words {
                        let word = word.trim_start_matches(['?', '!']);
                        let word = if names.contains(word) {
                            word
                        } else {
                            word.trim_end_matches(['?', '!'])
                        };
                        if let Some(name) = names.get(word) {
                            let line = u32::try_from(number).unwrap_or(u32::MAX).saturating_add(1);
                            written.entry(name).or_default().push((file.clone(), line));
                        }
                    }
                }
            }
            Ok::<_, crate::error::Error>(
                wanted
                    .iter()
                    .map(|(name, file, start, end)| {
                        written.get(name.as_str()).and_then(|places| {
                            places
                                .iter()
                                .find(|(at, line)| at != file || *line < *start || *line > *end)
                                .cloned()
                        })
                    })
                    .collect(),
            )
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))?
    }

    /// Build call and usage graphs from relationships.
    ///
    /// **Note:** Relationship type matching here uses plain string
    /// comparison. If the database ever stores `RelationshipType`
    /// ordinals or a canonical enum representation, this should be
    /// updated to parse into [`RelationshipType`](crate::entities::RelationshipType)
    /// first for type-safe dispatch.
    ///
    /// # Errors
    /// Returns an error if database operations fail
    async fn build_relationship_graphs(&mut self) -> Result<()> {
        let pool = self.database.conn_pool().clone();

        let relationships: Vec<(String, String, String)> = tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let mut stmt =
                conn.prepare("SELECT source_id, target_id, relationship_type FROM relationships")?;

            let rels = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok::<Vec<_>, crate::error::Error>(rels)
        })
        .await
        .map_err(|e| crate::error::Error::task_join("blocking task", e))??;

        for (source_id, target_id, rel_type) in relationships {
            let parsed = crate::entities::RelationshipType::parse(&rel_type)
                .unwrap_or(crate::entities::RelationshipType::RelatedTo);
            match parsed {
                // Forward edges only: the target is a genuinely referenced entity.
                // Reverse edges (CalledBy, UsedBy) must NOT insert into used_entities
                // because their target_id is the CALLER, not the callee — including
                // them would mark everything reachable and defeat dead-code detection.
                crate::entities::RelationshipType::Calls => {
                    self.used_entities.insert(target_id.clone());
                    self.call_graph
                        .entry(source_id)
                        .or_default()
                        .insert(target_id);
                }
                crate::entities::RelationshipType::Uses
                | crate::entities::RelationshipType::References
                | crate::entities::RelationshipType::Inherits
                | crate::entities::RelationshipType::Implements => {
                    self.used_entities.insert(target_id.clone());
                    self.usage_graph
                        .entry(source_id)
                        .or_default()
                        .insert(target_id);
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// The entry points: what a runtime, a framework or a test runner calls
    /// rather than any code by name — test code; a function a decorator or
    /// annotation registers; an override, called through the type it
    /// overrides; a name the runtime calls (`__str__`, a constructor, `main`,
    /// `apply`); the conventional entry names (`main`, `run`, `serve`, …); and
    /// what a framework's role files export (routes, controllers, views,
    /// resolvers).
    fn identify_entry_points(&mut self) {
        let entry_names = [
            "main",
            "__main__",
            "run",
            "start",
            "init",
            "initialize",
            "bootstrap",
            "launch",
            "serve",
            "listen",
            "handler",
        ];
        for func in &self.functions {
            let named_entry = entry_names
                .iter()
                .any(|entry| func.name.eq_ignore_ascii_case(entry));
            if named_entry
                || func.is_test_code()
                || func.has_entry_decorator()
                || func.overrides()
                || func.is_runtime_invoked()
                || (func.in_role_file() && !func.is_private())
                // A getter or setter is read and written as a property, which
                // no reference records.
                || func.is_accessor()
                // A contract's interface: what a transaction or another
                // contract calls.
                || (func.language == "solidity" && func.is_exported())
            {
                self.entry_points.insert(func.id.clone());
            }
        }
    }

    /// Find the functions nothing calls, references or reaches through a type
    /// they override or implement. One of a type that extends a type the
    /// index does not hold, which may implement a member of it, is reported
    /// at [`CONFIDENCE_UNINDEXED_SUPERTYPE`]: in a language whose methods may
    /// implement one unmarked, and unless kept private.
    fn find_unused_functions(&self, hierarchy: &Hierarchy) -> Vec<UnusedEntity> {
        let mut unused = Vec::new();

        for func in &self.functions {
            if self.entry_points.contains(&func.id)
                || self.is_entity_used(&func.id)
                || self.dispatched.contains(&func.id)
            {
                continue;
            }
            let mut confidence = confidence_for_visibility(func.is_exported());
            let mut reason = "No incoming calls or references".to_string();
            let private = func
                .access
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word, "private" | "fileprivate"));
            if !MARKED_IMPLEMENTATIONS.contains(&func.language.as_str())
                && !private
                && let Some(owner) = func.owner.as_deref()
                && let Some(supertype) = hierarchy.unindexed_supertype(owner)
            {
                confidence = confidence.min(CONFIDENCE_UNINDEXED_SUPERTYPE);
                reason = format!(
                    "{reason}, though {owner} extends {supertype}, which the index does not \
                     hold: it may implement a member of it"
                );
            }
            unused.push(UnusedEntity {
                name: func.name.clone(),
                file_path: func.file_path.clone(),
                line: func.start_line,
                language: func.language.clone(),
                entity_type: "function".to_string(),
                reason,
                confidence,
            });
        }

        unused
    }

    /// Find the classes nothing uses: not referenced, extended or
    /// instantiated, none of whose members is used or an entry point —
    /// `MoneyMath.round()` uses `MoneyMath`, `new Paginator()` the class whose
    /// constructor it calls — and that no framework or test runner takes up
    /// (a controller, a test class, a decorated class).
    fn find_unused_classes(&self) -> Vec<UnusedEntity> {
        let used_owners: HashSet<&str> = self
            .functions
            .iter()
            .filter(|func| {
                self.is_entity_used(&func.id)
                    || self.entry_points.contains(&func.id)
                    || self.dispatched.contains(&func.id)
            })
            .filter_map(|func| func.owner.as_deref())
            .collect();
        let mut unused = Vec::new();

        for cls in &self.classes {
            let taken_up = cls.is_test_code()
                || cls.has_entry_decorator()
                || cls.in_role_file()
                || ROLE_CLASS_SUFFIXES
                    .iter()
                    .any(|suffix| cls.name.ends_with(suffix) && cls.name != *suffix);
            if self.is_entity_used(&cls.id) || used_owners.contains(cls.name.as_str()) || taken_up {
                continue;
            }
            unused.push(UnusedEntity {
                name: cls.name.clone(),
                file_path: cls.file_path.clone(),
                line: cls.start_line,
                language: cls.language.clone(),
                entity_type: "class".to_string(),
                reason: "No instantiations, members used or inheritance".to_string(),
                confidence: confidence_for_visibility(cls.is_exported()),
            });
        }

        unused
    }

    /// Check if an entity is used (called, referenced, etc.)
    ///
    /// One O(1) lookup in the set of referenced entities.
    fn is_entity_used(&self, entity_id: &str) -> bool {
        self.used_entities.contains(entity_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dead_code_report() {
        let mut report = DeadCodeReport::new();

        report.add_dead_function(UnusedEntity {
            name: "unused_func".to_string(),
            file_path: "/test/file.rs".to_string(),
            line: 10,
            language: "rust".to_string(),
            entity_type: "function".to_string(),
            reason: "No callers".to_string(),
            confidence: CONFIDENCE_PRIVATE,
        });

        assert_eq!(report.dead_functions.len(), 1);
        assert_eq!(report.summary.total_findings, 1);

        report.generate_recommendations();
        assert!(!report.recommendations.is_empty());
    }

    #[test]
    fn test_unused_entity_creation() {
        let entity = UnusedEntity {
            name: "test_func".to_string(),
            file_path: "/test/file.py".to_string(),
            line: 42,
            language: "python".to_string(),
            entity_type: "function".to_string(),
            reason: "Test reason".to_string(),
            confidence: CONFIDENCE_PRIVATE,
        };

        assert_eq!(entity.name, "test_func");
        assert_eq!(entity.line, 42);
    }

    #[test]
    fn test_report_to_findings() {
        let mut report = DeadCodeReport::new();

        report.add_dead_function(UnusedEntity {
            name: "dead_func".to_string(),
            file_path: "/test.rs".to_string(),
            line: 10,
            language: "rust".to_string(),
            entity_type: "function".to_string(),
            reason: "Unused".to_string(),
            confidence: CONFIDENCE_PRIVATE,
        });

        report.add_dead_class(UnusedEntity {
            name: "DeadClass".to_string(),
            file_path: "/test.rs".to_string(),
            line: 20,
            language: "rust".to_string(),
            entity_type: "class".to_string(),
            reason: "Unused".to_string(),
            confidence: CONFIDENCE_PUBLIC,
        });

        let findings = report.to_findings();
        assert_eq!(findings.len(), 2);
    }

    /// A file's path is read within the outermost scanned directory that
    /// holds it, its project — a directory scanned again on its own is no
    /// project — and whole when none holds it.
    #[test]
    fn a_path_is_read_within_its_project() {
        let roots = [
            PathBuf::from("/home/me/test/shop"),
            PathBuf::from("/home/me/test/shop/tests"),
        ];
        assert_eq!(
            path_in_project("/home/me/test/shop/tests/test_cart.py", &roots),
            Path::new("tests/test_cart.py")
        );
        assert_eq!(
            path_in_project("/elsewhere/cart.py", &roots),
            Path::new("/elsewhere/cart.py")
        );
    }

    /// 4.2 AC: Only forward edges (Calls/Uses/…) mark a target as used.
    /// A reverse edge CalledBy(source=A, target=B) must NOT mark B as used —
    /// B is the caller and may itself be dead code.
    #[test]
    fn test_only_forward_edges_mark_used() {
        // Simulate exactly what build_relationship_graphs does.
        let mut used: HashSet<String> = HashSet::new();

        // Calls(A→B): forward — B is genuinely called, so B is "used"
        // CalledBy(C, target=D): reverse — D is the caller, must NOT be "used"
        let rels: Vec<(&str, &str, &str)> = vec![
            ("a", "b", "calls"),
            ("c", "d", "called_by"),
            ("e", "f", "uses"),
            ("g", "h", "used_by"),
        ];

        for (source_id, target_id, rel_type) in rels {
            let parsed = crate::entities::RelationshipType::parse(rel_type)
                .unwrap_or(crate::entities::RelationshipType::RelatedTo);
            match parsed {
                crate::entities::RelationshipType::Calls
                | crate::entities::RelationshipType::Uses
                | crate::entities::RelationshipType::References
                | crate::entities::RelationshipType::Inherits
                | crate::entities::RelationshipType::Implements => {
                    used.insert(target_id.to_string());
                    let _ = (source_id, target_id); // graphs not needed for this test
                }
                _ => {}
            }
        }

        assert!(
            used.contains("b"),
            "Calls forward target must be marked used"
        );
        assert!(
            used.contains("f"),
            "Uses forward target must be marked used"
        );
        assert!(
            !used.contains("d"),
            "CalledBy reverse target must NOT be marked used"
        );
        assert!(
            !used.contains("h"),
            "UsedBy reverse target must NOT be marked used"
        );
    }
}
