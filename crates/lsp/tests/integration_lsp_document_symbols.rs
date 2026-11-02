//! Where `get_document_symbols` and `get_symbols` put a symbol.
//!
//! To a client that does not say it takes them as a tree, servers list a
//! file's symbols flat, each at its whole declaration: clangd put
//! `luhn_check_digit` at the `int` before it, rust-analyzer a function at its
//! `pub`, and a hover asked there found nothing. Devscriptor takes the tree,
//! where each symbol has its name's range, and flattens it parent first.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::Path;

use devscriptor_lsp::Translator;
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::{DocumentSymbolsParams, Symbol, SymbolsParams};

/// Answers `textDocument/documentSymbol` for the file in [`SOURCE`]. With
/// the argument `honours`, it gives the tree only to a client that declares
/// `hierarchicalDocumentSymbolSupport`, as servers do, and else the flat
/// list, each symbol at its whole declaration; with `nested`, it always gives
/// the tree.
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

def span(line, start, end_line, end):
    return {"start": {"line": line, "character": start},
            "end": {"line": end_line, "character": end}}

def tree():
    amount = {"name": "amount", "kind": 6, "range": span(2, 8, 3, 29),
              "selectionRange": span(2, 12, 2, 18)}
    entry = {"name": "Entry", "kind": 5, "range": span(1, 4, 3, 29),
             "selectionRange": span(1, 10, 1, 15), "children": [amount]}
    return [{"name": "Ledger", "kind": 5, "range": span(0, 0, 3, 29),
             "selectionRange": span(0, 6, 0, 12), "children": [entry]}]

def flat(uri):
    at = lambda r: {"uri": uri, "range": r}
    return [
        {"name": "Ledger", "kind": 5, "location": at(span(0, 0, 3, 29))},
        {"name": "Entry", "kind": 5, "location": at(span(1, 4, 3, 29)),
         "containerName": "Ledger"},
        {"name": "amount", "kind": 6, "location": at(span(2, 8, 3, 29)),
         "containerName": "Entry"},
    ]

mode = sys.argv[1]
takes_tree = False
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
        symbols = ((msg["params"].get("capabilities") or {}).get("textDocument") or {}).get("documentSymbol") or {}
        takes_tree = symbols.get("hierarchicalDocumentSymbolSupport") is True
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "documentSymbolProvider": True, "textDocumentSync": 1}}})
    elif method == "textDocument/documentSymbol":
        uri = msg["params"]["textDocument"]["uri"]
        answer = tree() if mode == "nested" or takes_tree else flat(uri)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": answer})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// The file the server describes, lines and columns as [`SERVER`] gives them.
const SOURCE: &str =
    "class Ledger:\n    class Entry:\n        def amount(self):\n            return self.cents\n";

/// A translator running the fake server in `mode` for `.fake` files under
/// `dir`.
fn translator(dir: &Path, mode: &str) -> Translator {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    Translator::new(Config {
        workspace: WorkspaceConfig {
            roots: vec![dir.to_path_buf()],
            ..WorkspaceConfig::default()
        },
        servers: vec![LspServerConfig {
            language_id: "fake".into(),
            command: "python3".into(),
            args: vec![script.to_string_lossy().into_owned(), mode.into()],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["fake".into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        }],
        lifecycle: LifecycleConfig::default(),
    })
}

/// Each symbol's name, container, and where it starts (0-based).
fn outline(symbols: &[Symbol]) -> Vec<(&str, Option<&str>, (u32, u32))> {
    symbols
        .iter()
        .map(|s| {
            let start = &s.location.range.start;
            (
                s.name.as_str(),
                s.container_name.as_deref(),
                (start.line, start.character),
            )
        })
        .collect()
}

/// Every symbol at its name, under its own name, its parent's its container.
const AT_THEIR_NAMES: [(&str, Option<&str>, (u32, u32)); 3] = [
    ("Ledger", None, (0, 6)),
    ("Entry", Some("Ledger"), (1, 10)),
    ("amount", Some("Entry"), (2, 12)),
];

/// A server that gives the tree only to a client that asks for it puts each
/// symbol at its name.
#[tokio::test]
async fn a_symbol_is_where_its_name_is() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ledger.fake");
    std::fs::write(&file, SOURCE).unwrap();
    let path = file.to_string_lossy().into_owned();
    let translator = translator(dir.path(), "honours");

    let symbols = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: path.clone(),
        })
        .await
        .unwrap()
        .symbols;
    assert_eq!(outline(&symbols), AT_THEIR_NAMES);
    assert_eq!(symbols[2].location.cite, format!("{path}:3:13"));
    assert_eq!(symbols[2].location.file_path, path);

    let page = translator
        .handle_symbols(SymbolsParams {
            file_path: path.clone(),
            offset: 1,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(outline(&page.symbols), AT_THEIR_NAMES[1..2]);
    assert_eq!(page.pagination.total, 3);
    assert!(page.pagination.has_more);
    translator.shutdown().await;
}

/// A tree is flattened parent first, each symbol under its own name with its
/// parent's as its container, as a flat answer names them.
#[tokio::test]
async fn a_tree_is_flattened_as_a_flat_answer_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ledger.fake");
    std::fs::write(&file, SOURCE).unwrap();
    let translator = translator(dir.path(), "nested");

    let symbols = translator
        .handle_document_symbols(DocumentSymbolsParams {
            file_path: file.to_string_lossy().into_owned(),
        })
        .await
        .unwrap()
        .symbols;
    assert_eq!(outline(&symbols), AT_THEIR_NAMES);
    translator.shutdown().await;
}
