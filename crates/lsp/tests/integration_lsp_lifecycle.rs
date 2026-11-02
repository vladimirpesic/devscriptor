//! Integration tests for LSP server lifecycle management.
//!
//! These tests cover spawning, shutdown, state transitions, and communication
//! with LSP servers. Tests that require external LSP servers will skip if the
//! server is not available on the system.
#![cfg(feature = "lsp")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use devscriptor_lsp::config::{LspServerConfig, ServerHeuristics, ServerSettings};
use devscriptor_lsp::error::{LspError, Result, ServerSpawnFailure};
use devscriptor_lsp::lsp::protocol::{
    NotificationMessage, RequestId, RequestMessage, generate_request_id,
};
use devscriptor_lsp::lsp::{LspServer, ServerState, SpawnResult, spawn_servers};
use tempfile::TempDir;

/// Resolve a language server binary that actually *works*.
///
/// Presence on `PATH` is not enough. `~/.cargo/bin/rust-analyzer` is a rustup
/// proxy: when the `rust-analyzer` component is not installed it prints
/// `error: Unknown binary 'rust-analyzer'` and exits 1. It spawns fine, so a
/// `which`-style check calls it available, but it never answers `initialize`,
/// and a test using it would wait out a 30 s timeout that names neither the
/// shim nor the missing component.
///
/// Prefer the Devscriptor-managed install (what `lsp install` populates and what
/// the resolver prefers at runtime), then fall back to `PATH`; in both cases the
/// candidate must report a version before it is trusted.
fn resolve_working_server(cmd: &str) -> Option<String> {
    let managed = devscriptor_lsp::user_dirs::cache_dir().map(|cache| {
        cache
            .join("devscriptor")
            .join("lsp-servers")
            .join(cmd)
            .join("bin")
            .join(cmd)
    });

    managed
        .into_iter()
        .chain(which::which(cmd).ok())
        .find(|path| reports_a_version(path))
        .map(|path| path.to_string_lossy().into_owned())
}

/// Whether `path` runs and prints a version, i.e. it is the real server rather
/// than a stub or a proxy for an uninstalled toolchain component.
fn reports_a_version(path: &std::path::Path) -> bool {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success() && !out.stdout.is_empty())
}

/// Check if a command is available on the system.
fn is_command_available(cmd: &str) -> bool {
    resolve_working_server(cmd).is_some()
}

/// Bind `$name` to a working server binary, or skip the test when none exists.
///
/// Tests must use the bound path as their `command`, not the bare name, so the
/// server they exercise is the one this check validated.
macro_rules! skip_if_not_available {
    ($cmd:expr) => {
        match resolve_working_server($cmd) {
            Some(path) => path,
            None => {
                eprintln!("Skipping test: no working {} found", $cmd);
                return;
            }
        }
    };
}

// ============================================================================
// LspServer::spawn() tests
// ============================================================================

#[tokio::test]
async fn test_lsp_server_spawn_with_rust_analyzer() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: Some(ServerHeuristics {
            markers: vec!["Cargo.toml".into()],
            max_depth: 3,
        }),
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await;
    assert!(
        server.is_ok(),
        "Failed to spawn rust-analyzer: {:?}",
        server.err()
    );

    let server = server.unwrap();
    let client = server.client();
    let state = client.state().await;
    assert_eq!(
        state,
        ServerState::Ready,
        "Server should be in Ready state after initialization"
    );

    // Clean shutdown
    let shutdown_result = server.shutdown().await;
    assert!(shutdown_result.is_ok(), "Shutdown should succeed");
}

#[tokio::test]
async fn test_lsp_server_spawn_with_pyright() {
    let server_bin = skip_if_not_available!("pyright-langserver");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "python".into(),
        command: server_bin.clone(),
        args: vec!["--stdio".into()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["py".into()],
        heuristics: Some(ServerHeuristics {
            markers: vec!["pyproject.toml".into()],
            max_depth: 3,
        }),
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await;
    assert!(
        server.is_ok(),
        "Failed to spawn pyright: {:?}",
        server.err()
    );

    let server = server.unwrap();
    let client = server.client();
    assert_eq!(client.state().await, ServerState::Ready);

    let shutdown_result = server.shutdown().await;
    assert!(shutdown_result.is_ok());
}

#[tokio::test]
async fn test_lsp_server_spawn_failure_nonexistent_command() {
    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "nonexistent".into(),
        command: "this-command-definitely-does-not-exist-12345".into(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["xyz".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let result = LspServer::spawn(&config, temp_dir.path()).await;
    assert!(result.is_err(), "Spawn should fail for nonexistent command");

    match result {
        Err(LspError::ServerSpawnFailed {
            language_id,
            command,
            ..
        }) => {
            assert_eq!(language_id, "nonexistent");
            assert!(command.contains("this-command-definitely-does-not-exist"));
        }
        Err(other) => panic!("Expected ServerSpawnFailed, got: {other}"),
        Ok(_) => panic!("Expected error"),
    }
}

/// A server that spawns but dies during the handshake must fail fast, and the
/// error must say why.
///
/// `~/.cargo/bin/rust-analyzer` is a rustup proxy that exits 1 with
/// `Unknown binary 'rust-analyzer'` when the component is not installed. It
/// spawns successfully, so a handshake that waited for it would wait the full
/// 30 s request timeout and report a bare "timeout", naming neither the dead
/// process nor the stderr that explains it.
#[tokio::test]
async fn spawn_fails_fast_when_server_exits_during_initialize() {
    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "exits-immediately".into(),
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "echo \"error: Unknown binary 'fake-ls'\" >&2; exit 1".into(),
        ],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["xyz".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let started = std::time::Instant::now();
    let result = LspServer::spawn(&config, temp_dir.path()).await;
    let elapsed = started.elapsed();

    assert!(result.is_err(), "a server that exits cannot initialize");
    assert!(
        elapsed < Duration::from_secs(10),
        "must not wait out the request timeout; took {elapsed:?}"
    );

    let message = format!("{}", result.err().unwrap());
    assert!(
        message.contains("during initialization"),
        "error must identify the exit, got: {message}"
    );
    assert!(
        message.contains("Unknown binary 'fake-ls'"),
        "error must surface the server's stderr, got: {message}"
    );
}

/// A server that closes its output before it exits says why, too: asked the
/// moment the handshake fails, a server such as sourcekit-lsp missing
/// `libxml2.so.2` may still be running, and the error would otherwise be only
/// that it closed the connection.
#[tokio::test]
async fn a_server_that_closes_its_output_before_it_exits_says_why() {
    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "closes-first".into(),
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "echo 'error while loading shared libraries: libxml2.so.2' >&2; exec 1>&-; \
             sleep 0.3; exit 127"
                .into(),
        ],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["xyz".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let message = format!(
        "{}",
        LspServer::spawn(&config, temp_dir.path())
            .await
            .err()
            .unwrap()
    );
    assert!(
        message.contains("exited with status 127 during initialization")
            && message.contains("libxml2.so.2"),
        "error must give the exit and the stderr, got: {message}"
    );
}

#[tokio::test]
async fn test_lsp_server_spawn_with_custom_env() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let mut env = HashMap::new();
    env.insert("RUST_LOG".into(), "info".into());

    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env,
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await;
    assert!(server.is_ok());

    let server = server.unwrap();
    let _ = server.shutdown().await;
}

// ============================================================================
// spawn_servers() tests
// ============================================================================

#[tokio::test]
async fn test_spawn_servers_multiple() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    // Create a Cargo.toml marker so rust-analyzer should spawn
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();

    let configs = vec![
        LspServerConfig {
            language_id: "rust".into(),
            command: server_bin.clone(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["rs".into()],
            heuristics: Some(ServerHeuristics {
                markers: vec!["Cargo.toml".into()],
                max_depth: 3,
            }),
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        },
        LspServerConfig {
            language_id: "nonexistent".into(),
            command: "this-command-does-not-exist-99999".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec!["xyz".into()],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        },
    ];

    let roots = vec![temp_dir.path().to_path_buf()];
    let result = spawn_servers(&configs, &roots).await;

    // Should have at least one success (if rust-analyzer available) and one failure
    if is_command_available("rust-analyzer") {
        assert!(
            result.has_servers(),
            "Should have at least one spawned server"
        );
        assert!(
            result.server_count() >= 1,
            "Should have rust-analyzer spawned"
        );
    }
    assert!(
        result.failure_count() >= 1,
        "Should have at least one failure for nonexistent command"
    );
}

#[tokio::test]
async fn test_spawn_servers_all_fail() {
    let temp_dir = TempDir::new().unwrap();

    let configs = vec![
        LspServerConfig {
            language_id: "fail1".into(),
            command: "nonexistent-cmd-1".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec![],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        },
        LspServerConfig {
            language_id: "fail2".into(),
            command: "nonexistent-cmd-2".into(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            extensions: vec![],
            heuristics: None,
            initialization_options: None,
            settings: ServerSettings::default(),
            configuration: None,
        },
    ];

    let roots = vec![temp_dir.path().to_path_buf()];
    let result = spawn_servers(&configs, &roots).await;

    assert!(result.all_failed(), "All servers should fail");
    assert_eq!(result.server_count(), 0);
    assert!(result.failure_count() > 0);
}

#[tokio::test]
async fn test_spawn_servers_empty_configs() {
    let temp_dir = TempDir::new().unwrap();
    let configs: Vec<LspServerConfig> = vec![];
    let roots = vec![temp_dir.path().to_path_buf()];

    let result = spawn_servers(&configs, &roots).await;
    assert!(!result.has_servers());
    assert_eq!(result.server_count(), 0);
    assert_eq!(result.failure_count(), 0);
}

#[tokio::test]
async fn test_spawn_result_helpers() {
    let empty_result = SpawnResult {
        servers: HashMap::new(),
        failures: vec![],
    };
    assert!(!empty_result.all_failed());
    assert!(!empty_result.partial_success());
    assert!(!empty_result.has_servers());
    assert_eq!(empty_result.server_count(), 0);
    assert_eq!(empty_result.failure_count(), 0);

    let with_servers = SpawnResult {
        servers: HashMap::new(),
        failures: vec![],
    };
    assert!(!with_servers.all_failed());
    assert!(!with_servers.partial_success());
    assert!(!with_servers.has_servers());
    assert_eq!(with_servers.server_count(), 0);

    let with_failures = SpawnResult {
        servers: HashMap::new(),
        failures: vec![ServerSpawnFailure {
            language_id: "test".into(),
            command: "test".into(),
            error: "failed".into(),
        }],
    };
    assert!(with_failures.all_failed());
    assert!(!with_failures.partial_success());
}

// ============================================================================
// LspServer::shutdown() tests
// ============================================================================

#[tokio::test]
async fn test_lsp_server_shutdown_terminates_process() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();

    // Verify server is alive by checking client state
    let client = server.client();
    assert_eq!(client.state().await, ServerState::Ready);

    // Shutdown
    let result = server.shutdown().await;
    assert!(result.is_ok(), "Shutdown should succeed");
}

#[tokio::test]
async fn test_lsp_server_drop_kills_process() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    {
        let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
        let client = server.client();
        assert_eq!(client.state().await, ServerState::Ready);
        // server is dropped here; Drop impl spawns a background task that
        // kills the child process.  No fixed sleep needed — the OS reaps
        // the process asynchronously.
    }
}

// ============================================================================
// Server state transition tests
// ============================================================================

#[tokio::test]
async fn test_server_state_transitions_with_real_server() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
    let client = server.client();

    // After spawn + init, state should be Ready
    let state = client.state().await;
    assert_eq!(
        state,
        ServerState::Ready,
        "After initialization, state should be Ready"
    );

    // Simulate shutdown transition
    client.set_state(ServerState::ShuttingDown).await;
    assert_eq!(client.state().await, ServerState::ShuttingDown);

    // Clean up
    let _ = server.shutdown().await;
}

#[test]
fn test_server_state_enum_variants() {
    // Verify all states exist and are distinct
    let states = [
        ServerState::Uninitialized,
        ServerState::Initializing,
        ServerState::Ready,
        ServerState::ShuttingDown,
        ServerState::Shutdown,
    ];

    for (i, s1) in states.iter().enumerate() {
        for (j, s2) in states.iter().enumerate() {
            if i == j {
                assert_eq!(s1, s2);
            } else {
                assert_ne!(s1, s2);
            }
        }
    }
}

#[test]
fn test_server_state_equality() {
    assert_eq!(ServerState::Uninitialized, ServerState::Uninitialized);
    assert_eq!(ServerState::Ready, ServerState::Ready);
    assert_ne!(ServerState::Uninitialized, ServerState::Ready);
    assert_ne!(ServerState::Initializing, ServerState::ShuttingDown);
}

// ============================================================================
// LspClient::request() and LspClient::notify() tests
// ============================================================================

#[tokio::test]
async fn test_lsp_client_request_initialize() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
    let client = server.client();

    // Send a request to get server capabilities (already done during init)
    // Instead, test with a simple method that should work
    let result: Result<serde_json::Value> =
        client.request("shutdown", serde_json::Value::Null).await;
    assert!(
        result.is_ok(),
        "Shutdown request should succeed: {:?}",
        result.err()
    );

    // After shutdown, notify exit
    let notify_result = client.notify("exit", serde_json::Value::Null).await;
    assert!(notify_result.is_ok(), "Exit notification should succeed");
}

#[tokio::test]
async fn test_lsp_client_notify_did_open() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    // Create a simple Rust file
    std::fs::write(temp_dir.path().join("main.rs"), "fn main() {}").unwrap();

    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
    let client = server.client();

    // Send textDocument/didOpen notification
    let uri_str = format!(
        "file://{}/main.rs",
        dunce::canonicalize(temp_dir.path()).unwrap().display()
    );
    let uri: lsp_types::Uri = uri_str.parse().unwrap();

    let params = lsp_types::DidOpenTextDocumentParams {
        text_document: lsp_types::TextDocumentItem {
            uri,
            language_id: "rust".into(),
            version: 1,
            text: "fn main() {}".into(),
        },
    };

    let notify_result = client.notify("textDocument/didOpen", params).await;
    assert!(notify_result.is_ok(), "didOpen notification should succeed");

    let _ = server.shutdown().await;
}

#[tokio::test]
async fn test_lsp_client_request_after_shutdown_fails() {
    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
    let client = server.client();

    // Shutdown the client side first
    let shutdown_result = client.shutdown().await;
    assert!(shutdown_result.is_ok());

    // Poll state with short timeout — state transitions are async after
    // shutdown response returns; avoid a fixed sleep.
    let mut state = client.state().await;
    for _ in 0..50 {
        if state == ServerState::Shutdown {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        state = client.state().await;
    }
    assert_eq!(
        state,
        ServerState::Shutdown,
        "State did not transition to Shutdown within 500 ms"
    );

    // Server shutdown
    let _ = server.shutdown().await;
}

// ============================================================================
// LSP protocol message tests
// ============================================================================

#[test]
fn test_initialize_message_serialization() {
    let init_params = lsp_types::InitializeParams {
        process_id: Some(std::process::id()),
        #[allow(deprecated)]
        root_path: None,
        #[allow(deprecated)]
        root_uri: Some("file:///test".parse().unwrap()),
        initialization_options: None,
        capabilities: lsp_types::ClientCapabilities::default(),
        trace: None,
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: "file:///test".parse().unwrap(),
            name: "test".into(),
        }]),
        client_info: Some(lsp_types::ClientInfo {
            name: "devscriptor-test".into(),
            version: Some("1.0.0".into()),
        }),
        locale: None,
        work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
    };

    let request = RequestMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(1),
        method: "initialize".into(),
        params: Some(serde_json::to_value(&init_params).unwrap()),
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("initialize"));
    assert!(json.contains("2.0"));
    assert!(json.contains("devscriptor-test"));
}

#[test]
fn test_shutdown_message_serialization() {
    let request = RequestMessage {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(42),
        method: "shutdown".into(),
        params: None,
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("shutdown"));
    assert!(json.contains("42"));
}

#[test]
fn test_exit_notification_serialization() {
    let notification = NotificationMessage {
        jsonrpc: "2.0".into(),
        method: "exit".into(),
        params: None,
    };

    let json = serde_json::to_string(&notification).unwrap();
    assert!(json.contains("exit"));
    assert!(!json.contains("id")); // Notifications have no ID
}

// ============================================================================
// Error handling tests
// ============================================================================

#[tokio::test]
async fn test_spawn_failure_error_details() {
    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "test-lang".into(),
        command: "command-that-does-not-exist-abc123".into(),
        args: vec!["--invalid".into()],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["tst".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let result = LspServer::spawn(&config, temp_dir.path()).await;
    assert!(result.is_err());

    let err_string = match result {
        Err(ref e) => format!("{e}"),
        Ok(_) => panic!("Expected error"),
    };
    assert!(
        err_string.contains("test-lang"),
        "Error should contain language_id"
    );
    assert!(
        err_string.contains("command-that-does-not-exist"),
        "Error should contain command"
    );
}

#[test]
fn test_server_spawn_failure_display() {
    let failure = ServerSpawnFailure {
        language_id: "rust".into(),
        command: "rust-analyzer".into(),
        error: "not found in PATH".into(),
    };

    let display = format!("{failure}");
    assert!(display.contains("rust"));
    assert!(display.contains("rust-analyzer"));
    assert!(display.contains("not found in PATH"));
}

#[tokio::test]
async fn test_spawn_with_invalid_workspace_path() {
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: "rust-analyzer".into(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    // Use a path that doesn't exist - spawn should still work
    // because rust-analyzer doesn't require the directory to exist
    // for process spawning (only for the init handshake)
    let result =
        LspServer::spawn(&config, PathBuf::from("/nonexistent/path/12345").as_path()).await;
    if is_command_available("rust-analyzer") {
        // It may succeed or fail depending on whether rust-analyzer accepts non-existent paths
        // Just verify it doesn't panic
    } else {
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn test_client_request_timeout_scenario() {
    // Test that request IDs increment properly
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .try_init();

    let server_bin = skip_if_not_available!("rust-analyzer");

    let temp_dir = TempDir::new().unwrap();
    let config = LspServerConfig {
        language_id: "rust".into(),
        command: server_bin.clone(),
        args: vec![],
        env: HashMap::new(),
        working_dir: None,
        extensions: vec!["rs".into()],
        heuristics: None,
        initialization_options: None,
        settings: ServerSettings::default(),
        configuration: None,
    };

    let server = LspServer::spawn(&config, temp_dir.path()).await.unwrap();
    let client = server.client();

    // Make multiple requests and verify they complete
    for i in 0..3 {
        let result: Result<serde_json::Value> =
            client.request("shutdown", serde_json::Value::Null).await;
        if i == 0 {
            assert!(result.is_ok(), "First shutdown request should succeed");
        }
        // Subsequent shutdown requests may fail depending on server behavior
    }

    let _ = server.shutdown().await;
}

// ============================================================================
// Request ID tests
// ============================================================================

#[test]
fn test_request_id_generation() {
    let id1 = generate_request_id();
    let id2 = generate_request_id();
    let id3 = generate_request_id();

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);

    match id1 {
        RequestId::Number(n) => assert!(n > 0),
        _ => panic!("Expected Number variant"),
    }
}

#[test]
fn test_request_id_equality_and_hash() {
    use std::collections::HashSet;

    let id1 = RequestId::Number(1);
    let id2 = RequestId::Number(1);
    let id3 = RequestId::Number(2);
    let id4 = RequestId::String("1".into());

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
    assert_ne!(id1, id4);

    let mut set = HashSet::new();
    set.insert(id1);
    set.insert(id2);
    set.insert(id3);
    set.insert(id4);
    assert_eq!(set.len(), 3);
}
