//! Answers from the index follow the files on disk.
//!
//! Without a watcher, or before one catches up, the index can still hold an
//! edited function's old lines and body, and the functions of a deleted file.
//! Each answer therefore checks the files it names against the disk first,
//! re-indexes those that changed within a time budget, and names the files it
//! refreshed (`refreshed_files`) and those that may be newer than the answer
//! (`stale_files`).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Where nothing listens: the embedding model cannot be fetched, so searches
/// rank by keywords alone instead of downloading it.
const NO_SERVER: &str = "http://127.0.0.1:9";

const BILLING: &str = "def compute_total(items):\n    return sum(items)\n";

/// A project scanned into a database of its own, and never watched.
struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("project/src");
        std::fs::create_dir_all(src.join("generated")).unwrap();
        std::fs::write(src.join("billing.py"), BILLING).unwrap();
        std::fs::write(src.join("legacy.py"), "def legacy_rate():\n    return 3\n").unwrap();
        std::fs::write(
            src.join("generated/stub.py"),
            "def generated_stub():\n    return 0\n",
        )
        .unwrap();
        // An ignore rule outside a git repository: `.ignore`, not `.gitignore`.
        std::fs::write(dir.path().join("project/.ignore"), "src/generated/\n").unwrap();
        // A file a scan of the project never sees.
        std::fs::write(
            dir.path().join("elsewhere.py"),
            "def elsewhere_function():\n    return 1\n",
        )
        .unwrap();
        let project = Self { dir };
        let status = project
            .command()
            .args(["scan", "--skip-embeddings"])
            .arg(project.root())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the scan failed");
        project
    }

    fn root(&self) -> PathBuf {
        dunce::canonicalize(self.dir.path().join("project")).unwrap()
    }

    /// The path of `name` in the project's `src`, as the index stores it.
    fn file(&self, name: &str) -> String {
        self.root()
            .join("src")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    /// `devscriptor --database <db>`, isolated from the user's data.
    fn command(&self) -> Command {
        let base = self.dir.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_devscriptor"));
        command
            .arg("--database")
            .arg(base.join("index.db"))
            .env("XDG_DATA_HOME", base.join("data"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            // The server under test runs in this process, not in a daemon.
            .env("DEVSCRIPTOR_DAEMON", "0")
            .env("XDG_RUNTIME_DIR", base.join("run"))
            .env("DEVSCRIPTOR_HF_BASE_URL", NO_SERVER)
            .env("DEVSCRIPTOR_MODEL_MIRROR_URL", NO_SERVER)
            // A budget the re-index fits in on a slow machine too; the test of
            // an answer over the budget sets its own.
            .env("DEVSCRIPTOR_FRESHNESS_BUDGET_MS", "60000")
            .env_remove("DEVSCRIPTOR_LSP_STARTUP");
        command
    }

    /// `devscriptor mcp --no-watch` in the project, with `env` set.
    fn serve(&self, env: &[(&str, &str)]) -> Server {
        let mut command = self.command();
        command
            .args(["mcp", "--no-watch"])
            .current_dir(self.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let mut server = Server {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 0,
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }
}

struct Server {
    child: Child,
    /// Closed when the server is dropped, so that it exits by itself.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("the server's input is open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "the server closed stdout"
            );
            let message: Value = serde_json::from_str(&line).expect("JSON-RPC on stdout");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// The JSON result of calling the tool `name`.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let content = &response["result"]["content"][0];
        match content["text"].as_str() {
            Some(text) => serde_json::from_str(text).expect("a JSON result"),
            None => panic!("{name} returned no result: {response}"),
        }
    }

    /// The function named `name` that `search_functions` returns, and the
    /// whole answer.
    fn function(&mut self, name: &str) -> (Option<Value>, Value) {
        let answer = self.call("search_functions", json!({"query": name}));
        let found = answer["functions"]
            .as_array()
            .expect("a list of functions")
            .iter()
            .find(|function| function["name"] == name)
            .cloned();
        (found, answer)
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
fn an_edit_is_in_the_next_answer_without_a_watcher() {
    let project = Project::new();
    let billing = project.file("billing.py");
    let mut server = project.serve(&[]);

    let (found, answer) = server.function("compute_total");
    let found = found.unwrap_or_else(|| panic!("compute_total is indexed: {answer}"));
    assert_eq!(
        (found["start_line"].clone(), found["end_line"].clone()),
        (json!(1), json!(2))
    );
    assert!(answer.get("refreshed_files").is_none(), "{answer}");

    std::fs::write(
        &billing,
        "import math\n\n\ndef compute_total(items):\n    subtotal = sum(items)\n    \
         return math.ceil(subtotal * 1.2)\n",
    )
    .unwrap();
    let (found, answer) = server.function("compute_total");
    let found = found.unwrap_or_else(|| panic!("compute_total is still found: {answer}"));
    assert_eq!(
        (found["start_line"].clone(), found["end_line"].clone()),
        (json!(4), json!(6)),
        "the lines of the edited function: {answer}"
    );
    assert_eq!(answer["refreshed_files"], json!([billing]), "{answer}");
    assert!(answer.get("stale_files").is_none(), "{answer}");

    std::fs::write(
        &billing,
        "def compute_total(items, rate):\n    taxed = [item * rate for item in items]\n    \
         return round(sum(taxed), 2)\n",
    )
    .unwrap();
    let answer = server.call("semantic_search", json!({"query": "compute_total"}));
    let found = answer["results"]
        .as_array()
        .expect("a list of results")
        .iter()
        .find(|result| result["name"] == "compute_total")
        .unwrap_or_else(|| panic!("compute_total is found: {answer}"));
    assert_eq!(
        (found["start_line"].clone(), found["end_line"].clone()),
        (json!(1), json!(3)),
        "{answer}"
    );
    let preview = found["body_preview"].as_str().unwrap_or_default();
    assert!(
        preview.contains("2\t    taxed = [item * rate for item in items]"),
        "the preview shows the new body, numbered as in the file: {answer}"
    );
    assert_eq!(answer["refreshed_files"], json!([billing]), "{answer}");

    // Nothing changed since: nothing to report.
    let (_, answer) = server.function("compute_total");
    assert!(answer.get("refreshed_files").is_none(), "{answer}");
}

#[test]
fn a_deleted_file_leaves_the_next_answer() {
    let project = Project::new();
    let legacy = project.file("legacy.py");
    let mut server = project.serve(&[]);

    let (found, answer) = server.function("legacy_rate");
    assert!(found.is_some(), "legacy_rate is indexed: {answer}");

    std::fs::remove_file(&legacy).unwrap();
    let (found, answer) = server.function("legacy_rate");
    assert!(
        found.is_none(),
        "the deleted file's function is gone: {answer}"
    );
    assert_eq!(answer["refreshed_files"], json!([legacy]), "{answer}");

    let entities = server.call("search_entities", json!({"query": "legacy_rate"}));
    assert_eq!(entities["total_count"], 0, "{entities}");
}

#[test]
fn over_the_budget_the_answer_names_its_stale_files() {
    let project = Project::new();
    let billing = project.file("billing.py");
    let mut server = project.serve(&[("DEVSCRIPTOR_FRESHNESS_BUDGET_MS", "1")]);

    std::fs::write(
        &billing,
        "\n\ndef compute_total(items):\n    return sum(items) + 1\n",
    )
    .unwrap();
    let (found, answer) = server.function("compute_total");
    assert!(found.is_some(), "the answer is still given: {answer}");
    assert_eq!(answer["stale_files"], json!([billing]), "{answer}");
    assert!(answer.get("refreshed_files").is_none(), "{answer}");

    // The re-index goes on in the background, and a later answer is current.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (found, answer) = server.function("compute_total");
        let current =
            found.is_some_and(|f| f["start_line"] == 3) && answer.get("stale_files").is_none();
        if current {
            break;
        }
        assert!(Instant::now() < deadline, "never current: {answer}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn only_files_a_scan_would_index_are_indexed() {
    let project = Project::new();
    let mut server = project.serve(&[]);

    // A file created after the scan is indexed when an answer names it.
    let fresh = project.file("fresh.py");
    std::fs::write(&fresh, "def fresh_function():\n    return 2\n").unwrap();
    let described = server.call("describe_file", json!({"file_path": fresh}));
    assert!(
        described.to_string().contains("fresh_function"),
        "the new file's function is described: {described}"
    );
    assert_eq!(described["refreshed_files"], json!([fresh]), "{described}");

    // One the ignore rules exclude, or outside the scanned root, is not.
    let ignored = project.file("generated/stub.py");
    let outside = dunce::canonicalize(project.dir.path())
        .unwrap()
        .join("elsewhere.py")
        .to_string_lossy()
        .into_owned();
    for (path, function) in [(ignored, "generated_stub"), (outside, "elsewhere_function")] {
        let context = server.call("get_file_context", json!({"file_path": path}));
        assert!(
            context.get("refreshed_files").is_none() && context.get("stale_files").is_none(),
            "{path}: {context}"
        );
        let (found, answer) = server.function(function);
        assert!(found.is_none(), "{path} was indexed: {answer}");
    }
}
