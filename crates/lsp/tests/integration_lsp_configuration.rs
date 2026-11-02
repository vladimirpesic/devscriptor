//! A server gets the settings it is configured with.
//!
//! Metals takes its settings from the client: `autoImportBuilds` has it import
//! a build rather than ask whether to, a question a client without a user can
//! only decline. A server's configuration is sent in
//! `workspace/didChangeConfiguration` once it is initialized, and given to its
//! `workspace/configuration` requests, section by section.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::HoverParams;

/// Records the settings `didChangeConfiguration` brings, asks for its
/// `metals` section once initialized, and answers hover with both.
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

seen = {"pushed": None, "asked": None}
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialized":
        send({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration",
              "params": {"items": [{"section": "metals"}]}})
    elif method == "workspace/didChangeConfiguration":
        seen["pushed"] = msg["params"]["settings"]
    elif method is None and msg.get("id") == 900:
        seen["asked"] = msg.get("result")
    elif "id" not in msg:
        if method == "exit":
            break
    elif method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "textDocumentSync": 1, "hoverProvider": True}}})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"contents": json.dumps(seen)}})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
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
            configuration: Some(serde_json::json!({"metals": {"autoImportBuilds": "all"}})),
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

#[tokio::test]
async fn a_server_gets_the_settings_it_is_configured_with() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.plain");
    std::fs::write(&file, "one\n").unwrap();
    let translator = translator(dir.path());
    let hover = translator
        .handle_hover(HoverParams {
            file_path: file.to_string_lossy().into_owned(),
            line: 0,
            character: 0,
        })
        .await
        .unwrap();
    let seen: serde_json::Value = serde_json::from_str(&hover.contents).unwrap();
    assert_eq!(
        seen["pushed"],
        serde_json::json!({"metals": {"autoImportBuilds": "all"}})
    );
    assert_eq!(
        seen["asked"],
        serde_json::json!([{"autoImportBuilds": "all"}])
    );
    translator.shutdown().await;
}
