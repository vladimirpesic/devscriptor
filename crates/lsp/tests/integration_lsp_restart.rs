//! A language server that exits is restarted by the next request.
//!
//! A crashed server does not leave its client registered, failing every later
//! request with "Command channel closed" until `restart_server` is called. A
//! server that dies on every start must not be respawned forever.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::time::Duration;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{DocumentSymbolsParams, ServerStatusParams};

/// Answers `initialize` and `documentSymbol`. A `documentSymbol` that finds
/// the file named by its first argument removes it, replies, and exits; one
/// that finds the file named by its second removes it and exits without
/// replying. The test crashes the server when it chooses.
const CRASHING_SERVER: &str = r#"
import os, sys, json
crash_marker, silent_marker = sys.argv[1], sys.argv[2]

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

span = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}
while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        if msg.get("method") == "exit":
            break
        continue
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "documentSymbolProvider": True, "textDocumentSync": 1}}})
    elif method == "textDocument/documentSymbol":
        # Decide before replying: a marker the test writes once this reply
        # has arrived is meant for its next request, not this one.
        crash = os.path.exists(crash_marker)
        if crash:
            os.remove(crash_marker)
        if os.path.exists(silent_marker):
            os.remove(silent_marker)
            sys.exit(0)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "thing", "kind": 12, "range": span, "selectionRange": span}]})
        if crash:
            sys.exit(0)
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

fn translator(dir: &std::path::Path) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, CRASHING_SERVER).unwrap();
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
            language_id: "fake".into(),
            command: "python3".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                dir.join("crash").to_string_lossy().into_owned(),
                dir.join("die-silently").to_string_lossy().into_owned(),
            ],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["fake".into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

async fn state(translator: &Translator) -> (String, Option<u32>) {
    let status = translator
        .handle_server_status(ServerStatusParams {
            language_id: Some("fake".into()),
        })
        .await
        .unwrap();
    (status.servers[0].state.clone(), status.servers[0].pid)
}

/// Wait for the bridge to notice the process is gone.
async fn wait_until_exited(translator: &Translator) {
    for _ in 0..100 {
        if state(translator).await.0 == "exited" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never reported as exited");
}

#[tokio::test]
async fn an_exited_server_is_restarted_by_the_next_request_up_to_a_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.fake");
    std::fs::write(&file, "thing\n").unwrap();
    let translator = translator(dir.path());
    translator.spawn_applicable(None).await.unwrap();
    let params = || DocumentSymbolsParams {
        file_path: file.to_string_lossy().into_owned(),
    };

    let crash = || std::fs::write(dir.path().join("crash"), "").unwrap();

    let first = translator.handle_document_symbols(params()).await.unwrap();
    assert_eq!(first.symbols[0].name, "thing");
    let (_, mut pid) = state(&translator).await;

    // Three restarts are allowed in the window; each serves the request.
    for _ in 0..3 {
        crash();
        translator.handle_document_symbols(params()).await.unwrap();
        wait_until_exited(&translator).await;

        let again = translator.handle_document_symbols(params()).await.unwrap();
        assert_eq!(again.symbols[0].name, "thing");
        let (state_now, new_pid) = state(&translator).await;
        assert_eq!(state_now, "running");
        assert_ne!(new_pid, pid, "a new process answered");
        pid = new_pid;
    }

    // The fourth exit in the window leaves it down, saying how to restart it.
    crash();
    translator.handle_document_symbols(params()).await.unwrap();
    wait_until_exited(&translator).await;
    let err = translator
        .handle_document_symbols(params())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("restart_server"), "{err}");
}

/// A request the server exits on fails at once, saying so, instead of
/// waiting out the request timeout; the next request restarts the server.
#[tokio::test]
async fn a_request_the_server_exits_on_fails_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.fake");
    std::fs::write(&file, "thing\n").unwrap();
    let translator = translator(dir.path());
    translator.spawn_applicable(None).await.unwrap();
    let params = || DocumentSymbolsParams {
        file_path: file.to_string_lossy().into_owned(),
    };
    translator.handle_document_symbols(params()).await.unwrap();

    std::fs::write(dir.path().join("die-silently"), "").unwrap();
    let asked = std::time::Instant::now();
    let err = translator
        .handle_document_symbols(params())
        .await
        .unwrap_err();
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "failed only after {:?}, the request timeout is 30 s",
        asked.elapsed()
    );
    assert!(err.to_string().contains("closed the connection"), "{err}");

    let again = translator.handle_document_symbols(params()).await.unwrap();
    assert_eq!(again.symbols[0].name, "thing");
}
