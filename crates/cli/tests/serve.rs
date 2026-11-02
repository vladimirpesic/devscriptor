//! One Devscriptor process per workspace: `devscriptor serve` holds a
//! workspace's index, watcher and language servers once and serves every MCP
//! session that connects to its socket, and `devscriptor mcp` relays a
//! client's stdio to it.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

/// A scanned workspace, and the runtime directory its daemon uses.
struct Workspace {
    dir: TempDir,
    /// The runtime directory's parent, short: a socket path has room for
    /// about 100 bytes, which macOS's temporary folders leave too little of.
    run: TempDir,
}

impl Workspace {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("project")).unwrap();
        std::fs::write(
            dir.path().join("project/billing.py"),
            "def compute_total(items):\n    return sum(items)\n",
        )
        .unwrap();
        let run = tempfile::Builder::new()
            .prefix("ds")
            .tempdir_in("/tmp")
            .unwrap();
        let workspace = Self { dir, run };
        let status = workspace
            .command()
            .args(["scan", "--skip-embeddings", "."])
            .current_dir(workspace.root())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the scan failed");
        workspace
    }

    fn root(&self) -> PathBuf {
        dunce::canonicalize(self.dir.path().join("project")).unwrap()
    }

    fn runtime(&self) -> PathBuf {
        self.run.path().join("devscriptor")
    }

    /// `devscriptor --database <workspace>/devscriptor.db`, isolated from the
    /// user's data and runtime directories.
    fn command(&self) -> Command {
        let base = self.dir.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_devscriptor"));
        command
            .arg("--database")
            .arg(base.join("project/devscriptor.db"))
            .env("XDG_RUNTIME_DIR", self.run.path())
            .env("XDG_DATA_HOME", base.join("data"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env_remove("DEVSCRIPTOR_DAEMON")
            .env_remove("DEVSCRIPTOR_LSP_STARTUP");
        command
    }

    /// `devscriptor serve --workspace <root>`, once its socket is there.
    fn serve(&self) -> Daemon {
        let child = self
            .command()
            .args(["serve", "--workspace"])
            .arg(self.root())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let socket = wait_for(Duration::from_secs(30), || self.socket())
            .expect("the daemon's socket appears");
        Daemon { child, socket }
    }

    /// What an MCP client that names its database runs:
    /// `devscriptor --database <db> mcp` in the workspace, with `args`, as an
    /// initialized session over its stdio.
    fn proxy(&self, args: &[&str]) -> Proxy {
        self.proxy_with(args, |_| {})
    }

    /// [`Self::proxy`], with `adjust` applied to the command.
    fn proxy_with(&self, args: &[&str], adjust: impl FnOnce(&mut Command)) -> Proxy {
        let mut command = self.command();
        command
            .arg("mcp")
            .args(args)
            .current_dir(self.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        adjust(&mut command);
        let mut child = command.spawn().unwrap();
        let session = Session::open(
            BufReader::new(child.stdout.take().unwrap()),
            child.stdin.take().unwrap(),
        );
        Proxy { child, session }
    }

    /// The socket in the runtime directory, if there is one.
    fn socket(&self) -> Option<PathBuf> {
        self.runtime_file("sock")
    }

    /// The runtime directory's file with extension `extension`, if any.
    fn runtime_file(&self, extension: &str) -> Option<PathBuf> {
        std::fs::read_dir(self.runtime())
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == extension))
    }

    /// The `devscriptor serve` processes serving this workspace.
    fn daemons(&self) -> Vec<u32> {
        let root = self.root().to_string_lossy().into_owned();
        let listing = Command::new("ps")
            .args(["-eo", "pid=,args="])
            .output()
            .unwrap();
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter(|line| line.contains(" serve --workspace ") && line.contains(&root))
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .collect()
    }

    /// Stop the daemons serving this workspace, and wait until they are gone.
    fn stop_daemons(&self) {
        for pid in self.daemons() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        wait_for(Duration::from_secs(30), || {
            self.daemons().is_empty().then_some(())
        })
        .expect("the daemons stop");
    }
}

impl Drop for Workspace {
    /// A test that fails leaves no daemon behind.
    fn drop(&mut self) {
        for pid in self.daemons() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
}

/// Poll `found` until it gives a value or `limit` passes.
fn wait_for<T>(limit: Duration, mut found: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(value) = found() {
            return Some(value);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A running `devscriptor serve`.
struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    /// An initialized MCP session on the daemon's socket.
    fn session(&self) -> Session<BufReader<UnixStream>, UnixStream> {
        let stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        Session::open(BufReader::new(stream.try_clone().unwrap()), stream)
    }

    /// Stop the daemon as a session manager would, with SIGTERM.
    fn terminate(mut self) -> ExitStatus {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        wait_for(Duration::from_secs(30), || self.child.try_wait().unwrap())
            .expect("the daemon stops")
    }
}

/// A running `devscriptor mcp`, with its session.
struct Proxy {
    child: Child,
    session: Session<BufReader<ChildStdout>, ChildStdin>,
}

impl Proxy {
    /// Close the client's end, as a client exiting does, and wait for the
    /// proxy: what it wrote to stdout after the session, and to stderr.
    fn close(self) -> (ExitStatus, String, String) {
        let Self { mut child, session } = self;
        let Session {
            mut reader, writer, ..
        } = session;
        drop(writer);
        let status = wait_for(Duration::from_secs(30), || child.try_wait().unwrap())
            .expect("the proxy exits once its client closes stdin");
        let mut stdout = String::new();
        reader.read_to_string(&mut stdout).unwrap();
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status, stdout, stderr)
    }
}

/// One MCP session, over a socket or a proxy's stdio.
struct Session<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
}

impl<R: BufRead, W: Write> Session<R, W> {
    /// Initialize a session over `reader` and `writer`.
    fn open(reader: R, writer: W) -> Self {
        let mut session = Self {
            reader,
            writer,
            next_id: 0,
        };
        let info = session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        assert_eq!(info["result"]["serverInfo"]["name"], "devscriptor");
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        session
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.writer, "{message}").unwrap();
        self.writer.flush().unwrap();
    }

    /// Send a request and read its response. Every line read must be a
    /// JSON-RPC message.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.reader.read_line(&mut line).unwrap() > 0,
                "session closed"
            );
            let message: Value =
                serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line:?}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// The JSON a tool answered with.
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} answered {response}"));
        serde_json::from_str(text).unwrap()
    }

    /// The names of the functions `search_functions` finds for `query`.
    fn functions(&mut self, query: &str) -> Vec<String> {
        self.tool("search_functions", json!({"query": query}))["functions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|function| function["name"].as_str().unwrap().to_string())
            .collect()
    }
}

/// Two sessions at once on one daemon answer from its one index, a SIGTERM
/// stops it and removes its socket, and it writes its log, never stdout.
#[test]
fn sessions_share_one_daemon_until_it_is_stopped() {
    let workspace = Workspace::new();
    let mut daemon = workspace.serve();
    let mut first = daemon.session();
    let mut second = daemon.session();
    for session in [&mut first, &mut second] {
        assert_eq!(session.functions("compute_total"), ["compute_total"]);
    }
    let tools = second.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 75);
    drop((first, second));

    let mut stdout = String::new();
    let mut piped = daemon.child.stdout.take().unwrap();
    let status = daemon.terminate();
    assert!(status.success(), "the daemon stops cleanly: {status}");
    piped.read_to_string(&mut stdout).unwrap();
    assert!(stdout.is_empty(), "nothing on stdout: {stdout}");
    assert!(workspace.socket().is_none(), "the socket is removed");
    let log =
        std::fs::read_to_string(workspace.runtime_file("log").expect("the daemon's log")).unwrap();
    assert!(log.contains("Serving"), "{log}");
    assert!(log.contains("Stopping the daemon"), "{log}");
}

/// A second daemon for the same workspace leaves the first one serving.
#[test]
fn a_second_daemon_for_a_workspace_leaves() {
    let workspace = Workspace::new();
    let daemon = workspace.serve();
    let second = workspace
        .command()
        .args(["serve", "--workspace"])
        .arg(workspace.root())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(second.success(), "the second daemon exits at once, with 0");
    let mut session = daemon.session();
    assert_eq!(session.functions("compute_total"), ["compute_total"]);
    drop(session);
    assert!(daemon.terminate().success());
}

/// The runtime directory is the user's alone (0700), and so is the socket
/// (0600): another user can neither reach nor connect to the daemon.
#[test]
fn the_endpoint_is_private_to_its_user() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = Workspace::new();
    let daemon = workspace.serve();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&workspace.runtime()), 0o700);
    assert_eq!(mode(&daemon.socket), 0o600);
    assert!(daemon.terminate().success());
}

/// Sessions are independent: two clients calling tools at the same time each
/// get the answers to their own calls.
#[test]
fn concurrent_sessions_each_get_their_answers() {
    let workspace = Workspace::new();
    let daemon = workspace.serve();
    std::thread::scope(|scope| {
        for query in ["compute_total", "missing_function"] {
            let daemon = &daemon;
            scope.spawn(move || {
                let mut session = daemon.session();
                for _ in 0..5 {
                    let expected: &[&str] = if query == "compute_total" {
                        &["compute_total"]
                    } else {
                        &[]
                    };
                    assert_eq!(session.functions(query), expected);
                }
            });
        }
    });
    assert!(daemon.terminate().success());
}

/// `devscriptor --database <db> mcp` in the workspace starts the workspace's
/// daemon and relays to it, with nothing but JSON-RPC on stdout; the daemon
/// outlives the client.
#[test]
fn a_client_is_served_through_the_workspace_daemon() {
    let workspace = Workspace::new();
    let mut proxy = workspace.proxy(&[]);
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    let tools = proxy.session.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 75);
    assert!(workspace.socket().is_some(), "the daemon listens");
    assert_eq!(workspace.daemons().len(), 1, "one daemon serves");

    let (status, stdout, _) = proxy.close();
    assert!(status.success(), "the proxy exits cleanly: {status}");
    assert!(stdout.is_empty(), "nothing after the session: {stdout}");
    assert_eq!(
        workspace.daemons().len(),
        1,
        "the daemon outlives its client"
    );
    workspace.stop_daemons();
}

/// A daemon that cannot be reached leaves the client served by the proxy's
/// own process, and the reason on its stderr.
#[test]
fn without_a_daemon_the_client_is_served_in_process() {
    let workspace = Workspace::new();
    // A runtime directory that cannot be made: a file stands where it would be.
    let blocked = workspace.dir.path().join("blocked");
    std::fs::write(&blocked, "").unwrap();
    let mut proxy = workspace.proxy_with(&[], |command| {
        command.env("XDG_RUNTIME_DIR", &blocked);
    });
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    let (status, _, stderr) = proxy.close();
    assert!(status.success(), "{status}");
    assert!(stderr.contains("Serving in this process"), "{stderr}");
    assert!(workspace.daemons().is_empty(), "no daemon was started");
}

/// A daemon that leaves at once, here because its runtime directory is too
/// deep for a socket path, is reaped by the proxy that started it, instead of
/// staying a zombie for as long as the proxy runs.
#[test]
fn a_daemon_that_leaves_is_reaped() {
    let workspace = Workspace::new();
    // A socket path has room for about 100 bytes.
    let runtime = workspace.dir.path().join("r".repeat(110));
    let mut proxy = workspace
        .command()
        .arg("mcp")
        .current_dir(workspace.root())
        .env("XDG_RUNTIME_DIR", &runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The daemon's output goes to its log, which says why it left.
    let logs = runtime.join("devscriptor");
    let left = wait_for(Duration::from_secs(10), || {
        let entries = std::fs::read_dir(&logs).ok()?;
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
            .any(|entry| {
                std::fs::read_to_string(entry.path()).is_ok_and(|log| log.contains("cannot listen"))
            })
            .then_some(())
    });
    let reaped = left.and_then(|()| {
        wait_for(Duration::from_secs(5), || {
            children(proxy.id()).is_empty().then_some(())
        })
    });
    let remaining = children(proxy.id());
    let _ = proxy.kill();
    let _ = proxy.wait();
    assert!(left.is_some(), "the daemon started and left");
    assert!(
        reaped.is_some(),
        "the proxy's children, by pid and state: {remaining:?}"
    );
}

/// The children of process `pid`, as `<pid> <state>`: a zombie's state is `Z`.
fn children(pid: u32) -> Vec<String> {
    let listing = Command::new("ps")
        .args(["-o", "pid=,stat=", "--ppid", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .collect()
}

/// `--no-daemon` serves in the proxy's own process, and starts no daemon.
#[test]
fn no_daemon_serves_in_process() {
    let workspace = Workspace::new();
    let mut proxy = workspace.proxy(&["--no-daemon"]);
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    let (status, _, _) = proxy.close();
    assert!(status.success(), "{status}");
    assert!(workspace.socket().is_none(), "no daemon listens");
    assert!(workspace.daemons().is_empty(), "no daemon was started");
}

impl Workspace {
    /// The daemon's sidecar: its pid and version.
    fn sidecar(&self) -> Value {
        let path = self.runtime_file("json").expect("the daemon's sidecar");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}

impl<R: BufRead, W: Write> Session<R, W> {
    /// The pid of the daemon serving the session, as `health_check` names it.
    fn daemon_pid(&mut self) -> Option<u64> {
        self.tool("health_check", json!({}))["daemon_pid"].as_u64()
    }
}

/// Two clients of one workspace are served by one daemon, both answering:
/// `health_check` names the same `daemon_pid`, the one its sidecar records.
#[test]
fn two_clients_share_one_daemon() {
    let workspace = Workspace::new();
    let mut first = workspace.proxy(&[]);
    let mut second = workspace.proxy(&[]);
    let pid = first.session.daemon_pid().expect("served by a daemon");
    assert_eq!(second.session.daemon_pid(), Some(pid));
    assert_eq!(workspace.sidecar()["pid"].as_u64(), Some(pid));
    assert_eq!(workspace.daemons(), [u32::try_from(pid).unwrap()]);
    for proxy in [&mut first, &mut second] {
        assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    }
    assert!(first.close().0.success());
    assert!(second.close().0.success());
    workspace.stop_daemons();
}

/// A language server answering `initialize`, and `null` to anything else.
const LANGUAGE_SERVER: &str = r#"
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
    if msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {
            "documentSymbolProvider": True}}})
    else:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
"#;

/// Two sessions asking about one language use one language-server process:
/// the daemon's.
#[test]
fn two_sessions_share_one_language_server() {
    let workspace = Workspace::new();
    let script = workspace.dir.path().join("language_server.py");
    std::fs::write(&script, LANGUAGE_SERVER).unwrap();
    let config = workspace.dir.path().join("lsp.toml");
    std::fs::write(
        &config,
        format!(
            "[workspace]\nroots = [\".\"]\n\n\
             [[lsp_servers]]\nlanguage_id = \"python\"\ncommand = \"python3\"\n\
             args = [{script:?}]\nextensions = [\"py\"]\n",
            script = script.to_string_lossy()
        ),
    )
    .unwrap();
    let with_config = |command: &mut Command| {
        command.env("DEVSCRIPTOR_LSP_CONFIG", &config);
    };
    let mut first = workspace.proxy_with(&[], with_config);
    let mut second = workspace.proxy_with(&[], with_config);
    let file = workspace.root().join("billing.py");
    for proxy in [&mut first, &mut second] {
        let response = proxy.session.request(
            "tools/call",
            json!({
                "name": "get_document_symbols",
                "arguments": {"file_path": file.to_string_lossy()}
            }),
        );
        assert!(response.get("result").is_some(), "{response}");
        let status = proxy
            .session
            .tool("get_server_status", json!({"language_id": "python"}));
        assert_eq!(status["servers"][0]["state"], "running", "{status}");
    }
    let script = script.to_string_lossy().into_owned();
    let listing = Command::new("ps").args(["-eo", "args="]).output().unwrap();
    let servers = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter(|line| line.contains(&script))
        .count();
    assert_eq!(servers, 1, "one language server for both sessions");
    assert!(first.close().0.success());
    assert!(second.close().0.success());
    workspace.stop_daemons();
}

/// With `DEVSCRIPTOR_DAEMON_IDLE_SECS=1` the daemon is gone 3 s after its
/// last client disconnects, its socket, sidecar and lock with it.
#[test]
fn an_unused_daemon_stops() {
    let workspace = Workspace::new();
    let mut proxy = workspace.proxy_with(&[], |command| {
        command.env("DEVSCRIPTOR_DAEMON_IDLE_SECS", "1");
    });
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    assert_eq!(workspace.daemons().len(), 1);
    assert!(proxy.close().0.success());
    let gone = wait_for(Duration::from_secs(3), || {
        workspace.daemons().is_empty().then_some(())
    });
    assert!(gone.is_some(), "the daemon stops once unused");
    for extension in ["sock", "json", "lock"] {
        assert!(
            workspace.runtime_file(extension).is_none(),
            "the daemon's .{extension} is removed"
        );
    }
}

/// A daemon of another version, simulated by rewriting its sidecar, is
/// stopped by the next client and replaced by one of the client's version.
#[test]
fn a_daemon_of_another_version_is_replaced() {
    let workspace = Workspace::new();
    let mut first = workspace.proxy(&[]);
    let old = first.session.daemon_pid().expect("served by a daemon");
    assert!(first.close().0.success());

    let sidecar = workspace.runtime_file("json").unwrap();
    let mut record = workspace.sidecar();
    record["version"] = json!("0.0.1");
    std::fs::write(&sidecar, serde_json::to_vec(&record).unwrap()).unwrap();

    let mut second = workspace.proxy(&[]);
    let new = second.session.daemon_pid().expect("served by a daemon");
    assert_ne!(new, old, "a new daemon serves");
    assert_eq!(workspace.daemons(), [u32::try_from(new).unwrap()]);
    assert_eq!(
        workspace.sidecar()["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(second.session.functions("compute_total"), ["compute_total"]);
    assert!(second.close().0.success());
    workspace.stop_daemons();
}

/// The proxy's stdout carries the JSON-RPC messages of the session and
/// nothing else, a failing call's answer included.
#[test]
fn the_proxy_writes_only_json_rpc() {
    let workspace = Workspace::new();
    let mut proxy = workspace.proxy(&[]);
    let unknown = proxy.session.request(
        "tools/call",
        json!({"name": "no_such_tool", "arguments": {}}),
    );
    assert!(unknown.get("error").is_some(), "{unknown}");
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    let (status, rest, _) = proxy.close();
    assert!(status.success());
    for line in rest.lines() {
        let message: Value = serde_json::from_str(line).expect("JSON-RPC only");
        assert_eq!(message["jsonrpc"], "2.0");
    }
    workspace.stop_daemons();
}

/// `--no-daemon` runs the server in its own process: it answers from that
/// process, names no daemon, and leaves nothing in the runtime directory.
#[test]
fn no_daemon_behaves_as_before() {
    let workspace = Workspace::new();
    let mut proxy = workspace.proxy(&["--no-daemon"]);
    assert_eq!(proxy.session.daemon_pid(), None);
    assert_eq!(proxy.session.functions("compute_total"), ["compute_total"]);
    let tools = proxy.session.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 75);
    assert!(proxy.close().0.success());
    assert!(workspace.daemons().is_empty());
    assert!(
        std::fs::read_dir(workspace.runtime())
            .map(|entries| entries.count())
            .unwrap_or(0)
            == 0,
        "nothing in the runtime directory"
    );
}

/// A daemon a client starts follows the configuration file the client named
/// with `--config`, given relative to the client's directory.
#[test]
fn a_daemon_follows_the_config_its_client_named() {
    let workspace = Workspace::new();
    let config = workspace.dir.path().join("elsewhere.yaml");
    std::fs::write(&config, "scan:\n  include: [py]\n").unwrap();
    let mut proxy = workspace.proxy(&["--config", "../elsewhere.yaml"]);
    let health = proxy.session.tool("health_check", json!({}));
    assert!(
        health["daemon_pid"].is_u64(),
        "served by a daemon: {health}"
    );
    assert_eq!(
        health["project_config"]["path"],
        dunce::canonicalize(config)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(health["project_config"]["error"], Value::Null);
    drop(proxy);
    workspace.stop_daemons();
}
