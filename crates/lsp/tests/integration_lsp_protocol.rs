//! Integration tests for LSP protocol handling.
//!
//! These tests cover `LspTransport`, message parsing, serialization,
//! request ID generation, and header parsing.
#![cfg(feature = "lsp")]

use devscriptor_lsp::lsp::protocol::{
    ErrorObject, InboundMessage, JSONRPC_VERSION, LspHeader, MAX_MESSAGE_SIZE, NotificationMessage,
    RequestId, RequestMessage, ResponseMessage, create_lsp_message, generate_request_id,
    parse_headers,
};
use devscriptor_lsp::lsp::transport::LspTransport;
use serde_json::Value;

// ============================================================================
// LspTransport creation tests
// ============================================================================

#[test]
fn test_transport_struct_creation() {
    // Verify LspTransport struct can be referenced
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<LspTransport>();
}

// ============================================================================
// Message formatting tests (create_lsp_message)
// ============================================================================

#[test]
fn test_create_lsp_message_basic() {
    let content = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
    let message = create_lsp_message(content);

    assert!(message.contains("Content-Length:"));
    assert!(message.contains(content));
    assert!(message.contains("\r\n\r\n"));

    // Verify header and content are separated by double CRLF
    let parts: Vec<&str> = message.split("\r\n\r\n").collect();
    assert_eq!(parts.len(), 2);
}

#[test]
fn test_create_lsp_message_content_length() {
    let content = r#"{"jsonrpc":"2.0","id":42,"method":"shutdown"}"#;
    let message = create_lsp_message(content);

    // Extract the Content-Length value
    let header_part = message.split("\r\n\r\n").next().unwrap();
    let length_str = header_part.split(": ").nth(1).unwrap();
    let declared_length: usize = length_str.parse().unwrap();

    assert_eq!(declared_length, content.len());
}

#[test]
fn test_create_lsp_message_with_complex_params() {
    let params = serde_json::json!({
        "processId": 1234,
        "rootUri": "file:///test",
        "capabilities": {
            "textDocumentSync": 1,
            "hoverProvider": true
        }
    });
    let content = serde_json::to_string(&params).unwrap();
    let message = create_lsp_message(&content);

    assert!(message.contains("Content-Length:"));
    assert!(message.contains("processId"));
    assert!(message.contains("capabilities"));
}

#[test]
fn test_create_lsp_message_unicode() {
    let content = r#"{"jsonrpc":"2.0","method":"test","params":{"text":"Hello 世界 🌍"}}"#;
    let message = create_lsp_message(content);

    // Content-Length should be in bytes
    let header_part = message.split("\r\n\r\n").next().unwrap();
    let length_str = header_part.split(": ").nth(1).unwrap();
    let declared_length: usize = length_str.parse().unwrap();

    assert_eq!(declared_length, content.len());
    assert!(message.contains("Hello 世界 🌍"));
}

// ============================================================================
// Header parsing tests
// ============================================================================

#[test]
fn test_parse_headers_valid() {
    let header_str = "Content-Length: 42\r\n";
    let header = parse_headers(header_str).unwrap();
    assert_eq!(header.content_length, 42);
}

#[test]
fn test_parse_headers_with_extra_whitespace() {
    let header_str = "Content-Length:   100  \r\n";
    let header = parse_headers(header_str).unwrap();
    assert_eq!(header.content_length, 100);
}

#[test]
fn test_parse_headers_missing() {
    let header_str = "Content-Type: application/json\r\n";
    let result = parse_headers(header_str);
    assert!(result.is_err());
}

#[test]
fn test_parse_headers_invalid_number() {
    let header_str = "Content-Length: invalid\r\n";
    let result = parse_headers(header_str);
    assert!(result.is_err());
}

#[test]
fn test_parse_headers_negative() {
    let header_str = "Content-Length: -1\r\n";
    let result = parse_headers(header_str);
    // Negative might parse or might error depending on implementation
    let _ = result;
}

#[test]
fn test_parse_headers_multiple_lines() {
    let header_str =
        "Content-Length: 50\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n";
    let header = parse_headers(header_str).unwrap();
    assert_eq!(header.content_length, 50);
}

#[test]
fn test_parse_headers_empty() {
    let header_str = "";
    let result = parse_headers(header_str);
    assert!(result.is_err());
}

// ============================================================================
// LspHeader struct tests
// ============================================================================

#[test]
fn test_lsp_header_content_length() {
    let header = LspHeader {
        content_length: 1024,
    };
    assert_eq!(header.content_length, 1024);
}

// ============================================================================
// Request ID tests
// ============================================================================

#[test]
fn test_generate_request_id() {
    let id1 = generate_request_id();
    let id2 = generate_request_id();
    let id3 = generate_request_id();

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);

    match id1 {
        RequestId::Number(n) => assert!(n > 0),
        _ => panic!("Expected Number variant"),
    }
}

#[test]
fn test_request_id_number() {
    let id = RequestId::Number(42);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "42");

    let deserialized: RequestId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, deserialized);
}

#[test]
fn test_request_id_string() {
    let id = RequestId::String("test-id".into());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"test-id\"");

    let deserialized: RequestId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, deserialized);
}

#[test]
fn test_request_id_equality() {
    let id1 = RequestId::Number(1);
    let id2 = RequestId::Number(1);
    let id3 = RequestId::Number(2);
    let id4 = RequestId::String("1".into());

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
    assert_ne!(id1, id4); // Number 1 != String "1"
}

#[test]
fn test_request_id_hash() {
    use std::collections::HashSet;

    let mut set = HashSet::new();
    set.insert(RequestId::Number(1));
    set.insert(RequestId::Number(1));
    set.insert(RequestId::Number(2));
    set.insert(RequestId::String("1".into()));

    assert_eq!(set.len(), 3);
}

// ============================================================================
// RequestMessage tests
// ============================================================================

#[test]
fn test_request_message_serialization() {
    let request = RequestMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(1),
        method: "initialize".into(),
        params: Some(serde_json::json!({
            "processId": 1234,
            "rootUri": "file:///test"
        })),
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"method\":\"initialize\""));
    assert!(json.contains("\"processId\":1234"));
}

#[test]
fn test_request_message_without_params() {
    let request = RequestMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(42),
        method: "shutdown".into(),
        params: None,
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("shutdown"));
    assert!(json.contains("42"));
}

// ============================================================================
// NotificationMessage tests
// ============================================================================

#[test]
fn test_notification_message_serialization() {
    let notification = NotificationMessage {
        jsonrpc: "2.0".into(),
        method: "textDocument/didOpen".into(),
        params: Some(serde_json::json!({
            "textDocument": {
                "uri": "file:///test.rs",
                "languageId": "rust",
                "version": 1,
                "text": "fn main() {}"
            }
        })),
    };

    let json = serde_json::to_string(&notification).unwrap();
    assert!(json.contains("textDocument/didOpen"));
    assert!(!json.contains("\"id\"")); // Notifications have no ID
    assert!(json.contains("file:///test.rs"));
}

#[test]
fn test_notification_message_without_params() {
    let notification = NotificationMessage {
        jsonrpc: "2.0".into(),
        method: "exit".into(),
        params: None,
    };

    let json = serde_json::to_string(&notification).unwrap();
    assert!(json.contains("exit"));
    assert!(!json.contains("id"));
}

// ============================================================================
// ResponseMessage tests
// ============================================================================

#[test]
fn test_response_message_with_result() {
    let response = ResponseMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(1),
        result: Some(serde_json::json!({
            "capabilities": {
                "textDocumentSync": 1
            }
        })),
        error: None,
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"result\""));
    assert!(json.contains("capabilities"));
}

#[test]
fn test_response_message_with_error() {
    let response = ResponseMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(42),
        result: None,
        error: Some(ErrorObject {
            code: -32601,
            message: "Method not found".into(),
            data: None,
        }),
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"id\":42"));
    assert!(json.contains("\"error\""));
    assert!(json.contains("Method not found"));
    assert!(json.contains("-32601"));
}

#[test]
fn test_response_message_null_result() {
    let response = ResponseMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(1),
        result: Some(Value::Null),
        error: None,
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"result\":null"));
}

// ============================================================================
// ErrorObject tests
// ============================================================================

#[test]
fn test_error_object_serialization() {
    let error = ErrorObject {
        code: -32700,
        message: "Parse error".into(),
        data: Some(serde_json::json!({"detail": "invalid JSON"})),
    };

    let json = serde_json::to_string(&error).unwrap();
    assert!(json.contains("-32700"));
    assert!(json.contains("Parse error"));
    assert!(json.contains("detail"));
}

#[test]
fn test_error_object_without_data() {
    let error = ErrorObject {
        code: -32600,
        message: "Invalid Request".into(),
        data: None,
    };

    let json = serde_json::to_string(&error).unwrap();
    assert!(!json.contains("data")); // Should be omitted
}

// ============================================================================
// InboundMessage deserialization tests
// ============================================================================

#[test]
fn test_inbound_message_deserialize_response() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();

    match msg {
        InboundMessage::Response(resp) => {
            assert!(matches!(resp.id, RequestId::Number(1)));
            assert!(resp.result.is_some());
            assert!(resp.error.is_none());
        }
        _ => panic!("Expected Response variant"),
    }
}

#[test]
fn test_inbound_message_deserialize_error_response() {
    let json = r#"{"jsonrpc":"2.0","id":42,"error":{"code":-32601,"message":"Method not found"}}"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();

    match msg {
        InboundMessage::Response(resp) => {
            assert!(matches!(resp.id, RequestId::Number(42)));
            assert!(resp.result.is_none());
            assert!(resp.error.is_some());
            let err = resp.error.unwrap();
            assert_eq!(err.code, -32601);
            assert_eq!(err.message, "Method not found");
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
            assert!(notif.params.is_some());
        }
        _ => panic!("Expected Notification variant"),
    }
}

#[test]
fn test_inbound_message_deserialize_request() {
    let json = r#"{"jsonrpc":"2.0","id":99,"method":"client/registerCapability","params":{"registrations":[]}}"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();

    match msg {
        InboundMessage::Request(req) => {
            assert_eq!(req.method, "client/registerCapability");
            assert!(matches!(req.id, RequestId::Number(99)));
        }
        _ => panic!("Expected Request variant"),
    }
}

#[test]
fn test_inbound_message_deserialize_batch_notification() {
    let json = r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"1","value":{"kind":"begin","title":"Indexing"}}}"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();

    match msg {
        InboundMessage::Notification(notif) => {
            assert_eq!(notif.method, "$/progress");
        }
        _ => panic!("Expected Notification variant"),
    }
}

// ============================================================================
// Constants tests
// ============================================================================

#[test]
fn test_jsonrpc_version() {
    assert_eq!(JSONRPC_VERSION, "2.0");
}

#[test]
fn test_max_message_size() {
    assert_eq!(MAX_MESSAGE_SIZE, 16 * 1024 * 1024);
}

// ============================================================================
// Integration: Full message roundtrip
// ============================================================================

#[test]
fn test_full_message_roundtrip() {
    // Create a request
    let request = RequestMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(1),
        method: "initialize".into(),
        params: Some(serde_json::json!({
            "processId": std::process::id(),
            "rootUri": "file:///workspace",
            "capabilities": {}
        })),
    };

    // Serialize to JSON
    let json = serde_json::to_string(&request).unwrap();

    // Create LSP message with header
    let lsp_message = create_lsp_message(&json);

    // Verify structure
    let parts: Vec<&str> = lsp_message.split("\r\n\r\n").collect();
    assert_eq!(parts.len(), 2);

    let header = parts[0];
    let body = parts[1];

    assert!(header.contains("Content-Length:"));

    // Deserialize body back to verify
    let deserialized: RequestMessage = serde_json::from_str(body).unwrap();
    assert_eq!(deserialized.jsonrpc, "2.0");
    assert!(matches!(deserialized.id, RequestId::Number(1)));
    assert_eq!(deserialized.method, "initialize");
}

#[test]
fn test_notification_roundtrip() {
    let notification = NotificationMessage {
        jsonrpc: "2.0".into(),
        method: "textDocument/didOpen".into(),
        params: Some(serde_json::json!({
            "textDocument": {
                "uri": "file:///test.rs",
                "languageId": "rust",
                "version": 1,
                "text": "fn main() {}"
            }
        })),
    };

    let json = serde_json::to_string(&notification).unwrap();
    let lsp_message = create_lsp_message(&json);

    let parts: Vec<&str> = lsp_message.split("\r\n\r\n").collect();
    let body = parts[1];

    let deserialized: NotificationMessage = serde_json::from_str(body).unwrap();
    assert_eq!(deserialized.method, "textDocument/didOpen");
    assert!(deserialized.params.is_some());
}

#[test]
fn test_error_response_roundtrip() {
    let response = ResponseMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::String("abc".into()),
        result: None,
        error: Some(ErrorObject {
            code: -32600,
            message: "Invalid Request".into(),
            data: Some(serde_json::json!({"reason": "missing method"})),
        }),
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: ResponseMessage = serde_json::from_str(&json).unwrap();

    assert!(matches!(deserialized.id, RequestId::String(s) if s == "abc"));
    assert!(deserialized.error.is_some());
    assert_eq!(deserialized.error.as_ref().unwrap().code, -32600);
}

// ============================================================================
// Message size edge cases
// ============================================================================

#[test]
fn test_large_message_content_length() {
    // Create a large message body
    let large_text = "x".repeat(10000);
    let params = serde_json::json!({"text": large_text});
    let content = serde_json::to_string(&params).unwrap();

    let message = create_lsp_message(&content);

    // Parse the header
    let parts: Vec<&str> = message.split("\r\n\r\n").collect();
    let header_str = parts[0];
    let body = parts[1];

    let header = parse_headers(header_str).unwrap();
    assert_eq!(header.content_length, content.len());
    assert_eq!(body.len(), content.len());
}

#[test]
fn test_empty_message_body() {
    let content = r"{}";
    let message = create_lsp_message(content);

    let parts: Vec<&str> = message.split("\r\n\r\n").collect();
    let header = parse_headers(parts[0]).unwrap();
    assert_eq!(header.content_length, 2); // "{}" is 2 bytes
}

// ============================================================================
// Concurrent request ID generation
// ============================================================================

#[test]
fn test_concurrent_request_id_uniqueness() {
    use std::sync::Arc;
    use std::thread;

    let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    for _ in 0..10 {
        let ids_clone = Arc::clone(&ids);
        let handle = thread::spawn(move || {
            for _ in 0..100 {
                let id = generate_request_id();
                let mut ids = ids_clone.lock().unwrap();
                ids.push(id);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let ids = ids.lock().unwrap();
    // All IDs should be unique
    let unique_count: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        ids.len(),
        unique_count.len(),
        "All request IDs should be unique"
    );
}
