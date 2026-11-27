//! Diagnostics from servers that push them.
//!
//! typescript-language-server, clangd, gopls and jdtls offer no pull
//! diagnostics (`textDocument/diagnostic`): they push them with
//! `textDocument/publishDiagnostics` after a document is opened or changed.
//! Pulled from those servers, every `get_diagnostics` call would fail with
//! `-32601 Unhandled method`; it reports what they pushed instead.
//!
//! rust-analyzer answers pulls with its own analysis, and pushes what the
//! check a save starts finds (`cargo check`): it is told the file is saved,
//! the check waited for, and its pushes reported with the pulled ones.

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
/// waves, as typescript-language-server does — the syntax error, then a type
/// warning with it — spelling the document's URI differently from the client.
/// It pushes nothing for a change to a document, except in mode `slow-change`,
/// where it pushes `changed` 2.6 seconds after one. Modes `once` and `late`
/// push only: `once` for a document's first opening alone, `late` three
/// seconds after it. Modes `check` and `slow-check` answer pulls and take
/// saves, as rust-analyzer does: they push nothing for an opened or changed
/// document, but on a save run a check, reported as progress, that takes 0.8
/// or 3 seconds and pushes `checked` for a document whose text holds `bad`,
/// and an empty list for one it pushed `checked` for before.
const FAKE_SERVER: &str = r#"
import json, sys, threading, time, urllib.parse

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

sending = threading.Lock()

def send(obj):
    body = json.dumps(obj).encode()
    with sending:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def respell(uri):
    path = urllib.parse.unquote(uri[len("file://"):])
    return "file://" + urllib.parse.quote(path, safe="/").replace(".", "%2E")

def diagnostic(line, message, severity=1):
    return {"range": {"start": {"line": line, "character": 0},
                      "end": {"line": line, "character": 1}},
            "severity": severity, "source": "fake", "message": message}

mode = sys.argv[1]
checks = mode in ("check", "slow-check")
capabilities = {"textDocumentSync": {"openClose": True, "change": 1, "save": {}} if checks else 1}
opened = set()
texts = {}
flagged = set()
if mode not in ("push", "once", "late", "slow-change"):
    capabilities["diagnosticProvider"] = {"interFileDependencies": False,
                                          "workspaceDiagnostics": False}

def check(uri):
    send({"jsonrpc": "2.0", "method": "$/progress",
          "params": {"token": "check", "value": {"kind": "begin", "title": "check"}}})
    time.sleep(3 if mode == "slow-check" else 0.8)
    if "bad" in texts.get(uri, ""):
        flagged.add(uri)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": respell(uri), "diagnostics": [diagnostic(3, "checked")]}})
    elif uri in flagged:
        flagged.discard(uri)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": respell(uri), "diagnostics": []}})
    send({"jsonrpc": "2.0", "method": "$/progress",
          "params": {"token": "check", "value": {"kind": "end"}}})

while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": capabilities}})
    elif checks and method in ("textDocument/didOpen", "textDocument/didChange"):
        document = msg["params"]["textDocument"]
        texts[document["uri"]] = (document.get("text")
                                  or msg["params"]["contentChanges"][-1]["text"])
    elif checks and method == "textDocument/didSave":
        threading.Thread(target=check, args=(msg["params"]["textDocument"]["uri"],)).start()
    elif method == "textDocument/didOpen":
        uri = respell(msg["params"]["textDocument"]["uri"])
        if mode == "once" and uri in opened:
            continue
        opened.add(uri)
        time.sleep(3 if mode == "late" else 0.2)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": uri, "diagnostics": [diagnostic(0, "syntax")]}})
        time.sleep(0.6)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": uri, "diagnostics": [diagnostic(0, "syntax"),
                                                     diagnostic(1, "type", 2)]}})
    elif method == "textDocument/didChange" and mode == "slow-change":
        uri = respell(msg["params"]["textDocument"]["uri"])
        time.sleep(2.6)
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": uri, "diagnostics": [diagnostic(0, "changed")]}})
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
        prerequisites: Vec::new(),
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
        ..DiagnosticsParams::default()
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

/// A translator over the fake server in `mode`, and a document for it.
fn translator(mode: &str) -> (Scratch, Translator) {
    let scratch = scratch();
    let mut translator = Translator::new(Config {
        workspace: WorkspaceConfig::default(),
        servers: vec![server_config(&scratch.script, mode)],
        lifecycle: LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![]);
    (scratch, translator)
}

/// `severity` leaves out the diagnostics less severe than it.
#[tokio::test]
async fn severity_leaves_out_the_less_severe() {
    use devscriptor_lsp::mcp::tools::DiagnosticSeverity;
    let (scratch, translator) = translator("push");
    let ask = |severity| DiagnosticsParams {
        file_path: scratch.document.display().to_string(),
        severity,
        wait_ms: None,
    };
    let errors = translator
        .handle_diagnostics(ask(Some(DiagnosticSeverity::Error)))
        .await
        .unwrap();
    let names = |result: &devscriptor_lsp::mcp::tools::DiagnosticsResult| {
        result
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&errors), ["syntax"]);
    let warnings = translator
        .handle_diagnostics(ask(Some(DiagnosticSeverity::Warning)))
        .await
        .unwrap();
    assert_eq!(names(&warnings), ["syntax", "type"]);
    translator.shutdown().await;
}

/// After a change the server pushes nothing for, the answer is its last push
/// for the earlier text, said to be stale, within `wait_ms`.
#[tokio::test]
async fn a_change_with_no_push_in_time_is_answered_as_stale() {
    let (scratch, translator) = translator("once");
    let ask = |wait_ms| DiagnosticsParams {
        file_path: scratch.document.display().to_string(),
        severity: None,
        wait_ms,
    };
    let first = translator.handle_diagnostics(ask(None)).await.unwrap();
    assert!(!first.stale);
    assert_eq!(first.diagnostics.len(), 2);

    std::fs::write(&scratch.document, "one\ntwo\nthree\nfour\n").unwrap();
    let started = Instant::now();
    let after = translator.handle_diagnostics(ask(Some(300))).await.unwrap();
    let elapsed = started.elapsed();
    assert!(after.stale, "no push followed the change");
    assert_eq!(after.diagnostics.len(), 2);
    // The server's readiness is waited for first; the pushes no longer.
    assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
    let json = serde_json::to_value(&after).unwrap();
    assert_eq!(json["stale"], true);
    assert!(serde_json::to_value(&first).unwrap().get("stale").is_none());
    translator.shutdown().await;
}

/// A server that has not pushed within `wait_ms` for a document sent the
/// first time leaves the answer empty and stale: nothing is known yet.
#[tokio::test]
async fn nothing_pushed_in_time_is_unknown_not_clean() {
    let (scratch, translator) = translator("late");
    let result = translator
        .handle_diagnostics(DiagnosticsParams {
            file_path: scratch.document.display().to_string(),
            severity: None,
            wait_ms: Some(300),
        })
        .await
        .unwrap();
    assert!(result.stale);
    assert!(result.diagnostics.is_empty());
    translator.shutdown().await;
}

/// A file changed on disk since the server was sent it is sent again before
/// the request, and its diagnostics are those pushed for its text now, not
/// the last ones pushed for the text before, however the server takes its
/// time to push them.
#[tokio::test]
async fn a_file_changed_on_disk_is_diagnosed_as_it_is_now() {
    let (scratch, translator) = translator("slow-change");
    let ask = || DiagnosticsParams {
        file_path: scratch.document.display().to_string(),
        ..DiagnosticsParams::default()
    };
    let before = translator.handle_diagnostics(ask()).await.unwrap();
    assert_eq!(before.diagnostics.len(), 2);
    std::fs::write(&scratch.document, "one\ntwo\nthree\nfour\n").unwrap();
    let now = translator.handle_diagnostics(ask()).await.unwrap();
    let messages: Vec<&str> = now.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages, ["changed"]);
    translator.shutdown().await;
}

fn ask(scratch: &Scratch, wait_ms: Option<u64>) -> DiagnosticsParams {
    DiagnosticsParams {
        file_path: scratch.document.display().to_string(),
        severity: None,
        wait_ms,
    }
}

fn names(result: &devscriptor_lsp::mcp::tools::DiagnosticsResult) -> Vec<&str> {
    result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect()
}

/// A server that answers pulls and checks a file on its save is told the
/// file is saved, once for each text: its check is waited for, and what it
/// pushes is reported with what it answers — until a change it finds clean,
/// for which it pushes an empty list, or nothing at all.
#[tokio::test]
async fn the_check_a_save_starts_is_waited_for_and_reported() {
    let (scratch, translator) = translator("check");
    std::fs::write(&scratch.document, "bad\n").unwrap();
    let first = translator
        .handle_diagnostics(ask(&scratch, None))
        .await
        .unwrap();
    assert_eq!(names(&first), ["pulled", "checked"]);
    assert!(!first.stale && !first.checking);

    // The text was saved and checked: asked again, the server answers at once.
    let started = Instant::now();
    let again = translator
        .handle_diagnostics(ask(&scratch, None))
        .await
        .unwrap();
    assert_eq!(names(&again), ["pulled", "checked"]);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );

    std::fs::write(&scratch.document, "good\n").unwrap();
    let fixed = translator
        .handle_diagnostics(ask(&scratch, None))
        .await
        .unwrap();
    assert_eq!(names(&fixed), ["pulled"]);

    // A clean text checked clean pushes nothing: the end of the check answers.
    std::fs::write(&scratch.document, "fine\n").unwrap();
    let started = Instant::now();
    let clean = translator
        .handle_diagnostics(ask(&scratch, None))
        .await
        .unwrap();
    assert_eq!(names(&clean), ["pulled"]);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
    translator.shutdown().await;
}

/// A check still running when `wait_ms` ends leaves its findings out, and
/// the answer says so; asked again, the server's answer waits for its end.
#[tokio::test]
async fn a_check_still_running_is_said_to_be() {
    let (scratch, translator) = translator("slow-check");
    std::fs::write(&scratch.document, "bad\n").unwrap();
    // The check takes 3 seconds; the answer comes before it ends.
    let early = translator
        .handle_diagnostics(ask(&scratch, Some(300)))
        .await
        .unwrap();
    assert_eq!(names(&early), ["pulled"]);
    assert!(early.checking && !early.stale);
    assert_eq!(serde_json::to_value(&early).unwrap()["checking"], true);

    let later = translator
        .handle_diagnostics(ask(&scratch, None))
        .await
        .unwrap();
    assert_eq!(names(&later), ["pulled", "checked"]);
    assert!(!later.checking);
    assert!(
        serde_json::to_value(&later)
            .unwrap()
            .get("checking")
            .is_none()
    );
    translator.shutdown().await;
}

/// The work a save starts is a check: the client's readiness does not wait
/// for it, [`LspClient::await_check`] does.
#[tokio::test]
async fn readiness_does_not_wait_for_a_check() {
    let scratch = scratch();
    let mut child = Command::new("python3")
        .arg(&scratch.script)
        .arg("slow-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake lsp server");
    let transport = LspTransport::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let client = LspClient::from_transport(server_config(&scratch.script, "slow-check"), transport);
    let uri = path_to_uri(&scratch.document).unwrap();
    client
        .notify(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {
                "uri": uri.as_str(), "languageId": "fake", "version": 1, "text": "bad\n"
            }}),
        )
        .await
        .unwrap();
    let before = client.saving();
    client
        .notify(
            "textDocument/didSave",
            serde_json::json!({"textDocument": {"uri": uri.as_str()}}),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    client.wait_until_ready().await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
    assert!(!client.await_check(before, Duration::from_millis(200)).await);
    assert!(client.await_check(before, Duration::from_secs(10)).await);
    assert_eq!(
        client
            .pushed_diagnostics(uri.as_str())
            .as_deref()
            .map(messages),
        Some(vec!["checked"])
    );

    // A save that starts no check is not waited for past the grace.
    let before = client.saving();
    let started = Instant::now();
    assert!(client.await_check(before, Duration::from_secs(10)).await);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    let _ = child.kill().await;
}
