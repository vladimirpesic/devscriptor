//! `devscriptor mcp` starts language servers lazily unless told otherwise.
//!
//! The startup mode comes from `--lsp-startup`, then `DEVSCRIPTOR_LSP_STARTUP`,
//! then the configuration's `[lifecycle]` table; lazy is the default. A fake
//! Python language server, configured through `DEVSCRIPTOR_LSP_CONFIG`,
//! applies to the workspace, so an eager start runs it before the first
//! request, and `lsp_arch_detect` starts it in the background.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Answers `initialize` and nothing else of interest.
const SERVER: &str = r#"
import sys, json

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

while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        if msg.get("method") == "exit":
            break
        continue
    if msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// A workspace with a Python file, and a configuration naming the fake
/// `python` server, with `lifecycle` as its `[lifecycle]` table.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(lifecycle: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("main.py"), "thing = 1\n").unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        std::fs::write(
            dir.path().join("lsp.toml"),
            format!(
                "[workspace]\nroots = [\".\"]\n\n\
                 [[lsp_servers]]\nlanguage_id = \"python\"\ncommand = \"python3\"\n\
                 args = [{script:?}]\nextensions = [\"py\"]\n\n{lifecycle}",
                script = script.to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir }
    }

    /// `devscriptor mcp <args>` in the workspace, with `env` set.
    fn spawn(&self, args: &[&str], env: &[(&str, &str)]) -> Server {
        let root = self.dir.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_devscriptor"));
        command
            .arg("--database")
            .arg(root.join("index.db"))
            .arg("mcp")
            .args(args)
            .current_dir(root.join("workspace"))
            .env("DEVSCRIPTOR_LSP_CONFIG", root.join("lsp.toml"))
            .env_remove("DEVSCRIPTOR_LSP_STARTUP")
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            // The server under test runs in this process, not in a daemon.
            .env("DEVSCRIPTOR_DAEMON", "0")
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn devscriptor mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut server = Server {
            child,
            stdin: Some(stdin),
            stdout,
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
            None => content["json"].clone(),
        }
    }

    /// The `state` and `stop_reason` of the `python` server.
    fn python(&mut self) -> (String, Option<String>) {
        let status = self.call("get_server_status", json!({"language_id": "python"}));
        let server = &status["servers"][0];
        (
            server["state"].as_str().expect("a state").to_string(),
            server["stop_reason"].as_str().map(ToString::to_string),
        )
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

fn not_started() -> (String, Option<String>) {
    ("stopped".into(), Some("not started".into()))
}

fn running() -> (String, Option<String>) {
    ("running".into(), None)
}

#[test]
fn servers_start_lazily_by_default() {
    let fixture = Fixture::new("");
    assert_eq!(fixture.spawn(&[], &[]).python(), not_started());
}

#[test]
fn eager_startup_runs_the_applicable_servers_before_the_first_request() {
    let fixture = Fixture::new("");
    assert_eq!(
        fixture.spawn(&["--lsp-startup", "eager"], &[]).python(),
        running()
    );
}

/// `lsp_arch_detect` names the servers it starts, and they come up without
/// another request.
#[test]
fn lsp_arch_detect_starts_the_detected_servers_in_the_background() {
    let fixture = Fixture::new("");
    let workspace = fixture.dir.path().join("workspace");
    let mut server = fixture.spawn(&[], &[]);
    assert_eq!(server.python(), not_started());

    let detected = server.call(
        "lsp_arch_detect",
        json!({"project_path": workspace.to_string_lossy()}),
    );
    assert_eq!(detected["prewarming"], json!(["python"]), "{detected}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while server.python() != running() {
        assert!(
            std::time::Instant::now() < deadline,
            "the server never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Once it runs, nothing is left to start.
    let again = server.call(
        "lsp_arch_detect",
        json!({"project_path": workspace.to_string_lossy()}),
    );
    assert_eq!(again["prewarming"], json!([]), "{again}");
}

/// Eager startup started everything already; detection starts nothing.
#[test]
fn lsp_arch_detect_starts_nothing_after_an_eager_start() {
    let fixture = Fixture::new("");
    let workspace = fixture.dir.path().join("workspace");
    let mut server = fixture.spawn(&["--lsp-startup", "eager"], &[]);
    let detected = server.call(
        "lsp_arch_detect",
        json!({"project_path": workspace.to_string_lossy()}),
    );
    assert_eq!(detected["prewarming"], json!([]), "{detected}");
}

/// The flag, then the environment, then the file.
#[test]
fn the_flag_outranks_the_environment_which_outranks_the_file() {
    let eager_file = Fixture::new("[lifecycle]\nstartup = \"eager\"\n");
    assert_eq!(eager_file.spawn(&[], &[]).python(), running());
    assert_eq!(
        eager_file
            .spawn(&[], &[("DEVSCRIPTOR_LSP_STARTUP", "lazy")])
            .python(),
        not_started()
    );
    assert_eq!(
        eager_file
            .spawn(
                &["--lsp-startup", "eager"],
                &[("DEVSCRIPTOR_LSP_STARTUP", "lazy")]
            )
            .python(),
        running()
    );
}
