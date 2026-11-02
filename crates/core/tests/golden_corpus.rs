//! Golden corpus regression gate.
//!
//! ## Purpose
//! Each fixture in `tests/test_files/` has a sidecar
//! `tests/golden/<file>.expected.json` that records **minimum** entity counts.
//! This test reads every sidecar, parses the matching fixture with the real
//! tree-sitter parser, and asserts that counts meet or exceed the thresholds.
//!
//! ## Regenerating sidecars
//! After intentional extraction improvements, re-run:
//! ```text
//! cargo test --package devscriptor-core --test golden_corpus \
//!     generate_golden_sidecars -- --ignored --nocapture
//! ```
//! Then commit the updated JSON files.
//!
//! ## CI use
//! The regular (non-ignored) tests run in < 30 s with no model required.
//! Wire them into the per-PR CI job alongside unit tests.

use devscriptor_core::{
    entities::Entity,
    parser::{ParserConfig, TreeSitterParser},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Sidecar schema ────────────────────────────────────────────────────────────

/// Minimum thresholds for one fixture file.
/// Using minimums (not exact counts) means improvements never break the gate;
/// only regressions do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenExpected {
    pub file: String,
    pub language: String,
    pub min_functions: usize,
    pub min_classes: usize,
    pub min_enums: usize,
    pub min_interfaces: usize,
    pub min_type_aliases: usize,
    pub min_total_entities: usize,
    /// Observed counts at generation time — informational only, not asserted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<ObservedCounts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedCounts {
    pub functions: usize,
    pub classes: usize,
    pub enums: usize,
    pub interfaces: usize,
    pub type_aliases: usize,
    pub total: usize,
}

// ── Paths ─────────────────────────────────────────────────────────────────────

fn test_files_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_files")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

fn sidecar_path(fixture_name: &str) -> PathBuf {
    golden_dir().join(format!("{fixture_name}.expected.json"))
}

// ── Parser helper ─────────────────────────────────────────────────────────────

struct Counts {
    functions: usize,
    classes: usize,
    enums: usize,
    interfaces: usize,
    type_aliases: usize,
    total: usize,
}

async fn parse_counts(fixture: &Path) -> Counts {
    let parser = TreeSitterParser::new(ParserConfig::default()).expect("parser");
    let parsed = parser.parse_file(fixture).await.expect("parse_file");

    let mut c = Counts {
        functions: 0,
        classes: 0,
        enums: 0,
        interfaces: 0,
        type_aliases: 0,
        total: 0,
    };
    for e in &parsed.entities {
        c.total += 1;
        match e {
            Entity::Function(_) => c.functions += 1,
            Entity::Class(_) => c.classes += 1,
            Entity::Enum(_) => c.enums += 1,
            Entity::Interface(_) => c.interfaces += 1,
            Entity::TypeAlias(_) => c.type_aliases += 1,
            _ => {}
        }
    }
    c
}

// ── Generator (ignored — run explicitly to refresh sidecars) ──────────────────

/// Generate / refresh all sidecar JSON files.
///
/// `CREATE TRIGGER` and `CREATE PROCEDURE` yield no entity in `test_sql.sql`:
/// the SQL grammar cannot parse them and emits `ERROR` nodes instead.
///
/// Thresholds are set to `floor(observed * 0.85)` so minor fluctuations
/// (e.g. from grammar updates) do not immediately break CI, but meaningful
/// regressions (>15% drop) do.
#[ignore = "run explicitly to regenerate golden sidecars after intentional changes"]
#[tokio::test]
async fn generate_golden_sidecars() {
    std::fs::create_dir_all(golden_dir()).unwrap();

    // Map from fixture filename → language key used in language_config.rs
    let fixtures: &[(&str, &str)] = &[
        ("test_bash.sh", "bash"),
        ("test_c.c", "c"),
        ("test_cpp.cpp", "cpp"),
        ("test_csharp.cs", "c_sharp"),
        ("test_cuda.cu", "cuda"),
        ("test_dart.dart", "dart"),
        ("test_fsharp.fs", "fsharp"),
        ("test_go.go", "go"),
        ("test_graphql.gql", "graphql"),
        ("test_html.html", "html"),
        ("test_java.java", "java"),
        ("test_javascript.js", "javascript"),
        ("test_kotlin.kt", "kotlin"),
        ("test_objective_c.m", "objc"),
        ("test_php.php", "php"),
        ("test_python.py", "python"),
        ("test_ruby.rb", "ruby"),
        ("test_rust.rs", "rust"),
        ("test_scala.scala", "scala"),
        ("test_solidity.sol", "solidity"),
        ("test_sql.sql", "sql"),
        ("test_svelte.svelte", "svelte"),
        ("test_swift.swift", "swift"),
        ("test_tsx.tsx", "tsx"),
        ("test_typescript.ts", "typescript"),
        ("test_vb.vb", "vb_net"),
        ("test_vue.vue", "vue"),
    ];

    let mut generated = 0usize;
    for (filename, language) in fixtures {
        let fixture = test_files_dir().join(filename);
        if !fixture.exists() {
            eprintln!("SKIP {filename}: fixture not found");
            continue;
        }

        let c = parse_counts(&fixture).await;

        // Threshold = 85% of observed, minimum 0.
        let floor = |n: usize| (n as f64 * 0.85).floor() as usize;

        let expected = GoldenExpected {
            file: filename.to_string(),
            language: language.to_string(),
            min_functions: floor(c.functions),
            min_classes: floor(c.classes),
            min_enums: floor(c.enums),
            min_interfaces: floor(c.interfaces),
            min_type_aliases: floor(c.type_aliases),
            min_total_entities: floor(c.total),
            observed: Some(ObservedCounts {
                functions: c.functions,
                classes: c.classes,
                enums: c.enums,
                interfaces: c.interfaces,
                type_aliases: c.type_aliases,
                total: c.total,
            }),
        };

        let json = serde_json::to_string_pretty(&expected).unwrap();
        std::fs::write(sidecar_path(filename), &json).unwrap();

        println!(
            "  {filename} [{language}]: total={}, fn={}, class={}, enum={}, iface={}, alias={}",
            c.total, c.functions, c.classes, c.enums, c.interfaces, c.type_aliases
        );
        generated += 1;
    }

    println!(
        "\nGenerated {generated} sidecar files in {}",
        golden_dir().display()
    );
}

// ── Regression gate (runs in CI without --ignored) ───────────────────────────

/// Assert that every sidecar's thresholds are met by the current parser.
/// Skipped gracefully when no sidecar exists (first run before generation).
#[tokio::test]
async fn golden_corpus_regression_gate() {
    let sidecars: Vec<_> = std::fs::read_dir(golden_dir())
        .expect("golden/ dir must exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();

    if sidecars.is_empty() {
        println!("No sidecar files found — run generate_golden_sidecars first.");
        return;
    }

    let mut failures = Vec::new();

    for entry in &sidecars {
        let sidecar_bytes = std::fs::read(entry.path()).unwrap();
        let expected: GoldenExpected = serde_json::from_slice(&sidecar_bytes)
            .unwrap_or_else(|e| panic!("Failed to parse sidecar {}: {e}", entry.path().display()));

        let fixture = test_files_dir().join(&expected.file);
        if !fixture.exists() {
            println!("SKIP {}: fixture not found", expected.file);
            continue;
        }

        let c = parse_counts(&fixture).await;

        let mut errs = Vec::new();
        check(&mut errs, "functions", c.functions, expected.min_functions);
        check(&mut errs, "classes", c.classes, expected.min_classes);
        check(&mut errs, "enums", c.enums, expected.min_enums);
        check(
            &mut errs,
            "interfaces",
            c.interfaces,
            expected.min_interfaces,
        );
        check(
            &mut errs,
            "type_aliases",
            c.type_aliases,
            expected.min_type_aliases,
        );
        check(&mut errs, "total", c.total, expected.min_total_entities);

        if errs.is_empty() {
            println!(
                "  OK  {} [{}]: total={} fn={} class={} enum={} iface={} alias={}",
                expected.file,
                expected.language,
                c.total,
                c.functions,
                c.classes,
                c.enums,
                c.interfaces,
                c.type_aliases,
            );
        } else {
            let msg = format!(
                "{} [{}]: {}",
                expected.file,
                expected.language,
                errs.join(", ")
            );
            println!("  FAIL {msg}");
            failures.push(msg);
        }
    }

    assert!(
        failures.is_empty(),
        "\nGolden corpus regression(s) detected:\n{}",
        failures.join("\n")
    );

    println!("\nAll {} golden fixtures pass.", sidecars.len());
}

fn check(errs: &mut Vec<String>, name: &str, got: usize, min: usize) {
    if got < min {
        errs.push(format!("{name}: got {got} < min {min}"));
    }
}
