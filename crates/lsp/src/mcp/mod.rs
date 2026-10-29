//! MCP tool parameter and result types.
//!
//! The unified MCP server (`devscriptor-mcp`) exposes the LSP capabilities
//! to AI agents with these types.

pub mod tools;

pub use tools::{
    CodeActionsParams, CodeActionsResult, Completion, CompletionsParams, CompletionsResult,
    DefinitionParams, DefinitionResult, Diagnostic, DiagnosticSeverity, DiagnosticsParams,
    DiagnosticsResult, DocumentChanges, DocumentSymbolsParams, DocumentSymbolsResult,
    FormatDocumentParams, FormatDocumentResult, HoverParams, HoverResult, Location, Position2D,
    Range, ReferencesParams, ReferencesResult, RenameParams, RenameResult, Symbol, TextEdit,
    WorkspaceEdit, WorkspaceSymbol, WorkspaceSymbolParams, WorkspaceSymbolResult,
};
