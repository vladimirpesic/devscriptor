use super::schemas::{hierarchy_item_schema, position_schema};
use super::*;

mod name_resolution_tests {
    use super::*;

    #[test]
    fn identifier_column_finds_the_name_not_the_keyword() {
        // The stored column points at `def` (0); the identifier is at 4.
        let line = "def apply_discount(price, percent):";
        assert_eq!(identifier_column(line, "apply_discount", 0), Some(4));
    }

    #[test]
    fn identifier_column_rejects_substrings_of_larger_identifiers() {
        let line = "    total = apply_discount_v2(x)";
        // `apply_discount` occurs only inside `apply_discount_v2`, which is a
        // different identifier — no whole-word match.
        assert_eq!(identifier_column(line, "apply_discount", 0), None);
        assert_eq!(identifier_column(line, "apply_discount_v2", 0), Some(12));
    }

    /// The code index and `identifier_column` count bytes; LSP positions
    /// count UTF-16 units. `ñ`/`ú` are two bytes but one unit each.
    #[test]
    fn locate_identifier_returns_utf16_columns_on_non_ascii_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(
            &file,
            "// Déjà vu\nfn ñandú_total(a: i32) -> i32 { a + 1 }\nconst É: i32 = 1; fn after_é() {}\n",
        )
        .unwrap();
        let path = file.to_str().unwrap();
        assert_eq!(locate_identifier(path, 1, 0, "ñandú_total"), Some((1, 3)));
        // `after_é` starts at byte 22 (`É` is two bytes) but UTF-16 column 21.
        assert_eq!(locate_identifier(path, 2, 0, "after_é"), Some((2, 21)));
        // The fallback converts the stored byte column the same way.
        assert_eq!(definition_start(path, 2, 22), (2, 21));
    }

    #[test]
    fn utf16_column_counts_code_units() {
        assert_eq!(utf16_column("abc", 2), 2);
        assert_eq!(utf16_column("ñandú x", 7), 5);
        // An emoji is four bytes and two UTF-16 units.
        assert_eq!(utf16_column("😀 x", 5), 3);
        assert_eq!(
            utf16_column("ab", 99),
            2,
            "past the end counts the whole line"
        );
    }

    #[test]
    fn identifier_column_treats_unicode_letters_as_part_of_an_identifier() {
        // `total` inside `ñtotal` is not a standalone identifier.
        assert_eq!(
            identifier_column("let ñtotal = total;", "total", 0),
            Some(14)
        );
        assert_eq!(
            identifier_column("fn ñandú_total()", "ñandú_total", 0),
            Some(3)
        );
    }

    #[test]
    fn identifier_column_honours_the_floor() {
        // Two occurrences; the floor skips the first (a call) to the second.
        let line = "x = foo(); def foo():";
        let first = identifier_column(line, "foo", 0).unwrap();
        let second = identifier_column(line, "foo", first + 1).unwrap();
        assert!(second > first);
    }

    #[test]
    fn path_hint_matches_by_basename_suffix_and_exact() {
        assert!(path_hint_matches("/a/b/pricing.py", "pricing.py"));
        assert!(path_hint_matches("/a/b/pricing.py", "b/pricing.py"));
        assert!(path_hint_matches("/a/b/pricing.py", "/a/b/pricing.py"));
        assert!(!path_hint_matches("/a/b/pricing.py", "cart.py"));
    }

    #[test]
    fn locate_identifier_resolves_a_definition_to_its_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pricing.py");
        std::fs::write(
            &file,
            "def line_total(u, q):\n    return u * q\n\ndef apply_discount(price, percent):\n    return price * (1 - percent / 100)\n",
        )
        .unwrap();
        // The graph stores the definition start: line 3 (0-indexed), column 0 (`def`).
        let (line, character) =
            locate_identifier(file.to_str().unwrap(), 3, 0, "apply_discount").unwrap();
        assert_eq!(line, 3, "same line as the definition");
        assert_eq!(character, 4, "the identifier, past `def `");
    }

    /// The Context Graph stores 1-indexed lines (`start.row + 1`), while LSP is
    /// 0-indexed. The resolver must convert before scanning, or it reads one line
    /// too low, misses the identifier, and returns a dead position.
    #[test]
    fn graph_line_is_one_indexed_and_converts_before_locating() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pricing.py");
        std::fs::write(
            &file,
            "def apply_discount(price, percent):\n    return price\n",
        )
        .unwrap();

        // Stored for a def on 0-indexed line 0.
        let graph_start_line: u32 = 1;
        let zero_indexed = graph_start_line.saturating_sub(1);
        let position = locate_identifier(file.to_str().unwrap(), zero_indexed, 0, "apply_discount");
        assert_eq!(position, Some((0, 4)), "must land on the identifier at 0:4");

        // Without the conversion, scanning starts on the body line and misses it.
        let wrong = locate_identifier(
            file.to_str().unwrap(),
            graph_start_line,
            0,
            "apply_discount",
        );
        assert_eq!(wrong, None, "the un-converted line finds nothing");
    }

    #[test]
    fn position_schema_advertises_name_and_keeps_extras() {
        let schema = position_schema(json!({ "include_declaration": {"type": "boolean"} }));
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("name"));
        assert!(props.contains_key("entity_type"));
        assert!(props.contains_key("line"));
        assert!(props.contains_key("include_declaration"));
        // Nothing is strictly required — the handler enforces name-or-position.
        assert_eq!(schema["required"].as_array().unwrap().len(), 0);
    }

    /// A name resolves to where the symbol is in the file on disk, though the
    /// index last read an older version of it: the language server reads the
    /// disk, and the old line would point it at a blank one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_name_resolves_to_its_place_in_the_file_on_disk() {
        use devscriptor_core::database::DatabaseConfig;
        use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let root = dunce::canonicalize(root).unwrap();
        let file = root.join("app.py");
        std::fs::write(&file, "def helper():\n    return 1\n").unwrap();
        let db = dir.path().join("index.db");
        // A budget the re-index fits in on a slow machine too: what is tested
        // is where the name resolves once the index holds the file on disk.
        let state = crate::server::DevscriptorServerState::with_db_path(db.clone())
            .unwrap()
            .with_freshness_budget(std::time::Duration::from_secs(60));
        CodebasePipeline::with_database(
            PipelineConfig {
                database: DatabaseConfig {
                    path: db.to_string_lossy().into_owned(),
                    ..DatabaseConfig::default()
                },
                skip_embeddings: true,
                ..PipelineConfig::default()
            },
            state.database(),
        )
        .unwrap()
        .analyze_directory(&root)
        .await
        .unwrap();

        std::fs::write(
            &file,
            format!(
                "{}# helper moved down\ndef helper():\n    return 1\n",
                "\n".repeat(10)
            ),
        )
        .unwrap();
        let (path, line, character) = symbol_position_by_name(&state, "helper", None, None)
            .await
            .unwrap();
        assert_eq!(path, file.to_string_lossy());
        assert_eq!((line, character), (11, 4));
    }
}

mod hierarchy_item_tests {
    use super::*;

    /// A tool continuing a hierarchy takes the prepared `item`, spelled out, or
    /// the symbol itself; neither is required on its own.
    #[test]
    fn a_hierarchy_tool_takes_the_item_or_the_symbol() {
        let schema = hierarchy_item_schema("get_call_hierarchy");
        let props = schema["properties"].as_object().unwrap();
        for key in ["item", "name", "file_path", "line", "character"] {
            assert!(props.contains_key(key), "{key}");
        }
        assert_eq!(schema["required"].as_array().unwrap().len(), 0);

        let item = &props["item"];
        assert_eq!(item["required"], json!(["name", "kind", "location"]));
        let location = &item["properties"]["location"];
        assert_eq!(location["required"], json!(["file_path", "range"]));
        assert_eq!(
            location["properties"]["range"]["required"],
            json!(["start", "end"])
        );
    }

    /// A malformed item is answered with the shape it should have had and the
    /// way around copying it.
    #[test]
    fn a_malformed_item_is_answered_with_its_shape() {
        let error = serde_json::from_value::<CallHierarchyItem>(json!({
            "name": "suggestions",
            "kind": "Function",
            "location": {"file_path": "recommender.py", "range": {"start": {"line": 24, "character": 4}}}
        }))
        .unwrap_err();
        let message = invalid_item(&error, "get_call_hierarchy")
            .message
            .to_string();
        assert!(message.contains("missing field `end`"), "{message}");
        assert!(
            message.contains(
                "location: {file_path, range: {start: {line, character}, end: {line, character}}}"
            ),
            "{message}"
        );
        assert!(message.contains("`name`"), "{message}");
    }

    /// Every property of every LSP tool's schema, nested ones included, has an
    /// explicit `type`: some function-calling APIs refuse a property without.
    #[test]
    fn every_property_has_a_type() {
        fn untyped(schema: &serde_json::Value, at: &str, out: &mut Vec<String>) {
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (key, prop) in props {
                    let path = format!("{at}.{key}");
                    if prop.get("type").is_none() {
                        out.push(path.clone());
                    }
                    untyped(prop, &path, out);
                }
            }
        }
        let mut missing = Vec::new();
        for tool in tool_list() {
            let schema = serde_json::Value::Object((*tool.input_schema).clone());
            untyped(&schema, &tool.name, &mut missing);
        }
        assert!(missing.is_empty(), "untyped properties: {missing:?}");
    }

    /// `start_server` declares the `project` its handler reads: undeclared, no
    /// agent passed it, and a server could only start for the workspace.
    #[test]
    fn start_server_declares_its_project() {
        let tools = tool_list();
        let start = tools.iter().find(|t| t.name == "start_server").unwrap();
        let props = start.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("project"));
        assert_eq!(start.input_schema["required"], json!(["language_id"]));
    }

    /// An item passed back without the server's optional `data` is accepted,
    /// for type hierarchies as for call hierarchies.
    #[test]
    fn a_type_hierarchy_item_may_omit_its_data() {
        let item: TypeHierarchyItem = serde_json::from_value(json!({
            "name": "Shape",
            "kind": "Class",
            "location": {
                "file_path": "shapes.ts",
                "range": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 11}}
            }
        }))
        .unwrap();
        assert!(item.data.is_null());
    }
}

mod open_files_tests {
    use super::*;
    use devscriptor_lsp::config::{
        Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
    };

    /// A fake JavaScript language server that appends to the file its first
    /// argument names `open <uri>` for each document opened, and the method of
    /// each request for references or callers, which it answers with none. A
    /// call hierarchy it prepares is one item, of the file asked about.
    const FAKE_SERVER: &str = r#"
import json, sys

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def log(line):
    with open(sys.argv[1], "a") as out:
        out.write(line + "\n")

span = {"start": {"line": 0, "character": 16}, "end": {"line": 0, "character": 32}}
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "textDocumentSync": 1, "referencesProvider": True,
            "callHierarchyProvider": True}}})
    elif method == "textDocument/didOpen":
        log("open " + msg["params"]["textDocument"]["uri"])
    elif method == "textDocument/prepareCallHierarchy":
        uri = msg["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [{"name": "buildOrdersQuery",
            "kind": 12, "uri": uri, "range": span, "selectionRange": span}]})
    elif method in ("textDocument/references", "callHierarchy/incomingCalls"):
        log(method)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": []})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// `cite` with `/` between folders, as the expectations are written: a cite
    /// names the file as this platform spells it.
    fn slashed(cite: &str) -> String {
        cite.replace('\\', "/")
    }

    /// What the fake server logged when `tool` was asked about
    /// `buildOrdersQuery`, of a scanned project where a test file and a Python
    /// script call it and another file does not, and what the tool answered.
    async fn asked(tool: &str) -> (Vec<String>, serde_json::Value) {
        asked_about(tool, "buildOrdersQuery").await
    }

    /// [`asked`] about `name`: `buildOrdersQuery`, or the `ORDERS_LIMIT` the
    /// test file and the Python script read.
    async fn asked_about(tool: &str, name: &str) -> (Vec<String>, serde_json::Value) {
        asked_after(tool, name, |_| {}).await
    }

    /// [`asked_about`], once `edit` has changed the scanned project (its
    /// directory) on disk.
    async fn asked_after(
        tool: &str,
        name: &str,
        edit: impl FnOnce(&std::path::Path),
    ) -> (Vec<String>, serde_json::Value) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("admin");
        std::fs::create_dir_all(&project).unwrap();
        let project = dunce::canonicalize(project).unwrap();
        write(
            &project.join("src/api/orders.js"),
            "export function buildOrdersQuery() {\n  return '/api/orders';\n}\n\
             export function fetchOrders() {\n  return fetch(buildOrdersQuery());\n}\n\
             export const ORDERS_LIMIT = 50;\n",
        );
        write(
            &project.join("test/orders.test.js"),
            "import { buildOrdersQuery, ORDERS_LIMIT } from '../src/api/orders.js';\n\
             test('the query', () => {\n  buildOrdersQuery();\n});\n\
             test('the limit', () => {\n  expect(rows).toBeLessThan(ORDERS_LIMIT);\n});\n",
        );
        write(
            &project.join("scripts/report.py"),
            "def report():\n    return buildOrdersQuery()\n\n\ndef limit():\n    return ORDERS_LIMIT\n",
        );
        write(
            &project.join("src/other.js"),
            "export function unrelated() {\n  return 1;\n}\n",
        );
        let state =
            crate::server::DevscriptorServerState::with_db_path(temp.path().join("index.db"))
                .unwrap();
        let core = Arc::new(crate::CoreModule::new(Arc::new(state)));
        core.handle_tool(
            "scan_codebase",
            Some(json!({"path": project.to_string_lossy()})),
        )
        .await
        .unwrap();
        edit(&project);

        let script = temp.path().join("fake_lsp.py");
        write(&script, FAKE_SERVER);
        let log = temp.path().join("lsp.log");
        let mut translator = Translator::new(Config {
            workspace: WorkspaceConfig::default(),
            servers: vec![LspServerConfig {
                language_id: "javascript".into(),
                command: "python3".into(),
                args: vec![script.display().to_string(), log.display().to_string()],
                env: std::collections::HashMap::new(),
                working_dir: None,
                extensions: vec!["js".into()],
                heuristics: None,
                initialization_options: None,
                settings: ServerSettings::default(),
                configuration: None,
                prerequisites: Vec::new(),
            }],
            lifecycle: LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![project]);
        let translator = Arc::new(translator);
        let server = UnifiedMcpServer::with_lsp(core, Arc::clone(&translator)).unwrap();
        let mut args = serde_json::Map::new();
        args.insert("name".into(), json!(name));
        let answer = server.handle_lsp_tool(tool, args).await.unwrap();
        let answer = serde_json::from_str(&answer.content[0].as_text().unwrap().text).unwrap();
        translator.shutdown().await;
        let lines = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        (lines, answer)
    }

    /// A position names the identifier it is in, its column counted in UTF-16
    /// code units as a language server counts it: past `ö`, one unit, and
    /// `𝔸`, two.
    #[test]
    fn a_position_names_the_identifier_it_is_in() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("orders.js");
        write(&file, "// orders\nconst ö𝔸 = buildOrdersQuery(1);\n");
        let at = |character| identifier_at(&file, 1, character);
        assert_eq!(at(12).as_deref(), Some("buildOrdersQuery"));
        assert_eq!(at(27).as_deref(), Some("buildOrdersQuery"));
        assert_eq!(at(6).as_deref(), Some("ö𝔸"));
        assert_eq!(at(7).as_deref(), Some("ö𝔸"));
        assert_eq!(at(10), None, "`=`, between spaces");
        assert_eq!(identifier_at(&file, 7, 0), None, "no such line");
    }

    /// The files of its language the index says call a function are open in
    /// its server before the server is asked for the function's references or
    /// callers, and no other file is: typescript-language-server, without a
    /// `jsconfig.json`, gives a function's references without a test file's
    /// calls of it until the test file is open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_files_using_a_symbol_are_open_before_its_references_are_asked() {
        for (tool, request) in [
            ("get_references", "textDocument/references"),
            ("get_incoming_calls", "callHierarchy/incomingCalls"),
        ] {
            let (lines, _) = asked(tool).await;
            let test_opened = lines.iter().position(|line| {
                line.starts_with("open ") && line.ends_with("test/orders.test.js")
            });
            let asked = lines.iter().position(|line| line == request);
            assert!(
                test_opened.is_some() && test_opened < asked,
                "{tool}: the test file is open first: {lines:?}"
            );
            assert!(
                !lines
                    .iter()
                    .any(|line| line.ends_with("other.js") || line.ends_with(".py")),
                "{tool}: {lines:?}"
            );
        }
    }

    /// What the server does not see, the index fills in: the fake server
    /// reports no reference to `buildOrdersQuery`, and the answer lists the
    /// calls the index records in JavaScript — in the function's own file and
    /// in the test — and not the Python script's call of a function of the
    /// same name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_references_the_server_did_not_report() {
        let (_, answer) = asked("get_references").await;
        assert_eq!(answer["locations"], json!([]), "{answer}");
        let cites: Vec<String> = answer["index_locations"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_locations: {answer}"))
            .iter()
            .map(|l| slashed(l["cite"].as_str().unwrap()))
            .collect();
        assert!(
            cites.iter().any(|c| c.ends_with("src/api/orders.js:5")),
            "{cites:?}"
        );
        assert!(
            cites.iter().any(|c| c.ends_with("test/orders.test.js:3")),
            "{cites:?}"
        );
        assert!(!cites.iter().any(|c| c.contains("report.py")), "{cites:?}");
        assert!(
            answer["note"]
                .as_str()
                .is_some_and(|n| n.contains("`buildOrdersQuery`"))
        );
    }

    /// A constant's reads the server did not report, the index fills in too:
    /// the test file's read of `ORDERS_LIMIT`, which is open before the server
    /// is asked, and not the Python script's read of a constant of the same
    /// name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_reads_of_a_constant_the_server_did_not_report() {
        let (lines, answer) = asked_about("get_references", "ORDERS_LIMIT").await;
        assert_eq!(answer["locations"], json!([]), "{answer}");
        let cites: Vec<String> = answer["index_locations"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_locations: {answer}"))
            .iter()
            .map(|l| slashed(l["cite"].as_str().unwrap()))
            .collect();
        assert!(
            cites.iter().any(|c| c.ends_with("test/orders.test.js:6")),
            "{cites:?}"
        );
        assert!(!cites.iter().any(|c| c.contains("report.py")), "{cites:?}");
        let opened = lines
            .iter()
            .position(|line| line.starts_with("open ") && line.ends_with("test/orders.test.js"));
        let asked = lines
            .iter()
            .position(|line| line == "textDocument/references");
        assert!(
            opened.is_some() && opened < asked,
            "the test file is open first: {lines:?}"
        );
    }

    /// The calls the index adds are read from the files as they are: a call
    /// in a file deleted since the scan, or edited away, is not added.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_adds_no_call_that_is_gone() {
        let (_, answer) = asked_after("get_incoming_calls", "buildOrdersQuery", |project| {
            std::fs::remove_file(project.join("test/orders.test.js")).unwrap();
        })
        .await;
        let calls: Vec<String> = answer["index_calls"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_calls: {answer}"))
            .iter()
            .map(|c| slashed(c["cite"].as_str().unwrap()))
            .collect();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].ends_with("src/api/orders.js:5"), "{calls:?}");

        let (_, answer) = asked_after("get_references", "buildOrdersQuery", |project| {
            write(
                &project.join("src/api/orders.js"),
                "export function buildOrdersQuery() {\n  return '/api/orders';\n}\n",
            );
        })
        .await;
        let cites: Vec<String> = answer["index_locations"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_locations: {answer}"))
            .iter()
            .map(|l| slashed(l["cite"].as_str().unwrap()))
            .collect();
        assert!(
            !cites.iter().any(|c| c.ends_with("src/api/orders.js:5")),
            "{cites:?}"
        );
        assert!(
            cites.iter().any(|c| c.ends_with("test/orders.test.js:3")),
            "{cites:?}"
        );
    }

    /// The callers too: the fake server reports no call of
    /// `buildOrdersQuery`, and the answer lists the JavaScript calls the index
    /// records, each with what it is made in, and not the Python script's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_index_fills_in_the_callers_the_server_did_not_report() {
        let (_, answer) = asked("get_incoming_calls").await;
        assert_eq!(answer["calls"], json!([]), "{answer}");
        let calls: Vec<(&str, String)> = answer["index_calls"]
            .as_array()
            .unwrap_or_else(|| panic!("no index_calls: {answer}"))
            .iter()
            .map(|c| {
                (
                    c["caller"].as_str().unwrap(),
                    slashed(c["cite"].as_str().unwrap()),
                )
            })
            .collect();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[0].0 == "fetchOrders" && calls[0].1.ends_with("src/api/orders.js:5"),
            "{calls:?}"
        );
        assert!(
            calls[1].0 == "orders.test" && calls[1].1.ends_with("test/orders.test.js:3"),
            "{calls:?}"
        );
        assert!(
            answer["note"]
                .as_str()
                .is_some_and(|n| n.contains("call(s) of `buildOrdersQuery`"))
        );
    }
}

mod argument_tests {
    use super::*;
    use devscriptor_lsp::config::{
        Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
    };

    /// A fake language server that answers `workspace/symbol`, as gopls does,
    /// with the symbols of the files it has been sent: two in
    /// `services/a/lib.fake`, one each in `services/ab` and `services/b`, of
    /// the workspace its first argument names.
    const SYMBOL_SERVER: &str = r#"
import json, pathlib, sys

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def symbol(name, path):
    span = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}
    uri = pathlib.Path(sys.argv[1], path).as_uri()
    return {"name": name, "kind": 12, "location": {"uri": uri, "range": span}}

opened = set()
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "workspaceSymbolProvider": True, "textDocumentSync": 1}}})
    elif method == "textDocument/didOpen":
        opened.add(msg["params"]["textDocument"]["uri"])
    elif method == "workspace/symbol":
        every = [symbol("total_a", "services/a/lib.fake"),
                 symbol("total_a2", "services/a/lib.fake"),
                 symbol("total_ab", "services/ab/lib.fake"),
                 symbol("total_b", "services/b/lib.fake")]
        send({"jsonrpc": "2.0", "id": msg["id"],
              "result": [s for s in every if s["location"]["uri"] in opened]})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A server whose code index lives in `scratch`, for `workspace`, its
    /// language servers those of `servers`.
    fn serve(
        scratch: &std::path::Path,
        workspace: &std::path::Path,
        servers: Vec<LspServerConfig>,
    ) -> (UnifiedMcpServer, Arc<Translator>) {
        let state =
            crate::server::DevscriptorServerState::with_db_path(scratch.join("index.db")).unwrap();
        let core = Arc::new(crate::CoreModule::new(Arc::new(state)));
        let mut translator = Translator::new(Config {
            workspace: WorkspaceConfig::default(),
            servers,
            lifecycle: LifecycleConfig::default(),
        });
        translator.set_workspace_roots(vec![workspace.to_path_buf()]);
        let translator = Arc::new(translator);
        let server = UnifiedMcpServer::with_lsp(core, Arc::clone(&translator)).unwrap();
        (server, translator)
    }

    fn server_config(language_id: &str, command: &str, args: Vec<String>) -> LspServerConfig {
        LspServerConfig {
            language_id: language_id.into(),
            command: command.into(),
            args,
            env: std::collections::HashMap::new(),
            working_dir: None,
            extensions: vec![language_id.into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
            prerequisites: Vec::new(),
        }
    }

    /// `tool`'s answer to `args`, as the agent reads it, or its error.
    async fn call(
        server: &UnifiedMcpServer,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let args = args.as_object().unwrap().clone();
        match server.handle_lsp_tool(tool, args).await {
            Ok(result) => {
                Ok(serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap())
            }
            Err(error) => Err(error.message.to_string()),
        }
    }

    /// `get_project_files` lists the project its schema's `project` names,
    /// relative to the workspace root, or the workspace root without one,
    /// sorted and at any depth: not `.git`'s files, hidden ones, or what
    /// `.gitignore` leaves out.
    #[tokio::test]
    async fn get_project_files_lists_the_project_it_is_named_by() {
        let scratch = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace = dunce::canonicalize(workspace.path()).unwrap();
        write(&workspace.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&workspace.join(".gitignore"), "build/\n");
        write(&workspace.join(".env"), "TOKEN=1\n");
        write(&workspace.join("build/out.o"), "");
        // Neither in order nor in reverse, as a directory may list them.
        write(&workspace.join("docs/notes.md"), "# Notes\n");
        write(&workspace.join("a/b/c/d/e/f/g/h/i/j/k/deep.txt"), "deep\n");
        write(&workspace.join("src/main.rs"), "fn main() {}\n");
        let (server, translator) = serve(scratch.path(), &workspace, vec![]);

        let all = call(&server, "get_project_files", json!({"project": "."}))
            .await
            .unwrap();
        let listed = |page: &serde_json::Value| -> Vec<String> {
            page["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f.as_str().unwrap().to_string())
                .collect()
        };
        // Joined part by part: the listed paths have this platform's separators.
        let inside = |path: &str| {
            path.split('/')
                .fold(workspace.clone(), |dir, part| dir.join(part))
                .display()
                .to_string()
        };
        assert_eq!(
            listed(&all),
            [
                inside("a/b/c/d/e/f/g/h/i/j/k/deep.txt"),
                inside("docs/notes.md"),
                inside("src/main.rs"),
            ]
        );
        let unnamed = call(&server, "get_project_files", json!({})).await.unwrap();
        assert_eq!(listed(&unnamed), listed(&all));

        let page = call(
            &server,
            "get_project_files",
            json!({"project": workspace.join("docs").display().to_string(), "limit": 1}),
        )
        .await
        .unwrap();
        assert_eq!(listed(&page), [inside("docs/notes.md")]);

        let missing = call(&server, "get_project_files", json!({"project": "gone"}))
            .await
            .unwrap_err();
        assert!(
            missing.contains(&format!(
                "no project directory {}",
                workspace.join("gone").display()
            )),
            "{missing}"
        );
        translator.shutdown().await;
    }

    /// The server-management tools read what their schemas offer:
    /// `get_project_symbols` opens the files of `project` — a server like
    /// gopls searches only the projects of open files — keeps to it, and
    /// pages by `offset`,
    /// `get_server_projects` gives the folders of `language_id`'s server, and
    /// `get_server_status` answers for `language_id`, and says so of one no
    /// server is configured for rather than calling it stopped.
    #[tokio::test]
    async fn the_server_tools_read_what_their_schemas_offer() {
        let scratch = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace = dunce::canonicalize(workspace.path()).unwrap();
        let script = scratch.path().join("symbols.py");
        write(&script, SYMBOL_SERVER);
        for service in ["a", "ab", "b"] {
            write(
                &workspace.join(format!("services/{service}/lib.fake")),
                "total\n",
            );
        }
        let fake = server_config(
            "fake",
            "python3",
            vec![
                script.display().to_string(),
                workspace.display().to_string(),
            ],
        );
        let idle = server_config("other", "devscriptor-test-no-such-server", vec![]);
        let (server, translator) = serve(scratch.path(), &workspace, vec![fake, idle]);
        let names = |answer: &serde_json::Value| -> Vec<String> {
            answer["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_string())
                .collect()
        };

        let unopened = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(unopened["pagination"]["total"], 0, "no file is open yet");
        let ask = json!({"language_id": "fake", "project": "services/ab"});
        let found = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&found), ["total_ab"]);
        let ask = json!({"language_id": "fake", "project": "services/a", "query": "total"});
        let found = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&found), ["total_a", "total_a2"]);
        let ask = json!({"language_id": "fake", "project": "services/a", "offset": 1});
        let page = call(&server, "get_project_symbols", ask).await.unwrap();
        assert_eq!(names(&page), ["total_a2"]);
        assert_eq!(page["pagination"]["total"], 2);
        let opened = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(opened["pagination"]["total"], 3);
        let missing = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "fake", "project": "services/c"}),
        )
        .await
        .unwrap_err();
        assert!(missing.contains("no project directory"), "{missing}");
        let stopped = call(
            &server,
            "get_project_symbols",
            json!({"language_id": "other"}),
        )
        .await
        .unwrap_err();
        assert_eq!(stopped, "the other language server is not running");

        let folders = call(
            &server,
            "get_server_projects",
            json!({"language_id": "fake"}),
        )
        .await
        .unwrap();
        assert_eq!(
            folders["projects"],
            json!([workspace.display().to_string()])
        );
        let idle_folders = call(
            &server,
            "get_server_projects",
            json!({"language_id": "other"}),
        )
        .await
        .unwrap_err();
        assert!(idle_folders.contains("not running"), "{idle_folders}");

        let one = call(&server, "get_server_status", json!({"language_id": "fake"}))
            .await
            .unwrap();
        assert_eq!(one["servers"].as_array().unwrap().len(), 1);
        assert_eq!(one["servers"][0]["state"], "running");
        let all = call(&server, "get_server_status", json!({})).await.unwrap();
        assert_eq!(all["servers"].as_array().unwrap().len(), 2);
        let unknown = call(&server, "get_server_status", json!({"language_id": "dart"}))
            .await
            .unwrap_err();
        assert!(
            unknown.contains("No LSP server configured for language: dart"),
            "{unknown}"
        );
        translator.shutdown().await;
    }

    /// Every argument these tools read is in their schemas, under the name
    /// they read it by.
    #[test]
    fn the_schemas_offer_what_the_server_tools_read() {
        let tools = tool_list();
        let offered = |name: &str| -> Vec<String> {
            let tool = tools.iter().find(|t| t.name == name).unwrap();
            let mut properties: Vec<String> = tool.input_schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            properties.sort();
            properties
        };
        assert_eq!(offered("get_project_files"), ["limit", "offset", "project"]);
        assert_eq!(
            offered("get_project_symbols"),
            ["language_id", "limit", "offset", "project", "query"]
        );
        assert_eq!(offered("get_symbols"), ["file_path", "limit", "offset"]);
        assert_eq!(offered("get_server_status"), ["language_id"]);
        assert_eq!(offered("get_server_projects"), ["language_id"]);
        assert_eq!(offered("restart_server"), ["language_id"]);
    }
}
