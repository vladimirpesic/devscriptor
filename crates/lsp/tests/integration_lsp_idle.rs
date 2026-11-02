//! Servers left unused are stopped, and a cap bounds how many run.
//!
//! An idle server is stopped, so it does not keep its memory for the whole
//! session, and started again by the next request, without using up the
//! restarts a crashing server is allowed; with a cap, starting one more server
//! stops the least recently used.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, StartupMode, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{DocumentSymbolsParams, ServerStatusParams};

/// Answers `initialize` and `documentSymbol`. A `documentSymbol` that finds
/// the file named by its second argument removes it, replies, and exits.
const SERVER: &str = r#"
import os, sys, json
language, crash_marker = sys.argv[1], sys.argv[2]

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
        crash = os.path.exists(crash_marker)
        if crash:
            os.remove(crash_marker)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": language, "kind": 12, "range": span, "selectionRange": span}]})
        if crash:
            sys.exit(0)
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// A translator for `languages`, each with a file `main.<language>` in `dir`
/// and the fake server, under `lifecycle`.
fn translator(dir: &Path, languages: &[&str], lifecycle: LifecycleConfig) -> Arc<Translator> {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let servers = languages
        .iter()
        .map(|language| {
            std::fs::write(dir.join(format!("main.{language}")), "thing\n").unwrap();
            LspServerConfig {
                language_id: (*language).into(),
                command: "python3".into(),
                args: vec![
                    script.to_string_lossy().into_owned(),
                    (*language).into(),
                    crash_marker(dir).to_string_lossy().into_owned(),
                ],
                env: HashMap::new(),
                working_dir: None,
                extensions: vec![(*language).into()],
                heuristics: None,
                initialization_options: None,
                settings: ServerSettings::default(),
                configuration: None,
            }
        })
        .collect();
    Arc::new(Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers,
        lifecycle,
    }))
}

fn crash_marker(dir: &Path) -> std::path::PathBuf {
    dir.join("crash")
}

async fn request(translator: &Translator, dir: &Path, language: &str) -> String {
    translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: dir
                .join(format!("main.{language}"))
                .to_string_lossy()
                .into_owned(),
        })
        .await
        .unwrap()
        .symbols[0]
        .name
        .clone()
}

/// `(state, stop_reason, pid)` of `language`'s server.
async fn status(translator: &Translator, language: &str) -> (String, Option<String>, Option<u32>) {
    let status = translator
        .handle_server_status(ServerStatusParams {
            language_id: Some(language.into()),
        })
        .await
        .unwrap();
    let server = &status.servers[0];
    (server.state.clone(), server.stop_reason.clone(), server.pid)
}

/// Wait until `language`'s server is no longer running: exited, or stopped.
async fn wait_until_down(translator: &Translator, language: &str) {
    for _ in 0..200 {
        if status(translator, language).await.0 != "running" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the {language} server never went down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_server_is_stopped_and_started_again_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(
        dir.path(),
        &["fake"],
        LifecycleConfig {
            startup: StartupMode::Lazy,
            idle_timeout_secs: 1,
            maintenance_tick_ms: 200,
            ..LifecycleConfig::default()
        },
    );
    translator.spawn_maintenance();

    assert_eq!(request(&translator, dir.path(), "fake").await, "fake");
    let (state, _, first) = status(&translator, "fake").await;
    assert_eq!(state, "running");

    // Three seconds without a request.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (state, reason, pid) = status(&translator, "fake").await;
    assert_eq!(
        (state.as_str(), reason.as_deref(), pid),
        ("stopped", Some("idle"), None)
    );

    // The next request is served by a new process.
    assert_eq!(request(&translator, dir.path(), "fake").await, "fake");
    let (state, _, second) = status(&translator, "fake").await;
    assert_eq!(state, "running");
    assert_ne!(first, second);

    // The idle stop used none of the restarts a crashing server gets: all
    // three are still allowed.
    for _ in 0..3 {
        std::fs::write(crash_marker(dir.path()), "").unwrap();
        assert_eq!(request(&translator, dir.path(), "fake").await, "fake");
        wait_until_down(&translator, "fake").await;
        assert_eq!(request(&translator, dir.path(), "fake").await, "fake");
    }
    translator.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starting_a_server_beyond_the_cap_stops_the_least_recently_used() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(
        dir.path(),
        &["alpha", "beta"],
        LifecycleConfig {
            max_running_servers: 1,
            ..LifecycleConfig::default()
        },
    );

    assert_eq!(request(&translator, dir.path(), "alpha").await, "alpha");
    assert_eq!(status(&translator, "alpha").await.0, "running");

    assert_eq!(request(&translator, dir.path(), "beta").await, "beta");
    assert_eq!(status(&translator, "beta").await.0, "running");
    let (state, reason, _) = status(&translator, "alpha").await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("stopped", Some("cap"))
    );

    // Starting every applicable server at once also stays within the cap.
    translator.spawn_applicable(None).await.unwrap();
    let running = [
        status(&translator, "alpha").await.0,
        status(&translator, "beta").await.0,
    ];
    assert_eq!(running.iter().filter(|s| *s == "running").count(), 1);
    translator.shutdown().await;
}
