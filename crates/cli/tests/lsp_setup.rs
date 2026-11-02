//! `lsp setup` sets up projects, and installs the servers it is given by name.
//!
//! A server is named by its id (`pyright`) or its language (`bash`), and
//! installed whether a project calls for it or not; a name that is also a
//! directory here is the directory. Run with nothing on PATH, a named server's
//! install stops at its prerequisites, before anything is downloaded.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// `devscriptor lsp <args>` run in `base`, with nothing on PATH: whether it
/// succeeded, and what it printed.
fn lsp(base: &Path, args: &[&str]) -> (bool, String) {
    let empty = base.join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
        .arg("lsp")
        .args(args)
        .current_dir(base)
        .env("HOME", base.join("home"))
        .env("PATH", &empty)
        .env("XDG_CACHE_HOME", base.join("cache"))
        .env("XDG_CONFIG_HOME", base.join("config"))
        .env("XDG_DATA_HOME", base.join("data"))
        .env("DEVSCRIPTOR_DAEMON", "0")
        .output()
        .unwrap();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), printed)
}

#[test]
fn a_server_is_installed_by_its_id_or_its_language() {
    let base = TempDir::new().unwrap();
    for name in ["pyright", "bash"] {
        let (ok, printed) = lsp(base.path(), &["setup", name]);
        assert!(!ok, "{name}: {printed}");
        assert!(
            printed.contains("Missing prerequisite") && printed.contains("Node.js"),
            "{name}: {printed}"
        );
    }

    let (ok, printed) = lsp(base.path(), &["setup", "sourcekit-lsp"]);
    assert!(ok, "{printed}");
    assert!(printed.contains("comes with its toolchain"), "{printed}");

    let (ok, printed) = lsp(base.path(), &["setup", "no-such-server"]);
    assert!(!ok);
    assert!(
        printed.contains("no-such-server is neither a directory nor a language server")
            && printed.contains("rust-analyzer"),
        "{printed}"
    );

    let (ok, printed) = lsp(base.path(), &["setup", "no-such-server", "swift"]);
    assert!(!ok);
    assert!(printed.contains("comes with its toolchain"), "{printed}");
    assert!(
        printed.contains("1 of 2 failed: no-such-server"),
        "{printed}"
    );
}

/// A name that is also a directory here is the directory, which is set up as
/// a project.
#[test]
fn a_directory_of_a_servers_name_is_a_project() {
    let base = TempDir::new().unwrap();
    std::fs::create_dir_all(base.path().join("gopls")).unwrap();
    let (ok, printed) = lsp(base.path(), &["setup", "gopls"]);
    assert!(ok, "{printed}");
    assert!(printed.contains("Project: gopls"), "{printed}");
    assert!(!printed.contains("📦 gopls"), "not installed: {printed}");
}
