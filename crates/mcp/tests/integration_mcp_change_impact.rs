//! `get_change_impact` over a real git repository: the entities a change
//! changes, what depends on them, and the tests to run for it.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A git repository holding a small Python project, committed, and a module
/// whose index is in a folder apart.
fn project() -> (CoreModule, TempDir, TempDir) {
    let repo = TempDir::new().unwrap();
    let root = repo.path();
    write(
        root,
        "cart.py",
        "def price(item):\n    return item * 2\n\n\ndef total(items):\n    result = 0\n    for item in items:\n        result += price(item)\n    return result\n",
    );
    write(
        root,
        "checkout.py",
        "from cart import total\n\n\ndef checkout(items):\n    return total(items)\n",
    );
    write(
        root,
        "tests/test_cart.py",
        "from cart import total\n\n\ndef test_total():\n    assert total([1, 2]) == 6\n",
    );
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "start",
        ],
    );
    let index = TempDir::new().unwrap();
    let state =
        devscriptor_mcp::DevscriptorServerState::with_db_path(index.path().join("impact.db"))
            .unwrap();
    (CoreModule::new(Arc::new(state)), repo, index)
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

async fn call(module: &CoreModule, tool: &str, args: Value) -> Value {
    match module.handle_tool(tool, Some(args)).await.unwrap() {
        ToolResult::Success(value) => value,
        ToolResult::Error(e) => panic!("{tool}: {e}"),
    }
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_change_names_its_entities_their_dependents_and_its_tests() {
    let (module, repo, _index) = project();
    let root = repo.path().display().to_string();
    call(&module, "scan_codebase", json!({ "path": root })).await;

    let clean = call(&module, "get_change_impact", json!({ "path": root })).await;
    assert!(clean["files"].as_array().unwrap().is_empty(), "{clean}");

    write(
        repo.path(),
        "cart.py",
        "def price(item):\n    return item * 3\n\n\ndef total(items):\n    result = 0\n    for item in items:\n        result += price(item)\n    return result\n",
    );
    let impact = call(&module, "get_change_impact", json!({ "path": root })).await;
    assert_eq!(impact["base"], "HEAD");
    assert_eq!(names(&impact["changed"]), ["price"], "{impact}");
    assert_eq!(impact["changed"][0]["lines"], "2");
    assert_eq!(impact["files"][0]["status"], "modified");
    assert_eq!(impact["files"][0]["changed_lines"], 1);

    let dependents = impact["dependents"].as_array().unwrap();
    let total = dependents.iter().find(|d| d["name"] == "total").unwrap();
    assert_eq!(total["depth"], 1);
    assert_eq!(total["reaches"], json!(["price"]));
    let checkout = dependents.iter().find(|d| d["name"] == "checkout").unwrap();
    assert_eq!(checkout["depth"], 2);
    assert_eq!(names(&impact["tests"]), ["test_total"], "{impact}");
    assert_eq!(impact["tests"][0]["depth"], 2);
    assert_eq!(impact["tests"][0]["reaches"], json!(["price"]));

    // Depth 1 stops at the direct callers.
    let near = call(
        &module,
        "get_change_impact",
        json!({ "path": root, "depth": 1 }),
    )
    .await;
    assert_eq!(names(&near["dependents"]), ["total"], "{near}");
    assert!(near["tests"].as_array().unwrap().is_empty());

    // An untracked file is part of the change.
    write(repo.path(), "extra.py", "def extra():\n    return 1\n");
    let with_new = call(&module, "get_change_impact", json!({ "path": root })).await;
    let extra = with_new["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["file_path"].as_str().unwrap().ends_with("extra.py"))
        .unwrap();
    assert_eq!(extra["status"], "untracked");
    assert!(names(&with_new["changed"]).contains(&"extra"), "{with_new}");

    // A range compares commits, not the working tree.
    let range = call(
        &module,
        "get_change_impact",
        json!({ "path": root, "base": "HEAD..HEAD" }),
    )
    .await;
    assert!(range["files"].as_array().unwrap().is_empty(), "{range}");
}

#[tokio::test]
async fn an_option_or_a_bad_bound_is_refused() {
    let (module, repo, _index) = project();
    let root = repo.path().display().to_string();
    for args in [
        json!({ "path": root, "base": "--output=/tmp/x" }),
        json!({ "path": root, "base": "HEAD main" }),
        json!({ "path": root, "depth": 0 }),
        json!({ "path": root, "depth": 7 }),
        json!({ "path": root, "limit": "many" }),
    ] {
        assert!(
            module
                .handle_tool("get_change_impact", Some(args.clone()))
                .await
                .is_err(),
            "{args}"
        );
    }
    let outside = TempDir::new().unwrap();
    assert!(
        module
            .handle_tool(
                "get_change_impact",
                Some(json!({ "path": outside.path().display().to_string() }))
            )
            .await
            .is_err(),
        "not a git repository"
    );
}

/// A Rust test in a source file, which neither its file nor its name marks,
/// is found by its `#[test]` attribute.
#[tokio::test]
async fn a_test_attribute_marks_a_test_in_a_source_file() {
    let repo = TempDir::new().unwrap();
    let root = repo.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = |factor: u32| {
        format!(
            "pub fn price(item: u32) -> u32 {{\n    item * {factor}\n}}\n\npub fn total(items: &[u32]) -> u32 {{\n    items.iter().map(|&item| price(item)).sum()\n}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn sums_the_prices() {{\n        assert_eq!(total(&[1]), {factor});\n    }}\n}}\n"
        )
    };
    write(root, "src/lib.rs", &source(2));
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "start",
        ],
    );
    let index = TempDir::new().unwrap();
    let state =
        devscriptor_mcp::DevscriptorServerState::with_db_path(index.path().join("impact.db"))
            .unwrap();
    let module = CoreModule::new(Arc::new(state));
    let path = root.display().to_string();
    call(&module, "scan_codebase", json!({ "path": path })).await;

    let text = source(2).replacen("item * 2", "item * 3", 1);
    write(root, "src/lib.rs", &text);
    let impact = call(&module, "get_change_impact", json!({ "path": path })).await;
    assert_eq!(names(&impact["changed"]), ["price"], "{impact}");
    assert!(
        names(&impact["tests"]).contains(&"sums_the_prices"),
        "{impact}"
    );
    assert!(
        !names(&impact["dependents"]).contains(&"sums_the_prices"),
        "{impact}"
    );
}

/// The index the server reads, kept in the repository and not ignored, is
/// no part of a change.
#[tokio::test]
async fn the_index_in_the_repository_is_no_part_of_a_change() {
    let (_, repo, _index) = project();
    let state =
        devscriptor_mcp::DevscriptorServerState::with_db_path(repo.path().join("devscriptor.db"))
            .unwrap();
    let module = CoreModule::new(Arc::new(state));
    let root = repo.path().display().to_string();
    call(&module, "scan_codebase", json!({ "path": root })).await;
    write(repo.path(), "extra.py", "def extra():\n    return 1\n");

    let impact = call(&module, "get_change_impact", json!({ "path": root })).await;
    let files: Vec<&str> = impact["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["file_path"].as_str().unwrap())
        .collect();
    assert_eq!(files.len(), 1, "{impact}");
    assert!(files[0].ends_with("extra.py"), "{impact}");
}
