//! JSON-RPC protocol types for LSP communication.
//!
//! This module defines the message types used for JSON-RPC communication
//! with language servers.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};

/// JSON-RPC protocol version.
pub const JSONRPC_VERSION: &str = "2.0";

/// Maximum message size (16MB).
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Request ID type for JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    /// Numeric request ID.
    Number(i64),
    /// String request ID.
    String(String),
}

/// JSON-RPC request message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMessage {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: RequestId,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC notification message (no ID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMessage {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC response message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMessage {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: RequestId,
    /// Result value (if successful).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Additional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Inbound message types from the LSP server.
#[derive(Debug, Clone)]
pub enum InboundMessage {
    /// Request from server (rare, usually client-initiated).
    Request(RequestMessage),
    /// Response to a client request.
    Response(ResponseMessage),
    /// Notification from server.
    Notification(NotificationMessage),
}

impl<'de> Deserialize<'de> for InboundMessage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct InboundMessageVisitor;

        impl<'de> Visitor<'de> for InboundMessageVisitor {
            type Value = InboundMessage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON-RPC message object")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut obj = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    obj.insert(key, value);
                }

                let has_id = obj.contains_key("id");
                // A message with an id and a method is the server's request;
                // one with an id and no method answers ours, `result` or not:
                // servers built on lsp4j (jdtls, Metals) leave a null result
                // out, though JSON-RPC asks for it.
                let is_response = !obj.contains_key("method");

                if has_id {
                    let value = Value::Object(obj);
                    if is_response {
                        let resp =
                            ResponseMessage::deserialize(value).map_err(de::Error::custom)?;
                        Ok(InboundMessage::Response(resp))
                    } else {
                        let req = RequestMessage::deserialize(value).map_err(de::Error::custom)?;
                        Ok(InboundMessage::Request(req))
                    }
                } else {
                    let notif = NotificationMessage::deserialize(Value::Object(obj))
                        .map_err(de::Error::custom)?;
                    Ok(InboundMessage::Notification(notif))
                }
            }
        }

        deserializer.deserialize_map(InboundMessageVisitor)
    }
}

/// Outbound message types to the LSP server.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OutboundMessage {
    /// Request to server.
    Request(RequestMessage),
    /// Notification to server.
    Notification(NotificationMessage),
}

/// LSP message header.
#[derive(Debug, Clone)]
pub struct LspHeader {
    /// Content length in bytes.
    pub content_length: usize,
}

/// Generate a unique request ID.
pub fn generate_request_id() -> RequestId {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    RequestId::Number(COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Create an LSP message with Content-Length header.
#[must_use]
pub fn create_lsp_message(content: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{content}", content.len())
}

/// Parse LSP headers from a string.
pub fn parse_headers(header_str: &str) -> Result<LspHeader, crate::error::LspError> {
    let mut content_length = None;

    for line in header_str.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_lowercase();
            let value = value.trim();

            if key == "content-length" {
                let parsed = value.parse::<usize>().map_err(|e| {
                    crate::error::LspError::Protocol(format!("Invalid Content-Length: {e}"))
                })?;
                content_length = Some(parsed);
            }
        }
    }

    let content_length = content_length
        .ok_or_else(|| crate::error::LspError::Protocol("Missing Content-Length header".into()))?;

    Ok(LspHeader { content_length })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();

        assert_ne!(id1, id2);

        match id1 {
            RequestId::Number(n) => assert_eq!(n, 1),
            _ => panic!("Expected Number variant"),
        }

        match id2 {
            RequestId::Number(n) => assert_eq!(n, 2),
            _ => panic!("Expected Number variant"),
        }
    }

    /// An answer whose result is null may leave `result` out, as lsp4j's
    /// servers do: it is still a response, to the request of its id.
    #[test]
    fn an_answer_without_a_result_is_a_response() {
        let message: InboundMessage = serde_json::from_str(r#"{"jsonrpc":"2.0","id":7}"#).unwrap();
        match message {
            InboundMessage::Response(response) => {
                assert_eq!(response.id, RequestId::Number(7));
                assert!(response.result.is_none() && response.error.is_none());
            }
            other => panic!("not a response: {other:?}"),
        }
        let request: InboundMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":"a","method":"client/registerCapability","params":{}}"#,
        )
        .unwrap();
        assert!(matches!(request, InboundMessage::Request(_)));
        let notification: InboundMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"$/progress","params":{}}"#).unwrap();
        assert!(matches!(notification, InboundMessage::Notification(_)));
        let failure: InboundMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no"}}"#,
        )
        .unwrap();
        assert!(matches!(failure, InboundMessage::Response(r) if r.error.is_some()));
    }

    #[test]
    fn test_create_lsp_message() {
        let content = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let message = create_lsp_message(content);

        assert!(message.contains("Content-Length:"));
        assert!(message.contains(content));
        assert!(message.contains("\r\n\r\n"));
    }

    #[test]
    fn test_parse_headers_valid() {
        let header_str = "Content-Length: 42\r\n";
        let header = parse_headers(header_str).unwrap();

        assert_eq!(header.content_length, 42);
    }

    #[test]
    fn test_parse_headers_missing() {
        let header_str = "Content-Type: application/json\r\n";
        let result = parse_headers(header_str);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_headers_invalid() {
        let header_str = "Content-Length: invalid\r\n";
        let result = parse_headers(header_str);

        assert!(result.is_err());
    }

    #[test]
    fn test_request_id_equality() {
        let id1 = RequestId::Number(1);
        let id2 = RequestId::Number(1);
        let id3 = RequestId::Number(2);
        let id4 = RequestId::String("test".into());

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_ne!(id1, id4);
    }

    #[test]
    fn test_inbound_message_deserialize_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::Response(resp) => {
                assert!(matches!(resp.id, RequestId::Number(1)));
                assert!(resp.result.is_some());
            }
            _ => panic!("Expected Response variant"),
        }
    }

    #[test]
    fn test_inbound_message_deserialize_notification() {
        let json = r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///test.rs","diagnostics":[]}}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::Notification(notif) => {
                assert_eq!(notif.method, "textDocument/publishDiagnostics");
            }
            _ => panic!("Expected Notification variant"),
        }
    }
}
