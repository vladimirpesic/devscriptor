//! What `get_file_context` and `describe_modification` say about a file:
//! its real imports and the files the graph connects it to, and only the
//! entities a change touched.
//!
//! One test in its own binary: it changes the process-wide working directory,
//! as an agent's relative paths are resolved against it.

use devscriptor_mcp::{CoreModule, DevscriptorServerState, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;

async fn call(module: &CoreModule, tool: &str, args: Value) -> Value {
    match module.handle_tool(tool, Some(args)).await {
        Ok(ToolResult::Success(value)) => value,
        other => panic!("{tool} failed: {other:?}"),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn file_context_and_modifications_say_what_is_there() {
    let db_dir = tempfile::tempdir().expect("db dir");
    let project = tempfile::tempdir().expect("project dir");
    // The spelling Devscriptor stores paths in: canonical, without the `\\?\`
    // prefix `std` gives a canonical path on Windows.
    let root = dunce::canonicalize(project.path()).unwrap();
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("util.py"),
        "def helper(x):\n    return x + 1\n\n\ndef unused(y):\n    return y\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.py"),
        "from util import helper\nimport os as system\n\n\ndef run():\n    return helper(2)\n",
    )
    .unwrap();

    let state = Arc::new(
        DevscriptorServerState::with_db_path(db_dir.path().join("graph.db")).expect("state"),
    );
    let module = CoreModule::new(state);
    std::env::set_current_dir(&root).expect("cd into project");
    call(
        &module,
        "scan_codebase",
        json!({ "path": root.to_string_lossy() }),
    )
    .await;

    // The imports are the file's own, not its definitions; the files are the
    // ones the graph connects it to, each way.
    let main = call(
        &module,
        "get_file_context",
        json!({ "file_path": "src/main.py" }),
    )
    .await;
    assert_eq!(
        strings(&main["imports"]),
        ["helper from util", "os as system"],
        "{main}"
    );
    let util = src.join("util.py").to_string_lossy().into_owned();
    let main_path = src.join("main.py").to_string_lossy().into_owned();
    assert_eq!(
        strings(&main["dependencies"]),
        std::slice::from_ref(&util),
        "{main}"
    );
    let util_context = call(
        &module,
        "get_file_context",
        json!({ "file_path": "src/util.py" }),
    )
    .await;
    assert_eq!(
        strings(&util_context["related_files"]),
        [main_path],
        "{util_context}"
    );
    assert!(strings(&util_context["imports"]).is_empty());

    // Changing one function of two marks that one modified, and no other.
    let original = "def a():\n    return 1\n\n\ndef b():\n    return 2\n";
    let describe = |modified: &str| {
        json!({
            "file_path": util.clone(),
            "original_content": original,
            "modified_content": modified,
        })
    };
    let one = call(
        &module,
        "describe_modification",
        describe("def a():\n    return 1\n\n\ndef b():\n    return 3\n"),
    )
    .await;
    assert_eq!(strings(&one["entities_modified"]), ["b"], "{one}");
    assert!(strings(&one["entities_added"]).is_empty(), "{one}");
    assert!(strings(&one["entities_removed"]).is_empty(), "{one}");

    // A rename is the old name removed and the new one added.
    let renamed = call(
        &module,
        "describe_modification",
        describe("def a():\n    return 1\n\n\ndef c():\n    return 2\n"),
    )
    .await;
    assert_eq!(strings(&renamed["entities_added"]), ["c"], "{renamed}");
    assert_eq!(strings(&renamed["entities_removed"]), ["b"], "{renamed}");
    assert!(
        strings(&renamed["entities_modified"]).is_empty(),
        "{renamed}"
    );

    // An edit outside every entity, an import, changes none of them.
    let import_only = call(
        &module,
        "describe_modification",
        describe(&format!("import sys\n{original}")),
    )
    .await;
    assert!(
        strings(&import_only["entities_modified"]).is_empty(),
        "{import_only}"
    );
    assert_eq!(import_only["lines_added"], 1);

    // A method is named with its class; the class holding it changed too.
    let class = "class Shop:\n    def price(self):\n        return 1\n\n    def name(self):\n        return 'x'\n";
    let method = call(
        &module,
        "describe_modification",
        json!({
            "file_path": util,
            "original_content": class,
            "modified_content": class.replace("return 1", "return 2"),
        }),
    )
    .await;
    let mut changed = strings(&method["entities_modified"]);
    changed.sort();
    assert_eq!(changed, ["Shop", "Shop.price"], "{method}");
}
