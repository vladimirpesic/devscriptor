//! `devscriptor doctor` reports what it finds without changing it: a missing
//! database is reported, not created, and entities still without a vector
//! are counted.

use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// `devscriptor <args>` in `dir`, isolated from the user's data; its stdout.
fn devscriptor(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_devscriptor"))
        .args(args)
        .current_dir(dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "devscriptor {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn a_missing_database_is_reported_not_created() {
    let dir = TempDir::new().unwrap();
    let report = devscriptor(dir.path(), &["doctor"]);
    assert!(report.contains("None at"), "{report}");
    assert!(
        !dir.path().join("devscriptor.db").exists(),
        "doctor created a database"
    );
}

#[test]
fn entities_without_a_vector_are_counted() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("billing.py"),
        "def compute_total(items):\n    return sum(items)\n",
    )
    .unwrap();
    devscriptor(dir.path(), &["scan", "--skip-embeddings", "."]);
    let report = devscriptor(dir.path(), &["doctor"]);
    let line = report
        .lines()
        .find(|line| line.contains("Embeddings"))
        .unwrap_or_else(|| panic!("no Embeddings line: {report}"));
    assert!(line.contains("have no vector yet"), "{line}");
    assert!(line.contains("devscriptor embed"), "{line}");
}

/// The project's configuration file is checked, and one that cannot be
/// applied is reported without stopping the other checks.
#[test]
fn the_project_configuration_is_reported() {
    let dir = TempDir::new().unwrap();
    let report = devscriptor(dir.path(), &["doctor"]);
    assert!(report.contains("No devscriptor.yaml here"), "{report}");

    std::fs::write(
        dir.path().join("devscriptor.yaml"),
        "scan:\n  include: [rs]\n",
    )
    .unwrap();
    let report = devscriptor(dir.path(), &["doctor"]);
    assert!(report.contains("devscriptor.yaml — valid"), "{report}");

    std::fs::write(
        dir.path().join("devscriptor.yaml"),
        "database:\n  url: sqlite://x.db\n",
    )
    .unwrap();
    let report = devscriptor(dir.path(), &["doctor"]);
    let line = report
        .lines()
        .find(|line| line.contains("Project config"))
        .unwrap_or_else(|| panic!("no Project config line: {report}"));
    assert!(line.contains("unknown field `url`"), "{line}");
    assert!(report.contains("Embedding model"), "{report}");
}
