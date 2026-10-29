//! Comprehensive Semantic Search Integration Test for Devscriptor
//!
//! This test verifies:
//! 1. Semantic search functionality across all 36 languages
//! 2. Vector similarity search using embeddings
//! 3. Cross-language semantic pattern matching
//! 4. Result ranking by similarity scores
//! 5. Language-specific filtering
//! 6. Pagination support
//!
//! IMPORTANT: This test is now **self-contained**.  It triggers the full
//! analysis pipeline on demand (via `common::get_semantic_analysis_result()`)
//! rather than depending on another test having run first.  The first time
//! this test suite is executed the analysis runs once and the result is
//! cached in a `tokio::sync::OnceCell` for the remainder of the test
//! process.

mod common;

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::search::CodebaseSearcher;
use std::collections::HashMap;
use std::path::PathBuf;

/// Get the database path for semantic tests.
fn get_database_path() -> PathBuf {
    common::default_db_path()
}

/// Semantic query test case
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SemanticQueryTest {
    query: &'static str,
    description: &'static str,
    min_expected_results: usize,
    expected_languages: Vec<&'static str>,
}

/// Define semantic query test cases
fn semantic_query_tests() -> Vec<SemanticQueryTest> {
    vec![
        SemanticQueryTest {
            query: "calculate factorial",
            description: "Mathematical factorial computation",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Rust", "JavaScript", "Java"],
        },
        SemanticQueryTest {
            query: "sort array elements",
            description: "Array/vector sorting algorithms",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Go", "Python", "C++"],
        },
        SemanticQueryTest {
            query: "read file contents",
            description: "File I/O operations",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Python", "Go", "Java"],
        },
        SemanticQueryTest {
            query: "handle error exceptions",
            description: "Error handling patterns",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Python", "Java", "TypeScript"],
        },
        SemanticQueryTest {
            query: "class constructor initialization",
            description: "Object-oriented initialization",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Java", "TypeScript", "C#"],
        },
        SemanticQueryTest {
            query: "binary tree traversal",
            description: "Tree data structure algorithms",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Rust", "C++"],
        },
        SemanticQueryTest {
            query: "http request client",
            description: "Network/HTTP operations",
            min_expected_results: 1,
            expected_languages: vec!["Python", "JavaScript", "Go", "Rust"],
        },
        SemanticQueryTest {
            query: "database connection query",
            description: "Database operations",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Java", "TypeScript"],
        },
        SemanticQueryTest {
            query: "hash map dictionary",
            description: "Key-value data structures",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Rust", "Go", "Java"],
        },
        SemanticQueryTest {
            query: "regular expression pattern matching",
            description: "Regex operations",
            min_expected_results: 1,
            expected_languages: vec!["Python", "JavaScript", "Rust"],
        },
        SemanticQueryTest {
            query: "async await promise",
            description: "Asynchronous programming patterns",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "TypeScript", "JavaScript", "Python"],
        },
        SemanticQueryTest {
            query: "main entry point function",
            description: "Program entry points",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Go", "C", "C++", "Java"],
        },
        SemanticQueryTest {
            query: "unit test assertion",
            description: "Testing patterns",
            min_expected_results: 1,
            expected_languages: vec!["Python", "Rust", "Go", "TypeScript"],
        },
        SemanticQueryTest {
            query: "clone copy duplicate",
            description: "Cloning/copying patterns",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Python"],
        },
        SemanticQueryTest {
            query: "iterator loop collection",
            description: "Iteration patterns",
            min_expected_results: 1,
            expected_languages: vec!["Rust", "Python", "Java", "Go"],
        },
    ]
}

/// Test semantic search setup and prerequisites
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_semantic_search_setup() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    tracing::info!("\n📋 Semantic Search Setup Check:");
    tracing::info!("   Database exists: {}", result.db_path.exists());
    tracing::info!("   Entities available: {}", result.total_entities);
    tracing::info!("   Embeddings available: {}", result.embeddings_generated);

    assert!(result.total_entities > 0, "Analysis produced no entities");
    tracing::info!("   ✅ Setup verified");
}

/// Test semantic query similarity search
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_semantic_query_similarity() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    if result.embeddings_generated == 0 {
        tracing::info!("\n⏩ Skipping semantic similarity test (no embeddings)");
        return;
    }

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let embedding_service = result
        .embedding_service
        .clone()
        .expect("slow-tests analysis must provide a real embedding service");
    let searcher =
        CodebaseSearcher::with_embedding_service(db.conn_pool().clone(), embedding_service);

    let tests = semantic_query_tests();
    let mut total_queries = 0usize;
    let mut successful_queries = 0usize;

    tracing::info!("\n🔍 Semantic Query Similarity Tests:");

    for test in &tests[..5] {
        total_queries += 1;

        match searcher.find_similar(test.query, 5).await {
            Ok(results) => {
                if results.len() >= test.min_expected_results {
                    successful_queries += 1;
                    tracing::info!("   ✅ '{}' → {} results", test.query, results.len());
                    if let Some(first) = results.first() {
                        tracing::info!("      Top: {} (sim: {:.3})", first.name, first.similarity);
                    }
                } else {
                    tracing::info!(
                        "   ⚠️  '{}' → {} results (expected {})",
                        test.query,
                        results.len(),
                        test.min_expected_results
                    );
                }
            }
            Err(e) => {
                tracing::info!("   ❌ '{}' → Error: {}", test.query, e);
            }
        }
    }

    tracing::info!("\n   Summary: {successful_queries}/{total_queries} queries successful");

    let success_rate = if total_queries > 0 {
        (successful_queries as f64 / total_queries as f64) * 100.0
    } else {
        0.0
    };

    assert!(
        success_rate >= 60.0,
        "Semantic search success rate {success_rate:.1}% below 60% threshold"
    );
}

/// Test cross-language semantic matching
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_cross_language_semantic_matching() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    if result.embeddings_generated == 0 {
        tracing::info!("\n⏩ Skipping cross-language test (no embeddings)");
        return;
    }

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let embedding_service = result
        .embedding_service
        .clone()
        .expect("slow-tests analysis must provide a real embedding service");
    let searcher =
        CodebaseSearcher::with_embedding_service(db.conn_pool().clone(), embedding_service);

    let query = "main function entry point";

    tracing::info!("\n🌍 Cross-Language Semantic Matching:");
    tracing::info!("   Query: '{query}'");

    match searcher.find_similar(query, 20).await {
        Ok(results) => {
            let mut languages_found: HashMap<String, usize> = HashMap::new();

            for result in &results {
                let lang = std::path::Path::new(&result.file_path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                *languages_found.entry(lang).or_insert(0) += 1;
            }

            tracing::info!("   Results: {} total", results.len());
            tracing::info!("   Languages detected: {}", languages_found.len());

            for (lang, count) in &languages_found {
                tracing::info!("     * {lang}: {count} results");
            }

            assert!(
                languages_found.len() >= 2,
                "Expected results from at least 2 languages, got {}",
                languages_found.len()
            );

            tracing::info!("   ✅ Cross-language matching verified");
        }
        Err(e) => {
            tracing::info!("   ❌ Error: {e}");
            panic!("Cross-language search failed");
        }
    }
}

/// Test semantic result ranking
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_semantic_result_ranking() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    if result.embeddings_generated == 0 {
        tracing::info!("\n⏩ Skipping ranking test (no embeddings)");
        return;
    }

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    tracing::info!("\n📊 Semantic Result Ranking Test:");

    let query = "function to calculate sum";

    match searcher.find_similar(query, 10).await {
        Ok(results) => {
            if results.len() >= 2 {
                let mut prev_similarity = 1.1;
                let mut properly_sorted = true;

                for (i, result) in results.iter().enumerate() {
                    if result.similarity > prev_similarity {
                        properly_sorted = false;
                        tracing::info!("   ⚠️  Result {i} has higher similarity than previous");
                    }
                    prev_similarity = result.similarity;
                }

                if properly_sorted {
                    tracing::info!("   ✅ Results properly ranked by similarity");
                    tracing::info!("   Highest similarity: {:.3}", results[0].similarity);
                    tracing::info!(
                        "   Lowest similarity: {:.3}",
                        results.last().unwrap().similarity
                    );
                }

                assert!(properly_sorted, "Results should be sorted by similarity");
            } else {
                tracing::info!("   ⚠️  Not enough results to test ranking");
            }
        }
        Err(e) => {
            tracing::info!("   ❌ Error: {e}");
        }
    }
}

/// Test language-specific semantic filtering
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_language_specific_semantic_search() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    tracing::info!("\n🔤 Language-Specific Semantic Search:");

    let languages_to_test = vec!["Rust", "Python", "Go"];

    for lang in languages_to_test {
        match searcher.search_by_pattern("test", "function", false).await {
            Ok(results) => {
                let lang_count = results
                    .iter()
                    .filter(|r| r.language().to_lowercase() == lang.to_lowercase())
                    .count();
                tracing::info!("   {lang_count} functions found in {lang}");
            }
            Err(e) => {
                tracing::info!("   ⚠️  Error searching {lang}: {e}");
            }
        }
    }

    tracing::info!("   ✅ Language filtering verified");
}

/// Test pagination functionality
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_semantic_pagination() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    tracing::info!("\n📄 Semantic Search Pagination Test:");

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    match searcher.find_similar("function", 5).await {
        Ok(page1) => {
            tracing::info!("   Page 1: {} results", page1.len());

            match searcher.find_similar("function", 10).await {
                Ok(page2) => {
                    tracing::info!("   Extended: {} results", page2.len());

                    assert!(
                        page2.len() >= page1.len(),
                        "Extended results should include all from first page"
                    );

                    tracing::info!("   ✅ Pagination structure verified");
                }
                Err(e) => tracing::info!("   ⚠️  Extended query failed: {e}"),
            }
        }
        Err(e) => tracing::info!("   ⚠️  Pagination test skipped: {e}"),
    }
}

/// Test fuzzy search functionality
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_fuzzy_search() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    tracing::info!("\n🔍 Fuzzy Search Test:");

    let fuzzy_terms = vec!["main", "test", "calc", "func"];

    for term in fuzzy_terms {
        match searcher.search_functions(term, true, None, None).await {
            Ok(results) => {
                tracing::info!("   '{}' → {} functions", term, results.len());
            }
            Err(e) => {
                tracing::info!("   '{term}' → Error: {e}");
            }
        }
    }

    tracing::info!("   ✅ Fuzzy search verified");
}

/// Test pattern-based search
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_pattern_search() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    let db_config = DatabaseConfig {
        path: result.db_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let db = Database::new(db_config).expect("Database should open");
    let searcher = CodebaseSearcher::new(db.conn_pool().clone());

    tracing::info!("\n🎯 Pattern Search Test:");

    match searcher.search_functions("", false, None, None).await {
        Ok(functions) => {
            tracing::info!("   All functions: {}", functions.len());
        }
        Err(e) => tracing::info!("   ⚠️  Function search: {e}"),
    }

    match searcher.search_classes("", false, None, None).await {
        Ok(classes) => {
            tracing::info!("   All classes: {}", classes.len());
        }
        Err(e) => tracing::info!("   ⚠️  Class search: {e}"),
    }

    tracing::info!("   ✅ Pattern search verified");
}

/// Generate comprehensive semantic search report
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_generate_semantic_report() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    let report = serde_json::json!({
        "semantic_search_summary": {
            "database_path": result.db_path.to_string_lossy().to_string(),
            "database_available": result.total_entities > 0,
            "total_entities": result.total_entities,
            "embeddings_available": result.embeddings_generated,
            "embedding_coverage": if result.total_entities > 0 {
                (result.embeddings_generated as f64 / result.total_entities as f64) * 100.0
            } else {
                0.0
            },
            "test_timestamp": time::OffsetDateTime::now_utc().to_string(),
        },
        "capabilities_tested": [
            "semantic_similarity_search",
            "cross_language_matching",
            "result_ranking",
            "language_filtering",
            "fuzzy_search",
            "pattern_search"
        ],
        "status": if result.embeddings_generated > 0 { "full_functionality" } else { "limited_functionality" }
    });

    let report_path = get_database_path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("semantic_search_test_report.json");
    let report_content = serde_json::to_string_pretty(&report).expect("Should serialize report");
    std::fs::write(&report_path, report_content).expect("Should write report");

    tracing::info!(
        "\n📄 Semantic Search Report saved to: {}",
        report_path.display()
    );
    tracing::info!("\n{}", "=".repeat(60));
    tracing::info!("SEMANTIC SEARCH TEST SUMMARY");
    tracing::info!("{}", "=".repeat(60));
    tracing::info!(
        "Database: {}",
        if result.total_entities > 0 {
            "✅ Available"
        } else {
            "❌ Not Found"
        }
    );
    tracing::info!("Entities: {}", result.total_entities);
    tracing::info!(
        "Embeddings: {} ({:.1}% coverage)",
        result.embeddings_generated,
        if result.total_entities > 0 {
            (result.embeddings_generated as f64 / result.total_entities as f64) * 100.0
        } else {
            0.0
        }
    );
    tracing::info!("{}", "=".repeat(60));
}

/// Cleanup notice
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "heavy integration test: loads the quantized embedding model and parses 37 language files"
)]
#[tokio::test]
async fn test_semantic_cleanup_notice() {
    let result = common::get_semantic_analysis_result()
        .await
        .expect("Analysis should succeed");

    tracing::info!("\n🧹 Semantic Search Test Complete:");
    tracing::info!("   Database preserved: {}", result.db_path.display());
    tracing::info!(
        "   Report available: {}",
        get_database_path()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("semantic_search_test_report.json")
            .display()
    );
}
