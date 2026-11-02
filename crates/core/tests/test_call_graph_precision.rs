//! Call-edge precision/recall measurement against the labeled corpus.
//!
//! Fixture: `tests/labeled/call_graph_labeled.py` + sidecar JSON.
//!
//! Acceptance: precision ≥ 0.85, recall ≥ 0.70 on the labeled edge set.
//! Noise sources (string literals, comments) must not produce false edges.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::embeddings::{EmbeddingConfig, EmbeddingService, MockEmbeddingProvider};
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use serde::Deserialize;
use tempfile::TempDir;

#[derive(Deserialize)]
struct ExpectedEdge {
    caller: String,
    callee: String,
}

#[derive(Deserialize)]
struct LabeledFixture {
    expected_call_edges: Vec<ExpectedEdge>,
    precision_threshold: f64,
    recall_threshold: f64,
}

async fn run_pipeline_on_labeled(temp: &TempDir) -> devscriptor_core::pipeline::PipelineResult {
    let db_path = temp.path().join("pr_test.db");
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/labeled");

    let config = PipelineConfig {
        database: DatabaseConfig {
            path: db_path.to_string_lossy().to_string(),
            ..DatabaseConfig::default()
        },
        embedding: Some(EmbeddingConfig::default()),
        skip_embeddings: false,
        enable_cross_file: true,
        include_extensions: vec!["py".to_string()],
        ..PipelineConfig::default()
    };

    let db = Arc::new(Database::new(config.database.clone()).expect("db"));
    let svc = Arc::new(EmbeddingService::with_provider(MockEmbeddingProvider::new()));

    CodebasePipeline::with_database_and_service(config, db, Some(svc))
        .expect("pipeline creation must succeed")
        .analyze_directory(&fixture_dir)
        .await
        .expect("pipeline must succeed on labeled fixture")
}

/// Load all (caller_name, callee_name) pairs from the DB for `calls` edges.
async fn load_extracted_edges(db_path: &Path) -> HashSet<(String, String)> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path).expect("DB must open");

    let mut stmt = conn
        .prepare(
            "SELECT src.name, tgt.name
             FROM relationships r
             JOIN entities src ON src.id = r.source_id
             JOIN entities tgt ON tgt.id = r.target_id
             WHERE r.relationship_type = 'calls'",
        )
        .expect("query must prepare");

    stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .expect("query must execute")
    .filter_map(|r| r.ok())
    .collect()
}

#[tokio::test]
async fn test_call_graph_precision_recall() {
    let temp = TempDir::new().expect("temp dir");
    let sidecar_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/labeled/call_graph_labeled.json");

    let fixture: LabeledFixture = {
        let raw = std::fs::read_to_string(&sidecar_path)
            .expect("sidecar JSON must exist at tests/labeled/call_graph_labeled.json");
        serde_json::from_str(&raw).expect("sidecar JSON must parse")
    };

    let expected: HashSet<(String, String)> = fixture
        .expected_call_edges
        .iter()
        .map(|e| (e.caller.clone(), e.callee.clone()))
        .collect();

    let result = run_pipeline_on_labeled(&temp).await;
    let db_path = temp.path().join("pr_test.db");
    let extracted = load_extracted_edges(&db_path).await;

    let tp = extracted.intersection(&expected).count();
    let fp = extracted.len() - tp;
    let fn_ = expected.len() - tp;

    let precision = if extracted.is_empty() {
        0.0
    } else {
        tp as f64 / extracted.len() as f64
    };
    let recall = if expected.is_empty() {
        1.0
    } else {
        tp as f64 / expected.len() as f64
    };

    println!("Call-edge P/R measurement:");
    println!("  Extracted edges : {}", extracted.len());
    println!("  Expected edges  : {}", expected.len());
    println!("  True positives  : {tp}");
    println!("  False positives : {fp}");
    println!("  False negatives : {fn_}");
    println!("  Precision       : {precision:.2}");
    println!("  Recall          : {recall:.2}");
    println!("  Total entities  : {}", result.entity_count);
    println!("  Total rels      : {}", result.relationship_count);

    let missed: Vec<_> = expected.difference(&extracted).collect();
    if !missed.is_empty() {
        println!("  Missed edges (FN):");
        for (src, tgt) in &missed {
            println!("    {src} → {tgt}");
        }
    }

    let spurious: Vec<_> = extracted.difference(&expected).collect();
    if !spurious.is_empty() {
        println!("  Spurious edges (FP):");
        for (src, tgt) in &spurious {
            println!("    {src} → {tgt}");
        }
    }

    assert!(
        precision >= fixture.precision_threshold,
        "Precision {precision:.2} < threshold {:.2}. FP edges: {spurious:?}",
        fixture.precision_threshold
    );
    assert!(
        recall >= fixture.recall_threshold,
        "Recall {recall:.2} < threshold {:.2}. Missed edges: {missed:?}",
        fixture.recall_threshold
    );
}
