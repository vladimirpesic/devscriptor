//! Diagnostics from servers that push them.
//!
//! typescript-language-server, clangd, gopls and jdtls offer no pull
//! diagnostics (`textDocument/diagnostic`): they push them with
//! `textDocument/publishDiagnostics` after a document is opened or changed.
//! Pulled from those servers, every `get_diagnostics` call would fail with
//! `-32601 Unhandled method`; it reports what they pushed instead.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use devscriptor_lsp::bridge::{Translator, path_to_uri};
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::lsp::client::LspClient;
use devscriptor_lsp::lsp::transport::LspTransport;
use devscriptor_lsp::mcp::tools::DiagnosticsParams;
use tokio::process::Command;

/// A fake language server. In mode `push` it offers no pull diagnostics; in
/// `pull` it answers them; in `pull-unhandled` it offers them but answers
/// `-32601`. In every mode it pushes diagnostics for an opened document in two
/// waves, as typescript-language-server does — the syntax errors, then the
/// type errors — spelling the document's URI differently from the client.
const FAKE_SERVER: &str = r#"
import json, sys, time, urllib.parse

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

def respell(uri):
    path = urllib.parse.unquote(uri[len("file://"):])
    return "file://" + urllib.parse.quote(path, safe="/").replace(".", "%2E")

def diagnostic(line, message):
    return {"range": {"start": {"line": line, "character": 0},
                      "end": {"line": line, "character": 1}},
            "severity": 1, "source": "fake", "message": message}

mode = sys.argv[1]
capabilities = {"textDocumentSync": 1}
if mode != "push":
    capabilities["diagnosticProvider"] = {"interFileDependencies": False,
                                          "workspaceDiagnostics": False}

while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": capabilities}})
    elif method == "textDocument/didOpen":
        uri = respell(msg["params"]["textDocument"]["uri"])
        time.sleep(0.2)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": uri, "diagnostics": [diagnostic(0, "syntax")]}})
        time.sleep(0.6)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": uri, "diagnostics": [diagnostic(0, "syntax"),
                                                     diagnostic(1, "type")]}})
    elif method == "textDocument/diagnostic":
        if mode == "pull-unhandled":
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "error": {"code": -32601, "message": "Unhandled method textDocument/diagnostic"}})
        else:
            send({"jsonrpc": "2.0", "id": msg["id"],
                  "result": {"kind": "full", "items": [diagnostic(2, "pulled")]}})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

fn server_config(script: &std::path::Path, mode: &str) -> LspServerConfig {
    LspServerConfig {
        language_id: "fake".into(),
        command: "python3".into(),
        args: vec![script.display().to_string(), mode.into()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["fake".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    }
}

/// A scratch directory with the fake server and a document for it.
struct Scratch {
    _dir: tempfile::TempDir,
    script: std::path::PathBuf,
    document: std::path::PathBuf,
}

fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake_lsp.py");
    std::fs::write(&script, FAKE_SERVER).unwrap();
    let document = dir.path().join("cart.model.fake");
    std::fs::write(&document, "one\ntwo\nthree\n").unwrap();
    Scratch {
        _dir: dir,
        script,
        document,
    }
}

fn messages(diagnostics: &[lsp_types::Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.message.as_str()).collect()
}

/// The client keeps what is pushed, finds it under its own spelling of the
/// URI, and waits out a wave of pushes for the last, complete one.
#[tokio::test]
async fn the_client_waits_for_the_last_wave_of_pushes() {
    let scratch = scratch();
    let mut child = Command::new("python3")
        .arg(&scratch.script)
        .arg("push")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake lsp server");
    let transport = LspTransport::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let client = LspClient::from_transport(server_config(&scratch.script, "push"), transport);
    let uri = path_to_uri(&scratch.document).unwrap();

    let mark = client.diagnostics_mark();
    client
        .notify(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {
                "uri": uri.as_str(), "languageId": "fake", "version": 1, "text": "one\n"
            }}),
        )
        .await
        .unwrap();
    let pushed = client
        .await_pushed_diagnostics(
            uri.as_str(),
            mark,
            Duration::from_secs(5),
            Duration::from_secs(1),
        )
        .await
        .expect("the server pushed");
    assert_eq!(messages(&pushed), ["syntax", "type"]);
    assert_eq!(
        client.pushed_diagnostics(uri.as_str()).map(|d| d.len()),
        Some(2)
    );

    // Nothing is pushed for a document the server was never sent.
    let other = path_to_uri(&scratch.script).unwrap();
    assert!(
        client
            .await_pushed_diagnostics(
                other.as_str(),
                0,
                Duration::from_millis(300),
                Duration::from_secs(1)
            )
            .await
            .is_none()
    );
    let _ = child.kill().await;
}

async fn diagnostics_via_translator(mode: &str) -> (Vec<String>, Vec<String>, Duration) {
    let scratch = scratch();
    let mut translator = Translator::new(Config {
        workspace: WorkspaceConfig::default(),
        servers: vec![server_config(&scratch.script, mode)],
        lifecycle: LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![]);
    let params = || DiagnosticsParams {
        file_path: scratch.document.display().to_string(),
    };
    let first = translator.handle_diagnostics(params()).await.unwrap();
    let started = Instant::now();
    let again = translator.handle_diagnostics(params()).await.unwrap();
    let elapsed = started.elapsed();
    translator.shutdown().await;
    let messages = |result: devscriptor_lsp::mcp::tools::DiagnosticsResult| {
        result
            .diagnostics
            .into_iter()
            .map(|d| d.message)
            .collect::<Vec<_>>()
    };
    (messages(first), messages(again), elapsed)
}

/// A server without pull diagnostics is answered for by its pushes; asked
/// again about the unchanged document, its last push answers at once.
#[tokio::test]
async fn get_diagnostics_reports_what_a_push_only_server_pushed() {
    let (first, again, elapsed) = diagnostics_via_translator("push").await;
    assert_eq!(first, ["syntax", "type"]);
    assert_eq!(again, ["syntax", "type"]);
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

/// A server that offers pull diagnostics but has no handler for them falls
/// back to its pushes.
#[tokio::test]
async fn an_unhandled_pull_falls_back_to_the_pushes() {
    let (first, _, _) = diagnostics_via_translator("pull-unhandled").await;
    assert_eq!(first, ["syntax", "type"]);
}

/// A server that answers pull diagnostics is asked for them.
#[tokio::test]
async fn a_server_with_pull_diagnostics_is_asked() {
    let (first, again, _) = diagnostics_via_translator("pull").await;
    assert_eq!(first, ["pulled"]);
    assert_eq!(again, ["pulled"]);
}
