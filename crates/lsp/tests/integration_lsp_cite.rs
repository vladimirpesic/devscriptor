//! LSP results carry `cite`, the position counted from 1.
//!
//! LSP positions are 0-based while search results and editors count from 1,
//! and models converting between them cite lines off by one (line 41 for line
//! 42), so each result carries the 1-based `path:line:column` to quote.
#![cfg(feature = "lsp")]

use std::collections::HashMap;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{DocumentSymbolsParams, WorkspaceSymbolParams};

/// Reports one symbol, `answer`, at line 41, character 4 (0-based), both as
/// a document symbol and as a workspace symbol.
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

span = {"start": {"line": 41, "character": 4}, "end": {"line": 41, "character": 10}}
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
            {"name": "answer", "kind": 12, "range": span, "selectionRange": span}]})
    elif method == "workspace/symbol":
        uri = "file:///src/lib.fake"
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "answer", "kind": 12, "location": {"uri": uri, "range": span}}]})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

#[tokio::test]
async fn symbol_results_cite_their_position_counted_from_one() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let file = dir.path().join("lib.fake");
    std::fs::write(&file, "answer\n").unwrap();
    let translator = Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.path().to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
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
        }],
        lifecycle: LifecycleConfig::default(),
    });

    let path = file.to_string_lossy().into_owned();
    let symbols = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: path.clone(),
        })
        .await
        .unwrap()
        .symbols;
    let location = &symbols[0].location;
    assert_eq!(location.cite, format!("{path}:42:5"));
    // The range stays LSP's: 0-based.
    assert_eq!(
        (location.range.start.line, location.range.start.character),
        (41, 4)
    );

    let found = translator
        .handle_workspace_symbol(WorkspaceSymbolParams {
            query: "answer".into(),
        })
        .await
        .unwrap()
        .symbols;
    // `file:///src/lib.fake`, as this platform spells the path.
    let cited = if cfg!(windows) {
        r"\src\lib.fake:42:5"
    } else {
        "/src/lib.fake:42:5"
    };
    assert_eq!(found[0].location.cite, cited);

    // As serialized for the agent.
    let json = serde_json::to_value(&symbols[0]).unwrap();
    assert_eq!(json["location"]["cite"], format!("{path}:42:5"));
    translator.shutdown().await;
}
