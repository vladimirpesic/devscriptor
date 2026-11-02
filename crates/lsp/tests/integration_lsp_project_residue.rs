//! What a language server would leave in a project is kept out of it, or out
//! of git, before it starts: rust-analyzer's cargo resolves into a lock file
//! of its own, and Metals' `.metals/` is ignored.
#![cfg(all(feature = "lsp", unix))]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::config::{LspServerConfig, ServerSettings};
use devscriptor_lsp::lsp::lifecycle::LspServer;

/// A fake server: records, as it starts, the lock file cargo would be told
/// and whether `.metals/` is ignored already, answers `initialize` and
/// `shutdown`, and exits on `exit`.
const FAKE_SERVER: &str = r#"
import json, os, sys

open("started.txt", "w").write(json.dumps({
    "lockfile": os.environ.get("CARGO_RESOLVER_LOCKFILE_PATH"),
    "metals_ignored": os.path.isfile(os.path.join(".metals", ".gitignore")),
}))

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
    return json.loads(sys.stdin.buffer.read(length)) if length else None

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    msg = read_message()
    if msg is None or msg.get("method") == "exit":
        break
    if "id" in msg:
        result = {"capabilities": {}} if msg.get("method") == "initialize" else None
        send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
"#;

/// What the fake server, started as `language_id`'s in `project`, recorded.
async fn started_as(language_id: &str, project: &Path) -> serde_json::Value {
    let script = project.join("fake_server.py");
    std::fs::write(&script, FAKE_SERVER).unwrap();
    let config = LspServerConfig {
        language_id: language_id.into(),
        command: "python3".into(),
        args: vec![script.to_string_lossy().into_owned()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["x".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };
    let server = LspServer::spawn(&config, project).await.unwrap();
    server.shutdown().await.unwrap();
    serde_json::from_str(&std::fs::read_to_string(project.join("started.txt")).unwrap()).unwrap()
}

/// rust-analyzer is told to resolve into its build directory's lock file,
/// a copy of the project's, and a project without one gets none; Metals
/// finds its `.metals/` ignored already; another server is told neither.
#[tokio::test]
async fn a_server_starts_with_what_it_writes_kept_out_of_the_project() {
    let rust = tempfile::tempdir().unwrap();
    std::fs::write(rust.path().join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(rust.path().join("Cargo.lock"), "version = 4\n").unwrap();
    let started = started_as("rust", rust.path()).await;
    assert_eq!(started["lockfile"], "target/rust-analyzer/Cargo.lock");
    assert_eq!(
        std::fs::read_to_string(rust.path().join("target/rust-analyzer/Cargo.lock")).unwrap(),
        "version = 4\n"
    );

    let scala = tempfile::tempdir().unwrap();
    std::fs::write(scala.path().join("build.sbt"), "").unwrap();
    let started = started_as("scala", scala.path()).await;
    assert_eq!(started["metals_ignored"], true);
    assert_eq!(started["lockfile"], serde_json::Value::Null);

    let python = tempfile::tempdir().unwrap();
    let started = started_as("python", python.path()).await;
    assert_eq!(started["lockfile"], serde_json::Value::Null);
    assert!(!python.path().join(".metals").exists());
}
