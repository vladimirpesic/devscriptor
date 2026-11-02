//! The MCP server must start on a machine with no language servers installed,
//! and must never write anything but JSON-RPC to stdout.
//!
//! 1. With eager startup, `Translator::spawn_applicable` returns
//!    `Err(AllServersFailedToInit)` when every *applicable* language server fails
//!    to spawn — e.g. a Rust project on a machine without `rust-analyzer`. The
//!    server must still serve the 32 core tools and the 5 architecture tools,
//!    among them `lsp_arch_setup`, which installs the missing server. The
//!    servers here start eagerly, the mode in which that error arises; lazy
//!    startup (the default) starts none.
//!
//! 2. `tracing_subscriber::fmt()` defaults to stdout, which is the JSON-RPC
//!    channel. A single log line there breaks strict MCP clients with
//!    `serde error expected value at line 1 column 1`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// A workspace whose language (Rust) is recognised, so an LSP server *is*
/// applicable, and fails to spawn where none is installed.
fn rust_workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    dir
}

struct Server {
    child: Child,
    /// Closed when the server is dropped, so that it exits by itself.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    /// Spawn `devscriptor mcp --lsp-startup <startup>` where no tier finds a
    /// language server.
    fn spawn(cwd: &std::path::Path, data_home: &std::path::Path, startup: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
            .args(["mcp", "--lsp-startup", startup])
            .current_dir(cwd)
            // Deliberately exclude ~/.cargo/bin etc. so `rust-analyzer` cannot resolve.
            .env("PATH", "/usr/bin:/bin")
            // Nor from the developer's installed servers or VS Code extensions: a
            // real rust-analyzer started here, and killed with the server, left a
            // `/tmp/rust-analyzer<pid>-0` behind.
            .env("HOME", data_home)
            .env("XDG_CACHE_HOME", data_home.join("cache"))
            // Keep the test's database out of the developer's real one.
            .env("XDG_DATA_HOME", data_home)
            // The server under test runs in this process, not in a daemon.
            .env("DEVSCRIPTOR_DAEMON", "0")
            .env("XDG_RUNTIME_DIR", data_home.join("run"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn devscriptor mcp");

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 0,
        }
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().expect("the server's input is open");
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    /// Read the next stdout line and require it to be valid JSON.
    ///
    /// This is the stdout-purity assertion: a tracing line or a banner here fails
    /// the test exactly as it would break a real MCP client.
    fn read_message(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read stdout");
        assert!(read > 0, "server closed stdout — it died at startup");
        serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!("stdout must carry only JSON-RPC, got {line:?} ({e}). Logs belong on stderr.")
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = self.read_message();
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    /// Send `count` calls of `name` before reading any answer, as a client
    /// that fans calls out does, and return the answers.
    fn call_tool_burst(&mut self, name: &str, arguments: &Value, count: usize) -> Vec<Value> {
        for _ in 0..count {
            self.next_id += 1;
            let id = self.next_id;
            self.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }));
        }
        let mut answers = Vec::with_capacity(count);
        while answers.len() < count {
            let msg = self.read_message();
            if msg.get("id").is_some() {
                answers.push(msg);
            }
        }
        answers
    }

    fn initialize(&mut self) {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }
}

impl Drop for Server {
    /// Close the server's input, so that it exits as when its client leaves:
    /// it stops its language servers, whose temporary directories go with
    /// them. Killed if it has not exited within ten seconds.
    fn drop(&mut self) {
        drop(self.stdin.take());
        for _ in 0..100 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_server_starts_and_serves_all_75_tools_without_any_language_server() {
    for startup in ["eager", "lazy"] {
        let workspace = rust_workspace();
        let data_home = TempDir::new().unwrap();
        let mut server = Server::spawn(workspace.path(), data_home.path(), startup);

        server.initialize();

        let tools = server.request("tools/list", json!({}));
        let tools = tools["result"]["tools"]
            .as_array()
            .expect("tools/list must return a list");
        assert_eq!(
            tools.len(),
            75,
            "the server must advertise all 75 tools even with no language server running \
             ({startup} startup)"
        );

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for expected in [
            "health_check",
            "get_hover",
            "lsp_arch_setup",
            "start_watching",
            // Advertised *and* routable: a tool missing from `unified_server::CORE_TOOLS`
            // still appears here but fails at call time with "Unknown tool".
            "generate_embeddings",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }

        // Every tool says what it does to its environment, so a client can
        // run the read-only ones without asking.
        for tool in tools {
            let annotations = &tool["annotations"];
            assert!(
                annotations["title"].is_string() && annotations["readOnlyHint"].is_boolean(),
                "{} has no annotations: {tool}",
                tool["name"]
            );
        }
        let annotations_of = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .map(|t| t["annotations"].clone())
                .unwrap()
        };
        assert_eq!(annotations_of("semantic_search")["readOnlyHint"], true);
        let rename = annotations_of("rename_symbol");
        assert_eq!(rename["readOnlyHint"], false);
        assert_eq!(rename["destructiveHint"], true);
    }
}

#[test]
fn core_and_architecture_tools_work_without_any_language_server() {
    let workspace = rust_workspace();
    let data_home = TempDir::new().unwrap();
    let mut server = Server::spawn(workspace.path(), data_home.path(), "eager");
    server.initialize();

    let health = server.call_tool("health_check", json!({}));
    assert!(
        health.get("error").is_none(),
        "health_check must work: {health}"
    );

    // `lsp_arch_status` is how an agent discovers that servers are missing, and
    // `lsp_arch_setup` is how it installs them — neither may depend on a running
    // language server.
    let status = server.call_tool("lsp_arch_status", json!({}));
    assert!(
        status.get("error").is_none(),
        "lsp_arch_status must work: {status}"
    );

    // A tool registered in TOOL_IDS and the schema list but missing from
    // `unified_server::CORE_TOOLS` is advertised yet fails at call time with
    // "Unknown tool". Advertising alone is not enough — call it. On an unindexed
    // workspace this returns immediately (nothing to embed) without loading the
    // ONNX model, so it stays cheap.
    let embeddings = server.call_tool("generate_embeddings", json!({}));
    assert!(
        embeddings.get("error").is_none(),
        "generate_embeddings must be routable, not just advertised: {embeddings}"
    );
}

/// An LSP tool call must be survivable, whatever its outcome.
///
/// No language server resolves here, so `get_hover` fails; with one, it would
/// answer. The invariant worth pinning is that either way the answer is a
/// well-formed JSON-RPC message and the MCP server keeps serving.
#[test]
fn lsp_tool_calls_never_kill_the_server() {
    let workspace = rust_workspace();
    let data_home = TempDir::new().unwrap();
    let mut server = Server::spawn(workspace.path(), data_home.path(), "eager");
    server.initialize();

    let file = workspace.path().join("main.rs");
    let hover = server.call_tool(
        "get_hover",
        json!({"file_path": file.to_string_lossy(), "line": 0, "character": 3}),
    );

    // Exactly one of `error` / `result` — a per-call outcome, never a crash.
    assert!(
        hover.get("error").is_some() ^ hover.get("result").is_some(),
        "get_hover must answer with either an error or a result: {hover}"
    );

    // And the server must still answer afterwards.
    let stats = server.call_tool("get_context_statistics", json!({}));
    assert!(
        stats.get("error").is_none(),
        "server must survive an LSP call: {stats}"
    );
}

/// A call that runs and fails is the tool's answer, a result with `isError`
/// the model reads, as MCP has a tool report it: a missing argument, a name
/// the index does not know. Only a tool the server does not have is a
/// protocol error; none of these is a JSON-RPC internal error.
#[test]
fn a_failed_call_is_an_error_result_and_an_unknown_tool_a_protocol_error() {
    let workspace = rust_workspace();
    let data_home = TempDir::new().unwrap();
    let mut server = Server::spawn(workspace.path(), data_home.path(), "lazy");
    server.initialize();
    let text = |response: &Value| {
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let missing = server.call_tool("validate_file_edit", json!({}));
    assert!(missing.get("error").is_none(), "{missing}");
    assert_eq!(missing["result"]["isError"], true, "{missing}");
    assert!(text(&missing).contains("file_path"), "{missing}");

    let unknown_name = server.call_tool("get_hover", json!({"name": "no_symbol_by_this_name"}));
    assert!(unknown_name.get("error").is_none(), "{unknown_name}");
    assert_eq!(unknown_name["result"]["isError"], true, "{unknown_name}");
    assert!(
        text(&unknown_name).contains("no_symbol_by_this_name"),
        "{unknown_name}"
    );

    let no_such_tool = server.call_tool("no_such_tool", json!({}));
    assert_eq!(no_such_tool["error"]["code"], -32602, "{no_such_tool}");

    // Sixty calls at once overrun the tool's limit (a burst of 20, then 10 a
    // second); the calls turned away are failed calls that say when to retry.
    let burst = server.call_tool_burst("validate_file_edit", &json!({}), 60);
    assert!(burst.iter().all(|answer| answer.get("error").is_none()));
    let throttled: Vec<&Value> = burst
        .iter()
        .filter(|answer| text(answer).contains("Rate limit exceeded"))
        .collect();
    assert!(!throttled.is_empty(), "no call was throttled");
    for answer in throttled {
        assert_eq!(answer["result"]["isError"], true, "{answer}");
        assert!(text(answer).contains("retry after 1s"), "{answer}");
    }
}

/// A server is checked by its id or its language, and a name that is
/// neither is said to be unknown — not reported as a server that is not
/// installed, which sent a model to install one.
#[test]
fn a_server_check_takes_a_language_and_names_what_it_does_not_know() {
    let workspace = rust_workspace();
    let data_home = TempDir::new().unwrap();
    let mut server = Server::spawn(workspace.path(), data_home.path(), "lazy");
    server.initialize();
    let payload = |response: Value| -> Value {
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };

    let batch = payload(server.call_tool(
        "lsp_arch_batch_check",
        json!({"server_ids": ["dart", "rust-analyzer", "cobol"]}),
    ));
    let results = batch["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "{batch}");
    assert_eq!(results[0]["server_id"], "dart-ls", "{batch}");
    assert_eq!(results[0]["requested"], "dart", "{batch}");
    assert!(results[1].get("requested").is_none(), "{batch}");
    assert_eq!(batch["unknown"]["names"], json!(["cobol"]), "{batch}");
    assert!(
        batch["unknown"]["server_ids"]
            .as_array()
            .unwrap()
            .contains(&json!("dart-ls")),
        "{batch}"
    );

    let single = payload(server.call_tool("lsp_arch_check", json!({"server_id": "cobol"})));
    assert_eq!(single["available"], false);
    assert!(
        single["unknown"]
            .as_str()
            .unwrap()
            .contains("rust-analyzer"),
        "{single}"
    );
    let by_language = payload(server.call_tool("lsp_arch_check", json!({"server_id": "rust"})));
    assert_eq!(by_language["server_id"], "rust-analyzer", "{by_language}");
    assert_eq!(by_language["requested"], "rust", "{by_language}");
}
