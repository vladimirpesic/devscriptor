//! `devscriptor mcp`'s server checks say why a language server does not run,
//! as its install shows: `lsp_arch_check`, `lsp_arch_batch_check` and the
//! servers to install in `lsp_arch_detect` give the reason, and
//! `lsp_arch_status` sets an install that is there but cannot run apart from
//! the servers to install. Agents told only `available: false` of a mirror's
//! `ruby-lsp`, built for another Ruby, reported it as not installed.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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
    /// daemon), with `base` for its data, cache and home: the servers it finds
    /// are the installs under `base/cache`, and none of the user's.
    fn start(base: &Path, workspace: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
            .arg("--database")
            .arg(base.join("index.db"))
            .args(["mcp", "--no-watch"])
            .current_dir(workspace)
            .env("HOME", base.join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_DATA_HOME", base.join("data"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env("XDG_RUNTIME_DIR", base.join("run"))
            .env("DEVSCRIPTOR_DAEMON", "0")
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
    /// Close the server's input, so that it exits as when its client leaves.
    /// Killed if it has not exited within ten seconds.
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

/// The entry for `server_id` in one of `lsp_arch_status`'s groups.
fn in_group<'a>(status: &'a Value, group: &str, server_id: &str) -> Option<&'a Value> {
    status["servers"][group]
        .as_array()?
        .iter()
        .find(|server| server["server_id"] == server_id)
}

/// A jdtls install of another build than the pinned one cannot run, and an
/// absent Kotlin server is to be installed: each check says which, and why.
#[test]
fn a_server_that_does_not_run_says_why() {
    let base = TempDir::new().unwrap();
    let workspace = base.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("pom.xml"), "<project/>\n").unwrap();
    std::fs::write(workspace.join("src/App.java"), "class App {}\n").unwrap();
    std::fs::write(workspace.join("src/Main.kt"), "fun main() {}\n").unwrap();
    let jdtls = base.path().join("cache/devscriptor/lsp-servers/jdtls");
    std::fs::create_dir_all(jdtls.join("bin")).unwrap();
    std::fs::write(jdtls.join("bin/jdtls"), "#!/bin/sh\n").unwrap();
    std::fs::write(jdtls.join(".version"), "1.40.0\n").unwrap();
    let outdated = "is build 1.40.0, not the 1.61.0 this Devscriptor pins: `devscriptor lsp setup \
                    jdtls` replaces it";
    let absent = "is not installed: `devscriptor lsp setup kotlin-lsp` installs it";
    let mut server = Server::start(base.path(), &workspace);

    let check = server.call("lsp_arch_check", json!({"server_id": "java"}));
    assert_eq!(check["available"], false, "{check}");
    assert_eq!(check["reason"], outdated, "{check}");

    let batch = server.call(
        "lsp_arch_batch_check",
        json!({"server_ids": ["jdtls", "kotlin"]}),
    );
    assert_eq!(batch["results"][0]["reason"], outdated, "{batch}");
    assert_eq!(batch["results"][1]["reason"], absent, "{batch}");

    let status = server.call("lsp_arch_status", json!({}));
    let cannot_run = in_group(&status, "cannot_run", "jdtls").unwrap_or_else(|| panic!("{status}"));
    assert_eq!(cannot_run["status"], "cannot_run");
    assert_eq!(cannot_run["reason"], outdated);
    assert!(
        in_group(&status, "needs_install", "jdtls").is_none(),
        "{status}"
    );
    let kotlin = in_group(&status, "needs_install", "kotlin-lsp")
        .or_else(|| in_group(&status, "missing_prerequisites", "kotlin-lsp"))
        .unwrap_or_else(|| panic!("{status}"));
    assert_eq!(kotlin["reason"], absent, "{status}");

    let detected = server.call(
        "lsp_arch_detect",
        json!({"project_path": workspace.to_string_lossy()}),
    );
    let reasons: Vec<&Value> = detected["needs_install"]
        .as_array()
        .unwrap()
        .iter()
        .chain(detected["missing_prerequisites"].as_array().unwrap())
        .filter(|server| server["server_id"] == "jdtls" || server["server_id"] == "kotlin-lsp")
        .map(|server| &server["reason"])
        .collect();
    assert!(
        reasons.iter().any(|reason| **reason == outdated),
        "{detected}"
    );
}

/// `devscriptor lsp check`, `batch-check` and `status` give the same reasons.
#[test]
fn the_cli_says_why_a_server_does_not_run() {
    let base = TempDir::new().unwrap();
    let jdtls = base.path().join("cache/devscriptor/lsp-servers/jdtls");
    std::fs::create_dir_all(jdtls.join("bin")).unwrap();
    std::fs::write(jdtls.join("bin/jdtls"), "#!/bin/sh\n").unwrap();
    std::fs::write(jdtls.join(".version"), "1.40.0\n").unwrap();
    let outdated = "is build 1.40.0, not the 1.61.0 this Devscriptor pins: `devscriptor lsp setup \
                    jdtls` replaces it";
    let lsp = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
            .arg("lsp")
            .args(args)
            .current_dir(base.path())
            .env("HOME", base.path().join("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CACHE_HOME", base.path().join("cache"))
            .env("XDG_CONFIG_HOME", base.path().join("config"))
            .env("XDG_DATA_HOME", base.path().join("data"))
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let check = lsp(&["check", "jdtls"]);
    assert!(check.contains(&format!("jdtls {outdated}")), "{check}");
    let batch = lsp(&["batch-check", "jdtls", "kotlin-lsp"]);
    assert!(batch.contains(&format!("jdtls - {outdated}")), "{batch}");
    assert!(
        batch.contains("kotlin-lsp - is not installed: `devscriptor lsp setup kotlin-lsp`"),
        "{batch}"
    );
    let status = lsp(&["status"]);
    assert!(status.contains(&format!("❌ jdtls {outdated}")), "{status}");
}
