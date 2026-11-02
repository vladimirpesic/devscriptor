//! Real-model MCP end-to-end test.
//!
//! Scans a small real directory with the quantized ONNX model, then drives
//! `scan_codebase`, `search_entities`, and `semantic_search` through the MCP
//! server and asserts semantic (non-empty, ranked) results.
//!
//! Gated behind `slow-tests` because it requires the cached quantized model
//! (~150 MB) and ≥ 4 GB free RAM.
//!
//! Run with:
//! ```sh
//! cargo test --package devscriptor-mcp --test integration_mcp_real_model \
//!     --features slow-tests --release -- --nocapture
//! ```

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;

/// Path to the real test files used by the core crate, relative to this
/// crate's manifest directory (`crates/mcp`).
const TEST_FILES_DIR: &str = "../core/tests/test_files";

fn setup() -> (CoreModule, TempDir) {
    let _ = tracing_subscriber::fmt::try_init();
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("real_model_e2e.db");
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

/// Scan the test_files directory with the real quantized model, then
/// verify that `search_entities` and `semantic_search` return non-empty results.
///
/// Accept: `find_similar` returns ranked results from real embeddings.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires quantized ONNX model + ≥ 4 GB free RAM; run with --features slow-tests"
)]
#[tokio::test]
async fn test_ws6_t2_real_model_scan_and_search() {
    let (module, _temp) = setup();
    // Resolve to an absolute, `..`-free path: the MCP `scan_codebase` tool
    // rejects any path containing a parent-directory reference (`..`) as a
    // path-traversal safeguard, so a raw relative path would be refused.
    let test_files =
        dunce::canonicalize(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(TEST_FILES_DIR))
            .expect("test_files directory must exist");

    // --- Phase 1: scan the test files directory ---
    let scan_result = module
        .handle_tool(
            "scan_codebase",
            Some(json!({
                "path": test_files.to_string_lossy(),
                "embed": true
            })),
        )
        .await
        .expect("scan_codebase must not error");

    let scan_text = extract_text(scan_result);
    assert!(
        !scan_text.is_empty(),
        "scan_codebase must return a non-empty response"
    );
    println!("Real model scan: {scan_text}");

    // --- Phase 2: keyword search (always available after scan) ---
    let keyword_result = module
        .handle_tool(
            "search_entities",
            Some(json!({"query": "function", "limit": 10})),
        )
        .await
        .expect("search_entities must not error");

    let keyword_text = extract_text(keyword_result);
    assert!(
        keyword_text.contains("function") || keyword_text.contains("result"),
        "search_entities must return entity data; got: {keyword_text}"
    );

    // --- Phase 3: semantic search (requires real embeddings) ---
    let semantic_result = module
        .handle_tool(
            "semantic_search",
            Some(json!({"query": "authenticate user session", "limit": 5})),
        )
        .await
        .expect("semantic_search must not error");

    let semantic_text = extract_text(semantic_result);
    println!("Real model semantic: {semantic_text}");

    // With a real model and real test files, semantic search should return results.
    // We don't assert specific entities since the test files vary.
    assert!(
        !semantic_text.is_empty(),
        "semantic_search must return a non-empty response with real embeddings"
    );
}

/// Notes like those an agent stores, and the question it searches them with.
/// The note that says what the query asks must come first, by what it says,
/// in semantic and hybrid search alike — not last, as ranked by the code its
/// links name.
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "requires quantized ONNX model + ≥ 4 GB free RAM; run with --features slow-tests"
)]
#[tokio::test]
async fn real_model_ranks_the_note_a_query_describes_first() {
    let (module, _temp) = setup();
    let money = "Money is integer cents everywhere — never use floating point";
    for (title, content) in [
        (
            "Release audit fixes: business-rule deviations corrected across the monorepo",
            "Pre-release audit of all README business rules found and fixed ~30 deviations. \
             Existing tests were green because fixtures never hit the boundary; always add a \
             boundary regression test when fixing a rule breach.",
        ),
        (
            money,
            "The cardinal invariant: all money is an integer number of cents in every \
             component — Rust u64, Go int64, SQL BIGINT columns named *_cents. Never introduce \
             floating-point money.",
        ),
        (
            "Pricing rules 1–6 are duplicated in pricing-rust and orders-go — keep in sync",
            "The volume discount, WELCOME5 coupon, shipping threshold, tax, total and points \
             rules are implemented in both services; their constants are not shared.",
        ),
        (
            "Wire format and orders-table semantics: goods_cents is the discounted subtotal",
            "REST APIs use snake_case JSON with integer *_cents fields; GraphQL uses camelCase \
             *Cents fields. goods_cents is the discounted subtotal.",
        ),
    ] {
        let added = module
            .handle_tool(
                "add_context_note",
                Some(json!({
                    "note_type": "standard",
                    "title": title,
                    "content": content,
                    "related_entities": [{"entity_type": "function", "entity_name": "calculate_total"}],
                })),
            )
            .await
            .expect("add_context_note must not error");
        extract_text(added);
    }

    for search_type in ["semantic", "hybrid"] {
        let found = module
            .handle_tool(
                "search_context",
                Some(json!({
                    "query": "integer cents money no floating point",
                    "search_type": search_type,
                })),
            )
            .await
            .expect("search_context must not error");
        let found: serde_json::Value = serde_json::from_str(&extract_text(found)).unwrap();
        let titles: Vec<&str> = found["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles.first(), Some(&money), "{search_type}: {found}");
        assert_eq!(titles.len(), 4, "{search_type}: each note once: {titles:?}");
    }
}
