//! A server in a database that holds several projects: the whole-index
//! analyses (`path` `.`) take the workspace's project, each project is
//! re-indexed in its own configuration, and answers say when the workspace is
//! not indexed and which Context Graph notes a search's query matches.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use devscriptor_mcp::project::ProjectSettings;
use devscriptor_mcp::{CoreModule, DevscriptorServerState, ToolResult};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Projects side by side in one directory, indexed in one database.
struct Projects {
    dir: TempDir,
}

impl Projects {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    fn database(&self) -> PathBuf {
        self.dir.path().join("index.db")
    }

    /// The project `name`, with `files` written in it, symlinks resolved.
    fn project(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = self.dir.path().join(name);
        std::fs::create_dir_all(&root).expect("mkdir");
        for (path, content) in files {
            write(&root.join(path), content);
        }
        dunce::canonicalize(root).expect("canonical root")
    }

    /// A server whose workspace is `workspace`, over the shared database.
    fn server(&self, workspace: &Path) -> CoreModule {
        let state = DevscriptorServerState::for_project(
            self.database(),
            ProjectSettings::for_workspace(workspace, None),
        )
        .expect("server state")
        .with_freshness_budget(Duration::from_secs(60));
        CoreModule::new(Arc::new(state))
    }
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

async fn call(module: &CoreModule, tool: &str, arguments: Value) -> Value {
    match module.handle_tool(tool, Some(arguments)).await {
        Ok(ToolResult::Success(answer)) => answer,
        other => panic!("{tool} failed: {other:?}"),
    }
}

/// The names of the functions `find_dead_code` reports, sorted.
fn dead(answer: &Value) -> Vec<String> {
    let mut names: Vec<String> = answer["unused_functions"]
        .as_array()
        .unwrap_or_else(|| panic!("{answer}"))
        .iter()
        .map(|finding| finding["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

/// `.` is the workspace's project: its dead code, its counts. Another
/// project in the database is analyzed only when named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_whole_index_is_the_workspaces_project() {
    let projects = Projects::new();
    let ours = projects.project("ours", &[("a.py", "def ours_unused():\n    return 1\n")]);
    let theirs = projects.project(
        "theirs",
        &[
            ("b.py", "def theirs_unused():\n    return 2\n"),
            ("c.py", "def theirs_too():\n    return 3\n"),
        ],
    );
    let server = projects.server(&ours);
    call(&server, "scan_codebase", json!({"path": ours})).await;
    call(&server, "scan_codebase", json!({"path": theirs})).await;

    let everything = json!({"min_confidence": 0.0});
    let answer = call(&server, "find_dead_code", everything.clone()).await;
    assert_eq!(dead(&answer), ["ours_unused"], "{answer}");
    let named = call(
        &server,
        "find_dead_code",
        json!({"path": theirs, "min_confidence": 0.0}),
    )
    .await;
    assert_eq!(dead(&named), ["theirs_too", "theirs_unused"], "{named}");

    // A workspace inside a scanned root analyzes that root.
    let inner = ours.join("sub");
    std::fs::create_dir_all(&inner).expect("mkdir");
    let from_inside = call(&projects.server(&inner), "find_dead_code", everything).await;
    assert_eq!(dead(&from_inside), ["ours_unused"], "{from_inside}");

    let counts = call(&server, "get_statistics", json!({})).await;
    assert_eq!(counts["total_files"], json!(1), "{counts}");
    assert_eq!(
        counts["path"],
        json!(ours.join("").to_string_lossy()),
        "{counts}"
    );
}

/// Another project's files are re-indexed in its own configuration, its
/// `devscriptor.yaml`, not the workspace's: a file that only the
/// workspace's file excludes is indexed when an analysis brings that
/// project up to date.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_project_is_reindexed_in_its_own_configuration() {
    let projects = Projects::new();
    let ours = projects.project(
        "ours",
        &[
            ("devscriptor.yaml", "scan:\n  exclude: [generated]\n"),
            ("a.py", "def ours_unused():\n    return 1\n"),
        ],
    );
    let theirs = projects.project(
        "theirs",
        &[("b.py", "def theirs_unused():\n    return 2\n")],
    );
    call(
        &projects.server(&theirs),
        "scan_codebase",
        json!({"path": theirs}),
    )
    .await;
    let server = projects.server(&ours);
    call(&server, "scan_codebase", json!({"path": ours})).await;

    write(
        &theirs.join("generated/made.py"),
        "def made_unused():\n    return 3\n",
    );
    let answer = call(
        &server,
        "find_dead_code",
        json!({"path": theirs, "min_confidence": 0.0}),
    )
    .await;
    assert_eq!(dead(&answer), ["made_unused", "theirs_unused"], "{answer}");

    // The workspace's own files follow its file.
    write(
        &ours.join("generated/made.py"),
        "def ours_made_unused():\n    return 4\n",
    );
    let ours_answer = call(&server, "find_dead_code", json!({"min_confidence": 0.0})).await;
    assert_eq!(dead(&ours_answer), ["ours_unused"], "{ours_answer}");
}

/// An answer says when the workspace is not indexed: nothing is, or only
/// other directories are. Once it is, nothing is said.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_says_when_the_workspace_is_not_indexed() {
    let projects = Projects::new();
    let ours = projects.project("ours", &[("a.py", "def shared_name():\n    return 1\n")]);
    let theirs = projects.project("theirs", &[("b.py", "def shared_name():\n    return 2\n")]);
    let server = projects.server(&ours);
    let search = json!({"query": "shared_name"});

    let empty = call(&server, "search_functions", search.clone()).await;
    let notice = empty["notice"]
        .as_str()
        .unwrap_or_else(|| panic!("{empty}"));
    assert!(notice.starts_with("Nothing is indexed yet"), "{notice}");
    assert!(notice.contains(&*ours.to_string_lossy()), "{notice}");

    call(
        &projects.server(&theirs),
        "scan_codebase",
        json!({"path": theirs}),
    )
    .await;
    let elsewhere = call(&server, "search_functions", search.clone()).await;
    let notice = elsewhere["notice"]
        .as_str()
        .unwrap_or_else(|| panic!("{elsewhere}"));
    assert!(notice.contains("is not indexed"), "{notice}");
    assert!(notice.contains(&*ours.to_string_lossy()), "{notice}");
    let dead_elsewhere = call(&server, "find_dead_code", json!({})).await;
    assert!(dead_elsewhere["notice"].is_string(), "{dead_elsewhere}");

    call(&server, "scan_codebase", json!({"path": ours})).await;
    let indexed = call(&server, "search_functions", search).await;
    assert!(indexed.get("notice").is_none(), "{indexed}");
}

/// A search names the Context Graph notes its query matches, by type and
/// title; with none, it says nothing of notes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_names_the_notes_its_query_matches() {
    let projects = Projects::new();
    let ours = projects.project(
        "ours",
        &[(
            "prices.py",
            "def total_cents(items):\n    return sum(items)\n",
        )],
    );
    let server = projects.server(&ours);
    call(&server, "scan_codebase", json!({"path": ours})).await;

    let before = call(&server, "search_functions", json!({"query": "cents"})).await;
    assert!(before.get("context_notes").is_none(), "{before}");

    call(
        &server,
        "add_context_note",
        json!({
            "note_type": "adr",
            "title": "Money is kept in integer cents",
            "content": "Floats lost cents in totals, so every amount is an integer count of cents.",
            "priority": 8
        }),
    )
    .await;
    for tool in ["search_functions", "semantic_search", "fuzzy_search"] {
        let answer = call(&server, tool, json!({"query": "cents"})).await;
        assert_eq!(
            answer["context_notes"],
            json!(["adr: Money is kept in integer cents"]),
            "{tool}: {answer}"
        );
    }
    let unrelated = call(&server, "search_functions", json!({"query": "parser"})).await;
    assert!(unrelated.get("context_notes").is_none(), "{unrelated}");
}
