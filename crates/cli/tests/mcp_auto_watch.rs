//! `devscriptor mcp` watches the scanned project it runs in.
//!
//! Agents rarely ask for a watch, and an index that does not follow their edits
//! answers from code as it was. A server started inside a directory that a scan
//! indexed watches that directory by itself, and one started before the scan
//! watches it once the scan is done; `--no-watch` and `DEVSCRIPTOR_WATCH=0`
//! turn it off, and `start_watching` on the watched root reports the running
//! watch instead of failing.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// A scanned project and the database holding it.
struct Project {
    dir: TempDir,
}

impl Project {
    /// A project a scan has indexed.
    fn new() -> Self {
        let project = Self::unscanned();
        project.scan();
        project
    }

    /// A project no scan has indexed yet.
    fn unscanned() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/app.py"),
            "def existing_function():\n    return 1\n",
        )
        .unwrap();
        Self { dir }
    }

    /// `devscriptor scan` of the project, in a process of its own.
    fn scan(&self) {
        let status = self
            .command()
            .args(["scan", "--skip-embeddings"])
            .arg(self.root())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the scan failed");
    }

    fn root(&self) -> PathBuf {
        dunce::canonicalize(self.dir.path().join("project")).unwrap()
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
            .env_remove("DEVSCRIPTOR_WATCH")
            .env_remove("DEVSCRIPTOR_LSP_STARTUP");
        command
    }

    /// `devscriptor mcp <args>` in the project, with `env` set.
    fn serve(&self, args: &[&str], env: &[(&str, &str)]) -> Server {
        let mut command = self.command();
        command
            .arg("mcp")
            .args(args)
            .current_dir(self.root().join("src"))
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
            None => content["json"].clone(),
        }
    }

    /// Wait up to `limit` for `done` to hold of the watch status.
    fn wait_status(&mut self, limit: Duration, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + limit;
        loop {
            let status = self.call("get_watch_status", json!({}));
            if done(&status) || Instant::now() > deadline {
                return status;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Whether `search_functions` finds a function named `name`.
    fn finds(&mut self, name: &str) -> bool {
        let found = self.call("search_functions", json!({"query": name}));
        found.to_string().contains(name)
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
fn the_server_watches_the_scanned_project_it_runs_in() {
    let project = Project::new();
    let mut server = project.serve(&[], &[]);

    let status = server.wait_status(Duration::from_secs(30), |s| s["watching"] == true);
    assert_eq!(status["watching"], true, "{status}");
    assert_eq!(
        Path::new(status["path"].as_str().unwrap()),
        project.root(),
        "{status}"
    );

    // An edit is indexed without anyone asking. Where the system's inotify
    // watches are used up the server polls, and polling compares modification
    // times in whole seconds: edit in a later second than the file's creation.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(
        project.root().join("src/app.py"),
        "def existing_function():\n    return 1\n\n\ndef added_while_watched():\n    return 2\n",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !server.finds("added_while_watched") {
        assert!(Instant::now() < deadline, "the edit was not indexed");
        std::thread::sleep(Duration::from_millis(250));
    }

    // Asked to watch the same root, it reports the running watch.
    let again = server.call(
        "start_watching",
        json!({"path": project.root().to_string_lossy()}),
    );
    assert_eq!(again["already_watching"], true, "{again}");
    assert_eq!(again["watching"], true, "{again}");
}

#[test]
fn no_watch_and_the_environment_turn_it_off() {
    let project = Project::new();
    for (args, env) in [
        (vec!["--no-watch"], vec![]),
        (vec![], vec![("DEVSCRIPTOR_WATCH", "0")]),
    ] {
        let mut server = project.serve(&args, &env);
        // Long enough for a watch to have started, had one been wanted.
        std::thread::sleep(Duration::from_secs(3));
        let status = server.call("get_watch_status", json!({}));
        assert_eq!(status["watching"], false, "{args:?} {env:?}: {status}");
    }
}

/// A server started in a project no scan has indexed watches it once
/// `scan_codebase` has.
#[test]
fn a_scan_starts_the_watch_of_an_unscanned_workspace() {
    let project = Project::unscanned();
    let mut server = project.serve(&[], &[]);
    let status = server.call("get_watch_status", json!({}));
    assert_eq!(status["watching"], false, "{status}");

    server.call(
        "scan_codebase",
        json!({"path": project.root().to_string_lossy()}),
    );
    let status = server.wait_status(Duration::from_secs(30), |s| s["watching"] == true);
    assert_eq!(status["watching"], true, "{status}");
    assert_eq!(
        Path::new(status["path"].as_str().unwrap()),
        project.root(),
        "{status}"
    );
}

/// A scan made by another process, such as `devscriptor scan` in a terminal,
/// is noticed at the server's next tool call.
#[test]
fn a_scan_made_elsewhere_starts_the_watch_at_the_next_call() {
    let project = Project::unscanned();
    let mut server = project.serve(&[], &[]);
    project.scan();
    let status = server.wait_status(Duration::from_secs(30), |s| s["watching"] == true);
    assert_eq!(status["watching"], true, "{status}");
}

/// After `stop_watching`, a scan does not start the watch again.
#[test]
fn stop_watching_keeps_a_scan_from_watching_again() {
    let project = Project::unscanned();
    let mut server = project.serve(&[], &[]);
    server.call("stop_watching", json!({}));
    server.call(
        "scan_codebase",
        json!({"path": project.root().to_string_lossy()}),
    );
    // Long enough for a watch to have started, had one been wanted.
    std::thread::sleep(Duration::from_secs(3));
    let status = server.call("get_watch_status", json!({}));
    assert_eq!(status["watching"], false, "{status}");
}
