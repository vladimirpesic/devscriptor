//! `devscriptor-mcp-standalone` — the standalone MCP server binary.
//!
//! Provides Model Context Protocol tools for code analysis and exploration.
//! Fully integrated with devscriptor-core library.
//!
//! # This is not the binary AI agents run
//!
//! Agents run the universal `devscriptor` binary via `devscriptor mcp`, which
//! serves all **75** tools. This standalone binary is for development: without
//! `--features lsp` it serves only the **32 core** tools, since
//! the 38 LSP and 5 architecture tools are behind that compile-time feature.
//!
//! It is named `devscriptor-mcp-standalone` precisely so the two cannot be
//! confused — or overwrite each other in `target/release/`.
//!
//! # Feature Flags
//!
//! - `rmcp-sdk` (default): the MCP server, on the rmcp crate
//! - `lsp`: adds the 38 LSP + 5 architecture tools (75 total)

#[cfg(feature = "rmcp-sdk")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout carries the JSON-RPC stream; logs MUST go to stderr or they corrupt it.
    // Colour only for a terminal: stderr is normally a pipe to the client.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();

    tracing::info!(
        "Devscriptor MCP Server v{} (rmcp-sdk)",
        devscriptor_core::VERSION
    );
    tracing::info!("Starting rmcp SDK server...");

    devscriptor_mcp::run_rmcp_server().await
}
