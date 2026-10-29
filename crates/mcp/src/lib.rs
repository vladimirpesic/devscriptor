//! Devscriptor MCP Server Library
//!
//! Provides the MCP server implementation for Devscriptor.
//!
//! # Feature Flags
//!
//! - `rmcp-sdk` (default): the MCP server, on the rmcp crate
//! - `lsp`: the 38 LSP and 5 architecture tools (requires rmcp-sdk)

// ============================================================================
// Core Module (always available)
// ============================================================================

pub mod core_module;
pub mod tool_annotations;
pub mod tool_schemas;

// Re-export core types
pub use core_module::{CoreModule, ToolError, ToolResult};

// ============================================================================
// Server State & Tools (always available — used by CoreModule)
// ============================================================================

pub mod freshness;
pub mod project;
pub mod server;
pub mod tools;

/// In-memory analysis job (rich type with `JobStatus` enum).
///
/// For the JSON wire-format equivalent, see [`tools::AnalysisJobWire`].
pub use server::{AnalysisJob, DevscriptorServerState, JobStatus};

// Re-export specific tool types to avoid wildcard pollution.
// `tools::AnalysisJob` is a wire-format mirror of `server::AnalysisJob::to_dict()`;
// access it explicitly via `tools::AnalysisJob` to avoid confusion.
pub use tools::{
    AnalysisJobWire, ClassInfo, FunctionInfo, FuzzySearchRequest, FuzzySearchResponse,
    FuzzySearchResult, GetJobProgressRequest, SearchClassesRequest, SearchClassesResponse,
    SearchFunctionsRequest, SearchFunctionsResponse, SemanticSearchRequest, SemanticSearchResponse,
    SemanticSearchResult, StartAnalysisRequest, StartAnalysisResponse,
};

// ============================================================================
// Unified MCP Server (rmcp-sdk feature)
// ============================================================================

#[cfg(feature = "rmcp-sdk")]
pub mod unified_server;

#[cfg(feature = "rmcp-sdk")]
pub use unified_server::run_core_only_server as run_rmcp_server;

#[cfg(feature = "rmcp-sdk")]
pub use unified_server::UnifiedMcpServer;

#[cfg(feature = "lsp")]
pub use unified_server::{ServerOptions, run_unified_server};

#[cfg(feature = "lsp")]
pub mod daemon;

/// Language server lifecycle settings that [`ServerOptions`] carries.
#[cfg(feature = "lsp")]
pub use devscriptor_lsp::config::{LifecycleOverrides, StartupMode};

// ============================================================================
// Version
// ============================================================================

/// MCP server crate version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
