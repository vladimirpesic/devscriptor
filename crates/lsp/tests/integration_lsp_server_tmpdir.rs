//! A language server runs with a temporary directory of its own, removed with
//! it: rust-analyzer leaves empty `proc-macro-srv*` directories in `/tmp` even
//! when it exits cleanly, and a server's leftovers must not pile up there.
#![cfg(all(feature = "lsp", unix))]

use std::collections::HashMap;

use devscriptor_lsp::config::{LspServerConfig, ServerSettings};
use devscriptor_lsp::lsp::lifecycle::LspServer;

/// A fake server: records its `TMPDIR` in the workspace, leaves a directory
/// and a file there, answers `initialize` and `shutdown`, and exits on `exit`.
const FAKE_SERVER: &str = r#"
import json, os, sys

tmp = os.environ["TMPDIR"]
os.makedirs(os.path.join(tmp, "proc-macro-srv-left-behind"))
open(os.path.join(tmp, "scratch.bin"), "w").write("x")
open("tmpdir.txt", "w").write(tmp)

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

#[tokio::test]
async fn a_servers_temporary_directory_goes_with_it() {
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join("fake_server.py");
    std::fs::write(&script, FAKE_SERVER).unwrap();
    let config = LspServerConfig {
        language_id: "fake".into(),
        command: "python3".into(),
        args: vec![script.to_string_lossy().into_owned()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["fake".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, workspace.path()).await.unwrap();
    let tmp = std::path::PathBuf::from(
        std::fs::read_to_string(workspace.path().join("tmpdir.txt")).unwrap(),
    );
    let name = tmp.file_name().unwrap().to_string_lossy().into_owned();
    // Named for the Devscriptor process it belongs to, so that one left by
    // a process that is gone can be told and removed.
    let prefix = format!("devscriptor-lsp-{}-fake-", std::process::id());
    assert!(name.starts_with(&prefix), "{}", tmp.display());
    assert_eq!(tmp.parent(), Some(std::env::temp_dir().as_path()));
    assert!(tmp.join("proc-macro-srv-left-behind").is_dir());

    server.shutdown().await.unwrap();
    // The background reap of `Drop` may run a moment after `shutdown`.
    for _ in 0..50 {
        if !tmp.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!tmp.exists(), "{} was left behind", tmp.display());
}

/// A server that never answers `initialize`: the spawn fails, and the process
/// is stopped rather than left running beside the server that replaces it.
#[tokio::test]
async fn a_server_that_never_initializes_is_stopped() {
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join("silent_server.py");
    std::fs::write(
        &script,
        // It ignores even the end of its input, as a hung server does.
        "import os, time\nopen('pid.txt', 'w').write(str(os.getpid()))\nwhile True:\n    time.sleep(1)\n",
    )
    .unwrap();
    let mut config = LspServerConfig {
        language_id: "silent".into(),
        command: "python3".into(),
        args: vec![script.to_string_lossy().into_owned()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["silent".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };
    config.settings.timeout_seconds = 1;

    assert!(LspServer::spawn(&config, workspace.path()).await.is_err());
    let pid = std::fs::read_to_string(workspace.path().join("pid.txt")).unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
        "the silent server (pid {}) is still running",
        pid.trim()
    );
}
