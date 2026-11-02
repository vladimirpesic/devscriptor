//! Integration tests for MCP tool parameters: each one the JSON schema declares
//! is read by its handler.
//!
//! Each test asserts the parameter *changes behaviour* — passing it and omitting
//! it must produce different results — so a parameter the schema offers but the
//! handler ignores cannot pass unnoticed.

use std::sync::Arc;

use devscriptor_mcp::{CoreModule, ToolResult};
use serde_json::json;
use tempfile::TempDir;

fn setup() -> (CoreModule, TempDir) {
    let _ = tracing_subscriber::fmt::try_init();
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("params_test.db");
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

/// A project with a Rust file and a Python file, plus a second Rust directory.
///
/// The second directory is deliberately **not** one of the scanner's default
/// excludes (`vendor`, `target`, `node_modules`, …), otherwise the
/// `exclude_patterns` test would pass without the parameter doing anything.
fn write_mixed_project(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("extra")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn rust_only_symbol() {}\npub fn rust_public_symbol() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/app.py"),
        "def python_only_symbol():\n    pass\n",
    )
    .unwrap();
    std::fs::write(root.join("extra/extra_mod.rs"), "fn extra_symbol() {}\n").unwrap();
}

async fn scan(module: &CoreModule, args: serde_json::Value) -> serde_json::Value {
    inner(&success(
        module.handle_tool("scan_codebase", Some(args)).await,
    ))
}

async fn statistics(module: &CoreModule, args: serde_json::Value) -> serde_json::Value {
    inner(&success(
        module.handle_tool("get_statistics", Some(args)).await,
    ))
}

async fn entity_names(module: &CoreModule, args: serde_json::Value) -> Vec<String> {
    let out = inner(&success(
        module.handle_tool("search_entities", Some(args)).await,
    ));
    out["entities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// scan_codebase: extensions (whitelist) and exclude_patterns
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_codebase_extensions_whitelists_languages() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    write_mixed_project(&proj);

    scan(
        &module,
        json!({"path": proj.to_string_lossy(), "extensions": ["rs"]}),
    )
    .await;

    let names = entity_names(&module, json!({"query": "symbol", "limit": 100})).await;
    assert!(
        names.iter().any(|n| n == "rust_only_symbol"),
        "rust symbol should be scanned, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "python_only_symbol"),
        "`extensions: [rs]` must exclude .py files, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "extra_symbol"),
        "extra/ is not a default exclude, so its .rs file must be scanned: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_codebase_without_extensions_scans_all_languages() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    write_mixed_project(&proj);

    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let names = entity_names(&module, json!({"query": "symbol", "limit": 100})).await;
    assert!(
        names.iter().any(|n| n == "python_only_symbol"),
        "omitting `extensions` must keep the default (all languages), got {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_codebase_exclude_patterns_skips_matching_files() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    write_mixed_project(&proj);

    scan(
        &module,
        json!({
            "path": proj.to_string_lossy(),
            "extensions": ["rs"],
            "exclude_patterns": ["**/extra/**"]
        }),
    )
    .await;

    let names = entity_names(&module, json!({"query": "symbol", "limit": 100})).await;
    assert!(
        names.iter().any(|n| n == "rust_only_symbol"),
        "src should still be scanned, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "extra_symbol"),
        "`exclude_patterns` must skip extra/, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// get_statistics: path
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_statistics_path_scopes_counts() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    write_mixed_project(&proj);
    scan(
        &module,
        json!({"path": proj.to_string_lossy(), "extensions": ["rs"]}),
    )
    .await;

    let all = statistics(&module, json!({})).await;
    let extra = statistics(&module, json!({"path": "extra"})).await;

    let all_entities = all["total_entities"].as_u64().unwrap();
    let extra_entities = extra["total_entities"].as_u64().unwrap();

    assert!(all_entities > 0, "expected some entities, got {all}");
    assert!(
        extra_entities > 0 && extra_entities < all_entities,
        "`path` must scope the counts: extra={extra_entities} all={all_entities}"
    );
    assert_eq!(extra["path"], json!("extra"), "response echoes the scope");
    assert_eq!(extra["total_files"], json!(1), "extra/ holds one file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_statistics_unmatched_path_returns_zero_not_global_totals() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    write_mixed_project(&proj);
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let none = statistics(&module, json!({"path": "/definitely/not/here"})).await;
    assert_eq!(none["total_entities"], json!(0));
}

// ---------------------------------------------------------------------------
// search_entities: entity_type
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_entities_entity_type_filters_results() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("m.rs"), "struct Shape {}\nfn shape_helper() {}\n").unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let unfiltered = entity_names(&module, json!({"query": "Shape", "limit": 50})).await;
    assert!(!unfiltered.is_empty(), "expected a match for Shape");

    let functions = inner(&success(
        module
            .handle_tool(
                "search_entities",
                Some(json!({"query": "Shape", "entity_type": "function", "limit": 50})),
            )
            .await,
    ));
    for e in functions["entities"].as_array().unwrap_or(&vec![]) {
        assert_eq!(
            e["entity_type"], "function",
            "entity_type filter must be applied in SQL"
        );
    }

    let classes = inner(&success(
        module
            .handle_tool(
                "search_entities",
                Some(json!({"query": "Shape", "entity_type": "class", "limit": 50})),
            )
            .await,
    ));
    for e in classes["entities"].as_array().unwrap_or(&vec![]) {
        assert_eq!(e["entity_type"], "class");
    }
}

// ---------------------------------------------------------------------------
// search_functions / search_classes / get_entity_details: file_path
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_functions_file_path_filters_by_file() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("alpha.rs"), "fn duplicated() {}\n").unwrap();
    std::fs::write(proj.join("beta.rs"), "fn duplicated() {}\n").unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let all = inner(&success(
        module
            .handle_tool("search_functions", Some(json!({"query": "duplicated"})))
            .await,
    ));
    assert_eq!(
        all["functions"].as_array().unwrap().len(),
        2,
        "both files define the function"
    );

    let scoped = inner(&success(
        module
            .handle_tool(
                "search_functions",
                Some(json!({"query": "duplicated", "file_path": "alpha.rs"})),
            )
            .await,
    ));
    let scoped_fns = scoped["functions"].as_array().unwrap();
    assert_eq!(scoped_fns.len(), 1, "file_path must narrow the results");
    assert!(
        scoped_fns[0]["file_path"]
            .as_str()
            .unwrap()
            .ends_with("alpha.rs")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_entity_details_file_path_disambiguates() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("alpha.rs"), "fn duplicated() {}\n").unwrap();
    std::fs::write(proj.join("beta.rs"), "fn duplicated() {}\n").unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let beta = inner(&success(
        module
            .handle_tool(
                "get_entity_details",
                Some(json!({
                    "entity_type": "function",
                    "name": "duplicated",
                    "file_path": "beta.rs"
                })),
            )
            .await,
    ));
    assert!(
        beta["file_path"].as_str().unwrap().ends_with("beta.rs"),
        "file_path must select the entity from that file, got {beta}"
    );

    // A file that matches nothing must report "not found", not fall back to any entity.
    let missing = inner(&success(
        module
            .handle_tool(
                "get_entity_details",
                Some(json!({
                    "entity_type": "function",
                    "name": "duplicated",
                    "file_path": "no_such_file.rs"
                })),
            )
            .await,
    ));
    assert!(missing["error"].is_string(), "expected a not-found error");
}

/// The details of an entity are what the index holds of its kind: a
/// struct's fields and methods, an interface's methods, a function's
/// enclosing function and decorators; and where the others of its name are.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_entity_details_reports_the_structure_of_every_kind() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("cart.rs"),
        "pub struct Cart { items: Vec<u32> }\nimpl Cart {\n    pub fn add(&mut self) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("shape.go"),
        "package shape\n\ntype Shape interface {\n\tArea() float64\n}\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("views.py"),
        "import functools\n\ndef page():\n    @functools.lru_cache(maxsize=1)\n    def render():\n        \
         return 1\n    return render\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("jobs.py"),
        "def other():\n    def render():\n        return 2\n    return render\n",
    )
    .unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;
    let details = |arguments: serde_json::Value| {
        let module = &module;
        async move {
            inner(&success(
                module
                    .handle_tool("get_entity_details", Some(arguments))
                    .await,
            ))
        }
    };

    let cart = details(json!({"entity_type": "struct", "name": "Cart"})).await;
    assert_eq!(cart["methods"], json!(["add"]), "{cart}");
    assert_eq!(
        cart["fields"],
        json!([{"name": "items", "type": "Vec<u32>"}]),
        "{cart}"
    );
    let shape = details(json!({"entity_type": "interface", "name": "Shape"})).await;
    assert_eq!(shape["methods"], json!(["Area"]), "{shape}");

    let render = details(json!({
        "entity_type": "function",
        "name": "render",
        "file_path": "views.py"
    }))
    .await;
    assert_eq!(render["parent_function"], json!("page"), "{render}");
    assert_eq!(
        render["decorators"],
        json!(["functools.lru_cache"]),
        "{render}"
    );
    assert!(render.get("other_matches").is_none(), "{render}");
    let either = details(json!({"entity_type": "function", "name": "render"})).await;
    assert_eq!(either["total_matches"], json!(2), "{either}");
    assert_eq!(either["other_matches"].as_array().map(Vec::len), Some(1));
}

// ---------------------------------------------------------------------------
// get_relationships: relationship_type
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_relationships_relationship_type_filters_edges() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("m.rs"),
        "fn callee() {}\nfn caller() { callee(); }\n",
    )
    .unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let all = inner(&success(
        module
            .handle_tool("get_relationships", Some(json!({"entity_name": "caller"})))
            .await,
    ));
    let total = all["total_relationships"].as_u64().unwrap();

    let filtered = inner(&success(
        module
            .handle_tool(
                "get_relationships",
                Some(
                    json!({"entity_name": "caller", "relationship_type": "definitely_not_a_type"}),
                ),
            )
            .await,
    ));
    assert_eq!(
        filtered["total_relationships"],
        json!(0),
        "an unmatched relationship_type must filter every edge (total was {total})"
    );
    // Without an explicit entity_type the response reports "any", not a silent default.
    assert_eq!(all["entity_type"], json!("any"));
}

/// Each edge is given once, as its source's, with where both ends are and
/// the lines a call is made on: a caller calling twice has one `calls` edge,
/// on both lines, and no `called_by` row besides it unless asked for, so a
/// model citing a call from the index cites the line it is made on, not the
/// caller's first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_relationships_gives_each_edge_once_with_where_it_is() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("m.rs"),
        "fn callee() {}\nfn caller() {\n    callee();\n    callee();\n}\n",
    )
    .unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;
    let relationships = |args: serde_json::Value| {
        let module = &module;
        async move {
            inner(&success(
                module.handle_tool("get_relationships", Some(args)).await,
            ))["relationships"]
                .as_array()
                .expect("a list of relationships")
                .clone()
        }
    };

    let edges = relationships(json!({"entity_name": "callee"})).await;
    assert_eq!(edges.len(), 1, "{edges:?}");
    let edge = &edges[0];
    assert_eq!(edge["source"], "caller", "{edge}");
    assert_eq!(edge["target"], "callee", "{edge}");
    assert_eq!(edge["type"], "calls", "{edge}");
    assert_eq!(edge["lines"], json!([3, 4]), "{edge}");
    assert_eq!(edge["source_line"], json!(2), "{edge}");
    assert_eq!(edge["target_line"], json!(1), "{edge}");
    for end in ["source_file", "target_file"] {
        assert!(edge[end].as_str().unwrap().ends_with("m.rs"), "{edge}");
    }

    let reverse =
        relationships(json!({"entity_name": "callee", "relationship_type": "called_by"})).await;
    assert_eq!(reverse.len(), 1, "{reverse:?}");
    assert_eq!(reverse[0]["source"], "callee", "{reverse:?}");
}

/// `file_path` tells apart entities of one name: of two `points_for`, the
/// one a file names is the one whose callers are given.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_relationships_file_path_picks_one_of_a_name() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("loyalty.py"),
        "def points_for(cents):\n    return cents // 100\n\n\ndef charge(cents):\n    return points_for(cents)\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("cart.py"),
        "def points_for(cents):\n    return cents // 50\n\n\ndef points(cart):\n    return points_for(cart)\n",
    )
    .unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let callers = |file: &'static str| {
        let module = &module;
        async move {
            let answer = inner(&success(
                module
                    .handle_tool(
                        "get_relationships",
                        Some(json!({
                            "entity_name": "points_for",
                            "file_path": file,
                            "relationship_type": "calls",
                        })),
                    )
                    .await,
            ));
            answer["relationships"]
                .as_array()
                .expect("a list of relationships")
                .iter()
                .map(|edge| edge["source"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(callers("loyalty.py").await, ["charge"]);
    assert_eq!(callers("cart.py").await, ["points"]);
}

// ---------------------------------------------------------------------------
// find_dead_code: min_confidence
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_dead_code_min_confidence_filters_public_symbols() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("lib.rs"),
        "pub fn public_dead() {}\nfn private_dead() {}\n",
    )
    .unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let permissive = inner(&success(
        module
            .handle_tool(
                "find_dead_code",
                Some(json!({"path": proj.to_string_lossy(), "min_confidence": 0.0})),
            )
            .await,
    ));
    let strict = inner(&success(
        module
            .handle_tool(
                "find_dead_code",
                Some(json!({"path": proj.to_string_lossy(), "min_confidence": 0.9})),
            )
            .await,
    ));

    let names = |v: &serde_json::Value| -> Vec<String> {
        v["unused_functions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|i| i["name"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let permissive_names = names(&permissive);
    let strict_names = names(&strict);

    assert!(
        permissive_names.contains(&"private_dead".to_string()),
        "private dead function must be reported, got {permissive_names:?}"
    );
    assert!(
        permissive_names.contains(&"public_dead".to_string()),
        "min_confidence 0.0 keeps public findings, got {permissive_names:?}"
    );
    assert!(
        strict_names.contains(&"private_dead".to_string()),
        "private findings score 0.9 and survive min_confidence 0.9, got {strict_names:?}"
    );
    assert!(
        !strict_names.contains(&"public_dead".to_string()),
        "public findings score 0.5 and must be dropped at min_confidence 0.9, got {strict_names:?}"
    );

    // Every item exposes the confidence it was filtered on.
    for item in permissive["unused_functions"].as_array().unwrap() {
        let c = item["confidence"].as_f64().expect("confidence present");
        assert!((0.0..=1.0).contains(&c), "confidence out of range: {c}");
        assert!(item["reason"].is_string(), "reason present");
    }
    // The surest come first, so that `limit` keeps them; and what was in
    // scope is counted, telling no dead code from nothing indexed.
    assert_eq!(permissive_names, ["private_dead", "public_dead"]);
    assert_eq!(permissive["total_entities_analyzed"], json!(2));

    // A path that is not a directory is an error, not "no dead code".
    for (path, error) in [
        (proj.join("missing"), "does not exist"),
        (proj.join("lib.rs"), "not a directory"),
    ] {
        let failure = module
            .handle_tool(
                "find_dead_code",
                Some(json!({"path": path.to_string_lossy()})),
            )
            .await
            .expect_err("an error");
        assert!(failure.to_string().contains(error), "{failure}");
    }
}

// ---------------------------------------------------------------------------
// analyze_complexity: file_path
// ---------------------------------------------------------------------------

/// A file's functions are counted and ranked whatever their complexity: a
/// file of three simple functions has three analysed, though none is above
/// the threshold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_complexity_file_path_scopes_results() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let branchy = "fn f(x: i32) -> i32 { if x > 0 { if x > 1 { 1 } else { 2 } } else { 3 } }\n\
                   fn g() -> i32 { 1 }\n\
                   fn h(x: i32) -> i32 { if x > 0 { 1 } else { 0 } }\n";
    std::fs::write(proj.join("alpha.rs"), branchy).unwrap();
    std::fs::write(proj.join("beta.rs"), branchy).unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let scoped = inner(&success(
        module
            .handle_tool(
                "analyze_complexity",
                Some(json!({"path": proj.to_string_lossy(), "file_path": "alpha.rs"})),
            )
            .await,
    ));

    assert_eq!(scoped["total_functions_analyzed"], 3, "{scoped}");
    assert_eq!(scoped["threshold"], 10, "{scoped}");
    assert_eq!(scoped["functions_above_threshold"], 0, "{scoped}");
    let listed = scoped["most_complex_functions"].as_array().unwrap();
    let names: Vec<&str> = listed.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(
        names.first(),
        Some(&"f"),
        "the most complex first: {scoped}"
    );
    assert_eq!(listed.len(), 3, "{scoped}");
    for f in listed {
        assert!(
            f["file_path"].as_str().unwrap().ends_with("alpha.rs"),
            "file_path must scope complexity results, got {f}"
        );
        assert_eq!(f["risk_level"], "low", "{f}");
    }

    let everything = inner(&success(
        module
            .handle_tool("analyze_complexity", Some(json!({"top_n": 2})))
            .await,
    ));
    assert_eq!(everything["total_functions_analyzed"], 6, "{everything}");
    assert_eq!(
        everything["most_complex_functions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

// ---------------------------------------------------------------------------
// analyze_code: `path` is optional (its schema never marked it required)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_code_accepts_schema_valid_call_without_path() {
    let (module, _temp) = setup();
    // The schema requires only `analysis_type`; `path` defaults to the whole index.
    let result = module
        .handle_tool("analyze_code", Some(json!({"analysis_type": "dead_code"})))
        .await;
    assert!(
        result.is_ok(),
        "schema-valid call must not error: {result:?}"
    );
}

/// `analyze_code` returns the findings under its `path`, each with its file
/// and line, whatever the analysis, not only for dead code. Cross references
/// count calls, not their reverse rows, so what nothing calls is a caller
/// none calls back, not every entity that calls anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_code_scopes_its_findings_and_says_where_they_are() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    for (dir, file, source) in [
        (
            "alpha",
            "a.py",
            "def entry():\n    return helper()\n\n\ndef helper():\n    return leaf()\n\n\n\
             def leaf():\n    return 1\n",
        ),
        (
            "beta",
            "b.py",
            "def caller():\n    return callee()\n\n\ndef callee():\n    return 2\n",
        ),
    ] {
        std::fs::create_dir_all(proj.join(dir)).unwrap();
        std::fs::write(proj.join(dir).join(file), source).unwrap();
    }
    scan(&module, json!({"path": proj.to_string_lossy()})).await;
    let unused = |path: String| {
        let module = &module;
        async move {
            let response = inner(&success(
                module
                    .handle_tool(
                        "analyze_code",
                        Some(json!({"analysis_type": "cross_reference", "path": path})),
                    )
                    .await,
            ));
            let mut found: Vec<(String, String, u64)> = response["findings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|finding| {
                    let file = finding["file_path"].as_str().unwrap_or_default();
                    (
                        finding["title"].as_str().unwrap_or_default().to_string(),
                        std::path::Path::new(file)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        finding["line"].as_u64().unwrap_or_default(),
                    )
                })
                .collect();
            found.sort();
            found
        }
    };
    let finding = |title: &str, file: &str| (title.to_string(), file.to_string(), 1);
    assert_eq!(
        unused(proj.join("alpha").to_string_lossy().into_owned()).await,
        [finding("Unused entity: entry", "a.py")]
    );
    assert_eq!(
        unused(".".to_string()).await,
        [
            finding("Unused entity: caller", "b.py"),
            finding("Unused entity: entry", "a.py")
        ]
    );
    let failure = module
        .handle_tool(
            "analyze_code",
            Some(json!({
                "analysis_type": "cross_reference",
                "path": proj.join("gamma").to_string_lossy()
            })),
        )
        .await
        .expect_err("a missing path is an error");
    assert!(failure.to_string().contains("does not exist"), "{failure}");
}

/// A clone pair is where its first copy is, and under either copy's
/// directory; a call cycle where its first function is, and under its
/// directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_code_locates_clones_and_cycles() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    let body = "    total = 0\n    for item in items:\n        if item > 0:\n            \
                total += item\n        else:\n            total -= item\n    return total\n";
    for (dir, file, source) in [
        ("alpha", "a.py", format!("def sum_alpha(items):\n{body}")),
        ("beta", "b.py", format!("def sum_beta(items):\n{body}")),
        (
            "gamma",
            "c.py",
            "def ping(n):\n    return pong(n - 1)\n\n\ndef pong(n):\n    return ping(n - 1)\n"
                .to_string(),
        ),
    ] {
        std::fs::create_dir_all(proj.join(dir)).unwrap();
        std::fs::write(proj.join(dir).join(file), source).unwrap();
    }
    scan(&module, json!({"path": proj.to_string_lossy()})).await;
    let findings = |analysis: &'static str, dir: &'static str| {
        let module = &module;
        let path = proj.join(dir).to_string_lossy().into_owned();
        async move {
            let response = inner(&success(
                module
                    .handle_tool(
                        "analyze_code",
                        Some(json!({"analysis_type": analysis, "path": path})),
                    )
                    .await,
            ));
            response["findings"].as_array().cloned().unwrap_or_default()
        }
    };
    for dir in ["alpha", "beta"] {
        let clones = findings("clones", dir).await;
        assert_eq!(clones.len(), 1, "{dir}: {clones:?}");
        let file = clones[0]["file_path"].as_str().unwrap_or_default();
        assert!(
            file.ends_with("a.py") || file.ends_with("b.py"),
            "{clones:?}"
        );
        assert_eq!(clones[0]["line"], json!(1), "{clones:?}");
    }
    assert!(findings("clones", "gamma").await.is_empty());
    let cycles: Vec<_> = findings("call_graph", "gamma")
        .await
        .into_iter()
        .filter(|finding| finding["title"] == json!("Circular dependency detected"))
        .collect();
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    assert!(
        cycles[0]["file_path"]
            .as_str()
            .is_some_and(|file| file.ends_with("c.py")),
        "{cycles:?}"
    );
    assert!(cycles[0]["line"].as_u64().is_some_and(|line| line > 0));
    assert!(
        findings("call_graph", "alpha")
            .await
            .iter()
            .all(|finding| finding["title"] != json!("Circular dependency detected"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_code_rejects_unknown_analysis_type_and_lists_valid_ones() {
    let (module, _temp) = setup();
    let err = module
        .handle_tool("analyze_code", Some(json!({"analysis_type": "nope"})))
        .await
        .expect_err("unknown analysis_type must error");
    let msg = format!("{err}");
    for expected in [
        "dead_code",
        "call_graph",
        "code_smells",
        "cross_file",
        "all",
    ] {
        assert!(
            msg.contains(expected),
            "error should list {expected}: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// get_file_context: context_lines
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_file_context_context_lines_controls_snippet_size() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let src = (1..=40)
        .map(|i| {
            if i == 20 {
                "fn target() {}".to_string()
            } else {
                format!("// filler {i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let file = proj.join("big.rs");
    std::fs::write(&file, src).unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;

    let narrow = inner(&success(
        module
            .handle_tool(
                "get_file_context",
                Some(json!({"file_path": file.to_string_lossy(), "context_lines": 1})),
            )
            .await,
    ));
    let wide = inner(&success(
        module
            .handle_tool(
                "get_file_context",
                Some(json!({"file_path": file.to_string_lossy(), "context_lines": 8})),
            )
            .await,
    ));

    let snippet_lines = |v: &serde_json::Value| -> usize {
        v["entity_contexts"][0]["snippet"]
            .as_str()
            .map(|s| s.lines().count())
            .unwrap_or(0)
    };

    let narrow_lines = snippet_lines(&narrow);
    let wide_lines = snippet_lines(&wide);
    assert_eq!(narrow_lines, 3, "1 line either side of the definition");
    assert_eq!(wide_lines, 17, "8 lines either side of the definition");
    assert!(
        wide_lines > narrow_lines,
        "context_lines must change the snippet size"
    );
    assert_eq!(narrow["entity_contexts"][0]["line"], json!(20));
    // Each snippet line carries its line in the file.
    assert_eq!(
        narrow["entity_contexts"][0]["snippet_start_line"],
        json!(19)
    );
    assert_eq!(
        narrow["entity_contexts"][0]["snippet"],
        json!("19\t// filler 19\n20\tfn target() {}\n21\t// filler 21")
    );
}

// ---------------------------------------------------------------------------
// describe_file: line count and preview
// ---------------------------------------------------------------------------

/// `describe_file` counts the file's lines and shows its start with each line
/// numbered; a character that the read limit cuts in two does not cost the
/// preview.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_file_counts_lines_and_numbers_its_preview() {
    let (module, temp) = setup();
    let proj = temp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let small = proj.join("small.rs");
    std::fs::write(&small, "fn a() {}\r\nfn b() {}\r\n").unwrap();
    // `é` takes two bytes: from byte 13 on, the 64 KiB read limit falls
    // inside one.
    let wide = proj.join("wide.rs");
    std::fs::write(&wide, format!("fn c() {{}}\n// {}\n", "é".repeat(40_000))).unwrap();
    scan(&module, json!({"path": proj.to_string_lossy()})).await;
    let describe = |file: &std::path::Path| {
        let module = &module;
        let file = file.to_string_lossy().into_owned();
        async move {
            inner(&success(
                module
                    .handle_tool("describe_file", Some(json!({"file_path": file})))
                    .await,
            ))
        }
    };

    let small = describe(&small).await;
    assert_eq!(small["total_lines"], json!(2), "{small}");
    assert_eq!(
        small["content_preview"],
        json!("1\tfn a() {}\n2\tfn b() {}"),
        "{small}"
    );

    let wide = describe(&wide).await;
    assert_eq!(wide["total_lines"], json!(2), "{wide}");
    let preview = wide["content_preview"]
        .as_str()
        .unwrap_or_else(|| panic!("a preview, though the limit cut a character: {wide}"));
    assert!(preview.starts_with("1\tfn c() {}\n2\t// éé"), "{preview}");
}

// ---------------------------------------------------------------------------
// get_related_context: entity_name and file_path
// ---------------------------------------------------------------------------

async fn add_note(module: &CoreModule, title: &str, entity_name: &str, file_path: &str) {
    success(
        module
            .handle_tool(
                "add_context_note",
                Some(json!({
                    "note_type": "adr",
                    "title": title,
                    "content": "why",
                    "related_entities": [{
                        "entity_type": "function",
                        "entity_name": entity_name,
                        "file_path": file_path
                    }]
                })),
            )
            .await,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_related_context_filters_by_entity_name_and_file_path() {
    let (module, _temp) = setup();
    add_note(&module, "note-alpha", "alpha_fn", "src/alpha.rs").await;
    add_note(&module, "note-beta", "beta_fn", "src/beta.rs").await;

    let titles = |v: &serde_json::Value| -> Vec<String> {
        v["results"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| r["title"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    let all = inner(&success(
        module
            .handle_tool(
                "get_related_context",
                Some(json!({"entity_type": "function"})),
            )
            .await,
    ));
    assert_eq!(titles(&all).len(), 2, "both notes link to a function");

    let by_name = inner(&success(
        module
            .handle_tool(
                "get_related_context",
                Some(json!({"entity_type": "function", "entity_name": "alpha_fn"})),
            )
            .await,
    ));
    assert_eq!(titles(&by_name), vec!["note-alpha".to_string()]);

    let by_file = inner(&success(
        module
            .handle_tool(
                "get_related_context",
                Some(json!({"entity_type": "function", "file_path": "beta.rs"})),
            )
            .await,
    ));
    assert_eq!(titles(&by_file), vec!["note-beta".to_string()]);

    let by_both_conflicting = inner(&success(
        module
            .handle_tool(
                "get_related_context",
                Some(json!({
                    "entity_type": "function",
                    "entity_name": "alpha_fn",
                    "file_path": "beta.rs"
                })),
            )
            .await,
    ));
    assert!(
        titles(&by_both_conflicting).is_empty(),
        "both filters must be ANDed"
    );
}

// ---------------------------------------------------------------------------
// prune_context: max_notes
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_context_max_notes_caps_retained_active_notes() {
    let (module, _temp) = setup();
    for i in 0..5 {
        success(
            module
                .handle_tool(
                    "add_context_note",
                    Some(json!({
                        "note_type": "general",
                        "title": format!("note-{i}"),
                        "content": "body",
                        "priority": i + 1
                    })),
                )
                .await,
        );
    }

    let stats_before = inner(&success(
        module
            .handle_tool("get_context_statistics", Some(json!({})))
            .await,
    ));
    assert_eq!(stats_before["total_notes"], json!(5));

    // Dry run reports the cap without changing anything.
    let dry = inner(&success(
        module
            .handle_tool(
                "prune_context",
                Some(json!({"max_notes": 2, "dry_run": true})),
            )
            .await,
    ));
    assert_eq!(
        dry["would_archive"],
        json!(3),
        "5 active notes capped at 2 → 3 archived, got {dry}"
    );
    assert!(
        dry["message"].as_str().unwrap().contains("at most 2"),
        "message must mention the cap: {dry}"
    );

    // Applying the cap archives the surplus; the notes are not deleted.
    let applied = inner(&success(
        module
            .handle_tool(
                "prune_context",
                Some(json!({"max_notes": 2, "dry_run": false})),
            )
            .await,
    ));
    assert_eq!(applied["archived_count"], json!(3));
    assert_eq!(
        applied["deleted_count"],
        json!(0),
        "archiving never deletes"
    );

    let stats_after = inner(&success(
        module
            .handle_tool("get_context_statistics", Some(json!({})))
            .await,
    ));
    assert_eq!(
        stats_after["total_notes"],
        json!(5),
        "archived notes are retained"
    );

    // Highest-priority notes survive the cap.
    let remaining = inner(&success(
        module
            .handle_tool(
                "search_context",
                Some(json!({"query": "note", "status": "active"})),
            )
            .await,
    ));
    let active: Vec<&str> = remaining["results"]
        .as_array()
        .map(|a| a.iter().filter_map(|r| r["title"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(active.len(), 2, "only the cap survives, got {active:?}");
    assert!(
        active.contains(&"note-4") && active.contains(&"note-3"),
        "the two highest-priority notes must survive, got {active:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prune_context_without_max_notes_keeps_every_recent_note() {
    let (module, _temp) = setup();
    for i in 0..3 {
        success(
            module
                .handle_tool(
                    "add_context_note",
                    Some(json!({"note_type": "general", "title": format!("n{i}"), "content": "b"})),
                )
                .await,
        );
    }
    let applied = inner(&success(
        module
            .handle_tool("prune_context", Some(json!({"dry_run": false})))
            .await,
    ));
    assert_eq!(
        applied["archived_count"],
        json!(0),
        "fresh notes are neither aged out nor capped"
    );
}
