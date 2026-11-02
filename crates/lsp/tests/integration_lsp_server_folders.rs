//! The folders a server started on demand is given.
//!
//! A server is started for the workspace (`lsp_arch_detect`, eager startup) or
//! for the first file asked about; after its idle timeout it is started again
//! for the next file asked about. Started for a file, clangd is rooted at the
//! file's project, not its directory: for `src/luhn.c` of a Makefile project,
//! rooted at `src/`, the compilation database Devscriptor writes would list
//! `src/` alone, without the project's `include/`, and clangd would find no
//! caller of anything its headers declare.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use devscriptor_lsp::bridge::{Translator, path_to_uri};
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerHeuristics, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::DiagnosticsParams;

/// A fake language server that appends the workspace folders of each
/// `initialize` it gets, one JSON line each, to the file named by its first
/// argument, and pushes no diagnostics for an opened document.
const FAKE_SERVER: &str = r#"
import json, sys

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

log = sys.argv[1]
while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        folders = [f["uri"] for f in msg["params"].get("workspaceFolders") or []]
        with open(log, "a") as out:
            out.write(json.dumps(folders) + "\n")
        send({"jsonrpc": "2.0", "id": msg["id"],
              "result": {"capabilities": {"textDocumentSync": 1}}})
    elif method == "textDocument/didOpen":
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
              "params": {"uri": msg["params"]["textDocument"]["uri"], "diagnostics": []}})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// clangd as the registry configures it, run as the fake server.
fn clangd_config(script: &Path, log: &Path) -> LspServerConfig {
    LspServerConfig {
        language_id: "cpp".into(),
        command: "python3".into(),
        args: vec![script.display().to_string(), log.display().to_string()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["c".into()],
        heuristics: Some(ServerHeuristics {
            markers: vec![
                "compile_commands.json".into(),
                "compile_flags.txt".into(),
                ".clangd".into(),
                "CMakeLists.txt".into(),
                "Makefile".into(),
            ],
            max_depth: 3,
        }),
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The workspace folders of each start the fake server logged, as URIs.
fn starts(log: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn uri(path: &Path) -> String {
    path_to_uri(path).unwrap().to_string()
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap()
}

/// Started for a file of the workspace, clangd gets the workspace, as it does
/// started for the workspace; asked then about a file outside its folders, it
/// is started again with that file's directory added, since the compilation
/// database it reads lists only the files of its folders.
#[tokio::test]
async fn clangd_started_for_a_file_knows_its_workspace_and_takes_in_others() {
    let scratch = tempfile::tempdir().unwrap();
    let script = scratch.path().join("fake_lsp.py");
    write(&script, FAKE_SERVER);
    let log = scratch.path().join("starts.jsonl");
    let workspace = scratch.path().join("shop");
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    let workspace = canonical(&workspace);
    write(&workspace.join("services/geo-cpp/Makefile"), "all:\n");
    let luhn = workspace.join("services/geo-cpp/src/luhn.c");
    write(&luhn, "int luhn_check_digit(const char *s);\n");
    let elsewhere = tempfile::tempdir().unwrap();
    let loose = canonical(elsewhere.path()).join("tool/main.c");
    write(&loose, "int main(void) { return 0; }\n");

    let mut translator = Translator::new(Config {
        workspace: WorkspaceConfig::default(),
        servers: vec![clangd_config(&script, &log)],
        lifecycle: LifecycleConfig::default(),
    });
    translator.set_workspace_roots(vec![workspace.clone()]);
    let open = |file: &Path| DiagnosticsParams {
        file_path: file.display().to_string(),
    };

    translator.handle_diagnostics(open(&luhn)).await.unwrap();
    assert_eq!(starts(&log), [vec![uri(&workspace)]]);

    translator.handle_diagnostics(open(&loose)).await.unwrap();
    assert_eq!(
        starts(&log),
        [
            vec![uri(&workspace)],
            vec![uri(&workspace), uri(loose.parent().unwrap())],
        ]
    );
    translator.shutdown().await;
}
