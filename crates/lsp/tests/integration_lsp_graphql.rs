//! A GraphQL file without a GraphQL config fails at once.
//!
//! graphql-language-service loads its schema from a GraphQL config
//! (`.graphqlrc`, `graphql.config.*`, …), and started without one it spends
//! seconds returning no symbols. So a GraphQL file with no config above it has
//! no project root, as a Rust file without `Cargo.toml` has none, and the
//! request fails at once with a message saying what is missing.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::generated_registry;
use devscriptor_lsp::mcp::tools::DocumentSymbolsParams;

/// Answers `initialize` and `documentSymbol`, with one symbol.
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

span = {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 11}}
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
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"name": "Query", "kind": 5, "range": span, "selectionRange": span}]})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// The GraphQL server as the registry configures it, run by the fake script.
fn translator(dir: &Path) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    translator_running(dir, "python3", vec![script.to_string_lossy().into_owned()])
}

/// The GraphQL server as the registry configures it, run as `command`.
fn translator_running(dir: &Path, command: &str, args: Vec<String>) -> Translator {
    let id = "graphql-language-service";
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
            language_id: generated_registry::language_id(id).unwrap().into(),
            command: command.into(),
            args,
            env: HashMap::new(),
            working_dir: None,
            extensions: generated_registry::extensions(id)
                .unwrap()
                .iter()
                .map(|e| (*e).to_string())
                .collect(),
            heuristics: generated_registry::heuristics(id),
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

/// A project directory under a repository root, with a schema in it.
fn project(dir: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let project = dir.join("api");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("schema.graphql"), "type Query { id: ID }\n").unwrap();
    project
}

#[tokio::test]
async fn a_graphql_file_without_a_config_fails_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let project = project(dir.path());
    let translator = translator(dir.path());

    let started = Instant::now();
    let error = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: project
                .join("schema.graphql")
                .to_string_lossy()
                .into_owned(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(started.elapsed() < Duration::from_secs(2), "it waited");
    assert!(
        error.starts_with("the graphql language server has no project root above"),
        "{error}"
    );
    assert!(
        error.contains("graphql-language-service needs a GraphQL config at the project root"),
        "{error}"
    );
}

#[tokio::test]
async fn a_graphql_file_under_a_config_gets_its_server() {
    for config in [".graphqlrc", "graphql.config.ts", ".graphqlconfig"] {
        let dir = tempfile::tempdir().unwrap();
        let project = project(dir.path());
        std::fs::write(project.join(config), "schema: schema.graphql\n").unwrap();
        let translator = translator(dir.path());

        let symbols = translator
            .handle_document_symbols(DocumentSymbolsParams {
                file_path: project
                    .join("schema.graphql")
                    .to_string_lossy()
                    .into_owned(),
            })
            .await
            .unwrap_or_else(|e| panic!("with {config}: {e}"))
            .symbols;
        assert_eq!(symbols[0].name, "Query", "with {config}");
        translator.shutdown().await;
    }
}

/// A configured server that cannot start says so: it is not reported as a
/// language without a server.
#[tokio::test]
async fn a_server_that_cannot_start_is_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let project = project(dir.path());
    std::fs::write(project.join(".graphqlrc"), "schema: schema.graphql\n").unwrap();
    let translator = translator_running(dir.path(), "devscriptor-test-no-such-server", vec![]);

    let error = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: project
                .join("schema.graphql")
                .to_string_lossy()
                .into_owned(),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("the graphql language server failed to start for"),
        "{error}"
    );
    assert!(!error.contains("No LSP server configured"), "{error}");
}
