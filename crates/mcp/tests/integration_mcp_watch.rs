//! Integration tests for the MCP real-time watcher tools and Context Graph
//! entity linking, exercising [`CoreModule::handle_tool`].

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;

fn setup() -> (CoreModule, TempDir) {
    let _ = tracing_subscriber::fmt::try_init();
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("watch_test.db");
    let state = devscriptor_mcp::DevscriptorServerState::with_db_path(db_path)
        .expect("failed to create server state");
    let module = CoreModule::new(Arc::new(state));
    (module, temp_dir)
}

fn success(result: Result<ToolResult, devscriptor_mcp::ToolError>) -> serde_json::Value {
    match result.expect("tool call failed") {
        ToolResult::Success(v) => v,
        ToolResult::Error(e) => panic!("expected success, got error: {e}"),
    }
}

/// Handlers return their payload directly; this just asserts that and clones.
/// Handlers return a bare payload; MCP framing (`{"content": [{"type": "text", ...}]}`)
/// is the transport's job. Fails if an envelope creeps back into a handler.
fn assert_not_mcp_framed(v: &serde_json::Value) {
    assert!(
        v["content"][0]["type"] != "text",
        "tool payload is MCP-framed; framing belongs to the transport: {v}"
    );
}

fn inner(v: &serde_json::Value) -> serde_json::Value {
    assert_not_mcp_framed(v);
    v.clone()
}

/// The real-time watcher MCP tools manage the watcher lifecycle: start, report
/// status, prime the graph, stop, and restart cleanly.
///
/// Real-time change detection (incremental re-scan on edit/create/delete) is
/// verified end-to-end against a live binary in the CLI smoke test; this test
/// covers the MCP tool plumbing and state management, which run in-process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_tools_lifecycle() {
    let (module, temp) = setup();

    // A project directory (kept separate from the DB, which lives in temp root).
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src/main.rs"), "fn existing_fn() {}\n").unwrap();

    // Start watching — this also primes the graph with an initial scan.
    let started = success(
        module
            .handle_tool(
                "start_watching",
                Some(json!({"path": proj.to_string_lossy(), "debounce_ms": 200})),
            )
            .await,
    );
    assert_eq!(started["watching"], json!(true), "should be watching");
    assert!(
        started["path"].as_str().unwrap_or("").ends_with("proj"),
        "path should be echoed back"
    );

    // The priming scan indexed the initial file into the shared database.
    let res = success(
        module
            .handle_tool(
                "search_functions",
                Some(json!({"query": "existing_fn", "limit": 5})),
            )
            .await,
    );
    assert!(
        serde_json::to_string(&res)
            .unwrap_or_default()
            .contains("existing_fn"),
        "priming scan should have indexed the initial function"
    );

    // Status reflects a running watcher.
    let status = success(module.handle_tool("get_watch_status", None).await);
    assert_eq!(status["watching"], json!(true));
    assert_eq!(status["debounce_ms"], json!(200));

    // Stop the watcher.
    let stopped = success(module.handle_tool("stop_watching", None).await);
    assert_eq!(stopped["stopped"], json!(true));
    let status = success(module.handle_tool("get_watch_status", None).await);
    assert_eq!(status["watching"], json!(false));

    // Stopping again is a no-op, not an error.
    let stopped_again = success(module.handle_tool("stop_watching", None).await);
    assert_eq!(stopped_again["stopped"], json!(false));

    // The watcher can be restarted cleanly after stopping.
    let restarted = success(
        module
            .handle_tool(
                "start_watching",
                Some(json!({"path": proj.to_string_lossy()})),
            )
            .await,
    );
    assert_eq!(restarted["watching"], json!(true));
    let _ = module.handle_tool("stop_watching", None).await;
}

/// A file written the moment `start_watching` answers is indexed: the watch is
/// registered before the answer, not by a task spawned after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_written_as_start_watching_answers_is_indexed() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src/main.rs"), "fn existing_fn() {}\n").unwrap();

    let started = success(
        module
            .handle_tool(
                "start_watching",
                Some(json!({"path": proj.to_string_lossy(), "debounce_ms": 100})),
            )
            .await,
    );
    assert_eq!(started["watching"], json!(true));
    std::fs::write(
        proj.join("src/late.rs"),
        "fn written_as_watching_began() {}\n",
    )
    .unwrap();

    let mut indexed = false;
    for _ in 0..50 {
        let found = success(
            module
                .handle_tool(
                    "search_functions",
                    Some(json!({"query": "written_as_watching_began", "limit": 5})),
                )
                .await,
        );
        if found.to_string().contains("written_as_watching_began") {
            indexed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let _ = module.handle_tool("stop_watching", None).await;
    assert!(
        indexed,
        "the file written as watching began was not indexed in 5 s"
    );
}

/// One watcher runs per server. Asked again for the root it watches, a server
/// reports that watch; asked for another root, it refuses and names the
/// watched one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_start_reports_the_watch_or_is_rejected() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.rs"), "fn a() {}\n").unwrap();
    let other = temp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();

    let _ = success(
        module
            .handle_tool(
                "start_watching",
                Some(json!({"path": proj.to_string_lossy(), "debounce_ms": 200})),
            )
            .await,
    );
    let again = success(
        module
            .handle_tool(
                "start_watching",
                Some(json!({"path": proj.to_string_lossy()})),
            )
            .await,
    );
    assert_eq!(again["already_watching"], true, "{again}");
    assert_eq!(again["watching"], true, "{again}");

    let elsewhere = module
        .handle_tool(
            "start_watching",
            Some(json!({"path": other.to_string_lossy()})),
        )
        .await;
    let message = match elsewhere {
        Err(e) => e.to_string(),
        Ok(ToolResult::Error(e)) => e,
        Ok(ToolResult::Success(v)) => panic!("watching another root should fail: {v}"),
    };
    let watched = dunce::canonicalize(proj).unwrap();
    assert!(
        message.contains(&*watched.to_string_lossy()),
        "the refusal names the watched root: {message}"
    );

    let _ = module.handle_tool("stop_watching", None).await;
}

/// `add_context_note` persists entity links, retrievable via
/// `get_related_context`.
#[tokio::test]
async fn context_note_entity_links_round_trip() {
    let (module, _temp) = setup();

    let added = success(
        module
            .handle_tool(
                "add_context_note",
                Some(json!({
                    "note_type": "adr",
                    "title": "Auth uses JWT",
                    "content": "We chose JWT for stateless auth.",
                    "related_entities": [
                        {"entity_type": "function", "entity_name": "verify_token", "file_path": "src/auth.rs"}
                    ]
                })),
            )
            .await,
    );
    assert_eq!(
        inner(&added)["entity_links"],
        json!(1),
        "one entity link stored; got {added}"
    );

    // Retrieve notes linked to the "function" entity type.
    let related = success(
        module
            .handle_tool(
                "get_related_context",
                Some(json!({"entity_type": "function"})),
            )
            .await,
    );
    let text = serde_json::to_string(&related).unwrap_or_default();
    assert!(
        text.contains("Auth uses JWT"),
        "linked note should be retrievable by entity type: {text}"
    );
}
