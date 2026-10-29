//! CLI commands module
//!
//! This module contains all CLI command implementations organized by functionality.

pub mod analyze;
pub mod architecture;
pub mod cache;
pub mod call_graph;
pub mod common;
pub mod context;
pub mod database;
pub mod doctor;
pub mod embed;
#[cfg(feature = "lsp")]
pub mod lsp;
// The MCP server entry point is gated on `mcp`, independently of `lsp`.
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod performance;
pub mod profile;
pub mod repl;
pub mod scan;
pub mod search;
#[cfg(feature = "mcp")]
pub mod serve;
pub mod template;
pub mod user_templates;
pub mod validate;
pub mod watch;

pub use common::{
    AnalysisKind, CallGraphFormat, OutputFormat, ReportFormat, SearchType, print_banner,
};
