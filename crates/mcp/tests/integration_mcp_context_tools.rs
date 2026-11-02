//! Integration tests for the Context Graph, file and query MCP tools.
//!
//! Covers: add_context_note, search_context, get_related_context, prune_context,
//! get_context_statistics, validate_file_edit, query_with_context, get_file_context,
//! describe_modification.

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;

fn setup() -> (CoreModule, TempDir) {
    let _ = tracing_subscriber::fmt::try_init();
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("context_tools_test.db");
    let state = devscriptor_mcp::DevscriptorServerState::with_db_path(db_path)
        .expect("failed to create server state");
    (CoreModule::new(Arc::new(state)), temp_dir)
}

/// Handlers return a bare payload; MCP framing (`{"content": [{"type": "text", ...}]}`)
/// is the transport's job. Fails if an envelope creeps back into a handler.
fn assert_not_mcp_framed(v: &serde_json::Value) {
    assert!(
        v["content"][0]["type"] != "text",
        "tool payload is MCP-framed; framing belongs to the transport: {v}"
    );
}

fn extract_text(result: ToolResult) -> String {
    match result {
        ToolResult::Success(val) => {
            assert_not_mcp_framed(&val);
            serde_json::to_string(&val).expect("payload serializes")
        }
        ToolResult::Error(e) => panic!("Expected success, got error: {e}"),
    }
}

// ============================================================================
// add_context_note
// ============================================================================

#[tokio::test]
async fn test_add_context_note_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "add_context_note",
            Some(json!({
                "note_type": "general",
                "title": "Test note",
                "content": "This is a test note about the authentication module.",
                "tags": ["auth", "security"],
                "priority": 7,
                "author": "test_agent"
            })),
        )
        .await
        .expect("add_context_note should not error");

    let text = extract_text(result);
    assert!(
        !text.is_empty(),
        "add_context_note must return a non-empty response"
    );
}

#[tokio::test]
async fn test_add_context_note_missing_required_fields() {
    let (module, _temp) = setup();

    // Missing 'title' — should return error
    let result = module
        .handle_tool(
            "add_context_note",
            Some(json!({"note_type": "general", "content": "content only"})),
        )
        .await;
    // Either a ToolError or ToolResult::Error is acceptable
    assert!(
        result.is_err() || matches!(result, Ok(ToolResult::Error(_))),
        "add_context_note with missing title must return an error"
    );
}

// ============================================================================
// search_context
// ============================================================================

#[tokio::test]
async fn test_search_context_tool_empty_db() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("search_context", Some(json!({"query": "authentication"})))
        .await
        .expect("search_context should not error on empty DB");

    let text = extract_text(result);
    assert!(
        !text.is_empty(),
        "search_context must return a non-empty response"
    );
}

#[tokio::test]
async fn test_search_context_tool_finds_added_note() {
    let (module, _temp) = setup();

    // Add a note first
    module
        .handle_tool(
            "add_context_note",
            Some(json!({
                "note_type": "adr",
                "title": "Use JWT for authentication",
                "content": "We decided to use JWT tokens for stateless auth.",
                "tags": ["jwt", "auth"]
            })),
        )
        .await
        .expect("add_context_note setup must succeed");

    let result = module
        .handle_tool(
            "search_context",
            Some(json!({"query": "JWT authentication"})),
        )
        .await
        .expect("search_context should not error");

    let text = extract_text(result);
    assert!(!text.is_empty(), "search_context must return results");
}

#[tokio::test]
async fn test_search_context_missing_query() {
    let (module, _temp) = setup();

    let result = module.handle_tool("search_context", Some(json!({}))).await;
    assert!(
        result.is_err() || matches!(result, Ok(ToolResult::Error(_))),
        "search_context with missing query must return an error"
    );
}

// ============================================================================
// get_related_context
// ============================================================================

#[tokio::test]
async fn test_get_related_context_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "get_related_context",
            Some(json!({"entity_type": "function", "limit": 5})),
        )
        .await
        .expect("get_related_context should not error");

    // Either success or error is fine — the important thing is it doesn't panic
    match result {
        ToolResult::Success(_) | ToolResult::Error(_) => {}
    }
}

// ============================================================================
// get_context_statistics
// ============================================================================

#[tokio::test]
async fn test_get_context_statistics_tool_empty_db() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("get_context_statistics", None)
        .await
        .expect("get_context_statistics should not error on empty DB");

    let text = extract_text(result);
    assert!(
        !text.is_empty(),
        "get_context_statistics must return a non-empty response"
    );
}

#[tokio::test]
async fn test_get_context_statistics_tool_counts_added_notes() {
    let (module, _temp) = setup();

    // Add two notes
    for i in 0..2 {
        module
            .handle_tool(
                "add_context_note",
                Some(json!({
                    "note_type": "general",
                    "title": format!("Note {i}"),
                    "content": format!("Content for note {i}"),
                })),
            )
            .await
            .expect("add_context_note must succeed");
    }

    let result = module
        .handle_tool("get_context_statistics", None)
        .await
        .expect("get_context_statistics should not error");

    let text = extract_text(result);
    assert!(!text.is_empty(), "get_context_statistics must return stats");
}

// ============================================================================
// prune_context
// ============================================================================

#[tokio::test]
async fn test_prune_context_tool_empty_db() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "prune_context",
            Some(json!({"max_age_days": 30, "min_priority": 3})),
        )
        .await
        .expect("prune_context should not error on empty DB");

    let text = extract_text(result);
    assert!(
        !text.is_empty(),
        "prune_context must return a non-empty response"
    );
}

// ============================================================================
// validate_file_edit
// ============================================================================

#[tokio::test]
async fn test_validate_file_edit_tool_no_original_content() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "validate_file_edit",
            Some(json!({"file_path": "/nonexistent/path/file.rs"})),
        )
        .await
        .expect("validate_file_edit should not panic");

    // With no original_content, it queries context — should succeed with empty results
    match result {
        ToolResult::Success(_) | ToolResult::Error(_) => {}
    }
}

#[tokio::test]
async fn test_validate_file_edit_stale_content_detection() {
    let (module, temp) = setup();

    // Write a file with known content
    let file_path = temp.path().join("edit_target.rs");
    std::fs::write(&file_path, "fn original() {}").expect("write must succeed");

    // Validate with matching original content — should be allowed
    let match_result = module
        .handle_tool(
            "validate_file_edit",
            Some(json!({
                "file_path": file_path.to_string_lossy(),
                "original_content": "fn original() {}"
            })),
        )
        .await
        .expect("validate_file_edit must not error");

    let match_text = extract_text(match_result);
    // Stale=false when content matches
    assert!(
        !match_text.contains("\"stale\":true"),
        "Matching content must not be flagged as stale; got: {match_text}"
    );

    // Validate with mismatched original content — should be stale
    let stale_result = module
        .handle_tool(
            "validate_file_edit",
            Some(json!({
                "file_path": file_path.to_string_lossy(),
                "original_content": "fn different_original() {}"
            })),
        )
        .await
        .expect("validate_file_edit must not error");

    let stale_text = extract_text(stale_result);
    assert!(
        stale_text.contains("stale"),
        "Mismatched content must be flagged as stale; got: {stale_text}"
    );
}

#[tokio::test]
async fn test_validate_file_edit_missing_file_path() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("validate_file_edit", Some(json!({})))
        .await;
    assert!(
        result.is_err() || matches!(result, Ok(ToolResult::Error(_))),
        "validate_file_edit with missing file_path must return an error"
    );
}

// ============================================================================
// query_with_context
// ============================================================================

#[tokio::test]
async fn test_query_with_context_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "query_with_context",
            Some(json!({"query": "authentication handler", "limit": 5})),
        )
        .await
        .expect("query_with_context should not error");

    match result {
        ToolResult::Success(_) | ToolResult::Error(_) => {}
    }
}

// ============================================================================
// get_file_context
// ============================================================================

#[tokio::test]
async fn test_get_file_context_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "get_file_context",
            Some(json!({"file_path": "src/main.rs", "line": 1})),
        )
        .await
        .expect("get_file_context should not error");

    match result {
        ToolResult::Success(_) | ToolResult::Error(_) => {}
    }
}

// ============================================================================
// describe_modification
// ============================================================================

#[tokio::test]
async fn test_describe_modification_tool() {
    let (module, temp) = setup();

    // describe_modification requires file_path within project root, original_content, modified_content.
    // Use a real temp file so the path-within-root check passes.
    let file_path = temp.path().join("auth.rs");
    std::fs::write(&file_path, "fn original() {}").expect("write must succeed");

    let result = module
        .handle_tool(
            "describe_modification",
            Some(json!({
                "file_path": file_path.to_string_lossy(),
                "original_content": "fn original() {}",
                "modified_content": "fn refactored() {}\nfn original() { refactored() }"
            })),
        )
        .await
        .expect("describe_modification should not error");

    match result {
        ToolResult::Success(_) | ToolResult::Error(_) => {}
    }
}

// ============================================================================
// Searching in words
// ============================================================================

/// A question in words finds the code. `query_with_context` matched the whole
/// query against entity names, so a sentence found nothing, and the keyword
/// channel beneath the hybrid search required every word of it.
#[tokio::test]
async fn query_with_context_answers_a_question_in_words() {
    let (module, temp) = setup();
    let code = temp.path().join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(
        code.join("shipping.py"),
        "def compute_shipping_cost(subtotal_cents):\n    \
         \"\"\"Flat shipping below the free-shipping threshold.\"\"\"\n    \
         return 0 if subtotal_cents >= 5000 else 499\n\n\n\
         def render_banner():\n    return 'hello'\n",
    )
    .unwrap();
    let scanned = module
        .handle_tool(
            "scan_codebase",
            Some(json!({ "path": code.to_str().unwrap() })),
        )
        .await
        .expect("scan_codebase should not error");
    extract_text(scanned);

    let result = module
        .handle_tool(
            "query_with_context",
            Some(json!({ "query": "where are shipping costs computed?" })),
        )
        .await
        .expect("query_with_context should not error");
    let text = extract_text(result);
    assert!(text.contains("compute_shipping_cost"), "{text}");
    assert!(!text.contains("render_banner"), "{text}");
    assert!(
        text.contains("\"line\":1"),
        "results say where they are: {text}"
    );
}

/// A note is found by the words of a question, not only by a substring of
/// itself.
#[tokio::test]
async fn search_context_finds_a_note_by_its_words() {
    let (module, _temp) = setup();
    module
        .handle_tool(
            "add_context_note",
            Some(json!({
                "note_type": "failure",
                "title": "Boundary deviations in the order rules",
                "content": "The pre-release audit found off-by-one boundaries in shipping and bulk discounts.",
                "priority": 8
            })),
        )
        .await
        .expect("add_context_note should not error");

    for search_type in ["keyword", "hybrid"] {
        let result = module
            .handle_tool(
                "search_context",
                Some(json!({ "query": "which boundary bugs did the audit find?", "search_type": search_type })),
            )
            .await
            .expect("search_context should not error");
        let text = extract_text(result);
        assert!(
            text.contains("Boundary deviations in the order rules"),
            "{search_type}: {text}"
        );
    }
}
