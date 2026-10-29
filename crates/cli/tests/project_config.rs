//! A project's `devscriptor.yaml` takes effect: `scan` indexes what it
//! scopes, flags outrank it, `--config` names another file, `validate` checks
//! it, and `template apply` and `template analyze` write files the other
//! commands take. A file with a setting that does not exist stops a command.

use std::path::Path;
use std::process::{Command, Output};

use devscriptor_core::database::{Database, DatabaseConfig};
use tempfile::TempDir;

/// `devscriptor <args>` in `dir`, isolated from the user's data.
fn devscriptor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devscriptor"))
        .args(args)
        .current_dir(dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env_remove("DEVSCRIPTOR_EMBEDDING_SELECTION")
        .env_remove("DEVSCRIPTOR_DB_POOL_SIZE")
        .output()
        .unwrap()
}

fn succeeds(dir: &Path, args: &[&str]) -> String {
    let output = devscriptor(dir, args);
    assert!(
        output.status.success(),
        "devscriptor {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A project with a Python, a Rust and a TypeScript file.
fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.py"), "def app():\n    return 1\n").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn lib() -> u8 { 1 }\n").unwrap();
    std::fs::write(
        dir.path().join("web.ts"),
        "export function web(): number { return 1; }\n",
    )
    .unwrap();
    dir
}

/// Which of the project's three files the database `db` holds.
async fn indexed(dir: &Path, db: &str) -> Vec<&'static str> {
    let root = dunce::canonicalize(dir).unwrap();
    let database = Database::new(DatabaseConfig {
        path: dir.join(db).to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    })
    .unwrap();
    let mut found = Vec::new();
    for file in ["app.py", "lib.rs", "web.ts"] {
        let path = root.join(file).to_string_lossy().into_owned();
        if database.file_is_indexed(&path).await.unwrap() {
            found.push(file);
        }
    }
    found
}

#[tokio::test]
async fn scan_takes_the_project_file_and_its_flags_outrank_it() {
    let dir = project();
    std::fs::write(
        dir.path().join("devscriptor.yaml"),
        "scan:\n  include: [py, ts]\n  exclude: [\"web.ts\"]\n",
    )
    .unwrap();

    let report = succeeds(dir.path(), &["--database", "a.db", "scan", "."]);
    assert!(report.contains("devscriptor.yaml"), "{report}");
    assert_eq!(indexed(dir.path(), "a.db").await, ["app.py"]);

    // `--include` and `--exclude` replace the file's lists.
    succeeds(
        dir.path(),
        &[
            "--database",
            "b.db",
            "scan",
            "--include",
            "rs,ts",
            "--exclude",
            "nothing",
            ".",
        ],
    );
    assert_eq!(indexed(dir.path(), "b.db").await, ["lib.rs", "web.ts"]);
}

#[tokio::test]
async fn config_names_another_file() {
    let dir = project();
    std::fs::write(
        dir.path().join("rust-only.yaml"),
        "scan:\n  include: [rs]\n",
    )
    .unwrap();
    succeeds(
        dir.path(),
        &[
            "--config",
            "rust-only.yaml",
            "--database",
            "c.db",
            "scan",
            ".",
        ],
    );
    assert_eq!(indexed(dir.path(), "c.db").await, ["lib.rs"]);

    let output = devscriptor(dir.path(), &["--config", "missing.yaml", "scan", "."]);
    assert!(!output.status.success());
}

/// A setting the schema does not name is an error, not a no-op: `scan` stops
/// and says which.
#[test]
fn a_setting_that_does_not_exist_stops_the_command() {
    let dir = project();
    std::fs::write(
        dir.path().join("devscriptor.yaml"),
        "database:\n  url: sqlite://elsewhere.db\n",
    )
    .unwrap();
    let output = devscriptor(dir.path(), &["scan", "."]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown field `url`"), "{stderr}");
    assert!(!dir.path().join("devscriptor.db").exists());
}

#[test]
fn validate_checks_the_project_file() {
    let dir = project();
    // No file: nothing to validate.
    assert!(!devscriptor(dir.path(), &["validate"]).status.success());

    std::fs::write(
        dir.path().join("devscriptor.yaml"),
        "scan:\n  include: [py, docx]\n  cross_file: light\nembeddings:\n  selection: all\n",
    )
    .unwrap();
    let report = succeeds(dir.path(), &["validate"]);
    assert!(report.contains("Configuration is valid"), "{report}");
    assert!(report.contains(".docx"), "the warning names it: {report}");

    std::fs::write(
        dir.path().join("other.yaml"),
        "scan:\n  max_file_size_mb: 0\n",
    )
    .unwrap();
    assert!(
        !devscriptor(dir.path(), &["validate", "other.yaml"])
            .status
            .success()
    );
    assert!(
        !devscriptor(dir.path(), &["--config", "other.yaml", "validate"])
            .status
            .success()
    );
}

/// `template apply` writes a file `validate` accepts and `scan` follows: the
/// web template takes in TypeScript and Python, and leaves Rust out.
#[tokio::test]
async fn an_applied_template_takes_effect() {
    let dir = project();
    succeeds(
        dir.path(),
        &[
            "template",
            "apply",
            "web_development",
            "-o",
            "devscriptor.yaml",
        ],
    );
    let written = std::fs::read_to_string(dir.path().join("devscriptor.yaml")).unwrap();
    assert!(
        written.starts_with("# Devscriptor project configuration"),
        "{written}"
    );
    succeeds(dir.path(), &["validate"]);
    succeeds(dir.path(), &["--database", "d.db", "scan", "."]);
    assert_eq!(indexed(dir.path(), "d.db").await, ["app.py", "web.ts"]);
}

/// `template analyze` tells the stacks from their build files and writes the
/// configuration they suggest.
#[test]
fn template_analyze_writes_the_suggestion() {
    let dir = project();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let report = succeeds(
        dir.path(),
        &["template", "analyze", ".", "--output", "devscriptor.yaml"],
    );
    assert!(report.contains("Rust"), "{report}");
    let written = std::fs::read_to_string(dir.path().join("devscriptor.yaml")).unwrap();
    assert!(written.contains("- rs"), "{written}");
    assert!(written.contains("target"), "{written}");
    succeeds(dir.path(), &["validate"]);
}
