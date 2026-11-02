//! Integration tests for MCP core analysis tools.
//!
//! Exercises [`CoreModule::handle_tool`](devscriptor_mcp::CoreModule::handle_tool)
//! for analysis and search-oriented tools using real test files.

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;
use tracing::{debug, info};

#[allow(dead_code)]
const TEST_FILES_DIR: &str = "../../core/tests/test_files";

fn setup() -> (CoreModule, TempDir) {
    let _ = tracing_subscriber::fmt::try_init();
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("core_analysis_test.db");
    debug!("Setting up CoreModule with DB at {}", db_path.display());

    let state = devscriptor_mcp::DevscriptorServerState::with_db_path(db_path)
        .expect("failed to create server state");
    let module = CoreModule::new(Arc::new(state));
    (module, temp_dir)
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
// analyze_code
// ============================================================================

#[tokio::test]
async fn test_analyze_code_tool_dead_code() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "analyze_code",
            Some(json!({"analysis_type": "dead_code", "path": "."})),
        )
        .await
        .expect("analyze_code dead_code should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(
        parsed["analysis_type"].as_str().unwrap_or(""),
        "dead_code",
        "analysis_type should match"
    );
    assert!(
        parsed["total_findings"].is_number(),
        "response should contain total_findings"
    );
    info!("test_analyze_code_tool_dead_code passed");
}

#[tokio::test]
async fn test_analyze_code_tool_complexity() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "analyze_code",
            Some(json!({"analysis_type": "complexity", "path": "."})),
        )
        .await
        .expect("analyze_code complexity should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(parsed["analysis_type"].as_str().unwrap_or(""), "complexity");
    info!("test_analyze_code_tool_complexity passed");
}

#[tokio::test]
async fn test_analyze_code_tool_clones() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "analyze_code",
            Some(json!({"analysis_type": "clones", "path": "."})),
        )
        .await
        .expect("analyze_code clones should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(parsed["analysis_type"].as_str().unwrap_or(""), "clones");
    info!("test_analyze_code_tool_clones passed");
}

#[tokio::test]
async fn test_analyze_code_tool_cross_reference() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "analyze_code",
            Some(json!({"analysis_type": "cross_reference", "path": "."})),
        )
        .await
        .expect("analyze_code cross_reference should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(
        parsed["analysis_type"].as_str().unwrap_or(""),
        "cross_reference"
    );
    info!("test_analyze_code_tool_cross_reference passed");
}

#[tokio::test]
async fn test_analyze_code_invalid_type_returns_error() {
    let (module, _temp) = setup();

    let err = module
        .handle_tool(
            "analyze_code",
            Some(json!({"analysis_type": "invalid_type_xyz", "path": "."})),
        )
        .await
        .expect_err("invalid analysis_type should error");

    let msg = format!("{err}");
    assert!(
        msg.contains("Invalid") || msg.contains("analysis_type"),
        "error should mention invalid type: got {msg}"
    );
    info!("test_analyze_code_invalid_type_returns_error passed");
}

// ============================================================================
// find_dead_code
// ============================================================================

#[tokio::test]
async fn test_find_dead_code_tool_execution() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("find_dead_code", Some(json!({"limit": 10})))
        .await
        .expect("find_dead_code should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(
        parsed["unused_functions"].is_array(),
        "response should contain unused_functions array"
    );
    assert!(
        parsed["unused_classes"].is_array(),
        "response should contain unused_classes array"
    );
    assert!(
        parsed["total_unused_functions"].is_number(),
        "response should contain total_unused_functions"
    );
    info!("test_find_dead_code_tool_execution passed");
}

// ============================================================================
// analyze_complexity
// ============================================================================

#[tokio::test]
async fn test_analyze_complexity_tool_execution() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("analyze_complexity", Some(json!({"top_n": 5})))
        .await
        .expect("analyze_complexity should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(
        parsed["total_functions_analyzed"].is_number(),
        "response should contain total_functions_analyzed"
    );
    assert!(
        parsed["average_complexity"].is_number(),
        "response should contain average_complexity"
    );
    assert!(
        parsed["most_complex_functions"].is_array(),
        "response should contain most_complex_functions array"
    );
    info!("test_analyze_complexity_tool_execution passed");
}

// ============================================================================
// search_functions
// ============================================================================

#[tokio::test]
async fn test_search_functions_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "search_functions",
            Some(json!({"query": "main", "limit": 10, "offset": 0})),
        )
        .await
        .expect("search_functions should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(parsed["total"].is_number(), "response should contain total");
    assert!(
        parsed["returned"].is_number(),
        "response should contain returned"
    );
    assert!(
        parsed["functions"].is_array(),
        "response should contain functions array"
    );
    info!("test_search_functions_tool passed");
}

#[tokio::test]
async fn test_search_functions_empty_query() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("search_functions", Some(json!({"query": "", "limit": 5})))
        .await
        .expect("search_functions with empty query should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(parsed["functions"].is_array());
    info!("test_search_functions_empty_query passed");
}

// ============================================================================
// search_classes
// ============================================================================

#[tokio::test]
async fn test_search_classes_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "search_classes",
            Some(json!({"query": "Test", "limit": 10, "offset": 0})),
        )
        .await
        .expect("search_classes should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(parsed["total"].is_number(), "response should contain total");
    assert!(
        parsed["classes"].is_array(),
        "response should contain classes array"
    );
    info!("test_search_classes_tool passed");
}

// ============================================================================
// fuzzy_search
// ============================================================================

#[tokio::test]
async fn test_fuzzy_search_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool("fuzzy_search", Some(json!({"query": "test", "limit": 5})))
        .await
        .expect("fuzzy_search should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(
        parsed["query"].as_str().unwrap_or(""),
        "test",
        "query should be echoed"
    );
    assert!(
        parsed["results"].is_array(),
        "response should contain results array"
    );
    assert!(parsed["total"].is_number(), "response should contain total");
    info!("test_fuzzy_search_tool passed");
}

#[tokio::test]
async fn test_fuzzy_search_missing_query_errors() {
    let (module, _temp) = setup();

    let err = module
        .handle_tool("fuzzy_search", Some(json!({"limit": 5})))
        .await
        .expect_err("fuzzy_search without query should error");

    let msg = format!("{err}");
    assert!(
        msg.contains("query") || msg.contains("Missing"),
        "error should mention missing query: got {msg}"
    );
    info!("test_fuzzy_search_missing_query_errors passed");
}

// ============================================================================
// get_entity_details
// ============================================================================

#[tokio::test]
async fn test_get_entity_details_tool() {
    let (module, _temp) = setup();

    // On an empty database this will likely return a "not found" response,
    // but it should still execute without panicking.
    let result = module
        .handle_tool(
            "get_entity_details",
            Some(json!({"entity_type": "function", "name": "main"})),
        )
        .await
        .expect("get_entity_details should not error");

    let text = extract_text(result);
    assert!(
        text.contains("entity_type") || text.contains("error"),
        "response should contain entity_type or error message"
    );
    info!("test_get_entity_details_tool passed");
}

#[tokio::test]
async fn test_get_entity_details_missing_params_errors() {
    let (module, _temp) = setup();

    let err = module
        .handle_tool("get_entity_details", Some(json!({"name": "main"})))
        .await
        .expect_err("missing entity_type should error");

    let msg = format!("{err}");
    assert!(
        msg.contains("entity_type") || msg.contains("Missing"),
        "error should mention missing entity_type: got {msg}"
    );
    info!("test_get_entity_details_missing_params_errors passed");
}

// ============================================================================
// get_relationships
// ============================================================================

#[tokio::test]
async fn test_get_relationships_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "get_relationships",
            Some(json!({"entity_name": "main", "entity_type": "function"})),
        )
        .await
        .expect("get_relationships should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert!(
        parsed["entity"].is_string(),
        "response should contain entity"
    );
    assert!(
        parsed["total_relationships"].is_number(),
        "response should contain total_relationships"
    );
    assert!(
        parsed["relationships"].is_array(),
        "response should contain relationships array"
    );
    info!("test_get_relationships_tool passed");
}

#[tokio::test]
async fn test_get_relationships_missing_entity_name_errors() {
    let (module, _temp) = setup();

    let err = module
        .handle_tool(
            "get_relationships",
            Some(json!({"entity_type": "function"})),
        )
        .await
        .expect_err("missing entity_name should error");

    let msg = format!("{err}");
    assert!(
        msg.contains("entity_name") || msg.contains("Missing"),
        "error should mention missing entity_name: got {msg}"
    );
    info!("test_get_relationships_missing_entity_name_errors passed");
}

/// A call to overloads is linked to each of them, and each of those edges says
/// it is one of several possible targets; an edge with one possible target
/// says nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_relationships_marks_ambiguous_edges() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("Calc.java"),
        "public class Calc {\npublic int add(int a) {\nreturn a;\n}\n\
         public long add(long a) {\nreturn a;\n}\n}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("Main.java"),
        "public class Main {\npublic void run(Calc calc) {\ncalc.add(1);\n}\n}\n",
    )
    .unwrap();
    module
        .handle_tool(
            "scan_codebase",
            Some(json!({"path": proj.to_string_lossy()})),
        )
        .await
        .expect("scan_codebase should not error");

    let edges = |kind: &'static str| {
        let module = &module;
        async move {
            let result = module
                .handle_tool(
                    "get_relationships",
                    Some(json!({"entity_name": "run", "relationship_type": kind})),
                )
                .await
                .expect("get_relationships should not error");
            let parsed: serde_json::Value =
                serde_json::from_str(&extract_text(result)).expect("JSON payload");
            parsed["relationships"]
                .as_array()
                .expect("a relationships array")
                .clone()
        }
    };

    let calls = edges("calls").await;
    assert_eq!(calls.len(), 2, "one edge per overload: {calls:?}");
    for edge in &calls {
        assert_eq!(edge["target"], "add", "{edge}");
        assert_eq!(edge["ambiguous"], true, "{edge}");
    }

    // `run` uses `Calc`, its parameter's type, which has one definition.
    let uses = edges("uses").await;
    assert!(uses.iter().any(|edge| edge["target"] == "Calc"), "{uses:?}");
    assert!(
        uses.iter().all(|edge| edge.get("ambiguous").is_none()),
        "{uses:?}"
    );
}

// ============================================================================
// semantic_search
// ============================================================================

#[tokio::test]
async fn test_semantic_search_tool() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "semantic_search",
            Some(json!({
                "query": "function that handles errors",
                "entity_type": "function",
                "limit": 5
            })),
        )
        .await
        .expect("semantic_search should not error");

    let text = extract_text(result);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    assert_eq!(
        parsed["query"].as_str().unwrap_or(""),
        "function that handles errors",
        "query should be echoed"
    );
    assert!(
        parsed["results"].is_array(),
        "response should contain results array"
    );
    info!("test_semantic_search_tool passed");
}

/// On an index with no vectors yet only keywords can match. An empty answer
/// then says so instead of looking like "nothing relevant exists".
#[tokio::test]
async fn an_empty_search_before_any_embedding_says_how_to_enable_it() {
    let (module, _temp) = setup();

    let result = module
        .handle_tool(
            "semantic_search",
            Some(json!({"query": "zzzz no keyword can match this", "limit": 5})),
        )
        .await
        .expect("semantic_search should not error");

    let parsed: serde_json::Value =
        serde_json::from_str(&extract_text(result)).unwrap_or_else(|_| json!({}));
    assert_eq!(parsed["results"].as_array().map(Vec::len), Some(0));
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(message.contains("generate_embeddings"), "{parsed}");
}

#[tokio::test]
async fn test_semantic_search_missing_query_errors() {
    let (module, _temp) = setup();

    let err = module
        .handle_tool(
            "semantic_search",
            Some(json!({"entity_type": "function", "limit": 5})),
        )
        .await
        .expect_err("semantic_search without query should error");

    let msg = format!("{err}");
    assert!(
        msg.contains("query") || msg.contains("Missing"),
        "error should mention missing query: got {msg}"
    );
    info!("test_semantic_search_missing_query_errors passed");
}
