//! What each tool does to its environment, declared to clients as MCP tool
//! annotations: a `title`, `readOnlyHint`, `destructiveHint`,
//! `idempotentHint` and `openWorldHint`.
//!
//! A client that knows which tools only read can run them without asking and
//! hand them to read-only agents; without annotations it has to guess from a
//! tool's name. Every tool the server lists has an entry here, and a test
//! fails for a tool that has none.
//!
//! Starting a language server to answer a query counts as reading: every LSP
//! query starts its server on first use. Changes to Devscriptor's own state —
//! the index, context notes, analysis jobs, the watcher, running servers —
//! are changes to the environment, like edits to files.

/// How a tool affects its environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Reads the index, the files or a language server, and changes nothing.
    Read,
    /// Adds or refreshes state without removing any; `idempotent` when
    /// repeating the call with the same arguments changes nothing more.
    Additive { idempotent: bool },
    /// Rewrites files, deletes records or stops work; `idempotent` as above.
    Destructive { idempotent: bool },
}

/// One tool's declared behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolEffect {
    pub name: &'static str,
    /// The human-readable name a client shows.
    pub title: &'static str,
    pub effect: Effect,
    /// Whether the tool reaches beyond the workspace and the machine
    /// (downloads, for one).
    pub open_world: bool,
}

const fn read(name: &'static str, title: &'static str) -> ToolEffect {
    ToolEffect {
        name,
        title,
        effect: Effect::Read,
        open_world: false,
    }
}

const fn adds(name: &'static str, title: &'static str, idempotent: bool) -> ToolEffect {
    ToolEffect {
        name,
        title,
        effect: Effect::Additive { idempotent },
        open_world: false,
    }
}

const fn destroys(name: &'static str, title: &'static str, idempotent: bool) -> ToolEffect {
    ToolEffect {
        name,
        title,
        effect: Effect::Destructive { idempotent },
        open_world: false,
    }
}

/// Every tool's behaviour, core tools first, then LSP and LSP-architecture
/// tools.
pub static TOOL_EFFECTS: &[ToolEffect] = &[
    // Scanning and analysis jobs.
    adds("scan_codebase", "Scan codebase", true),
    read("analyze_code", "Analyze code"),
    adds("start_analysis", "Start background analysis", false),
    read("get_analysis_progress", "Analysis progress"),
    read("list_analysis_jobs", "List analysis jobs"),
    destroys("cleanup_old_jobs", "Remove old analysis jobs", true),
    destroys("cancel_analysis", "Cancel analysis", true),
    // Statistics and health.
    read("get_statistics", "Index statistics"),
    read("health_check", "Health check"),
    // Search and embeddings.
    read("search_entities", "Search entities"),
    read("search_functions", "Search functions"),
    read("search_classes", "Search classes and types"),
    read("fuzzy_search", "Fuzzy search"),
    read("semantic_search", "Semantic search"),
    adds("generate_embeddings", "Generate embeddings", true),
    // Code quality.
    read("find_dead_code", "Find dead code"),
    read("analyze_complexity", "Analyze complexity"),
    // Entities and relationships.
    read("get_entity_details", "Entity details"),
    read("get_relationships", "Entity relationships"),
    // Context graph.
    adds("add_context_note", "Add context note", false),
    read("search_context", "Search context notes"),
    read("get_related_context", "Notes about an entity"),
    destroys("prune_context", "Prune context notes", true),
    read("get_context_statistics", "Context graph statistics"),
    // File context and edit validation.
    read("describe_file", "Describe file"),
    read("describe_modification", "Describe a modification"),
    read("get_file_context", "File context"),
    read("validate_file_edit", "Validate file edit"),
    read("query_with_context", "Query with context"),
    // Real-time watcher.
    adds("start_watching", "Start watcher", true),
    destroys("stop_watching", "Stop watcher", true),
    read("get_watch_status", "Watcher status"),
    // LSP navigation.
    read("get_hover", "Hover"),
    read("get_definition", "Go to definition"),
    read("get_references", "Find references"),
    read("get_type_definitions", "Type definitions"),
    read("get_implementations", "Implementations"),
    // LSP completion.
    read("get_completions", "Completions"),
    read("get_completion_resolve", "Resolve completion"),
    // LSP diagnostics and code actions. Resolving an action returns its
    // edits; it applies none.
    read("get_diagnostics", "Diagnostics"),
    read("get_code_actions", "Code actions"),
    read("get_code_action_resolve", "Resolve code action"),
    // LSP symbols.
    read("get_document_symbols", "Document symbols"),
    read("get_workspace_symbols", "Workspace symbols"),
    read("get_symbols", "File symbols"),
    read("get_project_symbols", "Project symbols"),
    // LSP call and type hierarchies.
    read("get_call_hierarchy", "Call hierarchy"),
    read("get_incoming_calls", "Incoming calls"),
    read("get_outgoing_calls", "Outgoing calls"),
    read("get_type_hierarchy", "Type hierarchy"),
    read("get_subtypes", "Subtypes"),
    read("get_supertypes", "Supertypes"),
    // LSP document features.
    read("get_highlights", "Document highlights"),
    read("get_colors", "Document colors"),
    read("get_folding_ranges", "Folding ranges"),
    read("get_selection_range", "Selection ranges"),
    read("get_linked_editing_range", "Linked editing ranges"),
    read("get_inlay_hints", "Inlay hints"),
    read("get_links", "Document links"),
    read("get_semantic_tokens", "Semantic tokens"),
    // LSP edits, written to the files on disk. A second rename by name can
    // pick another symbol of that name, so it is not idempotent.
    destroys("format_document", "Format document", true),
    destroys("get_range_format", "Format range", true),
    destroys("rename_symbol", "Rename symbol", false),
    // Language server management.
    read("get_server_status", "Language server status"),
    read("get_server_capabilities", "Language server capabilities"),
    read("get_server_projects", "Language server projects"),
    read("get_project_files", "Language server project files"),
    adds("start_server", "Start language server", true),
    destroys("stop_server", "Stop language server", true),
    destroys("restart_server", "Restart language server", false),
    // LSP architecture. Detection starts the servers already installed, as a
    // query would; setup can download servers.
    read("lsp_arch_detect", "Detect languages and servers"),
    read("lsp_arch_check", "Check a language server"),
    read("lsp_arch_batch_check", "Check language servers"),
    read("lsp_arch_status", "Language server resolution"),
    ToolEffect {
        name: "lsp_arch_setup",
        title: "Set up language servers",
        effect: Effect::Additive { idempotent: true },
        open_world: true,
    },
];

/// The declared behaviour of the tool called `name`.
#[must_use]
pub fn effect_of(name: &str) -> Option<&'static ToolEffect> {
    TOOL_EFFECTS.iter().find(|effect| effect.name == name)
}

impl ToolEffect {
    /// The annotations as `tools/list` carries them.
    #[cfg(feature = "rmcp-sdk")]
    #[must_use]
    pub fn annotations(&self) -> rmcp::model::ToolAnnotations {
        let annotations =
            rmcp::model::ToolAnnotations::with_title(self.title).open_world(self.open_world);
        match self.effect {
            Effect::Read => annotations.read_only(true),
            Effect::Additive { idempotent } => annotations
                .read_only(false)
                .destructive(false)
                .idempotent(idempotent),
            Effect::Destructive { idempotent } => annotations
                .read_only(false)
                .destructive(true)
                .idempotent(idempotent),
        }
    }

    /// The annotations as a JSON object, for a server that builds its
    /// `tools/list` answer by hand.
    #[must_use]
    pub fn annotations_json(&self) -> serde_json::Value {
        let mut annotations = serde_json::json!({
            "title": self.title,
            "readOnlyHint": self.effect == Effect::Read,
            "openWorldHint": self.open_world,
        });
        if let Effect::Additive { idempotent } | Effect::Destructive { idempotent } = self.effect {
            annotations["destructiveHint"] =
                serde_json::Value::Bool(matches!(self.effect, Effect::Destructive { .. }));
            annotations["idempotentHint"] = serde_json::Value::Bool(idempotent);
        }
        annotations
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_tool_is_declared_once() {
        let mut seen = HashSet::new();
        for effect in TOOL_EFFECTS {
            assert!(
                seen.insert(effect.name),
                "{} is declared twice",
                effect.name
            );
            assert!(!effect.title.is_empty(), "{} has no title", effect.name);
        }
        assert_eq!(TOOL_EFFECTS.len(), 75);
        for name in crate::core_module::TOOL_IDS.keys() {
            assert!(
                effect_of(name).is_some(),
                "core tool {name} is not declared"
            );
        }
    }

    /// The tools that write files or delete records are never offered as
    /// read-only, and only setup reaches the network.
    #[test]
    fn writes_and_downloads_are_declared() {
        for name in [
            "format_document",
            "get_range_format",
            "rename_symbol",
            "prune_context",
        ] {
            assert_eq!(
                effect_of(name).map(|e| e.effect),
                Some(Effect::Destructive {
                    idempotent: name != "rename_symbol"
                }),
                "{name}"
            );
        }
        for name in ["scan_codebase", "add_context_note", "generate_embeddings"] {
            assert!(
                matches!(
                    effect_of(name).map(|e| e.effect),
                    Some(Effect::Additive { .. })
                ),
                "{name}"
            );
        }
        let open: Vec<_> = TOOL_EFFECTS
            .iter()
            .filter(|e| e.open_world)
            .map(|e| e.name)
            .collect();
        assert_eq!(open, ["lsp_arch_setup"]);
    }

    #[test]
    fn the_json_form_carries_the_hints_that_apply() {
        let search = effect_of("semantic_search").unwrap().annotations_json();
        assert_eq!(
            search,
            serde_json::json!({
                "title": "Semantic search",
                "readOnlyHint": true,
                "openWorldHint": false,
            })
        );
        let rename = effect_of("rename_symbol").unwrap().annotations_json();
        assert_eq!(rename["readOnlyHint"], false);
        assert_eq!(rename["destructiveHint"], true);
        assert_eq!(rename["idempotentHint"], false);
    }

    #[cfg(feature = "rmcp-sdk")]
    #[test]
    fn the_sdk_form_matches_the_json_form() {
        for effect in TOOL_EFFECTS {
            assert_eq!(
                serde_json::to_value(effect.annotations()).unwrap(),
                effect.annotations_json(),
                "{}",
                effect.name
            );
        }
    }
}
