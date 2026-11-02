//! A request for what the server does not offer fails as unsupported.
//!
//! Servers answer a request for a capability they do not declare with an
//! error of their own: `-32601 Unhandled method`, or worse, jdtls's `-32603
//! Internal error`. Where the server declares no capability for the request
//! and it fails, the tool says the server does not support the feature; a
//! server that answers all the same is taken at its word, and the error of a
//! request it does declare is its own, but for `-32601`: sql-language-server
//! declares renames it has no handler for.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{
    DefinitionParams, FoldingRangesParams, HoverParams, ReferencesParams, TypeHierarchyParams,
};

/// Declares hover, definition and references. Hover answers; definition
/// fails; references have no handler; folding ranges, which it does not
/// declare, fail as a server's do; a type hierarchy, which it does not
/// declare either, answers all the same.
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

while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        if msg.get("method") == "exit":
            break
        continue
    method, rid = msg.get("method"), msg["id"]
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"capabilities": {
            "textDocumentSync": 1, "hoverProvider": True, "definitionProvider": True,
            "referencesProvider": True}}})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": rid, "result": {"contents": "a hover"}})
    elif method == "textDocument/definition":
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32603, "message": "no index yet"}})
    elif method == "textDocument/prepareTypeHierarchy":
        send({"jsonrpc": "2.0", "id": rid, "result": []})
    elif method in ("shutdown",):
        send({"jsonrpc": "2.0", "id": rid, "result": None})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "Unhandled method " + method}})
"#;

fn translator(dir: &Path) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
            language_id: "plain".into(),
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["plain".into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

#[tokio::test]
async fn what_the_server_does_not_offer_fails_as_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.plain");
    std::fs::write(&file, "one two\n").unwrap();
    let file_path = file.to_string_lossy().into_owned();
    let translator = translator(dir.path());

    let hover = translator
        .handle_hover(HoverParams {
            file_path: file_path.clone(),
            line: 0,
            character: 1,
        })
        .await
        .unwrap();
    assert_eq!(hover.contents, "a hover");

    let undeclared = translator
        .handle_folding_ranges(FoldingRangesParams {
            file_path: file_path.clone(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        undeclared,
        "the plain language server does not support folding ranges"
    );

    let declared = translator
        .handle_definition(DefinitionParams {
            file_path: file_path.clone(),
            line: 0,
            character: 1,
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(declared.contains("no index yet"), "{declared}");

    let unhandled = translator
        .handle_references(ReferencesParams {
            file_path: file_path.clone(),
            line: 0,
            character: 1,
            include_declaration: true,
        })
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        unhandled,
        "the plain language server does not support references"
    );

    let answered = translator
        .handle_type_hierarchy(TypeHierarchyParams {
            file_path,
            line: 0,
            character: 1,
        })
        .await;
    assert!(answered.is_ok(), "{answered:?}");
    translator.shutdown().await;
}
