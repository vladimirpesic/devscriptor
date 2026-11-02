//! Several pipelines write one database: in `devscriptor mcp` the watcher and
//! the query-time freshness check, and a scan may run in another process.
//!
//! A run reads the definitions it resolves against from the database, rather
//! than keeping the entities under a root between runs, which could be older
//! than the database: a call to a function another writer removed would
//! resolve to the removed entity's id, and with foreign keys on, that one edge
//! would fail the whole re-index. An edge whose end another writer removes
//! before it is written is skipped.

use std::path::Path;

use devscriptor_core::database::DatabaseConfig;
use devscriptor_core::pipeline::{CodebasePipeline, PipelineConfig};
use tempfile::TempDir;

fn pipeline(db: &Path) -> CodebasePipeline {
    CodebasePipeline::new(PipelineConfig {
        database: DatabaseConfig {
            path: db.to_string_lossy().into_owned(),
            ..DatabaseConfig::default()
        },
        incremental: true,
        include_extensions: vec!["py".to_string()],
        skip_embeddings: true,
        ..PipelineConfig::default()
    })
    .expect("pipeline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_to_a_function_another_writer_removed_is_not_linked() {
    let tree = TempDir::new().expect("tempdir for the tree");
    let root = dunce::canonicalize(tree.path()).expect("canonical root");
    let lib = root.join("lib.py");
    let user = root.join("user.py");
    std::fs::write(&lib, "def helper():\n    return 1\n").expect("write lib.py");
    std::fs::write(
        &user,
        "from lib import helper\n\n\ndef caller():\n    return helper()\n",
    )
    .expect("write user.py");
    let index = TempDir::new().expect("tempdir for the index");
    let db = index.path().join("index.db");

    let first = pipeline(&db);
    first.analyze_directory(&root).await.expect("scan");
    // The first pipeline re-indexes the caller while `helper` exists.
    std::fs::write(
        &user,
        "from lib import helper\n\n\ndef caller():\n    # once\n    return helper()\n",
    )
    .expect("edit user.py");
    first
        .reindex_paths(&root, std::slice::from_ref(&user))
        .await
        .expect("first re-index");

    // Another writer renames `helper`: its entity is deleted.
    std::fs::write(&lib, "def renamed_helper():\n    return 1\n").expect("rename");
    pipeline(&db)
        .reindex_paths(&root, std::slice::from_ref(&lib))
        .await
        .expect("the other writer's re-index");

    // The caller still calls `helper`, which is gone.
    std::fs::write(
        &user,
        "from lib import helper\n\n\ndef caller():\n    # twice\n    return helper()\n",
    )
    .expect("edit user.py again");
    first
        .reindex_paths(&root, std::slice::from_ref(&user))
        .await
        .expect("a call to a removed function fails nothing");

    let database = first.db();
    let callers = database
        .get_entities_by_exact_name("caller", 10)
        .await
        .expect("find caller");
    assert_eq!(callers.len(), 1, "caller is indexed once: {callers:?}");
    assert!(
        database
            .get_entities_by_exact_name("helper", 10)
            .await
            .expect("find helper")
            .is_empty(),
        "the renamed function is gone"
    );
    for edge in database
        .get_relationships(callers[0].id())
        .await
        .expect("caller's edges")
    {
        let ends = database
            .get_entity_names_by_ids(&[edge.source_id, edge.target_id])
            .await
            .expect("the edge's ends");
        assert_eq!(ends.len(), 2, "an edge to a removed entity: {edge:?}");
    }
}
