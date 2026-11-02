//! Core entity types representing code constructs
//!
//! # Design note: loosely-typed fields
//!
//! Many fields use `Option<String>` / `Option<serde_json::Value>` rather than
//! richer enums or dedicated types (e.g. visibility, modifiers, type references,
//! attribute bags). This is a deliberate storage-flexibility trade-off: entities
//! span 26 languages with divergent semantics, are serialized verbatim into the
//! SQLite `data` column, and must round-trip without per-language schema churn.
//! The cost is reduced compile-time guarantees on those fields — an accepted
//! maintainability trade-off, not an oversight.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Unique identifier for entities
pub type EntityId = Uuid;

/// Base trait for all code entities
pub trait CodeEntity {
    /// Get entity ID
    fn id(&self) -> EntityId;
    /// Get entity name
    fn name(&self) -> &str;
    /// Get file path
    fn file_path(&self) -> &str;
    /// Get language
    fn language(&self) -> &str;
}

/// Source code location
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Start line (1-indexed)
    pub start_line: u32,
    /// End line (1-indexed, inclusive)
    pub end_line: u32,
    /// Start column (0-indexed)
    pub start_column: u32,
    /// End column (0-indexed, inclusive)
    pub end_column: u32,
}

impl SourceLocation {
    /// Create a new source location
    #[must_use]
    pub const fn new(start_line: u32, start_column: u32, end_line: u32, end_column: u32) -> Self {
        Self {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    /// Get line count
    #[must_use]
    pub fn line_count(&self) -> u32 {
        self.end_line.saturating_sub(self.start_line) + 1
    }
}

/// Represents a function or method
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Function {
    /// Unique identifier
    pub id: EntityId,
    /// Function name
    pub name: String,
    /// File path where the function is defined
    pub file_path: String,
    /// Function body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Parent function name if nested
    pub parent_function: Option<String>,
    /// Parent class name if a method
    pub parent_class: Option<String>,
    /// Function parameters
    pub parameters: Option<String>,
    /// Return type
    pub return_type: Option<String>,
    /// Modifiers (e.g., async, static)
    pub modifiers: Option<String>,
    /// Access modifiers (e.g., public, private)
    pub access_modifiers: Option<String>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Decorator / annotation names applied to this function (e.g. `["Injectable", "Override"]`).
    #[serde(default)]
    pub decorators: Vec<String>,
    /// The interface or trait whose member it implements, where its
    /// definition says so: `Display` for the `fmt` of Rust's
    /// `impl Display for Order`, `IDisposable` for C#'s
    /// `void IDisposable.Dispose()`, an F# `interface … with` member's, a
    /// VB.NET `Implements` clause's. A call through that type reaches it.
    pub implements: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Function {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a class
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Class {
    /// Unique identifier
    pub id: EntityId,
    /// Class name
    pub name: String,
    /// File path where the class is defined
    pub file_path: String,
    /// Class body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Fields/properties of the class
    pub fields: Option<serde_json::Value>,
    /// Method names in the class
    pub methods: Option<Vec<String>>,
    /// Parent class name
    pub extends: Option<String>,
    /// Implemented interfaces
    pub implements: Option<String>,
    /// Class constants
    pub constants: Option<String>,
    /// Access modifiers (e.g., public, private)
    pub access_modifiers: Option<String>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Decorator / annotation names applied to this class (e.g. `["Component", "Injectable"]`).
    #[serde(default)]
    pub decorators: Vec<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Class {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a struct (Rust, Go, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Struct {
    /// Unique identifier
    pub id: EntityId,
    /// Struct name
    pub name: String,
    /// File path where the struct is defined
    pub file_path: String,
    /// Struct body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Fields of the struct
    pub fields: Option<serde_json::Value>,
    /// Method names associated with the struct
    pub methods: Option<Vec<String>>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Struct {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents an enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enum {
    /// Unique identifier
    pub id: EntityId,
    /// Enum name
    pub name: String,
    /// File path where the enum is defined
    pub file_path: String,
    /// Enum body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Enum variants
    pub variants: Option<serde_json::Value>,
    /// Parent class if nested
    pub parent_class: Option<String>,
    /// Backed type for enums (e.g., int, string)
    pub backed_type: Option<String>,
    /// Implemented interfaces
    pub implements: Option<String>,
    /// Method names associated with the enum
    pub methods: Option<Vec<String>>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Enum {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents an interface/trait
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interface {
    /// Unique identifier
    pub id: EntityId,
    /// Interface name
    pub name: String,
    /// File path where the interface is defined
    pub file_path: String,
    /// Interface body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Method signatures in the interface
    pub methods: Option<Vec<String>>,
    /// Properties defined in the interface
    pub properties: Option<serde_json::Value>,
    /// Extended interfaces
    pub extends: Option<Vec<String>>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Interface {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a trait (Rust-specific)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trait {
    /// Unique identifier
    pub id: EntityId,
    /// Trait name
    pub name: String,
    /// File path where the trait is defined
    pub file_path: String,
    /// Trait body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Method signatures in the trait
    pub methods: Option<Vec<String>>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Trait {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    /// Unique identifier
    pub id: EntityId,
    /// Module name
    pub name: String,
    /// File path where the module is defined
    pub file_path: String,
    /// Module body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Exported items
    pub exports: Option<serde_json::Value>,
    /// Imported modules
    pub imports: Option<Vec<String>>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Module {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a namespace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {
    /// Unique identifier
    pub id: EntityId,
    /// Namespace name
    pub name: String,
    /// File path where the namespace is defined
    pub file_path: String,
    /// Namespace body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Namespace {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a constant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constant {
    /// Unique identifier
    pub id: EntityId,
    /// Constant name
    pub name: String,
    /// File path where the constant is defined
    pub file_path: String,
    /// Constant body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Constant value
    pub value: Option<String>,
    /// Constant type
    pub constant_type: Option<String>,
    /// The type the constant is a member of: `ReeferLimits` of `object
    /// ReeferLimits { const val MAX_CELSIUS = 25.0 }`, which a qualified use
    /// (`ReeferLimits.MAX_CELSIUS`) names. `None` at a module's top level.
    #[serde(default)]
    pub parent_class: Option<String>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Constant {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a type alias
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeAlias {
    /// Unique identifier
    pub id: EntityId,
    /// Type alias name
    pub name: String,
    /// File path where the type alias is defined
    pub file_path: String,
    /// Type alias body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// The type being aliased
    pub aliased_type: String,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for TypeAlias {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a union type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Union {
    /// Unique identifier
    pub id: EntityId,
    /// Union name
    pub name: String,
    /// File path where the union is defined
    pub file_path: String,
    /// Union body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Union member types
    pub members: Vec<String>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Union {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a macro
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Macro {
    /// Unique identifier
    pub id: EntityId,
    /// Macro name
    pub name: String,
    /// File path where the macro is defined
    pub file_path: String,
    /// Macro body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Macro {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a component (e.g., Vue, React, Svelte)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    /// Unique identifier
    pub id: EntityId,
    /// Component name
    pub name: String,
    /// File path where the component is defined
    pub file_path: String,
    /// Component body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Component props
    pub props: Option<serde_json::Value>,
    /// Method names in the component
    pub methods: Option<Vec<String>>,
    /// Template content
    pub template: Option<String>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Component {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a smart contract
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    /// Unique identifier
    pub id: EntityId,
    /// Contract name
    pub name: String,
    /// File path where the contract is defined
    pub file_path: String,
    /// Contract body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Functions in the contract
    pub functions: Option<serde_json::Value>,
    /// Events defined in the contract
    pub events: Option<Vec<String>>,
    /// Modifiers defined in the contract
    pub modifiers: Option<Vec<String>>,
    /// State variables in the contract
    pub state_variables: Option<serde_json::Value>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Contract {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents an extension
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extension {
    /// Unique identifier
    pub id: EntityId,
    /// Extension name
    pub name: String,
    /// File path where the extension is defined
    pub file_path: String,
    /// Extension body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Type being extended
    pub extended_type: Option<String>,
    /// Method names in the extension
    pub methods: Option<Vec<String>>,
    /// Documentation string
    pub docstring: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Extension {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a decorator (Python decorators, TypeScript/ES decorators)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decorator {
    /// Unique identifier
    pub id: EntityId,
    /// Decorator name
    pub name: String,
    /// File path where the decorator is defined
    pub file_path: String,
    /// Decorator body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Decorator type (e.g., "class", "method", "property")
    pub decorator_type: Option<String>,
    /// Target being decorated
    pub target: Option<String>,
    /// Decorator parameters
    pub parameters: Option<serde_json::Value>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Decorator {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents documentation comments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Documentation {
    /// Unique identifier
    pub id: EntityId,
    /// Documentation identifier/name
    pub name: String,
    /// File path where the documentation is defined
    pub file_path: String,
    /// Documentation body text
    pub body: String,
    /// Source location
    pub location: SourceLocation,
    /// Programming language
    pub language: String,
    /// Documentation type (e.g., "docstring", "comment", "jsdoc")
    pub doc_type: Option<String>,
    /// Associated entity name
    pub associated_entity: Option<String>,
    /// Associated entity type
    pub associated_entity_type: Option<String>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

impl CodeEntity for Documentation {
    fn id(&self) -> EntityId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn file_path(&self) -> &str {
        &self.file_path
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// Represents a file in the codebase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Unique identifier
    pub id: EntityId,
    /// File path
    pub path: String,
    /// Programming language
    pub language: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Number of lines in the file
    pub line_count: u32,
    /// Content checksum
    pub checksum: String,
    /// Last modification timestamp
    pub last_modified: OffsetDateTime,
    /// Entity IDs contained in this file
    pub entities: Vec<EntityId>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
    /// Last update timestamp
    pub updated_at: OffsetDateTime,
}

/// Types of relationships between entities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationshipType {
    /// Function calls another function
    Calls,
    /// Function is called by another function
    CalledBy,
    /// Class inherits from another class
    Inherits,
    /// Class is inherited by another class
    InheritedBy,
    /// Class implements an interface
    Implements,
    /// Interface is implemented by a class
    ImplementedBy,
    /// Function is a method of a class
    MethodOf,
    /// Class contains a method
    HasMethod,
    /// Function uses/imports a module
    Uses,
    /// Module is used by a function
    UsedBy,
    /// An entity uses a name as a value: a constant or property it reads or
    /// writes, a function it passes (`items.map(format)`)
    References,
    /// A constant or function is used as a value by an entity
    ReferencedBy,
    /// Function overrides another function
    Overrides,
    /// Function is overridden by another
    OverriddenBy,
    /// Generic relationship
    RelatedTo,
}

impl RelationshipType {
    /// Every variant, in declaration order.
    pub const ALL: [Self; 15] = [
        Self::Calls,
        Self::CalledBy,
        Self::Inherits,
        Self::InheritedBy,
        Self::Implements,
        Self::ImplementedBy,
        Self::MethodOf,
        Self::HasMethod,
        Self::Uses,
        Self::UsedBy,
        Self::References,
        Self::ReferencedBy,
        Self::Overrides,
        Self::OverriddenBy,
        Self::RelatedTo,
    ];

    /// Whether this kind is the reverse row of another kind's edge
    /// (`called_by` for `calls`, …), stored with the forward edge's endpoints
    /// swapped. A file's own content produces the forward edges whose source
    /// is one of its entities, and the reverse rows whose target is.
    #[must_use]
    pub const fn is_reverse(&self) -> bool {
        match self {
            Self::CalledBy
            | Self::InheritedBy
            | Self::ImplementedBy
            | Self::UsedBy
            | Self::ReferencedBy
            | Self::OverriddenBy => true,
            Self::Calls
            | Self::Inherits
            | Self::Implements
            | Self::Uses
            | Self::References
            | Self::Overrides
            | Self::HasMethod
            | Self::MethodOf
            | Self::RelatedTo => false,
        }
    }

    /// Whether this kind nests a member in its type (`has_method`,
    /// `method_of`): an edge the structure of a file yields, where every other
    /// forward edge resolves a reference one of its entities makes.
    #[must_use]
    pub const fn is_nesting(&self) -> bool {
        matches!(self, Self::HasMethod | Self::MethodOf)
    }

    /// Parse relationship type from string
    ///
    /// # Errors
    /// Returns an error if the string is not a valid relationship type
    pub fn parse(s: &str) -> crate::error::Result<Self> {
        // Build a temporary lowercase string once so we can match with
        // O(1) dispatch instead of a long if-else chain.
        match s.to_ascii_lowercase().as_str() {
            "calls" => Ok(Self::Calls),
            "called_by" => Ok(Self::CalledBy),
            "inherits" => Ok(Self::Inherits),
            "inherited_by" => Ok(Self::InheritedBy),
            "implements" => Ok(Self::Implements),
            "implemented_by" => Ok(Self::ImplementedBy),
            "method_of" => Ok(Self::MethodOf),
            "has_method" => Ok(Self::HasMethod),
            "uses" => Ok(Self::Uses),
            "used_by" => Ok(Self::UsedBy),
            "references" => Ok(Self::References),
            "referenced_by" => Ok(Self::ReferencedBy),
            "overrides" => Ok(Self::Overrides),
            "overridden_by" => Ok(Self::OverriddenBy),
            "related_to" => Ok(Self::RelatedTo),
            _ => Err(crate::error::Error::Analysis(format!(
                "Invalid relationship type: {s}"
            ))),
        }
    }

    /// Convert to string representation
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::CalledBy => "called_by",
            Self::Inherits => "inherits",
            Self::InheritedBy => "inherited_by",
            Self::Implements => "implements",
            Self::ImplementedBy => "implemented_by",
            Self::MethodOf => "method_of",
            Self::HasMethod => "has_method",
            Self::Uses => "uses",
            Self::UsedBy => "used_by",
            Self::References => "references",
            Self::ReferencedBy => "referenced_by",
            Self::Overrides => "overrides",
            Self::OverriddenBy => "overridden_by",
            Self::RelatedTo => "related_to",
        }
    }
}

impl std::str::FromStr for RelationshipType {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Whether `entity_type` (an [`Entity::entity_type`] string) is a type
/// definition: what a type annotation, a supertype or an `impl … for` names.
/// A constructor named after its class is a `function`, not one.
#[must_use]
pub fn is_type_definition(entity_type: &str) -> bool {
    matches!(
        entity_type,
        "class" | "struct" | "enum" | "interface" | "trait" | "type_alias" | "union" | "contract"
    )
}

/// Represents a relationship between two entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Unique identifier
    pub id: EntityId,
    /// Source entity ID
    pub source_id: EntityId,
    /// Target entity ID
    pub target_id: EntityId,
    /// Type of relationship
    pub relationship_type: RelationshipType,
    /// Additional metadata
    pub metadata: Option<serde_json::Value>,
    /// Creation timestamp
    pub created_at: OffsetDateTime,
}

impl Relationship {
    /// Record that this edge is one of `candidates` equally good resolutions
    /// of a name: sets `"ambiguous": true` and `"candidates"` in the metadata.
    /// Does nothing for a single candidate.
    pub fn mark_ambiguous(&mut self, candidates: usize) {
        if candidates < 2 {
            return;
        }
        let metadata = self
            .metadata
            .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(fields) = metadata.as_object_mut() {
            fields.insert("ambiguous".to_string(), serde_json::Value::Bool(true));
            fields.insert("candidates".to_string(), candidates.into());
        }
    }

    /// Whether this edge is one of several equally good resolutions of a name
    /// (see [`Self::mark_ambiguous`]).
    #[must_use]
    pub fn is_ambiguous(&self) -> bool {
        self.metadata
            .as_ref()
            .is_some_and(|metadata| metadata["ambiguous"] == true)
    }
}

/// A name a source entity refers to — a call, a type it uses, a supertype it
/// extends or implements — recorded whether or not it resolves. The edges
/// of the graph are derived by resolving references: a reference to a name
/// the index does not define yet links once a definition appears.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolRef {
    /// The entity the reference occurs in.
    pub source_id: EntityId,
    /// The forward relationship it yields once resolved: `Calls`, `Uses`,
    /// `Inherits` or `Implements`. A supertype named where a class extends
    /// (not in an `implements` list) is `Inherits`, and yields `Implements`
    /// when it resolves to an interface, trait or protocol.
    pub kind: RelationshipType,
    /// The name referred to.
    pub target_name: String,
    /// What the name was qualified with, when the source shows it: the
    /// receiver of a method call (`orders` in `orders.total()`), or a path,
    /// which ends with its `::` (`Order::` in `Order::new()`) as it names a
    /// type or a module, not a value (see
    /// [`CallScope`](crate::parser::symbol_table::CallScope)).
    pub qualifier: Option<String>,
    /// The line the reference is on, counted from 1.
    pub line: u32,
}

/// Enumeration of all entity types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Entity {
    /// Function entity
    Function(Function),
    /// Class entity
    Class(Class),
    /// Struct entity
    Struct(Struct),
    /// SQL table / view — a named relational data container. Reuses the `Class`
    /// shape (name, body, docstring, columns-in-body) but is typed `table` so it
    /// embeds and reads as a table rather than a class.
    Table(Class),
    /// Enum entity
    Enum(Enum),
    /// Interface entity
    Interface(Interface),
    /// Trait entity
    Trait(Trait),
    /// Module entity
    Module(Module),
    /// Namespace entity
    Namespace(Namespace),
    /// Constant entity
    Constant(Constant),
    /// Type alias entity
    TypeAlias(TypeAlias),
    /// Union entity
    Union(Union),
    /// Macro entity
    Macro(Macro),
    /// Component entity
    Component(Component),
    /// Contract entity
    Contract(Contract),
    /// Extension entity
    Extension(Extension),
    /// Decorator entity (Python/TypeScript)
    Decorator(Decorator),
    /// A titled section of a prose document — one heading and the text beneath
    /// it. Reuses the [`Documentation`] shape (name, body, location) but is
    /// typed `section` so it embeds and reads as retrievable prose rather than
    /// as a docstring attached to code, and so callers can filter documentation
    /// out of a code search.
    Section(Documentation),
    /// A SQL index. Reuses the [`Class`] shape (name, body) like [`Entity::Table`]
    /// but is typed `index`, so "which tables exist?" is not answered with
    /// index names.
    Index(Class),
}

/// Centralised match macro for the `Entity` enum.
///
/// Adding a new entity variant only requires updating this single list,
/// eliminating the previous 200+ arm duplication across accessor methods.
#[macro_export]
macro_rules! match_entity {
    ($value:expr, $var:ident => $body:expr) => {
        match $value {
            Entity::Function($var) => $body,
            Entity::Class($var) => $body,
            Entity::Table($var) => $body,
            Entity::Struct($var) => $body,
            Entity::Enum($var) => $body,
            Entity::Interface($var) => $body,
            Entity::Trait($var) => $body,
            Entity::Module($var) => $body,
            Entity::Namespace($var) => $body,
            Entity::Constant($var) => $body,
            Entity::TypeAlias($var) => $body,
            Entity::Union($var) => $body,
            Entity::Macro($var) => $body,
            Entity::Component($var) => $body,
            Entity::Contract($var) => $body,
            Entity::Extension($var) => $body,
            Entity::Decorator($var) => $body,
            Entity::Section($var) => $body,
            Entity::Index($var) => $body,
        }
    };
}

impl Entity {
    /// Get entity ID
    #[must_use]
    pub fn id(&self) -> EntityId {
        match_entity!(self, e => e.id)
    }

    /// Get entity name
    #[must_use]
    pub fn name(&self) -> &str {
        match_entity!(self, e => &e.name)
    }

    /// Get file path
    #[must_use]
    pub fn file_path(&self) -> &str {
        match_entity!(self, e => &e.file_path)
    }

    /// Get language
    #[must_use]
    pub fn language(&self) -> &str {
        match_entity!(self, e => &e.language)
    }

    /// Override the language label. Used when entities are extracted from an
    /// embedded sub-language (e.g. the JavaScript inside a Svelte `<script>`) but
    /// should be attributed to the host file's language.
    pub fn set_language(&mut self, language: String) {
        match_entity!(self, e => e.language = language);
    }

    /// Get body
    #[must_use]
    pub fn body(&self) -> &str {
        match_entity!(self, e => &e.body)
    }

    /// Get entity type string for database storage
    #[must_use]
    pub fn entity_type(&self) -> &'static str {
        match self {
            Entity::Function(_) => "function",
            Entity::Class(_) => "class",
            Entity::Table(_) => "table",
            Entity::Struct(_) => "struct",
            Entity::Enum(_) => "enum",
            Entity::Interface(_) => "interface",
            Entity::Trait(_) => "trait",
            Entity::Module(_) => "module",
            Entity::Namespace(_) => "namespace",
            Entity::Constant(_) => "constant",
            Entity::TypeAlias(_) => "type_alias",
            Entity::Union(_) => "union",
            Entity::Macro(_) => "macro",
            Entity::Component(_) => "component",
            Entity::Contract(_) => "contract",
            Entity::Extension(_) => "extension",
            Entity::Decorator(_) => "decorator",
            Entity::Section(_) => "section",
            Entity::Index(_) => "index",
        }
    }

    /// Whether this entity defines a type (see [`is_type_definition`]).
    #[must_use]
    pub fn is_type_definition(&self) -> bool {
        is_type_definition(self.entity_type())
    }

    /// A stability disambiguator used — together with file path, entity type and
    /// name — to derive a content-stable [`EntityId`].
    ///
    /// It must depend only on the entity's *identity*, never on its line
    /// position, so that re-parsing a file after unrelated edits keeps the same
    /// id (and therefore its embedding). For callables this is the enclosing
    /// scope plus the parameter list (so overloads get distinct ids); for most
    /// kinds the name alone already disambiguates, so this returns empty.
    #[must_use]
    pub fn identity_disambiguator(&self) -> String {
        match self {
            Entity::Function(f) => {
                let parent = f
                    .parent_class
                    .as_deref()
                    .or(f.parent_function.as_deref())
                    .unwrap_or("");
                let params = f.parameters.as_deref().unwrap_or("");
                format!("{parent}#({params})")
            }
            _ => String::new(),
        }
    }

    /// Get source location
    #[must_use]
    pub const fn location(&self) -> SourceLocation {
        match self {
            Entity::Function(e) => e.location,
            Entity::Class(e) => e.location,
            Entity::Table(e) => e.location,
            Entity::Struct(e) => e.location,
            Entity::Enum(e) => e.location,
            Entity::Interface(e) => e.location,
            Entity::Trait(e) => e.location,
            Entity::Module(e) => e.location,
            Entity::Namespace(e) => e.location,
            Entity::Constant(e) => e.location,
            Entity::TypeAlias(e) => e.location,
            Entity::Union(e) => e.location,
            Entity::Macro(e) => e.location,
            Entity::Component(e) => e.location,
            Entity::Contract(e) => e.location,
            Entity::Extension(e) => e.location,
            Entity::Decorator(e) => e.location,
            Entity::Section(e) => e.location,
            Entity::Index(e) => e.location,
        }
    }

    /// Decompose entity into database fields to avoid per-arm cloning in insert operations.
    ///
    /// # Errors
    /// Returns a `Serialization` error if the inner entity cannot be serialized.
    pub fn to_db_fields(
        &self,
    ) -> crate::error::Result<(
        String,
        String,
        String,
        String,
        SourceLocation,
        String,
        serde_json::Value,
    )> {
        // IMPORTANT: Serialize the inner entity type, not the Entity wrapper.
        // The data column stores just the entity struct (e.g., Function, Class),
        // not the Entity enum wrapper. This is required for row_to_entity() to
        // correctly deserialize the data back into the proper type.
        let (id, name, file_path, body, location, language, data) = match_entity!(self, e => (
            e.id.to_string(), e.name.clone(), e.file_path.clone(), e.body.clone(), e.location, e.language.clone(), serde_json::to_value(e)?,
        ));
        Ok((id, name, file_path, body, location, language, data))
    }
}
