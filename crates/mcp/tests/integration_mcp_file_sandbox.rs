//! The file-reading tools must be usable inside the project and closed outside it.
//!
//! `describe_file`, `describe_modification` and `validate_file_edit` confine reads
//! so a prompt-injected agent cannot exfiltrate host files. The project is the
//! working directory or the database's directory: `devscriptor mcp` defaults to a
//! shared `~/.local/share/devscriptor/default.db`, whose directory is no project,
//! and with it alone as the root every project file would be refused:
//!
//! ```text
//! Invalid parameters: Path '…/src/lib.rs' is outside of base '~/.local/share/devscriptor'
//! ```
//!
//! This pins both halves: the project is readable, and everything above it still is
//! not — with the database deliberately placed *outside* the project.
//!
//! All assertions live in one test because they share the process-wide working
//! directory, which parallel tests would race on.

use devscriptor_mcp::{CoreModule, DevscriptorServerState, ToolError, ToolResult};
use serde_json::json;
use std::sync::Arc;

fn is_error(result: Result<ToolResult, ToolError>) -> bool {
    !matches!(result, Ok(ToolResult::Success(_)))
}

async fn describe(module: &CoreModule, path: &str) -> bool {
    is_error(
        module
            .handle_tool("describe_file", Some(json!({ "file_path": path })))
            .await,
    )
}

#[tokio::test]
async fn file_tools_read_the_project_but_nothing_above_it() {
    let db_dir = tempfile::tempdir().expect("db dir");
    let project = tempfile::tempdir().expect("project dir");
    std::fs::write(project.path().join("lib.rs"), "pub fn hello() {}\n").unwrap();

    // The database lives outside the project — the default for `devscriptor mcp`.
    let state = Arc::new(
        DevscriptorServerState::with_db_path(db_dir.path().join("graph.db"))
            .expect("state with an out-of-project database"),
    );
    let module = CoreModule::new(state);

    // Tools resolve paths against the working directory, as under an agent.
    std::env::set_current_dir(project.path()).expect("cd into project");

    // --- allowed: inside the project ---
    assert!(
        !describe(&module, "lib.rs").await,
        "a project file must be readable when the database lives outside the project"
    );

    let absolute = project.path().join("lib.rs");
    assert!(
        !describe(&module, &absolute.to_string_lossy()).await,
        "an absolute in-project path must be allowed"
    );

    // --- denied: outside the project ---
    for path in [
        "/etc/passwd",
        "../../../../../../etc/shadow",
        "../outside.rs",
    ] {
        assert!(
            describe(&module, path).await,
            "`{path}` must be rejected: it is outside the project"
        );
    }
}
