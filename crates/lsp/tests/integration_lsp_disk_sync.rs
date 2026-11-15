//! A language server answers from the files as they are on disk.
//!
//! A server holding a document open answers from the text it was sent, and
//! one that read a file it was never sent answers from that read until told
//! the file changed. So before each request the translator sends the open
//! documents that changed on disk again, closes those whose files are gone,
//! and tells the server of the changes on disk noted since its last request
//! (`workspace/didChangeWatchedFiles`). A server that held a deleted file open,
//! or was never told of an edit, would answer about code that is gone.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use devscriptor_lsp::bridge::{DiskChange, Translator, path_to_uri};
use devscriptor_lsp::config::{
    Config, LifecycleConfig, LspServerConfig, ServerSettings, WorkspaceConfig,
};
use devscriptor_lsp::mcp::tools::HoverParams;
use serde_json::Value;

/// A fake language server that writes every message it is sent, one JSON
/// object a line, to the file its first argument names, and answers every
/// request but `initialize` with `null`.
const FAKE_SERVER: &str = r#"
import json, sys

log = open(sys.argv[1], "a", buffering=1)

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
    log.write(json.dumps(msg) + "\n")
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"],
              "result": {"capabilities": {"textDocumentSync": 1, "hoverProvider": True}}})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// A project of `.fake` files served by the fake server, and the log of what
/// the server was sent.
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    translator: Translator,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap().join("project");
        std::fs::create_dir(&root).unwrap();
        for (name, text) in files {
            std::fs::write(root.join(name), text).unwrap();
        }
        let script = dir.path().join("fake_lsp.py");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        let log = dir.path().join("sent.jsonl");
        let mut translator = Translator::new(Config {
            workspace: WorkspaceConfig::default(),
            servers: vec![LspServerConfig {
                language_id: "fake".into(),
                command: "python3".into(),
                args: vec![script.display().to_string(), log.display().to_string()],
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
        translator.set_workspace_roots(vec![]);
        Self {
            _dir: dir,
            root,
            log,
            translator,
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Ask the server about `name`, as any request does.
    async fn ask(&self, name: &str) {
        self.translator
            .handle_hover(HoverParams {
                file_path: self.file(name).display().to_string(),
                line: 0,
                character: 0,
            })
            .await
            .expect("hover");
    }

    /// What the server was sent, in order.
    fn sent(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The messages of `method` the server was sent.
    fn sent_of(&self, method: &str) -> Vec<Value> {
        self.sent()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }

    /// Wait up to five seconds for the server to have been sent `count`
    /// messages of `method`.
    async fn await_sent(&self, method: &str, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sent = self.sent_of(method);
            if sent.len() >= count || Instant::now() > deadline {
                return sent;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn uri(&self, name: &str) -> String {
        path_to_uri(&self.file(name)).unwrap().as_str().to_string()
    }
}

/// The text of the last `didChange` of `uri` among `changes`.
fn changed_text<'a>(changes: &'a [Value], uri: &str) -> Option<&'a str> {
    changes
        .iter()
        .rev()
        .find(|change| change["params"]["textDocument"]["uri"] == uri)
        .and_then(|change| change["params"]["contentChanges"][0]["text"].as_str())
}

fn set_mtime(path: &Path, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// A document the server holds open that changes on disk is sent again
/// before the next request, whichever file that request is about.
#[tokio::test]
async fn an_open_document_changed_on_disk_is_sent_again() {
    let project = Project::new(&[("a.fake", "alpha\n"), ("b.fake", "beta\n")]);
    project.ask("a.fake").await;
    project.ask("b.fake").await;
    assert_eq!(project.await_sent("textDocument/didOpen", 2).await.len(), 2);

    std::fs::write(project.file("a.fake"), "alpha changed\n").unwrap();
    project.ask("b.fake").await;
    let changes = project.await_sent("textDocument/didChange", 1).await;
    assert_eq!(
        changed_text(&changes, &project.uri("a.fake")),
        Some("alpha changed\n")
    );

    // Sent once: the next request finds it current.
    project.ask("b.fake").await;
    assert_eq!(project.sent_of("textDocument/didChange").len(), 1);
    project.translator.shutdown().await;
}

/// A document the server holds open whose file is deleted is closed before
/// the next request, so the server stops answering from it.
#[tokio::test]
async fn an_open_document_whose_file_is_gone_is_closed() {
    let project = Project::new(&[("a.fake", "alpha\n"), ("b.fake", "beta\n")]);
    project.ask("a.fake").await;
    std::fs::remove_file(project.file("a.fake")).unwrap();
    project.ask("b.fake").await;
    let closed = project.await_sent("textDocument/didClose", 1).await;
    assert_eq!(
        closed
            .iter()
            .map(|close| close["params"]["textDocument"]["uri"].clone())
            .collect::<Vec<_>>(),
        [Value::from(project.uri("a.fake"))]
    );
    project.translator.shutdown().await;
}

/// A file written again within the same modification time and at the same
/// size, which its time and size cannot tell from the version sent, is read
/// again and sent when its content differs.
#[tokio::test]
async fn an_open_document_rewritten_in_the_same_second_is_sent_again() {
    let project = Project::new(&[("a.fake", "alpha_v1\n"), ("b.fake", "beta\n")]);
    let file = project.file("a.fake");
    let written = std::fs::metadata(&file).unwrap().modified().unwrap();
    project.ask("a.fake").await;

    std::fs::write(&file, "alpha_v2\n").unwrap();
    set_mtime(&file, written);
    project.ask("b.fake").await;
    let changes = project.await_sent("textDocument/didChange", 1).await;
    assert_eq!(
        changed_text(&changes, &project.uri("a.fake")),
        Some("alpha_v2\n")
    );
    project.translator.shutdown().await;
}

/// The changes on disk noted for the servers reach each server before its
/// next request, once, and only those under its folders; a document it holds
/// open is told of by its text instead.
#[tokio::test]
async fn noted_changes_reach_the_server_before_its_next_request() {
    let project = Project::new(&[("a.fake", "alpha\n"), ("b.fake", "beta\n")]);
    project.ask("a.fake").await;

    project.translator.note_disk_changes([
        (project.file("new.fake"), DiskChange::Created),
        (project.file("c.fake"), DiskChange::Changed),
        (project.file("old.fake"), DiskChange::Deleted),
        (project.file("a.fake"), DiskChange::Changed),
        (PathBuf::from("/elsewhere/d.fake"), DiskChange::Changed),
    ]);
    project.ask("a.fake").await;
    let told = project
        .await_sent("workspace/didChangeWatchedFiles", 1)
        .await;
    assert_eq!(told.len(), 1, "{told:?}");
    let mut changes: Vec<(String, i64)> = told[0]["params"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["uri"].as_str().unwrap().to_string(),
                change["type"].as_i64().unwrap(),
            )
        })
        .collect();
    changes.sort();
    let mut want = vec![
        (project.uri("c.fake"), 2),
        (project.uri("new.fake"), 1),
        (project.uri("old.fake"), 3),
    ];
    want.sort();
    assert_eq!(changes, want);

    project.ask("b.fake").await;
    assert_eq!(project.sent_of("workspace/didChangeWatchedFiles").len(), 1);
    project.translator.shutdown().await;
}

/// The server learns that the client tells it of changes on disk, and that
/// it takes no patterns to watch.
#[tokio::test]
async fn the_client_declares_that_it_tells_of_changes_on_disk() {
    let project = Project::new(&[("a.fake", "alpha\n")]);
    project.ask("a.fake").await;
    let initialize = project.await_sent("initialize", 1).await;
    assert_eq!(
        initialize[0]["params"]["capabilities"]["workspace"]["didChangeWatchedFiles"],
        serde_json::json!({"dynamicRegistration": false, "relativePatternSupport": false})
    );
    project.translator.shutdown().await;
}
