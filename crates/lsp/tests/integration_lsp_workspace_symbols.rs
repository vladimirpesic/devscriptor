//! `get_workspace_symbols` asks every running server that offers workspace
//! symbols and merges their answers: in a polyglot workspace
//! typescript-language-server, with no project open, answers every query "No
//! Project", with tsserver's JavaScript stack, so a query for a Rust function
//! asked of it alone would fail.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{DocumentSymbolsParams, WorkspaceSymbolParams};

/// Offers `workspace/symbol`, and lists `teu_of` as the symbol of the file it
/// is opened with. With `answers`, it finds `teu_of` there; with `fails`, it
/// answers as tsserver does without a project: an error whose message ends in
/// a stack.
const SERVER: &str = r#"
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

mode = sys.argv[1]
opened = None
span = {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 9}}
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "textDocument/didOpen":
        opened = msg["params"]["textDocument"]["uri"]
    if "id" not in msg:
        if method == "exit":
            break
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "documentSymbolProvider": True, "workspaceSymbolProvider": True,
            "textDocumentSync": 1}}})
    elif method == "workspace/symbol" and mode == "fails":
        send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": 1, "message":
            "<syntax> TypeScript Server Error (5.9.3)\nNo Project.\nError: No Project.\n"
            "    at Object.ThrowNoProject (typescript.js:186170:11)\n"
            "    at ProjectService.getDefaultProjectForFile (typescript.js:187001:9)"}})
    elif method == "workspace/symbol":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "teu_of", "kind": 12, "location": {"uri": opened, "range": span}}]})
    elif method == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "teu_of", "kind": 12, "range": span, "selectionRange": span}]})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// A translator running the fake server, once per `(language, mode)`, for
/// files of the language's name as their extension under `dir`.
fn translator(dir: &Path, servers: &[(&str, &str)]) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: servers
            .iter()
            .map(|(language, mode)| LspServerConfig {
                language_id: (*language).into(),
                command: "python3".into(),
                args: vec![script.to_string_lossy().into_owned(), (*mode).into()],
                env: HashMap::new(),
                working_dir: None,
                extensions: vec![(*language).into()],
                heuristics: None,
                initialization_options: None,
                settings: ServerSettings::default(),
                configuration: None,
            })
            .collect(),
        lifecycle: LifecycleConfig::default(),
    })
}

/// Start each server by asking it about a file of its language.
async fn start(translator: &Translator, dir: &Path, languages: &[&str]) {
    for language in languages {
        let file = dir.join(format!("main.{language}"));
        std::fs::write(&file, "fn teu_of() {}\n").unwrap();
        translator
            .handle_document_symbols(DocumentSymbolsParams {
                file_path: file.to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
    }
}

/// A server that cannot answer — first among them — leaves the others'
/// symbols, and is named in `note` with its error, without its stack.
#[tokio::test]
async fn every_running_server_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(dir.path(), &[("alpha", "fails"), ("beta", "answers")]);
    start(&translator, dir.path(), &["alpha", "beta"]).await;

    let found = translator
        .handle_workspace_symbol(WorkspaceSymbolParams {
            query: "teu_of".into(),
        })
        .await
        .unwrap();
    assert_eq!(found.symbols.len(), 1, "{found:?}");
    assert_eq!(found.symbols[0].name, "teu_of");
    assert!(found.symbols[0].location.file_path.ends_with("main.beta"));
    let note = found.note.expect("the server that failed is named");
    assert!(
        note.contains("the alpha server: LSP server error 1: <syntax> TypeScript Server Error")
            && note.contains("Error: No Project."),
        "{note}"
    );
    assert!(!note.contains("ThrowNoProject"), "{note}");
    translator.shutdown().await;
}

/// When no server can answer, the call fails, naming each and why.
#[tokio::test]
async fn no_answer_at_all_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let translator = translator(dir.path(), &[("alpha", "fails")]);
    start(&translator, dir.path(), &["alpha"]).await;

    let error = translator
        .handle_workspace_symbol(WorkspaceSymbolParams {
            query: "teu_of".into(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("no language server could answer: the alpha server:"),
        "{error}"
    );
    translator.shutdown().await;
}
