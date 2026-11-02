//! `get_file_context` takes a file path relative to the project, as agents
//! pass it, as well as an absolute one.
//!
//! The index stores absolute paths, so a relative path is resolved against the
//! project before it is looked up: as given, `services/rating-rust/src/discount.rs`
//! would match nothing and the answer would be "File contains 0 entities", with
//! no sign that the path was the problem.
//!
//! One test in its own binary: it changes the process-wide working directory.

use devscriptor_mcp::{CoreModule, DevscriptorServerState, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;

async fn call(module: &CoreModule, tool: &str, args: Value) -> Value {
    match module.handle_tool(tool, Some(args)).await {
        Ok(ToolResult::Success(value)) => value,
        other => panic!("{tool} failed: {other:?}"),
    }
}

fn entity_names(context: &Value) -> Vec<String> {
    context["entity_contexts"]
        .as_array()
        .map(|entities| {
            entities
                .iter()
                .filter_map(|e| e["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_relative_path_finds_the_files_entities() {
    let db_dir = tempfile::tempdir().expect("db dir");
    let project = tempfile::tempdir().expect("project dir");
    let src = project.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("discount.rs"),
        "pub fn volume_discount_cents(base: u64) -> u64 {\n    base * 5 / 100\n}\n",
    )
    .unwrap();

    let state = Arc::new(
        DevscriptorServerState::with_db_path(db_dir.path().join("graph.db")).expect("state"),
    );
    let module = CoreModule::new(state);
    std::env::set_current_dir(project.path()).expect("cd into project");
    call(
        &module,
        "scan_codebase",
        json!({ "path": project.path().to_string_lossy() }),
    )
    .await;

    let relative = call(
        &module,
        "get_file_context",
        json!({ "file_path": "src/discount.rs" }),
    )
    .await;
    assert_eq!(
        entity_names(&relative),
        ["volume_discount_cents"],
        "{relative}"
    );

    let absolute = src.join("discount.rs");
    let absolute = call(
        &module,
        "get_file_context",
        json!({ "file_path": absolute.to_string_lossy() }),
    )
    .await;
    assert_eq!(entity_names(&absolute), ["volume_discount_cents"]);

    // Outside the project stays closed, as for `describe_file`.
    assert!(
        module
            .handle_tool(
                "get_file_context",
                Some(json!({ "file_path": "../outside.rs" }))
            )
            .await
            .is_err()
    );
}
