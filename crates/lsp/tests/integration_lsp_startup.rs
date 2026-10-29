//! Language servers start when they are needed, not with the translator.
//!
//! Starting every applicable server before answering `initialize` would cost
//! memory and time for servers most sessions never use — up to about 1.8 GB
//! each — though each also starts on first use. Startup is lazy; eager startup
//! is a setting.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, StartupMode, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{
    DocumentSymbolsParams, ServerStatus, ServerStatusParams, StopServerParams,
    WorkspaceSymbolParams,
};

/// Answers `initialize`, `textDocument/documentSymbol` and `workspace/symbol`
/// with one symbol named after the server's language.
const SERVER: &str = r#"
import sys, json
language = sys.argv[1]

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
            "documentSymbolProvider": True, "workspaceSymbolProvider": True,
            "textDocumentSync": 1}}})
    elif method == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": language, "kind": 12, "range": span, "selectionRange": span}]})
    elif method == "workspace/symbol":
        uri = "file:///" + language
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": language, "kind": 12, "location": {"uri": uri, "range": span}}]})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// The fake server for `language`, serving files with that extension.
fn server(script: &Path, language: &str) -> LspServerConfig {
    LspServerConfig {
        language_id: language.into(),
        command: "python3".into(),
        args: vec![script.to_string_lossy().into_owned(), language.into()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec![language.into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    }
}

/// A workspace holding a file of two languages, both applicable.
fn translator(dir: &Path, startup: StartupMode) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    for language in ["alpha", "beta"] {
        std::fs::write(dir.join(format!("main.{language}")), "thing\n").unwrap();
    }
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![server(&script, "alpha"), server(&script, "beta")],
        lifecycle: LifecycleConfig {
            startup,
            ..LifecycleConfig::default()
        },
    })
}

async fn status(translator: &Translator) -> Vec<ServerStatus> {
    let mut servers = translator
        .handle_server_status(ServerStatusParams { language_id: None })
        .await
        .unwrap()
        .servers;
    servers.sort_by(|a, b| a.language_id.cmp(&b.language_id));
    servers
}

/// `(language, state, stop_reason)` of every configured server.
async fn states(translator: &Translator) -> Vec<(String, String, Option<String>)> {
    status(translator)
        .await
        .into_iter()
        .map(|s| (s.language_id, s.state, s.stop_reason))
        .collect()
}

fn row(language: &str, state: &str, reason: Option<&str>) -> (String, String, Option<String>) {
    (language.into(), state.into(), reason.map(Into::into))
}

#[tokio::test]
async fn a_lazy_translator_starts_only_the_server_a_request_needs() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(dir.path(), StartupMode::Lazy);

    // Nothing runs after construction.
    assert_eq!(
        states(&translator).await,
        [
            row("alpha", "stopped", Some("not started")),
            row("beta", "stopped", Some("not started")),
        ]
    );
    assert!(status(&translator).await.iter().all(|s| s.pid.is_none()));

    // A request about an `alpha` file starts the `alpha` server alone.
    let symbols = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: dir.path().join("main.alpha").to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    assert_eq!(symbols.symbols[0].name, "alpha");
    assert_eq!(
        states(&translator).await,
        [
            row("alpha", "running", None),
            row("beta", "stopped", Some("not started")),
        ]
    );

    // A server stopped on request says so.
    translator
        .handle_stop_server(StopServerParams {
            language_id: "alpha".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        states(&translator).await,
        [
            row("alpha", "stopped", Some("requested")),
            row("beta", "stopped", Some("not started")),
        ]
    );
    translator.shutdown().await;
}

#[tokio::test]
async fn a_lazy_workspace_symbol_search_starts_the_applicable_servers() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(dir.path(), StartupMode::Lazy);

    let found = translator
        .handle_workspace_symbol(WorkspaceSymbolParams {
            query: String::new(),
        })
        .await
        .unwrap();
    // Both servers started, and both are asked.
    let mut names: Vec<&str> = found.symbols.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["alpha", "beta"], "{:?}", found.symbols);
    assert_eq!(
        states(&translator).await,
        [row("alpha", "running", None), row("beta", "running", None)]
    );
    translator.shutdown().await;
}

/// Eager startup starts every applicable server at once, as before.
#[tokio::test]
async fn an_eager_start_runs_every_applicable_server() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(dir.path(), StartupMode::Eager);
    assert_eq!(translator.startup_mode(), StartupMode::Eager);

    translator.spawn_applicable(None).await.unwrap();
    assert_eq!(
        states(&translator).await,
        [row("alpha", "running", None), row("beta", "running", None)]
    );
    // A second call starts nothing new.
    let pids: Vec<Option<u32>> = status(&translator).await.iter().map(|s| s.pid).collect();
    translator.spawn_applicable(None).await.unwrap();
    let again: Vec<Option<u32>> = status(&translator).await.iter().map(|s| s.pid).collect();
    assert_eq!(pids, again);
    translator.shutdown().await;
}
