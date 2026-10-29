//! Real-time watcher tool implementations: start, stop, and status.
//!
//! These tools drive the [`CodeWatcher`](devscriptor_core::watcher::CodeWatcher)
//! embedded in the server state, keeping the knowledge graph current as source
//! files change while the MCP server runs.

use std::path::PathBuf;

use serde_json::{Value, json};
use tracing::info;

use super::{CoreModule, ToolError, ToolResult};

impl CoreModule {
    pub(super) async fn start_watching(
        &self,
        arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        let args = arguments.ok_or_else(|| ToolError::InvalidParams("Missing arguments".into()))?;

        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'path' argument".into()))?;
        let debounce_ms = args
            .get("debounce_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(400);
        let embed = args
            .get("embed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        info!(
            "Starting code watcher on {} (debounce {debounce_ms}ms)",
            path
        );

        match self
            .state
            .start_code_watching(PathBuf::from(path), debounce_ms, embed)
            .await
        {
            Ok(v) => Ok(ToolResult::Success(v)),
            Err(e) => Err(ToolError::Internal(e.to_string())),
        }
    }

    pub(super) async fn stop_watching(
        &self,
        _arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        info!("Stopping code watcher");
        self.state.stop_watching_by_itself();
        Ok(ToolResult::Success(self.state.stop_code_watching()))
    }

    pub(super) async fn get_watch_status(
        &self,
        _arguments: Option<Value>,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::Success(json!(self.state.code_watch_status())))
    }
}
