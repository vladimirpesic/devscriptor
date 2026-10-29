//! `devscriptor call-graph` names the entities at both ends of every
//! relationship, in every format.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// A scanned project in which `main` calls `compute_total`, which calls
/// `add_tax`, and its database.
fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("billing.py"),
        "def compute_total(items):\n    return add_tax(sum(items))\n\n\n\
         def add_tax(amount):\n    return amount * 1.2\n\n\n\
         def main():\n    print(compute_total([1, 2]))\n",
    )
    .unwrap();
    let status = devscriptor(dir.path())
        .args(["scan", "--skip-embeddings"])
        .arg(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the scan failed");
    dir
}

/// `devscriptor --database <dir>/index.db`, isolated from the user's data.
fn devscriptor(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devscriptor"));
    command
        .arg("--database")
        .arg(dir.join("index.db"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"));
    command
}

/// The output of `call-graph --entity compute_total --format <format>`.
fn call_graph(dir: &Path, format: &str) -> String {
    let output = devscriptor(dir)
        .args([
            "call-graph",
            "--entity",
            "compute_total",
            "--format",
            format,
        ])
        .stderr(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "call-graph --format {format} failed"
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn every_format_names_both_ends() {
    let dir = project();

    let json: Value = serde_json::from_str(&call_graph(dir.path(), "json")).unwrap();
    let edges: Vec<(String, String, String)> = json["relationships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            assert!(edge["source"].as_str().is_some_and(|id| !id.is_empty()));
            assert!(
                edge["source_type"]
                    .as_str()
                    .is_some_and(|kind| !kind.is_empty()),
                "{edge}"
            );
            (
                edge["source_name"].as_str().unwrap().to_string(),
                edge["type"].as_str().unwrap().to_string(),
                edge["target_name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for expected in [
        ("compute_total", "calls", "add_tax"),
        ("main", "calls", "compute_total"),
    ] {
        assert!(
            edges.contains(&(
                expected.0.to_string(),
                expected.1.to_string(),
                expected.2.to_string()
            )),
            "{expected:?} in {edges:?}"
        );
    }

    let text = call_graph(dir.path(), "text");
    assert!(
        text.contains("compute_total (function) --calls--> add_tax (function)"),
        "{text}"
    );

    // Every node an edge names is declared once, with its name, and every
    // relationship is listed once.
    let dot = call_graph(dir.path(), "dot");
    let edges: Vec<&str> = dot.lines().filter(|line| line.contains(" -> ")).collect();
    for line in &edges {
        let ends: Vec<&str> = line.split('"').filter(|part| part.len() == 36).collect();
        for id in ends {
            let declared = dot
                .lines()
                .filter(|l| l.trim_start().starts_with(&format!("\"{id}\" [label=")))
                .count();
            assert_eq!(declared, 1, "{id} is declared once: {dot}");
        }
    }
    let unique: std::collections::HashSet<&&str> = edges.iter().collect();
    assert_eq!(unique.len(), edges.len(), "an edge is listed twice: {dot}");
    assert!(dot.contains("[label=\"add_tax (function)\"]"), "{dot}");

    let mermaid = call_graph(dir.path(), "mermaid");
    assert!(mermaid.contains("[\"add_tax (function)\"]"), "{mermaid}");
    assert!(mermaid.contains("-->|calls|"), "{mermaid}");
}
