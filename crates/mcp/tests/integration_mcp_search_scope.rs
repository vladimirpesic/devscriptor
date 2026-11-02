//! An unqualified `semantic_search` must search every entity type.
//!
//! A default `entity_type` of `function` would give a caller who asked a plain
//! question a function-only answer that looks complete: classes, modules and
//! documentation sections absent with nothing to say they had been filtered
//! out — indistinguishable, from the caller's side, from those entities not
//! existing.

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;

fn setup() -> (CoreModule, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("index.db");
    let state =
        devscriptor_mcp::DevscriptorServerState::with_db_path(db_path).expect("server state");
    (CoreModule::new(Arc::new(state)), dir)
}

fn text(result: ToolResult) -> String {
    match result {
        ToolResult::Success(v) => serde_json::to_string(&v).expect("serializes"),
        ToolResult::Error(e) => panic!("expected success, got error: {e}"),
    }
}

async fn scanned_project() -> (CoreModule, TempDir) {
    let (module, dir) = setup();
    std::fs::write(
        dir.path().join("thresholds.py"),
        "MAX_SAFE_CELSIUS_CENTI = 8500\n\n\nclass ReadingGuard:\n    \
         def is_unsafe(self, centi):\n        return centi > MAX_SAFE_CELSIUS_CENTI\n",
    )
    .expect("write");

    let scan = module
        .handle_tool(
            "scan_codebase",
            Some(json!({ "path": dir.path().to_string_lossy() })),
        )
        .await
        .expect("scan");
    let _ = text(scan);
    (module, dir)
}

/// Without an `entity_type` the search must reach kinds other than `function`.
#[tokio::test]
async fn an_unqualified_search_is_not_limited_to_functions() {
    let (module, _dir) = scanned_project().await;

    let out = text(
        module
            .handle_tool(
                "semantic_search",
                Some(json!({ "query": "MAX_SAFE_CELSIUS_CENTI", "limit": 10 })),
            )
            .await
            .expect("search"),
    );

    assert!(
        out.contains("MAX_SAFE_CELSIUS_CENTI"),
        "the constant the query names was filtered out of its own search: {out}"
    );
    assert!(
        !out.contains("\"entity_type\":\"function\"") || out.contains("constant"),
        "results should not be function-only: {out}"
    );
}

/// An explicit `entity_type` must still narrow the search — the default
/// changing must not cost callers the filter.
#[tokio::test]
async fn an_explicit_entity_type_still_filters() {
    let (module, _dir) = scanned_project().await;

    let out = text(
        module
            .handle_tool(
                "semantic_search",
                Some(json!({
                    "query": "MAX_SAFE_CELSIUS_CENTI",
                    "entity_type": "constant",
                    "limit": 10
                })),
            )
            .await
            .expect("search"),
    );

    assert!(
        !out.contains("\"entity_type\":\"function\""),
        "a constant-filtered search returned functions: {out}"
    );
}

/// Each match carries its lines, so it can be cited as `file:line` without
/// reading the file. Results named only the file, and agents counting lines
/// by hand to cite a match got them wrong.
#[tokio::test]
async fn search_results_carry_the_lines_of_each_match() {
    let (module, _dir) = scanned_project().await;

    for (tool, args) in [
        (
            "semantic_search",
            json!({ "query": "is_unsafe", "limit": 10 }),
        ),
        ("fuzzy_search", json!({ "query": "is_unsafe", "limit": 10 })),
    ] {
        let out: serde_json::Value = serde_json::from_str(&text(
            module.handle_tool(tool, Some(args)).await.expect(tool),
        ))
        .expect("json");
        let hit = out["results"]
            .as_array()
            .and_then(|results| results.iter().find(|r| r["name"] == "is_unsafe"))
            .unwrap_or_else(|| panic!("{tool} did not find is_unsafe: {out}"));
        assert_eq!(hit["start_line"], 5, "{tool}: {hit}");
        assert_eq!(hit["end_line"], 6, "{tool}: {hit}");
    }
}
