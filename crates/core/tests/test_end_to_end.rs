//! End-to-End Integration Tests for Devscriptor
//!
//! These tests verify the complete workflow from parsing to analysis.

use std::sync::Arc;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use devscriptor_core::{
    analysis::{complexity::ComplexityAnalyzer, dead_code::DeadCodeDetector},
    database::{Database, DatabaseConfig},
    entities::{Entity, FileInfo, Function, SourceLocation},
    parser::{ParserConfig, TreeSitterParser},
    search::CodebaseSearcher,
};

/// Test parsing a simple Python file end-to-end
#[tokio::test]
async fn test_parse_python_file() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.py");

    fs::write(
        &file_path,
        r"
def main():
    pass

class TestClass:
    def method(self):
        return 42
",
    )
    .await
    .unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let result = parser.parse_file(&file_path).await.unwrap();

    // Verify entities were extracted with exact counts
    let functions: Vec<_> = result.entities_of_type("function").into_iter().collect();
    let classes: Vec<_> = result.entities_of_type("class").into_iter().collect();

    assert_eq!(
        functions.len(),
        2,
        "Should extract exactly 2 functions (main, method)"
    );
    assert_eq!(classes.len(), 1, "Should extract exactly 1 class");

    // Verify function names
    let func_names: Vec<_> = functions.iter().map(|f| f.name()).collect();
    assert!(func_names.contains(&"main"), "Should find 'main' function");
    assert!(
        func_names.contains(&"method"),
        "Should find 'method' function"
    );

    // Verify class name
    assert_eq!(classes[0].name(), "TestClass", "Should find 'TestClass'");
}

/// Test parsing a Rust file end-to-end
#[tokio::test]
async fn test_parse_rust_file() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.rs");

    fs::write(
        &file_path,
        r#"
fn main() {
    tracing::info!("Hello");
}

pub struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}
"#,
    )
    .await
    .unwrap();

    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let result = parser.parse_file(&file_path).await.unwrap();

    let functions: Vec<_> = result.entities_of_type("function").into_iter().collect();
    let structs: Vec<_> = result.entities_of_type("struct").into_iter().collect();

    assert_eq!(
        functions.len(),
        2,
        "Should extract exactly 2 functions (main, new)"
    );
    assert_eq!(structs.len(), 1, "Should extract exactly 1 struct");

    let func_names: Vec<_> = functions.iter().map(|f| f.name()).collect();
    assert!(func_names.contains(&"main"), "Should find 'main' function");
    assert!(func_names.contains(&"new"), "Should find 'new' function");
    assert_eq!(structs[0].name(), "Point", "Should find 'Point' struct");
}

/// Test database operations with entities
#[tokio::test]
async fn test_database_entity_storage() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let db = Database::new(DatabaseConfig {
        path: db_path.to_string_lossy().to_string(),
        ..Default::default()
    })
    .unwrap();

    // Test entity storage
    let file_id = Uuid::new_v4();
    let test_file = FileInfo {
        id: file_id,
        path: "/test/file.rs".to_string(),
        language: "rust".to_string(),
        size_bytes: 100,
        line_count: 10,
        checksum: "abc123".to_string(),
        last_modified: OffsetDateTime::now_utc(),
        entities: vec![],
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    // Store and verify success
    let result = db.upsert_file(&test_file).await;
    assert!(result.is_ok(), "Should be able to store file info");

    // Verify retrieval by searching for the stored file
    let stats = db.get_stats().await;
    assert!(stats.is_ok(), "Should be able to get database stats");
    let (file_count, _, _) = stats.unwrap();
    assert_eq!(file_count, 1, "Database should contain exactly 1 file");
}

/// Test search functionality end-to-end
#[tokio::test]
async fn test_search_engine_basic() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("search_test.db");

    let db = Database::new(DatabaseConfig {
        path: db_path.to_string_lossy().to_string(),
        ..Default::default()
    })
    .unwrap();

    // Create a file first (required by foreign key constraint)
    let test_file = FileInfo {
        id: Uuid::new_v4(),
        path: "/test/file.rs".to_string(),
        language: "rust".to_string(),
        size_bytes: 100,
        line_count: 5,
        checksum: "abc123".to_string(),
        last_modified: OffsetDateTime::now_utc(),
        entities: vec![],
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    db.upsert_file(&test_file).await.unwrap();

    // Insert a test entity first
    let func_id = Uuid::new_v4();
    let func = Function {
        id: func_id,
        name: "test_function".to_string(),
        file_path: "/test/file.rs".to_string(),
        body: "fn test_function() {}".to_string(),
        location: SourceLocation::new(1, 5, 0, 20),
        language: "rust".to_string(),
        parent_function: None,
        parent_class: None,
        parameters: None,
        return_type: None,
        modifiers: None,
        access_modifiers: None,
        docstring: None,
        decorators: vec![],
        implements: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    let entity = Entity::Function(func);
    db.insert_entities(&[entity]).await.unwrap();

    // Create a searcher after data is inserted
    let pool = db.conn_pool().clone();
    let searcher = CodebaseSearcher::new(pool);

    // Search should find the function
    let results = searcher
        .search_functions("test_function", false, None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "Should find exactly 1 function");
    assert_eq!(
        results[0].name(),
        "test_function",
        "Should find correct function name"
    );
    assert_eq!(
        results[0].id(),
        func_id,
        "Should match the inserted function ID"
    );
}

/// Test dead code analysis workflow
#[tokio::test]
async fn test_dead_code_analysis() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("deadcode_test.db");

    let db = Arc::new(
        Database::new(DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap(),
    );

    // Create a file first (required by foreign key constraint)
    let test_file = FileInfo {
        id: Uuid::new_v4(),
        path: "/project/src/file.rs".to_string(),
        language: "rust".to_string(),
        size_bytes: 100,
        line_count: 3,
        checksum: "abc123".to_string(),
        last_modified: OffsetDateTime::now_utc(),
        entities: vec![],
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    db.upsert_file(&test_file).await.unwrap();

    // Insert a used function
    let used_func = Function {
        id: Uuid::new_v4(),
        name: "used_function".to_string(),
        file_path: "/project/src/file.rs".to_string(),
        body: "fn used_function() {}".to_string(),
        location: SourceLocation::new(1, 1, 0, 22),
        language: "rust".to_string(),
        parent_function: None,
        parent_class: None,
        parameters: None,
        return_type: None,
        modifiers: None,
        access_modifiers: Some("public".to_string()),
        docstring: None,
        decorators: vec![],
        implements: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    // Insert an unused function
    let unused_func = Function {
        id: Uuid::new_v4(),
        name: "unused_function".to_string(),
        file_path: "/project/src/file.rs".to_string(),
        body: "fn unused_function() {}".to_string(),
        location: SourceLocation::new(2, 3, 0, 25),
        language: "rust".to_string(),
        parent_function: None,
        parent_class: None,
        parameters: None,
        return_type: None,
        modifiers: None,
        access_modifiers: Some("private".to_string()),
        docstring: None,
        decorators: vec![],
        implements: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    db.insert_entities(&[Entity::Function(used_func), Entity::Function(unused_func)])
        .await
        .unwrap();

    // Create analyzer and run analysis
    let mut analyzer = DeadCodeDetector::new(db);
    let report = analyzer.analyze().await;

    // Analysis should succeed; with entities in the database the detector
    // can identify candidates even without relationships.
    let report = report.expect("Dead code analysis should complete successfully");
    assert!(
        !report.dead_functions.is_empty() || !report.unused_imports.is_empty(),
        "Should find at least one dead code candidate"
    );
}

/// Test complexity analysis
#[tokio::test]
async fn test_complexity_analysis() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("complexity_test.db");

    let db = Arc::new(
        Database::new(DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap(),
    );

    // Create a file first (required by foreign key constraint)
    let test_file = FileInfo {
        id: Uuid::new_v4(),
        path: "/test/file.rs".to_string(),
        language: "rust".to_string(),
        size_bytes: 300,
        line_count: 20,
        checksum: "def456".to_string(),
        last_modified: OffsetDateTime::now_utc(),
        entities: vec![],
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    db.upsert_file(&test_file).await.unwrap();

    // Insert a complex function with 4 branches
    let complex_func = Function {
        id: Uuid::new_v4(),
        name: "complex_function".to_string(),
        file_path: "/test/file.rs".to_string(),
        body: r"
fn complex_function(x: i32) -> i32 {
    if x > 0 {
        if x > 10 {
            if x > 100 {
                return 100;
            }
            return 10;
        }
        return 1;
    }
    0
}
"
        .to_string(),
        location: SourceLocation::new(1, 20, 0, 300),
        language: "rust".to_string(),
        parent_function: None,
        parent_class: None,
        parameters: None,
        return_type: None,
        modifiers: None,
        access_modifiers: None,
        docstring: None,
        decorators: vec![],
        implements: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    let entity = Entity::Function(complex_func);
    db.insert_entities(&[entity]).await.unwrap();

    // Create analyzer and run analysis
    let analyzer = ComplexityAnalyzer::new();
    let report = analyzer.analyze(db).await;

    // Should complete without errors
    assert!(report.is_ok(), "Complexity analysis should complete");
    let report = report.unwrap();
    assert_eq!(
        report.function_metrics.len(),
        1,
        "Should analyze exactly 1 function"
    );

    // Verify the complex function has elevated cyclomatic complexity
    let metric = &report.function_metrics[0];
    assert!(
        metric.cyclomatic_complexity >= 2,
        "Nested ifs should yield cyclomatic complexity >= 2, got {}",
        metric.cyclomatic_complexity
    );
    assert!(
        metric.lines_of_code > 0,
        "Lines of code should be positive, got {}",
        metric.lines_of_code
    );
}

/// Test complete workflow: parse -> store -> search
#[tokio::test]
async fn test_complete_workflow() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("workflow_test.py");
    let db_path = temp_dir.path().join("workflow.db");

    // Create test file
    fs::write(
        &file_path,
        r"
def calculate_sum(a, b):
    return a + b

class Calculator:
    def __init__(self):
        self.value = 0
    
    def add(self, x):
        self.value += x
        return self.value
",
    )
    .await
    .unwrap();

    // Step 1: Parse the file
    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();
    let parsed = parser.parse_file(&file_path).await.unwrap();

    assert!(
        parsed.entities.len() >= 3,
        "Should parse at least 3 entities (calculate_sum, Calculator, add); found {}",
        parsed.entities.len()
    );

    let func_names: Vec<_> = parsed.entities.iter().map(|e| e.name()).collect();
    assert!(
        func_names.contains(&"calculate_sum"),
        "Should parse calculate_sum"
    );
    assert!(
        func_names.contains(&"Calculator"),
        "Should parse Calculator"
    );
    assert!(func_names.contains(&"add"), "Should parse add method");

    // Step 2: Store in database
    let db = Arc::new(
        Database::new(DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap(),
    );

    // Insert file first (required by foreign key constraint)
    let file_info = FileInfo {
        id: Uuid::new_v4(),
        path: file_path.to_string_lossy().to_string(),
        language: "python".to_string(),
        size_bytes: 200,
        line_count: 10,
        checksum: "workflow123".to_string(),
        last_modified: OffsetDateTime::now_utc(),
        entities: parsed
            .entities
            .iter()
            .map(devscriptor_core::Entity::id)
            .collect(),
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    db.upsert_file(&file_info).await.unwrap();
    db.insert_entities(&parsed.entities).await.unwrap();

    // Step 3: Search for entities
    let pool = db.conn_pool().clone();
    let searcher = CodebaseSearcher::new(pool);

    let results = searcher
        .search_functions("calculate", true, None, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "Should find exactly 1 function matching 'calculate'"
    );
    assert_eq!(
        results[0].name(),
        "calculate_sum",
        "Should find calculate_sum by name"
    );
}

/// Test file discovery with exclusions
#[tokio::test]
async fn test_file_discovery() {
    use devscriptor_core::processing::scanner::{FileFilter, FileScanner};

    let temp_dir = TempDir::new().unwrap();

    // Create test files
    fs::write(temp_dir.path().join("main.py"), "def main(): pass")
        .await
        .unwrap();
    fs::write(temp_dir.path().join("test.js"), "function test() {}")
        .await
        .unwrap();
    fs::create_dir(temp_dir.path().join(".git")).await.unwrap();
    fs::write(temp_dir.path().join(".git/config"), "git config")
        .await
        .unwrap();
    fs::create_dir(temp_dir.path().join("node_modules"))
        .await
        .unwrap();
    fs::write(
        temp_dir.path().join("node_modules/package.js"),
        "module.exports = {}",
    )
    .await
    .unwrap();
    fs::write(temp_dir.path().join("README.md"), "# Test")
        .await
        .unwrap();

    let filter = FileFilter::default();
    let scanner = FileScanner::new(filter);
    let entries = scanner.scan(temp_dir.path(), None).await.unwrap();

    let file_names: Vec<_> = entries
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        file_names.contains(&"main.py".to_string()),
        "Should include main.py"
    );
    assert!(
        file_names.contains(&"test.js".to_string()),
        "Should include test.js"
    );
    assert!(
        !file_names.iter().any(|n| n == "config"),
        "Should exclude .git contents"
    );
    assert!(
        !file_names.iter().any(|n| n == "package.js"),
        "Should exclude node_modules"
    );
    // Note: README.md may be included depending on FileFilter settings
}

/// Test multi-language support
#[tokio::test]
async fn test_multilanguage_parsing() {
    let temp_dir = TempDir::new().unwrap();
    let parser = TreeSitterParser::new(ParserConfig::default()).unwrap();

    // Test Python
    let py_file = temp_dir.path().join("test.py");
    fs::write(&py_file, "def foo(): pass").await.unwrap();
    let py_result = parser.parse_file(&py_file).await.unwrap();
    let py_funcs: Vec<_> = py_result.entities_of_type("function").into_iter().collect();
    assert_eq!(py_funcs.len(), 1, "Should extract 1 Python function");
    assert_eq!(py_funcs[0].name(), "foo", "Should find 'foo' in Python");

    // Test JavaScript
    let js_file = temp_dir.path().join("test.js");
    fs::write(&js_file, "function bar() {}").await.unwrap();
    let js_result = parser.parse_file(&js_file).await.unwrap();
    let js_funcs: Vec<_> = js_result.entities_of_type("function").into_iter().collect();
    assert_eq!(js_funcs.len(), 1, "Should extract 1 JavaScript function");
    assert_eq!(js_funcs[0].name(), "bar", "Should find 'bar' in JavaScript");

    // Test Rust
    let rs_file = temp_dir.path().join("test.rs");
    fs::write(&rs_file, "fn baz() {}").await.unwrap();
    let rs_result = parser.parse_file(&rs_file).await.unwrap();
    let rs_funcs: Vec<_> = rs_result.entities_of_type("function").into_iter().collect();
    assert_eq!(rs_funcs.len(), 1, "Should extract 1 Rust function");
    assert_eq!(rs_funcs[0].name(), "baz", "Should find 'baz' in Rust");

    // Test TypeScript
    let ts_file = temp_dir.path().join("test.ts");
    fs::write(&ts_file, "function qux(): void {}")
        .await
        .unwrap();
    let ts_result = parser.parse_file(&ts_file).await.unwrap();
    let ts_funcs: Vec<_> = ts_result.entities_of_type("function").into_iter().collect();
    assert_eq!(ts_funcs.len(), 1, "Should extract 1 TypeScript function");
    assert_eq!(ts_funcs[0].name(), "qux", "Should find 'qux' in TypeScript");
}

/// `.ts` is parsed by tree-sitter-typescript: the fixture yields the node kinds
/// only that grammar has — `interface_declaration`, `type_alias_declaration`,
/// `enum_declaration`.
#[tokio::test]
async fn test_typescript_grammar_produces_ts_entities() {
    let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_files/test_typescript.ts");
    assert!(ts_path.exists(), "test_typescript.ts not found");

    let lang = devscriptor_core::parser::language_config::extension_to_language("ts");
    assert_eq!(
        lang.as_deref(),
        Some("typescript"),
        ".ts must map to typescript language"
    );

    let parser = TreeSitterParser::new(ParserConfig::default()).expect("parser");
    let parsed = parser.parse_file(&ts_path).await.expect("parse_file");

    assert!(
        parsed.tree.is_some(),
        "TypeScript file must produce a parse tree"
    );

    // Count entity types
    let fn_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::Function(_)))
        .count();
    let class_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::Class(_)))
        .count();
    let iface_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::Interface(_)))
        .count();
    let enum_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::Enum(_)))
        .count();
    let type_alias_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::TypeAlias(_)))
        .count();

    // The fixture has ≥ 5 functions, ≥ 4 classes (including abstract), ≥ 5 interfaces,
    // ≥ 3 enums (Color/Priority/Direction), ≥ 3 type aliases (Name/Age/ID)
    assert!(fn_count >= 5, "expected ≥ 5 TS functions, got {fn_count}");
    assert!(
        class_count >= 4,
        "expected ≥ 4 TS classes, got {class_count}"
    );
    assert!(
        iface_count >= 4,
        "expected ≥ 4 TS interfaces, got {iface_count}"
    );
    assert!(enum_count >= 3, "expected ≥ 3 TS enums, got {enum_count}");
    assert!(
        type_alias_count >= 3,
        "expected ≥ 3 TS type aliases, got {type_alias_count}"
    );
}

/// A `.sh` file is parsed by tree-sitter: Bash is not one of the
/// SIMPLE_FILE_LANGUAGES, which skip parsing (`.gitignore` is).
#[tokio::test]
async fn test_bash_function_extraction() {
    let bash_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_files/test_bash.sh");
    assert!(bash_path.exists(), "test_bash.sh not found");

    let lang = devscriptor_core::parser::language_config::extension_to_language("sh");
    assert_eq!(lang.as_deref(), Some("bash"));

    let parser = TreeSitterParser::new(ParserConfig::default()).expect("parser");
    let parsed = parser.parse_file(&bash_path).await.expect("parse_file");

    assert!(parsed.tree.is_some(), "bash file must produce a parse tree");

    let fn_count = parsed
        .entities
        .iter()
        .filter(|e| matches!(e, devscriptor_core::entities::Entity::Function(_)))
        .count();

    // test_bash.sh has exactly 24 function definitions
    assert!(
        fn_count >= 24,
        "expected ≥ 24 bash functions, got {fn_count}"
    );
}
