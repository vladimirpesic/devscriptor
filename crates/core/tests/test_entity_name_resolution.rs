//! Relationship endpoints must be resolvable to names: an edge's `target` (a
//! called function, a parent class, a referenced table) is named, not given as
//! a bare UUID with type "unknown". `get_entity_names_by_ids` is the batched
//! lookup that lets `get_relationships` name every endpoint.

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::parser::parse_content_sync;
use std::collections::HashMap;
use std::path::Path;

#[tokio::test]
async fn get_entity_names_by_ids_resolves_relationship_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::new(DatabaseConfig {
        path: tmp.path().join("names.db").to_string_lossy().to_string(),
        ..DatabaseConfig::default()
    })
    .expect("db");

    // `caller` calls `callee` → a Calls edge whose source is caller, target callee.
    let pf = parse_content_sync(
        "fn callee() -> u8 { 1 }\nfn caller() -> u8 { callee() }\n",
        "rust",
        Path::new("lib.rs"),
    )
    .expect("parse");
    db.batch_insert_parsed_files(&[pf.to_record()], None)
        .await
        .expect("insert");

    // Take the `caller` entity and its outgoing Calls edge.
    let caller = pf
        .entities
        .iter()
        .find(|e| e.name() == "caller")
        .expect("caller entity");
    let rels = db.get_relationships(caller.id()).await.expect("rels");
    let calls_edge = rels
        .iter()
        .find(|r| r.relationship_type.as_str() == "calls")
        .expect("a calls edge from caller");

    // The edge's target id is NOT the queried entity, so it must be resolved from
    // the DB. Batch-resolve both endpoints by id.
    let resolved: HashMap<_, _> = db
        .get_entity_names_by_ids(&[calls_edge.source_id, calls_edge.target_id])
        .await
        .expect("resolve names")
        .into_iter()
        .map(|header| {
            (
                header.id,
                (header.name, header.entity_type, header.file_path),
            )
        })
        .collect();

    let (src_name, _, _) = resolved
        .get(&calls_edge.source_id)
        .expect("source resolved");
    let (tgt_name, tgt_type, tgt_file) = resolved
        .get(&calls_edge.target_id)
        .expect("target resolved");
    assert_eq!(src_name, "caller");
    assert_eq!(
        tgt_name, "callee",
        "target must resolve to a name, not a UUID"
    );
    assert_eq!(
        tgt_type, "function",
        "target type must resolve, not be 'unknown'"
    );
    assert_eq!(tgt_file, "lib.rs", "the file the target is defined in");
}

#[tokio::test]
async fn get_entity_names_by_ids_handles_empty_and_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::new(DatabaseConfig {
        path: tmp.path().join("empty.db").to_string_lossy().to_string(),
        ..DatabaseConfig::default()
    })
    .expect("db");

    assert!(
        db.get_entity_names_by_ids(&[])
            .await
            .expect("empty")
            .is_empty()
    );
    // Unknown ids simply don't appear in the result (no error).
    let missing = uuid::Uuid::new_v4();
    assert!(
        db.get_entity_names_by_ids(&[missing])
            .await
            .expect("missing")
            .is_empty()
    );
}
