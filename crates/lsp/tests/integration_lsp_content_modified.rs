//! `ContentModified` (-32801) must be retried, not surfaced.
//!
//! A server that is still digesting a `didOpen`/`didChange` answers
//! `ContentModified` instead of computing against a stale tree. The LSP spec
//! treats it as a "try again" signal: unretried, the *first* request against a
//! newly opened document would fail, as it does against a real rust-analyzer.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::process::Stdio;

use devscriptor_lsp::config::{LspServerConfig, ServerSettings};
use devscriptor_lsp::lsp::client::LspClient;
use devscriptor_lsp::lsp::transport::LspTransport;
use tokio::process::Command;

/// A fake LSP server: answers the first `n` requests with `ContentModified`,
/// then returns `{"ok": true}` forever. Writes LSP-framed JSON-RPC on stdout.
const FAKE_SERVER: &str = r#"
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

fail_first = int(sys.argv[1])
seen = 0
while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        continue  # notification
    seen += 1
    if seen <= fail_first:
        send({"jsonrpc": "2.0", "id": msg["id"],
              "error": {"code": -32801, "message": "content modified"}})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"ok": True, "attempt": seen}})
"#;

fn config() -> LspServerConfig {
    LspServerConfig {
        language_id: "fake".into(),
        command: "python3".into(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["fake".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    }
}

/// Spawns the fake server and wires a client to it.
fn client_failing_first(n: u32, script: &std::path::Path) -> (LspClient, tokio::process::Child) {
    let mut child = Command::new("python3")
        .arg(script)
        .arg(n.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake lsp server");

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let transport = LspTransport::new(stdin, stdout);
    (LspClient::from_transport(config(), transport), child)
}

fn write_script(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("fake_lsp.py");
    std::fs::write(&path, FAKE_SERVER).unwrap();
    path
}

/// One `ContentModified` — the real-world case — is transparently retried.
#[tokio::test]
async fn a_single_content_modified_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(&dir);
    let (client, mut child) = client_failing_first(1, &script);

    let result: serde_json::Value = client
        .request("textDocument/hover", serde_json::json!({}))
        .await
        .expect("ContentModified must be retried, not surfaced");

    assert_eq!(result["ok"], serde_json::json!(true));
    assert_eq!(
        result["attempt"],
        serde_json::json!(2),
        "the second attempt must be the one that succeeded"
    );
    let _ = child.kill().await;
}

/// The retry budget is bounded: a server stuck in `ContentModified` must
/// eventually surface the error rather than spin forever.
#[tokio::test]
async fn content_modified_retries_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(&dir);
    let (client, mut child) = client_failing_first(99, &script);

    let result: Result<serde_json::Value, _> = client
        .request("textDocument/hover", serde_json::json!({}))
        .await;

    let err = result.expect_err("a permanently modified document must surface the error");
    assert!(
        format!("{err}").contains("content modified") || format!("{err}").contains("-32801"),
        "unexpected error: {err}"
    );
    let _ = child.kill().await;
}

/// Errors that are not `ContentModified` must not be retried: a genuine failure
/// should fail once, immediately.
#[tokio::test]
async fn other_server_errors_are_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("always_fail.py");
    std::fs::write(
        &path,
        FAKE_SERVER.replace("\"code\": -32801", "\"code\": -32603"),
    )
    .unwrap();

    let (client, mut child) = client_failing_first(99, &path);
    let started = std::time::Instant::now();
    let result: Result<serde_json::Value, _> = client
        .request("textDocument/hover", serde_json::json!({}))
        .await;

    assert!(result.is_err());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(400),
        "a non-retryable error must not sleep through the retry backoff"
    );
    let _ = child.kill().await;
}

/// `ServerCancelled` (-32802, LSP 3.17) asks for the request again too.
#[tokio::test]
async fn a_server_cancelled_request_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cancels_once.py");
    std::fs::write(
        &path,
        FAKE_SERVER.replace("\"code\": -32801", "\"code\": -32802"),
    )
    .unwrap();

    let (client, mut child) = client_failing_first(1, &path);
    let result: serde_json::Value = client
        .request("textDocument/hover", serde_json::json!({}))
        .await
        .expect("ServerCancelled must be retried, not surfaced");

    assert_eq!(result["attempt"], serde_json::json!(2));
    let _ = child.kill().await;
}
