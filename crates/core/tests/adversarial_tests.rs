//! Adversarial Test Suite for Devscriptor-Core
//!
//! Covers edge cases that could crash, hang, or compromise the system:
//! 1. 0-byte file parsing
//! 2. Malformed UTF-8 handling
//! 3. Circular imports (no infinite loop)
//! 4. Tree-sitter partial parse (extracts what is parseable)
//! 5. Very deep nesting (stack safety)
//! 6. Path traversal rejection (`validate_file_path`)
//! 7. SQL injection in search queries (parameterized queries)

use devscriptor_core::{
    database::{Database, DatabaseConfig},
    entities::FileInfo,
    parser::{ParserConfig, TreeSitterParser},
    utils::validation::{PathValidationOptions, validate_file_path},
};
use tempfile::TempDir;
use tokio::fs;

// ------------------------------------------------------------------
// 1. 0-byte file parsing
// ------------------------------------------------------------------

#[tokio::test]
async fn test_zero_byte_file_parsing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("empty.py");
    fs::write(&path, b"").await.unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let result = parser.parse_file(&path).await;

    // Should not crash — either succeed with zero/one entities or return a controlled error.
    match result {
        Ok(parsed) => {
            // Empty files may yield 0 entities or 1 (the root file node).
            assert!(
                parsed.entity_count() <= 1,
                "0-byte file should yield at most 1 entity, got {}",
                parsed.entity_count()
            );
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            // Acceptable errors: empty content, parser failure, generic
            assert!(
                msg.contains("empty")
                    || msg.contains("parse")
                    || msg.contains("generic")
                    || msg.contains("failed"),
                "Unexpected error for 0-byte file: {e}"
            );
        }
    }
}

// ------------------------------------------------------------------
// 2. Malformed UTF-8 handling
// ------------------------------------------------------------------

#[tokio::test]
async fn test_malformed_utf8_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("bad_utf8.rs");
    // Invalid UTF-8 sequence: 0x80 0x81 0x82
    fs::write(&path, b"fn main() {\n\x80\x81\x82\n}")
        .await
        .unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let result = parser.parse_file(&path).await;

    match result {
        Ok(parsed) => {
            // Tree-sitter may parse the valid prefix; ensure it doesn't crash.
            assert!(
                parsed.entity_count() <= 10,
                "Malformed UTF-8 should not cause unbounded entity explosion"
            );
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            assert!(
                msg.contains("utf8")
                    || msg.contains("utf-8")
                    || msg.contains("invalid")
                    || msg.contains("io")
                    || msg.contains("parse")
                    || msg.contains("generic"),
                "Unexpected error for malformed UTF-8: {e}"
            );
        }
    }
}

// ------------------------------------------------------------------
// 3. Circular imports (no infinite loop)
// ------------------------------------------------------------------

#[tokio::test]
async fn test_circular_imports_do_not_hang() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Create a.py that imports b, and b.py that imports a.
    fs::write(root.join("a.py"), "import b\n\ndef func_a():\n    pass\n")
        .await
        .unwrap();
    fs::write(root.join("b.py"), "import a\n\ndef func_b():\n    pass\n")
        .await
        .unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();

    // Parse both files; each should complete independently without hanging.
    let res_a = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        parser.parse_file(&root.join("a.py")),
    )
    .await;
    let res_b = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        parser.parse_file(&root.join("b.py")),
    )
    .await;

    assert!(
        res_a.is_ok(),
        "Parsing a.py timed out (possible infinite loop)"
    );
    assert!(
        res_b.is_ok(),
        "Parsing b.py timed out (possible infinite loop)"
    );
}

// ------------------------------------------------------------------
// 4. Tree-sitter partial parse (extracts what is parseable)
// ------------------------------------------------------------------

#[tokio::test]
async fn test_partial_parse_extraction() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("broken.rs");

    // Valid function followed by syntactic garbage.
    let content = r#"
fn valid_function() -> i32 {
    42
}

!!! this is not valid rust syntax @@@

fn another_valid() {}
"#;
    fs::write(&path, content).await.unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let parsed = parser.parse_file(&path).await.unwrap();

    // Should still extract *some* entities from the parseable regions.
    let names: Vec<_> = parsed.entities.iter().map(|e| e.name()).collect();
    assert!(
        names.contains(&"valid_function") || names.contains(&"another_valid"),
        "Partial parse should still extract valid functions. Got: {names:?}"
    );
}

// ------------------------------------------------------------------
// 5. Very deep nesting (stack safety)
// ------------------------------------------------------------------

#[tokio::test]
async fn test_deep_nesting_does_not_stack_overflow() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("deep.py");

    // Build deeply nested Python: 5000 levels of indentation.
    let depth = 5_000;
    let mut content = String::with_capacity(depth * 8);
    for i in 0..depth {
        content.push_str(&" ".repeat(i * 4));
        content.push_str("if True:\n");
    }
    content.push_str(&" ".repeat(depth * 4));
    content.push_str("pass\n");

    fs::write(&path, &content).await.unwrap();

    // A guard against a hang, not a measure of speed: the file is 50 MB, which
    // a debug build parses in seconds here and in tens on a slow runner.
    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(60), parser.parse_file(&path)).await;

    assert!(
        result.is_ok(),
        "Deeply nested file caused timeout (possible stack overflow or hang)"
    );
}

// ------------------------------------------------------------------
// 6. Path traversal rejection (`validate_file_path`)
// ------------------------------------------------------------------

#[tokio::test]
async fn test_path_traversal_rejected() {
    let temp = TempDir::new().unwrap();
    let base = dunce::canonicalize(temp.path()).unwrap();

    // Attempt to escape the temp directory.
    let evil = "../etc/passwd";
    let result = validate_file_path(evil, Some(&base), &PathValidationOptions::default());
    assert!(result.is_err(), "Path traversal should be rejected: {evil}");
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("traversal") || err.to_lowercase().contains("validation"),
        "Error should mention traversal or validation: {err}"
    );
}

#[tokio::test]
async fn test_absolute_path_outside_base_rejected() {
    let temp = TempDir::new().unwrap();
    let base = dunce::canonicalize(temp.path()).unwrap();

    let evil = "/etc/passwd";
    let result = validate_file_path(evil, Some(&base), &PathValidationOptions::default());
    assert!(
        result.is_err(),
        "Absolute path outside base should be rejected: {evil}"
    );
}

// ------------------------------------------------------------------
// 7. SQL injection in search queries (parameterized queries)
// ------------------------------------------------------------------

#[tokio::test]
async fn test_search_query_sql_injection_safe() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("test.db");

    let db = Database::new(DatabaseConfig {
        path: db_path.to_str().unwrap().to_string(),
        ..DatabaseConfig::default()
    })
    .unwrap();

    // Seed a benign entity.
    let file = FileInfo {
        id: uuid::Uuid::new_v4(),
        path: "src/main.rs".to_string(),
        language: "rust".to_string(),
        size_bytes: 100,
        line_count: 10,
        checksum: "abc".to_string(),
        last_modified: time::OffsetDateTime::now_utc(),
        entities: vec![],
        created_at: time::OffsetDateTime::now_utc(),
        updated_at: time::OffsetDateTime::now_utc(),
    };
    let record = devscriptor_core::database::FileRecord {
        file_info: file,
        entities: vec![],
        relationships: vec![],
        refs: vec![],
        imports: vec![],
    };
    db.batch_insert_parsed_files(&[record], None).await.unwrap();

    // Injections that would break naive string concatenation.
    let injections = vec![
        "'; DROP TABLE entities; --",
        "1' OR '1'='1",
        "test%'; DELETE FROM entities WHERE '1'='1",
        "'; INSERT INTO entities_fts(rowid, name, body) VALUES (999, 'hacked', 'hacked'); --",
    ];

    for payload in &injections {
        let result = db.search_entities(payload, 10).await;
        // Should either succeed (returning 0 or benign results) or fail gracefully.
        // The critical requirement: the database must still be intact.
        match result {
            Ok(results) => {
                // No hacked entries should appear.
                for r in &results {
                    assert!(
                        r.name() != "hacked",
                        "SQL injection succeeded: found 'hacked' entity after payload: {payload}"
                    );
                }
            }
            Err(e) => {
                // Errors are fine as long as they're not panic/segfault.
                let msg = e.to_string().to_lowercase();
                assert!(
                    !msg.contains("panic") && !msg.contains("segfault"),
                    "SQL injection caused catastrophic failure: {e}"
                );
            }
        }
    }

    // Final integrity check: the entities table should still exist and have our row.
    let stats = db.get_stats().await.unwrap();
    assert!(
        stats.0 > 0 || stats.1 > 0,
        "Database was corrupted by SQL injection payloads"
    );
}
