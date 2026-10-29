//! MCP tool parameter types.
//!
//! This module defines all parameter and result types for the MCP tools
//! that expose LSP capabilities to AI agents.

use serde::{Deserialize, Serialize};

// ============================================================================
// Existing Tools (10)
// ============================================================================

/// Parameters for hover tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of hover tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverResult {
    /// Hover contents (markdown).
    pub contents: String,
}

/// Parameters for go-to-definition tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of go-to-definition tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionResult {
    /// Definition locations.
    pub locations: Vec<Location>,
}

/// Parameters for find-references tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferencesParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
    /// Include declaration in results.
    #[serde(default = "default_true")]
    pub include_declaration: bool,
}

fn default_true() -> bool {
    true
}

/// Result of find-references tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferencesResult {
    /// Reference locations, as the language server reported them.
    pub locations: Vec<Location>,
    /// Uses of the symbol that the code index records — calls, type uses and
    /// the imports naming it — and the server did not report: in files the
    /// server cannot read (a `.vue` or `.svelte` file, for
    /// typescript-language-server) or has not analysed (Metals before a
    /// build). Matched by name, so they want checking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index_locations: Vec<IndexLocation>,
    /// What `index_locations` are; absent when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ReferencesResult {
    /// Add the use sites of `name` the index records, `(file, 1-based line)`,
    /// that are not among the server's locations, and say what they are.
    pub fn add_index_uses(&mut self, name: &str, sites: impl IntoIterator<Item = (String, u32)>) {
        let reported: std::collections::HashSet<(&str, u32)> = self
            .locations
            .iter()
            .map(|l| (l.file_path.as_str(), l.range.start.line.saturating_add(1)))
            .collect();
        let mut missing: Vec<(String, u32)> = sites
            .into_iter()
            .filter(|(file, line)| !reported.contains(&(file.as_str(), *line)))
            .collect();
        missing.sort();
        missing.dedup();
        self.index_locations.extend(
            missing
                .into_iter()
                .map(|(file_path, line)| IndexLocation::new(file_path, line)),
        );
        if !self.index_locations.is_empty() {
            self.note = Some(format!(
                "The language server did not report {} use(s) of `{name}` that the code index \
                 records, listed in `index_locations`: in files it cannot read or has not \
                 analysed. The index matches by name, so check them.",
                self.index_locations.len()
            ));
        }
    }
}

/// A use of a symbol that the code index records, by line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexLocation {
    /// Path to the file.
    pub file_path: String,
    /// The line, 1-based.
    pub line: u32,
    /// `path:line`: what to quote.
    pub cite: String,
}

impl IndexLocation {
    #[must_use]
    pub fn new(file_path: String, line: u32) -> Self {
        let cite = format!("{file_path}:{line}");
        Self {
            file_path,
            line,
            cite,
        }
    }
}

/// Parameters for completions tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionsParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of completions tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionsResult {
    /// Completion items.
    pub items: Vec<Completion>,
}

/// Parameters for diagnostics tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of diagnostics tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsResult {
    /// Diagnostics (errors/warnings).
    pub diagnostics: Vec<Diagnostic>,
}

/// Parameters for document symbols tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSymbolsParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of document symbols tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSymbolsResult {
    /// Document symbols.
    pub symbols: Vec<Symbol>,
}

/// Parameters for workspace symbol search tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbolParams {
    /// Search query.
    pub query: String,
}

/// Result of workspace symbol search tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbolResult {
    /// Workspace symbols, from every running server that answered.
    pub symbols: Vec<WorkspaceSymbol>,
    /// The servers that could not answer, and why; absent when all did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Parameters for format document tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatDocumentParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of format document tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatDocumentResult {
    /// Text edits to apply.
    pub edits: Vec<TextEdit>,
}

/// Parameters for code actions tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActionsParams {
    /// Path to the file.
    pub file_path: String,
    /// Range in the document.
    pub range: Range,
}

/// Result of code actions tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActionsResult {
    /// Available code actions.
    pub actions: Vec<CodeAction>,
}

/// Parameters for rename tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
    /// New name for the symbol.
    pub new_name: String,
}

/// Result of rename tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResult {
    /// Document changes to apply.
    pub changes: Vec<DocumentChanges>,
}

// ============================================================================
// NEW: Signature Help
// ============================================================================

/// Parameters for signature help tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureHelpParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of signature help tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureHelpResult {
    /// Signatures available.
    pub signatures: Vec<Signature>,
    /// Active signature index.
    pub active_signature: Option<u32>,
    /// Active parameter index.
    pub active_parameter: Option<u32>,
}

/// Signature information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    /// Signature label.
    pub label: String,
    /// Documentation.
    pub documentation: Option<String>,
    /// Parameters.
    pub parameters: Vec<Parameter>,
}

/// Parameter information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter label.
    pub label: String,
    /// Documentation.
    pub documentation: Option<String>,
}

// ============================================================================
// NEW: Type Definition
// ============================================================================

/// Parameters for type definition tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDefinitionParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of type definition tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeDefinitionResult {
    /// Type definition locations.
    pub locations: Vec<Location>,
}

// ============================================================================
// NEW: Implementation
// ============================================================================

/// Parameters for implementation tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of implementation tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationResult {
    /// Implementation locations.
    pub locations: Vec<Location>,
}

// ============================================================================
// NEW: Call Hierarchy
// ============================================================================

/// Parameters for call hierarchy preparation tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallHierarchyParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of call hierarchy preparation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallHierarchyResult {
    /// Call hierarchy items.
    pub items: Vec<CallHierarchyItem>,
}

/// Call hierarchy item.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallHierarchyItem {
    /// Name of the symbol.
    pub name: String,
    /// Kind of symbol.
    pub kind: String,
    /// Location: the whole symbol, body included.
    pub location: Location,
    /// The range of the symbol's name within `location`: what a server resolves
    /// the item by when it is passed back. Rebuilt from `location` instead, it
    /// would point at the start of the whole symbol — `pub` rather than the
    /// name — and rust-analyzer would answer every incoming-calls request with
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_range: Option<Range>,
    /// Where the symbol's name is, as `path:line:column`, 1-based ([`cite`]):
    /// what to quote.
    #[serde(default)]
    pub cite: String,
    /// Container name.
    pub container_name: Option<String>,
    /// Data for resolving calls. Optional per the LSP spec (`data?: LSPAny`); a
    /// caller that echoes a `get_call_hierarchy` item back may legitimately omit
    /// it, so default to `null` rather than rejecting the request.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Parameters for incoming calls tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingCallsParams {
    /// Call hierarchy item.
    pub item: CallHierarchyItem,
}

/// Result of incoming calls tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IncomingCallsResult {
    /// Incoming call items, as the language server reported them.
    pub calls: Vec<CallIncoming>,
    /// Calls of the symbol that the code index records and the server did not
    /// report, with what each is made in: in files the server cannot read (a
    /// `.vue` or `.svelte` file, for typescript-language-server) or has not
    /// analysed. Matched by name, so they want checking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index_calls: Vec<IndexCall>,
    /// What `index_calls` are; absent when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl IncomingCallsResult {
    /// Add the calls of `name` the index records, `(file, 1-based line,
    /// caller)`, that are not at a line the server reported a call from, and
    /// say what they are.
    pub fn add_index_calls(
        &mut self,
        name: &str,
        sites: impl IntoIterator<Item = (String, u32, String)>,
    ) {
        let reported: std::collections::HashSet<(&str, u32)> = self
            .calls
            .iter()
            .flat_map(|call| {
                call.from_ranges.iter().map(|range| {
                    (
                        call.from.location.file_path.as_str(),
                        range.start.line.saturating_add(1),
                    )
                })
            })
            .collect();
        let mut missing: Vec<(String, u32, String)> = sites
            .into_iter()
            .filter(|(file, line, _)| !reported.contains(&(file.as_str(), *line)))
            .collect();
        missing.sort();
        missing.dedup();
        self.index_calls.extend(
            missing
                .into_iter()
                .map(|(file_path, line, caller)| IndexCall::new(caller, file_path, line)),
        );
        if !self.index_calls.is_empty() {
            self.note = Some(format!(
                "The language server did not report {} call(s) of `{name}` that the code index \
                 records, listed in `index_calls`: in files it cannot read or has not analysed. \
                 The index matches by name, so check them.",
                self.index_calls.len()
            ));
        }
    }
}

/// A call of a symbol that the code index records: what it is made in, and
/// where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexCall {
    /// The function, method or other entity the call is made in.
    pub caller: String,
    /// Path to the file.
    pub file_path: String,
    /// The line of the call, 1-based.
    pub line: u32,
    /// `path:line`: what to quote.
    pub cite: String,
}

impl IndexCall {
    #[must_use]
    pub fn new(caller: String, file_path: String, line: u32) -> Self {
        let cite = format!("{file_path}:{line}");
        Self {
            caller,
            file_path,
            line,
            cite,
        }
    }
}

/// Incoming call information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallIncoming {
    /// From location.
    pub from: CallHierarchyItem,
    /// From ranges.
    pub from_ranges: Vec<Range>,
}

/// Parameters for outgoing calls tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingCallsParams {
    /// Call hierarchy item.
    pub item: CallHierarchyItem,
}

/// Result of outgoing calls tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingCallsResult {
    /// Outgoing call items.
    pub calls: Vec<CallOutgoing>,
}

/// Outgoing call information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallOutgoing {
    /// To location.
    pub to: CallHierarchyItem,
    /// From ranges.
    pub from_ranges: Vec<Range>,
}

// ============================================================================
// NEW: Type Hierarchy
// ============================================================================

/// Parameters for type hierarchy preparation tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeHierarchyParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of type hierarchy preparation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeHierarchyResult {
    /// Type hierarchy items.
    pub items: Vec<TypeHierarchyItem>,
}

/// Type hierarchy item.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TypeHierarchyItem {
    /// Name of the type.
    pub name: String,
    /// Kind of type.
    pub kind: String,
    /// Location: the whole type, body included.
    pub location: Location,
    /// The range of the type's name within `location`; see
    /// [`CallHierarchyItem::selection_range`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_range: Option<Range>,
    /// Where the type's name is, as `path:line:column`, 1-based ([`cite`]):
    /// what to quote.
    #[serde(default)]
    pub cite: String,
    /// Data for resolving hierarchy. Optional per the LSP spec, as for
    /// [`CallHierarchyItem::data`].
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Parameters for subtypes tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtypesParams {
    /// Type hierarchy item.
    pub item: TypeHierarchyItem,
}

/// Result of subtypes tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtypesResult {
    /// Subtype items.
    pub subtypes: Vec<TypeHierarchyItem>,
}

/// Parameters for supertypes tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupertypesParams {
    /// Type hierarchy item.
    pub item: TypeHierarchyItem,
}

/// Result of supertypes tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupertypesResult {
    /// Supertype items.
    pub supertypes: Vec<TypeHierarchyItem>,
}

// ============================================================================
// NEW: Document Highlights
// ============================================================================

/// Parameters for document highlights tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentHighlightsParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of document highlights tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentHighlightsResult {
    /// Highlight locations.
    pub highlights: Vec<DocumentHighlight>,
}

/// Document highlight information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentHighlight {
    /// Range of the highlight.
    pub range: Range,
    /// Kind of highlight (text, read, write).
    pub kind: Option<String>,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. `range` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
}

// ============================================================================
// NEW: Document Colors
// ============================================================================

/// Parameters for document colors tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentColorsParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of document colors tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentColorsResult {
    /// Color information.
    pub colors: Vec<ColorInformation>,
}

/// Color information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorInformation {
    /// Range of the color.
    pub range: Range,
    /// Color value.
    pub color: Color,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. `range` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
}

/// RGB color representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Color {
    /// Red component (0-1).
    pub red: f64,
    /// Green component (0-1).
    pub green: f64,
    /// Blue component (0-1).
    pub blue: f64,
    /// Alpha component (0-1).
    pub alpha: f64,
}

// ============================================================================
// NEW: Folding Ranges
// ============================================================================

/// Parameters for folding ranges tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldingRangesParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of folding ranges tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldingRangesResult {
    /// Folding ranges.
    pub ranges: Vec<FoldingRange>,
}

/// Folding range information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldingRange {
    /// Start line (0-indexed).
    pub start_line: u32,
    /// Start character (0-indexed).
    pub start_character: Option<u32>,
    /// End line (0-indexed).
    pub end_line: u32,
    /// End character (0-indexed).
    pub end_character: Option<u32>,
    /// Kind of folding range (comment, imports, region).
    pub kind: Option<String>,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. The other fields are LSP's, 0-based.
    #[serde(default)]
    pub cite: String,
}

// ============================================================================
// NEW: Selection Range
// ============================================================================

/// Parameters for selection range tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRangeParams {
    /// Path to the file.
    pub file_path: String,
    /// Positions to get selection ranges for.
    pub positions: Vec<Position2D>,
}

/// Result of selection range tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRangeResult {
    /// Selection ranges.
    pub ranges: Vec<SelectionRange>,
}

/// Selection range information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRange {
    /// Range of this selection.
    pub range: Range,
    /// Parent selection range (for nested ranges).
    pub parent: Option<Box<SelectionRange>>,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. `range` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
}

// ============================================================================
// NEW: Linked Editing Range
// ============================================================================

/// Parameters for linked editing range tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedEditingRangeParams {
    /// Path to the file.
    pub file_path: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Result of linked editing range tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedEditingRangeResult {
    /// Ranges that should be edited together.
    pub ranges: Vec<Range>,
    /// Word pattern for valid renames.
    pub word_pattern: Option<String>,
}

// ============================================================================
// NEW: Inlay Hints
// ============================================================================

/// Parameters for inlay hints tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlayHintsParams {
    /// Path to the file.
    pub file_path: String,
    /// Start line (0-indexed).
    pub start_line: u32,
    /// Start character (0-indexed).
    pub start_character: u32,
    /// End line (0-indexed).
    pub end_line: u32,
    /// End character (0-indexed).
    pub end_character: u32,
}

/// Result of inlay hints tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlayHintsResult {
    /// Inlay hints.
    pub hints: Vec<InlayHint>,
}

/// Inlay hint information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlayHint {
    /// Position of the hint.
    pub position: Position2D,
    /// Label to display.
    pub label: String,
    /// Kind of hint (type, parameter).
    pub kind: Option<String>,
    /// Tooltip text.
    pub tooltip: Option<String>,
    /// Where it is as `path:line:column`, 1-based ([`cite`]): what to quote.
    /// `position` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
}

// ============================================================================
// NEW: Document Links
// ============================================================================

/// Parameters for document links tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentLinksParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of document links tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentLinksResult {
    /// Document links.
    pub links: Vec<DocumentLink>,
}

/// Document link information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentLink {
    /// Range of the link.
    pub range: Range,
    /// Target URL (optional, may require resolution).
    pub target: Option<String>,
    /// Tooltip.
    pub tooltip: Option<String>,
    /// Data for resolving the link.
    pub data: Option<serde_json::Value>,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. `range` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
}

// ============================================================================
// NEW: Semantic Tokens
// ============================================================================

/// Parameters for semantic tokens tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticTokensParams {
    /// Path to the file.
    pub file_path: String,
}

/// Result of semantic tokens tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticTokensResult {
    /// Result ID for delta requests.
    pub result_id: Option<String>,
    /// Token data (encoded as per LSP spec).
    pub data: Vec<u32>,
}

// ============================================================================
// NEW: Formatting Variants
// ============================================================================

/// Parameters for range formatting tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatRangeParams {
    /// Path to the file.
    pub file_path: String,
    /// Range to format.
    pub range: Range,
}

/// Result of range formatting tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatRangeResult {
    /// Text edits to apply.
    pub edits: Vec<TextEdit>,
}

// ============================================================================
// NEW: Resolution Tools
// ============================================================================

/// Parameters for completion resolve tool.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionResolveParams {
    /// Path to the file.
    pub file_path: String,
    /// Completion item to resolve.
    pub item: Completion,
}

/// Result of completion resolve tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResolveResult {
    /// Resolved completion item.
    pub item: Completion,
}

/// Parameters for code action resolve tool.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeActionResolveParams {
    /// Path to the file.
    pub file_path: String,
    /// Code action to resolve.
    pub action: CodeAction,
}

/// Result of code action resolve tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActionResolveResult {
    /// Resolved code action.
    pub action: CodeAction,
}

// ============================================================================
// NEW: Server Management
// ============================================================================

/// Parameters for start server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartServerParams {
    /// Language ID.
    pub language_id: String,
    /// Project path (optional).
    pub project: Option<String>,
}

/// Result of start server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartServerResult {
    /// Whether the server started successfully.
    pub success: bool,
    /// Server status message.
    pub message: String,
}

/// Parameters for stop server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopServerParams {
    /// Language ID.
    pub language_id: String,
}

/// Result of stop server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopServerResult {
    /// Whether the server stopped successfully.
    pub success: bool,
    /// Server status message.
    pub message: String,
}

/// Parameters for restart server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartServerParams {
    /// Language ID.
    pub language_id: String,
}

/// Result of restart server tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartServerResult {
    /// Whether the server restarted successfully.
    pub success: bool,
    /// Server status message.
    pub message: String,
}

/// Parameters for get server status tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusParams {
    /// Language ID (optional, if not provided returns all servers).
    pub language_id: Option<String>,
}

/// Result of server status tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusResult {
    /// Server statuses.
    pub servers: Vec<ServerStatus>,
}

/// Server status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    /// Language ID.
    pub language_id: String,
    /// Server state.
    pub state: String,
    /// Process ID (if running).
    pub pid: Option<u32>,
    /// Uptime in seconds (if running).
    pub uptime: Option<u64>,
    /// Last error message (if any).
    pub last_error: Option<String>,
    /// Why a `stopped` server is not running: `not started`, `idle`,
    /// `requested` (by `stop_server`) or `cap`. Absent unless stopped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stop_reason: Option<String>,
}

/// Parameters for get server capabilities tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilitiesParams {
    /// Language ID.
    pub language_id: String,
}

/// Result of server capabilities tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilitiesResult {
    /// Server capabilities.
    pub capabilities: serde_json::Value,
}

/// Parameters for get server projects tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerProjectsParams {
    /// The server whose folders to list; empty for the workspace's.
    #[serde(default)]
    pub language_id: String,
}

/// Result of server projects tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerProjectsResult {
    /// Project paths.
    pub projects: Vec<String>,
}

// ============================================================================
// NEW: Workspace File Operations
// ============================================================================

/// Parameters for get project files tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFilesParams {
    /// The project's directory; a relative path is taken from the workspace
    /// root.
    pub project: String,
    /// Pagination limit.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Pagination offset.
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    100
}

/// Result of project files tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFilesResult {
    /// Files in the project.
    pub files: Vec<String>,
    /// Pagination metadata.
    pub pagination: PaginationInfo,
}

/// Pagination information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationInfo {
    /// Total number of items.
    pub total: u32,
    /// Current offset.
    pub offset: u32,
    /// Whether there are more items.
    pub has_more: bool,
}

// ============================================================================
// NEW: Symbol Variants with Pagination
// ============================================================================

/// Parameters for get symbols with pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolsParams {
    /// Path to the file.
    pub file_path: String,
    /// Pagination limit.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Pagination offset.
    #[serde(default)]
    pub offset: u32,
}

/// Result of symbols tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolsResult {
    /// Document symbols.
    pub symbols: Vec<Symbol>,
    /// Pagination info.
    pub pagination: PaginationInfo,
}

/// Parameters for project symbols with pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSymbolsParams {
    /// Language ID.
    pub language_id: String,
    /// Only the symbols under this directory, when not empty; a relative
    /// path is taken from the workspace root.
    #[serde(default)]
    pub project: String,
    /// Search query.
    #[serde(default = "default_empty")]
    pub query: String,
    /// Pagination limit.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Pagination offset.
    #[serde(default)]
    pub offset: u32,
}

fn default_empty() -> String {
    String::new()
}

/// Result of project symbols tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSymbolsResult {
    /// Workspace symbols.
    pub symbols: Vec<WorkspaceSymbol>,
    /// Pagination info.
    pub pagination: PaginationInfo,
}

// ============================================================================
// LSP Architecture Tools - LSP Detection Workflow (5 tools)
// ============================================================================

/// Parameters for `lsp_arch_detect` tool.
/// Analyzes a project, detects languages, and returns resolved/needed/missing servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchDetectParams {
    /// Path to the project to analyze.
    pub project_path: String,
}

/// A detected language in the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedLanguageInfo {
    /// Language ID (e.g., "rust", "typescript", "python").
    pub language_id: String,
    /// Confidence level (0.0 - 1.0).
    pub confidence: f32,
    /// Files found that indicate this language.
    pub indicator_files: Vec<String>,
    /// Suggested LSP server IDs.
    pub suggested_servers: Vec<String>,
}

/// LSP source type for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspSourceType {
    /// User-provided configuration.
    UserConfig,
    /// Installed on demand by Devscriptor.
    Installed,
    /// Bundled with Devscriptor.
    Bundled,
    /// Found in system PATH or SDK paths.
    SystemWide,
    /// Found in VS Code extensions.
    VsCodeExtension,
}

/// LSP source information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSourceInfo {
    /// Source type.
    pub source_type: LspSourceType,
    /// Path to the LSP binary.
    pub path: String,
    /// Additional args (if user config).
    pub args: Option<Vec<String>>,
}

impl From<&crate::lsp_architecture::LspSource> for LspSourceInfo {
    fn from(source: &crate::lsp_architecture::LspSource) -> Self {
        use crate::lsp_architecture::LspSource;
        match source {
            LspSource::UserConfig { path, args } => Self {
                source_type: LspSourceType::UserConfig,
                path: path.to_string_lossy().to_string(),
                args: Some(args.clone()),
            },
            LspSource::Installed(path) => Self {
                source_type: LspSourceType::Installed,
                path: path.to_string_lossy().to_string(),
                args: None,
            },
            LspSource::Bundled(path) => Self {
                source_type: LspSourceType::Bundled,
                path: path.to_string_lossy().to_string(),
                args: None,
            },
            LspSource::SystemWide(path) => Self {
                source_type: LspSourceType::SystemWide,
                path: path.to_string_lossy().to_string(),
                args: None,
            },
            LspSource::VsCodeExtension(path) => Self {
                source_type: LspSourceType::VsCodeExtension,
                path: path.to_string_lossy().to_string(),
                args: None,
            },
        }
    }
}

/// A resolved server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedServer {
    /// Server ID.
    pub server_id: String,
    /// LSP source information.
    pub source: LspSourceInfo,
}

/// Prerequisite information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrerequisiteInfo {
    /// Prerequisite name.
    pub name: String,
    /// Installation instructions.
    pub install_instructions: String,
}

/// Server status for setup workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspServerStatus {
    /// Server is available/resolved.
    Available,
    /// Server needs installation (prerequisites satisfied).
    NeedsInstall,
    /// Server has missing prerequisites.
    MissingPrerequisites,
    /// Server is installed, but its install cannot run here.
    CannotRun,
}

/// Server needing installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerNeedsInstall {
    /// Server ID.
    pub server_id: String,
    /// Why it does not run here, and what makes it run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Satisfied prerequisites.
    pub satisfied_prerequisites: Vec<PrerequisiteInfo>,
    /// Missing prerequisites.
    pub missing_prerequisites: Vec<PrerequisiteInfo>,
}

/// Server with missing prerequisites.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMissingPrerequisites {
    /// Server ID.
    pub server_id: String,
    /// Why it does not run here, and what makes it run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Missing prerequisites.
    pub missing_prerequisites: Vec<PrerequisiteInfo>,
}

/// Result of `lsp_arch_detect` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchDetectResult {
    /// Detected languages in the project.
    pub languages: Vec<DetectedLanguageInfo>,
    /// Resolved servers (found and available).
    pub resolved_servers: Vec<ResolvedServer>,
    /// Servers that need installation (prerequisites satisfied).
    pub needs_install: Vec<ServerNeedsInstall>,
    /// Servers with missing prerequisites.
    pub missing_prerequisites: Vec<ServerMissingPrerequisites>,
    /// Languages whose installed servers this call started in the background
    /// (lazy startup only), so that the first LSP request finds them
    /// running. Empty when none starts.
    #[serde(default)]
    pub prewarming: Vec<String>,
}

/// Parameters for `lsp_arch_check` tool.
/// Check if a specific LSP server is available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchCheckParams {
    /// Server ID to check (e.g., "rust-analyzer", "typescript-language-server").
    pub server_id: String,
}

/// Result of `lsp_arch_check` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchCheckResult {
    /// Server ID that was checked.
    pub server_id: String,
    /// The name the call gave, when it named the server by its language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    /// Whether the server is available.
    pub available: bool,
    /// Source information if available.
    pub source: Option<LspSourceInfo>,
    /// Why it is not available, and what makes it run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why nothing was checked: the name is no server and no language a
    /// server serves. Names the server ids there are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

/// Parameters for `lsp_arch_batch_check` tool.
/// Check multiple LSP servers at once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchBatchCheckParams {
    /// List of server IDs to check.
    pub server_ids: Vec<String>,
}

/// Result for a single server in batch check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCheckResultItem {
    /// Server ID.
    pub server_id: String,
    /// The name the call gave, when it named the server by its language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    /// Whether the server is available.
    pub available: bool,
    /// Source information if available.
    pub source: Option<LspSourceInfo>,
    /// Why it is not available, and what makes it run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Result of `lsp_arch_batch_check` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchBatchCheckResult {
    /// Results for each server.
    pub results: Vec<BatchCheckResultItem>,
    /// Number of available servers.
    pub available_count: usize,
    /// Number of known servers that are not available.
    pub missing_count: usize,
    /// The names that are no server and no language a server serves, which
    /// were not checked, and the server ids there are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<UnknownServers>,
}

/// Names a check could not take, and what it takes instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnknownServers {
    /// The names given that are no server and no language a server serves.
    pub names: Vec<String>,
    /// The server ids there are.
    pub server_ids: Vec<String>,
}

/// Server status info for status listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusInfo {
    /// Server ID.
    pub server_id: String,
    /// Server status.
    pub status: LspServerStatus,
    /// Source information (if available).
    pub source: Option<LspSourceInfo>,
    /// Missing prerequisites (if any).
    pub missing_prerequisites: Option<Vec<PrerequisiteInfo>>,
    /// Why it is not available, and what makes it run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Servers grouped by source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServersBySource {
    /// Bundled servers.
    pub bundled: Vec<ServerStatusInfo>,
    /// System-wide servers.
    pub system: Vec<ServerStatusInfo>,
    /// Installed-by-Devscriptor servers.
    pub installed: Vec<ServerStatusInfo>,
    /// VS Code extension servers.
    pub vscode: Vec<ServerStatusInfo>,
    /// User config servers.
    pub user_config: Vec<ServerStatusInfo>,
    /// Servers needing installation.
    pub needs_install: Vec<ServerStatusInfo>,
    /// Servers with missing prerequisites.
    pub missing_prerequisites: Vec<ServerStatusInfo>,
    /// Servers installed here whose install cannot run: built for another
    /// Ruby, another build than the pinned one, or holding no binary.
    #[serde(default)]
    pub cannot_run: Vec<ServerStatusInfo>,
}

/// Parameters for `lsp_arch_status` tool.
/// List all available servers grouped by source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchStatusParams {
    /// Optional language filter.
    #[serde(default)]
    pub language_id: Option<String>,
}

/// Result of `lsp_arch_status` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchStatusResult {
    /// All servers grouped by source.
    pub servers: ServersBySource,
    /// Total number of servers checked.
    pub total_checked: usize,
    /// Number of available servers.
    pub available_count: usize,
}

/// Installation result for a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInstallResult {
    /// Server ID.
    pub server_id: String,
    /// Whether installation was successful.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Parameters for `lsp_arch_setup` tool.
/// Full setup workflow with optional auto-installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchSetupParams {
    /// Path to the project to set up.
    pub project_path: String,
    /// Whether to auto-install missing servers.
    #[serde(default)]
    pub auto_install: bool,
    /// Specific servers to install (if empty and `auto_install` is true, installs all needed).
    #[serde(default)]
    pub servers_to_install: Vec<String>,
}

/// Result of `lsp_arch_setup` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspArchSetupResult {
    /// Detected languages.
    pub languages: Vec<DetectedLanguageInfo>,
    /// Servers that were already available.
    pub already_available: Vec<ResolvedServer>,
    /// Servers that were installed (if `auto_install` was true).
    pub installed: Vec<ServerInstallResult>,
    /// Servers that still need installation (if `auto_install` was false or failed).
    pub still_needed: Vec<ServerNeedsInstall>,
    /// Servers with missing prerequisites.
    pub missing_prerequisites: Vec<ServerMissingPrerequisites>,
    /// Whether setup is complete.
    pub setup_complete: bool,
    /// Summary message.
    pub summary: String,
}

// ============================================================================
// Common Types
// ============================================================================

/// 2D position in a document.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Position2D {
    /// Line number (0-indexed).
    pub line: u32,
    /// Character position (0-indexed).
    pub character: u32,
}

/// Range in a document.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Range {
    /// Start position.
    pub start: Position2D,
    /// End position.
    pub end: Position2D,
}

/// `path:line:column` for a 0-based LSP position, counted from 1 as editors
/// and people count: line 41, character 4 of `a.rs` is `a.rs:42:5`. LSP
/// positions are 0-based, while search results and editors are 1-based, and
/// models converting between them cited lines off by one.
#[must_use]
pub fn cite(path: &str, line: u32, character: u32) -> String {
    format!(
        "{path}:{}:{}",
        line.saturating_add(1),
        character.saturating_add(1)
    )
}

/// Location in a document.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Location {
    /// Path to the file.
    pub file_path: String,
    /// Range in the file, LSP's: 0-based, in UTF-16 code units.
    pub range: Range,
    /// Where the range starts as `path:line:column`, 1-based ([`cite`]):
    /// what to quote.
    #[serde(default)]
    pub cite: String,
}

impl Location {
    /// The location of `range` in `file_path`, with its [`cite`].
    #[must_use]
    pub fn new(file_path: String, range: Range) -> Self {
        let cite = cite(&file_path, range.start.line, range.start.character);
        Self {
            file_path,
            range,
            cite,
        }
    }
}

/// Symbol information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Symbol name.
    pub name: String,
    /// Symbol kind (e.g., "function", "class").
    pub kind: String,
    /// Location in the document.
    pub location: Location,
    /// Container name (optional).
    pub container_name: Option<String>,
}

/// Workspace symbol information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbol {
    /// Symbol name.
    pub name: String,
    /// Symbol kind.
    pub kind: String,
    /// Location in the workspace.
    pub location: Location,
    /// Container name (optional).
    pub container_name: Option<String>,
}

/// Completion item.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Completion {
    /// Label to display.
    pub label: String,
    /// Kind of completion.
    pub kind: Option<String>,
    /// Additional details.
    pub detail: Option<String>,
    /// Documentation.
    pub documentation: Option<String>,
    /// Text to insert.
    pub insert_text: Option<String>,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSeverity {
    /// Error severity.
    Error,
    /// Warning severity.
    Warning,
    /// Information severity.
    Information,
    /// Hint severity.
    Hint,
}

/// Diagnostic information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Location of the diagnostic.
    pub range: Range,
    /// Where it starts as `path:line:column`, 1-based ([`cite`]): what to
    /// quote. `range` is LSP's, 0-based in UTF-16 code units.
    #[serde(default)]
    pub cite: String,
    /// Severity level (optional).
    pub severity: Option<DiagnosticSeverity>,
    /// Diagnostic code (optional).
    pub code: Option<String>,
    /// Source of the diagnostic.
    pub source: Option<String>,
    /// Diagnostic message.
    pub message: String,
}

/// Text edit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    /// Range to replace.
    pub range: Range,
    /// New text.
    pub new_text: String,
}

/// Document changes for rename operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChanges {
    /// Path to the file.
    pub file_path: String,
    /// Edits to apply.
    pub edits: Vec<TextEdit>,
}

/// Code action information.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeAction {
    /// Title of the action.
    pub title: String,
    /// Kind of action (optional).
    pub kind: Option<String>,
    /// Workspace edit (optional).
    pub edit: Option<WorkspaceEdit>,
    /// Command to execute (optional).
    pub command: Option<String>,
}

/// Workspace edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    /// Changes per file.
    pub changes: Vec<(String, Vec<TextEdit>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hover_params_serialization() {
        let params = HoverParams {
            file_path: "/test/file.rs".into(),
            line: 10,
            character: 5,
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("file_path"));
        assert!(json.contains("/test/file.rs"));
    }

    fn reference(file: &str, line: u32) -> Location {
        Location::new(
            file.to_string(),
            Range {
                start: Position2D { line, character: 4 },
                end: Position2D { line, character: 9 },
            },
        )
    }

    /// The index's uses fill in what the server could not see — a `.vue`
    /// file, for typescript-language-server — and never repeat one it
    /// reported; with nothing missing, the answer is the server's alone.
    #[test]
    fn index_uses_add_only_what_the_server_missed() {
        let mut refs = ReferencesResult {
            locations: vec![reference("/p/test/validate.test.js", 8)],
            ..ReferencesResult::default()
        };
        refs.add_index_uses(
            "windowProblems",
            [
                ("/p/test/validate.test.js".to_string(), 9),
                ("/p/src/BerthWindowForm.vue".to_string(), 12),
                ("/p/src/BerthWindowForm.vue".to_string(), 12),
            ],
        );
        assert_eq!(
            refs.index_locations,
            [IndexLocation::new(
                "/p/src/BerthWindowForm.vue".to_string(),
                12
            )]
        );
        assert_eq!(
            refs.index_locations[0].cite,
            "/p/src/BerthWindowForm.vue:12"
        );
        assert!(
            refs.note
                .as_deref()
                .is_some_and(|n| n.contains("`windowProblems`"))
        );

        let mut complete = ReferencesResult {
            locations: vec![reference("/p/a.js", 0)],
            ..ReferencesResult::default()
        };
        complete.add_index_uses("f", [("/p/a.js".to_string(), 1)]);
        assert!(complete.index_locations.is_empty() && complete.note.is_none());
        let json = serde_json::to_value(&complete).unwrap();
        assert!(json.get("index_locations").is_none() && json.get("note").is_none());
    }

    /// The index's calls fill in the callers the server could not see — a
    /// `.vue` component, for typescript-language-server — each with what the
    /// call is made in, and never repeat a call the server reported.
    #[test]
    fn index_calls_add_only_what_the_server_missed() {
        let caller = |file: &str, line: u32| CallIncoming {
            from: CallHierarchyItem {
                name: "test".into(),
                kind: "Function".into(),
                location: reference(file, line),
                selection_range: None,
                cite: String::new(),
                container_name: None,
                data: serde_json::Value::Null,
            },
            from_ranges: vec![Range {
                start: Position2D {
                    line: 8,
                    character: 9,
                },
                end: Position2D {
                    line: 8,
                    character: 23,
                },
            }],
        };
        let mut calls = IncomingCallsResult {
            calls: vec![caller("/p/test/validate.test.js", 7)],
            ..IncomingCallsResult::default()
        };
        calls.add_index_calls(
            "windowProblems",
            [
                (
                    "/p/test/validate.test.js".to_string(),
                    9,
                    "test".to_string(),
                ),
                (
                    "/p/src/BerthWindowForm.vue".to_string(),
                    12,
                    "BerthWindowForm".to_string(),
                ),
            ],
        );
        assert_eq!(
            calls.index_calls,
            [IndexCall::new(
                "BerthWindowForm".to_string(),
                "/p/src/BerthWindowForm.vue".to_string(),
                12
            )]
        );
        assert_eq!(calls.index_calls[0].cite, "/p/src/BerthWindowForm.vue:12");
        assert!(
            calls
                .note
                .as_deref()
                .is_some_and(|n| n.contains("call(s) of `windowProblems`"))
        );

        let mut complete = IncomingCallsResult::default();
        complete.add_index_calls("f", []);
        let json = serde_json::to_value(&complete).unwrap();
        assert!(json.get("index_calls").is_none() && json.get("note").is_none());
    }

    #[test]
    fn a_cite_counts_lines_and_columns_from_one() {
        assert_eq!(cite("a.rs", 41, 4), "a.rs:42:5");
        let location = Location::new(
            "a.rs".into(),
            Range {
                start: Position2D {
                    line: 41,
                    character: 4,
                },
                end: Position2D {
                    line: 41,
                    character: 9,
                },
            },
        );
        assert_eq!(location.cite, "a.rs:42:5");
        assert_eq!(location.range.start.line, 41, "the range stays 0-based");
    }

    #[test]
    fn test_position_2d_default_values() {
        let pos = Position2D {
            line: 1,
            character: 1,
        };
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 1);
    }

    #[test]
    fn test_diagnostic_severity_serialization() {
        let sev = DiagnosticSeverity::Error;
        let json = serde_json::to_string(&sev).unwrap();
        assert_eq!(json, "\"error\"");
    }

    #[test]
    fn test_references_params_default_include_declaration() {
        let json = r#"{"file_path":"test.rs","line":1,"character":1}"#;
        let params: ReferencesParams = serde_json::from_str(json).unwrap();
        assert!(params.include_declaration);
    }

    #[test]
    fn test_signature_params_serialization() {
        let params = SignatureHelpParams {
            file_path: "/test/file.rs".into(),
            line: 10,
            character: 5,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("file_path"));
        assert!(json.contains("/test/file.rs"));
    }

    #[test]
    fn test_call_hierarchy_params() {
        let params = CallHierarchyParams {
            file_path: "/test/file.rs".into(),
            line: 10,
            character: 5,
        };
        assert_eq!(params.line, 10);
    }

    #[test]
    fn test_document_colors_params() {
        let params = DocumentColorsParams {
            file_path: "/test/file.css".into(),
        };
        assert_eq!(params.file_path, "/test/file.css");
    }

    #[test]
    fn test_folding_ranges_params() {
        let params = FoldingRangesParams {
            file_path: "/test/file.rs".into(),
        };
        assert_eq!(params.file_path, "/test/file.rs");
    }

    #[test]
    fn test_inlay_hints_params() {
        let params = InlayHintsParams {
            file_path: "/test/file.rs".into(),
            start_line: 1,
            start_character: 1,
            end_line: 10,
            end_character: 1,
        };
        assert_eq!(params.end_line, 10);
    }

    #[test]
    fn test_semantic_tokens_params() {
        let params = SemanticTokensParams {
            file_path: "/test/file.rs".into(),
        };
        assert_eq!(params.file_path, "/test/file.rs");
    }

    #[test]
    fn test_server_status_params() {
        let params = ServerStatusParams {
            language_id: Some("rust".into()),
        };
        assert_eq!(params.language_id, Some("rust".into()));
    }

    #[test]
    fn test_pagination_defaults() {
        let params: ProjectFilesParams =
            serde_json::from_str(r#"{"language_id":"rust","project":"/test"}"#).unwrap();
        assert_eq!(params.limit, 100);
        assert_eq!(params.offset, 0);
    }
}
