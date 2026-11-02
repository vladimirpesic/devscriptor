//! A `.m` file goes to the server for Objective-C only when it is
//! Objective-C: MATLAB uses the extension too.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::DocumentSymbolsParams;

/// Answers `textDocument/documentSymbol` with one class, `Legacy`.
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

span = {"start": {"line": 1, "character": 16}, "end": {"line": 1, "character": 22}}
while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        if msg.get("method") == "exit":
            break
        continue
    if msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "documentSymbolProvider": True, "textDocumentSync": 1}}})
    elif msg.get("method") == "textDocument/documentSymbol":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "Legacy", "kind": 5, "range": span, "selectionRange": span}]})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// A translator running the fake server for `.m` files under `dir`, as the
/// registry's clangd entry routes them.
fn translator(dir: &Path) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
            language_id: "cpp".into(),
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: HashMap::new(),
            working_dir: None,
            extensions: devscriptor_lsp::generated_registry::extensions("clangd")
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect(),
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

/// An Objective-C `.m` is answered by the server; a MATLAB one is told it is
/// not Objective-C, and no server is asked.
#[tokio::test]
async fn an_objective_c_file_is_answered_and_matlab_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let objc = dir.path().join("Legacy.m");
    std::fs::write(
        &objc,
        "#import \"Legacy.h\"\n@implementation Legacy\n- (int)limit { return 30480; }\n@end\n",
    )
    .unwrap();
    let matlab = dir.path().join("plot_limits.m");
    std::fs::write(&matlab, "function plot_limits(x)\n  disp(x);\nend\n").unwrap();
    let translator = translator(dir.path());

    let symbols = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: objc.to_string_lossy().into_owned(),
        })
        .await
        .unwrap()
        .symbols;
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "Legacy");

    let refused = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: matlab.to_string_lossy().into_owned(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("plot_limits.m is not Objective-C (MATLAB uses .m too)"),
        "{refused}"
    );
    translator.shutdown().await;
}
