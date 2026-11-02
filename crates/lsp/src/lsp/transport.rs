//! LSP transport layer for stdio communication.
//!
//! This module handles the low-level communication with language servers
//! over stdin/stdout using the LSP message protocol.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::error::{LspError, Result};
use crate::lsp::protocol::{
    InboundMessage, LspHeader, MAX_MESSAGE_SIZE, create_lsp_message, parse_headers,
};
use serde_json::Value;

/// Transport layer for LSP communication.
pub struct LspTransport {
    /// Standard input stream for sending messages.
    stdin: ChildStdin,
    /// Buffered standard output stream for receiving messages.
    stdout: BufReader<ChildStdout>,
    /// Reusable buffer for reading content.
    read_buffer: Vec<u8>,
}

/// Read-only half of a split [`LspTransport`].
///
/// Owns the `BufReader<ChildStdout>` and the reusable read buffer.
/// Running this in its own `tokio::spawn` task makes `receive()` unconditionally
/// safe to cancel — the underlying I/O loop never drops a partially-read header
/// or content body.
pub struct LspReader {
    stdout: BufReader<ChildStdout>,
    read_buffer: Vec<u8>,
}

impl LspReader {
    /// Receive a JSON-RPC message from the LSP server's stdout.
    ///
    /// Must be driven from a dedicated task so it is never dropped mid-read.
    pub async fn receive(&mut self) -> crate::error::Result<InboundMessage> {
        let headers = self.read_headers().await?;
        let content_length = headers.content_length;

        if content_length > MAX_MESSAGE_SIZE {
            return Err(crate::error::LspError::Protocol(format!(
                "Message too large: {content_length} bytes (max {MAX_MESSAGE_SIZE})"
            )));
        }

        let content = self.read_content(content_length).await?;
        serde_json::from_str(&content).map_err(|e| {
            crate::error::LspError::Json(format!(
                "Failed to parse LSP message ({} bytes) at position {}: {e}",
                content.len(),
                e.column()
            ))
        })
    }

    async fn read_headers(&mut self) -> crate::error::Result<LspHeader> {
        let mut header_buffer = Vec::new();
        loop {
            let mut line = Vec::new();
            let bytes_read = self
                .stdout
                .read_until(b'\n', &mut line)
                .await
                .map_err(|e| {
                    crate::error::LspError::Transport(format!("Failed to read header: {e}"))
                })?;
            if bytes_read == 0 {
                // See the sibling reader: EOF between messages is a normal exit.
                if header_buffer.is_empty() {
                    return Err(crate::error::LspError::ServerClosed);
                }
                return Err(crate::error::LspError::Transport(
                    "Connection closed midway through a message header".into(),
                ));
            }
            if line == b"\r\n" || line == b"\n" {
                break;
            }
            header_buffer.extend_from_slice(&line);
        }
        let header_str = String::from_utf8_lossy(&header_buffer);
        parse_headers(&header_str)
    }

    async fn read_content(&mut self, length: usize) -> crate::error::Result<String> {
        self.read_buffer.resize(length, 0);
        let mut total_read = 0;
        while total_read < length {
            let n = self
                .stdout
                .read(&mut self.read_buffer[total_read..])
                .await
                .map_err(|e| {
                    crate::error::LspError::Transport(format!("Failed to read content: {e}"))
                })?;
            if n == 0 {
                return Err(crate::error::LspError::Transport(format!(
                    "Connection closed after {total_read} of {length} bytes"
                )));
            }
            total_read += n;
        }
        Ok(std::str::from_utf8(&self.read_buffer[..length])
            .map_err(|e| {
                crate::error::LspError::Protocol(format!("Invalid UTF-8 in content: {e}"))
            })?
            .to_string())
    }
}

/// Write-only half of a split [`LspTransport`].
///
/// Owns `ChildStdin`. Safe to share with the command-dispatch task.
pub struct LspWriter {
    stdin: ChildStdin,
}

impl LspWriter {
    /// Send a JSON-RPC message to the LSP server's stdin.
    pub async fn send(&mut self, message: &Value) -> crate::error::Result<()> {
        let content = serde_json::to_string(message)?;
        let lsp_message = create_lsp_message(&content);
        self.stdin
            .write_all(lsp_message.as_bytes())
            .await
            .map_err(|e| crate::error::LspError::Transport(format!("Failed to write: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| crate::error::LspError::Transport(format!("Failed to flush: {e}")))?;
        Ok(())
    }
}

impl LspTransport {
    /// Create a new LSP transport from process streams.
    #[must_use]
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            stdout: BufReader::new(stdout),
            read_buffer: Vec::with_capacity(8192),
        }
    }

    /// Split the transport into independent read and write halves.
    ///
    /// The [`LspReader`] should be driven by its own `tokio::spawn` task so
    /// `receive()` is never dropped mid-message, making the overall
    /// client cancellation-safe.
    #[must_use]
    pub fn split(self) -> (LspReader, LspWriter) {
        (
            LspReader {
                stdout: self.stdout,
                read_buffer: self.read_buffer,
            },
            LspWriter { stdin: self.stdin },
        )
    }

    /// Send a JSON-RPC message to the LSP server.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to stdin fails.
    pub async fn send(&mut self, message: &Value) -> Result<()> {
        let content = serde_json::to_string(message)?;
        let lsp_message = create_lsp_message(&content);

        self.stdin
            .write_all(lsp_message.as_bytes())
            .await
            .map_err(|e| LspError::Transport(format!("Failed to write: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| LspError::Transport(format!("Failed to flush: {e}")))?;

        Ok(())
    }

    /// Receive a JSON-RPC message from the LSP server.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Reading from stdout fails
    /// - The message is too large
    /// - The message cannot be parsed as JSON
    pub async fn receive(&mut self) -> Result<InboundMessage> {
        let headers = self.read_headers().await?;
        let content_length = headers.content_length;

        if content_length > MAX_MESSAGE_SIZE {
            return Err(LspError::Protocol(format!(
                "Message too large: {content_length} bytes (max {MAX_MESSAGE_SIZE})"
            )));
        }

        let content = self.read_content(content_length).await?;
        let message: InboundMessage = serde_json::from_str(&content).map_err(|e| {
            // SECURITY: Do not include raw message content in error to avoid
            // leaking sensitive source code in logs.
            LspError::Json(format!(
                "Failed to parse LSP message ({} bytes) at position {}: {e}",
                content.len(),
                e.column()
            ))
        })?;

        Ok(message)
    }

    /// Read headers until double CRLF.
    async fn read_headers(&mut self) -> Result<LspHeader> {
        let mut header_buffer = Vec::new();

        loop {
            let mut line = Vec::new();
            let bytes_read = self
                .stdout
                .read_until(b'\n', &mut line)
                .await
                .map_err(|e| LspError::Transport(format!("Failed to read header: {e}")))?;

            if bytes_read == 0 {
                // EOF before any header byte is the server exiting between
                // messages — its normal end, not a failure. EOF *after* a
                // partial header is a real truncation and stays a transport
                // error.
                if header_buffer.is_empty() {
                    return Err(LspError::ServerClosed);
                }
                return Err(LspError::Transport(
                    "Connection closed midway through a message header".into(),
                ));
            }

            // Check for end of headers (empty line)
            if line == b"\r\n" || line == b"\n" {
                break;
            }

            header_buffer.extend_from_slice(&line);
        }

        let header_str = String::from_utf8_lossy(&header_buffer);
        parse_headers(&header_str)
    }

    /// Read exactly `content_length` bytes.
    async fn read_content(&mut self, length: usize) -> Result<String> {
        self.read_buffer.resize(length, 0);

        let mut total_read = 0;
        while total_read < length {
            let n = self
                .stdout
                .read(&mut self.read_buffer[total_read..])
                .await
                .map_err(|e| LspError::Transport(format!("Failed to read content: {e}")))?;

            if n == 0 {
                return Err(LspError::Transport(format!(
                    "Connection closed after {total_read} of {length} bytes"
                )));
            }

            total_read += n;
        }

        Ok(std::str::from_utf8(&self.read_buffer[..length])
            .map_err(|e| LspError::Protocol(format!("Invalid UTF-8 in content: {e}")))?
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::create_lsp_message;

    #[test]
    fn test_transport_struct() {
        // This test just verifies the struct can be created with the right types.
        // Actual testing would require a spawned process.
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<LspTransport>();
    }

    #[test]
    fn test_message_formatting() {
        // Test that messages are formatted correctly per LSP spec
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&message).unwrap());

        // Verify the header contains Content-Length
        assert!(lsp_message.contains("Content-Length:"));
        assert!(lsp_message.contains("\r\n\r\n"));

        // Verify JSON follows header
        let parts: Vec<&str> = lsp_message.split("\r\n\r\n").collect();
        assert_eq!(parts.len(), 2);

        let json_part: serde_json::Value = serde_json::from_str(parts[1]).unwrap();
        assert_eq!(json_part["jsonrpc"], "2.0");
        assert_eq!(json_part["id"], 1);
        assert_eq!(json_part["method"], "initialize");
    }

    #[test]
    fn test_message_with_params() {
        // Test message formatting with complex params
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {
                    "uri": "file:///test.rs"
                },
                "position": {
                    "line": 10,
                    "character": 5
                }
            }
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&message).unwrap());

        // Verify Content-Length is correct
        let content = serde_json::to_string(&message).unwrap();
        let expected_header = format!("Content-Length: {}", content.len());
        assert!(lsp_message.contains(&expected_header));

        // Verify full message structure
        assert!(lsp_message.ends_with(&content));
    }

    #[test]
    fn test_notification_message_format() {
        // Test notification formatting (no id field)
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///test.rs",
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn main() {}"
                }
            }
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&notification).unwrap());

        // Verify no id in message
        assert!(!lsp_message.contains("\"id\""));
        assert!(lsp_message.contains("textDocument/didOpen"));
    }

    #[test]
    fn test_response_message_format() {
        // Test response formatting
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 123,
            "result": {
                "capabilities": {
                    "textDocumentSync": 1,
                    "hoverProvider": true
                }
            }
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&response).unwrap());

        assert!(lsp_message.contains("\"id\":123"));
        assert!(lsp_message.contains("\"result\""));
        assert!(lsp_message.contains("capabilities"));
    }

    #[test]
    fn test_error_response_format() {
        // Test error response formatting
        let error_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 456,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&error_response).unwrap());

        assert!(lsp_message.contains("\"id\":456"));
        assert!(lsp_message.contains("\"error\""));
        assert!(lsp_message.contains("Method not found"));
    }

    #[test]
    fn test_unicode_content() {
        // Test that unicode content is handled correctly
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "test",
            "params": {
                "text": "Hello 世界 🌍"
            }
        });

        let lsp_message = create_lsp_message(&serde_json::to_string(&message).unwrap());

        // The Content-Length should be in bytes, not characters
        let content = serde_json::to_string(&message).unwrap();
        let expected_header = format!("Content-Length: {}", content.len());
        assert!(lsp_message.contains(&expected_header));
        assert!(lsp_message.contains("Hello 世界 🌍"));
    }
}
