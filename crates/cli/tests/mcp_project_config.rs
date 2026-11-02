//! `devscriptor mcp` follows the workspace's `devscriptor.yaml`: its scan
//! tools take the file's scope, `health_check` names the file and any error in
//! it, and an edit takes effect at the next tool call, without a restart. An
//! edit that breaks the file keeps the settings it had.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use devscriptor_core::database::{Database, DatabaseConfig};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Server {
    child: Child,
    /// Closed when the server is dropped, so that it exits by itself.
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    /// `devscriptor mcp --no-watch` in `workspace`, in this process (no
    /// daemon), isolated from the user's data.
    fn start(base: &Path, workspace: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
            .arg("--database")
            .arg(base.join("index.db"))
            .args(["mcp", "--no-watch"])
            .current_dir(workspace)
            .env("XDG_DATA_HOME", base.join("data"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env("XDG_RUNTIME_DIR", base.join("run"))
            .env("DEVSCRIPTOR_DAEMON", "0")
            .env_remove("DEVSCRIPTOR_EMBEDDING_SELECTION")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self {
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
                "server exited"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    }

    /// The JSON a tool answers with.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: {reply}"));
        serde_json::from_str(text).unwrap()
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

/// Whether the index under `base` holds `file` of `root`.
async fn indexed(base: &Path, root: &Path, file: &str) -> bool {
    let database = Database::new(DatabaseConfig {
        path: base.join("index.db").to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .unwrap();
    database
        .file_is_indexed(&root.join(file).to_string_lossy())
        .await
        .unwrap()
}

#[tokio::test]
async fn the_server_follows_the_workspace_configuration() {
    let base = TempDir::new().unwrap();
    let root = base.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = dunce::canonicalize(root).unwrap();
    std::fs::write(root.join("app.py"), "def app():\n    return 1\n").unwrap();
    std::fs::write(root.join("lib.rs"), "fn lib() -> u8 { 1 }\n").unwrap();
    let config = root.join("devscriptor.yaml");
    std::fs::write(&config, "scan:\n  include: [py]\n").unwrap();

    let mut server = Server::start(base.path(), &root);
    let health = server.call("health_check", json!({}));
    assert_eq!(
        health["project_config"]["path"],
        config.to_string_lossy().as_ref()
    );
    assert_eq!(health["project_config"]["error"], Value::Null);
    assert_eq!(health["hot_reload"]["enabled"], true);

    let path = root.to_string_lossy().into_owned();
    server.call("scan_codebase", json!({"path": path}));
    assert!(indexed(base.path(), &root, "app.py").await);
    assert!(!indexed(base.path(), &root, "lib.rs").await);

    // Broken mid-edit: reported, and the scope stays.
    std::fs::write(&config, "scan:\n  include: [rs\n").unwrap();
    let health = server.call("health_check", json!({}));
    assert!(health["project_config"]["error"].is_string(), "{health}");
    server.call("scan_codebase", json!({"path": path}));
    assert!(!indexed(base.path(), &root, "lib.rs").await);

    // Fixed: the next call takes it, and the server was not restarted.
    std::fs::write(&config, "scan:\n  include: [py, rs]\n").unwrap();
    server.call("scan_codebase", json!({"path": path}));
    assert!(indexed(base.path(), &root, "lib.rs").await);
    let health = server.call("health_check", json!({}));
    assert_eq!(health["project_config"]["error"], Value::Null);

    // A tool's own arguments outrank the file.
    std::fs::write(root.join("web.ts"), "export const web = 1;\n").unwrap();
    server.call("scan_codebase", json!({"path": path, "extensions": ["ts"]}));
    assert!(indexed(base.path(), &root, "web.ts").await);
}
