//! The input schemas of the 38 LSP tools, as `tools/list` gives them.

use super::*;

pub(crate) fn create_schema(
    obj: serde_json::Value,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    match obj {
        serde_json::Value::Object(map) => std::sync::Arc::new(map),
        _ => std::sync::Arc::new(serde_json::Map::new()),
    }
}

/// Input schema for a position-based LSP tool.
///
/// Every such tool accepts a symbol `name` (resolved from the code index) as an
/// alternative to raw `line`/`character` coordinates — the difference between an
/// agent naming `apply_discount` and one guessing its 0-indexed column. `extra`
/// properties (e.g. `include_declaration`) are merged in. Nothing is strictly
/// required: the handler resolves a `name` when no explicit position is given,
/// and errors clearly when it has neither.
pub(super) fn position_schema(
    extra: serde_json::Value,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let mut properties = json!({
        "name": {
            "type": "string",
            "description": "Symbol to resolve from the code index (function, class, method, …) — use instead of line/character. Requires the project to have been scanned."
        },
        "entity_type": {
            "type": "string",
            "description": "Optional disambiguator for `name`: function, class, method, struct, etc."
        },
        "file_path": {
            "type": "string",
            "description": "Source file. Required alongside line/character; optional with `name`, where it disambiguates a name defined in several files."
        },
        "line": {"type": "integer", "description": "0-indexed line. Provide with `character`, or omit and pass `name`."},
        "character": {"type": "integer", "description": "0-indexed column on `line`, in UTF-16 code units (the LSP default): one per character, two for a character outside the Basic Multilingual Plane such as an emoji."}
    });
    if let (Some(dst), Some(src)) = (properties.as_object_mut(), extra.as_object()) {
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    create_schema(json!({
        "type": "object",
        "properties": properties,
        // `name` OR `line`+`character`. That is a disjunction JSON-Schema
        // expresses only through `oneOf`, which many MCP clients validate
        // poorly, so both stay optional and the handler enforces the choice.
        "required": []
    }))
}

/// Input schema for a tool that continues a call or type hierarchy: the `item`
/// that `prepared_by` returned, or — as [`position_schema`] takes it — the
/// symbol itself, for which the hierarchy is then prepared.
pub(super) fn hierarchy_item_schema(
    prepared_by: &str,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let position = json!({
        "type": "object",
        "properties": {"line": {"type": "integer"}, "character": {"type": "integer"}},
        "required": ["line", "character"]
    });
    position_schema(json!({
        "item": {
            "type": "object",
            // `data` is left out of `properties`: the server's own payload has
            // no one type to declare, and every property here has one. An item
            // passed back unchanged carries it all the same.
            "description": format!(
                "One of `{prepared_by}`'s `items`, passed back unchanged, its `data` \
                 included. Or leave it out and name the symbol instead: `name`, or \
                 `file_path` with `line` and `character`."
            ),
            "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string"},
                "location": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "range": {
                            "type": "object",
                            "properties": {"start": position, "end": position},
                            "required": ["start", "end"]
                        }
                    },
                    "required": ["file_path", "range"]
                },
                "selection_range": {
                    "type": "object",
                    "description": "The range of the symbol's name, as received.",
                    "properties": {"start": position, "end": position}
                },
                "container_name": {"type": "string"}
            },
            "required": ["name", "kind", "location"]
        }
    }))
}

/// Build the 38 LSP tool schema definitions for [`list_tools`].
pub fn tool_list() -> Vec<Tool> {
    vec![
        // Navigation
        Tool::new(
            "get_hover",
            "Get hover info (type signature/docs) for a symbol.",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_definition",
            "Find where a symbol is defined.",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_references",
            "Find all references to a symbol. The uses the code index records (calls, reads of constants and properties, functions passed, type uses, imports) that the language server did not report, in files it cannot read such as `.vue` and `.svelte` files for the TypeScript server, come in `index_locations`, by line.",
            position_schema(json!({
                "include_declaration": {"type": "boolean"}
            })),
        ),
        Tool::new(
            "get_type_definitions",
            "Find a symbol's type definition(s).",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_implementations",
            "Find implementations",
            position_schema(json!({})),
        ),
        // Completion
        Tool::new(
            "get_completions",
            "Get code completions",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_completion_resolve",
            "Resolve completion item details",
            create_schema(json!({
                "type": "object",
                "properties": {"item": {"type": "object", "description": "An item returned by `get_completions`. Pass it back unchanged; it must contain `label`."}},
                "required": ["item"]
            })),
        ),
        // Diagnostics & Actions
        Tool::new(
            "get_diagnostics",
            "Get diagnostics (errors/warnings) for a file, as it is on disk; `severity` \
             leaves out the less severe, `wait_ms` bounds the wait for a server that pushes \
             them, and `stale: true` says the answer may describe an older text of the file",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "severity": {
                        "type": "string",
                        "enum": ["error", "warning", "information", "hint"],
                        "description": "The least severe diagnostics to report; `error` reports errors alone. A diagnostic without a severity counts as an error. Default: all"
                    },
                    "wait_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The longest to wait, in milliseconds, for a server that pushes its diagnostics (10 s the first time, 3 s after a change, by default)"
                    }
                },
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_code_actions",
            "Get available code actions",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "get_code_action_resolve",
            "Resolve code action details",
            create_schema(json!({
                "type": "object",
                "properties": {"action": {"type": "object", "description": "An item returned by `get_code_actions`. Pass it back unchanged; it must contain `title`."}},
                "required": ["action"]
            })),
        ),
        // Symbols
        Tool::new(
            "get_document_symbols",
            "Get all symbols in a file, parent first: each at its name, with the symbol it is declared in as `container_name`",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_workspace_symbols",
            "Search symbols across the workspace: every running language server that offers it is asked, and their answers merged; one that could not answer is named in `note`",
            create_schema(json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            })),
        ),
        Tool::new(
            "get_symbols",
            "Get symbols for a file as get_document_symbols gives them, a page at a time",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                },
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_project_symbols",
            "Search a language server's symbols by name, a page at a time",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "Language server to query, e.g. 'rust'. Optional when exactly one server is running."
                    },
                    "project": {
                        "type": "string",
                        "description": "Only the symbols under this directory, whose files are opened in the server first: gopls and typescript-language-server search only the projects of open files. Relative paths are taken from the workspace root."
                    },
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                }
            })),
        ),
        // Call Hierarchy
        Tool::new(
            "get_call_hierarchy",
            "Prepare the call hierarchy for a symbol (feed the result to get_incoming_calls/get_outgoing_calls).",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_incoming_calls",
            "Get the calls into a function: pass an `item` from `get_call_hierarchy`, or name the function (`name`, or `file_path` with `line`/`character`) and the call hierarchy is prepared for you. Calls the code index records that the language server did not report (in files it cannot read, such as `.vue` and `.svelte` files for the TypeScript server) come in `index_calls`, each with its caller and line.",
            hierarchy_item_schema("get_call_hierarchy"),
        ),
        Tool::new(
            "get_outgoing_calls",
            "Get the calls a function makes: pass an `item` from `get_call_hierarchy`, or name the function (`name`, or `file_path` with `line`/`character`) and the call hierarchy is prepared for you.",
            hierarchy_item_schema("get_call_hierarchy"),
        ),
        // Type Hierarchy
        Tool::new(
            "get_type_hierarchy",
            "Get type hierarchy",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_subtypes",
            "Get a type's subtypes: pass an `item` from `get_type_hierarchy`, or name the type (`name`, or `file_path` with `line`/`character`) and the type hierarchy is prepared for you.",
            hierarchy_item_schema("get_type_hierarchy"),
        ),
        Tool::new(
            "get_supertypes",
            "Get a type's supertypes: pass an `item` from `get_type_hierarchy`, or name the type (`name`, or `file_path` with `line`/`character`) and the type hierarchy is prepared for you.",
            hierarchy_item_schema("get_type_hierarchy"),
        ),
        // Document Features
        Tool::new(
            "get_highlights",
            "Get document highlights",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_colors",
            "Get document colors",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_folding_ranges",
            "Get folding ranges",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_selection_range",
            "Get selection ranges",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "positions": {"type": "array"}
                },
                "required": ["file_path", "positions"]
            })),
        ),
        Tool::new(
            "get_linked_editing_range",
            "Get linked editing ranges",
            position_schema(json!({})),
        ),
        Tool::new(
            "get_inlay_hints",
            "Get inlay hints",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "get_links",
            "Get document links",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_semantic_tokens",
            "Get semantic tokens",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        // Formatting
        Tool::new(
            "format_document",
            "Format a document with the language server and write the formatted result to disk. Returns the edits that were applied.",
            create_schema(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"]
            })),
        ),
        Tool::new(
            "get_range_format",
            "Format a line/character range with the language server and write the result to disk. Returns the edits that were applied.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "start_line": {"type": "integer"},
                    "start_character": {"type": "integer"},
                    "end_line": {"type": "integer"},
                    "end_character": {"type": "integer"}
                },
                "required": ["file_path", "start_line", "start_character", "end_line", "end_character"]
            })),
        ),
        Tool::new(
            "rename_symbol",
            "Rename a symbol across the whole workspace, applying the edits to every affected file on disk; returns the per-file changes applied.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Symbol to rename, resolved from the code index — use instead of line/character."},
                    "entity_type": {"type": "string", "description": "Optional disambiguator for `name`: function, class, method, etc."},
                    "file_path": {"type": "string"},
                    "line": {"type": "integer"},
                    "character": {"type": "integer"},
                    "new_name": {"type": "string", "description": "The new identifier."}
                },
                "required": ["new_name"]
            })),
        ),
        // Server Management
        Tool::new(
            "get_server_status",
            "Get server status",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "One server's status, e.g. 'rust'. Omit for every configured server's."
                    }
                }
            })),
        ),
        Tool::new(
            "get_server_capabilities",
            "Get server capabilities",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {
                        "type": "string",
                        "description": "Language server to query, e.g. 'rust'. Optional when exactly one server is running."
                    }
                }
            })),
        ),
        Tool::new(
            "get_server_projects",
            "Get the folders a language server has open: `language_id`'s, or the workspace's when none is given",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {"type": "string", "description": "A running server, e.g. 'rust'."}
                }
            })),
        ),
        Tool::new(
            "get_project_files",
            "Get a project's files, sorted, without hidden files and what its ignore files leave out (`.git`, a `.gitignore`d `node_modules`), a page at a time",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "The project's directory, the workspace root when none is given. Relative paths are taken from the workspace root."
                    },
                    "limit": {"type": "integer", "default": 100},
                    "offset": {"type": "integer", "default": 0}
                }
            })),
        ),
        Tool::new(
            "start_server",
            "Start a language server: for `project`, or for the workspace when none is given. A server already running keeps running, restarted only to take a project it does not cover yet.",
            create_schema(json!({
                "type": "object",
                "properties": {
                    "language_id": {"type": "string", "description": "Language of the server, e.g. 'rust'."},
                    "project": {
                        "type": "string",
                        "description": "A path in the project to start it for — the project is the outermost directory above it with the server's manifest (e.g. Cargo.toml). Relative paths are taken from the workspace root."
                    }
                },
                "required": ["language_id"]
            })),
        ),
        Tool::new(
            "stop_server",
            "Stop a language server",
            create_schema(json!({
                "type": "object",
                "properties": {"language_id": {"type": "string"}},
                "required": ["language_id"]
            })),
        ),
        Tool::new(
            "restart_server",
            "Restart a language server",
            create_schema(json!({
                "type": "object",
                "properties": {"language_id": {"type": "string"}},
                "required": ["language_id"]
            })),
        ),
    ]
}
