//! Shared tool schema definitions for MCP servers
//!
//! The one definition of the core tools' schemas, which the unified server
//! lists.
//!
//! Each schema is defined as a lazy-initialized `serde_json::Value` constant using
//! `std::sync::LazyLock`, ensuring evaluation happens once at first access rather
//! than at compile time (which `serde_json::Value` does not support for `const`).
//!
//! # Pre-computed tool list
//!
//! When the `rmcp-sdk` feature is enabled, the 32 core tools are additionally
//! pre-computed as [`LazyLock<Vec<Tool>>`][`CORE_TOOLS`] so that `list_tools`
//! never re-allocates their schemas on every call.

use serde_json::Value;
use serde_json::json;
use std::sync::LazyLock;

#[cfg(feature = "rmcp-sdk")]
use std::sync::Arc;

#[cfg(feature = "rmcp-sdk")]
use rmcp::model::Tool;

/// Convert a `serde_json::Value` (expected to be an Object) into
/// `Arc<serde_json::Map<String, serde_json::Value>>` for use with rmcp's `Tool`.
#[cfg(feature = "rmcp-sdk")]
#[must_use]
pub fn to_arc_map(value: &Value) -> Arc<serde_json::Map<String, serde_json::Value>> {
    match value {
        Value::Object(map) => Arc::new(map.clone()),
        _ => Arc::new(serde_json::Map::new()),
    }
}

/// Pre-computed list of the 32 core tools (rmcp SDK format).
///
/// This avoids rebuilding the tool list (and re-cloning every schema) on every
/// `list_tools` call.  Schemas are already `LazyLock<Value>`; here we pay the
/// conversion cost exactly once at first access.
#[cfg(feature = "rmcp-sdk")]
pub static CORE_TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        Tool::new(
            "scan_codebase",
            "Scan a codebase and build the entity database",
            to_arc_map(&SCAN_CODEBASE_SCHEMA),
        ),
        Tool::new(
            "search_entities",
            "Search for entities in the database. `line_number` counts from 1, like the `cite` of LSP results",
            to_arc_map(&SEARCH_ENTITIES_SCHEMA),
        ),
        Tool::new(
            "analyze_code",
            "Run one of the code analyses — dead code, complexity, cross references, call graph, code smells, cross-file dependencies, clones, or all — and return its findings under a directory, each with its file, line and details",
            to_arc_map(&ANALYZE_CODE_SCHEMA),
        ),
        Tool::new(
            "get_statistics",
            "Get database statistics",
            to_arc_map(&GET_STATISTICS_SCHEMA),
        ),
        Tool::new(
            "start_analysis",
            "Start a background analysis job",
            to_arc_map(&START_ANALYSIS_SCHEMA),
        ),
        Tool::new(
            "get_analysis_progress",
            "Get progress of an analysis job",
            to_arc_map(&GET_ANALYSIS_PROGRESS_SCHEMA),
        ),
        Tool::new(
            "list_analysis_jobs",
            "List all analysis jobs",
            to_arc_map(&LIST_ANALYSIS_JOBS_SCHEMA),
        ),
        Tool::new(
            "cleanup_old_jobs",
            "Clean up old completed jobs",
            to_arc_map(&CLEANUP_OLD_JOBS_SCHEMA),
        ),
        Tool::new(
            "cancel_analysis",
            "Cancel a running analysis job",
            to_arc_map(&CANCEL_ANALYSIS_SCHEMA),
        ),
        Tool::new(
            "search_functions",
            "Search for functions. Result lines (`start_line`, `end_line`) count from 1, like the `cite` of LSP results",
            to_arc_map(&SEARCH_FUNCTIONS_SCHEMA),
        ),
        Tool::new(
            "search_classes",
            "Search for classes/types. Result lines (`start_line`, `end_line`) count from 1, like the `cite` of LSP results",
            to_arc_map(&SEARCH_CLASSES_SCHEMA),
        ),
        Tool::new(
            "fuzzy_search",
            "Fuzzy search across entities. Result lines (`start_line`, `end_line`) count from 1, like the `cite` of LSP results",
            to_arc_map(&FUZZY_SEARCH_SCHEMA),
        ),
        Tool::new(
            "semantic_search",
            "Semantic search using embeddings. Result lines (`start_line`, `end_line`) count from 1, like the `cite` of LSP results",
            to_arc_map(&SEMANTIC_SEARCH_SCHEMA),
        ),
        Tool::new(
            "generate_embeddings",
            "Generate and store semantic embedding vectors for the entities missing them that the project's embedding selection picks (idempotent); enables semantic_search",
            to_arc_map(&GENERATE_EMBEDDINGS_SCHEMA),
        ),
        Tool::new(
            "get_entity_details",
            "Where an entity is and what the index holds of it: a function's signature, owner, enclosing function, modifiers and decorators; a type's methods, fields with their types, and supertypes; and where the other entities of its name are",
            to_arc_map(&GET_ENTITY_DETAILS_SCHEMA),
        ),
        Tool::new(
            "get_relationships",
            "What an entity is linked to in the index — the calls it makes and gets, the types it uses, its supertypes and members — each edge once, with the file and line of both ends and the lines of the source a call or use is made on",
            to_arc_map(&GET_RELATIONSHIPS_SCHEMA),
        ),
        Tool::new(
            "find_dead_code",
            "Find the functions and classes nothing reaches: no call, reference, use or supertype, and no runtime, framework or test runner — tests, decorated routes and handlers, overrides and implementations of an interface's members, constructors and the code of framework role files (controllers, views, resolvers) are not reported; each finding says how sure it is",
            to_arc_map(&FIND_DEAD_CODE_SCHEMA),
        ),
        Tool::new(
            "analyze_complexity",
            "Rank the functions in scope by cyclomatic complexity: every function is counted, the most complex are listed whatever their score, and `threshold` says above which score a function is complex",
            to_arc_map(&ANALYZE_COMPLEXITY_SCHEMA),
        ),
        Tool::new(
            "add_context_note",
            "Add a note to the context graph",
            to_arc_map(&ADD_CONTEXT_NOTE_SCHEMA),
        ),
        Tool::new(
            "search_context",
            "Search context notes",
            to_arc_map(&SEARCH_CONTEXT_SCHEMA),
        ),
        Tool::new(
            "get_related_context",
            "Get related context for an entity",
            to_arc_map(&GET_RELATED_CONTEXT_SCHEMA),
        ),
        Tool::new(
            "prune_context",
            "Prune old context notes",
            to_arc_map(&PRUNE_CONTEXT_SCHEMA),
        ),
        Tool::new(
            "get_context_statistics",
            "Get context graph statistics",
            to_arc_map(&GET_CONTEXT_STATISTICS_SCHEMA),
        ),
        Tool::new(
            "describe_file",
            "Get a description of a file's contents",
            to_arc_map(&DESCRIBE_FILE_SCHEMA),
        ),
        Tool::new(
            "describe_modification",
            "Describe what a modification would do",
            to_arc_map(&DESCRIBE_MODIFICATION_SCHEMA),
        ),
        Tool::new(
            "get_file_context",
            "Get context for a file",
            to_arc_map(&GET_FILE_CONTEXT_SCHEMA),
        ),
        Tool::new(
            "health_check",
            "Check server health, and the project configuration file (devscriptor.yaml) the server follows",
            to_arc_map(&HEALTH_CHECK_SCHEMA),
        ),
        Tool::new(
            "validate_file_edit",
            "Validate a file edit with context",
            to_arc_map(&VALIDATE_FILE_EDIT_SCHEMA),
        ),
        Tool::new(
            "query_with_context",
            "Query with context awareness",
            to_arc_map(&QUERY_WITH_CONTEXT_SCHEMA),
        ),
        Tool::new(
            "start_watching",
            "Start the real-time watcher on a directory: the knowledge graph follows its source-file changes. `devscriptor mcp` already watches the scanned project it runs in, or does once a scan has indexed it (see get_watch_status); asked for that root, this reports the running watch with `already_watching`",
            to_arc_map(&START_WATCHING_SCHEMA),
        ),
        Tool::new(
            "stop_watching",
            "Stop the real-time watcher. A later scan does not start the server's own watch again",
            to_arc_map(&EMPTY_OBJECT_SCHEMA),
        ),
        Tool::new(
            "get_watch_status",
            "Get the real-time watcher status: whether and how it watches (`mode`: events or poll), re-index count, last re-index stats, and `auto_watch_error` when the server's own watcher did not start",
            to_arc_map(&EMPTY_OBJECT_SCHEMA),
        ),
    ]
});

// ============================================================================
// Analysis & Scanning Schemas
// ============================================================================

/// Schema for the `scan_codebase` tool.
pub static SCAN_CODEBASE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the codebase to scan"
            },
            "extensions": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Only scan these file extensions, with or without a leading dot, e.g. [\"rs\", \".py\"] (default: the project's devscriptor.yaml scan.include, else all supported languages)"
            },
            "exclude_patterns": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Glob patterns matched against file paths to exclude, e.g. [\"tests/**\", \"*_test.rs\"] (default: the project's devscriptor.yaml scan.exclude)"
            }
        },
        "required": ["path"]
    })
});

/// Schema for the `search_entities` tool.
pub static SEARCH_ENTITIES_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query (entity name pattern)"
            },
            "entity_type": {
                "type": "string",
                "description": "Entity type filter (e.g., 'function', 'class')"
            },
            "limit": {
                "type": "number",
                "description": "Maximum number of results (default: 10)"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `analyze_code` tool.
pub static ANALYZE_CODE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "default": ".",
                "description": "Directory whose findings are returned — those located under it; `.`, the default, is the whole index"
            },
            "analysis_type": {
                "type": "string",
                "enum": ["dead_code", "complexity", "cross_reference", "call_graph",
                         "code_smells", "cross_file", "clones", "all"],
                "description": "Type of analysis to run"
            }
        },
        "required": ["analysis_type"]
    })
});

/// Schema for the `get_statistics` tool.
pub static GET_STATISTICS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Optional path fragment; counts only entities whose file path contains it"
            }
        }
    })
});

// ============================================================================
// Job Management Schemas
// ============================================================================

/// Schema for the `start_analysis` tool.
pub static START_ANALYSIS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Path to the codebase to analyze"
            },
            "extensions": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Only scan these file extensions, with or without a leading dot (default: the project's devscriptor.yaml scan.include, else all supported languages)"
            },
            "exclude_patterns": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Glob patterns matched against file paths to exclude, e.g. [\"tests/**\", \"*_test.rs\"] (default: the project's devscriptor.yaml scan.exclude)"
            }
        },
        "required": ["path"]
    })
});

/// Schema for the `get_analysis_progress` tool.
pub static GET_ANALYSIS_PROGRESS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "job_id": {
                "type": "string",
                "description": "ID of the analysis job"
            },
            "wait": {
                "type": "boolean",
                "description": "Whether to wait for completion (default: false)"
            },
            "timeout": {
                "type": "number",
                "description": "Timeout in seconds when waiting (default: 30)"
            }
        },
        "required": ["job_id"]
    })
});

/// Schema for the `list_analysis_jobs` tool.
pub static LIST_ANALYSIS_JOBS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "limit": {
                "type": "number",
                "description": "Maximum number of jobs to return (default: 10)"
            }
        }
    })
});

/// Schema for the `cleanup_old_jobs` tool.
pub static CLEANUP_OLD_JOBS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "max_age_seconds": {
                "type": "number",
                "description": "Maximum age of jobs to keep in seconds (default: 3600)"
            }
        }
    })
});

/// Schema for the `cancel_analysis` tool.
pub static CANCEL_ANALYSIS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "job_id": {
                "type": "string",
                "description": "ID of the analysis job to cancel"
            }
        },
        "required": ["job_id"]
    })
});

// ============================================================================
// Search Schemas
// ============================================================================

/// Schema for the `search_functions` tool.
pub static SEARCH_FUNCTIONS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for function names"
            },
            "limit": {
                "type": "number",
                "description": "Maximum results (default: 20)"
            },
            "offset": {
                "type": "number",
                "description": "Result offset for pagination (default: 0)"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file path filter"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `search_classes` tool.
pub static SEARCH_CLASSES_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for class names"
            },
            "limit": {
                "type": "number",
                "description": "Maximum results (default: 20)"
            },
            "offset": {
                "type": "number",
                "description": "Result offset for pagination (default: 0)"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file path filter"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `fuzzy_search` tool.
pub static FUZZY_SEARCH_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Fuzzy search query"
            },
            "limit": {
                "type": "number",
                "description": "Maximum results (default: 10)"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `semantic_search` tool.
pub static SEMANTIC_SEARCH_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Natural language search query"
            },
            "entity_type": {
                "type": "string",
                "description": "Optional filter by entity type (e.g. 'function', 'class', 'section'). Omit to search every type."
            },
            "limit": {
                "type": "number",
                "description": "Maximum results (default: 10)"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `generate_embeddings` tool.
pub static GENERATE_EMBEDDINGS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "force": {
                "type": "boolean",
                "description": "Re-embed entities that already have vectors (default: false)"
            },
            "batch_size": {
                "type": "number",
                "description": "ONNX batch size for embedding generation (default: 16, clamped to 1-256)"
            }
        }
    })
});

// ============================================================================
// Entity Details Schemas
// ============================================================================

/// Schema for the `get_entity_details` tool.
pub static GET_ENTITY_DETAILS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "entity_type": {
                "type": "string",
                "description": "Type of entity (function, class, etc.)"
            },
            "name": {
                "type": "string",
                "description": "Name of the entity"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file path to disambiguate"
            }
        },
        "required": ["entity_type", "name"]
    })
});

/// Schema for the `get_relationships` tool.
pub static GET_RELATIONSHIPS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "entity_name": {
                "type": "string",
                "description": "Name of the entity"
            },
            "entity_type": {
                "type": "string",
                "description": "Only entities of this type (`function`, `class`, …); entities of every type by default"
            },
            "file_path": {
                "type": "string",
                "description": "Only entities defined in a file whose path contains this: tells apart entities of one name"
            },
            "relationship_type": {
                "type": "string",
                "description": "Only edges of this kind (`calls`, `called_by`, `uses`, `inherits`, `has_method`, …); without it each edge is given once, as the source's (`calls`, not also `called_by`)"
            }
        },
        "required": ["entity_name"]
    })
});

// ============================================================================
// Analysis Schemas
// ============================================================================

/// Schema for the `find_dead_code` tool.
pub static FIND_DEAD_CODE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "default": ".",
                "description": "Directory whose functions and classes are analyzed (what uses them is looked for everywhere); `.`, the default, is the whole index"
            },
            "min_confidence": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0,
                "description": "Drop findings below this confidence. An unreferenced private entity scores 0.9; one its language exports 0.5 (it may be used outside the codebase); one whose name is written outside its definition, or a method of a type extending one the index does not hold, 0.3 (it may be reached in a way no reference records — another language, a template, a program a script runs, a framework's base class). Default: 0.5"
            },
            "limit": {
                "type": "number",
                "description": "Maximum findings, the surest first (default: 20)"
            }
        }
    })
});

/// Schema for the `analyze_complexity` tool.
pub static ANALYZE_COMPLEXITY_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "default": ".",
                "description": "Directory whose functions are analyzed; `.`, the default, is the whole index"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file to analyze (matched as a substring)"
            },
            "top_n": {
                "type": "number",
                "description": "Number of most complex functions to return (default: 10)"
            }
        }
    })
});

// ============================================================================
// Context Graph Schemas
// ============================================================================

/// Schema for the `add_context_note` tool.
pub static ADD_CONTEXT_NOTE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "note_type": {
                "type": "string",
                "description": "One of: adr, standard, optimization, failure, practice, reflection, general, file_description, modification",
                "enum": ["adr", "standard", "optimization", "failure", "practice", "reflection", "general", "file_description", "modification"]
            },
            "title": {
                "type": "string",
                "description": "Title of the note"
            },
            "content": {
                "type": "string",
                "description": "Content of the note"
            },
            "summary": {
                "type": "string",
                "description": "Optional summary"
            },
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional tags"
            },
            "priority": {
                "type": "number",
                "description": "Priority 1-10 (default: 5)"
            },
            "author": {
                "type": "string",
                "description": "Note author (default: 'agent')"
            },
            "related_entities": {
                "type": "array",
                "description": "Links to concrete code entities",
                "items": {
                    "type": "object",
                    "properties": {
                        "entity_type": { "type": "string", "description": "Entity type, e.g. 'function', 'class' (required)" },
                        "entity_id": { "type": "string", "description": "Optional entity UUID" },
                        "entity_name": { "type": "string", "description": "Optional entity name" },
                        "file_path": { "type": "string", "description": "Optional file path" },
                        "link_type": { "type": "string", "description": "Link type (default: 'references')" }
                    },
                    "required": ["entity_type"]
                }
            },
            "related_concepts": {
                "type": "array",
                "description": "Links to abstract concepts/modules/patterns",
                "items": {
                    "type": "object",
                    "properties": {
                        "concept_type": { "type": "string", "description": "Concept type, e.g. 'module', 'pattern' (required)" },
                        "concept_name": { "type": "string", "description": "Concept name (required)" },
                        "concept_path": { "type": "string", "description": "Optional concept path" },
                        "link_type": { "type": "string", "description": "Link type (default: 'references')" }
                    },
                    "required": ["concept_type", "concept_name"]
                }
            }
        },
        "required": ["note_type", "title", "content"]
    })
});

/// Schema for the `search_context` tool.
pub static SEARCH_CONTEXT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query"
            },
            "top_k": {
                "type": "number",
                "description": "Number of results (default: 10)"
            },
            "search_type": {
                "type": "string",
                "description": "Search strategy (default: keyword). 'semantic' and 'hybrid' use vector similarity and fall back to keyword when embeddings are unavailable.",
                "enum": ["keyword", "semantic", "hybrid"]
            },
            "note_type": {
                "type": "string",
                "description": "Optional filter by note type",
                "enum": ["adr", "standard", "optimization", "failure", "practice", "reflection", "general", "file_description", "modification"]
            },
            "status": {
                "type": "string",
                "description": "Optional filter by status",
                "enum": ["active", "superseded", "archived"]
            },
            "min_priority": {
                "type": "number",
                "description": "Optional minimum priority (1-10)"
            },
            "max_content_length": {
                "type": "number",
                "description": "Maximum content length per result in characters (default: 500)"
            }
        },
        "required": ["query"]
    })
});

/// Schema for the `get_related_context` tool.
pub static GET_RELATED_CONTEXT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "entity_type": {
                "type": "string",
                "description": "Type of entity"
            },
            "entity_name": {
                "type": "string",
                "description": "Name of the entity"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file path"
            },
            "max_content_length": {
                "type": "number",
                "description": "Maximum content length per result in characters (default: 300)"
            }
        },
        "required": ["entity_type"]
    })
});

/// Schema for the `prune_context` tool.
pub static PRUNE_CONTEXT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "max_age_days": {
                "type": "number",
                "description": "Maximum age in days (default: 90)"
            },
            "max_notes": {
                "type": "number",
                "description": "Cap the number of retained active notes; the lowest-priority, least-recently-updated surplus is archived (never deleted). Omit for no cap."
            },
            "dry_run": {
                "type": "boolean",
                "description": "If true, only report what would be removed (default: true)"
            }
        }
    })
});

/// Schema for the `get_context_statistics` tool.
pub static GET_CONTEXT_STATISTICS_SCHEMA: LazyLock<Value> =
    LazyLock::new(|| json!({ "type": "object", "properties": {} }));

// ============================================================================
// File Description Schemas
// ============================================================================

/// Schema for the `describe_file` tool.
pub static DESCRIBE_FILE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "Path to the file"
            },
            "include_content": {
                "type": "boolean",
                "description": "Whether to include file content preview (default: true)"
            }
        },
        "required": ["file_path"]
    })
});

/// Schema for the `describe_modification` tool.
pub static DESCRIBE_MODIFICATION_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "Path to the modified file"
            },
            "original_content": {
                "type": "string",
                "description": "Original file content"
            },
            "modified_content": {
                "type": "string",
                "description": "Modified file content"
            }
        },
        "required": ["file_path", "original_content", "modified_content"]
    })
});

/// Schema for the `get_file_context` tool.
pub static GET_FILE_CONTEXT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "Path to the file"
            },
            "context_lines": {
                "type": "number",
                "description": "Number of context lines (default: 10)"
            }
        },
        "required": ["file_path"]
    })
});

// ============================================================================
// Utility Schemas
// ============================================================================

/// Schema for the `health_check` tool.
pub static HEALTH_CHECK_SCHEMA: LazyLock<Value> =
    LazyLock::new(|| json!({ "type": "object", "properties": {} }));

/// Shared empty-object schema for tools that take no parameters.
pub static EMPTY_OBJECT_SCHEMA: LazyLock<Value> =
    LazyLock::new(|| json!({ "type": "object", "properties": {} }));

/// Schema for the `start_watching` tool.
pub static START_WATCHING_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Directory to watch and keep the knowledge graph current for"
            },
            "debounce_ms": {
                "type": "number",
                "description": "Quiet period in milliseconds before a re-scan runs (default: 400)"
            },
            "embed": {
                "type": "boolean",
                "description": "Re-embed changed entities inline after each re-scan (default: false). Needs 4 GB of free RAM; with less, the watcher runs without embedding and the response says so in `embed_skipped`"
            }
        },
        "required": ["path"]
    })
});

/// Schema for the `validate_file_edit` tool.
pub static VALIDATE_FILE_EDIT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "Path to the file to validate"
            },
            "original_content": {
                "type": "string",
                "description": "Optional original file content as read by the client. If provided, the server verifies the file has not changed since."
            }
        },
        "required": ["file_path"]
    })
});

/// Schema for the `query_with_context` tool.
pub static QUERY_WITH_CONTEXT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "What to find, in words or as a name (e.g. 'where are shipping costs computed?' or 'apply_bulk_discount'). Context Graph notes that point at code are consulted first; otherwise the code is searched by keyword and meaning together."
            },
            "entity_type": {
                "type": "string",
                "description": "Optional filter ('function', 'class', etc.)"
            },
            "file_path": {
                "type": "string",
                "description": "Optional file path filter (matched as a substring)"
            },
            "force_broad_search": {
                "type": "boolean",
                "description": "If true, skip the Context Graph and search the code directly"
            }
        },
        "required": ["query"]
    })
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify every `TOOL_IDS` key has a matching schema static, and vice versa.
    /// Prevents schema drift between the dispatch table and schema definitions (MCP-8).
    #[test]
    fn test_tool_ids_match_schemas() {
        let schema_map: std::collections::HashMap<&str, &Value> = [
            ("scan_codebase", &*SCAN_CODEBASE_SCHEMA),
            ("search_entities", &*SEARCH_ENTITIES_SCHEMA),
            ("analyze_code", &*ANALYZE_CODE_SCHEMA),
            ("get_statistics", &*GET_STATISTICS_SCHEMA),
            ("start_analysis", &*START_ANALYSIS_SCHEMA),
            ("get_analysis_progress", &*GET_ANALYSIS_PROGRESS_SCHEMA),
            ("list_analysis_jobs", &*LIST_ANALYSIS_JOBS_SCHEMA),
            ("cleanup_old_jobs", &*CLEANUP_OLD_JOBS_SCHEMA),
            ("search_functions", &*SEARCH_FUNCTIONS_SCHEMA),
            ("search_classes", &*SEARCH_CLASSES_SCHEMA),
            ("fuzzy_search", &*FUZZY_SEARCH_SCHEMA),
            ("semantic_search", &*SEMANTIC_SEARCH_SCHEMA),
            ("generate_embeddings", &*GENERATE_EMBEDDINGS_SCHEMA),
            ("get_entity_details", &*GET_ENTITY_DETAILS_SCHEMA),
            ("get_relationships", &*GET_RELATIONSHIPS_SCHEMA),
            ("find_dead_code", &*FIND_DEAD_CODE_SCHEMA),
            ("analyze_complexity", &*ANALYZE_COMPLEXITY_SCHEMA),
            ("add_context_note", &*ADD_CONTEXT_NOTE_SCHEMA),
            ("search_context", &*SEARCH_CONTEXT_SCHEMA),
            ("get_related_context", &*GET_RELATED_CONTEXT_SCHEMA),
            ("prune_context", &*PRUNE_CONTEXT_SCHEMA),
            ("get_context_statistics", &*GET_CONTEXT_STATISTICS_SCHEMA),
            ("describe_file", &*DESCRIBE_FILE_SCHEMA),
            ("describe_modification", &*DESCRIBE_MODIFICATION_SCHEMA),
            ("get_file_context", &*GET_FILE_CONTEXT_SCHEMA),
            ("health_check", &*HEALTH_CHECK_SCHEMA),
            ("validate_file_edit", &*VALIDATE_FILE_EDIT_SCHEMA),
            ("cancel_analysis", &*CANCEL_ANALYSIS_SCHEMA),
            ("query_with_context", &*QUERY_WITH_CONTEXT_SCHEMA),
            ("start_watching", &*START_WATCHING_SCHEMA),
            ("stop_watching", &*EMPTY_OBJECT_SCHEMA),
            ("get_watch_status", &*EMPTY_OBJECT_SCHEMA),
        ]
        .into_iter()
        .collect();

        // Every TOOL_IDS key must have a schema
        for tool_name in crate::core_module::TOOL_IDS.keys() {
            assert!(
                schema_map.contains_key(tool_name),
                "TOOL_IDS key '{tool_name}' has no matching schema static"
            );
        }

        // Every schema must have a TOOL_IDS key
        for schema_name in schema_map.keys() {
            assert!(
                crate::core_module::TOOL_IDS.contains_key(schema_name),
                "Schema '{schema_name}' has no matching TOOL_IDS entry"
            );
        }

        assert_eq!(
            crate::core_module::TOOL_IDS.len(),
            schema_map.len(),
            "TOOL_IDS and schema count must match"
        );
    }
}
